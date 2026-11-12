//! Experimental standalone Resolve 21.2 archive reading and writing.
mod archive;
mod atom;
mod audio;
mod avid;
mod binary;
mod dissolve;
mod group;
pub mod linked;
mod media;
mod prepare;
mod premiere;
pub mod relink;
pub mod source;
pub mod turnover;
mod video;

pub use archive::{
    publish, publish_group, publish_linked, publish_linked_with_templates, publish_turnover,
    Published,
};
pub use atom::{verify_atom_binding, AtomBindingSpec};
pub use avid::{convert_avid_atom, convert_avid_ume, prepare_avid_turnover, AvidAtomConversion};
pub use group::{prepare_group, PreparedGroup};
pub use media::{
    verify_binding, verify_group_binding, BindingSpec, StreamSelector, VerifiedBinding,
};
pub use prepare::{prepare, PreparedResolve};
pub use premiere::{list_premiere_sequences, prepare_premiere_turnover};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const RATE: f64 = 24000.0 / 1001.0;
const DAY: i64 = 24 * 60 * 60 * 24;

fn seconds(frames: i64) -> f64 {
    frames as f64 / RATE
}

fn timecode(frames: i64) -> String {
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        frames / 86400,
        frames / 1440 % 60,
        frames / 24 % 60,
        frames % 24
    )
}
