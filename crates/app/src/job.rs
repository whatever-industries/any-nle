//! Versioned shared job contract for the native wizard and command line.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathMapping {
    pub saved_prefix: PathBuf,
    pub actual_root: PathBuf,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipMode {
    #[default]
    RewrapNative,
    Grouped,
    Merged,
    Separate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectMode {
    Convert,
    Approximate,
    Skip,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectChoice {
    pub key: String,
    pub mode: EffectMode,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JobSpec {
    pub schema: String,
    pub bins: Vec<PathBuf>,
    pub project_folder: Option<PathBuf>,
    pub entire_project: bool,
    pub include_trash_attic: bool,
    pub include_dependencies: bool,
    pub output: PathBuf,
    pub media_roots: Vec<PathBuf>,
    pub original_roots: Vec<PathBuf>,
    pub path_mappings: Vec<PathMapping>,
    pub project_name: Option<String>,
    pub mode: ClipMode,
    pub new_media: Option<PathBuf>,
    pub policy_json: String,
    pub effects: Vec<EffectChoice>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
}
impl Default for JobSpec {
    fn default() -> Self {
        Self {
            schema: "anynle.job/1".into(),
            bins: vec![],
            project_folder: None,
            entire_project: false,
            include_trash_attic: false,
            include_dependencies: true,
            output: PathBuf::new(),
            media_roots: vec![],
            original_roots: vec![],
            path_mappings: vec![],
            project_name: None,
            mode: ClipMode::RewrapNative,
            new_media: None,
            policy_json: String::new(),
            effects: vec![],
            ffmpeg: None,
            ffprobe: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct BinEntry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub clips: usize,
    pub sequences: usize,
    pub dependency: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub bin_revisions: Vec<(PathBuf, String)>,
    pub bins: Vec<BinEntry>,
    pub found_media: usize,
    pub total_media: usize,
    pub native_media: usize,
    pub ume_originals: usize,
    pub effects: Vec<EffectCapability>,
    pub marker_colors: Vec<String>,
    pub clip_colors: Vec<String>,
    pub notes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct EffectCapability {
    pub key: String,
    pub label: String,
    pub count: usize,
    pub modes: Vec<EffectMode>,
    pub explanation: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub inventory: Inventory,
    pub estimated_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub notes: Vec<ReportNote>,
    pub cli_equivalent: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportNote {
    pub severity: String,
    pub object: String,
    pub message: String,
    pub why: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub stage: String,
    pub current_bin: Option<PathBuf>,
    pub completed: usize,
    pub total: usize,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Complete,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    pub status: CompletionStatus,
    pub output: Option<PathBuf>,
    pub retained_paths: Vec<PathBuf>,
    pub receipt: Option<PathBuf>,
    pub report: Vec<ReportNote>,
    pub message: String,
}
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("cancelled".into())
        } else {
            Ok(())
        }
    }
}

mod inventory;
pub use inventory::inventory;

mod engine;
pub use engine::{execute, load, load_encoded, prepare, PreparedJob};
