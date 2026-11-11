//! Source-facing, owned Avid facts. These are not Premiere/model/1 values.
//!
//! A range is portable only when `range` is present. Compatibility payload fields
//! retain the existing extracted effects/gain/pan, but cannot establish a missing
//! source rate, offset, owner, or timing transform.
use crate::extract::{AvidMarker, MasterClip, SeqEvent, SequenceInfo, Skipped};
use crate::io::MobId;
use std::fmt;
use std::path::{Path, PathBuf};

pub const MAX_SOURCE_FACT_BYTES: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub num: i64,
    pub den: i64,
}
impl Rate {
    pub fn new(num: i64, den: i64) -> Option<Self> {
        if num <= 0 || den <= 0 {
            return None;
        }
        let (mut a, mut b) = (num, den);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Some(Self {
            num: num / a,
            den: den / a,
        })
    }
    /// Stored record/source span ratio for the two observed automatic adapters.
    pub fn automatic_adapter_span(source: Self, record: Self) -> Option<(i64, i64)> {
        match ((source.num, source.den), (record.num, record.den)) {
            ((24, 1), (24_000, 1001)) => Some((1000, 1001)),
            ((24_000, 1001), (24, 1)) => Some((1001, 1000)),
            _ => None,
        }
    }

    /// The existing observed Avid decimal-rate family; no arbitrary float rounding.
    pub fn from_avid(rate: f64) -> Option<Self> {
        if !rate.is_finite() || rate <= 0.0 || rate > 384_000.0 {
            return None;
        }
        if (rate - rate.round()).abs() < 1e-9 {
            return Self::new(rate.round() as i64, 1);
        }
        if rate >= 1000.0 {
            return None;
        }
        let base = (rate * 1.001).round();
        ((rate - base * 1000.0 / 1001.0).abs() < 1e-3)
            .then(|| Self::new(base as i64 * 1000, 1001))
            .flatten()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observed<T> {
    Known(T),
    Unresolved(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInput {
    pub path: PathBuf,
    /// SHA-256 of the exact input revision, verified by the caller.
    pub revision: String,
}
#[derive(Clone, Copy, Debug)]
pub struct SourceLimits {
    pub max_fact_bytes: usize,
}
impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            max_fact_bytes: MAX_SOURCE_FACT_BYTES,
        }
    }
}
#[derive(Debug)]
pub struct SourceReadError {
    pub path: PathBuf,
    pub message: String,
}
impl fmt::Display for SourceReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}
impl std::error::Error for SourceReadError {}

#[derive(Debug)]
pub struct SourceProject {
    pub masters: Vec<SourceMaster>,
    pub groups: Vec<SourceGroup>,
    pub sequences: Vec<SourceSequence>,
    pub skipped: Vec<Skipped>,
    pub duplicates: Vec<(String, usize)>,
    /// Accounted retained facts, not a process RSS or decoder-allocation measurement.
    pub accounted_bytes: usize,
}
#[derive(Debug)]
pub struct SourceMaster {
    pub id: String,
    pub master: MasterClip,
    pub rate: Option<Rate>,
    /// Same order as master.tracks. Absence is never promoted to measured zero.
    pub offsets: Vec<Observed<i64>>,
    /// Same order as master.markers; compatibility marker.frame is not itself
    /// evidence that an owner-local-to-master transform was established.
    pub marker_witnesses: Vec<MasterMarkerWitness>,
    /// Same order as master.tracks; nested FILE locators and raw descriptor facts.
    pub originals: Vec<Option<OriginalTrack>>,
    /// Picture master selected by a qualified traditional Freeze Frame owner.
    pub rendered_freeze: bool,
    /// Upstream precompute identity for a timeline-bound AudioSuite Gain master.
    pub rendered_gain: Option<crate::io::MobId>,
}
/// Saved original-file facts, independent of current path availability or codec admission.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalTrack {
    pub locators: Vec<String>,
    pub descriptor_class: String,
    pub descriptor_rate: f64,
    pub descriptor_length: i64,
    pub channels: Option<i64>,
    pub sample_rate: Option<f64>,
    pub stored_quantization_bits: Option<i64>,
    /// Referenced file-mob track label; not a decoder stream or channel ordinal.
    pub file_track: Option<i64>,
    /// Evidence for the observed automatic adapter's linked AAC origin.
    pub motion_audio_track_start: Option<i64>,
}
#[derive(Debug)]
pub struct MasterMarkerWitness {
    pub owner: String,
    pub kind: i64,
    pub track: i64,
    pub local_offset: Observed<i64>,
    pub local_rate: Option<Rate>,
    pub master_frame: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub placements: Vec<String>,
    /// None means the structural owner resolved to these placements. A resolved
    /// one-to-many owner does not imply that every destination can prepare it.
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteWitness {
    pub mob: MobId,
    pub track: i64,
    pub rate: Option<Rate>,
    pub source_offset: Observed<i64>,
    pub length: Observed<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualifiedRange {
    pub record_start: i64,
    pub length: i64,
    pub record_rate: Rate,
    pub source_in: i64,
    pub source_length: i64,
    pub source_rate: Rate,
    pub route: Vec<RouteWitness>,
}
#[derive(Debug)]
pub struct SourceEvent {
    pub id: String,
    pub owner: String,
    pub event: SeqEvent,
    pub range: Option<QualifiedRange>,
    pub reason: Option<String>,
    pub volume_automation: Option<VolumeAutomation>,
    pub picture_extension: Option<PictureExtension>,
    /// Full-span picture effects in input-to-output order, if the chain is resolved.
    pub picture_chain: Vec<PictureStage>,
    /// Saved phase for the bounded automatic adapter between 24 and 24000/1001.
    pub motion_adapter_phase: Option<i64>,
    /// Visible source frame offsets from the qualified range start for a saved Timewarp map.
    pub saved_timewarp: Option<Vec<i64>>,
}
#[derive(Clone, Copy, Debug)]
pub struct ResizeKey {
    pub frame: i64,
    pub position: [f64; 2],
    pub scale: [f64; 2],
    /// Source-space fractions cropped from left, top, right and bottom.
    pub crop: [f64; 4],
}
impl ResizeKey {
    pub fn has_crop(keys: [Self; 2]) -> bool {
        keys.iter().any(|key| key.crop != [0.0; 4])
    }
}
/// The saved trim cohort has aligned position and height keys, and independent width timing.
#[derive(Clone, Copy, Debug)]
pub struct LinearTrim {
    pub motion: [LinearTrimKey; 3],
    pub width: [(i64, f64); 3],
    pub fixed_width: bool,
    /// A shared, edit-relative clock for the four source-space crop edges.
    pub crop: Option<[(i64, [f64; 4]); 3]>,
}
#[derive(Clone, Copy, Debug)]
pub struct LinearTrimKey {
    pub frame: i64,
    pub position: [f64; 2],
    pub height: f64,
}
#[derive(Clone, Copy, Debug)]
pub enum PictureExtension {
    Flip { native_id: &'static str, horizontal: bool, vertical: bool },
    Resize([ResizeKey; 2]),
    ResizeCrop([ResizeKey; 2]),
    Warp([ResizeKey; 2]),
    WarpTrim(LinearTrim),
    ResizeTrim(LinearTrim),
    UnsupportedTransform(&'static str),
    Unsupported(&'static str),
}
#[derive(Debug)]
pub enum PictureStage {
    Static(std::sync::Arc<crate::transform::PictureEffect>),
    Extension(PictureExtension),
}
impl PictureExtension {
    pub fn replaces_static_transform(self) -> bool {
        self.requires_square_pixels() || matches!(self, Self::UnsupportedTransform(_))
    }

    pub fn requires_square_pixels(self) -> bool {
        matches!(self, Self::ResizeCrop(_) | Self::Warp(_) | Self::WarpTrim(_) | Self::ResizeTrim(_))
    }
}
#[derive(Clone, Copy, Debug)]
pub struct VolumePoint {
    pub frame_numerator: i64,
    pub frame_denominator: i64,
    pub gain_q29: i64,
}
#[derive(Debug)]
pub enum VolumeAutomation {
    Keys(Vec<VolumePoint>),
    Unsupported(&'static str),
}
#[derive(Debug)]
pub struct SourceTransition {
    pub id: String,
    pub kind: i64,
    pub track: i64,
    pub position: Observed<i64>,
    pub length: Observed<i64>,
    pub cutpoint: Observed<i64>,
    pub rate: Option<Rate>,
    pub effect: Option<String>,
    pub outgoing: Binding,
    pub incoming: Binding,
}
#[derive(Debug)]
pub struct SourceMarker {
    pub id: String,
    pub owner: String,
    pub kind: i64,
    pub track: i64,
    pub owner_kind: crate::extract::MarkerOwner,
    /// Marker.frame is the owner-local comp_offset, never guessed record time.
    pub marker: AvidMarker,
    pub local_offset: Observed<i64>,
    pub local_rate: Option<Rate>,
    pub record_frame: Option<i64>,
    pub binding: Binding,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceIssueKind {
    Route,
    RouteTiming,
}
#[derive(Debug)]
pub struct SourceIssue {
    pub owner: String,
    pub reason: String,
    pub issue_kind: SourceIssueKind,
    pub route: Vec<RouteWitness>,
    pub record_start: Observed<i64>,
    pub record_rate: Option<Rate>,
    pub kind: i64,
    pub track: i64,
}
#[derive(Debug)]
pub struct SourceSequence {
    pub id: String,
    /// Common sequence metadata/diagnostics. Events, markers and transitions are
    /// moved to the explicitly qualified source collections below (not duplicated).
    pub info: SequenceInfo,
    pub rate: Option<Rate>,
    pub clock: Option<SequenceClock>,
    /// Native input-channel count for each sequence-local `event.combined` group.
    /// Includes unresolved/unsupported inputs that emitted no qualified event.
    /// An absent group witness cannot establish a complete atomic combiner.
    pub combiner_channels: Vec<usize>,
    pub events: Vec<SourceEvent>,
    pub transitions: Vec<SourceTransition>,
    pub markers: Vec<SourceMarker>,
    pub unresolved: Vec<SourceIssue>,
    pub texts: Vec<SourceText>,
    pub group_uses: Vec<SourceGroupUse>,
}

/// Absolute frame count from the qualified direct primary timecode track.
/// Sequence-local edit and marker positions do not include this origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceClock {
    pub count: u64,
    pub rate: Rate,
    pub display_base: u16,
    pub track_ordinal: usize,
}

/// Native group definitions retain every lane's alternatives independently.
/// Audio ordinals are not video camera ordinals (AVB-0006/0018/0019).
#[derive(Debug)]
pub struct SourceGroup {
    pub id: String,
    pub bin: usize,
    pub mob: Option<MobId>,
    pub name: String,
    pub rate: Option<Rate>,
    pub stored_rate: Option<f64>,
    pub length: Option<i64>,
    pub lanes: Vec<SourceGroupLane>,
    pub marks: Vec<(String, Observed<Option<i64>>)>,
    pub markers: Vec<AvidMarker>,
    pub reason: Option<String>,
}
#[derive(Debug)]
pub struct SourceGroupLane {
    /// Stored flag only; does not establish audio-follow-video.
    pub is_ganged: Option<bool>,
    pub kind: Option<i64>,
    pub label: Option<i64>,
    pub rate: Option<Rate>,
    pub length: Option<i64>,
    pub selected: Option<i64>,
    pub alternatives: Vec<SourceGroupAlternative>,
    pub reason: Option<String>,
}
#[derive(Debug)]
pub struct SourceGroupAlternative {
    pub endpoint: GroupEndpoint,
    pub pieces: Vec<SourceGroupPiece>,
    pub reason: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupEndpoint {
    pub class: [u8; 4],
    pub kind: Option<i64>,
    pub rate: Option<Rate>,
    pub mob: Option<MobId>,
    pub track: Option<i64>,
    pub start: Option<i64>,
    pub length: Option<i64>,
}
#[derive(Debug)]
pub enum SourceGroupPiece {
    Gap { start: i64, length: i64 },
    Media { start: i64, length: i64, master: MobId, track: i64, source_in: i64, rate: Rate },
}
#[derive(Debug)]
pub struct SourceGroupUse {
    pub is_ganged: Option<bool>,
    pub id: String,
    pub kind: i64,
    pub track: i64,
    pub position: Option<i64>,
    pub length: Option<i64>,
    pub rate: Option<Rate>,
    pub selected: Option<i64>,
    pub alternatives: Vec<GroupEndpoint>,
    /// Actual structural descendants, never a first timestamp match.
    pub binding: Binding,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind { SubCap, TitlerPlus }
#[derive(Debug)]
pub struct SourceText {
    pub id: String,
    pub kind: TextKind,
    pub track: i64,
    pub content: Option<String>,
    pub record_start: Option<i64>,
    pub length: Option<i64>,
    pub rate: Option<Rate>,
    pub reason: Option<String>,
    /// Selected stored text properties with exact list/owner-role witnesses. These
    /// are native-only values, not resolved render styles or neutral geometry.
    pub native_properties: Vec<NativeTextProperty>,
    pub native_properties_reason: Option<String>,
}
#[derive(Debug)]
pub struct NativeTextProperty {
    pub scope: String,
    pub list: usize,
    pub name: String,
    pub value: String,
}

impl SourceSequence {
    /// Empty metadata envelope for synthetic/differential consumers. Real reads
    /// use the bounded extraction entry points and retain original rate witnesses.
    pub fn new(id: String, name: String, bin: usize, rate: Option<Rate>, length: i64) -> Self {
        let edit_rate = rate.map_or(0.0, |r| r.num as f64 / r.den as f64);
        Self {
            id,
            info: SequenceInfo::source_metadata(name, bin, edit_rate, length),
            rate,
            clock: None,
            combiner_channels: Vec::new(),
            events: Vec::new(),
            transitions: Vec::new(),
            markers: Vec::new(),
            unresolved: Vec::new(),
            texts: Vec::new(),
            group_uses: Vec::new(),
        }
    }
}

pub fn extract_source_paths(
    inputs: &[SourceInput],
    limits: SourceLimits,
) -> Result<SourceProject, SourceReadError> {
    crate::extract::project::extract_source_paths(inputs, limits)
}

/// Called before each bin is decoded, on the calling thread. Returning an error cancels further reads.
pub fn extract_source_paths_observed(inputs:&[SourceInput],limits:SourceLimits,before:&mut dyn FnMut(usize,&Path)->Result<(),String>)->Result<SourceProject,SourceReadError>{crate::extract::project::extract_source_paths_observed(inputs,limits,before)}

pub(crate) fn failure(path: &Path, message: impl Into<String>) -> SourceReadError {
    SourceReadError {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// Synthetic/differential entry point for callers that already own decoded bins.
pub fn extract_source_files(
    inputs: &[SourceInput],
    files: &[&crate::file::AvbFile],
    limits: SourceLimits,
) -> Result<SourceProject, SourceReadError> {
    crate::extract::project::extract_source_files(inputs, files, limits)
}

#[cfg(test)]
mod progress_tests {
 use super::*;
 #[test] fn cancellation_observer_runs_before_a_bin_is_opened(){let input=SourceInput{path:PathBuf::from("missing-synthetic-bin.avb"),revision:"0".repeat(64)};let mut calls=0;let err=extract_source_paths_observed(&[input],SourceLimits::default(),&mut |n,p|{calls+=1;assert_eq!(n,0);assert_eq!(p,Path::new("missing-synthetic-bin.avb"));Err("cancelled before decode".into())}).unwrap_err();assert!(err.message.contains("cancelled before decode"));assert_eq!(calls,1);}
}
