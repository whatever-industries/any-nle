//! `prproj`: Adobe Premiere Pro project (`.prproj`) core for Any NLE.
//!
//! Rust port of py_premiere's document layer (the vendored fork in
//! `converter/vendor/py_premiere`) plus the converter's store-once payload
//! helpers and deterministic IDs. The Python writer is the oracle: everything
//! here reproduces its bytes exactly.
//!
//! Map from Python to Rust:
//!
//! | Python | Rust |
//! |---|---|
//! | `xml.etree.ElementTree.Element` | [`xml::Tree`] + [`xml::NodeId`] |
//! | `ET.fromstring(text)` | [`xml::Tree::parse_fragment`] |
//! | `xml/serializer.py` | [`xml::serializer`] |
//! | `xml/gzip_io.py` | [`xml::gzip_io`] |
//! | `xml/mutations.py` | [`xml::mutations`] |
//! | `xml/document.py` `PremiereDocument`, `ReferenceIndex`, `parse_prproj` | [`xml::PremiereDocument`], [`xml::ReferenceIndex`], [`xml::parse_prproj`] |
//! | `models/project.py` `Project._splice_fragment` | [`splice::splice_fragment`] |
//! | `avid2prproj/lumetri.py` `binary_hash`, `set_payload` | [`payload::binary_hash`], [`payload::set_payload`] |
//! | `avid2prproj/determinism.py` `seeded_ids`, `uuid.uuid4` | [`ids::SeededUuids`], [`ids::RandomUuids`], [`ids::UuidSource`] |
//! | `base64.b64encode/b64decode` | [`base64`] |

pub mod base64;
pub mod build;
pub mod caption;
pub mod cli;
pub mod convert;
pub mod error;
pub mod export;
pub mod hash;
pub mod ids;
pub mod model;
pub mod payload;
pub mod probe;
pub mod pynum;
pub mod pystr;
mod publication;
pub mod splice;
pub mod xml;

pub use error::{Error, Result};
pub use ids::{RandomUuids, SeededUuids, Uuid, UuidSource};
pub use xml::{NodeId, PremiereDocument, Tree};
