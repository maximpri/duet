//! Error type shared by every module.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Reading a file failed.
    Io {
        path: String,
        source: std::io::Error,
    },
    /// A line of a feed could not be parsed.
    Feed {
        carrier: String,
        line: usize,
        message: String,
    },
    /// A configuration file is malformed.
    Config {
        file: String,
        line: usize,
        message: String,
    },
    /// A data file (customers, rates, remote areas) is malformed.
    Data {
        file: String,
        line: usize,
        message: String,
    },
    /// A value could not be parsed (timestamp, weight, money...).
    Value {
        kind: &'static str,
        text: String,
    },
    UnknownCarrier(String),
    Usage(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn value(kind: &'static str, text: impl Into<String>) -> Self {
        Error::Value { kind, text: text.into() }
    }

    pub fn feed(carrier: &str, line: usize, message: impl Into<String>) -> Self {
        Error::Feed { carrier: carrier.to_owned(), line, message: message.into() }
    }

    pub fn data(file: &str, line: usize, message: impl Into<String>) -> Self {
        Error::Data { file: file.to_owned(), line, message: message.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{path}: {source}"),
            Error::Feed { carrier, line, message } => write!(f, "{carrier} feed line {line}: {message}"),
            Error::Config { file, line, message } => write!(f, "{file}:{line}: {message}"),
            Error::Data { file, line, message } => write!(f, "{file}:{line}: {message}"),
            Error::Value { kind, text } => write!(f, "invalid {kind}: {text:?}"),
            Error::UnknownCarrier(code) => write!(f, "unknown carrier {code}"),
            Error::Usage(msg) => write!(f, "usage: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Reads a file to a string, attaching the path to any error.
pub fn read_to_string(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io { path: path.display().to_string(), source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_feed_errors_with_carrier_and_line() {
        let e = Error::feed("NRP", 7, "bad weight");
        assert_eq!(e.to_string(), "NRP feed line 7: bad weight");
    }

    #[test]
    fn displays_value_errors() {
        assert_eq!(Error::value("weight", "x").to_string(), "invalid weight: \"x\"");
    }

    #[test]
    fn io_errors_name_the_path() {
        let e = read_to_string(std::path::Path::new("/definitely/not/here.txt")).unwrap_err();
        assert!(e.to_string().starts_with("/definitely/not/here.txt:"));
    }
}
