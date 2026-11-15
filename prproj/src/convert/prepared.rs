//! Typed writer input. Original files are never represented as PCM OP-Atom model/1 clips.
use super::model::*;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct ClipMetadata {
    pub id: String,
    pub name: String,
    pub material_umid: Option<String>,
    pub tape_name: Option<String>,
    pub avid: Option<AvidInfo>,
    pub time_display: Option<i64>,
    pub rate: Option<Fraction>,
    pub markers: Vec<Marker>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalVideo {
    pub codec: String,
    pub width: i64,
    pub height: i64,
    pub rate: Fraction,
    pub duration: i64,
    pub stream_index: i64,
    /// Embedded, non-drop source timecode in Premiere ticks, independent of file PTS.
    pub source_start: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleRepresentation {
    Aac,
    Pcm16,
    Pcm24,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalAudio {
    /// BWF display origin, independent of file-relative PCM trims.
    pub source_start: i64,
    pub sample_rate: i64,
    pub channels: i64,
    pub duration: i64,
    pub stream_index: i64,
    pub representation: SampleRepresentation,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalChannel {
    pub avid_track: i64,
    pub channel_index: i64,
    pub gain_db: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalClip {
    pub proxy_path: Option<String>,
    pub metadata: ClipMetadata,
    pub path: String,
    pub container: String,
    pub video: Option<OriginalVideo>,
    pub audio: Option<OriginalAudio>,
    pub channels: Vec<OriginalChannel>,
    pub groups: Vec<Vec<i64>>,
}
fn automatic_adapter_span(c: &OriginalClip, sequence_rate: Fraction) -> Option<(i128, i128)> {
    let video = c.video.as_ref()?;
    if c.container != "mov" || video.codec != "avc1" || video.width != 1920 || video.height != 1080
        || !c.audio.as_ref().is_some_and(|a| {
            a.sample_rate == 48_000 && a.channels == 2
                && a.representation == SampleRepresentation::Aac
        })
    {
        return None;
    }
    match ((video.rate.num, video.rate.den), (sequence_rate.num, sequence_rate.den)) {
        ((24, 1), (24_000, 1001)) => Some((1000, 1001)),
        ((24_000, 1001), (24, 1)) => Some((1001, 1000)),
        _ => None,
    }
}

// The forward cohort stores equal Source In and phase. In the reverse cohort
// Source In is floor(phase * 1000 / 1001), which can admit two adjacent phases.
fn automatic_adapter_phases(first: i128, span: i128, source: i128) -> std::ops::Range<i128> {
    if span < source {
        first..first + 1
    } else {
        (first * span + source - 1) / source..((first + 1) * span + source - 1) / source
    }
}

fn is_automatic_adapter_picture(c: &OriginalClip, s: &Sequence, e: &Event, map: &[i64]) -> bool {
    let Some((span, source)) = automatic_adapter_span(c, s.rate) else { return false; };
    if !(2..=16_384).contains(&map.len()) || e.video_track.is_none()
        || !e.audio_tracks.is_empty() || !e.stereo.is_empty() || e.source_duration.is_some()
        || e.matte.is_some() || e.transform.is_some() || e.color_correction.is_some()
    { return false; }
    let v = c.video.as_ref().unwrap();
    let source_unit = ticks_per_unit(v.rate);
    let record_unit = ticks_per_unit(s.rate);
    if e.source_in % source_unit != 0 || e.duration / record_unit != map.len() as i64 {
        return false;
    }
    let first = i128::from(e.source_in / source_unit);
    automatic_adapter_phases(first, span, source).any(|phase| {
        map.iter().enumerate().all(|(i, &f)| {
            let f = i128::from(f);
            f == first + (phase + i as i128) * source / span - phase * source / span
                && (f + 1) * i128::from(source_unit) <= i128::from(v.duration)
        })
    })
}

fn is_automatic_adapter_audio_tail(
    c: &OriginalClip, s: &Sequence, e: &Event,
    picture_spans: &HashSet<(&str, i64, i64, i64)>,
) -> bool {
    let Some((span, source)) = automatic_adapter_span(c, s.rate) else { return false; };
    let sample_unit = TICKS_PER_SECOND / 48_000;
    let source_unit = ticks_per_unit(c.video.as_ref().unwrap().rate);
    let record_unit = ticks_per_unit(s.rate);
    let source_samples_per_frame = source_unit / sample_unit;
    let source_sample = e.source_in / sample_unit;
    let first = source_sample / source_samples_per_frame - 1;
    let nominal_span = e.duration.checked_add(record_unit - e.duration % record_unit);
    e.video_track.is_none() && !e.audio_tracks.is_empty() && e.stereo.is_empty()
        && e.speed_ratio.is_none() && e.time_remap.is_none() && e.source_duration.is_none()
        && e.source_in % sample_unit == 0 && e.duration % sample_unit == 0
        && e.source_in.checked_add(e.duration) == c.audio.as_ref().map(|a| a.duration)
        && first >= 0
        && automatic_adapter_phases(i128::from(first), span, source).any(|p| {
            i128::from(source_sample % source_samples_per_frame)
                == (p * source % span) * i128::from(source_samples_per_frame) / span
        })
        && nominal_span.zip(first.checked_mul(source_unit)).is_some_and(|(duration, source_in)| {
            picture_spans.contains(&(e.clip.as_str(), e.record_start, duration, source_in))
        })
}
#[derive(Clone, Debug, PartialEq)]
pub enum PreparedClip {
    Legacy(Clip),
    Original(OriginalClip),
}
impl PreparedClip {
    pub fn id(&self) -> &str {
        match self {
            Self::Legacy(c) => &c.id,
            Self::Original(c) => &c.metadata.id,
        }
    }
}
/// A one-sided Constant Gain fade attached to a specific audio edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedAudioEdgeFade {
    pub sequence: usize,
    pub event: usize,
    pub track: i64,
    pub head: i64,
    pub tail: i64,
}

/// A Constant Gain crossfade between two identified mono WAV edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedAudioCrossfade {
    pub sequence: usize,
    pub track: i64,
    pub outgoing: usize,
    pub incoming: usize,
    pub duration: i64,
    pub before_cut: i64,
}

pub const MAX_AUDIO_PAN_KEYS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioPanKey {
    /// Absolute time on the file's Premiere source clock.
    pub ticks: i64,
    /// Normalized panner position from full left (0) to full right (1).
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedAudioPan {
    pub sequence: usize,
    pub event: usize,
    pub track: i64,
    pub keys: Vec<AudioPanKey>,
}

pub fn is_mono_pcm24_wav(clip: &OriginalClip) -> bool {
    clip.container == "wav" && clip.video.is_none()
        && clip.audio.as_ref().is_some_and(|audio| audio.channels == 1
            && audio.sample_rate == 48_000 && audio.representation == SampleRepresentation::Pcm24)
        && matches!((clip.channels.as_slice(), clip.groups.as_slice()),
            ([channel], [group]) if group.as_slice() == [channel.avid_track])
}

pub fn qualifies_audio_pan(clip: &OriginalClip) -> bool {
    is_mono_pcm24_wav(clip) && clip.channels[0].gain_db == 0.0
}

fn managed_audio_pan_covers(clip: &Clip, event: &Event, end: i64) -> bool {
    let [channel @ (1 | 2)] = event.audio_channels.as_slice() else {
        return false;
    };
    clip.rate == Some(Fraction::new(24, 1))
        && clip.video.as_ref().is_some_and(|video| video.start == 0
            && video.rate == Fraction::new(24, 1)
            && (video.width, video.height) == (1920, 1080))
        && clip.multichannel_groups.is_empty()
        && clip.audio.len() == 2
        && clip.audio.iter().any(|audio|
            audio.avid_track == Some(*channel) && audio.bits == 16
            && audio.sample_rate == 48_000 && audio.start == 0 && audio.gain_db == 0.0
            && end <= audio.duration && std::path::Path::new(&audio.path).extension()
                .and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("mxf")))
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedProjectInput {
    pub project_name: String,
    pub options: Options,
    pub clips: Vec<PreparedClip>,
    pub bins: Vec<Bin>,
    pub sequences: Vec<Sequence>,
    /// Destination sequence index -> bin for reusable AutoSync items.
    pub autosync_bins: BTreeMap<usize, usize>,
    pub texts: Vec<PreparedText>,
    pub video_dissolves: Vec<PreparedVideoDissolve>,
    pub audio_edge_fades: Vec<PreparedAudioEdgeFade>,
    pub audio_crossfades: Vec<PreparedAudioCrossfade>,
    pub audio_pans: Vec<PreparedAudioPan>,
    pub audio_mixers: BTreeMap<usize, PreparedAudioMixer>,
    pub track_states: BTreeMap<usize, Vec<PreparedTrackState>>,
    pub picture_effects: Vec<PreparedPictureEffect>,
    pub source_luts: Vec<PreparedSourceLut>,
    /// A label on one placed clip, independent of its source item.
    pub edit_labels: Vec<PreparedEditLabel>,
    /// Verified single-lane edits to write using the native Enable switch.
    pub disabled_edits: HashSet<(usize, usize)>,
    /// Verified direct DNx OP-Atom clip IDs and their physical media start ticks.
    /// The legacy clip fields carry the effective logging timecode.
    pub direct_dnxhr_origins: std::collections::HashMap<String, i64>,
    /// Verified 23.976 DNxHD LB/PCM24 cameras using the DNxHD media template.
    pub direct_dnxhd_pcm24: HashSet<String>,
    pub constant_speeds: Vec<PreparedConstantSpeed>,
    pub multicams: Vec<super::multicam::PreparedMulticam>,
    pub multicam_uses: Vec<super::multicam::PreparedMulticamUse>,
    pub picture_subclips: Vec<PreparedPictureSubclip>,
    pub av_subclips: Vec<PreparedAvSubclip>,
    pub av_subclip_uses: Vec<PreparedAvSubclipUse>,
    pub picture_nests: Vec<PreparedPictureNest>,
    pub layered_submasters: Vec<PreparedLayeredSubmaster>,
    pub png_mattes: PreparedPngMattes,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedPngMattes {
    pub stills: Vec<PreparedPngStill>,
    pub edits: Vec<PreparedPngEdit>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPngStill {
    pub bin: usize,
    pub path: std::path::PathBuf,
    pub name: String,
    pub alpha: PngAlpha,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngAlpha {
    Ignore,
    Straight { invert: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedPngEdit {
    pub sequence: usize,
    pub start: i64,
    pub background: usize,
    pub foreground: Option<usize>,
}

impl PreparedPngMattes {
    pub(super) fn validate(&self, input: &PreparedProjectInput) -> Result<(), ModelError> {
        let invalid = || ModelError("invalid PNG Matte Key source, edit clock or ownership".into());
        if self.edits.is_empty() && self.stills.is_empty() {
            return Ok(());
        }
        if input.options.bin_mode != "separate" || self.edits.is_empty()
            || self.edits.len() > 10_000 || self.stills.len() > 10_000
        {
            return Err(invalid());
        }
        for still in &self.stills {
            if still.bin >= input.bins.len() || !still.path.is_absolute()
                || !still.path.extension().and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("png"))
            {
                return Err(invalid());
            }
            super::model::check_text(&still.name, "imported PNG", 4096, false)?;
        }
        let mut occupied = HashSet::new();
        let text_sequences: HashSet<_> = input.texts.iter().map(|t| t.sequence).collect();
        let mut children = 0usize;
        let rate = Fraction::new(24, 1);
        let frame = ticks_per_unit(rate);
        for edit in &self.edits {
            let seq = input.sequences.get(edit.sequence).ok_or_else(invalid)?;
            let background = self.stills.get(edit.background).ok_or_else(invalid)?;
            if seq.rate != rate || (seq.width, seq.height) != (1920, 1080)
                || seq.start < 0 || seq.start % frame != 0
                || !seq.events.is_empty() || !seq.track_levels.is_empty() || !seq.audio_crossfades.is_empty()
                || text_sequences.contains(&edit.sequence)
                || edit.start < 0 || edit.start % frame != 0
                || edit.start.checked_add(frame).and_then(|end| seq.start.checked_add(end)).is_none()
                || !occupied.insert((edit.sequence, edit.start, 1))
                || background.alpha != PngAlpha::Ignore
            {
                return Err(invalid());
            }
            if let Some(foreground) = edit.foreground {
                match self.stills.get(foreground).ok_or_else(invalid)?.alpha {
                    PngAlpha::Ignore if occupied.insert((edit.sequence, edit.start, 2)) => {}
                    PngAlpha::Straight { .. } => children += 1,
                    _ => return Err(invalid()),
                }
            }
        }
        if input.sequences.len() + children > 10_000 {
            return Err(invalid());
        }
        Ok(())
    }
}

/// An editable picture retime whose signed playback rate and raw source
/// coordinates have a qualified Premiere mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedConstantSpeed {
    pub sequence: usize,
    pub event: usize,
    /// Signed source frames per output frame.
    pub ratio: (i64, i64),
}

impl PreparedConstantSpeed {
    pub(super) fn raw_trim(self, seq: &Sequence, event: &Event, clip: &Clip) -> Option<(i64, i64)> {
        let video = clip.video.as_ref()?;
        let rate = Fraction::new(24, 1);
        let frame = ticks_per_unit(rate);
        let (saved, source_frames) = match self.ratio {
            (1, 2) => ((2, 1), 25),
            (2, 1) => ((50, 99), 99),
            (-1, 1) => ((-1, 1), 50),
            _ => return None,
        };
        if seq.rate != rate || video.rate != rate || video.codec != "AVdn"
            || (seq.width, seq.height) != (1920, 1080)
            || (video.width, video.height) != (1920, 1080)
            || video.start != 0 || video.duration % frame != 0
            || event.video_track.is_none() || !event.audio_tracks.is_empty() || !event.stereo.is_empty()
            || event.time_remap.is_some() || event.speed_ratio != Some(saved)
            || event.source_duration != Some(source_frames * frame) || event.duration != 50 * frame
            || event.record_start < 0 || event.record_start % frame != 0
            || event.source_in < 0 || event.source_in % frame != 0
        {
            return None;
        }
        let end = event.source_in.checked_add(source_frames * frame)?;
        if end > video.duration {
            return None;
        }
        Some(if self.ratio.0 < 0 {
            (video.duration - end, video.duration - event.source_in)
        } else {
            (event.source_in, end)
        })
    }
}

pub(super) fn validate_constant_speeds(
    speeds: &[PreparedConstantSpeed], options: &Options, sequences: &[Sequence],
    mut clip: impl FnMut(&str, &Sequence, &Event, PreparedConstantSpeed) -> bool,
) -> Result<(), ModelError> {
    let mut owners = HashSet::new();
    for &speed in speeds {
        let valid = options.bin_mode == "separate"
            && owners.insert((speed.sequence, speed.event))
            && sequences.get(speed.sequence).is_some_and(|seq| {
                seq.events.get(speed.event).is_some_and(|event| clip(&event.clip, seq, event, speed))
            });
        if !valid {
            return Err(ModelError("invalid prepared constant speed, source range, or edit owner".into()));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedPictureNest {
    pub sequence: usize,
    pub event: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedLayeredSubmaster {
    pub sequence: usize,
    pub track: i64,
    pub start: i64,
    pub duration: i64,
    /// Lower and upper picture in child V1/V2 order, in Premiere ticks.
    pub layers: [PreparedPictureLayer; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedPictureLayer {
    pub clip: String,
    pub start: i64,
    pub source_in: i64,
    pub duration: i64,
}

pub const PICTURE_NEST_24_RATE: Fraction = Fraction { num: 24, den: 1 };
pub const PICTURE_NEST_RATES: [Fraction; 2] = [
    PICTURE_NEST_24_RATE,
    Fraction { num: 24_000, den: 1001 },
];

impl PreparedLayeredSubmaster {
    pub fn overlaps(&self, track: i64, start: i64, duration: i64) -> bool {
        self.track == track && self.start < start.saturating_add(duration)
            && start < self.start.saturating_add(self.duration)
    }

    pub fn has_picture_conflict(&self, input: &PreparedProjectInput) -> bool {
        let Some(sequence) = input.sequences.get(self.sequence) else { return true };
        sequence.events.iter().any(|e| {
            e.video_track.is_some_and(|track| self.overlaps(track, e.record_start, e.duration))
        }) || input.texts.iter().any(|text| {
            text.sequence == self.sequence && text.kind == PreparedTextKind::Graphic
                && self.overlaps(text.track, text.start, text.duration)
        }) || input.video_dissolves.iter().filter(|transition| transition.sequence == self.sequence)
            .any(|transition| {
                sequence.events.get(transition.incoming).is_none_or(|edit| {
                    edit.video_track.is_none_or(|track| {
                        track == self.track && edit.record_start.checked_sub(transition.before_cut)
                            .is_none_or(|start| self.overlaps(track, start, transition.duration))
                    })
                })
            })
    }
}

pub fn plain_picture_edit(edit: &Event) -> bool {
    edit.video_track.is_some_and(|track| track > 0) && edit.stereo.is_empty()
        && edit.speed_ratio.is_none() && edit.source_duration.is_none() && edit.time_remap.is_none()
        && edit.transform.is_none() && edit.matte.is_none() && edit.color_correction.is_none()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedPictureSubclip {
    pub bin: usize,
    pub parent_clip: String,
    pub name: String,
    /// Exclusive file-relative bounds in Premiere ticks.
    pub start: i64,
    pub end: i64,
}

/// A reusable V1/A1/A2 item with file-relative bounds in Premiere ticks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedAvSubclip {
    pub bin: usize,
    pub parent_clip: String,
    pub name: String,
    pub start: i64,
    pub end: i64,
}

impl PreparedAvSubclip {
    /// A shared marker collection can carry a picture marker at its backing-file position.
    pub fn source_marker<'a>(&self, parent: &'a Clip) -> std::result::Result<Option<&'a Marker>, &'static str> {
        let invalid = "A/V subclip source markers require one V1 range contained in the subclip";
        let mut contained = None;
        for marker in &parent.markers {
            let end = marker.start.checked_add(marker.duration).ok_or(invalid)?;
            // A range ending at In stays on the backing master. A point at
            // In belongs to the subclip. Out is exclusive for both.
            if marker.start >= self.end || (marker.start < self.start && end <= self.start) {
                continue;
            }
            if marker.track.as_deref() != Some("V1")
                || marker.start < self.start || end > self.end || contained.is_some()
            {
                return Err(invalid);
            }
            contained = Some(marker);
        }
        Ok(contained)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedAvSubclipUse {
    pub sequence: usize,
    pub event: usize,
    /// None selects picture. Some selects a destination mono track.
    pub audio_track: Option<i64>,
    pub subclip: usize,
}

pub fn av_subclip_edit_fits(sequence: &Sequence, event: &Event, parent: &Clip, subclip: &PreparedAvSubclip) -> bool {
    let Some(picture) = &parent.video else { return false };
    let unit = ticks_per_unit(picture.rate);
    event.clip == parent.id && sequence.rate == picture.rate
        && event.speed_ratio.is_none() && event.time_remap.is_none() && event.stereo.is_empty()
        && event.source_in >= subclip.start && event.source_in % unit == 0
        && event.duration > 0 && event.duration % unit == 0
        && event.source_in.checked_add(event.duration).is_some_and(|end| end <= subclip.end)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedEditLabel {
    pub sequence: usize,
    pub event: usize,
    /// None selects picture. Some selects a discrete mono audio lane.
    pub audio_track: Option<i64>,
    pub index: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedTrackState {
    pub lane: PreparedTrackLane,
    pub name: Option<String>,
    pub locked: bool,
    pub picture_disabled: bool,
    /// A named lane backed by saved empty track data, with no placements to create it.
    pub create_empty: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreparedTrackLane {
    Picture(i64),
    Audio(i64),
}

impl PreparedTrackLane {
    pub fn populated(sequence: &Sequence) -> HashSet<Self> {
        sequence.events.iter().flat_map(|edit|
            edit.video_track.into_iter().map(Self::Picture)
                .chain(edit.audio_tracks.iter().copied().map(Self::Audio))
                .chain(edit.stereo.iter().map(|stereo| Self::Audio(stereo.audio_track))))
            .filter(|lane| matches!(lane, Self::Picture(1..=99) | Self::Audio(1..=99)))
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedAudioMixer {
    /// Static stereo mix fader in dB.
    pub mix_level_db: Option<f64>,
    /// One-based audio track indices with static mute.
    pub muted_tracks: Vec<i64>,
    /// Static percent pan on mono inputs A1 and A2. A missing lane uses the standard stereo input.
    pub mono_track_pans: Option<[Option<f64>; 2]>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionKey {
    pub ticks: i64,
    pub position: [f64; 2],
    pub scale: [f64; 2],
    pub crop: Option<[f64; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarpRollKey {
    pub ticks: i64,
    pub x: f64,
    pub width: f64,
    pub rotation: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpRollInterpolation {
    Linear,
    Hold,
    /// Zero slopes and temporal handles of 40% of the adjacent key span.
    FlatBezier,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrimmedMotionKey {
    pub ticks: i64,
    pub position: [f64; 2],
    pub height: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrimmedMotion {
    pub motion: [TrimmedMotionKey; 3],
    pub width: [(i64, f64); 3],
    pub fixed_width: bool,
    pub crop: Option<[(i64, [f64; 4]); 3]>,
}

impl TrimmedMotion {
    pub fn keys_fit_media(&self, end: i64) -> bool {
        self.motion.iter().map(|k| k.ticks).chain(self.width.iter().map(|k| k.0))
            .chain(self.crop.iter().flatten().map(|k| k.0))
            .all(|time| (0..end).contains(&time))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PictureEffect {
    Flip { horizontal: bool, vertical: bool },
    CornerPin(CornerPin),
    /// One horizontal reflection and one static pin, in source render order.
    CornerPinWithFlop { pin: CornerPin, flop_before_pin: bool },
    StaticMotion,
    Resize([MotionKey; 2]),
    WarpRoll { keys: [WarpRollKey; 3], interpolation: WarpRollInterpolation },
    TrimmedWarp(TrimmedMotion),
    TrimmedResize(TrimmedMotion),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerPin {
    /// Full-image edge coordinates in TL, TR, BR, BL order.
    pub corners: [[f64; 2]; 4],
    /// Left, top, right and bottom margins, before the pin.
    pub source_crop: [f64; 4],
}

impl CornerPin {
    pub fn is_valid(self) -> bool {
        let [left, top, right, bottom] = self.source_crop;
        self.source_crop.iter().all(|v| (0.0..1.0).contains(v))
            && left + right < 1.0
            && top + bottom < 1.0
            && self.corners.iter().flatten().all(|v| (-1.0..=2.0).contains(v))
            && (0..4).all(|i| {
                let [a, b, c] = [self.corners[i], self.corners[(i + 1) % 4], self.corners[(i + 2) % 4]];
                (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) > 1e-9
            })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPictureEffect {
    pub sequence: usize,
    pub event: usize,
    pub effect: PictureEffect,
    pub reflection_before_motion: [bool; 2],
}

/// An embedded 2×2×2 cube attached to one plain picture edit.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedSourceLut {
    pub sequence: usize,
    pub event: usize,
    pub filename: String,
    pub points: [(f64, f64, f64); 8],
}

impl PreparedSourceLut {
    pub fn valid_filename(name: &str) -> bool {
        name.strip_suffix(".cube").is_some_and(|stem| !stem.is_empty())
            && name.len() <= 128
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b" ._()-".contains(&b))
    }
}

pub(super) fn validate_picture_effects(effects: &[PreparedPictureEffect], sequences: &[Sequence]) -> Result<(), ModelError> {
    let fail = |s: &str| ModelError(s.into());
    if effects.len() > MAX_ITEMS { return Err(fail("prepared picture effect count exceeds limit")); }
    let mut owners = HashSet::new();
    for p in effects {
        let sequence = sequences.get(p.sequence).ok_or_else(|| fail("unknown picture effect sequence"))?;
        let event = sequence.events.get(p.event).ok_or_else(|| fail("unknown picture effect edit"))?;
        if event.video_track.is_none() || !owners.insert((p.sequence, p.event)) {
            return Err(fail("picture effect needs one unique picture edit"));
        }
        if p.reflection_before_motion != [false; 2] && matches!(p.effect, PictureEffect::Flip { .. }) {
            return Err(fail("a reflection before Motion needs a Motion stage"));
        }
        match p.effect {
            PictureEffect::CornerPin(pin) | PictureEffect::CornerPinWithFlop { pin, .. } if pin.is_valid()
                && sequence.rate == Fraction::new(24, 1)
                && (sequence.width, sequence.height) == (1920, 1080)
                && p.reflection_before_motion == [false; 2]
                && event.transform.is_none() && event.matte.is_none() && event.color_correction.is_none()
                && event.speed_ratio.is_none() && event.time_remap.is_none() => {}
            PictureEffect::CornerPin(_) | PictureEffect::CornerPinWithFlop { .. } =>
                return Err(fail("invalid static Corner Pin or unqualified effect stack")),
            PictureEffect::Flip { horizontal, vertical }
                if (horizontal || vertical) && event.transform.is_none()
                    && event.matte.is_none() && event.color_correction.is_none() => {}
            PictureEffect::StaticMotion if event.transform.as_ref().is_some_and(|t|
                t.anchor == (0.0, 0.0) && t.rotation == 0.0
                && t.scale.0 > 0.0 && t.scale.1 > 0.0
                && t.crop.iter().all(|v| (0.0..1.0).contains(v))
                && t.crop[0] + t.crop[2] < 1.0 && t.crop[1] + t.crop[3] < 1.0)
                && event.matte.is_none() && event.color_correction.is_none()
                && event.speed_ratio.is_none() && event.time_remap.is_none() => {}
            PictureEffect::WarpRoll { keys, .. } => {
                let frame = ticks_per_unit(Fraction::new(24, 1));
                let end = event.source_in.checked_add(event.duration).and_then(|n| n.checked_sub(frame));
                let [a, b, c] = keys.map(|k| k.ticks);
                if sequence.rate != Fraction::new(24, 1) || (sequence.width, sequence.height) != (1920, 1080)
                    || event.transform.is_some() || event.matte.is_some() || event.color_correction.is_some()
                    || event.speed_ratio.is_some() || event.time_remap.is_some() || p.reflection_before_motion != [false; 2]
                    || a != event.source_in || a < 0 || Some(c) != end || !(a < b && b < c)
                    || [a, b, c].iter().any(|v| v % frame != 0)
                    || keys.iter().any(|k| !(-60.0/999.0..=90.0/999.0).contains(&k.x)
                        || !(70.0..=100.0).contains(&k.width) || !(-15.0..=25.0).contains(&k.rotation))
                {
                    return Err(fail("invalid Warp Rotation keys or picture stack"));
                }
            }
            PictureEffect::Resize(keys) => {
                check_rate(sequence.rate, "Resize sequence rate")?;
                let frame = ticks_per_unit(sequence.rate);
                let end = event.source_in.checked_add(event.duration).and_then(|n| n.checked_sub(frame));
                let cropped = keys[0].crop.is_some();
                if cropped != keys[1].crop.is_some()
                    || (cropped && (sequence.rate != Fraction::new(24_000, 1001)
                        || (sequence.width, sequence.height) != (1920, 1080)))
                    || event.transform.as_ref().is_none_or(|t| t.crop != [0.0; 4] || t.rotation != 0.0 || t.anchor != (0.0, 0.0))
                    || event.matte.is_some() || event.color_correction.is_some()
                    || frame <= 0 || event.speed_ratio.is_some() || event.time_remap.is_some()
                    || keys[0].ticks != event.source_in || event.source_in % frame != 0 || Some(keys[1].ticks) != end
                    || keys[1].ticks <= keys[0].ticks
                    || keys.iter().any(|k| k.position.iter().any(|x| !x.is_finite() || x.abs() > 64.0)
                        || k.scale.iter().any(|x| !(0.0..=10000.0).contains(x))
                        || k.crop.is_some_and(|[l, t, r, b]| {
                            k.scale.contains(&0.0)
                                || [l, t, r, b].iter().any(|x| !(0.0..1.0).contains(x))
                                || l + r >= 1.0 || t + b >= 1.0
                        }))
                {
                    return Err(fail("invalid Linear Motion key times or values"));
                }
            }
            PictureEffect::TrimmedWarp(trim) | PictureEffect::TrimmedResize(trim) => {
                let frame = ticks_per_unit(Fraction::new(24_000, 1001));
                let end = event.source_in.checked_add(event.duration).and_then(|n| n.checked_sub(frame));
                let [a, b, c] = trim.motion.map(|k| k.ticks);
                let [x, y, z] = trim.width.map(|(t, _)| t);
                let aligned_width = [x, y, z] == [a, b, c];
                let spans_qualified = if matches!(p.effect, PictureEffect::TrimmedResize(_)) {
                    end.is_some_and(|end| {
                        a <= event.source_in && x <= event.source_in && c >= end && z >= end
                            && ((a == event.source_in && x == event.source_in) || (c == end && z == end))
                            && (trim.fixed_width || aligned_width)
                    })
                } else {
                    a == event.source_in && Some(c) == end && a < y && y < c
                        && if trim.fixed_width {
                            (x < a && z == c) || (x == a && z > c)
                        } else {
                            aligned_width
                        }
                };
                if sequence.rate != Fraction::new(24_000, 1001) || (sequence.width, sequence.height) != (1920, 1080)
                    || event.transform.as_ref().is_none_or(|t| t.crop != [0.0; 4] || t.rotation != 0.0 || t.anchor != (0.0, 0.0))
                    || event.matte.is_some() || event.color_correction.is_some()
                    || event.speed_ratio.is_some() || event.time_remap.is_some()
                    || !spans_qualified || !(a < b && b < c && x < y && y < z)
                    || trim.crop.is_some_and(|keys| {
                        let [first, middle, last] = keys.map(|(t, _)| t);
                        end.is_none_or(|end| {
                            first > event.source_in || last < end
                                || (first != event.source_in && last != end)
                        }) || !(first < middle && middle < last)
                            || keys.iter().any(|&(t, [l, top, r, bottom])| {
                                t < 0 || t % frame != 0
                                    || [l, top, r, bottom].iter().any(|v| !(0.0..1.0).contains(v))
                                    || l + r >= 1.0 || top + bottom >= 1.0
                            })
                    })
                    || [a, b, c, x, y, z].iter().any(|t| *t < 0 || t % frame != 0)
                    || trim.motion.iter().any(|k| k.position.iter().any(|v| !v.is_finite() || v.abs() > 64.0)
                        || !(k.height > 0.0 && k.height <= 10_000.0))
                    || trim.width.iter().any(|(_, v)| !(*v > 0.0 && *v <= 10_000.0))
                {
                    return Err(fail("invalid trimmed Linear Motion key times or values"));
                }
            }
            _ => return Err(fail("empty Flip effect or unqualified effect stack")),
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedVideoDissolve {
    pub sequence: usize,
    pub outgoing: usize,
    pub incoming: usize,
    pub duration: i64,
    pub before_cut: i64,
}

pub(super) fn validate_video_dissolves(
    dissolves: &[PreparedVideoDissolve], sequences: &[Sequence],
) -> Result<(), ModelError> {
    let fail = |s: &str| ModelError(s.into());
    if dissolves.len() > MAX_ITEMS { return Err(fail("prepared dissolve count exceeds limit")); }
    let mut spans = Vec::with_capacity(dissolves.len());
    for d in dissolves {
        let s = sequences.get(d.sequence).ok_or_else(|| fail("dissolve references unknown sequence"))?;
        let out = s.events.get(d.outgoing).ok_or_else(|| fail("dissolve references unknown outgoing edit"))?;
        let inc = s.events.get(d.incoming).ok_or_else(|| fail("dissolve references unknown incoming edit"))?;
        let track = out.video_track.filter(|&t| inc.video_track == Some(t))
            .ok_or_else(|| fail("dissolve endpoints are not on one picture track"))?;
        let after = d.duration.checked_sub(d.before_cut).filter(|n| *n >= 0)
            .ok_or_else(|| fail("invalid dissolve extent"))?;
        check_rate(s.rate, "dissolve sequence rate")?;
        let frame = ticks_per_unit(s.rate);
        if frame <= 0 || d.before_cut <= 0 || d.before_cut % frame != 0 || after % frame != 0
            || out.record_start.checked_add(out.duration) != Some(inc.record_start)
            || d.before_cut > out.duration || after > inc.duration
            || [out, inc].iter().any(|e| e.speed_ratio.is_some() || e.time_remap.is_some())
        {
            return Err(fail("invalid two-sided dissolve cut, frame grid, or endpoint duration"));
        }
        let start = inc.record_start.checked_sub(d.before_cut).ok_or_else(|| fail("dissolve start overflows"))?;
        let end = inc.record_start.checked_add(after).ok_or_else(|| fail("dissolve end overflows"))?;
        spans.push((d.sequence, track, start, end));
    }
    spans.sort_unstable();
    if spans.windows(2).any(|p| (p[0].0, p[0].1) == (p[1].0, p[1].1) && p[1].2 < p[0].3) {
        return Err(fail("prepared video dissolves overlap"));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedTextKind { Caption, Graphic }
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedText {
    pub sequence: usize,
    pub source_id: String,
    pub kind: PreparedTextKind,
    pub track: i64,
    pub start: i64,
    pub duration: i64,
    pub content: String,
}
pub fn validate_texts(texts: &[PreparedText], sequences: &[Sequence]) -> Result<(), ModelError> {
    let fail = |s: &str| ModelError(s.into());
    if texts.len() > MAX_ITEMS { return Err(fail("prepared text count exceeds limit")); }
    let mut ids = HashSet::new();
    let mut payload_bytes = texts.len().checked_mul(std::mem::size_of::<PreparedText>()).ok_or_else(|| fail("text budget overflow"))?;
    for t in texts {
        let s = sequences.get(t.sequence).ok_or_else(|| fail("text references unknown sequence"))?;
        check_text(&t.source_id, "text source id", 32768, false)?;
        payload_bytes = payload_bytes.checked_add(t.source_id.len()).and_then(|n|n.checked_add(t.content.len())).filter(|n|*n <= 64<<20).ok_or_else(||fail("text payload exceeds 64 MiB"))?;
        if t.content.is_empty() || t.content.len() > 32768 || t.content.chars().any(|c| c.is_control() && c != '\n' && c != '\t') { return Err(fail("text content exceeds bounds or contains unsupported controls")); }
        if !ids.insert((t.sequence, &t.source_id)) || !(1..=64).contains(&t.track) || t.start < 0 || t.duration <= 0 || t.start.checked_add(t.duration).is_none_or(|end| end > crate::caption::builder::SYNTHETIC_DURATION - 3600 * crate::build::time::TICKS_PER_SECOND) { return Err(fail("invalid prepared text identity, track or span")); }
        if s.rate.num <= 0 || s.rate.den <= 0 { return Err(fail("text sequence rate must be positive")); }
        let scaled = crate::build::time::TICKS_PER_SECOND as i128 * s.rate.den as i128;
        if scaled % s.rate.num as i128 != 0 { return Err(fail("text sequence rate is not exactly representable")); }
        let frame = i64::try_from(scaled / s.rate.num as i128).map_err(|_| fail("text frame clock overflow"))?;
        if frame <= 0 || t.start % frame != 0 || t.duration % frame != 0 { return Err(fail("text span is not exactly frame aligned")); }
    }
    let mut captions: Vec<_> = texts.iter().filter(|t|t.kind == PreparedTextKind::Caption).collect();
    captions.sort_by_key(|t|(t.sequence,t.start));
    for pair in captions.windows(2) {
        if pair[0].sequence == pair[1].sequence && pair[1].start < pair[0].start + pair[0].duration { return Err(fail("prepared captions overlap on the active subtitle track")); }
    }
    let graphic_sequences: HashSet<_> = texts.iter().filter(|t|t.kind == PreparedTextKind::Graphic).map(|t|t.sequence).collect();
    let mut spans = Vec::new();
    for &si in &graphic_sequences {
        if sequences[si].events.len() > MAX_ITEMS { return Err(fail("text sequence event count exceeds limit")); }
        for e in &sequences[si].events {
            if let Some(track) = e.video_track { let end=e.record_start.checked_add(e.duration).ok_or_else(||fail("media endpoint overflow"))?;spans.push((si,track,e.record_start,end,false)); }
        }
    }
    for t in texts.iter().filter(|t|t.kind == PreparedTextKind::Graphic) { spans.push((t.sequence,t.track,t.start,t.start+t.duration,true)); }
    spans.sort_unstable();
    let (mut lane,mut media_end,mut text_end)=(None,0,0);
    for (sequence,track,start,end,is_text) in spans {
        if lane != Some((sequence,track)) {lane=Some((sequence,track));media_end=0;text_end=0;}
        if (is_text && start < media_end.max(text_end)) || (!is_text && start < text_end) {return Err(fail("prepared graphic overlaps another item on its video track"));}
        if is_text {text_end=text_end.max(end);}else{media_end=media_end.max(end);}
    }
    Ok(())
}
impl PreparedProjectInput {
    fn validate_layered_submasters(&self, clips: &HashMap<&str, &PreparedClip>) -> Result<(), ModelError> {
        let fail = || ModelError("invalid layered Submaster track, picture source, or extent".into());
        let total_children = self.layered_submasters.len().saturating_add(self.picture_nests.len());
        if self.layered_submasters.is_empty() {
            return Ok(());
        }
        if self.options.bin_mode != "separate" || self.sequences.len().saturating_add(total_children) > 10_000 {
            return Err(fail());
        }
        let mut extents = Vec::with_capacity(self.layered_submasters.len());
        for nest in &self.layered_submasters {
            let seq = self.sequences.get(nest.sequence).ok_or_else(fail)?;
            if !PICTURE_NEST_RATES.contains(&seq.rate) || (seq.width, seq.height) != (1920, 1080) {
                return Err(fail());
            }
            let unit = ticks_per_unit(seq.rate);
            let end = nest.start.checked_add(nest.duration).ok_or_else(fail)?;
            if !(1..=64).contains(&nest.track) || nest.start < 0 || nest.start % unit != 0
                || nest.duration <= 0 || nest.duration % unit != 0
                || seq.start <= 0 || seq.start % unit != 0 || seq.start.checked_add(end).is_none()
                || nest.layers[0].start != 0 || nest.layers[0].duration != nest.duration
                || nest.has_picture_conflict(self)
            {
                return Err(fail());
            }
            extents.push((nest.sequence, nest.track, nest.start, end));
            for layer in &nest.layers {
                let Some(PreparedClip::Legacy(clip)) = clips.get(layer.clip.as_str()).copied() else {
                    return Err(fail());
                };
                let video = clip.video.as_ref().ok_or_else(fail)?;
                let zero_origin_24 = seq.rate == PICTURE_NEST_24_RATE && video.start == 0 && video.codec == "AVdn";
                if (!zero_origin_24 && !self.direct_dnxhr_origins.contains_key(&layer.clip))
                    || (video.width, video.height) != (1920, 1080) || video.rate != seq.rate
                    || layer.start < 0 || layer.start % unit != 0 || layer.source_in < 0
                    || layer.source_in % unit != 0 || layer.duration <= 0 || layer.duration % unit != 0
                    || layer.start.checked_add(layer.duration).is_none_or(|end| end > nest.duration)
                    || layer.source_in.checked_add(layer.duration).is_none_or(|end| end > video.duration)
                {
                    return Err(fail());
                }
            }
        }
        extents.sort_unstable();
        if extents.windows(2).any(|pair| {
            let [(sequence, track, _, end), (next_sequence, next_track, start, _)] = pair else { unreachable!() };
            sequence == next_sequence && track == next_track && start < end
        }) {
            return Err(fail());
        }
        Ok(())
    }

    fn validate_picture_nests(&self, clips: &HashMap<&str, &PreparedClip>) -> Result<(), ModelError> {
        let fail = || ModelError("invalid prepared picture nest, source clock, or edit owner".into());
        if self.picture_nests.is_empty() {
            return Ok(());
        }
        if self.options.bin_mode != "separate" || self.sequences.len().saturating_add(self.picture_nests.len()) > 10_000 {
            return Err(fail());
        }
        let mut owners = HashSet::new();
        for nest in &self.picture_nests {
            if !owners.insert((nest.sequence, nest.event)) {
                return Err(fail());
            }
            let seq = self.sequences.get(nest.sequence).ok_or_else(fail)?;
            let event = seq.events.get(nest.event).ok_or_else(fail)?;
            let Some(PreparedClip::Legacy(clip)) = clips.get(event.clip.as_str()).copied() else { return Err(fail()); };
            let Some(video) = &clip.video else { return Err(fail()); };
            if !PICTURE_NEST_RATES.contains(&seq.rate) || video.rate != seq.rate {
                return Err(fail());
            }
            let unit = ticks_per_unit(seq.rate);
            let zero_origin_24 = seq.rate == PICTURE_NEST_24_RATE && video.start == 0 && video.codec == "AVdn";
            if !plain_picture_edit(event)
                || (seq.width, seq.height) != (1920, 1080) || (video.width, video.height) != (1920, 1080)
                || seq.start <= 0 || seq.start % unit != 0
                || event.record_start < 0 || event.record_start % unit != 0
                || event.record_start.checked_add(event.duration).is_none()
                || seq.start.checked_add(event.record_start).is_none()
                || event.source_in < 0 || event.source_in % unit != 0
                || event.duration <= 0 || event.duration % unit != 0
                || event.source_in.checked_add(event.duration).is_none_or(|end| end > video.duration)
                || (!zero_origin_24 && !self.direct_dnxhr_origins.contains_key(&event.clip))
            {
                return Err(fail());
            }
        }
        let used = |sequence, event| owners.contains(&(sequence, event));
        if self.multicam_uses.iter().any(|e| used(e.sequence, e.event))
            || self.av_subclip_uses.iter().any(|e| used(e.sequence, e.event))
            || self.picture_effects.iter().any(|e| used(e.sequence, e.event))
            || self.edit_labels.iter().any(|e| used(e.sequence, e.event))
            || self.video_dissolves.iter().any(|e| used(e.sequence, e.incoming) || used(e.sequence, e.outgoing))
            || self.sequences.iter().enumerate().any(|(si, seq)| seq.markers.iter().any(|m| {
                m.owner == "edit" && m.event.and_then(|i| usize::try_from(i).ok()).is_some_and(|ei| used(si, ei))
            }))
        {
            return Err(fail());
        }
        Ok(())
    }

    fn validate_audio_pans(&self, clips: &HashMap<&str, &PreparedClip>) -> Result<(), ModelError> {
        if self.audio_pans.is_empty() {
            return Ok(());
        }
        let fail = || ModelError("invalid prepared mono clip pan or source clock".into());
        if self.audio_pans.len() > MAX_ITEMS {
            return Err(fail());
        }
        let fades: HashSet<_> = self.audio_edge_fades.iter()
            .map(|fade| (fade.sequence, fade.event, fade.track)).collect();
        let crossfades: HashSet<_> = self.audio_crossfades.iter()
            .flat_map(|fade| [fade.outgoing, fade.incoming].map(|edit| (fade.sequence, edit, fade.track)))
            .collect();
        let mut seen = HashSet::new();
        for pan in &self.audio_pans {
            let seq = self.sequences.get(pan.sequence).ok_or_else(fail)?;
            let event = seq.events.get(pan.event).ok_or_else(fail)?;
            let owner = (pan.sequence, pan.event, pan.track);
            let end = event.source_in.checked_add(event.duration).ok_or_else(fail)?;
            let source_covers_range = match clips.get(event.clip.as_str()) {
                Some(PreparedClip::Original(clip)) => self.options.bin_mode == "separate"
                    && qualifies_audio_pan(clip)
                    && clip.audio.as_ref().is_some_and(|audio| end <= audio.duration),
                Some(PreparedClip::Legacy(clip)) =>
                    (self.options.bin_mode != "merged" || self.direct_dnxhr_origins.get(&clip.id) == Some(&0))
                    && managed_audio_pan_covers(clip, event, end),
                None => false,
            };
            if seq.rate != Fraction::new(24, 1)
                || (seq.width, seq.height) != (1920, 1080)
                || !seen.insert(owner) || fades.contains(&owner) || crossfades.contains(&owner)
                || self.audio_mixers.get(&pan.sequence).and_then(|mixer| mixer.mono_track_pans).is_some()
                || !source_covers_range
                || !(1..=99).contains(&pan.track) || event.audio_tracks != [pan.track]
                || event.video_track.is_some() || !event.stereo.is_empty()
                || event.speed_ratio.is_some() || event.source_duration.is_some() || event.time_remap.is_some()
                || event.audio != EventAudio::default()
                || event.audio_gains.iter().flatten().any(|gain| *gain != 0.0)
                || event.audio_pans.iter().flatten().any(|pan| *pan != 0.0)
                || event.source_in < 0 || event.duration <= 0
                || seq.audio_crossfades.iter().any(|fade| fade.track == pan.track)
                || !(2..=MAX_AUDIO_PAN_KEYS).contains(&pan.keys.len())
                || pan.keys.iter().any(|key| !(event.source_in..=end).contains(&key.ticks)
                    || !(0.0..=1.0).contains(&key.value))
                || pan.keys.windows(2).any(|pair| pair[0].ticks >= pair[1].ticks)
            {
                return Err(fail());
            }
        }
        Ok(())
    }

    fn validate_audio_crossfades(&self, clips: &HashMap<&str, &PreparedClip>) -> Result<(), ModelError> {
        let fail = || ModelError("invalid prepared mono audio crossfade or source handles".into());
        if self.audio_crossfades.len() > MAX_ITEMS {
            return Err(fail());
        }
        let mut spans = Vec::with_capacity(self.audio_crossfades.len());
        let edge_fade_lanes: HashSet<_> = self.audio_edge_fades.iter()
            .map(|f| (f.sequence, f.track)).collect();
        for fade in &self.audio_crossfades {
            let seq = self.sequences.get(fade.sequence).ok_or_else(fail)?;
            let out = seq.events.get(fade.outgoing).ok_or_else(fail)?;
            let inc = seq.events.get(fade.incoming).ok_or_else(fail)?;
            let after = fade.duration.checked_sub(fade.before_cut).filter(|n| *n > 0).ok_or_else(fail)?;
            check_rate(seq.rate, "audio crossfade sequence rate")?;
            let frame = ticks_per_unit(seq.rate);
            let mono_input = usize::try_from(fade.track).ok().and_then(|t| t.checked_sub(1))
                .and_then(|i| self.audio_mixers.get(&fade.sequence)?.mono_track_pans?.get(i).copied())
                .flatten().is_some();
            if !(1..=99).contains(&fade.track) || frame <= 0 || fade.before_cut <= 0
                || fade.before_cut % frame != 0 || after % frame != 0
                || out.record_start.checked_add(out.duration) != Some(inc.record_start)
                || fade.before_cut > out.duration || after > inc.duration
                || inc.source_in < fade.before_cut || mono_input
                || self.options.match_avid_default_pan
                || edge_fade_lanes.contains(&(fade.sequence, fade.track))
                || seq.audio_crossfades.iter().any(|f| f.track == fade.track)
            {
                return Err(fail());
            }
            for (edit, postroll) in [(out, after), (inc, 0)] {
                let Some(PreparedClip::Original(clip)) = clips.get(edit.clip.as_str()) else {
                    return Err(fail());
                };
                if edit.video_track.is_some() || edit.audio_tracks != [fade.track]
                    || !edit.stereo.is_empty() || edit.speed_ratio.is_some() || edit.time_remap.is_some()
                    || edit.source_duration.is_some() || edit.source_in < 0
                    || edit.audio != super::model::EventAudio::default()
                    || edit.audio_pans.iter().flatten().any(|pan| *pan != 0.0)
                    || edit.audio_gains.iter().flatten().any(|gain| *gain != 0.0)
                    || clip.container != "wav"
                    || !clip.audio.as_ref().is_some_and(|a| a.channels == 1 && a.sample_rate == 48_000
                        && edit.source_in.checked_add(edit.duration).and_then(|n| n.checked_add(postroll))
                            .is_some_and(|end| end <= a.duration))
                {
                    return Err(fail());
                }
            }
            let start = inc.record_start.checked_sub(fade.before_cut).ok_or_else(fail)?;
            let end = inc.record_start.checked_add(after).ok_or_else(fail)?;
            spans.push((fade.sequence, fade.track, start, end));
        }
        spans.sort_unstable();
        if spans.windows(2).any(|w| (w[0].0, w[0].1) == (w[1].0, w[1].1) && w[1].2 < w[0].3) {
            return Err(fail());
        }
        Ok(())
    }

    pub fn from_model(m: &Model) -> Result<Self, ModelError> {
        validate_model(m)?;
        Ok(Self {
            project_name: m.project_name.clone(),
            options: m.options.clone(),
            clips: m
                .clips
                .values()
                .cloned()
                .map(PreparedClip::Legacy)
                .collect(),
            bins: m.bins.clone(),
            sequences: m.sequences.clone(),
            autosync_bins: BTreeMap::new(),
            texts: Vec::new(),
            video_dissolves: Vec::new(),
            audio_edge_fades: Vec::new(),
            audio_crossfades: Vec::new(),
            audio_pans: Vec::new(),
            audio_mixers: BTreeMap::new(),
            track_states: BTreeMap::new(),
            picture_effects: Vec::new(),
            source_luts: Vec::new(),
            edit_labels: Vec::new(),
            disabled_edits: Default::default(),
            direct_dnxhr_origins: Default::default(),
            direct_dnxhd_pcm24: Default::default(),
            constant_speeds: Vec::new(),
            multicams: Vec::new(),
            multicam_uses: Vec::new(),
            picture_subclips: Vec::new(),
            av_subclips: Vec::new(),
            av_subclip_uses: Vec::new(),
            picture_nests: Vec::new(),
            layered_submasters: Vec::new(),
            png_mattes: PreparedPngMattes::default(),
        })
    }
    /// Strict legacy/native media subset for rewrap planning. Timeline references
    /// remain in this typed input and are validated jointly by `validate`.
    pub fn build_native_media_model(&self) -> Result<Model, ModelError> {
        self.validate()?;
        let clips: ClipMap = self
            .clips
            .iter()
            .filter_map(|c| match c {
                PreparedClip::Legacy(c) => Some(c.clone()),
                _ => None,
            })
            .collect();
        let bins = self
            .bins
            .iter()
            .map(|b| Bin {
                name: b.name.clone(),
                clips: b
                    .clips
                    .iter()
                    .filter(|id| clips.contains_key(id))
                    .cloned()
                    .collect(),
            })
            .collect();
        let model = Model {
            project_name: self.project_name.clone(),
            options: self.options.clone(),
            clips,
            bins,
            sequences: vec![],
        };
        validate_model(&model)?;
        Ok(model)
    }
    pub fn validate(&self) -> Result<(), ModelError> {
        self.png_mattes.validate(self)?;
        let mut lut_owners = HashSet::new();
        let picture_effects: HashSet<_> = self.picture_effects.iter()
            .map(|effect| (effect.sequence, effect.event)).collect();
        for lut in &self.source_luts {
            let key = (lut.sequence, lut.event);
            let valid_edit = self.sequences.get(lut.sequence).is_some_and(|sequence|
                sequence.rate == Fraction::new(24, 1) && (sequence.width, sequence.height) == (1920, 1080)
                && sequence.events.get(lut.event).is_some_and(|event|
                    event.video_track == Some(1) && event.transform.is_none() && event.matte.is_none()
                    && event.color_correction.is_none() && event.speed_ratio.is_none()
                    && event.time_remap.is_none()));
            if !valid_edit || !lut_owners.insert(key) || picture_effects.contains(&key)
                || !PreparedSourceLut::valid_filename(&lut.filename)
                || lut.points.iter().any(|&(r, g, b)| [r, g, b].iter()
                    .any(|v| !(0.0..=1.0).contains(v)))
            {
                return Err(ModelError("invalid prepared source LUT or unqualified picture stack".into()));
            }
        }
        if !self.constant_speeds.is_empty() {
            let clips: HashMap<_, _> = self.clips.iter().filter_map(|c| match c {
                PreparedClip::Legacy(c) => Some((c.id.as_str(), c)),
                PreparedClip::Original(_) => None,
            }).collect();
            validate_constant_speeds(&self.constant_speeds, &self.options, &self.sequences,
                |id, seq, event, speed| clips.get(id).is_some_and(|clip| speed.raw_trim(seq, event, clip).is_some()))?;
        }
        validate_texts(&self.texts, &self.sequences)?;
        validate_video_dissolves(&self.video_dissolves, &self.sequences)?;
        validate_picture_effects(&self.picture_effects, &self.sequences)?;
        let fail = |s: &str| ModelError(s.into());
        for (&sequence, states) in &self.track_states {
            let seq = self.sequences.get(sequence).ok_or_else(|| fail("unknown sequence for track state"))?;
            let populated = PreparedTrackLane::populated(seq);
            let mut seen = HashSet::new();
            if states.is_empty() || states.len() > 198 || states.iter().any(|state| {
                let supported_state = !(state.locked || state.picture_disabled) || match state.lane {
                    PreparedTrackLane::Picture(1) => true,
                    PreparedTrackLane::Audio(lane @ (1 | 2)) => !state.picture_disabled
                        && seq.events.iter().all(|edit| edit.stereo.iter().all(|s| s.audio_track != lane)),
                    _ => false,
                };
                let has_placements = populated.contains(&state.lane);
                let valid_lane = if state.create_empty {
                    !has_placements && state.name.is_some() && !state.locked && !state.picture_disabled
                        && matches!(state.lane, PreparedTrackLane::Picture(1..=99) | PreparedTrackLane::Audio(1..=99))
                } else {
                    has_placements
                };
                !valid_lane || !seen.insert(state.lane) || !supported_state
                    || state.name.as_ref().is_some_and(|name| {
                        name.is_empty() || name.len() > 255 || name.chars().any(char::is_control)
                    })
                    || state.name.is_none() && !state.locked && !state.picture_disabled
            }) {
                return Err(fail("invalid prepared track state or lane"));
            }
        }
        if self.edit_labels.len() > MAX_ITEMS {
            return Err(fail("prepared edit label count exceeds limit"));
        }
        let mut labeled = HashSet::new();
        for label in &self.edit_labels {
            let event = self.sequences.get(label.sequence)
                .and_then(|sequence| sequence.events.get(label.event))
                .ok_or_else(|| fail("unknown prepared label edit"))?;
            let on_edit = match label.audio_track {
                None => event.video_track.is_some(),
                Some(track) => event.audio_tracks.iter().filter(|&&n| n == track).count() == 1
                    && event.stereo.iter().all(|edit| edit.audio_track != track),
            };
            if !on_edit || usize::from(label.index) >= super::colors::LABEL_NAMES.len()
                || !labeled.insert((label.sequence, label.event, label.audio_track))
            {
                return Err(fail("prepared label needs one unique edit lane and a Premiere palette index"));
            }
        }
        if self.disabled_edits.len() > MAX_ITEMS || self.disabled_edits.iter().any(|&(si, ei)| {
            self.options.bin_mode != "separate" || self.sequences.get(si)
                .and_then(|s| s.events.get(ei)).is_none_or(|e| {
                    let mono = e.audio_tracks.len() == 1 && e.stereo.is_empty();
                    let stereo = e.audio_tracks.is_empty()
                        && matches!(e.stereo.as_slice(), [edit] if edit.channels == [1, 2]
                            && edit.gain_db.is_none() && edit.pans.iter().all(Option::is_none));
                    let picture = e.audio_tracks.is_empty() && e.stereo.is_empty()
                        && e.video_track == Some(1) && e.transform.is_none()
                        && e.color_correction.is_none() && e.matte.is_none();
                    let audio = e.video_track.is_none() && (mono || stereo);
                    !(picture || audio) || e.speed_ratio.is_some() || e.time_remap.is_some()
                })
        }) {
            return Err(fail("disabled edit requires one plain picture or audio placement in Separate files mode"));
        }
        // Count every retained nested payload without allocating another text/value tree.
        // This conservative bounded representation is separate from peak RSS.
        use std::fmt::Write;
        struct Counter(usize);
        impl std::fmt::Write for Counter {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                self.0 = self
                    .0
                    .checked_add(s.len())
                    .filter(|n| *n <= 64 << 20)
                    .ok_or(std::fmt::Error)?;
                Ok(())
            }
        }
        let mut count = Counter(
            std::mem::size_of::<Self>()
                + self.clips.capacity() * std::mem::size_of::<PreparedClip>()
                + self.bins.capacity() * std::mem::size_of::<Bin>()
                + self.sequences.capacity() * std::mem::size_of::<Sequence>(),
        );
        write!(&mut count, "{self:?}")
            .map_err(|_| fail("prepared input payload exceeds 64 MiB"))?;
        // Strict old validators validate legacy metadata/options and every legacy clip.
        let legacy: Model = Model {
            project_name: self.project_name.clone(),
            options: self.options.clone(),
            clips: Default::default(),
            bins: vec![],
            sequences: vec![],
        };
        validate_model(&legacy)?;
        if self.clips.len() > MAX_ITEMS
            || self.bins.len() > MAX_ITEMS
            || self.sequences.len() > 10_000
        {
            return Err(fail("prepared input collection limit"));
        }
        if !self.autosync_bins.is_empty() && (self.options.bin_mode != "separate"
            || self.autosync_bins.iter().any(|(&seq, &bin)| seq >= self.sequences.len() || bin >= self.bins.len()))
        {
            return Err(fail("invalid reusable AutoSync sequence or bin assignment"));
        }
        for (&sequence, mixer) in &self.audio_mixers {
            let mut muted = HashSet::new();
            if sequence >= self.sequences.len()
                || mixer.mix_level_db.is_some_and(|db| !(-200.0..=15.0).contains(&db))
                || mixer.mono_track_pans.is_some_and(|pans| {
                    !pans.iter().flatten().all(|pan| (-100.0..=100.0).contains(pan))
                })
                || mixer.muted_tracks.iter().any(|&track| {
                    !(1..=99).contains(&track) || !muted.insert(track)
                })
            {
                return Err(fail("invalid prepared audio mixer controls"));
            }
            if let Some(pans) = mixer.mono_track_pans {
                let seq = &self.sequences[sequence];
                let mono = |track| match track {
                    1 => pans[0].is_some(),
                    2 => pans[1].is_some(),
                    _ => false,
                };
                if self.options.match_avid_default_pan
                    || seq.events.iter().any(|event| {
                        event.stereo.iter().any(|edit| mono(edit.audio_track))
                            || event.audio_tracks.iter().zip(&event.audio_pans).any(|(&track, pan)| {
                                mono(track) && pan.is_some_and(|value| value != 0.0)
                            })
                    })
                    || seq.audio_crossfades.iter().any(|fade| mono(fade.track))
                {
                    return Err(fail("unsupported edits on mono inputs with track pan"));
                }
            }
        }
        let mut clips = HashMap::new();
        let mut budget = std::mem::size_of::<Self>() + self.project_name.len();
        for c in &self.clips {
            if clips.insert(c.id(), c).is_some() {
                return Err(fail("duplicate prepared clip id"));
            }
            budget = budget
                .checked_add(clip_bytes(c))
                .ok_or_else(|| fail("prepared input size overflow"))?;
            if budget > 64 << 20 {
                return Err(fail("prepared input exceeds 64 MiB"));
            }
            match c {
                PreparedClip::Original(c) => validate_original(c)?,
                PreparedClip::Legacy(c) => super::model::validate_clip(c, "prepared legacy clip")?,
            }
        }
        if self.picture_subclips.len() > MAX_ITEMS {
            return Err(fail("prepared subclip collection limit"));
        }
        for subclip in &self.picture_subclips {
            super::model::check_text(&subclip.name, "subclip name", 4096, false)?;
            let video = match clips.get(subclip.parent_clip.as_str()).copied() {
                Some(PreparedClip::Legacy(clip)) => clip.video.as_ref(),
                _ => None,
            }.ok_or_else(|| fail("subclip requires a prepared picture master"))?;
            let unit = ticks_per_unit(video.rate);
            if self.options.bin_mode != "separate"
                || !self.direct_dnxhr_origins.contains_key(&subclip.parent_clip)
                || subclip.bin >= self.bins.len()
                || !(0 <= subclip.start && subclip.start < subclip.end && subclip.end <= video.duration)
                || subclip.start % unit != 0
                || subclip.end % unit != 0
                || self.direct_dnxhr_origins[&subclip.parent_clip].checked_add(subclip.end).is_none()
            {
                return Err(fail("subclip requires a qualified linked picture range and bin"));
            }
        }
        if self.av_subclips.len() > MAX_ITEMS || self.av_subclip_uses.len() > MAX_ITEMS {
            return Err(fail("prepared A/V subclip collection limit"));
        }
        for subclip in &self.av_subclips {
            super::model::check_text(&subclip.name, "A/V subclip name", 4096, false)?;
            let Some(PreparedClip::Legacy(parent)) = clips.get(subclip.parent_clip.as_str()).copied() else {
                return Err(fail("A/V subclip requires a prepared backing master"));
            };
            let Some(picture) = &parent.video else {
                return Err(fail("A/V subclip requires backing picture"));
            };
            let frame = ticks_per_unit(picture.rate);
            let supported_backing = match parent.audio.as_slice() {
                [a, b] if picture.rate == Fraction::new(24, 1) =>
                    picture.start == 0 && [a, b].into_iter().all(|lane| {
                        lane.bits == 16 && lane.sample_rate == 48_000
                            && lane.start == 0 && lane.duration == picture.duration
                    })
                        && a.avid_track == Some(1) && b.avid_track == Some(2),
                [a, b] if picture.rate == Fraction::new(30_000, 1001) =>
                    parent.time_display == Some(102) && a.bits == 24 && b.bits == 24
                        && self.direct_dnxhr_origins.get(&subclip.parent_clip) == Some(&picture.start),
                [a, b] => picture.rate == Fraction::new(24_000, 1001)
                    && self.direct_dnxhr_origins.contains_key(&subclip.parent_clip)
                    && matches!(a.bits, 16 | 24) && a.bits == b.bits,
                _ => false,
            };
            if self.options.bin_mode != "separate" || subclip.bin >= self.bins.len()
                || !supported_backing
                || !(0 <= subclip.start && subclip.start < subclip.end && subclip.end <= picture.duration)
                || subclip.start % frame != 0 || subclip.end % frame != 0
                || picture.start.checked_add(subclip.end - subclip.start).is_none()
            {
                return Err(fail("A/V subclip requires a verified linked managed master with two matching mono PCM lanes"));
            }
            subclip.source_marker(parent).map_err(fail)?;
        }
        let mut subclip_lanes = HashSet::new();
        let group_uses: HashSet<_> = self.multicam_uses.iter().map(|u| (u.sequence, u.event)).collect();
        let marked_edits: HashSet<_> = self.sequences.iter().enumerate().flat_map(|(si, seq)| {
            seq.markers.iter().filter(|m| m.owner == "edit" && !m.inherited)
                .filter_map(move |m| Some((si, usize::try_from(m.event?).ok()?)))
        }).collect();
        for usage in &self.av_subclip_uses {
            let subclip = self.av_subclips.get(usage.subclip)
                .ok_or_else(|| fail("unknown prepared A/V subclip"))?;
            let seq = self.sequences.get(usage.sequence)
                .ok_or_else(|| fail("unknown prepared A/V subclip sequence"))?;
            let event = seq.events.get(usage.event)
                .ok_or_else(|| fail("unknown prepared A/V subclip edit"))?;
            let PreparedClip::Legacy(parent) = clips[subclip.parent_clip.as_str()] else { unreachable!() };
            let lane_exists = match usage.audio_track {
                Some(track) => event.audio_tracks.iter().filter(|&&n| n == track).count() == 1
                    && event_audio_files(parent, event).is_ok_and(|files| files.len() == event.audio_tracks.len()),
                None => event.video_track.is_some(),
            };
            if !lane_exists || group_uses.contains(&(usage.sequence, usage.event))
                || marked_edits.contains(&(usage.sequence, usage.event))
                || !subclip_lanes.insert((usage.sequence, usage.event, usage.audio_track))
                || !av_subclip_edit_fits(seq, event, parent, subclip)
            {
                return Err(fail("invalid or duplicate prepared A/V subclip edit"));
            }
        }
        let mut fade_owners = HashSet::new();
        for fade in &self.audio_edge_fades {
            let sequence = self.sequences.get(fade.sequence)
                .ok_or_else(|| fail("audio edge fade references unknown sequence"))?;
            let event = sequence.events.get(fade.event)
                .ok_or_else(|| fail("audio edge fade references unknown edit"))?;
            let source = clips.get(event.clip.as_str());
            let mono = event.audio_tracks.contains(&fade.track) && event.stereo.is_empty()
                && matches!(source, Some(PreparedClip::Original(clip)) if clip.container == "wav");
            let stereo = self.options.bin_mode == "separate"
                && sequence.rate == Fraction::new(24_000, 1001)
                && event.audio_tracks.is_empty() && fade.head == 0
                && matches!(&event.stereo[..], [st] if st.audio_track == fade.track && st.channels == [1, 2])
                && matches!(source, Some(PreparedClip::Legacy(clip)) if clip.video.is_none()
                    && clip.audio.len() == 2
                    && clip.audio.iter().all(|a| a.sample_rate == 48_000 && a.bits == 24));
            if !fade_owners.insert((fade.sequence, fade.event, fade.track))
                || event.video_track.is_some()
                || !(mono || stereo)
                || fade.head < 0 || fade.tail < 0
                || fade.head.checked_add(fade.tail).is_none_or(|n| n == 0 || n > event.duration)
                || sequence.audio_crossfades.iter().any(|f| f.track == fade.track)
            {
                return Err(fail("unqualified or duplicate audio edge fade"));
            }
        }
        self.validate_audio_crossfades(&clips)?;
        self.validate_picture_nests(&clips)?;
        self.validate_layered_submasters(&clips)?;
        self.validate_audio_pans(&clips)?;
        for b in &self.bins {
            super::model::check_text(&b.name, "prepared bin", 4096, false)?;
            for id in &b.clips {
                if !clips.contains_key(id.as_str()) {
                    return Err(fail("prepared bin references unknown clip"));
                }
            }
        }
        for (i, s) in self.sequences.iter().enumerate() {
            super::model::validate_sequence(s, &format!("prepared sequences[{i}]"))?;
            let mapped_picture_spans: HashSet<_> = s.events.iter().filter(|e| {
                e.video_track.is_some() && e.time_remap.is_some()
            }).map(|e| (e.clip.as_str(), e.record_start, e.duration, e.source_in)).collect();
            for e in &s.events {
                let c = clips
                    .get(e.clip.as_str())
                    .ok_or_else(|| fail("prepared event references unknown clip"))?;
                let (video, labels, duration, audio_count) = match c {
                    PreparedClip::Legacy(c) => (
                        c.video.is_some(),
                        c.audio
                            .iter()
                            .filter_map(|a| a.avid_track)
                            .collect::<Vec<_>>(),
                        c.audio
                            .iter()
                            .map(|a| a.duration)
                            .chain(c.video.iter().map(|v| v.duration))
                            .max()
                            .unwrap(),
                        c.audio.len(),
                    ),
                    PreparedClip::Original(c) => (
                        c.video.is_some(),
                        c.channels.iter().map(|a| a.avid_track).collect(),
                        c.audio
                            .iter()
                            .map(|a| a.duration)
                            .chain(c.video.iter().map(|v| v.duration))
                            .max()
                            .unwrap(),
                        c.channels.len(),
                    ),
                };
                if e.video_track.is_some() && !video
                    || e.audio_tracks.len() > audio_count
                    || e.audio_channels
                        .iter()
                        .chain(e.stereo.iter().flat_map(|s| &s.channels))
                        .any(|n| !labels.contains(n))
                {
                    return Err(fail("prepared event stream/channel reference mismatch"));
                }
                let unit = ticks_per_unit(s.rate);
                if e.record_start % unit != 0
                    || (e.duration % unit != 0 && !matches!(c, PreparedClip::Original(c)
                        if is_automatic_adapter_audio_tail(c, s, e, &mapped_picture_spans)))
                    || e.source_in as i128 + e.source_duration.unwrap_or(e.duration) as i128
                        > duration as i128
                {
                    return Err(fail("prepared event range/grid mismatch"));
                }
                if let PreparedClip::Original(c) = c {
                    if (e.speed_ratio.is_some() && !is_linked_prores_constant_speed(c, s, e))
                        || e.time_remap.as_ref().is_some_and(|map| {
                        !is_automatic_adapter_picture(c, s, e, map)
                    }) {
                        return Err(fail("original-file retiming is not qualified"));
                    }
                    if !e.stereo.is_empty()
                        && (e.audio.volume_db.is_some() || !e.audio.volume_keys.is_empty())
                    {
                        return Err(fail("original stereo volume automation requires a qualified stereo component"));
                    }
                    for st in &e.stereo {
                        if st.pans.iter().any(Option::is_some)
                            && !(st.pans[0].is_none_or(|p| p == -100.0)
                                && st.pans[1].is_none_or(|p| p == 100.0))
                        {
                            return Err(fail(
                                "original stereo per-channel pan requires qualification",
                            ));
                        }
                        if !c.groups.contains(&st.channels) {
                            return Err(fail(
                                "original grouped event lacks exact declared channels",
                            ));
                        }
                    }
                    let mono: Vec<_> = if e.audio_channels.is_empty() {
                        labels.iter().copied().take(e.audio_tracks.len()).collect()
                    } else {
                        e.audio_channels.clone()
                    };
                    if mono.iter().any(|n| !c.groups.contains(&vec![*n])) {
                        return Err(fail("original mono event requires a declared mono group"));
                    }
                    // Every selected physical stream must cover this range, not just the longest one.
                    let end = e.source_in as i128 + e.duration as i128;
                    if e.video_track.is_some() && e.time_remap.is_none()
                        && c.video.as_ref().is_none_or(|v| end > v.duration as i128)
                        || (!e.audio_tracks.is_empty() || !e.stereo.is_empty())
                            && c.audio.as_ref().is_none_or(|a| end > a.duration as i128)
                    {
                        return Err(fail("original event exceeds selected physical stream"));
                    }
                }
            }
        }
        super::multicam::validate(self, &clips)
    }
}

fn is_linked_prores_constant_speed(c: &OriginalClip, s: &Sequence, e: &Event) -> bool {
    let frame = ticks_per_unit(s.rate);
    c.container == "mov"
        && c.video.as_ref().is_some_and(|v| v.codec == "apch" && v.rate == s.rate)
        && c.audio.is_none()
        && s.rate == Fraction::new(24, 1)
        && (s.width, s.height) == (1920, 1080)
        && e.video_track == Some(1)
        && e.audio_tracks.is_empty()
        && e.stereo.is_empty()
        && e.source_in % frame == 0
        && e.time_remap.is_none()
        && match e.speed_ratio {
            Some((2, 1)) => e.source_duration.is_some_and(|source|
                source % frame == 0 && source.checked_mul(2) == Some(e.duration)),
            Some((-1, 1)) => e.source_duration == Some(e.duration),
            _ => false,
        }
}
pub fn validate_original(c: &OriginalClip) -> Result<(), ModelError> {
    let require = |b: bool, s: &str| if b { Ok(()) } else { Err(ModelError(s.into())) };
    super::model::check_text(&c.metadata.id, "original id", 256, false)?;
    super::model::check_text(&c.metadata.name, "original name", 4096, false)?;
    super::model::check_path(&c.path, "original path")?;
    if let Some(rate) = c.metadata.rate {
        super::model::check_rate(rate, "original clip rate")?;
    }
    if let Some(id) = &c.metadata.material_umid {
        require(
            id.len() == 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)),
            "original master identity is not full upper-case hex",
        )?;
    }
    if let Some(tape) = &c.metadata.tape_name {
        super::model::check_text(tape, "original tape", 4096, false)?;
    }
    if let Some(avid) = &c.metadata.avid {
        require(
            avid.columns.len() <= 512 && avid.aux_timecodes.len() <= 64,
            "original metadata collection limit",
        )?;
        super::model::check_text(&avid.mob_id, "original Avid identity", 4096, false)?;
        let mut keys = HashSet::new();
        for (k, v) in &avid.columns {
            require(keys.insert(k), "duplicate original metadata column")?;
            super::model::check_text(k, "original metadata column", 256, false)?;
            super::model::check_text(v, "original metadata value", 4096, true)?;
        }
        require(
            avid.color_rgb16
                .is_none_or(|rgb| rgb.into_iter().all(|n| (0..=65535).contains(&n))),
            "original metadata color range",
        )?;
        for s in avid.source_file.iter().chain(avid.source_path.iter()) {
            super::model::check_text(s, "original source metadata", 4096, false)?;
        }
    }
    require(
        c.video.is_some() || c.audio.is_some(),
        "original has no streams",
    )?;
    require(
        matches!(c.container.as_str(), "mov" | "wav"),
        "unqualified original container",
    )?;
    if let Some(v) = &c.video {
        let qualified_codec = v.codec == "avc1" || (v.codec == "apch"
            && std::path::Path::new(&c.path).extension()
                .and_then(|suffix| suffix.to_str())
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case("mov"))
            && (c.audio.is_none() || c.proxy_path.is_some()) && v.width == 1920 && v.height == 1080
            && (v.rate == Fraction::new(24, 1) || v.rate == Fraction::new(24000, 1001)));
        require(
            c.container == "mov" && qualified_codec && v.stream_index == 0,
            "unqualified original picture stream/profile",
        )?;
        super::model::check_dimensions(v.width, v.height, "original video")?;
        super::model::check_rate(v.rate, "original video")?;
        super::model::check_ticks(v.duration as i128, "original video", true)?;
        let frame = ticks_per_unit(v.rate);
        require(
            v.source_start >= 0
                && v.source_start % frame == 0
                && (v.source_start == 0 || v.codec == "apch"),
            "unqualified original picture source timecode",
        )?;
        super::model::check_ticks(
            i128::from(v.source_start) + i128::from(v.duration), "original video end", true,
        )?;
        require(
            v.duration % ticks_per_unit(v.rate) == 0,
            "original picture extent is not whole frames",
        )?;
    }
    if let Some(path) = &c.proxy_path {
        super::model::check_path(path, "Avid proxy path")?;
        require(path != &c.path && std::path::Path::new(path).is_absolute()
            && c.video.as_ref().is_some_and(|v| v.codec == "apch" && (v.width,v.height) == (1920,1080)
                && v.rate == Fraction::new(24000,1001))
            && std::path::Path::new(path).extension().and_then(|s| s.to_str())
                == Some(if c.audio.is_some() {"mov"} else {"mxf"})
            && c.audio.as_ref().is_none_or(|a| a.channels == 2 && a.sample_rate == 48000
                && a.duration == c.video.as_ref().unwrap().duration
                && matches!(a.representation, SampleRepresentation::Pcm16 | SampleRepresentation::Pcm24)),
            "unqualified Avid proxy shape or path")?;
    }
    if let Some(a) = &c.audio {
        let wav8 = a.channels == 8 && c.container == "wav" && c.video.is_none()
            && a.sample_rate == 48000 && a.representation == SampleRepresentation::Pcm16
            && c.metadata.rate == Some(Fraction::new(24, 1))
            && c.groups.len() == 8
            && c.groups.iter().enumerate().all(|(i, group)| group == &[i as i64 + 1])
            && c.channels.iter().all(|ch| ch.avid_track == ch.channel_index + 1);
        let frame = ticks_per_unit(Fraction::new(24, 1));
        require(if wav8 {
            a.source_start >= 0 && a.source_start < 24 * 60 * 60 * 24 * frame
                && a.source_start % frame == 0
                && a.source_start.checked_add(a.duration).is_some()
        } else { a.source_start == 0 }, "original audio start is outside the qualified BWF profile")?;
        require(
            (matches!(a.channels, 1 | 2) || wav8)
                && matches!(a.sample_rate, 44100 | 48000 | 96000)
                && a.stream_index == if c.video.is_some() { 1 } else { 0 },
            "unqualified original audio shape/stream",
        )?;
        require(
            match (&a.representation, c.container.as_str()) {
                (SampleRepresentation::Aac, "mov") => a.channels == 2,
                (SampleRepresentation::Pcm16 | SampleRepresentation::Pcm24, "mov") => c.proxy_path.is_some(),
                (SampleRepresentation::Pcm16 | SampleRepresentation::Pcm24, "wav") => true,
                _ => false,
            },
            "unqualified original sample representation",
        )?;
        super::model::check_ticks(a.duration as i128, "original audio", true)?;
        require(
            a.duration % ticks_per_unit(Fraction::new(a.sample_rate, 1)) == 0,
            "original audio extent is not whole samples",
        )?;
        require(
            c.channels.len() == a.channels as usize,
            "original audio channel coverage incomplete",
        )?;
    } else {
        require(
            c.channels.is_empty() && c.groups.is_empty(),
            "original audio mapping without stream",
        )?;
    }
    let mut labels = HashSet::new();
    let mut physical = HashSet::new();
    for ch in &c.channels {
        require(
            (1..=64).contains(&ch.avid_track)
                && labels.insert(ch.avid_track)
                && physical.insert(ch.channel_index)
                && c.audio
                    .as_ref()
                    .is_some_and(|a| (0..a.channels).contains(&ch.channel_index)),
            "original channel mapping invalid",
        )?;
        super::model::check_db(ch.gain_db, "original gain", -96., 96.)?;
    }
    let mut grouped = HashSet::new();
    for g in &c.groups {
        if let Some(first) = g
            .first()
            .and_then(|n| c.channels.iter().find(|a| a.avid_track == *n))
        {
            require(
                g.iter().all(|n| {
                    c.channels
                        .iter()
                        .find(|a| a.avid_track == *n)
                        .is_some_and(|a| a.gain_db == first.gain_db)
                }),
                "per-channel gain within an original stereo group requires qualification",
            )?;
        }
        require(
            matches!(g.len(), 1 | 2) && g.iter().all(|n| labels.contains(n) && grouped.insert(*n)),
            "original groups overlap or name missing channels",
        )?;
    }
    require(
        grouped == labels,
        "original groups must cover every logical channel",
    )?;
    for m in &c.metadata.markers {
        super::model::validate_marker(m, "original marker", false)?;
    }
    Ok(())
}

fn clip_bytes(c: &PreparedClip) -> usize {
    match c {
        PreparedClip::Legacy(c) => {
            std::mem::size_of::<Clip>()
                + c.id.len()
                + c.name.len()
                + c.audio
                    .iter()
                    .map(|a| a.path.len() + std::mem::size_of::<AudioFile>())
                    .sum::<usize>()
                + c.video.as_ref().map_or(0, |v| v.path.len() + v.codec.len())
        }
        PreparedClip::Original(c) => {
            std::mem::size_of::<OriginalClip>()
                + c.path.len()
                + c.metadata.id.len()
                + c.metadata.name.len()
                + c.channels.len() * std::mem::size_of::<OriginalChannel>()
        }
    }
}
