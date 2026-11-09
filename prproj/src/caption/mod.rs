//! Captions: py_premiere's caption tracks, caption items and styled-text payloads.
//!
//! Port of `models/caption.py`, `models/caption_builder.py`, `data/caption_template.py` and
//! `parsers/caption.py`, plus the Project/Sequence entry points that use them
//! (`Project._import_caption`, `Sequence.create_caption_track`, the `DataTrackGroup` branch of
//! `parse_sequence`). The Python code's output is the spec; see the README section
//! "Captions" for the full map.
//!
//! | Python | Rust |
//! |---|---|
//! | `models/caption.py` module functions (`decode_caption_text`, `replace_payload_text`, `read_/write_style_*`, `read_/write_block_*`, `read_/write_font_family`, `read_/write_font_size`, `write_payload_element`) | [`flat`] |
//! | `models/caption.py` `Caption`, `CaptionTrack` | [`Project`](crate::model::Project) methods `caption_*` / `caption_track_*`, handles [`CaptionId`], [`CaptionTrackId`] ([`model`]) |
//! | `models/caption_builder.py` | [`builder`] |
//! | `data/caption_template.py` | [`template`] |
//! | `parsers/caption.py` | [`parse`] |
//! | `Project._import_caption` | [`Project::import_caption`](crate::model::Project::import_caption) |
//! | `Sequence.create_caption_track`, `.caption_tracks` | [`Project::create_caption_track`](crate::model::Project::create_caption_track), [`Project::caption_tracks`](crate::model::Project::caption_tracks) |
//!
//! Python's exception classes matter to callers of the payload functions (a corrupt payload
//! raises `ValueError` or `struct.error` depending on where it is cut), so this module reports
//! [`PyError`], which keeps the class beside Python's message. It converts to and from the
//! crate-wide [`crate::Error`].

use std::fmt;

use crate::error::Error;

pub mod builder;
mod digits;
mod entry;
pub mod flat;
pub mod model;
pub mod parse;
pub mod template;

pub use flat::Color;
pub use model::{Caption, CaptionData, CaptionFormat, CaptionId, CaptionStore, CaptionTrackData, CaptionTrackId};

/// A Python exception, by class, with Python's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PyError {
    /// `ValueError` (including `binascii.Error` and `json` errors).
    Value(String),
    /// `NotImplementedError`.
    NotImplemented(String),
    /// `struct.error`.
    Struct(String),
    /// `IndexError`.
    Index(String),
    /// `UnicodeDecodeError` (a `ValueError` subclass).
    Unicode(String),
    /// `OSError` family (file reads).
    Os(String),
}

impl PyError {
    /// The Python class name.
    pub fn class(&self) -> &'static str {
        match self {
            PyError::Value(_) => "ValueError",
            PyError::NotImplemented(_) => "NotImplementedError",
            PyError::Struct(_) => "error",
            PyError::Index(_) => "IndexError",
            PyError::Unicode(_) => "UnicodeDecodeError",
            PyError::Os(_) => "OSError",
        }
    }

    /// Python's message (`str(exception)`).
    pub fn message(&self) -> &str {
        match self {
            PyError::Value(m)
            | PyError::NotImplemented(m)
            | PyError::Struct(m)
            | PyError::Index(m)
            | PyError::Unicode(m)
            | PyError::Os(m) => m,
        }
    }
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PyError {}

impl From<Error> for PyError {
    fn from(e: Error) -> Self {
        match e {
            Error::Io(m) => PyError::Os(m),
            Error::NotImplemented(m) => PyError::NotImplemented(m),
            Error::Struct(m) => PyError::Struct(m),
            Error::Index(m) => PyError::Index(m),
            Error::UnicodeDecode(m) => PyError::Unicode(m),
            other => PyError::Value(other.to_string()),
        }
    }
}

impl From<PyError> for Error {
    fn from(e: PyError) -> Self {
        match e {
            PyError::Value(m) => Error::Value(m),
            PyError::NotImplemented(m) => Error::NotImplemented(m),
            PyError::Struct(m) => Error::Struct(m),
            PyError::Index(m) => Error::Index(m),
            PyError::Unicode(m) => Error::UnicodeDecode(m),
            PyError::Os(m) => Error::Io(m),
        }
    }
}

/// Result of the caption layer.
pub type PyResult<T> = std::result::Result<T, PyError>;

pub(crate) fn value_err<T>(message: impl Into<String>) -> PyResult<T> {
    Err(PyError::Value(message.into()))
}
