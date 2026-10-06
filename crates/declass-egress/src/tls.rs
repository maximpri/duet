// SPDX-License-Identifier: GPL-3.0-or-later
//! The server name a TLS client asks for, read from its first record.
//!
//! The proxy does not look inside tunnels. It reads only the plaintext
//! ClientHello that opens one, so a tunnel opened to an allowed host cannot
//! carry a TLS session for another name served from the same address (a
//! shared CDN): the name the client asks the server for must be the one the
//! tunnel was allowed for.

/// Largest TLS record (2^14 bytes of payload plus room for expansion).
pub const MAX_RECORD: usize = 5 + 16_384 + 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hello {
    /// More bytes are needed to hold the first record.
    Incomplete,
    /// A ClientHello naming this server (lowercase, without a trailing dot).
    Name(String),
    /// Not a TLS ClientHello, or one without a server name.
    Unnamed,
}

/// Reads the first record in `buf`.
pub fn client_hello(buf: &[u8]) -> Hello {
    if buf.len() < 5 {
        return Hello::Incomplete;
    }
    // Handshake record, TLS 1.0 to 1.3 record versions.
    if buf[0] != 0x16 || buf[1] != 0x03 {
        return Hello::Unnamed;
    }
    let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if buf.len() < 5 + len {
        return Hello::Incomplete;
    }
    server_name(&buf[5..5 + len]).map_or(Hello::Unnamed, Hello::Name)
}

/// A cursor over bytes; every read is bounds-checked.
struct ByteReader<'a>(&'a [u8]);

impl<'a> ByteReader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|b| b[0] as usize)
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
    }
    fn u24(&mut self) -> Option<usize> {
        self.take(3)
            .map(|b| (b[0] as usize) << 16 | (b[1] as usize) << 8 | b[2] as usize)
    }
}

/// The `server_name` of a ClientHello handshake message (RFC 8446 4.1.2,
/// RFC 6066 3). The whole ClientHello must be in this record.
fn server_name(record: &[u8]) -> Option<String> {
    let mut c = ByteReader(record);
    if c.u8()? != 1 {
        return None; // not a ClientHello
    }
    let len = c.u24()?;
    let mut hello = ByteReader(c.take(len)?);
    hello.take(2 + 32)?; // legacy version, random
    let n = hello.u8()?;
    hello.take(n)?; // session id
    let n = hello.u16()?;
    hello.take(n)?; // cipher suites
    let n = hello.u8()?;
    hello.take(n)?; // compression methods
    let n = hello.u16()?;
    let mut exts = ByteReader(hello.take(n)?);
    while !exts.0.is_empty() {
        let kind = exts.u16()?;
        let n = exts.u16()?;
        let body = exts.take(n)?;
        if kind != 0 {
            continue;
        }
        let mut list = ByteReader(body);
        let n = list.u16()?;
        let mut names = ByteReader(list.take(n)?);
        while !names.0.is_empty() {
            let name_type = names.u8()?;
            let n = names.u16()?;
            let name = names.take(n)?;
            if name_type == 0 {
                let name = std::str::from_utf8(name).ok()?;
                return Some(name.trim_end_matches('.').to_ascii_lowercase());
            }
        }
        return None;
    }
    None
}

/// A minimal ClientHello record naming `name` (tests; `None`: no name).
#[doc(hidden)]
pub fn sample_hello(name: Option<&str>) -> Vec<u8> {
    let mut exts = Vec::new();
    if let Some(name) = name {
        let n = name.len();
        let mut sni = Vec::new();
        sni.extend_from_slice(&((n + 3) as u16).to_be_bytes());
        sni.push(0);
        sni.extend_from_slice(&(n as u16).to_be_bytes());
        sni.extend_from_slice(name.as_bytes());
        exts.extend_from_slice(&0u16.to_be_bytes());
        exts.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        exts.extend_from_slice(&sni);
    }
    // An unrelated extension (supported_versions) first, as real clients send.
    let mut all = vec![0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04];
    all.extend_from_slice(&exts);
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[7u8; 32]);
    body.push(0); // session id
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // one cipher suite
    body.extend_from_slice(&[0x01, 0x00]); // null compression
    body.extend_from_slice(&(all.len() as u16).to_be_bytes());
    body.extend_from_slice(&all);
    let mut hs = vec![0x01];
    hs.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    hs.extend_from_slice(&body);
    let mut record = vec![0x16, 0x03, 0x01];
    record.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    record.extend_from_slice(&hs);
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_name_is_read_from_the_first_record() {
        let r = sample_hello(Some("Registry.NPMJS.org."));
        assert_eq!(client_hello(&r), Hello::Name("registry.npmjs.org".into()));
        for cut in [0, 3, 5, r.len() - 1] {
            assert_eq!(client_hello(&r[..cut]), Hello::Incomplete, "{cut}");
        }
        assert_eq!(client_hello(&sample_hello(None)), Hello::Unnamed);
        assert_eq!(client_hello(b"GET / HTTP/1.1\r\n\r\n"), Hello::Unnamed);
    }

    #[test]
    fn truncated_or_lying_lengths_never_panic() {
        let r = sample_hello(Some("a.example"));
        for i in 5..r.len() {
            for v in [0u8, 0xff] {
                let mut bad = r.clone();
                bad[i] = v;
                let _ = client_hello(&bad);
            }
        }
    }
}
