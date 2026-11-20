//! Versioned shared job contract for the native wizard and command line.
pub(crate) const PREFLIGHT_REPORT_SCHEMA: &str = "anynle.preflight-report/1";

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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub allow_offline_media: bool,
    /// Accepted for existing jobs. Linked conversions now check unused clips by default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub skip_unused_bin_items: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub prefer_source_media: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub attach_avid_proxies: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_rewrap_dir: Option<PathBuf>,
    pub new_media: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrap_needed: Option<PathBuf>,
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
            allow_offline_media: false,
            skip_unused_bin_items: false,
            prefer_source_media: false,
            attach_avid_proxies: false,
            proxy_rewrap_dir: None,
            new_media: None,
            rewrap_needed: None,
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
    pub rewrap_sources: Vec<Vec<PathBuf>>,
    pub estimated_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub camera_originals: Vec<CameraOriginalLink>,
    pub media_linking: Option<MediaLinking>,
    pub notes: Vec<ReportNote>,
    /// Display names keyed by the diagnostic's stable source object ID.
    pub subjects: std::collections::HashMap<String, String>,
    pub cli_equivalent: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MediaLinking {
    pub direct_files: usize,
    pub premiere_files: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rewrap_available: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rewrapped_clips: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conversion_pending: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraOriginalLink {
    pub name: String,
    pub picture: PathBuf,
    /// Saved mono track and one-based physical channel in the camera file.
    pub audio_channels: Vec<(i64, i64)>,
    pub proxy: Option<PathBuf>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerKind {
    ProjectSource,
    ProjectOutput,
    AvidMedia,
    RewrapMissingMedia,
    RewrapMissingOriginal,
    LegacyMxfPackage,
    OriginalLocator,
    OriginalMedia,
    NewMedia,
    ProxyMedia,
    MediaTools,
    Effects,
    Rewrap,
    GroupClipMode,
    SourceContent,
    SequenceTiming,
    Retry,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewBlocker {
    pub kind: BlockerKind,
    pub message: String,
    pub affected: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rewrap_sources: Vec<Vec<PathBuf>>,
}
impl ReviewBlocker {
    pub fn new(kind: BlockerKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), affected: Vec::new(), rewrap_sources: Vec::new() }
    }
    pub(crate) fn cli_summary(self) -> String {
        if self.affected.is_empty() {
            return self.message;
        }
        match self.kind {
            BlockerKind::SequenceTiming => {
                let mut summary = self.affected[..self.affected.len().min(8)].join("; ");
                if self.affected.len() > 8 {
                    summary.push_str("; More edits are affected in the selected bins.");
                }
                format!("{}: {summary}", self.message)
            }
            BlockerKind::Rewrap => format!("{}\n{}", self.message, self.affected.join("\n")),
            BlockerKind::OriginalLocator | BlockerKind::LegacyMxfPackage => {
                format!("{}: {}", self.message, self.affected.join("; "))
            }
            _ => self.message,
        }
    }
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
#[cfg(target_os = "macos")]
pub(crate) use inventory::selected_paths;

mod engine;
pub(crate) use engine::{blocked_preflight_report, sequence_timing_blocker};
pub use engine::{
    execute, load, load_encoded, prepare, prepare_for_review, save_blocked_preflight_report, PreparedJob,
};
