// SPDX-License-Identifier: GPL-3.0-or-later
//! The route from a bubblewrap sandbox to the host's egress proxy.
//!
//! Under bubblewrap a command has a network namespace of its own: its own
//! loopback and nothing of the host's, so its servers work and no host
//! service is reachable. To reach the egress proxy it is started under a
//! helper ([`main`]; `declass __sandbox-bridge`) that listens on 127.0.0.1 inside
//! that namespace, points the proxy variables at the port ([`proxy_env`]) and
//! runs the command. Every connection it accepts is joined to one end of a new
//! socket pair, and the other end is handed to the host over the channel the
//! helper has as its standard input (`SCM_RIGHTS`); the host serves it as a
//! proxy client ([`Incoming`]).
//!
//! Socket pairs and handing descriptors over are the only Unix-socket
//! operations involved, so the seccomp filter that refuses creating `AF_UNIX`
//! sockets stays on: the command still cannot connect to a socket file it can
//! see. The channel leads to the proxy only, which decides every connection;
//! a command holding it could do no more than connect to the helper's port.

use crate::{ProxyRoute, proxy_env};
use std::ffi::OsString;
use std::io::{IoSlice, IoSliceMut};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

/// Where bubblewrap mounts the helper program inside the sandbox.
pub const HELPER_PATH: &str = "/run/declass-bridge";

/// A channel for one command: the route to put in its [`crate::Spec`] and
/// the host's end, which yields the command's connections. Needs a Tokio
/// runtime.
pub fn channel(helper: Vec<OsString>) -> std::io::Result<(ProxyRoute, Incoming)> {
    let (host, sandbox) = UnixStream::pair()?;
    host.set_nonblocking(true)?;
    let route = ProxyRoute::Bridge {
        channel: Arc::new(OwnedFd::from(sandbox)),
        helper,
    };
    let incoming = Incoming {
        fd: tokio::io::unix::AsyncFd::new(OwnedFd::from(host))?,
    };
    Ok((route, incoming))
}

/// The host's end of a channel.
pub struct Incoming {
    fd: tokio::io::unix::AsyncFd<OwnedFd>,
}

impl Incoming {
    /// The next connection the command made; `None` once the helper (and
    /// everything that could hold its end) is gone.
    pub async fn next(&self) -> Option<std::io::Result<tokio::net::UnixStream>> {
        loop {
            let mut ready = match self.fd.readable().await {
                Ok(r) => r,
                Err(e) => return Some(Err(e)),
            };
            match ready.try_io(|fd| receive(fd.get_ref())) {
                Ok(Ok(Some(stream))) => {
                    return Some(
                        stream
                            .set_nonblocking(true)
                            .and_then(|()| tokio::net::UnixStream::from_std(stream)),
                    );
                }
                Ok(Ok(None)) => return None,
                Ok(Err(e)) => return Some(Err(e)),
                Err(_would_block) => continue,
            }
        }
    }
}

/// One descriptor from the channel (`None` at its end).
fn receive(channel: &OwnedFd) -> std::io::Result<Option<UnixStream>> {
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
    let mut space = [0u8; rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let mut byte = [0u8; 1];
    #[cfg(target_os = "linux")]
    let flags = RecvFlags::CMSG_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let flags = RecvFlags::empty();
    let got = rustix::net::recvmsg(
        channel,
        &mut [IoSliceMut::new(&mut byte)],
        &mut control,
        flags,
    )?;
    let mut received = None;
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(fds) = message {
            for fd in fds {
                received.get_or_insert(fd);
            }
        }
    }
    match received {
        Some(fd) => {
            #[cfg(not(target_os = "linux"))]
            rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
            Ok(Some(UnixStream::from(fd)))
        }
        None if got.bytes == 0 => Ok(None),
        // A byte without a descriptor: nothing to serve.
        None => Err(std::io::Error::other("a message without a connection")),
    }
}

/// Hands `stream` to the host.
fn send(channel: &UnixStream, stream: &UnixStream) -> std::io::Result<()> {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
    let mut space = [0u8; rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    let fds = [stream.as_fd()];
    if !control.push(SendAncillaryMessage::ScmRights(&fds)) {
        return Err(std::io::Error::other("no room for the descriptor"));
    }
    rustix::net::sendmsg(
        channel,
        &[IoSlice::new(b"c")],
        &mut control,
        SendFlags::empty(),
    )?;
    Ok(())
}

/// The helper, inside the sandbox: `args` is the command and its arguments;
/// the standard input is the channel. Returns the command's exit code (128
/// plus the signal that ended it), or 125 when the bridge cannot start and
/// 127 when the command cannot.
pub fn main(args: Vec<OsString>) -> i32 {
    match run(args) {
        Ok(code) => code,
        Err((code, e)) => {
            eprintln!("declass sandbox bridge: {e}");
            code
        }
    }
}

fn run(args: Vec<OsString>) -> Result<i32, (i32, String)> {
    use std::os::unix::process::ExitStatusExt;
    let setup = |e: std::io::Error| (125, e.to_string());
    let (program, rest) = args
        .split_first()
        .ok_or((125, "no command given".to_owned()))?;
    // Our own copy, not inherited by the command (whose input is empty).
    let channel =
        rustix::io::fcntl_dupfd_cloexec(std::io::stdin(), 3).map_err(|e| setup(e.into()))?;
    let channel = UnixStream::from(channel);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(setup)?;
    let port = listener.local_addr().map_err(setup)?.port();
    let mut child = std::process::Command::new(program)
        .args(rest)
        .envs(proxy_env(port))
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| (127, format!("{}: {e}", program.to_string_lossy())))?;
    std::thread::spawn(move || forward(&listener, &channel));
    let status = child.wait().map_err(setup)?;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

/// Accepts the command's connections and hands each to the host.
fn forward(listener: &TcpListener, channel: &UnixStream) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let Ok((ours, theirs)) = UnixStream::pair() else {
            continue;
        };
        if send(channel, &theirs).is_err() {
            // The host is gone: nothing more can be forwarded.
            return;
        }
        drop(theirs);
        std::thread::spawn(move || pump(conn, ours));
    }
}

/// Copies both ways until each side has finished sending.
fn pump(tcp: TcpStream, unix: UnixStream) {
    let (Ok(tcp_out), Ok(unix_in)) = (tcp.try_clone(), unix.try_clone()) else {
        return;
    };
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut &tcp_out, &mut &unix_in);
        let _ = unix_in.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut &unix, &mut &tcp);
    let _ = tcp.shutdown(Shutdown::Write);
    let _ = up.join();
}
