//! Source-facing, owned Avid facts. These are not Premiere/model/1 values.
//!
//! A range is portable only when `range` is present. Compatibility payload fields
//! retain the existing extracted effects/gain/pan, but cannot establish a missing
//! source rate, offset, owner, or timing transform.
use crate::extract::{AvidMarker, MasterClip, SeqEvent, SequenceInfo, Skipped};
use crate::io::MobId;
use roxmltree::{Document, Node as XmlNode, ParsingOptions};
use std::fmt;
use std::path::{Path, PathBuf};

/// Project-wide source capture budget, independent of the per-bin decoder.
pub const MAX_SOURCE_FACT_BYTES: usize = if usize::BITS >= 64 {
    (8_u64 << 30) as usize
} else {
    256 << 20
};
pub(crate) const MAX_INPUT_TRANSFORM_BYTES: usize = 16 * 1024;

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
    pub imported_stills: Vec<SourceImportedStill>,
    pub subclips: Vec<SourceSubclip>,
    pub groups: Vec<SourceGroup>,
    pub sequences: Vec<SourceSequence>,
    pub skipped: Vec<Skipped>,
    pub duplicates: Vec<(String, usize)>,
    /// Peak accounted source allocations during extraction, in bytes.
    pub accounted_bytes: usize,
}
/// Saved picture master and original PNG locator, independent of managed codec support.
#[derive(Debug)]
pub struct SourceImportedStill {
    pub id: String,
    pub bin: usize,
    pub master: crate::io::MobId,
    pub track: i64,
    pub file_mob: crate::io::MobId,
    pub file_track: i64,
    pub rate: Option<Rate>,
    pub master_length: Option<i64>,
    /// Offset and span of the master's SourceClip. These describe the saved edit.
    pub file_in: Option<i64>,
    pub source_length: Option<i64>,
    pub import_aspect: Option<i64>,
    pub color_level: Option<i64>,
    pub import: MatteImport,
}
/// One saved usage-2 bin occurrence. Range attributes and component offsets
/// are native observations on their respective owners.
#[derive(Debug)]
pub struct SourceSubclip {
    pub id: String,
    pub bin: usize,
    pub mob: Option<crate::io::MobId>,
    pub name: String,
    pub rate: Option<Rate>,
    pub backing_length: Option<i64>,
    pub start: Observed<i64>,
    pub end: Observed<i64>,
    pub tracks: Vec<SourceSubclipTrack>,
}

#[derive(Debug)]
pub struct SourceSubclipTrack {
    pub label: Option<i64>,
    pub component_class: Option<[u8; 4]>,
    pub kind: Option<i64>,
    pub rate: Option<Rate>,
    pub length: Option<i64>,
    pub target_mob: Option<crate::io::MobId>,
    pub target_track: Option<i64>,
    pub target_offset: Option<i64>,
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
    /// Saved direct file-to-physical timecode routes, in master track order.
    pub file_clocks: Vec<Option<SavedFileClock>>,
    /// Saved routing for a master backed by a managed multi-descriptor file.
    pub managed_multistream: Option<ManagedMultistream>,
    /// A saved import route. The original WAV bytes have not been verified.
    pub imported_audio: Option<Box<SavedPcmImport>>,
    /// A verified saved master topology with two PCM siblings followed by
    /// one MPEG audio frame. Separate physical coverage is required.
    pub pcm_prefix: Option<SourcePcmPrefix>,
    /// Observed on the directly referenced picture file mob, in master track order.
    pub input_transforms: Vec<SourceInputTransform>,
    /// Picture master selected by a qualified traditional Freeze Frame owner.
    pub rendered_freeze: bool,
    /// Upstream precompute identity for a timeline-bound AudioSuite Gain master.
    pub rendered_gain: Option<crate::io::MobId>,
}
#[derive(Clone, Copy, Debug)]
pub struct SourcePcmPrefix {
    pub frames: i64,
    pub tail_package: MobId,
}
#[derive(Debug)]
pub struct ManagedMultistream {
    pub file_package: MobId,
    pub tracks: Vec<ManagedMultistreamTrack>,
    pub descriptors: Vec<ManagedMultistreamDescriptor>,
}
#[derive(Debug)]
pub struct ManagedMultistreamTrack {
    pub kind: i64,
    pub master_label: i64,
    pub file_label: i64,
    pub physical_track_number: Option<u32>,
    pub master_offset: Option<i64>,
    pub file_source_package: Option<MobId>,
    pub file_source_track: Option<i64>,
    pub file_source_offset: Option<i64>,
    pub file_source_length: Option<i64>,
    pub file_source_rate: Option<Rate>,
}
#[derive(Debug)]
pub struct ManagedMultistreamDescriptor {
    pub class: [u8; 4],
    pub length: Option<i64>,
    pub edit_rate: Option<Rate>,
    pub sample_rate: Option<Rate>,
    pub bits: Option<i64>,
    pub channels: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub frame_layout: Option<i64>,
    pub display_aspect: Option<(i64, i64)>,
    pub resolution_id: Option<i64>,
    pub compression: Option<[u8; 4]>,
}
#[derive(Debug)]
pub struct SavedPcmImport {
    pub locator: String,
    pub physical_package: crate::io::MobId,
    pub start_frame: i64,
    pub frames: i64,
    /// Saved ordinal within the physical package. File channel identity needs
    /// independent verification against the eight managed files.
    pub lanes: [SavedPcmImportLane; 8],
}
#[derive(Debug, Clone, Copy)]
pub struct SavedPcmImportLane {
    pub master_track: i64,
    pub file_package: crate::io::MobId,
    pub file_track: i64,
    pub physical_track: i64,
}
#[derive(Debug)]
pub struct SourceInputTransform {
    pub track: i64,
    pub file_mob: crate::io::MobId,
    pub value: Observed<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedFileClock {
    pub package: crate::io::MobId,
    pub rate: Rate,
    pub count: i64,
    pub offset: i64,
    pub base: i64,
    pub drop: bool,
    pub file_track: i64,
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
    pub display_raster: Option<(i64, i64)>,
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
    pub pan_automation: Option<PanAutomation>,
    /// Subclip PVOLs on the source route with unset unity gain and saved center pan.
    pub subclip_center_pan_count: u8,
    /// Explicit level stored on one same-rate subclip wrapper, independent of timeline gain.
    pub subclip_gain: Option<SourceSubclipGain>,
    /// An infrequent picture payload kept out of the event array.
    pub picture_extension: Option<Box<PictureExtension>>,
    /// Full-span picture effects in input-to-output order, if the chain is resolved.
    pub picture_chain: Vec<PictureStage>,
    /// Saved phase for the bounded automatic adapter between 24 and 24000/1001.
    pub motion_adapter_phase: Option<i64>,
    /// The enclosing PVOL has an unset pan flag and saves the center position.
    pub motion_adapter_center_pan: bool,
    /// Visible source frame offsets from the qualified range start for a saved Timewarp map.
    pub saved_timewarp: Option<Vec<i64>>,
    /// A single AudioSuite Gain was omitted from this otherwise qualified dry route.
    pub dry_gain_bypass: bool,
    /// Raw controls and optional cache binding for a legacy EQMB.
    pub equalizer: Option<Box<SourceEqualizer>>,
    /// A saved single AudioSuite effect and the verified AVB binding to its render.
    pub audiosuite: Option<Box<SourceAudioSuiteRender>>,
    /// Direct dry master and in-point proved by the saved Gain binding.
    pub gain_dry_input: Option<Box<(MobId, i64)>>,
    /// Signed playback ratio established by the saved constant Speed and Position controls.
    pub constant_speed: Option<(i64, i64)>,
    /// A top-level, neutral Collapse over one full-span picture edit.
    pub plain_submaster: bool,
}
#[derive(Debug)]
pub struct SourceAudioSuiteRender {
    pub file: MobId,
    pub origin: MobId,
    pub start: i64,
    pub frames: i64,
    pub padding_samples: i64,
    /// The saved subclip input and its local channel and in-point, if present.
    pub input_subclip: Option<(MobId, i64, i64)>,
    pub input_master: MobId,
    pub input_track: i64,
    pub input_file: MobId,
    pub file_track: i64,
    pub input_start: i64,
    pub name: String,
    pub manufacturer_id: u32,
    pub product_id: u32,
    pub plugin_id: u32,
    pub chunk_id: u32,
    pub chunk_version: i64,
    pub chunk: Vec<u8>,
}
#[derive(Debug, PartialEq)]
pub struct SourceEqualizer {
    pub enabled: bool,
    pub filter_name: String,
    pub bands: Vec<SourceEqualizerBand>,
    pub render: Option<SourceEqualizerRender>,
    pub pitch: Option<Box<SourcePitchCache>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceEqualizerRender {
    pub file: MobId,
    pub origin: MobId,
    pub start: i64,
    pub frames: i64,
    pub dry_master: MobId,
    pub dry_master_track: i64,
    pub dry_file: MobId,
    pub dry_track: i64,
    pub dry_start: i64,
    pub dry_subclip: Option<MobId>,
    pub dry_wrapped_master: bool,
    pub input_gain: Option<SourceSubclipGain>,
}
#[derive(Debug, PartialEq)]
pub struct SourceEqualizerBand {
    pub filter_type: i64,
    pub frequency: i64,
    pub gain: i64,
    pub q: i64,
    pub enabled: bool,
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
pub struct WarpRollKey {
    pub frame: i64,
    pub x: f64,
    pub width: f64,
    pub rotation: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpRollInterpolation {
    Shelf,
    Linear,
    Bezier,
    Spline,
}
#[derive(Clone, Copy, Debug)]
pub enum PictureExtension {
    Flip { native_id: &'static str, horizontal: bool, vertical: bool },
    Resize([ResizeKey; 2]),
    ResizeCrop([ResizeKey; 2]),
    Warp([ResizeKey; 2]),
    WarpRoll { keys: [WarpRollKey; 3], interpolation: WarpRollInterpolation },
    WarpTrim(LinearTrim),
    ResizeTrim(LinearTrim),
    UnsupportedTransform(&'static str),
    Unsupported(&'static str),
}
#[derive(Debug)]
pub enum PictureStage {
    Static(std::sync::Arc<crate::transform::PictureEffect>),
    Extension(PictureExtension),
    SpectraMatte { rgb: [u8; 3], progress_end: i64 },
}
impl PictureExtension {
    pub fn replaces_static_transform(&self) -> bool {
        self.requires_square_pixels() || matches!(self, Self::UnsupportedTransform(_))
    }

    pub fn requires_square_pixels(&self) -> bool {
        matches!(self, Self::ResizeCrop(_) | Self::Warp(_) | Self::WarpRoll { .. } | Self::WarpTrim(_) | Self::ResizeTrim(_))
    }
}
#[derive(Clone, Copy, Debug)]
pub struct VolumePoint {
    pub frame_numerator: i64,
    pub frame_denominator: i64,
    pub gain_q29: i64,
}
#[derive(Clone, Copy, Debug)]
pub struct PanPoint {
    pub frame_numerator: i64,
    pub frame_denominator: i64,
    pub raw_pan: i64,
}
#[derive(Debug)]
pub enum Automation<P> {
    Keys(Vec<P>),
    Unsupported(&'static str),
}
pub type VolumeAutomation = Automation<VolumePoint>;
pub type PanAutomation = Automation<PanPoint>;
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
    pub audio_curve_attribute: Option<i64>,
    pub audio_curve_parameter: Option<i64>,
    /// Direct outgoing filler span on a saved dissolve starting at the cut.
    pub outgoing_filler: Option<i64>,
    /// Direct incoming filler span at the sequence rate.
    pub incoming_filler: Option<i64>,
    /// Saved adjacency used when a Gain endpoint has no portable edit.
    pub gain_gap_edges: [Option<GainGapEdge>; 2],
    pub outgoing: Binding,
    pub incoming: Binding,
}
#[derive(Debug)]
pub enum GainGapEdge {
    Gain(String),
    Filler,
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
    /// An unsupported effect with a verified interval on its record lane.
    UntranslatedEffect { effect: &'static str, length: i64 },
    /// Untranslated Timewarp with exact record bounds. The optional route identifies
    /// its saved render master, file and origin, in that order.
    Timewarp { length: i64 },
    MatteKey { length: Option<i64> },
    ColorAdapter { length: Option<i64> },
    LayeredSubmaster { length: Option<i64>, input_count: usize },
    DisabledClip { length: i64, backing_class: [u8; 4], plain_mono: bool, plain_stereo: bool, neutral_picture: Option<MobId> },
}
/// Saved subclip PVOL scalars. The pan field is center in this profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSubclipGain {
    pub level_q29: i32,
    pub pan_set: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatteInput {
    pub mob: crate::io::MobId,
    pub track: i64,
    pub start: i64,
    pub length: i64,
    pub rate: Rate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatteBackground {
    LowerTrack,
    Explicit,
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatteInputs {
    pub background: MatteBackground,
    pub fill: Option<MatteInput>,
    pub matte: Option<MatteInput>,
}

/// Saved import settings on the master shared by the fill and matte.
#[derive(Debug)]
pub struct MatteImport {
    pub ignore_alpha: Option<bool>,
    pub invert_alpha: Option<bool>,
    pub locators: Vec<String>,
}

#[derive(Debug)]
pub struct SubmasterInput {
    pub track: Option<i64>,
    pub component_class: Option<[u8; 4]>,
    /// Direct component or the sole nonempty entry inside an input sequence.
    pub segment_class: Option<[u8; 4]>,
    pub segment_effect: Option<String>,
    /// A direct picture reference with its position inside this saved input.
    pub picture_source: Option<SubmasterPictureInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmasterPictureInput {
    pub mob: crate::io::MobId,
    pub track: i64,
    pub source_start: i64,
    pub local_start: i64,
    pub length: i64,
    pub local_length: i64,
    pub rate: Rate,
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
    pub matte_inputs: Option<Box<MatteInputs>>,
    pub matte_import: Option<MatteImport>,
    pub color_adapter: Option<Box<SourceColorAdapter>>,
    pub submaster_inputs: Vec<SubmasterInput>,
    /// Saved control/route match only. Media identity and destination composition
    /// still require verification; a match does not remove the source loss.
    pub picture_profile: Option<SourcePictureProfile>,
    /// Saved Timewarp controls when the occurrence could not become a portable map.
    pub timewarp: Option<Box<SourceTimewarpControls>>,
    /// Verified sampling on a plain source clip that selects a retimed picture subclip.
    pub nested_picture_motion: Option<Box<SourceSubclipMotion>>,
}

/// Saved inner AudioSuite cache. The enclosing EQ render supplies playback.
#[derive(Debug, PartialEq, Eq)]
pub struct SourcePitchCache {
    pub master: MobId,
    pub file: MobId,
    pub origin: MobId,
    pub samples: i64,
    pub pitch_chunk: [u8; 148],
}

#[derive(Debug)]
pub struct SourceTimewarpControls {
    pub span_ratio: Option<(i64, i64)>,
    pub phase_offset: Option<i64>,
    pub offset_adjust: Option<f64>,
    pub render_mode: Option<i64>,
    pub ignore_render_setting: Option<i64>,
    pub graph_mode: Option<i64>,
    pub input_format: Option<i64>,
    pub output_format: Option<i64>,
    pub new_source_calculation: Option<bool>,
    pub input: Option<TimewarpInput>,
    pub precompute: Option<TimewarpInput>,
    pub precompute_present: bool,
    /// Owner of a verified saved-cache reference for the enclosing Resize.
    pub resize_render_owner: Option<String>,
    pub curves: Vec<SourceTimewarpCurve>,
}

#[derive(Debug)]
pub struct SourceSavedTimewarp {
    pub owner: String,
    pub record: i64,
    pub track: i64,
    pub length: i64,
    pub controls: Box<SourceTimewarpControls>,
}

impl SourceTimewarpControls {
    /// Names read back in Media Composer for explicit interpolation overrides.
    pub fn explicit_fluid_motion(&self) -> Option<&'static str> {
        match (self.ignore_render_setting, self.render_mode) {
            (Some(1), Some(6)) => Some("FluidMotion"),
            (Some(1), Some(7)) => Some("FluidMotion Draft"),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct TimewarpInput {
    pub mob: MobId,
    pub track: Option<i64>,
    pub start: Option<i64>,
    pub length: Option<i64>,
    pub rate: Option<Rate>,
}

#[derive(Debug)]
pub struct SourceTimewarpCurve {
    pub parameter: crate::io::Uuid,
    pub enabled: Option<bool>,
    pub interpolation: Option<i64>,
    pub extrapolation: Option<i64>,
    pub fields: Option<i64>,
    pub point_count: usize,
    pub points: Option<SourceTimewarpPoints>,
}

#[derive(Debug)]
pub enum SourceTimewarpPoints {
    HalfFrameSamples { first: i64, values: Vec<f64> },
    Keys(Vec<SourceTimewarpKey>),
}

pub type SourceTimewarpKey = SourceParamKey;

/// Saved numeric Avid curve point in its owner's native time coordinates.
#[derive(Debug)]
pub struct SourceParamKey {
    pub time: (i64, i64),
    pub timescale: i64,
    pub value: f64,
    pub handles: [Option<f64>; 4],
    pub anchor: Option<f64>,
    pub flags: Option<i64>,
}
/// An edit-side payload and its checked relationship to the direct picture source.
#[derive(Debug)]
pub struct SourceColorAdapter {
    pub master: MobId,
    pub master_track: i64,
    pub source_in: i64,
    /// Master coordinates after one checked picture subclip hop.
    pub subclip_master: Option<Box<ColorAdapterMaster>>,
    /// Saved controls for a nested motion effect whose rendered cadence is unqualified.
    pub subclip_motion: Option<Box<SourceSubclipMotion>>,
    pub picture_file: Observed<MobId>,
    /// Same picture package, with the same saved color steps in a different order.
    pub reordered_picture_file: Option<MobId>,
    pub transform: String,
    pub neutral_wrapper: bool,
    /// Neutral linked wrapper with a direct picture span on the same saved clock.
    pub plain_same_rate_linked_input: bool,
    /// Saved edit-side clip color on the effect wrapper, separate from the transform.
    pub wrapper_clip_color: Option<[u16; 3]>,
    pub managed_picture_span: Option<(i64, i64)>,
    pub centered_still_fit: bool,
}

/// The two saved, standalone 10-bit range transforms observed in Avid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceColorRange {
    VideoToFull,
    FullToVideo,
}

impl SourceColorRange {
    pub fn from_saved_xml(text: &str) -> Option<Self> {
        if !text.contains("<LinearLut") {
            return None;
        }
        let doc = color_transform_document(text)?;
        let lut = standalone_color_step(&doc, "LinearLut")?;
        let mut fields = [None; 4];
        let mut inverted = false;
        for field in color_content(lut) {
            if !bare_element(field) {
                return None;
            }
            let index = match field.tag_name().name() {
                "Name" => 0,
                "BitDepth" => 1,
                "Black" => 2,
                "White" => 3,
                "Inverted" if !inverted && field.first_child().is_none() => {
                    inverted = true;
                    continue;
                }
                _ => return None,
            };
            if fields[index].replace(color_scalar(text, field)?).is_some() {
                return None;
            }
        }
        let [Some(name), Some(depth), Some(black), Some(white)] = fields else {
            return None;
        };
        // Avid treats whitespace inside a number differently from whitespace between tags.
        if [
            (depth, Self::BIT_DEPTH),
            (black, Self::VIDEO_BLACK),
            (white, Self::VIDEO_WHITE),
        ]
        .into_iter()
        .any(|(value, expected)| {
            !value.bytes().all(|byte| byte.is_ascii_digit()) || value.parse::<i64>() != Ok(expected)
        }) {
            return None;
        }
        match (name, inverted) {
            ("Levels scaling (video levels to full range)", false) => Some(Self::VideoToFull),
            ("Levels scaling (full range to video levels)", true) => Some(Self::FullToVideo),
            _ => None,
        }
    }

    pub const BIT_DEPTH: i64 = 10;
    pub const VIDEO_BLACK: i64 = 64;
    pub const VIDEO_WHITE: i64 = 940;
}

/// A saved single-step 10-bit ASC CDL with clamped slope, offset, and power
/// and unit saturation. The original XML stays on the source owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceClampedCdl {
    pub slope: [f64; 3],
    pub offset: [f64; 3],
    pub power: [f64; 3],
}

impl SourceClampedCdl {
    pub fn from_saved_xml(text: &str) -> Option<Self> {
        if !text.contains("<ColorDecision") {
            return None;
        }
        let doc = color_transform_document(text)?;
        let decision = standalone_color_step(&doc, "ColorDecision")?;
        let [depth, sop, black, white, clamp] =
            exactly_named_color_fields(decision, ["BitDepth", "SOPNode", "Black", "White", "Clamp"])?;
        if color_scalar(text, depth)? != "10"
            || color_scalar(text, black)? != "64"
            || color_scalar(text, white)? != "940"
            || color_scalar(text, clamp)? != "true"
        {
            return None;
        }
        let [slope, offset, power] = exactly_named_color_fields(sop, ["Slope", "Offset", "Power"])?;
        let power = color_scalar(text, power)?;
        let power = cdl_triplet(power).or_else(|| Some([cdl_decimal(power)?; 3]))?;
        if power.iter().any(|&value| value <= 0.0) {
            return None;
        }
        Some(Self {
            slope: cdl_triplet(color_scalar(text, slope)?)?,
            offset: cdl_triplet(color_scalar(text, offset)?)?,
            power,
        })
    }

    pub fn is_affine(self) -> bool {
        self.power == [1.0; 3]
    }
}

fn cdl_triplet(text: &str) -> Option<[f64; 3]> {
    // The saved triplets use ASCII decimal text and a trailing space.
    let mut values = text.trim_end_matches(' ').split(' ');
    let mut triplet = [0.0; 3];
    for value in &mut triplet {
        *value = cdl_decimal(values.next()?)?;
    }
    values.next().is_none().then_some(triplet)
}

fn cdl_decimal(text: &str) -> Option<f64> {
    if !text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'-' | b'.')) {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn color_transform_document(text: &str) -> Option<Document<'_>> {
    if text.len() > MAX_INPUT_TRANSFORM_BYTES {
        return None;
    }
    let options = ParsingOptions {
        nodes_limit: 64,
        ..Default::default()
    };
    Document::parse_with_options(text, options).ok()
}

fn standalone_color_step<'a, 'input>(
    doc: &'a Document<'input>,
    name: &str,
) -> Option<XmlNode<'a, 'input>> {
    let list = only_element(doc.root(), "ColorTransformationList")?;
    only_element(only_element(list, "ColorTransformation")?, name)
}

fn exactly_named_color_fields<'a, 'input, const N: usize>(
    parent: XmlNode<'a, 'input>,
    names: [&str; N],
) -> Option<[XmlNode<'a, 'input>; N]> {
    let mut content = color_content(parent);
    let mut fields = [parent; N];
    for (field, name) in fields.iter_mut().zip(names) {
        let node = content.next()?;
        if !bare_element(node) || node.tag_name().name() != name {
            return None;
        }
        *field = node;
    }
    content.next().is_none().then_some(fields)
}

fn color_scalar<'input>(text: &'input str, node: XmlNode<'_, 'input>) -> Option<&'input str> {
    let mut content = node.children();
    let scalar = content.next().filter(XmlNode::is_text)?;
    let raw = text.get(scalar.range())?;
    (scalar.text() == Some(raw) && content.next().is_none()).then_some(raw)
}

fn color_content<'a, 'input: 'a>(
    node: XmlNode<'a, 'input>,
) -> impl Iterator<Item = XmlNode<'a, 'input>> {
    node.children().filter(|child| {
        !child.is_comment()
            && !(child.is_text() && child.text().is_some_and(|s| s.trim_ascii().is_empty()))
    })
}

fn bare_element(node: XmlNode<'_, '_>) -> bool {
    node.is_element() && node.tag_name().namespace().is_none() && node.attributes().len() == 0
}

fn only_element<'a, 'input: 'a>(parent: XmlNode<'a, 'input>, name: &str) -> Option<XmlNode<'a, 'input>> {
    let mut content = color_content(parent);
    let node = content.next()?;
    (bare_element(node) && node.tag_name().name() == name && content.next().is_none()).then_some(node)
}

/// Match exact saved payloads or two representations of the same qualified range transform.
pub fn saved_color_transforms_match(left: &str, right: &str) -> bool {
    left == right || SourceColorRange::from_saved_xml(left).is_some_and(|range| {
        SourceColorRange::from_saved_xml(right) == Some(range)
    })
}

/// The single 3D .cube descriptor saved on the qualified Avid effect wrapper.
pub fn is_canonical_external_cube(xml: &str) -> bool {
    color_list_body(xml).and_then(external_cube).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceColorStack<'a> {
    pub range: SourceColorRange,
    pub cube_name: &'a str,
    pub cube_file: &'a str,
    pub cube_first: bool,
}

const VIDEO_TO_FULL_STEP: &str = concat!(
    "<LinearLut><Name>Levels scaling (video levels to full range)</Name>",
    "<BitDepth>10</BitDepth><Black>64</Black><White>940</White></LinearLut>",
);

impl<'a> SourceColorStack<'a> {
    /// A manual 10-bit video-to-full adjustment and a 3D cube with native scaling disabled.
    pub fn from_saved_xml(xml: &'a str) -> Option<Self> {
        let (first, second) = color_list_body(xml)?.split_once(
            "</ColorTransformation><ColorTransformation>",
        )?;
        let (cube, cube_first) = match (first, second) {
            (VIDEO_TO_FULL_STEP, cube) => (cube, false),
            (cube, VIDEO_TO_FULL_STEP) => (cube, true),
            _ => return None,
        };
        let (cube_name, cube_file) = external_cube(cube)?;
        Some(Self { range: SourceColorRange::VideoToFull, cube_name, cube_file, cube_first })
    }

    pub fn is_reordered(self, other: Self) -> bool {
        self.cube_first != other.cube_first
            && Self { cube_first: other.cube_first, ..self } == other
    }
}

fn color_list_body(xml: &str) -> Option<&str> {
    xml.strip_prefix("<ColorTransformationList><ColorTransformation>")?
        .strip_suffix("</ColorTransformation></ColorTransformationList>")
}

fn external_cube(body: &str) -> Option<(&str, &str)> {
    let body = body.strip_prefix("<ExternalLut><Name>")?
        .strip_suffix("</LutFileName><LutDimension>3D</LutDimension><InhibitScaling/></ExternalLut>")?;
    let (name, file) = body.split_once("</Name><LutFileName>")?;
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b" ._()-".contains(&b))
        && file.len() <= 128 && file.strip_suffix(".cube").is_some_and(|stem| !stem.is_empty())
        && file.bytes().all(|b| b.is_ascii_alphanumeric() || b" ._()-".contains(&b)))
        .then_some((name, file))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorAdapterMaster {
    pub mob: MobId,
    pub track: i64,
    pub source_in: i64,
}
#[derive(Debug)]
pub struct SourceSubclipMotion {
    pub input_mob: MobId,
    pub input_track: i64,
    pub input_start: i64,
    pub input_length: i64,
    pub length: i64,
    pub advanced: bool,
    pub span_ratio: (i64, i64),
    pub speed: f64,
    pub speed_key_frame: (i64, i64),
    pub position: [f64; 2],
    pub phase_offset: i64,
    pub offset_adjust: f64,
    pub render_mode: i64,
    pub ignore_render_setting: i64,
    pub new_source_calculation: bool,
    pub graph_mode: Option<i64>,
    pub sampled_input_frames: Option<SourceSubclipSampling>,
    /// When the saved input is itself a subclip, the backing master and its
    /// first selected frame. The saved input ID and cadence stay separate.
    pub sampled_picture_master: Option<ColorAdapterMaster>,
    /// Saved package for a direct, zero-offset picture route over the sampled range.
    pub sampled_picture_file: Option<MobId>,
}

#[derive(Debug, Clone, Copy)]
pub struct SourceSubclipSampling {
    /// First and last selected frames in the saved input's clock.
    pub first: i64,
    pub last: i64,
    /// Exact source frames per record frame.
    pub ratio: (i64, i64),
    /// Fractional part of the unrounded first position, in units of 1/ratio.1.
    pub phase: i64,
}

impl SourceSubclipSampling {
    /// Exclusive bound of the raw trim, rounded up to the next whole frame.
    pub fn source_end(self, length: i64) -> Option<i64> {
        let (num, den) = self.ratio;
        let span = i128::from(self.phase) + i128::from(length) * i128::from(num);
        let frames = span.checked_add(i128::from(den.checked_sub(1)?))?.checked_div(i128::from(den))?;
        self.first.checked_add(i64::try_from(frames).ok()?)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePictureProfile {
    ImportedPngMatte,
    EmptyColorAdapter,
    PlainLayeredSubmaster,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceWarpInput {
    Filler,
    Picture(SubmasterPictureInput),
    Unresolved,
}

/// File-order parameter IDs with saved-control names established in AVB-0009.
pub const WARP_COMPOSITING_PARAMETERS: &[(&str, &str)] = &[
    ("f281568d7e84d511935a50f857c10000", "foreground_group_gate"),
    ("f381568d7e84d511935a50f857c10000", "key_mode_code"),
    ("f681568d7e84d511935a50f857c10000", "foreground_opacity_percent"),
    ("0682568d7e84d511935a50f857c10000", "background_color_selector"),
    ("0782568d7e84d511935a50f857c10000", "background_hue"),
    ("0882568d7e84d511935a50f857c10000", "background_saturation"),
    ("0982568d7e84d511935a50f857c10000", "background_luminance"),
    ("f0481979d5ff594184d9f6a6e9b6d3bb", "background_limit_to_video_levels"),
    ("f481568d7e84d511935a50f857c10000", "invert_key"),
    ("f581568d7e84d511935a50f857c10000", "swap_sources"),
    ("1a82568d7e84d511935a50f857c10000", "show_alpha"),
    ("1b82568d7e84d511935a50f857c10000", "reverse_animation"),
    ("fe81568d7e84d511935a50f857c10000", "key_threshold"),
    ("ff81568d7e84d511935a50f857c10000", "key_softness"),
    ("0082568d7e84d511935a50f857c10000", "key_lum_range_enabled"),
    ("0182568d7e84d511935a50f857c10000", "key_range"),
    ("d081568d7e84d511935a50f857c10000", "border_group_gate"),
    ("d181568d7e84d511935a50f857c10000", "border_style_code"),
    ("d281568d7e84d511935a50f857c10000", "border_width"),
    ("d381568d7e84d511935a50f857c10000", "border_softness"),
    ("d481568d7e84d511935a50f857c10000", "border_hue"),
    ("d581568d7e84d511935a50f857c10000", "border_saturation"),
    ("d681568d7e84d511935a50f857c10000", "border_luminance"),
    ("e281568d7e84d511935a50f857c10000", "shadow_group_gate"),
    ("e381568d7e84d511935a50f857c10000", "shadow_x"),
    ("e481568d7e84d511935a50f857c10000", "shadow_y"),
    ("e681568d7e84d511935a50f857c10000", "shadow_opacity_percent"),
    ("df81568d7e84d511935a50f857c10000", "defocus_group_gate"),
    ("1c82568d7e84d511935a50f857c10000", "defocus_mode_code"),
    ("e081568d7e84d511935a50f857c10000", "defocus_internal_horizontal"),
    ("e181568d7e84d511935a50f857c10000", "defocus_internal_vertical"),
    ("1d82568d7e84d511935a50f857c10000", "defocus_fg_only_horizontal"),
    ("1e82568d7e84d511935a50f857c10000", "defocus_fg_only_vertical"),
];

#[derive(Clone, Copy, Debug)]
pub enum SourceWarpControlValue {
    Int(i64),
    Float(f64),
    Unresolved,
}

#[derive(Debug)]
pub enum SourceWarpControlCurve {
    Absent,
    Empty,
    Keyframed(Box<SourceWarpCurve>),
    Unresolved,
}

#[derive(Debug)]
pub struct SourceWarpCurve {
    pub value_type: Option<i64>,
    pub interpolation: Option<i64>,
    pub extrapolation: Option<i64>,
    pub fields: Option<i64>,
    pub point_count: usize,
    pub keys: Option<Vec<SourceParamKey>>,
}

/// The saved PRIT scalar is separate from both its enable flag and its curve.
#[derive(Debug)]
pub struct SourceWarpControl {
    pub parameter_id: &'static str,
    pub name: &'static str,
    pub value_type: Option<i64>,
    pub value: SourceWarpControlValue,
    pub enabled: Option<bool>,
    pub curve: SourceWarpControlCurve,
}

/// A saved track's filler proxy, independently of its component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFillerProxy {
    Absent,
    Relative { scope: i64, track: i64 },
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePictureBlendKind {
    Warp,
    Superimpose,
    PictureInPicture,
}

impl SourcePictureBlendKind {
    pub fn effect_id(self) -> &'static str {
        match self {
            Self::Warp => "EFF2_SBLEND",
            Self::Superimpose => "EFF2_BLEND_SUPER",
            Self::PictureInPicture => "EFF2_BLEND_PIP",
        }
    }
}

/// Saved input slots and controls on a picture blend occurrence, before evaluation.
#[derive(Debug)]
pub struct SourcePictureBlendInputs {
    pub kind: SourcePictureBlendKind,
    pub owner: String,
    pub track: i64,
    pub record_start: Option<i64>,
    pub record_length: Option<i64>,
    pub effect_rate: Option<Rate>,
    pub effect_length: i64,
    pub swap_sources: Option<bool>,
    pub inputs: [SourceWarpInput; 2],
    pub filler_proxies: [SourceFillerProxy; 2],
    pub controls: Vec<SourceWarpControl>,
}

#[derive(Debug)]
pub struct SourceSequence {
    pub id: String,
    /// Common sequence metadata/diagnostics. Events, markers and transitions are
    /// moved to the explicitly qualified source collections below (not duplicated).
    pub info: SequenceInfo,
    pub rate: Option<Rate>,
    pub clock: Option<SequenceClock>,
    pub track_names: Vec<SourceTrackName>,
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
    pub saved_timewarps: Vec<SourceSavedTimewarp>,
    pub picture_blend_inputs: Vec<SourcePictureBlendInputs>,
}

#[derive(Debug)]
pub struct SourceTrackName {
    pub kind: i64,
    pub label: i64,
    pub name: String,
    pub ambiguous: bool,
    /// Direct picture or an explicitly saved standard mono audio lane.
    pub plain_layout: bool,
    /// Direct filler only, with a standard mono format on audio tracks.
    pub plain_empty: bool,
}

/// Absolute frame count from the qualified direct primary timecode track.
/// Sequence-local edit and marker positions do not include this origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceClock {
    pub count: u64,
    pub rate: Rate,
    pub display_base: u16,
    pub drop: bool,
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
    pub markers: Vec<(AvidMarker, Observed<i64>)>,
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
    /// The exact selector is the full-span picture input to one qualified Warp.
    pub warp_picture_input: bool,
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
            track_names: Vec::new(),
            combiner_channels: Vec::new(),
            events: Vec::new(),
            transitions: Vec::new(),
            markers: Vec::new(),
            unresolved: Vec::new(),
            texts: Vec::new(),
            group_uses: Vec::new(),
            saved_timewarps: Vec::new(),
            picture_blend_inputs: Vec::new(),
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

/// Inspect the same decoded bin used for source capture while it is resident.
pub fn extract_source_paths_inspected(
    inputs: &[SourceInput], limits: SourceLimits,
    before: &mut dyn FnMut(usize, &Path) -> Result<(), String>,
    inspect: &mut dyn FnMut(&crate::file::AvbFile) -> Result<(), String>,
) -> Result<SourceProject, SourceReadError> {
    crate::extract::project::extract_source_paths_inspected(inputs, limits, before, inspect)
}

/// Reader findings from bins already decoded. Later source and media checks can
/// resolve or add findings when the full selection is known.
pub enum BinReadProgress<'a> {
    Reading { index: usize, path: &'a Path, findings: usize },
    Finished { total: usize, findings: usize },
}

/// Observe progress on the calling thread and inspect each bin during its only decode.
pub fn extract_source_paths_with_progress(
    inputs: &[SourceInput], limits: SourceLimits,
    observe: &mut dyn FnMut(BinReadProgress<'_>) -> Result<(), String>,
    inspect: &mut dyn FnMut(&crate::file::AvbFile) -> Result<(), String>,
) -> Result<SourceProject, SourceReadError> {
    crate::extract::project::extract_source_paths_with_progress(inputs, limits, observe, inspect)
}

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
