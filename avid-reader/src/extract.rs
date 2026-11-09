//! From decoded bin objects to the facts the converter uses: master clips, their file source
//! mobs (one per track, OP-Atom), and the import source mob behind them (Start, Aux TC,
//! Source File). Interpretations cite the Avid KB (pinned in converter/research-dependencies.json):
//! - Start = TCCP start of the referenced source mob + SourceClip offset (BCOL-085);
//! - clip color = _COLOR_R/_COLOR_G/_COLOR_B attributes (BCOL-019);
//! - custom/user columns = the _USER attribute set (AVB-0006).
//! Aux TC (BCOL-010) and Source File (BCOL-083) carriers are provisional (Avid queue Q-0001).

use std::collections::HashMap;

use crate::color::{file_order_hex, ColorCorrection, ParamValue};
use crate::file::AvbFile;
use crate::io::MobId;
use crate::objects::{Node, Value};
use crate::animatte::{animatte, AniMatteData};
use crate::transform::{Kind as PictureKind, PictureEffect};

pub const MOB_COMPOSITION: i64 = 1;
pub const MOB_MASTER: i64 = 2;
pub const MOB_SOURCE: i64 = 3;
pub const KIND_PICTURE: i64 = 1;
pub const KIND_SOUND: i64 = 2;
pub const KIND_TIMECODE: i64 = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Timecode {
    /// Frames at `fps` (Avid's timecode base), already including the SourceClip offset.
    pub frames: i64,
    pub fps: i64,
    pub drop: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuxTimecode {
    /// Track label on the import mob. Hypothesis (Q-0001): Aux TC number = index - 2.
    pub track_index: i64,
    /// Position of the segment on that track; `start` is the timecode at the clip's offset.
    pub position: i64,
    pub length: i64,
    pub start: i64,
    pub fps: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Essence {
    Picture { compression: String, resolution_id: i64, width: i64, height: i64, display_aspect: Option<(i64, i64)> },
    Sound { sample_rate: f64, bits: i64, channels: i64 },
    Other(String),
}

/// An Avid marker (TMBC; KB AVB-0001 framing, AVB-0002 ownership). Attached to a component
/// through its `_TMP_CRM` attribute (a TMCS list); `frame` = the component's position plus the
/// marker's `comp_offset`, counted in the owner component's own frames (KB 8bd1ef2: owner-local
/// placement supported; the transform through speed effects and its rounding are unverified).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AvidMarker {
    pub frame: i64,
    /// Spanned markers: `_ATN_CRM_LENGTH` frames (Phase 20 "spanned marker test": 22).
    pub length: Option<i64>,
    pub name: String,
    pub comment: String,
    pub user: String,
    /// `_ATN_CRM_COLOR_EXTENDED` (the 16-name palette); falls back to `_ATN_CRM_COLOR`, the
    /// legacy 8-name value, whose RGB the TMBC stores (Grey is saved as Black's RGB).
    pub color: String,
    pub rgb16: [i64; 3],
    pub id: String,
    pub created: Option<i64>,
    pub track_attr: Option<String>,
}

impl AvidMarker {
    /// Same visible content (name, comment, user, color, creation stamp). Not an identity
    /// claim: every Phase 20 TMBC has its own distinct `_ATN_CRM_ID` (KB 8bd1ef2,
    /// phase20-marker-report-review), and the cut-in copy lifecycle is unverified. The
    /// converter pairs this with an equal source frame, so a marker it skips is one the
    /// Premiere source clip already shows.
    pub fn same_content(&self, other: &AvidMarker) -> bool {
        (self.created, &self.user, &self.color, &self.comment, &self.name)
            == (other.created, &other.user, &other.color, &other.comment, &other.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaTrack {
    pub kind: i64,
    /// Avid track label (V1, A1, A2 -> 1, 1, 2).
    pub index: i64,
    pub file_mob: MobId,
    /// Edit rate and length of the file mob's essence, in its own units.
    pub edit_rate: f64,
    pub length: i64,
    pub essence: Option<Essence>,
    /// SourceClip offset from the master into the file mob, in master edit units.
    pub offset: i64,
    pub clip_length: i64,
    /// Clip-level pan/volume on this master track (PVOL): (gain dB, raw pan), when set.
    /// Level is Q2.29 linear gain (KB AVB-0005): 269072847 -> 0.50119 -> -6.000 dB.
    pub gain_db: Option<f64>,
    pub pan_raw: Option<i64>,
}

/// Raw master `_AUDIO_OUTPUT_FORMAT` observation (AVB-0005). Absence is not a
/// qualified Stereo default; reference indices are opaque and local to this bin.
/// This is independent of channel grouping and does not describe a mixing matrix.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MasterOutputMix {
    #[default]
    Absent,
    Integer(i64),
    Text(String),
    Bytes(Vec<u8>),
    Reference(u32),
    Null,
}
impl MasterOutputMix {
    pub fn payload_capacity(&self) -> usize {
        match self {
            Self::Text(v) => v.capacity(),
            Self::Bytes(v) => v.capacity(),
            _ => 0,
        }
    }
    fn from_attributes(attrs: &[(String, Value)]) -> Result<Self, String> {
        let mut found = attrs.iter().filter(|(k, _)| k == "_AUDIO_OUTPUT_FORMAT");
        let Some((_, value)) = found.next() else { return Ok(Self::Absent); };
        if found.next().is_some() { return Err("duplicate _AUDIO_OUTPUT_FORMAT".into()); }
        // Check before making another owned copy. The overall source-facts budget
        // additionally accounts for records and all retained payload capacity.
        let bytes = match value { Value::Str(v) => v.len(), Value::Bytes(v) => v.len(), _ => 0 };
        if bytes > crate::source::MAX_SOURCE_FACT_BYTES {
            return Err("_AUDIO_OUTPUT_FORMAT exceeds source-facts budget".into());
        }
        Ok(match value {
            Value::Int(v) => Self::Integer(*v),
            Value::Str(v) => Self::Text(v.clone()),
            Value::Bytes(v) => Self::Bytes(v.clone()),
            Value::Ref(v) => Self::Reference(*v),
            Value::Null => Self::Null,
            _ => return Err("unsupported _AUDIO_OUTPUT_FORMAT attribute type".into()),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MasterClip {
    pub name: String,
    pub mob_id: MobId,
    pub edit_rate: f64,
    pub length: i64,
    pub tracks: Vec<MediaTrack>,
    /// _USER column values in file order.
    pub columns: Vec<(String, String)>,
    pub color: Option<[i64; 3]>,
    pub start: Option<Timecode>,
    pub aux_timecodes: Vec<AuxTimecode>,
    pub source_file: Option<String>,
    pub source_path: Option<String>,
    /// Index of the bin (in the order given) the master was read from.
    pub bin: usize,
    /// Audio channel grouping from `_CHANNEL_GROUP_LIST` (KB AVID-UI-OPTIONS, Phase 22):
    /// "ST:A1A2," = one stereo pair of track labels 1 and 2. Tracks in no group are mono
    /// (fight bin "dual mono": attribute absent). Other group kinds kept raw.
    pub stereo_pairs: Vec<(i64, i64)>,
    pub other_groups: Vec<String>,
    pub output_mix: MasterOutputMix,
    /// Markers on the master's tracks: (media kind, track label, marker), frame from clip start.
    pub markers: Vec<(i64, i64, AvidMarker)>,
}

/// Parse `_CHANNEL_GROUP_LIST`: comma-terminated "KIND:A1A2..." entries.
pub fn parse_channel_groups(raw: &str) -> (Vec<(i64, i64)>, Vec<String>) {
    let (mut stereo, mut other) = (Vec::new(), Vec::new());
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let tracks: Option<Vec<i64>> = entry
            .split_once(':')
            .map(|(_, t)| t.split('A').filter(|x| !x.is_empty()).map(|x| x.parse::<i64>().ok()).collect::<Option<Vec<_>>>())
            .flatten();
        match (entry.split_once(':').map(|(k, _)| k), tracks.as_deref()) {
            (Some("ST"), Some([l, r])) => stereo.push((*l, *r)),
            _ => other.push(entry.to_string()),
        }
    }
    (stereo, other)
}

/// Recognize complete saved 5.1/7.1 declarations without inferring a layout from
/// file count. Unknown declarations remain in `other_groups` for preservation.
pub fn parse_multichannel_group(raw: &str) -> Option<(&'static str, Vec<i64>)> {
    if raw.len()>27 {return None;}
    let (kind, body) = raw.split_once(':')?;
    let (format, count) = match kind { "51" => ("5.1",6), "71" => ("7.1",8), _ => return None };
    let body = body.strip_prefix('A')?;
    let channels: Vec<i64> = body.split('A').map(|s| {
        if s.is_empty() || !s.bytes().all(|b|b.is_ascii_digit()) { None } else { s.parse().ok() }
    }).collect::<Option<_>>()?;
    if channels.len()!=count || channels.iter().any(|n|!(1..=64).contains(n)) || channels.iter().collect::<std::collections::HashSet<_>>().len()!=count { return None; }
    Some((format,channels))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct BinContents {
    pub masters: Vec<MasterClip>,
    pub sequences: Vec<SequenceInfo>,
    pub skipped: Vec<Skipped>,
    /// Master clips found again in a later bin: (name, bin index). Each copy is kept.
    pub duplicates: Vec<(String, usize)>,
}

/// One picture or sound segment of a sequence, resolved to a master clip track.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqEvent {
    pub kind: i64,
    pub track: i64,
    pub record_start: i64,
    pub length: i64,
    pub master: MobId,
    /// Master track label (V1 -> 1, A2 -> 2) and offset into the master, in master units.
    pub master_track: i64,
    pub source_in: i64,
    /// What was looked through to get here (subclip, group angle, effect), for the report.
    pub via: Vec<String>,
    /// Motion effect (SPED) stored span ratio = timeline frames / source frames, negative =
    /// reverse (KB AVB-0010). It is not always the exact rendered frame-sampling speed:
    /// the observed 200% test stores 50/99. `source_length` is the stored source span.
    pub speed: Option<(i64, i64)>,
    pub source_length: i64,
    /// Part of a multichannel edit (Avid EFF2_AUDIO_CHANNEL_COMBINER on a stereo track):
    /// (edit number within the sequence, channel index 0 = left, 1 = right).
    pub combined: Option<(usize, usize)>,
    /// The edit's own pan/volume (sequence PVOL; KB AVB-0005 / Phase 21): Avid copies the
    /// master's values at cut-in, so these are the edit's effective values when present.
    /// gain_db: Q2.29 level, only when its "gain set" flag is on. pan: percent -100 (L)..+100 (R)
    /// from 0..32768, only when "pan set" is on (otherwise Avid's default pan applies).
    pub edit_gain_db: Option<f64>,
    pub edit_pan: Option<f64>,
    /// Avid Color Correction effect on this edit (picture), raw parameters.
    pub color: Option<ColorCorrection>,
    /// Avid Resize / 3D Warp on this edit (picture), raw parameters; nested ones not composed.
    pub picture: Option<PictureEffect>,
    pub picture_nested: bool,
    /// Avid AniMatte on this edit (picture): shapes and tracker samples, plus the effect's
    /// start in the edit's frames (0 when the effect spans the edit).
    pub animatte: Option<AniMatteData>,
}

/// Avid PVOL static pan 0..32768 -> percent (-100 = full left, 0 = centre, +100 = full right).
pub fn pan_percent(raw: i64) -> f64 {
    (raw as f64 - 16384.0) / 16384.0 * 100.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeqTransition {
    pub kind: i64,
    pub track: i64,
    /// Start of the overlap; pyavb/AAF rule: a transition starts its own length before the
    /// position where it sits, and the next segment starts there too.
    pub position: i64,
    pub length: i64,
    pub cutpoint: i64,
    pub effect: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(Default))]
pub struct SequenceInfo {
    pub name: String,
    pub mob_id: Option<MobId>,
    pub bin: usize,
    pub edit_rate: f64,
    pub length: i64,
    pub start: Option<Timecode>,
    pub events: Vec<SeqEvent>,
    pub transitions: Vec<SeqTransition>,
    /// Things not converted, with their track and position.
    pub problems: Vec<String>,
    /// Audio track label -> `_TRACK_FORMAT` (2 = stereo track, KB Phase 10/12; absent = mono).
    pub audio_track_formats: Vec<(i64, i64)>,
    /// Markers in the sequence, at timeline frames.
    pub markers: Vec<SeqMarker>,
    combiner_count: usize,
}

impl SequenceInfo {
    /// Construct metadata for synthetic/source-facing callers without exposing
    /// the legacy compiler's private combiner counter.
    pub fn source_metadata(name: String, bin: usize, edit_rate: f64, length: i64) -> Self {
        Self { name, mob_id: None, bin, edit_rate, length, start: None,
            events: Vec::new(), transitions: Vec::new(), problems: Vec::new(),
            audio_track_formats: Vec::new(), markers: Vec::new(), combiner_count: 0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkerOwner {
    /// On the sequence itself (its own attributes; Avid shows these on the TC track).
    Sequence,
    /// On filler of a picture/sound track.
    Filler,
    /// On an edit (or an effect/group around it) of a track.
    Edit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeqMarker {
    pub owner: MarkerOwner,
    /// Media kind and track label; 0/0 for sequence-level markers.
    pub kind: i64,
    pub track: i64,
    pub marker: AvidMarker,
}

struct Ctx<'a> {
    file: &'a AvbFile,
    mobs: HashMap<MobId, &'a Node>,
}

fn int(n: &Node, k: &str) -> Option<i64> {
    n.get(k).and_then(Value::as_int)
}

fn float(n: &Node, k: &str) -> Option<f64> {
    match n.get(k) {
        Some(Value::Float(f)) => Some(*f),
        _ => None,
    }
}

fn mob_id(n: &Node) -> Option<MobId> {
    match n.get("mob_id") {
        Some(Value::MobId(m)) => Some(*m),
        _ => None,
    }
}

impl<'a> Ctx<'a> {
    /// Filler, or a sequence of filler only (an effect's empty background track).
    fn is_empty(&self, n: &Node) -> bool {
        &n.class == b"FILL"
            || (&n.class == b"SEQU"
                && match n.get("components") {
                    Some(Value::RefList(items)) => items.iter().all(|r| self.deref(Some(r)).is_some_and(|x| &x.class == b"FILL")),
                    _ => false,
                })
    }

    fn deref(&self, v: Option<&Value>) -> Option<&'a Node> {
        v.and_then(Value::as_index).and_then(|i| self.file.node(i))
    }

    fn attrs(&self, n: &Node) -> Vec<(String, Value)> {
        match self.deref(n.get("attributes")).and_then(|a| a.get("entries")) {
            Some(Value::Attrs(e)) => e.clone(),
            _ => Vec::new(),
        }
    }

    /// The markers attached to a component or mob (`_TMP_CRM` -> TMCS list of TMBC), with
    /// `frame` = `position` + comp_offset.
    fn markers(&self, n: &Node, position: i64) -> Vec<AvidMarker> {
        let mut out = Vec::new();
        for (k, v) in self.attrs(n) {
            if k != "_TMP_CRM" {
                continue;
            }
            let Some(list) = self.deref(Some(&v)) else { continue };
            let items: Vec<&Node> = match (&list.class, list.get("items")) {
                (b"TMCS", Some(Value::RefList(refs))) => refs.iter().filter_map(|r| self.deref(Some(r))).collect(),
                (b"TMBC", _) => vec![list],
                _ => Vec::new(),
            };
            for m in items.into_iter().filter(|m| &m.class == b"TMBC") {
                let mut marker = AvidMarker { frame: position + int(m, "comp_offset").unwrap_or(0), ..Default::default() };
                if let Some(Value::List(c)) = m.get("color") {
                    for (i, v) in c.iter().take(3).enumerate() {
                        marker.rgb16[i] = v.as_int().unwrap_or(0);
                    }
                }
                let mut legacy = String::new();
                for (ak, av) in self.attrs(m) {
                    let text = || av.as_str().map(str::to_string).unwrap_or_default();
                    match ak.as_str() {
                        "_ATN_CRM_COM" => marker.comment = text(),
                        "_ATN_CRM_MARKNAME" => marker.name = text(),
                        "_ATN_CRM_USER" => marker.user = text(),
                        "_ATN_CRM_COLOR_EXTENDED" => marker.color = text(),
                        "_ATN_CRM_COLOR" => legacy = text(),
                        "_ATN_CRM_ID" => marker.id = text(),
                        "_ATN_CRM_TRK" => marker.track_attr = av.as_str().map(str::to_string),
                        "_ATN_CRM_LENGTH" => marker.length = av.as_int().filter(|l| *l > 0),
                        "_ATN_CRM_LONG_CREATE_DATE" => marker.created = av.as_int(),
                        _ => {}
                    }
                }
                if marker.color.is_empty() {
                    marker.color = legacy;
                }
                out.push(marker);
            }
        }
        out
    }

    /// Markers anywhere in a master clip track's component: on it, on sequence children (at
    /// their positions) and on effect inputs (same position).
    fn component_markers(&self, c: &'a Node, position: i64, depth: usize, out: &mut Vec<AvidMarker>) {
        if depth > MAX_NESTING {
            return;
        }
        out.extend(self.markers(c, position));
        if let Some(Value::RefList(_)) = c.get("components") {
            for (pos, child) in self.segments(c) {
                self.component_markers(child, position + pos, depth + 1, out);
            }
        } else {
            for t in self.tracks(c) {
                if let Some(input) = self.deref(t.get("component")) {
                    self.component_markers(input, position, depth + 1, out);
                }
            }
        }
    }

    fn tracks(&self, mob: &'a Node) -> Vec<&'a Node> {
        match mob.get("tracks") {
            Some(Value::List(ts)) => ts
                .iter()
                .filter_map(|t| match t {
                    Value::Node(n) => Some(n.as_ref()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The (position, component) pairs of a track's component: a Sequence's children
    /// laid end to end, or the component itself at 0.
    fn segments(&self, component: &'a Node) -> Vec<(i64, &'a Node)> {
        if let Some(Value::RefList(children)) = component.get("components") {
            let mut pos = 0;
            let mut out = Vec::new();
            for child in children {
                if let Some(c) = self.deref(Some(child)) {
                    out.push((pos, c));
                    pos += int(c, "length").unwrap_or(0);
                }
            }
            out
        } else {
            vec![(0, component)]
        }
    }

    /// A master track's clip-level pan/volume effect, if its component is one.
    fn track_pan_volume(&self, track: &Node) -> Option<&'a Node> {
        self.deref(track.get("component")).filter(|c| &c.class == b"PVOL")
    }

    /// The single non-empty SourceClip of a track, if it has exactly one; looks through a
    /// clip-level pan/volume effect (PVOL) with one input.
    fn only_source_clip(&self, track: &Node) -> Result<&'a Node, String> {
        let mut component = self.deref(track.get("component")).ok_or("track has no component")?;
        if &component.class == b"PVOL" {
            let inputs: Vec<&Node> = self.tracks(component).into_iter().filter_map(|t| self.deref(t.get("component"))).collect();
            match inputs.as_slice() {
                [one] => component = one,
                _ => return Err(format!("pan/volume effect with {} inputs", inputs.len())),
            }
        }
        let clips: Vec<&Node> = self
            .segments(component)
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| &c.class == b"SCLP" && int(c, "length").unwrap_or(0) > 0)
            .collect();
        match clips.as_slice() {
            [one] => Ok(one),
            [] => Err("track has no source clip".into()),
            _ => Err(format!("track has {} source clips", clips.len())),
        }
    }
}

/// One bin on its own (sequences in it can only use masters from the same bin).
pub fn extract(file: &AvbFile) -> BinContents {
    extract_project(&[file])
}

fn bin_items(file: &AvbFile) -> Vec<&Node> {
    let root = match file.root() {
        Some(r) if &r.class == b"ABIN" => r,
        _ => return Vec::new(),
    };
    match root.get("items") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|i| match i {
                Value::Node(n) => file.node(n.get("mob").and_then(Value::as_index)?),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) mod project;
pub use project::{extract_project, extract_project_paths, ProjectReadError};

fn master(ctx: &Ctx, mob: &Node) -> Result<MasterClip, String> {
    let name = mob.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let id = mob_id(mob).ok_or("master clip has no mob ID")?;
    let attrs = ctx.attrs(mob);
    let output_mix = MasterOutputMix::from_attributes(&attrs)?;
    let mut columns = Vec::new();
    let mut color = [None, None, None];
    let mut groups = (Vec::new(), Vec::new());
    for (k, v) in &attrs {
        match k.as_str() {
            "_USER" => {
                if let Some(user) = ctx.deref(Some(v)) {
                    if let Some(Value::Attrs(e)) = user.get("entries") {
                        for (ck, cv) in e {
                            if let Value::Str(s) = cv {
                                columns.push((ck.clone(), s.clone()));
                            }
                        }
                    }
                }
            }
            "_CHANNEL_GROUP_LIST" => {
                if let Some(raw) = v.as_str() {
                    groups = parse_channel_groups(raw);
                }
            }
            "_COLOR_R" => color[0] = v.as_int(),
            "_COLOR_G" => color[1] = v.as_int(),
            "_COLOR_B" => color[2] = v.as_int(),
            _ => {}
        }
    }
    let color = match color {
        [Some(r), Some(g), Some(b)] => Some([r, g, b]),
        _ => None,
    };

    let mut tracks = Vec::new();
    let mut import_refs = Vec::new();
    let mut markers = Vec::new();
    for track in ctx.tracks(mob) {
        let component = match ctx.deref(track.get("component")) {
            Some(c) => c,
            None => continue,
        };
        let kind = int(component, "media_kind_id").unwrap_or(0);
        if kind != KIND_PICTURE && kind != KIND_SOUND {
            continue;
        }
        let index = int(track, "index").unwrap_or(0);
        let mut found = Vec::new();
        ctx.component_markers(component, 0, 0, &mut found);
        markers.extend(found.into_iter().map(|m| (kind, index, m)));
        let clip = ctx.only_source_clip(track).map_err(|e| format!("track {}: {}", index, e))?;
        let file_mob_id = mob_id(clip).ok_or_else(|| format!("track {}: source clip has no mob ID", index))?;
        let file_mob = *ctx
            .mobs
            .get(&file_mob_id)
            .ok_or_else(|| format!("track {}: file mob {} is not in this bin", index, file_mob_id.urn()))?;
        let descriptor = ctx.deref(file_mob.get("descriptor"));
        let essence = descriptor.map(|d| match &d.class {
            b"CDCI" => Essence::Picture {
                compression: match d.get("compress_method") {
                    Some(Value::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                    _ => String::new(),
                },
                resolution_id: int(d, "resolution_id").unwrap_or(0),
                width: int(d, "stored_width").unwrap_or(0),
                height: int(d, "stored_height").unwrap_or(0),
                display_aspect: match d.get("aspect_ratio") {
                    Some(Value::List(parts)) if parts.len() == 2 => match (&parts[0], &parts[1]) {
                        (Value::Int(n), Value::Int(den)) if *n > 0 && *den > 0 => Some((*n, *den)),
                        _ => None,
                    },
                    _ => None,
                },
            },
            b"PCMA" => Essence::Sound {
                sample_rate: float(d, "sample_rate").unwrap_or(0.0),
                bits: int(d, "quantization_bits").unwrap_or(0),
                channels: int(d, "channels").unwrap_or(0),
            },
            other => Essence::Other(String::from_utf8_lossy(other).into_owned()),
        });
        // The file mob's own track points at the import (or tape) source mob.
        if let Some(t) = ctx.tracks(file_mob).into_iter().next() {
            if let Ok(up) = ctx.only_source_clip(t) {
                if let Some(up_id) = mob_id(up) {
                    import_refs.push((up_id, int(up, "start_time").unwrap_or(0)));
                }
            }
        }
        tracks.push(MediaTrack {
            kind,
            index,
            file_mob: file_mob_id,
            edit_rate: descriptor.and_then(|d| float(d, "edit_rate")).unwrap_or(0.0),
            length: descriptor.and_then(|d| int(d, "length")).unwrap_or(0),
            essence,
            offset: int(clip, "start_time").unwrap_or(0),
            clip_length: int(clip, "length").unwrap_or(0),
            gain_db: ctx.track_pan_volume(track).filter(|p| p.get("level_set") == Some(&Value::Bool(true))).and_then(|p| {
                let level = int(p, "level")?;
                (level > 0).then(|| 20.0 * (level as f64 / (1u64 << 29) as f64).log10())
            }),
            pan_raw: ctx.track_pan_volume(track).filter(|p| p.get("pan_set") == Some(&Value::Bool(true))).and_then(|p| int(p, "pan")),
        });
    }
    if tracks.is_empty() {
        return Err("no picture or sound tracks".into());
    }

    // All file mobs of an OP-Atom master clip normally share one import mob and offset.
    let mut start = None;
    let mut aux_timecodes = Vec::new();
    let mut source_file = None;
    let mut source_path = None;
    import_refs.dedup();
    if let [(import_id, offset)] = import_refs.as_slice() {
        if let Some(import) = ctx.mobs.get(import_id) {
            source_file = import.get("name").and_then(Value::as_str).map(str::to_string);
            for track in ctx.tracks(import) {
                let component = match ctx.deref(track.get("component")) {
                    Some(c) => c,
                    None => continue,
                };
                if int(component, "media_kind_id") != Some(KIND_TIMECODE) {
                    continue;
                }
                let index = int(track, "index").unwrap_or(0);
                for (position, seg) in ctx.segments(component) {
                    if &seg.class != b"TCCP" {
                        continue;
                    }
                    let fps = int(seg, "fps").unwrap_or(0);
                    let seg_start = int(seg, "start").unwrap_or(0);
                    let length = int(seg, "length").unwrap_or(0);
                    if index == 1 {
                        if position <= *offset && *offset < position + length && start.is_none() {
                            start = Some(Timecode {
                                frames: seg_start + (offset - position),
                                fps,
                                drop: int(seg, "flags").unwrap_or(0) & 1 != 0,
                            });
                        }
                    } else if position <= *offset && *offset < position + length {
                        // Same rule as Start: the value at the clip's offset. In the fight bin
                        // the Aux segments sit at 0..242 while the clip is at 143014, and Avid
                        // exports Auxiliary TC3/TC4 blank (KB fdd7065 fight-bin-export-intake).
                        aux_timecodes.push(AuxTimecode {
                            track_index: index,
                            position,
                            length,
                            start: seg_start + (offset - position),
                            fps,
                        });
                    }
                }
            }
        }
    }
    if let Some((_, unc)) = columns.iter().find(|(k, _)| k == "UNC Path") {
        source_path = unc.rfind('/').map(|i| unc[..=i].to_string());
    }

    Ok(MasterClip {
        name,
        mob_id: id,
        edit_rate: float(mob, "edit_rate").unwrap_or(0.0),
        length: int(mob, "length").unwrap_or(0),
        tracks,
        columns,
        color,
        start,
        aux_timecodes,
        source_file,
        source_path,
        bin: 0,
        stereo_pairs: groups.0,
        other_groups: groups.1,
        output_mix,
        markers,
    })
}

const MAX_NESTING: usize = 16;

fn kind_label(kind: i64, track: i64) -> String {
    format!("{}{}", if kind == KIND_PICTURE { "V" } else { "A" }, track)
}

/// Every Avid Color Correction effect in a bin, in object-table order (for tests and reports).
pub fn color_effects(file: &AvbFile) -> Vec<ColorCorrection> {
    let ctx = Ctx { file, mobs: HashMap::new() };
    (0..file.object_count() as u32)
        .filter_map(|i| file.node(i))
        .filter(|n| &n.class == b"TKFX" && n.get("effect_id").and_then(Value::as_str) == Some("EFF2_RGB_COLOR_CORRECTION"))
        .map(|n| color_params(&ctx, n))
        .collect()
}

/// A TrackEffect's parameter items (PRLS of PRIT; curve payloads are AVUP objects).
fn color_params(ctx: &Ctx, effect: &Node) -> ColorCorrection {
    let mut out = ColorCorrection::default();
    let Some(list) = ctx.deref(effect.get("param_list")) else { return out };
    let Some(Value::RefList(items)) = list.get("items") else { return out };
    for item in items.iter().filter_map(|r| ctx.deref(Some(r))) {
        if &item.class != b"PRIT" {
            continue;
        }
        let Some(Value::Uuid(u)) = item.get("uuid") else { continue };
        let value = match item.get("value") {
            Some(Value::Int(i)) => ParamValue::Int(*i),
            Some(Value::Float(f)) => ParamValue::Float(*f),
            Some(r @ Value::Ref(_)) => match ctx.deref(Some(r)) {
                Some(n) if &n.class == b"AVUP" => match n.get("data") {
                    Some(Value::Bytes(b)) => ParamValue::Bytes(b.clone()),
                    _ => ParamValue::Other,
                },
                _ => ParamValue::Other,
            },
            _ => ParamValue::Other,
        };
        out.params.push((file_order_hex(u), value));
    }
    out
}

/// HH:MM:SS:FF for a non-drop frame count (wrapping at 24 h, as Avid displays).
pub fn format_timecode(frames: i64, fps: i64) -> String {
    if fps <= 0 {
        return String::new();
    }
    let day = 24 * 3600 * fps;
    let f = frames.rem_euclid(day);
    let (h, rem) = (f / (3600 * fps), f % (3600 * fps));
    let (m, rem) = (rem / (60 * fps), rem % (60 * fps));
    format!("{:02}:{:02}:{:02}:{:02}", h, m, rem / fps, rem % fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_formatting() {
        assert_eq!(format_timecode(143014, 24), "01:39:18:22");
        assert_eq!(format_timecode(9, 24), "00:00:00:09");
        assert_eq!(format_timecode(26439890, 24), "18:01:02:02");
    }

    #[test]
    fn channel_groups() {
        assert_eq!(parse_channel_groups("ST:A1A2,"), (vec![(1, 2)], vec![]));
        assert_eq!(parse_channel_groups("ST:A1A2,ST:A7A8,"), (vec![(1, 2), (7, 8)], vec![]));
        assert_eq!(parse_channel_groups("51:A2A1A3A4A5A6,"), (vec![], vec!["51:A2A1A3A4A5A6".to_string()]));
        assert_eq!(parse_channel_groups(""), (vec![], vec![]));
        assert_eq!(parse_multichannel_group("51:A1A3A2A5A6A4"),Some(("5.1",vec![1,3,2,5,6,4])));
        assert_eq!(parse_multichannel_group("71:A1A3A2A7A8A5A6A4"),Some(("7.1",vec![1,3,2,7,8,5,6,4])));
        assert_eq!(parse_multichannel_group("51:A2A1A3A4A5A6"),Some(("5.1",vec![2,1,3,4,5,6]))); // AAC
        assert_eq!(parse_multichannel_group("71:A1A3A2A5A6A7A8A4"),Some(("7.1",vec![1,3,2,5,6,7,8,4]))); // SMPTE DS
        for invalid in ["51:A1A2A3A4A5", "51:A1A2A3A4A5A5", "71:A1A2A3A4A5A6A7A65", "51:A1A2A3A4A5A6,", "ST:A1A2", "51:A+1A2A3A4A5A6"] {assert!(parse_multichannel_group(invalid).is_none());}
    }

    #[test]
    fn master_output_mix_is_a_raw_independent_observation() {
        use MasterOutputMix as M;
        assert_eq!(M::from_attributes(&[]).unwrap(), M::Absent);
        for (raw, expected) in [
            (Value::Int(4), M::Integer(4)), (Value::Int(5), M::Integer(5)),
            (Value::Int(-17), M::Integer(-17)), (Value::Str("unknown".into()), M::Text("unknown".into())),
            (Value::Bytes(vec![0,255]), M::Bytes(vec![0,255])), (Value::Ref(7), M::Reference(7)),
            (Value::Null, M::Null),
        ] {
            let row = ("_AUDIO_OUTPUT_FORMAT".into(), raw);
            assert_eq!(M::from_attributes(&[row.clone()]).unwrap(), expected);
            assert!(M::from_attributes(&[row.clone(),row]).unwrap_err().contains("duplicate"));
        }
        assert!(M::from_attributes(&[("_AUDIO_OUTPUT_FORMAT".into(),Value::Bool(true))]).is_err());
    }
}
