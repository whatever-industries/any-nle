//! Opt-in external FFmpeg 8.1 OP-Atom -> OP1a preparation; no original-file writes.
//! Plan construction is read-only. Execution and receipts keep their existing
//! validation order, exclusive writes and retention of partial outputs on failure.
mod backend;
mod execution;
mod planning;
mod mov;
mod mono;
mod receipts;
mod source;
mod terminal;
mod validation;

#[cfg(test)]
mod tests;

pub use validation::{normalize_source_clocks, validate_bin_audio};
pub(crate) use planning::{NativeMediaPreparation, PrepareIssue};
pub(crate) use mono::gain_paths;

use backend::Probe;
use graph::json::Value;
use prproj::convert::model::Fraction;
use source::Stamp;
use std::path::PathBuf;

type R<T> = Result<T, String>;

struct Input {
    path: PathBuf,
    aliases: Vec<String>,
    stamp: Stamp,
    probe: Probe,
    identity: Value,
    terminal: Option<terminal::Terminal>,
}
struct Job {
    inputs: Vec<usize>,
    clips: Vec<Value>,
    tc: Option<String>,
    rate: Fraction,
    mov: bool,
}
pub struct Plan {
    destination: PathBuf,
    inputs: Vec<Input>,
    jobs: Vec<Job>,
    mono: Vec<mono::MonoPcm>,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    versions: Vec<String>,
    estimate: u64,
    free: u64,
}
