//! The crate's error type.
//!
//! Python's py_premiere raises `ValueError` for almost everything; the variants
//! here only say which layer refused, the message carries the detail (worded
//! like the Python message where one exists). [`Error::py_class`] names the
//! Python exception class.

use std::fmt;

/// Every failure the document core can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not well-formed XML (Python: `ET.ParseError`, re-raised as
    /// `ValueError("not a Premiere project file: ...")` by `from_bytes`).
    Parse(String),
    /// The XML parsed but re-serializing it does not give the same bytes
    /// (Python: the `from_bytes` self-check).
    RoundTrip(String),
    /// Bad gzip framing or deflate stream.
    Gzip(String),
    /// The tree cannot be written in Premiere's style (mixed content).
    Serialize(String),
    /// Any other refusal (unknown ObjectRef, duplicate ObjectID, bad base64,
    /// missing anchor child...). Python's generic `ValueError`.
    Value(String),
    /// File system error (Python `OSError`).
    Io(String),
    /// Python `TypeError`: a value of the wrong kind where Rust's types cannot rule it out
    /// (e.g. a boolean given to a scalar parameter).
    Type(String),
    /// Python `KeyError`: a name lookup that found nothing (the message is the key's repr).
    Key(String),
    /// Python `NotImplementedError`: a shape py_premiere does not support yet.
    NotImplemented(String),
    /// Python `AttributeError` (renaming the root item).
    Attribute(String),
    /// Python `FileExistsError`.
    FileExists(String),
    /// Python `FileNotFoundError`.
    FileNotFound(String),
    /// Python `UnicodeDecodeError` (a `ValueError` subclass), with CPython's message.
    UnicodeDecode(String),
    /// Python `struct.error` (a packed value out of its format's range).
    Struct(String),
    /// Python `OverflowError`.
    Overflow(String),
    /// Python `IndexError`.
    Index(String),
    /// Python `ZeroDivisionError`.
    ZeroDivision(String),
}

impl Error {
    /// The Python exception class py_premiere raises for this error.
    pub fn py_class(&self) -> &'static str {
        match self {
            Error::Parse(_) | Error::RoundTrip(_) | Error::Gzip(_) | Error::Serialize(_) | Error::Value(_) => {
                "ValueError"
            }
            Error::Io(_) => "OSError",
            Error::Type(_) => "TypeError",
            Error::Key(_) => "KeyError",
            Error::NotImplemented(_) => "NotImplementedError",
            Error::Attribute(_) => "AttributeError",
            Error::FileExists(_) => "FileExistsError",
            Error::FileNotFound(_) => "FileNotFoundError",
            Error::UnicodeDecode(_) => "UnicodeDecodeError",
            Error::Struct(_) => "error",
            Error::Overflow(_) => "OverflowError",
            Error::Index(_) => "IndexError",
            Error::ZeroDivision(_) => "ZeroDivisionError",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(m) => write!(f, "not a Premiere project file: {m}"),
            Error::RoundTrip(m) => write!(f, "cannot faithfully round-trip this file; {m}"),
            Error::Gzip(m)
            | Error::Serialize(m)
            | Error::Value(m)
            | Error::Io(m)
            | Error::Type(m)
            | Error::Key(m)
            | Error::NotImplemented(m)
            | Error::Attribute(m)
            | Error::FileExists(m)
            | Error::FileNotFound(m)
            | Error::UnicodeDecode(m)
            | Error::Struct(m)
            | Error::Overflow(m)
            | Error::Index(m)
            | Error::ZeroDivision(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

/// Shorthand result type.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn value<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}

pub(crate) fn type_err<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Type(message.into()))
}

pub(crate) fn not_implemented<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::NotImplemented(message.into()))
}
