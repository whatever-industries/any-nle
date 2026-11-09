//! Export a parsed project to interchange formats (port of `py_premiere/export`).
//!
//! A one-way layer: it reads the model and writes a different file format, so there is no
//! `.prproj` byte-fidelity contract here; the contract is byte-identity with the Python
//! exporter's output.

pub mod fcpxml;

pub use fcpxml::{export_fcpxml, export_fcpxml_bytes};
