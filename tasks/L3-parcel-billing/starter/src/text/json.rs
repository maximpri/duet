//! A small JSON reader for JSON-lines feeds (RFC 8259 subset: no surrogate
//! pair validation; numbers are kept as their source text so decimals stay
//! exact).

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number exactly as written.
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(map) => map.get(key),
            _ => None,
        }
    }

    /// Follows a dotted path: `parcel.dims.l`.
    pub fn path(&self, path: &str) -> Option<&Value> {
        path.split('.').try_fold(self, |v, k| v.get(k))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Strings and numbers as text (feeds are inconsistent about quoting).
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::Number(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: &'static str,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut p = Parser { bytes: text.as_bytes(), pos: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.pos != p.bytes.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

const MAX_DEPTH: usize = 64;

impl Parser<'_> {
    fn err(&self, message: &'static str) -> ParseError {
        ParseError { offset: self.pos, message }
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.bytes.get(self.pos) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, b: u8) -> Result<(), ParseError> {
        if self.bytes.get(self.pos) == Some(&b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err("unexpected character"))
        }
    }

    fn literal(&mut self, word: &str, v: Value) -> Result<Value, ParseError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.bytes.get(self.pos) {
            None => Err(self.err("unexpected end")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.eat(b'{')?;
        let mut map = BTreeMap::new();
        self.ws();
        if self.bytes.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.eat(b':')?;
            self.ws();
            let v = self.value(depth + 1)?;
            map.insert(key, v);
            self.ws();
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.err("expected , or }")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.bytes.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.bytes.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.err("expected , or ]")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let digits = |p: &mut Self| {
            let s = p.pos;
            while let Some(b'0'..=b'9') = p.bytes.get(p.pos) {
                p.pos += 1;
            }
            p.pos > s
        };
        if !digits(self) {
            return Err(self.err("expected digits"));
        }
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if !digits(self) {
                return Err(self.err("expected fraction digits"));
            }
        }
        if let Some(b'e' | b'E') = self.bytes.get(self.pos) {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.bytes.get(self.pos) {
                self.pos += 1;
            }
            if !digits(self) {
                return Err(self.err("expected exponent digits"));
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| self.err("invalid utf-8"))?;
        Ok(Value::Number(text.to_owned()))
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.pos;
            while let Some(&b) = self.bytes.get(self.pos) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| self.err("invalid utf-8"))?);
            match self.bytes.get(self.pos) {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let c = match self.bytes.get(self.pos) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            let hex =
                                self.bytes.get(self.pos + 1..self.pos + 5).ok_or_else(|| self.err("short escape"))?;
                            let hex = std::str::from_utf8(hex).map_err(|_| self.err("bad escape"))?;
                            let code = u32::from_str_radix(hex, 16).map_err(|_| self.err("bad escape"))?;
                            self.pos += 4;
                            char::from_u32(code).unwrap_or('\u{fffd}')
                        }
                        _ => return Err(self.err("bad escape")),
                    };
                    self.pos += 1;
                    out.push(c);
                }
                _ => return Err(self.err("unterminated string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_and_paths() {
        let v = parse(r#"{"id":"X1","parcel":{"kg":2.50,"dims":[60,40,40]},"ok":true,"n":null}"#).unwrap();
        assert_eq!(v.get("id").and_then(Value::as_str), Some("X1"));
        assert_eq!(v.path("parcel.kg").and_then(Value::as_text), Some("2.50"));
        assert_eq!(v.path("parcel.dims").and_then(Value::as_array).map(<[Value]>::len), Some(3));
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)));
        assert!(v.get("n").unwrap().is_null());
        assert!(v.path("parcel.nope").is_none());
    }

    #[test]
    fn parses_escapes() {
        let v = parse(r#""a\"b\\c\né""#).unwrap();
        assert_eq!(v.as_str(), Some("a\"b\\c\né"));
    }

    #[test]
    fn keeps_numbers_exact() {
        assert_eq!(parse("-12.500e3").unwrap(), Value::Number("-12.500e3".into()));
    }

    #[test]
    fn rejects_malformed_json() {
        for bad in ["{", "[1,]", r#"{"a" 1}"#, "tru", "\"open", "1 2", "01.", "-"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn limits_nesting() {
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert_eq!(parse(&deep).unwrap_err().message, "nesting too deep");
    }
}
