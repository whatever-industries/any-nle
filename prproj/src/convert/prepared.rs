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
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleRepresentation {
    Aac,
    Pcm16,
    Pcm24,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalAudio {
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
/// A one-sided Constant Gain fade attached to a specific mono edit.
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

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedProjectInput {
    pub project_name: String,
    pub options: Options,
    pub clips: Vec<PreparedClip>,
    pub bins: Vec<Bin>,
    pub sequences: Vec<Sequence>,
    pub texts: Vec<PreparedText>,
    pub video_dissolves: Vec<PreparedVideoDissolve>,
    pub audio_edge_fades: Vec<PreparedAudioEdgeFade>,
    pub audio_crossfades: Vec<PreparedAudioCrossfade>,
    pub audio_mixers: BTreeMap<usize, PreparedAudioMixer>,
    pub track_states: BTreeMap<usize, Vec<PreparedTrackState>>,
    pub picture_effects: Vec<PreparedPictureEffect>,
    /// A label on one placed clip, independent of its source item.
    pub edit_labels: Vec<PreparedEditLabel>,
    /// Verified direct DNxHR LB clip IDs and their physical media start ticks.
    /// The legacy clip fields carry the effective logging timecode.
    pub direct_dnxhr_origins: std::collections::HashMap<String, i64>,
    pub multicams: Vec<super::multicam::PreparedMulticam>,
    pub multicam_uses: Vec<super::multicam::PreparedMulticamUse>,
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
    /// None selects V1. Some selects the one-based mono audio lane.
    pub audio_track: Option<i64>,
    pub name: Option<String>,
    pub locked: bool,
    pub picture_disabled: bool,
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
    StaticMotion,
    Resize([MotionKey; 2]),
    TrimmedWarp(TrimmedMotion),
    TrimmedResize(TrimmedMotion),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPictureEffect {
    pub sequence: usize,
    pub event: usize,
    pub effect: PictureEffect,
    pub reflection_before_motion: [bool; 2],
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
            texts: Vec::new(),
            video_dissolves: Vec::new(),
            audio_edge_fades: Vec::new(),
            audio_crossfades: Vec::new(),
            audio_mixers: BTreeMap::new(),
            track_states: BTreeMap::new(),
            picture_effects: Vec::new(),
            edit_labels: Vec::new(),
            direct_dnxhr_origins: Default::default(),
            multicams: Vec::new(),
            multicam_uses: Vec::new(),
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
        validate_texts(&self.texts, &self.sequences)?;
        validate_video_dissolves(&self.video_dissolves, &self.sequences)?;
        validate_picture_effects(&self.picture_effects, &self.sequences)?;
        let fail = |s: &str| ModelError(s.into());
        for (&sequence, states) in &self.track_states {
            let seq = self.sequences.get(sequence).ok_or_else(|| fail("unknown sequence for track state"))?;
            let mut seen = HashSet::new();
            if states.is_empty() || states.len() > 3 || states.iter().any(|state| {
                let has_lane = match state.audio_track {
                    None => seq.events.iter().any(|edit| edit.video_track == Some(1)),
                    Some(lane @ (1 | 2)) => !state.picture_disabled
                        && seq.events.iter().any(|edit| edit.audio_tracks.contains(&lane))
                        && seq.events.iter().all(|edit| edit.stereo.iter().all(|s| s.audio_track != lane)),
                    _ => false,
                };
                !has_lane || !seen.insert(state.audio_track)
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
        let mut fade_owners = HashSet::new();
        for fade in &self.audio_edge_fades {
            let sequence = self.sequences.get(fade.sequence)
                .ok_or_else(|| fail("audio edge fade references unknown sequence"))?;
            let event = sequence.events.get(fade.event)
                .ok_or_else(|| fail("audio edge fade references unknown edit"))?;
            if !fade_owners.insert((fade.sequence, fade.event, fade.track))
                || !event.audio_tracks.contains(&fade.track)
                || event.video_track.is_some()
                || !event.stereo.is_empty()
                || !matches!(clips.get(event.clip.as_str()),
                    Some(PreparedClip::Original(clip)) if clip.container == "wav")
                || fade.head < 0 || fade.tail < 0
                || fade.head.checked_add(fade.tail).is_none_or(|n| n == 0 || n > event.duration)
                || sequence.audio_crossfades.iter().any(|f| f.track == fade.track)
            {
                return Err(fail("unqualified or duplicate mono audio edge fade"));
            }
        }
        self.validate_audio_crossfades(&clips)?;
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
            && c.audio.is_none() && v.width == 1920 && v.height == 1080
            && (v.rate == Fraction::new(24, 1) || v.rate == Fraction::new(24000, 1001)));
        require(
            c.container == "mov" && qualified_codec && v.stream_index == 0,
            "unqualified original picture stream/profile",
        )?;
        super::model::check_dimensions(v.width, v.height, "original video")?;
        super::model::check_rate(v.rate, "original video")?;
        super::model::check_ticks(v.duration as i128, "original video", true)?;
        require(
            v.duration % ticks_per_unit(v.rate) == 0,
            "original picture extent is not whole frames",
        )?;
    }
    if let Some(a) = &c.audio {
        require(
            matches!(a.channels, 1 | 2)
                && matches!(a.sample_rate, 44100 | 48000 | 96000)
                && a.stream_index == if c.video.is_some() { 1 } else { 0 },
            "unqualified original audio shape/stream",
        )?;
        require(
            match (&a.representation, c.container.as_str()) {
                (SampleRepresentation::Aac, "mov") => a.channels == 2,
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
