//! Gzipped-XML document layer (port of `py_premiere/xml/`): arena DOM,
//! ElementTree-compatible parser, Premiere-style serializer, gzip framing,
//! formatting-preserving mutations and the `PremiereDocument` object table.

pub mod document;
pub mod gzip_io;
pub mod mutations;
pub mod parser;
mod name_tables;
mod path;
pub mod serializer;
pub mod tree;

pub use document::{parse_prproj, PremiereDocument, ReferenceIndex};
pub use gzip_io::{compress_prproj, decompress_prproj, GzipFraming};
pub use serializer::{serialize_document, serialize_element};
pub use tree::{Element, NodeId, Tree};
