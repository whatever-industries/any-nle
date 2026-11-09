//! Avid bin (.avb) reader for the avid2prproj converter. std-only.
//!
//! The container and class decoders are a partial port of pyavb
//! (https://github.com/markreidvfx/pyavb, MIT, Copyright (c) 2022 Mark Reid) at
//! commit ff64e06b60077ecabe5db3fef239898b08363898; see NOTICE.

pub mod animatte;
pub mod bin_nav;
pub mod bin_writer;
pub mod color;
pub mod class_registry;
pub mod component_writer;
pub mod curve_writer;
pub mod descriptor_writer;
pub mod document;
pub mod dump;
pub mod experimental_writer;
pub mod extract;
pub mod file;
mod file_io;
pub mod graph;
pub mod id_stable_writer;
pub mod interpolation;
pub mod io;
pub mod inputs;
pub mod json;
pub mod misc_writer;
pub mod misc_extra_writer;
pub mod mobid;
pub mod model;
pub mod mxf;
pub mod objects;
pub mod param_eval;
pub mod parameter_registry;
pub mod property_registry;
pub mod payload_writer;
pub mod sequence_nav;
pub mod semantic_helpers;
pub mod primitives_extra;
pub mod semantic_extra;
pub mod source;
pub mod document_extra;
pub mod document_attributes_extra;
pub mod document_tracks_extra;
pub mod document_embedded_lists;
pub mod document_nested_lists;
pub mod trackgroup_writer;
pub mod transform;
pub mod writer_io;

pub use document::AvbDocument;
pub use file::{AvbFile, AvbIndex};
