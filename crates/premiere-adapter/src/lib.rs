//! Borrowed source interpretation -> bounded, destination-specific Premiere input.
//! This is deliberately separate from graph/1 projection and from project I/O.
use graph::source::{SourceGraph, SourceItem, SourceMarker, SourceMedia, SourceSequence};
use graph::{GraphError, Loss, MarkerOwner, Placement, Rational, StreamKind, TrackRef, Value};
use prproj::convert::model::{
    self, AudioCrossfade, AudioFile, Bin, Clip, Event, EventAudio, Fraction, Marker, Model,
    Options, Sequence, StereoEdit, TrackLevel, Transform, VideoFile, VolumeKey, MultichannelGroup, TICKS_PER_SECOND,
};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use prproj::convert::prepared::{PreparedProjectInput,PreparedClip,ClipMetadata,OriginalClip,OriginalVideo,OriginalAudio,OriginalChannel,SampleRepresentation,PreparedVideoDissolve,PreparedAudioPan};
mod text;
mod multicam;
mod picture;
mod subclips;
mod autosync;
mod pan;
mod gain;
mod nests;
pub use nests::{has_layered_candidates, LAYERED_LOSS};
mod disabled;
mod track_names;
mod gain_gap;
mod offline_gap;

type R<T> = Result<T, GraphError>;
fn error(s: impl Into<String>) -> GraphError {
    GraphError(s.into())
}
fn ticks(t: Rational, what: &str) -> R<i64> {
    t.to_units(TICKS_PER_SECOND)
        .ok_or_else(|| error(format!("{what}: {t} is not on the Premiere tick clock")))
}
fn prepare_volume_keys(
    facts: &PreparationFacts, p: &graph::Placement, losses: &mut Vec<Loss>,
) -> R<Vec<VolumeKey>> {
    let avid = side(&facts.sidecars, "avid.source", &p.id)
        .and_then(|v| key(v, "pvol_volume_keys")).and_then(boolean) == Some(true);
    let mut rounded = false;
    let keys = p.volume_keys.iter().map(|k| {
        let ticks = match k.at.to_units(TICKS_PER_SECOND) {
            Some(ticks) => ticks,
            None if avid && !k.at.is_negative() => {
                let den = i128::from(k.at.den());
                let scaled = i128::from(k.at.num()) * i128::from(TICKS_PER_SECOND);
                rounded = true;
                i64::try_from((scaled + den / 2) / den)
                    .map_err(|_| error("volume key tick rounding overflows"))?
            }
            None => ticks(k.at, "volume key")?,
        };
        Ok(VolumeKey { ticks, db: k.db })
    }).collect::<R<Vec<_>>>()?;
    if avid && !keys.is_empty() {
        if keys.windows(2).any(|pair| pair[0].ticks >= pair[1].ticks) {
            loss(losses, &p.id, "volume_automation", "volume key positions collide on the Premiere tick clock");
            return Ok(Vec::new());
        }
        loss(losses, &p.id, "volume_interpolation",
            "Avid volume points written as editable Premiere linear keys; source envelope interpolation is unqualified");
        if rounded {
            loss(losses, &p.id, "volume_key_clock",
                "subframe volume times rounded to the nearest Premiere tick; exact values retained in source graph");
        }
    }
    Ok(keys)
}
fn fraction(r: Rational) -> Fraction {
    Fraction {
        num: r.num(),
        den: r.den(),
    }
}
fn add(a: Rational, b: Rational) -> R<Rational> {
    a.checked_add(b)
        .ok_or_else(|| error("preparation time overflow"))
}
fn sub(a: Rational, b: Rational) -> R<Rational> {
    a.checked_sub(b)
        .ok_or_else(|| error("preparation time overflow"))
}
fn clip_id(id: &str) -> String {
    id.strip_prefix("clip:").unwrap_or(id).to_owned()
}
fn key<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    if let Value::Obj(xs) = v {
        xs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    } else {
        None
    }
}
fn str_value(v: &Value) -> Option<&str> {
    if let Value::Str(s) = v {
        Some(s)
    } else {
        None
    }
}
fn external_source_lut_filename(transforms: &[Value]) -> Option<String> {
    let [transform] = transforms else { return None };
    let xml = str_value(key(transform, "text")?)?;
    if xml.len() > 16 * 1024 {
        return None;
    }
    let mut tree = prproj::xml::Tree::new();
    let root = tree.parse_fragment(xml).ok()?;
    let [entry] = tree.children(root) else { return None };
    let [lut] = tree.children(*entry) else { return None };
    if tree.tag(root) != "ColorTransformationList"
        || tree.tag(*entry) != "ColorTransformation"
        || tree.tag(*lut) != "ExternalLut"
    {
        return None;
    }
    let leaf_text = |tag| {
        let mut fields = tree.children(*lut).iter().filter(|&&id| tree.tag(id) == tag);
        let &field = fields.next()?;
        if fields.next().is_some() || !tree.is_childless(field) {
            return None;
        }
        tree.text(field)
    };
    if leaf_text("LutDimension")? != "3D" {
        return None;
    }
    let filename = leaf_text("LutFileName")?;
    let (stem, extension) = filename.rsplit_once('.')?;
    if stem.is_empty()
        || !extension.eq_ignore_ascii_case("cube")
        || filename.len() > 255
        || filename.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return None;
    }
    Some(filename.to_owned())
}
fn boolean(v: &Value) -> Option<bool> {
    if let Value::Bool(b) = v {
        Some(*b)
    } else {
        None
    }
}
fn integer(v: &Value) -> Option<i64> {
    if let Value::Int(n) = v {
        Some(*n)
    } else {
        None
    }
}

/// Require the retained source declaration as well as the neutral ordered group.
/// Six/eight channels without this witness do not establish speaker semantics.
fn declared_multichannel(raw: &str) -> Option<MultichannelGroup> {
    if raw.len()>27 {return None;}
    let (kind, body) = raw.split_once(':')?;
    let (format, width) = match kind { "51" => ("5.1",6), "71" => ("7.1",8), _ => return None };
    let channels = body.strip_prefix('A')?.split('A').map(|s| {
        if s.is_empty() || !s.bytes().all(|b|b.is_ascii_digit()) { None } else { s.parse::<i64>().ok() }
    }).collect::<Option<Vec<_>>>()?;
    if channels.len()!=width || channels.iter().any(|n|!(1..=64).contains(n)) || channels.iter().collect::<HashSet<_>>().len()!=width { return None; }
    Some(MultichannelGroup { format: format.into(), channels })
}

/// Initial structural cutover deliberately retains the old import clock rules.
/// Rewrap uses the separately retained native file-clock interpretation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockProfile {
    LegacyImport,
    VerifiedRewrap,
}

pub struct PreparedPremiere {
    pub model: Model,
    pub losses: Vec<Loss>,
    pub diagnostics: Vec<String>,
    /// Source placement -> prepared sequence/event, including many-to-one assembly.
    pub ownership: HashMap<String, (usize, usize)>,
    pub texts: Vec<prproj::convert::prepared::PreparedText>,
    pub video_dissolves: Vec<PreparedVideoDissolve>,
    pub picture_effects: Vec<prproj::convert::prepared::PreparedPictureEffect>,
    pub direct_dnxhr_origins: HashMap<String, i64>,
    pub constant_speeds: Vec<prproj::convert::prepared::PreparedConstantSpeed>,
}
pub struct PreparedPremiereInput {
    pub input:PreparedProjectInput, pub losses:Vec<Loss>,pub diagnostics:Vec<String>,pub ownership:HashMap<String,(usize,usize)>,
}

pub struct PreparationIssue {
    pub message: String,
    pub sequence: Option<String>,
    pub occurrence: Option<PreparationOccurrence<String>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PreparationOccurrence<Id> {
    Item(Id),
    Transition(Id),
    Placement(Id),
    CombinedPlacements { ids: Vec<Id>, truncated: bool },
    Group(Id),
    GroupUse(Id),
}

impl From<String> for PreparationIssue {
    fn from(message: String) -> Self {
        Self { message, sequence: None, occurrence: None }
    }
}

#[derive(Default)]
struct PreparationContext<'a> {
    sequence: Option<&'a str>,
    occurrence: Option<PreparationOccurrence<&'a str>>,
}

impl<'a> PreparationContext<'a> {
    fn at_placement(&mut self, sequence: &'a SourceSequence, placement: &'a Placement) {
        self.sequence = Some(&sequence.id);
        self.occurrence = Some(PreparationOccurrence::Placement(&placement.id));
    }

    fn at_prepared_event(
        &mut self,
        graph: &'a SourceGraph,
        sequence: usize,
        event: Option<usize>,
        ownership: &HashMap<String, (usize, usize)>,
    ) {
        let Some(source) = graph.sequences.get(sequence) else { return };
        self.sequence = Some(&source.id);
        let Some(event) = event else { return };
        const MAX_EVENT_DETAILS: usize = 1024;
        let slot = (sequence, event);
        let mut owners = source.placements.iter()
            .filter(|edit| ownership.get(&edit.id) == Some(&slot))
            .map(|edit| edit.id.as_str());
        let ids: Vec<_> = owners.by_ref().take(MAX_EVENT_DETAILS).collect();
        self.occurrence = match ids.as_slice() {
            [] => None,
            [id] => Some(PreparationOccurrence::Placement(id)),
            _ => Some(PreparationOccurrence::CombinedPlacements {
                ids,
                truncated: owners.next().is_some(),
            }),
        };
    }
}

/// Prepare an Avid job and retain the saved clip or timeline owner of a preparation failure.
pub fn prepare_avid_job(
    graph: &SourceGraph,
    options: &Options,
    clock: ClockProfile,
    spooled: bool,
    assisted: bool,
    managed: Option<(&HashSet<&str>, &HashSet<&str>)>,
) -> Result<PreparedPremiereInput, PreparationIssue> {
    let clock = if managed.is_some() { ClockProfile::LegacyImport } else { clock };
    let linked = assisted || clock == ClockProfile::LegacyImport;
    let mut context = PreparationContext::default();
    prepare_input(graph, options, clock, true, false, !linked || !spooled, linked,
        assisted, managed, &mut context).map_err(|error| PreparationIssue {
            message: error.to_string(),
            sequence: context.sequence.map(str::to_owned),
            occurrence: context.occurrence.map(|occurrence| match occurrence {
                PreparationOccurrence::Item(id) => PreparationOccurrence::Item(id.into()),
                PreparationOccurrence::Transition(id) => PreparationOccurrence::Transition(id.into()),
                PreparationOccurrence::Placement(id) => PreparationOccurrence::Placement(id.into()),
                PreparationOccurrence::Group(id) => PreparationOccurrence::Group(id.into()),
                PreparationOccurrence::GroupUse(id) => PreparationOccurrence::GroupUse(id.into()),
                PreparationOccurrence::CombinedPlacements { ids, truncated } =>
                    PreparationOccurrence::CombinedPlacements {
                        ids: ids.into_iter().map(str::to_owned).collect(),
                        truncated,
                    },
            }),
        })
}

pub fn prepare_originals(g:&SourceGraph,options:&Options,clock:ClockProfile)->R<PreparedPremiereInput>{prepare_input(g,options,clock,true,false,true,false,false,None,&mut PreparationContext::default())}

/// A validated working graph for native publication only. It is not an
/// exportable graph/2 document and has no aggregate wire-encoding bound.
pub fn prepare_linked_originals(g: &SourceGraph, options: &Options, clock: ClockProfile, spooled: bool) -> R<PreparedPremiereInput> {
    prepare_input(g, options, clock, true, false, !spooled, true, false, None, &mut PreparationContext::default())
}

/// Use a verified Premiere donor when standalone media construction does not
/// support a linked original's otherwise representable stream layout.
#[doc(hidden)]
pub fn prepare_assisted_originals(
    g: &SourceGraph, options: &Options, clock: ClockProfile, spooled: bool,
) -> R<PreparedPremiereInput> {
    prepare_input(g, options, clock, true, false, !spooled, true, true, None, &mut PreparationContext::default())
}

pub fn prepare_managed_rewrap(
    graph: &SourceGraph, options: &Options, spooled: bool, assisted: bool,
    selected: &HashSet<&str>, omitted: &HashSet<&str>,
) -> R<PreparedPremiereInput> {
    prepare_input(graph, options, ClockProfile::LegacyImport, true, false, !spooled, true,
        assisted, Some((selected, omitted)), &mut PreparationContext::default())
}

pub fn prepare_resolve_originals(g: &SourceGraph, options: &Options) -> R<PreparedPremiereInput> {
    prepare_input(g, options, ClockProfile::VerifiedRewrap, true, true, true, false, false,
        None, &mut PreparationContext::default())
}
fn loss(out: &mut Vec<Loss>, object: &str, aspect: &str, reason: impl Into<String>) {
    out.push(Loss {
        object: object.into(),
        aspect: aspect.into(),
        reason: reason.into(),
    });
}
struct Routes {
    video: Option<Option<Rational>>,
    video_rate: Option<Rational>,
    video_raster: Option<(i64, i64)>,
    explicit_linked_raster: bool,
    video_duration: Option<Rational>,
    audio: HashMap<i64, Option<Rational>>,
    first_audio: Option<i64>,
}

impl Routes {
    fn supports_corner_pin(&self, rate: Rational, canvas: (i64, i64)) -> bool {
        let picture = self.video_rate.map(fraction).zip(self.video_raster);
        prproj::convert::prepared::CornerPin::supports_geometry(
            fraction(rate), canvas, picture, self.explicit_linked_raster,
        )
    }

    fn chain_geometry(&self, frame_size: (i64, i64)) -> picture::ChainGeometry {
        if self.video_raster != Some(frame_size) {
            return picture::ChainGeometry::Unverified;
        }
        match frame_size {
            (1920, 1080) => picture::ChainGeometry::Hd,
            _ if self.explicit_linked_raster => picture::ChainGeometry::MatchedLinked,
            _ => picture::ChainGeometry::Unverified,
        }
    }
}
type Sidecars<'a> = HashMap<(&'a str, &'a str), &'a Value>;
type NativeMarkers<'a> = HashMap<(&'a str, usize), &'a Value>;
type InheritedKey<'a> = (
    &'a str,
    TrackRef,
    i64,
    Rational,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    i64,
);
type LossIndex<'a> = HashMap<(&'a str, &'a str), Vec<&'a str>>;
struct PreparationFacts<'a> {
    sidecars: Sidecars<'a>,
    same_rate_aac_views: HashMap<&'a str, String>,
    pcm_movie_mono_views: HashMap<&'a str, String>,
    routes: HashMap<&'a str, Routes>,
    items_by_clip: HashMap<&'a str, &'a graph::source::SourceItem>,
    losses: LossIndex<'a>,
    native_markers: NativeMarkers<'a>,
    source_marker_uniqueness: HashMap<InheritedKey<'a>, bool>,
    editable_nested_motion: HashSet<&'a str>,
    omitted_offline_items: HashSet<&'a str>,
    omitted_managed_items: HashSet<&'a str>,
    omitted_picture_items: HashSet<&'a str>,
}
impl PreparationFacts<'_> {
    fn omits_placement(&self, placement: &Placement) -> bool {
        self.omitted_offline_items.contains(placement.item.as_str())
            || (placement.video_track.is_some() && self.omitted_picture_items.contains(placement.item.as_str()))
    }
}
fn source_losses(index: &LossIndex, object: &str, aspect: &str) -> Vec<String> {
    index
        .get(&(object, aspect))
        .into_iter()
        .flatten()
        .map(|s| (*s).to_owned())
        .collect()
}

fn sample_animatte_acceleration50(
    matte: &mut model::Matte,
    frames: i64,
    points_remaining: &mut usize,
) -> bool {
    if !(2..=256).contains(&frames) || matte.shapes.len() > 6 {
        return false;
    }
    let mut points = 0usize;
    let mut animated_points = 0usize;
    for keys in &matte.shapes {
        let Some((0, first)) = keys.first() else {
            return false;
        };
        let Some(total) = points.checked_add(first.len()) else {
            return false;
        };
        points = total;
        if points > 90 || first.len() < 3
            || keys.iter().any(|(_, path)| path.len() != first.len())
        {
            return false;
        }
        if keys.len() > 1 {
            if keys.last().unwrap().0 != frames - 1
                || keys.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            {
                return false;
            }
            animated_points += first.len();
        }
    }
    let cost = animated_points * frames as usize;
    if cost == 0 || cost > *points_remaining {
        return false;
    }
    *points_remaining -= cost;
    for keys in &mut matte.shapes {
        if keys.len() <= 1 {
            continue;
        }
        let mut samples = Vec::with_capacity(frames as usize);
        for pair in keys.windows(2) {
            let (start, from) = &pair[0];
            let (end, to) = &pair[1];
            samples.push((*start, from.clone()));
            for frame in start + 1..*end {
                let t = (frame - start) as f64 / (end - start) as f64;
                // The retained Acceleration 50 render supports this cosine/linear blend.
                let blend = 0.5 * t + 0.25 * (1.0 - (std::f64::consts::PI * t).cos());
                let path = from.iter().zip(to).map(|(a, b)| {
                    std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * blend)
                }).collect();
                samples.push((frame, path));
            }
        }
        samples.push(keys.last().unwrap().clone());
        *keys = samples;
    }
    true
}

fn side<'a>(index: &Sidecars<'a>, adapter: &str, object: &str) -> Option<&'a Value> {
    index.get(&(adapter, object)).copied()
}
fn native_created(native: &Value) -> Option<i64> {
    // A missing timestamp is not evidence that two occurrences are copies.
    key(native, "created").and_then(integer)
}
fn inherited_key<'a>(
    item: &'a str,
    marker: &'a SourceMarker,
    source_track: TrackRef,
    time: i64,
    created: i64,
) -> InheritedKey<'a> {
    let (palette, color) = marker
        .color
        .as_ref()
        .map(|c| (c.palette.as_str(), c.name.as_str()))
        .unwrap_or(("", ""));
    (
        item,
        source_track,
        time,
        marker.duration,
        &marker.name,
        &marker.comment,
        &marker.user,
        palette,
        color,
        created,
    )
}

fn marker_source_track(marker: &SourceMarker, placement: &Placement, route: &Routes) -> Option<TrackRef> {
    let track = marker.track?;
    match track.kind {
        StreamKind::Video if placement.video_track == Some(track.index) => {
            Some(TrackRef { kind: StreamKind::Video, index: 1 })
        }
        StreamKind::Audio if placement.audio_tracks.as_slice() == [track.index] => {
            let index = match placement.audio_streams.as_slice() {
                [index] => *index,
                [] => route.first_audio?,
                _ => return None,
            };
            Some(TrackRef { kind: StreamKind::Audio, index })
        }
        _ => None,
    }
}

// Graphs saved before the source adapter populated master marker tracks
// still have the track witness in the native marker receipt.
fn master_marker_track(marker: &SourceMarker, native: Option<&Value>) -> Option<TrackRef> {
    marker.track.or_else(|| {
        let native = native?;
        let kind = match key(native, "kind").and_then(integer) {
            Some(1) => StreamKind::Video,
            Some(2) => StreamKind::Audio,
            _ => return None,
        };
        let index = key(native, "track").and_then(integer).filter(|&index| index > 0)?;
        Some(TrackRef { kind, index })
    })
}

fn native_marker_index<'a>(g: &'a SourceGraph, sc: &Sidecars<'a>) -> R<NativeMarkers<'a>> {
    let mut native_markers = NativeMarkers::new();
    for (object, count) in g.items.iter().map(|i| (i.id.as_str(), i.markers.len()))
        .chain(g.sequences.iter().map(|s| (s.id.as_str(), s.markers.len())))
    {
        if let Some(Value::Arr(rows)) = side(sc, "avid.source", object)
            .and_then(|v| key(v, "marker_native"))
        {
            for row in rows {
                if let Some(index) = key(row, "graph_index").and_then(integer) {
                    let index = usize::try_from(index)
                        .map_err(|_| error("native marker metadata has negative graph index"))?;
                    if index >= count || native_markers.insert((object, index), row).is_some() {
                        return Err(error(format!("native marker metadata contradicts graph marker references for {object}")));
                    }
                }
            }
        }
    }
    Ok(native_markers)
}
fn old_clock(sc: &Sidecars, m: &SourceMedia) -> R<i64> {
    let Some(raw) = side(sc, "avid.source", &m.id).and_then(|v| key(v, "legacy_clock")) else {
        return Err(error(format!(
            "{}: legacy clock provenance unavailable; corrected source clock cannot be substituted",
            m.id
        )));
    };
    if *raw == Value::Null {
        return Ok(0);
    }
    let count = key(raw, "count")
        .and_then(str_value)
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| error("invalid retained legacy clock count"))?;
    let base = key(raw, "rounded_base")
        .and_then(integer)
        .ok_or_else(|| error("invalid retained legacy clock base"))?;
    let drop = key(raw, "drop")
        .and_then(boolean)
        .ok_or_else(|| error("invalid retained legacy clock drop flag"))?;
    // Exact old code casts u64 to i64 before tick conversion, and falls back to
    // zero for drop, absent base, off-grid or overflowing values.
    Ok(if !drop && base > 0 {
        Rational::from_units(count as i64, base)
            .and_then(|r| r.to_units(TICKS_PER_SECOND))
            .unwrap_or(0)
    } else {
        0
    })
}

/// Direct DNxHD and DNxHR share the graph's AVdn codec label. Require the
/// exact picture descriptor and coding UL observed for each native profile.
fn direct_opatom_clocks(
    item: &SourceItem,
    media: &HashMap<&str, &SourceMedia>,
    sc: &Sidecars,
    separate: bool,
    exact_24_pcm16: bool,
    group_camera: bool,
) -> Option<(i64, i64, &'static str, bool)> {
    direct_opatom_profile(item, media, sc, separate, exact_24_pcm16, group_camera, 2, false)
}

fn direct_opatom_profile(
    item: &SourceItem,
    media: &HashMap<&str, &SourceMedia>,
    sc: &Sidecars,
    separate: bool,
    exact_24_pcm16: bool,
    group_camera: bool,
    mono_lanes: usize,
    allow_partial: bool,
) -> Option<(i64, i64, &'static str, bool)> {
    let rate = item.rate?;
    let pcm16 = (rate.num(), rate.den()) == (24_000, 1001)
        && item.media.iter().any(|binding| {
            let m = media[binding.media.as_str()];
            m.kind == StreamKind::Audio && m.bits == Some(16)
        });
    const DNXHD_LB: &str = "060E2B340401010A0401020271130000";
    const DNXHR_LB_0A: &str = "060E2B340401010A0401020271280000";
    let dnxhd_group_camera = separate && (group_camera || mono_lanes == 0) && !pcm16
        && (rate.num(), rate.den()) == (24_000, 1001)
        && item.media.iter().any(|binding| {
            let m = media[binding.media.as_str()];
            m.kind == StreamKind::Video
                && side(sc, "avid.source", &m.id).and_then(|v| key(v, "picture_coding"))
                    .and_then(str_value) == Some(DNXHD_LB)
        });
    let (base, drop, compression, coding, bits, profile) = match (rate.num(), rate.den()) {
        (24, 1) if exact_24_pcm16 =>
            (24, false, "AVHD", "060E2B340401010A0401020271130000", 16, "24 DNxHD LB/PCM16"),
        (24_000, 1001) if pcm16 => (24, false, "AVHD", "060E2B340401010A0401020271130000", 16, "23.976 DNxHD LB/PCM16"),
        (24_000, 1001) if dnxhd_group_camera =>
            (24, false, "AVHD", DNXHD_LB, 24, if mono_lanes == 0 { "23.976 DNxHD LB" } else { "23.976 DNxHD LB/PCM24" }),
        (24_000, 1001) => (24, false, "RI00", "060E2B340401010D0401020271280000", 24, "23.976 DNxHR LB"),
        (30_000, 1001) => (30, true, "AVHD", "060E2B340401010A0401020271130000", 24, "29.97 DF DNxHD LB"),
        _ => return None,
    };
    if mono_lanes == 3 && (!separate || !group_camera || drop || bits != 24
        || (rate.num(), rate.den()) != (24_000, 1001))
    {
        return None;
    }
    let wide_or_picture_only = matches!(mono_lanes, 0 | 8);
    if wide_or_picture_only && (!separate || (mono_lanes == 8 && !group_camera)
        || (rate.num(), rate.den()) != (24_000, 1001) || pcm16)
    {
        return None;
    }
    let (missing_mono, missing_picture) = if allow_partial && separate && !group_camera && mono_lanes == 2
        && compression == "RI00" && (rate.num(), rate.den()) == (24_000, 1001)
        && item.media.len() < 3
    {
        let mono = match side(sc, "avid.missing_managed_mono", &item.id) {
            Some(receipt) => {
                let (track, package) = missing_managed_track(receipt)?;
                let slot = match track { 1 => 0, 2 => 1, _ => return None };
                Some((slot, package))
            }
            None => None,
        };
        let picture = match side(sc, "avid.missing_managed_picture", &item.id) {
            Some(receipt) => {
                let (1, package) = missing_managed_track(receipt)? else { return None };
                Some(package)
            }
            None => None,
        };
        (mono, picture)
    } else {
        (None, None)
    };
    let absent_packages = [missing_picture, missing_mono.map(|(_, package)| package)];
    let absent_count = absent_packages.iter().flatten().count();
    if item.media.len() + absent_count != mono_lanes + 1 || !item.stream_groups.is_empty()
        || matches!(absent_packages, [Some(picture), Some(mono)] if picture.eq_ignore_ascii_case(mono))
        || side(sc, "avid.source", &item.id).and_then(|v| key(v, "raw_groups"))
            .is_some_and(|v| !matches!(v, Value::Arr(groups) if groups.is_empty()))
    {
        return None;
    }
    let mut picture = None;
    let mut audio = [None; 8];
    let mut paths = HashSet::new();
    let mut packages = HashSet::new();
    for binding in &item.media {
        let m = media[binding.media.as_str()];
        let [path] = m.locations.as_slice() else { return None };
        let clock = m.clock.as_ref()?;
        let start = clock.exact_start()?;
        let origin = Rational::new(
            i128::from(clock.provenance.origin_count) * i128::from(rate.den()),
            i128::from(rate.num()),
        )?;
        let facts = side(sc, "avid.source", &m.id)?;
        let descriptor = key(facts, "descriptor")?;
        let package = key(facts, "file_package").and_then(str_value)?;
        let offline = mono_lanes == 2 && m.online == Some(false) && !drop && !pcm16 && !exact_24_pcm16 && !dnxhd_group_camera
            && missing_picture.is_none()
            && clock.provenance.route == "avid_saved_file_to_physical_timecode";
        if m.original.is_some() || (!offline && m.online != Some(true))
            || binding.source_offset != Some(Rational::ZERO)
            || !path.to_ascii_lowercase().ends_with(".mxf") || !paths.insert(path)
            || package.is_empty() || !packages.insert(package.to_ascii_lowercase())
            || absent_packages.iter().flatten().any(|absent| absent.eq_ignore_ascii_case(package))
            || ((mono_lanes == 3 || wide_or_picture_only || absent_count != 0) && !subclips::mob_id(package))
            || clock.rate != rate || clock.display_base != base || clock.drop != drop
            || (!offline && clock.provenance.route != "file_source_clip_to_physical_timecode_track")
            || m.start != Some(start)
            || m.duration <= Rational::ZERO
        {
            return None;
        }
        if drop && (clock.provenance.origin_count != clock.effective_count
            || clock.provenance.source_offset != 0
            || clock.effective_count >= 2_589_408)
        {
            return None;
        }
        if (pcm16 || exact_24_pcm16) && (clock.provenance.origin_count != clock.effective_count
            || clock.provenance.source_offset != 0
            || clock.effective_count >= 24 * 60 * 60 * 24)
        {
            return None;
        }
        let (slot, units) = match m.kind {
            StreamKind::Video if m.codec.as_deref() == Some("AVdn") && m.rate == Some(rate)
                && m.frame_size == Some((1920, 1080))
                && clock.provenance.source_rate == rate
                && key(descriptor, "compression").and_then(str_value) == Some(compression)
                && (compression != "AVHD" || key(descriptor, "resolution_id").and_then(integer) == Some(1253))
                && matches!(key(descriptor, "display_aspect"),
                    Some(Value::Arr(a)) if a.as_slice() == [Value::Int(16), Value::Int(9)])
                && (offline || key(facts, "picture_coding").and_then(str_value) == Some(coding)
                    || (wide_or_picture_only && compression == "RI00"
                        && key(facts, "picture_coding").and_then(str_value) == Some(DNXHR_LB_0A))) =>
            {
                let units = Rational::new(
                    i128::from(m.duration.num()) * i128::from(rate.num()),
                    i128::from(m.duration.den()) * i128::from(rate.den()),
                )?.to_units(1)?;
                (&mut picture, units)
            }
            StreamKind::Audio if m.sample_rate == Some(48_000) && m.bits == Some(bits)
                && clock.provenance.source_rate == Rational::integer(48_000)
                && (!(pcm16 || exact_24_pcm16) || key(descriptor, "bits").and_then(integer) == Some(16))
                && (!(dnxhd_group_camera || mono_lanes >= 3 || missing_picture.is_some())
                    || key(descriptor, "bits").and_then(integer) == Some(24))
                && key(descriptor, "channels").and_then(integer) == Some(1) =>
            {
                let label = usize::try_from(m.stream?).ok()?;
                let slot = audio[..mono_lanes].get_mut(label.checked_sub(1)?)?;
                (slot, m.duration.to_units(48_000)?)
            }
            _ => return None,
        };
        if key(facts, "descriptor_length").and_then(integer) != Some(units)
            || slot.replace((start, origin, m.duration)).is_some()
        {
            return None;
        }
    }
    let (start, origin, duration) = if missing_picture.is_some() {
        if picture.is_some() { return None; }
        audio[..mono_lanes].iter().flatten().copied().next()?
    } else {
        picture?
    };
    if audio[..mono_lanes].iter().enumerate().any(|(index, lane)| {
        let absent = missing_mono.is_some_and(|(missing, _)| missing == index);
        *lane != if absent { None } else { Some((start, origin, duration)) }
    }) {
        return None;
    }
    Some((start.to_units(TICKS_PER_SECOND)?, origin.to_units(TICKS_PER_SECOND)?, profile, dnxhd_group_camera))
}

fn missing_managed_track(receipt: &Value) -> Option<(i64, &str)> {
    let track = integer(key(receipt, "track")?)?;
    let package = str_value(key(receipt, "file_package")?).filter(|package| subclips::mob_id(package))?;
    Some((track, package))
}

fn linked_package_key<'a>(
    item: &'a SourceItem, media: &HashMap<&str, &'a SourceMedia>,
) -> Option<(&'a str, [&'a str; 3])> {
    let mob = item.umid.as_deref().filter(|mob| subclips::mob_id(mob))?;
    let [a, b, c] = item.media.as_slice() else { return None };
    let path = |media_id: &str| {
        let [path] = media[media_id].locations.as_slice() else { return None };
        Some(path.as_str())
    };
    let mut paths = [path(&a.media)?, path(&b.media)?, path(&c.media)?];
    paths.sort_unstable();
    Some((mob, paths))
}

fn display(rate: Option<Rational>, drop: bool) -> Option<i64> {
    match rate.map(|r| (r.num(), r.den()))? {
        (24, 1) => Some(100),
        (25, 1) => Some(101),
        (30000, 1001) => Some(if drop { 102 } else { 103 }),
        (30, 1) => Some(104),
        (50, 1) => Some(105),
        (60000, 1001) => Some(if drop { 106 } else { 107 }),
        (60, 1) => Some(108),
        (24000, 1001) => Some(110),
        (48, 1) => Some(113),
        _ => None,
    }
}
fn marker(
    m: &SourceMarker, losses: &mut Vec<Loss>, object: &str, index: usize, in_sequence: bool,
) -> R<Option<Marker>> {
    let mut timing_loss = |reason: &str| {
        loss(
            losses,
            object,
            "marker_timing",
            format!("{object}/marker/{index}: marker {:?}: {reason}", m.name),
        );
    };
    let Some(start) = m.start else {
        timing_loss("owner-to-record transform unresolved");
        return Ok(None);
    };
    let start = match ticks(start, "marker start") {
        Ok(t) => t,
        Err(e) => {
            timing_loss(&e.to_string());
            return Ok(None);
        }
    };
    let duration = match ticks(m.duration, "marker duration") {
        Ok(v) => v,
        Err(e) => {
            timing_loss(&e.to_string());
            return Ok(None);
        }
    };
    if start.checked_add(duration).is_none() {
        timing_loss("record span exceeds destination tick range");
        return Ok(None);
    }
    let mut owner = m.owner;
    let source = match m.source.map(|s| ticks(s, "marker source")).transpose() {
        Ok(source) => source,
        Err(e) if in_sequence && owner == MarkerOwner::Edit => {
            loss(
                losses,
                object,
                "marker_ownership",
                format!(
                    "{object}/marker/{index}: marker {:?}: record range preserved on sequence; {e}",
                    m.name,
                ),
            );
            owner = MarkerOwner::Sequence;
            None
        }
        Err(e) => {
            timing_loss(&e.to_string());
            return Ok(None);
        }
    };
    let color = match m.color.as_ref() {
        Some(label) if label.palette == "avid" => label.name.clone(),
        Some(label) => {
            let destination = match (m.owner, label.palette.as_str(), label.name.as_str()) {
                (
                    MarkerOwner::Sequence | MarkerOwner::Edit,
                    "resolve",
                    name @ ("Green" | "Red" | "Blue" | "Cyan" | "Yellow"),
                ) => Some(name),
                (MarkerOwner::Sequence | MarkerOwner::Edit, "resolve", "Lavender") => Some("Magenta"),
                (MarkerOwner::Sequence | MarkerOwner::Edit, "resolve", "Sand") => Some("Orange"),
                (MarkerOwner::Sequence | MarkerOwner::Edit, "resolve", "Cream") => Some("White"),
                _ => None,
            };
            let change = match destination {
                Some(name) if name == label.name => None,
                Some(name) => Some(format!("mapped to Premiere {name}")),
                None => Some("Premiere default retained".into()),
            };
            if let Some(change) = change {
                loss(
                    losses,
                    object,
                    "marker_color",
                    format!(
                        "{}: {} color {}: {change}",
                        m.owner_occurrence, label.palette, label.name,
                    ),
                );
            }
            destination.unwrap_or_default().into()
        }
        None => String::new(),
    };
    Ok(Some(Marker {
        name: m.name.clone(),
        comment: m.comment.clone(),
        user: m.user.clone(),
        color,
        start,
        duration,
        track: m.track.map(|t| {
            format!(
                "{}{}",
                if t.kind == StreamKind::Video {
                    'V'
                } else {
                    'A'
                },
                t.index
            )
        }),
        owner: owner.as_str().into(),
        event: None,
        source,
        inherited: m.inherited && owner == m.owner,
    }))
}
fn normalize_rewrap_clocks(
    g: &SourceGraph,
    sc: &Sidecars,
    clips: &mut model::ClipMap,
    diagnostics: &mut Vec<String>,
    only: Option<&HashSet<&str>>,
) -> R<()> {
    let media: HashMap<_, _> = g.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut shared = HashMap::new();
    for item in &g.items {
        if only.is_some_and(|items| !items.contains(item.id.as_str())) { continue; }
        let Some(c) = clips.get_mut(&clip_id(&item.id)) else {
            // Linked originals already use their own physical stream clocks.
            // They do not enter the native MXF model, even in rewrap mode.
            if !item.media.is_empty()
                && item.media.iter().all(|link| media[link.media.as_str()].original.is_some())
            {
                continue;
            }
            return Err(error(format!("{}: native media model is missing a clip", item.name)));
        };
        let Some(video) = &mut c.video else { continue };
        let rate = video.rate;
        if !prproj::convert::rewrap::supported_rate(rate) {
            return Err(error(format!("unsupported MXF rewrap rate {rate}")));
        }
        let unit = model::ticks_per_unit(rate);
        if video.codec == "apch" && item.media.first().is_some_and(|binding|
            side(sc, "avid.source", &binding.media).and_then(|v| key(v, "legacy_file_clock"))
                == Some(&Value::Null)) {
            let [binding] = item.media.as_slice() else {
                return Err(error("generated Freeze Frame requires one picture binding"));
            };
            let m = media[binding.media.as_str()];
            let clock = side(sc, "avid.source", &m.id).and_then(|v| key(v, "legacy_file_clock"));
            if video.start != 0 || m.start.is_some() || m.clock.is_some() || clock != Some(&Value::Null) {
                return Err(error("generated Freeze Frame must use a zero-relative picture clock without source timecode"));
            }
            c.time_display = Some(i64::from(
                prproj::convert::rewrap::display_format(rate, false).ok_or_else(|| error("unsupported freeze rate"))?,
            ));
            continue;
        }
        let mut drop = None;
        for im in &item.media {
            let m = media[im.media.as_str()];
            let raw = side(sc, "avid.source", &m.id)
                .and_then(|v| key(v, "legacy_file_clock"))
                .filter(|v| **v != Value::Null)
                .ok_or_else(|| {
                    error(format!(
                        "{}: missing/ambiguous native source timecode",
                        m.id
                    ))
                })?;
            let count = key(raw, "count")
                .and_then(str_value)
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| error("invalid native file clock count"))?;
            let base = key(raw, "rounded_base")
                .and_then(integer)
                .ok_or_else(|| error("invalid native clock base"))?;
            let df = key(raw, "drop")
                .and_then(boolean)
                .ok_or_else(|| error("invalid native drop flag"))?;
            if i128::from(base)
                != (i128::from(rate.num) + i128::from(rate.den) / 2) / i128::from(rate.den)
            {
                return Err(error("source timecode base disagrees with picture rate"));
            }
            if drop.replace(df).is_some_and(|d| d != df) {
                return Err(error("source drop-frame flags disagree"));
            }
            let exact = i64::try_from(i128::from(count) * i128::from(unit))
                .map_err(|_| error("source start overflow"))?;
            if exact < 0 || prproj::convert::rewrap::display_format(rate, df).is_none() {
                return Err(error("unsupported rate/drop-frame combination"));
            }
            let tc = prproj::build::time::Time::new(exact)
                .to_timecode(unit, df)
                .map_err(|e| error(e.to_string()))?;
            if tc
                .split(':')
                .next()
                .and_then(|h| h.parse::<u32>().ok())
                .filter(|h| *h < 24)
                .is_none()
            {
                return Err(error("timecode outside one day"));
            }
            let path = m
                .locations
                .first()
                .ok_or_else(|| error("source location missing"))?;
            if shared
                .insert(path.clone(), (rate, exact))
                .is_some_and(|v| v != (rate, exact))
            {
                return Err(error("shared source has conflicting picture clocks"));
            }
            let start = if m.kind == StreamKind::Video {
                &mut video.start
            } else {
                &mut c
                    .audio
                    .iter_mut()
                    .find(|a| Some(a.avid_track.unwrap_or(0)) == m.stream && a.path == *path)
                    .ok_or_else(|| error("native audio binding mismatch"))?
                    .start
            };
            if *start != exact {
                diagnostics.push(format!(
                    "rewrap source clock normalized from native frame count at {rate} fps: {path}"
                ));
                *start = exact;
            }
        }
        c.time_display = Some(i64::from(
            prproj::convert::rewrap::display_format(rate, drop.unwrap_or(false))
                .ok_or_else(|| error("unsupported drop-frame rate"))?,
        ));
    }
    Ok(())
}

fn has_unmodified_original_stereo_pair(clip: &OriginalClip, raw: Option<&Value>) -> bool {
    // The admitted movie/AAC profile requires a physically probed stereo layout.
    // The saved Avid facts must also witness ordinary A1=left, A2=right with no
    // custom master mix, gain difference, or explicit pan.
    if clip.container != "mov"
        || !clip.audio.as_ref().is_some_and(|audio| {
            audio.channels == 2 && audio.representation == SampleRepresentation::Aac
        })
        || clip.channels.len() != 2
        || clip.channels[0].gain_db != clip.channels[1].gain_db
    {
        return false;
    }
    let Some(raw) = raw else { return false };
    if !matches!(key(raw, "raw_groups"), Some(Value::Arr(groups)) if groups.is_empty())
        || key(raw, "master_output_mix")
            .and_then(|mix| key(mix, "type"))
            .and_then(str_value) != Some("absent")
    {
        return false;
    }
    let Some(Value::Arr(tracks)) = key(raw, "tracks") else { return false };
    [1, 2].into_iter().all(|lane| {
        if !clip.channels.iter().any(|c| c.avid_track == lane && c.channel_index == lane - 1) {
            return false;
        }
        // AVB kind 2 is sound. Picture can share the same track label.
        let mut matching = tracks.iter().filter(|t| {
            key(t, "kind").and_then(integer) == Some(2)
                && key(t, "label").and_then(integer) == Some(lane)
        });
        matches!(matching.next().and_then(|t| key(t, "pan_raw")), Some(Value::Null))
            && matching.next().is_none()
    })
}

fn output_mix_notice(value: &Value) -> R<Option<String>> {
    let malformed = || error("malformed avid.source master_output_mix observation");
    let Value::Obj(fields) = value else { return Err(malformed()); };
    if fields.len() != 2 { return Err(malformed()); }
    let kind = key(value, "type").and_then(str_value).ok_or_else(malformed)?;
    let raw = key(value, "raw").ok_or_else(malformed)?;
    let description = match (kind, raw) {
        ("absent", Value::Null) => return Ok(None),
        ("integer", Value::Int(n)) => match n {
            4 => "integer 4 (observed 5.1 Sequence association)".into(),
            5 => "integer 5 (observed 7.1 Sequence association)".into(),
            _ => format!("uninterpreted integer {n}"),
        },
        ("text", Value::Str(v)) => format!("uninterpreted text ({} UTF-8 bytes)", v.len()),
        ("bytes_hex", Value::Str(v)) if v.len()%2 == 0 && v.bytes().all(|b| b.is_ascii_hexdigit()) => format!("uninterpreted bytes ({} bytes)", v.len()/2),
        ("bin_object_ref", Value::Int(n)) if (1..=u32::MAX as i64).contains(n) => format!("unresolved bin-local object reference {n}"),
        ("null", Value::Null) => "explicit null".into(),
        _ => return Err(malformed()),
    };
    Ok(Some(format!("Master Clip Output Mix Format: _AUDIO_OUTPUT_FORMAT {description}; raw setting retained in graph/2; Avid output mixing is not applied; destination routing follows channel groups")))
}

// A timeline combiner uses two mono parts even when the bin master has no
// stereo declaration. Supply the pair to Separate files preparation only.
fn timeline_stereo_items(g: &SourceGraph) -> (HashSet<&str>, HashSet<&str>) {
    let mut qualified = HashSet::new();
    let mut unsupported = HashSet::new();
    for sequence in &g.sequences {
        for edit in &sequence.placements {
            if edit.grouped_audio.is_empty() {
                continue;
            }
            if sequence.rate == Rational::integer(24) && edit.speed.is_none()
                && edit.video_track.is_none() && edit.frame_map.is_none()
                && edit.audio_tracks.is_empty() && edit.volume_keys.is_empty()
                && edit.volume_db.is_none() && edit.gain_db == 0.0
                && matches!(edit.grouped_audio.as_slice(), [pair]
                    if pair.streams == [1, 2] && pair.gain_db.is_none()
                    && pair.pans.iter().all(Option::is_none))
            {
                qualified.insert(edit.item.as_str());
            } else {
                unsupported.insert(edit.item.as_str());
            }
        }
    }
    qualified.retain(|item| !unsupported.contains(item));
    (qualified, unsupported)
}

fn verified_stereo_siblings(
    item: &SourceItem,
    media: &HashMap<&str, &SourceMedia>,
    sidecars: &Sidecars,
) -> bool {
    if item.rate != Some(Rational::integer(24)) || !item.stream_groups.is_empty() {
        return false;
    }
    let mut audio = item.media.iter()
        .filter(|binding| media[binding.media.as_str()].kind == StreamKind::Audio);
    let (Some(first), Some(second), None) = (audio.next(), audio.next(), audio.next()) else {
        return false;
    };
    let tracks = [media[first.media.as_str()], media[second.media.as_str()]];
    if first.gain_db != 0.0 || second.gain_db != 0.0
        || tracks[0].duration != tracks[1].duration || tracks[0].locations == tracks[1].locations
        || !matches!(tracks.map(|m| m.stream), [Some(1), Some(2)] | [Some(2), Some(1)])
    {
        return false;
    }
    let package = |m: &SourceMedia| {
        side(sidecars, "avid.source", &m.id)
            .and_then(|facts| key(facts, "file_package")).and_then(str_value)
    };
    let (Some(left), Some(right)) = (package(tracks[0]), package(tracks[1])) else {
        return false;
    };
    !left.is_empty() && !right.is_empty() && left != right
        && tracks.iter().all(|m| {
            let descriptor = side(sidecars, "avid.source", &m.id).and_then(|v| key(v, "descriptor"));
            m.original.is_none() && m.online == Some(true)
                && m.locations.len() == 1 && m.locations[0].to_ascii_lowercase().ends_with(".mxf")
                && m.sample_rate == Some(48_000) && m.bits == Some(16)
                && descriptor.and_then(|v| key(v, "channels")).and_then(integer) == Some(1)
                && old_clock(sidecars, m).ok() == Some(0)
                && m.start == Some(Rational::ZERO)
                && m.clock.as_ref().is_some_and(|c|
                    c.rate == Rational::integer(24) && !c.drop && c.display_base == 24
                        && c.exact_start() == Some(Rational::ZERO)
                        && c.provenance.source_offset == 0
                        && c.provenance.origin_count == 0
                        && c.provenance.source_rate == Rational::integer(48_000)
                        && c.provenance.route == "file_source_clip_to_physical_timecode_track")
                && m.duration > Rational::ZERO
        })
}

pub fn has_avid_filler_fade(g: &SourceGraph) -> bool {
    g.sidecars.iter().any(|sidecar| sidecar.adapter == "avid.source"
        && matches!(key(&sidecar.data, "native_transitions"), Some(Value::Arr(rows))
            if rows.iter().any(|row| key(row, "incoming_filler_frames").and_then(integer).is_some()
                && key(row, "effect").and_then(str_value) == Some("BLEND_AUDIO_DISSOLVE"))))
}

fn avid_stereo_fade_sources<'a>(
    g: &'a SourceGraph, media: &HashMap<&str, &SourceMedia>, sidecars: &Sidecars,
) -> HashSet<&'a str> {
    let rate = Rational::new(24_000, 1001).unwrap();
    g.items.iter().filter(|item| {
        let [left, right] = item.media.as_slice() else { return false };
        let [a, b] = [media[left.media.as_str()], media[right.media.as_str()]];
        let package = |m: &SourceMedia| side(sidecars, "avid.source", &m.id)
            .and_then(|v| key(v, "file_package")).and_then(str_value);
        item.rate == Some(rate) && item.stream_groups == [vec![1, 2]]
            && left.gain_db == right.gain_db && a.duration == b.duration
            && a.start == b.start && a.locations != b.locations
            && matches!([a.stream, b.stream], [Some(1), Some(2)] | [Some(2), Some(1)])
            && package(a).zip(package(b)).is_some_and(|(x, y)|
                subclips::mob_id(x) && subclips::mob_id(y) && x != y)
            && [(left, a), (right, b)].iter().all(|(binding, m)| {
                m.kind == StreamKind::Audio && m.original.is_none() && m.online == Some(true)
                    && m.sample_rate == Some(48_000) && m.bits == Some(24)
                    && m.locations.len() == 1 && binding.source_offset == Some(Rational::ZERO)
                    && m.clock.as_ref().is_some_and(|c| c.rate == rate && !c.drop
                        && c.provenance.source_rate == Rational::integer(48_000)
                        && c.provenance.route == "file_source_clip_to_physical_timecode_track")
            })
    }).map(|item| item.id.as_str()).collect()
}

/// A direct caller should use typed preparation when a disabled edit may be recovered.
pub fn has_disabled_edit_candidates(g: &SourceGraph) -> bool {
    g.sequences.iter().any(|sequence| !sequence.disabled_placements.is_empty())
        || g.sidecars.iter().any(|receipt| receipt.adapter == "avid.source"
        && matches!(key(&receipt.data, "unresolved"), Some(Value::Arr(issues)) if issues.iter()
            .any(|issue| key(issue, "neutral_picture_file").and_then(str_value).is_some_and(subclips::mob_id)
                || ["plain_mono_backing", "plain_stereo_backing"].into_iter()
                    .any(|flag| key(issue, flag).and_then(boolean) == Some(true)))))
}

/// Managed clip pan requires the typed Premiere writer.
pub fn has_managed_pan_candidates(g: &SourceGraph) -> bool {
    let has_portable_pan = g.sequences.iter().flat_map(|seq| &seq.placements)
        .any(|edit| edit.effects.iter().any(|effect| effect.kind == graph::audio_pan::LINEAR_KIND));
    if !has_portable_pan && !g.losses.iter().any(|loss| loss.aspect == "pan_automation"
        && loss.reason == pan::SOURCE_OMISSION)
    {
        return false;
    }
    let media = g.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let sidecars = g.sidecars.iter().map(|s|
        ((s.adapter.as_str(), s.object.as_str()), &s.data)).collect();
    !pan::managed_sources(g, &media, &sidecars).is_empty()
}

pub fn has_track_name_candidates(graph: &SourceGraph) -> bool {
    graph.sequences.iter().any(|seq| seq.tracks.iter().any(|track| track.name.is_some()))
        || graph.sidecars.iter().any(|receipt| receipt.adapter == track_names::ADAPTER)
}

pub fn prepare(g: &SourceGraph, options: &Options, clock: ClockProfile) -> R<PreparedPremiere> {
    if !g.multicams.is_empty() || !g.multicam_uses.is_empty() {
        return Err(error("multicam_native_route_required: editable Groups require typed native rewrap preparation"));
    }
    let p=prepare_input(g,options,clock,false,false,true,false,false,None,&mut PreparationContext::default())?;
    let model=Model{project_name:p.input.project_name,options:p.input.options,clips:p.input.clips.into_iter().map(|c|match c{PreparedClip::Legacy(c)=>c,_=>unreachable!()}).collect(),bins:p.input.bins,sequences:p.input.sequences};
    model::validate_model(&model).map_err(|e|error(e.to_string()))?;
    Ok(PreparedPremiere{model,losses:p.losses,diagnostics:p.diagnostics,ownership:p.ownership,texts:p.input.texts,video_dissolves:p.input.video_dissolves,picture_effects:p.input.picture_effects,direct_dnxhr_origins:p.input.direct_dnxhr_origins,constant_speeds:p.input.constant_speeds})
}
fn prepare_input<'a>(
    g: &'a SourceGraph, options: &Options, clock: ClockProfile, allow_originals: bool,
    allow_original_speed: bool, check_encoding: bool, best_effort: bool, assisted: bool,
    managed_rewrap: Option<(&HashSet<&str>, &HashSet<&str>)>, context: &mut PreparationContext<'a>,
) -> R<PreparedPremiereInput> {
    g.validate()?;
    anynle_media_verification::linked_original::qualify(&g.media)?;
    if check_encoding {
        g.encoded_len()?;
    }
    if g.items.len() > model::MAX_ITEMS
        || g.bins.len() > model::MAX_ITEMS
        || g.sequences.len() > model::MAX_ITEMS
        || g.sequences
            .iter()
            .any(|s| s.placements.len() > model::MAX_ITEMS)
    {
        return Err(error(
            "Premiere preparation exceeds destination item/event count limit",
        ));
    }
    let sc: Sidecars = g
        .sidecars
        .iter()
        .map(|s| ((s.adapter.as_str(), s.object.as_str()), &s.data))
        .collect();
    let mut losses = Vec::new();
    let mut diagnostics=vec![match clock {
        ClockProfile::LegacyImport=>"legacy-import media clocks retained; sequence start uses a qualified sequence clock or the compatibility origin",
        ClockProfile::VerifiedRewrap=>"native rewrap media clocks retained; sequence start uses a qualified sequence clock or the compatibility origin",
    }.into()];
    let media: HashMap<_, _> = g.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let layered_candidates = if allow_originals && clock == ClockProfile::LegacyImport && options.bin_mode == "separate" {
        nests::layered_candidates(g, &sc, &media)
    } else {
        Vec::new()
    };
    let layered_sources: HashSet<_> = layered_candidates.iter()
        .flat_map(|candidate| candidate.draft.layers.iter().map(|layer| layer.clip.as_str()))
        .collect();
    let layered_packages: HashSet<_> = if layered_sources.is_empty() {
        HashSet::new()
    } else {
        g.items.iter().filter(|item| layered_sources.contains(clip_id(&item.id).as_str()))
            .filter_map(|item| linked_package_key(item, &media))
            .collect()
    };
    let mut clips = Vec::new();
    let mut ids = HashSet::new();
    let native_markers = native_marker_index(g, &sc)?;
    let mut direct_dnxhr_origins = HashMap::new();
    let mut direct_dnxhd_pcm24 = HashSet::new();
    let mut same_rate_aac_views = HashMap::new();
    let mut pcm_movie_mono_views = HashMap::new();
    let mut explicit_linked_items = HashSet::new();
    let mut mono_view_ids = Vec::new();
    let mut group_cameras = HashMap::new();
    let media_counts: HashMap<_, _> = g.items.iter()
        .map(|item| (item.id.as_str(), item.media.len())).collect();
    for group in &g.multicams {
        let audio_count = group.lanes.iter().filter(|lane| lane.kind == StreamKind::Audio).count();
        for item in group.lanes.iter().filter(|lane| lane.kind == StreamKind::Video)
            .flat_map(|lane| &lane.alternatives).flat_map(|alt| &alt.pieces)
            .filter_map(|piece| piece.item.as_deref())
        {
            let count = group_cameras.entry(item).or_insert(2);
            if audio_count == 3 {
                *count = 3;
            } else if audio_count == 1 && media_counts.get(item) == Some(&9) {
                *count = 8;
            }
        }
    }
    let mut direct_drop_frame_items = HashSet::new();
    let mut omitted_offline_items = HashSet::new();
    let mut omitted_managed_items = HashSet::new();
    let mut omitted_picture_items = HashSet::new();
    let combined_items: HashSet<_> = if managed_rewrap.is_some() {
        g.sequences.iter().flat_map(|seq| seq.placements.iter().chain(&seq.disabled_placements))
            .filter(|p| p.video_track.is_some() && (!p.audio_tracks.is_empty() || !p.grouped_audio.is_empty()))
            .map(|p| p.item.as_str()).collect()
    } else {
        HashSet::new()
    };
    let (timeline_stereo, unsupported_stereo) = if clock == ClockProfile::LegacyImport && options.bin_mode == "separate" {
        timeline_stereo_items(g)
    } else {
        (HashSet::new(), HashSet::new())
    };
    let items_used_for_sequence_audio: HashSet<_> = g.sequences.iter()
        .flat_map(|s| &s.placements)
        .filter(|p| !p.audio_tracks.is_empty() || !p.grouped_audio.is_empty())
        .map(|p| p.item.as_str())
        .collect();
    let items_used_for_mono_audio: HashSet<_> = g.sequences.iter()
        .flat_map(|s| &s.placements)
        .filter(|p| !p.audio_tracks.is_empty())
        .map(|p| p.item.as_str())
        .collect();
    let combined_mode = matches!(options.bin_mode.as_str(), "grouped" | "merged");
    let incompatible_combined_24: HashSet<_> = if combined_mode {
        g.sequences.iter().flat_map(|seq| seq.placements.iter().filter_map(|edit|
            (seq.rate != Rational::integer(24) || !edit.grouped_audio.is_empty())
                .then_some(edit.item.as_str()))).collect()
    } else {
        HashSet::new()
    };
    for item in &g.items {
        context.occurrence = Some(PreparationOccurrence::Item(&item.id));
        let id = clip_id(&item.id);
        if !ids.insert(id.clone()) {
            return Err(error("prepared clip identity collision"));
        }
        let omit_managed = managed_rewrap.is_some_and(|(_, omitted)| omitted.contains(item.id.as_str()));
        let keep_mono = omit_managed && options.bin_mode == "separate"
            && !combined_items.contains(item.id.as_str())
            && !group_cameras.contains_key(item.id.as_str()) && !layered_sources.contains(id.as_str())
            && item.stream_groups.is_empty() && item.media.iter().any(|im| media[im.media.as_str()].kind == StreamKind::Audio)
            && item.media.iter().filter(|im| media[im.media.as_str()].kind == StreamKind::Audio).all(|im| {
                let audio = media[im.media.as_str()];
                audio.original.is_none() && audio.online == Some(true) && audio.sample_rate == Some(48_000)
                    && matches!(audio.bits, Some(16 | 24)) && im.source_offset == Some(Rational::ZERO)
                    && side(&sc, "avid.source", &audio.id).and_then(|v| key(v, "descriptor"))
                        .and_then(|v| key(v, "channels")).and_then(integer) == Some(1)
            });
        if omit_managed {
            omitted_managed_items.insert(item.id.as_str());
            if keep_mono {
                omitted_picture_items.insert(item.id.as_str());
            } else {
                omitted_offline_items.insert(item.id.as_str());
            }
            loss(&mut losses, &item.id, "managed_media_omitted",
                if keep_mono { format!("Omitted picture for {:?}. Independent mono audio stays linked.", item.name) }
                else { format!("Omitted managed clip {:?} without a qualified selected media route.", item.name) });
            if !keep_mono { continue; }
        }
        let mut video = None;
        let mut audio = Vec::new();
        let mut original:Option<OriginalClip>=None;
        let mut same_rate_aac_channels = 0;
        for im in &item.media {
            let m = media[im.media.as_str()];
            let Some(receipt) = side(&sc, "avid.source", &m.id) else { continue; };
            let Some(origin) = key(receipt, "same_rate_aac_file_track_start") else { continue; };
            if m.original.is_none() || m.kind != StreamKind::Audio || integer(origin) != Some(1)
                || key(receipt, "verified_aac_priming_samples").and_then(integer) != Some(1024)
            {
                return Err(error(format!("{}: invalid linked AAC source origin receipt", item.id)));
            }
            same_rate_aac_channels += 1;
        }
        let picture_missing = sc.contains_key(&("avid.missing_managed_picture", item.id.as_str()));
        let track_count = group_cameras.get(item.id.as_str()).copied()
            .unwrap_or(if item.media.len() == 1 && !picture_missing { 0 } else { 2 });
        let direct_clocks = if clock == ClockProfile::LegacyImport && !keep_mono {
            let combined_24 = combined_mode
                && item.rate == Some(Rational::integer(24))
                && !incompatible_combined_24.contains(item.id.as_str());
            direct_opatom_profile(item, &media, &sc, options.bin_mode == "separate", combined_24,
                group_cameras.contains_key(item.id.as_str()) || layered_sources.contains(id.as_str())
                    || (!layered_packages.is_empty()
                        && linked_package_key(item, &media).is_some_and(|key| layered_packages.contains(&key))),
                track_count, allow_originals)
                .filter(|&(start, origin, _, _)| !combined_24 || (start == 0 && origin == 0))
        } else {
            None
        };
        if picture_missing && direct_clocks.is_none() {
            return Err(error(format!(
                "{}: retaining managed audio without picture requires verified PCM24 MXF siblings with matching file clocks and extents",
                item.name
            )));
        }
        if item.media.iter().any(|im| {
            let m = media[im.media.as_str()];
            m.online == Some(false) && m.original.is_none()
        }) && direct_clocks.is_none() {
            if best_effort && !check_encoding
                && !layered_sources.contains(id.as_str())
                && item.media.iter().all(|im| {
                    let m = media[im.media.as_str()];
                    m.online == Some(false) && m.original.is_none()
                })
            {
                omitted_offline_items.insert(item.id.as_str());
                loss(&mut losses, &item.id, "omitted_offline_bin_item",
                    format!("Omitted offline managed clip {:?} without a qualified Premiere link.", item.name));
                continue;
            }
            if group_cameras.get(item.id.as_str()) == Some(&3) {
                return Err(error(format!("{}: three-mono Group cameras need all four original files online", item.name)));
            }
            return Err(error(format!("{}: offline managed clips require one qualified picture and two matching mono siblings", item.name)));
        }
        if let Some((_, origin, profile, dnxhd_group_camera)) = direct_clocks {
            direct_dnxhr_origins.insert(id.clone(), origin);
            if dnxhd_group_camera {
                direct_dnxhd_pcm24.insert(id.clone());
            }
            if item.rate == Rational::new(30_000, 1001) {
                direct_drop_frame_items.insert(item.id.as_str());
            }
            let lanes = match track_count {
                0 => "picture only",
                3 => "discrete A1/A2/A3",
                8 => "discrete A1–A8",
                _ if picture_missing && item.media.len() == 1 => "one available mono sibling with missing picture",
                _ if picture_missing => "discrete A1/A2 with missing picture",
                _ if item.media.len() == 2 => "one available mono sibling",
                _ => "discrete A1/A2",
            };
            diagnostics.push(format!("{}: direct {profile} OP-Atom with {lanes}, separate file origin and clip timecode, and no new media", item.name));
        }
        let original_count=item.media.iter().filter(|im|media[im.media.as_str()].original.is_some()).count();
        if original_count>0 && (!allow_originals || original_count!=item.media.len()){return Err(error("original-file media require the typed prepared route with complete original bindings"));}
        let dual_mono = original_count == 3 && item.rate.is_some_and(|r|
            matches!((r.num(), r.den()), (25, 1) | (30_000, 1001)))
            && item.stream_groups.is_empty()
            && [1, 2].into_iter().all(|lane| item.media.iter().any(|im| {
                let m = media[im.media.as_str()];
                m.kind == StreamKind::Audio && m.stream == Some(lane) && m.original.as_ref().is_some_and(|o|
                    matches!(o.container.as_str(), "mov" | "mxf") && o.sample_representation.as_deref() == Some("pcm24")
                        && o.channels == Some(1) && o.channel_index == Some(0) && o.stream_index == lane)
            }));
        for im in &item.media {
            if keep_mono && media[im.media.as_str()].kind == StreamKind::Video { continue; }
            match im.source_offset {
                None=>return Err(error(format!("unresolved_source_offset {} -> {}: bin/master construction cannot assume an unresolved item-to-file origin is zero",item.id,im.media))),
                Some(offset) if offset!=Rational::ZERO=>return Err(error(format!("nonzero_source_offset {} -> {}: origin {offset} retained; current bin/master preparation requires separate qualification",item.id,im.media))),
                _=>{},
            }
            let m = media[im.media.as_str()];
            if m.kind == StreamKind::Video && video.is_some() {
                return Err(error(format!("multiple_picture_streams {}: Premiere clip preparation requires a qualified single picture stream",item.id)));
            }
            let path = m
                .locations
                .first()
                .ok_or_else(|| error(format!("{}: no media location", m.id)))?
                .clone();
            if m.locations.len() > 1 {
                loss(&mut losses,&m.id,"locations",format!("{} further source locations are not represented by the current single-path writer",m.locations.len()-1));
            }
            if let Some(o)=&m.original {
                if m.locations.len()!=1{return Err(error("original stream requires one unambiguous qualified physical path"));}
                let c=original.get_or_insert_with(||OriginalClip{premiere_assisted:false,proxy_path:None,separate_mono_streams:dual_mono,metadata:ClipMetadata{id:id.clone(),name:item.name.clone(),material_umid:item.umid.clone(),tape_name:item.tape.clone(),avid:None,time_display:None,rate:item.rate.map(fraction),markers:vec![]},path:path.clone(),container:o.container.clone(),video:None,audio:None,channels:vec![],groups:vec![]});
                if c.path!=path||c.container!=o.container{return Err(error("one original master must resolve to one physical file"));}
                let duration=ticks(m.duration,"original stream duration")?;
                match m.kind {
                    StreamKind::Video=>{
                        if c.video.is_some(){return Err(error("multiple original picture streams"));}
                        let (Some(codec),Some((width,height)),Some(rate))=(&m.codec,m.frame_size,m.rate)else{return Err(error("original picture facts incomplete"));};
                        let source_start = m.start.filter(|_| m.clock.as_ref().is_some_and(|clock|
                            matches!(clock.provenance.route.as_str(),
                                graph::source::AVID_SOURCE_COLUMNS_TIMECODE | graph::source::AVID_LINKED_MOV_TIMECODE
                                    | graph::source::VERIFIED_LINKED_MOV_TIMECODE
                                    | graph::source::VERIFIED_LINKED_R3D_TIMECODE)))
                            .map(|start| ticks(start, "original source timecode"))
                            .transpose()?.unwrap_or_default();
                        if let Some(receipt) = side(&sc, "avid.source", &m.id)
                            .and_then(|native| key(native, "embedded_mov_timecode"))
                        {
                            if let Some(label) = key(receipt, "label").and_then(str_value) {
                                diagnostics.push(format!(
                                    "{}: embedded MOV timecode {label} is recorded in the source graph and does not change file-relative edit positions",
                                    item.name
                                ));
                            } else {
                                diagnostics.push(format!(
                                    "{}: embedded MOV timecode could not be verified",
                                    item.name
                                ));
                            }
                        }
                        c.video=Some(OriginalVideo{codec:codec.clone(),width,height,rate:fraction(rate),duration,stream_index:o.stream_index,source_start});
                    },
                    StreamKind::Audio=>{
                        let representation=match o.sample_representation.as_deref(){Some("aac")=>SampleRepresentation::Aac,Some("mp3")=>SampleRepresentation::Mp3,Some("pcm16")=>SampleRepresentation::Pcm16,Some("pcm16be")=>SampleRepresentation::Pcm16Be,Some("pcm24")=>SampleRepresentation::Pcm24,_=>return Err(error("original audio representation unresolved"))};
                        let channels = o.channels.ok_or_else(|| error("original physical channels unresolved"))?;
                        let source_start = if channels == 8 {
                            let start = m.clock.as_ref().and_then(|clock| clock.exact_start())
                                .filter(|start| Some(*start) == m.start)
                                .ok_or_else(|| error("verified BWF source clock is missing"))?;
                            ticks(start, "BWF origin")?
                        } else {
                            0
                        };
                        let a = OriginalAudio {
                            sample_rate: m.sample_rate.ok_or_else(|| error("original sample clock unresolved"))?,
                            channels: if dual_mono { 2 } else { channels },
                            duration, stream_index: if dual_mono { 1 } else { o.stream_index },
                            representation, source_start,
                        };
                        if c.audio.as_ref().is_some_and(|old|old!=&a){return Err(error("original audio physical stream facts disagree"));}c.audio=Some(a);
                        let avid_track = m.stream.ok_or_else(|| error("original logical lane unresolved"))?;
                        c.channels.push(OriginalChannel {
                            avid_track,
                            channel_index: if dual_mono { o.stream_index - 1 } else {
                                o.channel_index.ok_or_else(|| error("original physical channel unresolved"))?
                            },
                            gain_db: im.gain_db,
                        });
                    }
                }
                continue;
            }
            let start = match direct_clocks {
                Some((start, _, _, _)) => start,
                None if sc.contains_key(&("avid.managed_op1a", m.id.as_str())) => {
                    ticks(m.start.ok_or_else(|| error("managed OP1a source clock is missing"))?,
                          "managed OP1a source timecode")?
                }
                None => old_clock(&sc, m)?,
            };
            let duration = ticks(m.duration, "media duration")?;
            match m.kind {
                StreamKind::Video => {
                    let (Some(codec), Some((width, height)), Some(rate)) =
                        (&m.codec, m.frame_size, m.rate)
                    else {
                        return Err(error("Premiere picture facts incomplete"));
                    };
                    video = Some(VideoFile {
                        path,
                        codec: codec.clone(),
                        width,
                        height,
                        rate: fraction(rate),
                        duration,
                        start,
                    });
                }
                StreamKind::Audio => audio.push(AudioFile {
                    path,
                    sample_rate: m
                        .sample_rate
                        .ok_or_else(|| error("audio sample rate unresolved"))?,
                    bits: m.bits.ok_or_else(|| error("audio bit depth unresolved"))?,
                    duration,
                    start,
                    avid_track: m.stream,
                    gain_db: im.gain_db,
                }),
            }
        }
        let mut avid = side(&sc, "avid", &item.id)
            .map(|v| bridge::avid_from_sidecar(v, item.color_rgb16))
            .transpose()?;
        if let Some(avid) = &mut avid {
            avid.columns.retain(|(name, value)| {
                let Some(issue) = model::avid_column_text_issue(name, value) else { return true };
                let mut chars = name.chars();
                let preview: String = chars.by_ref().take(96).collect();
                let suffix = if chars.next().is_some() { "…" } else { "" };
                loss(&mut losses, &item.id, "avid_logging_column", format!(
                    "Avid logging column {preview:?}{suffix} has an invalid {issue} and was omitted in Premiere. The original column is retained in graph/2."
                ));
                false
            });
        }
        if item.color_rgb16.is_some() && avid.is_none() {
            loss(&mut losses,&item.id,"color","item label has no Avid metadata block for the current writer; source color retained only in graph");
        }
        let raw = side(&sc, "avid.source", &item.id);
        if let Some(Value::Arr(transforms)) = raw.and_then(|v| key(v, "input_color_transforms")) {
            if !transforms.is_empty() {
                let reason = match external_source_lut_filename(transforms) {
                    Some(filename) => format!(
                        "{}: Avid references the external source LUT {filename:?}. Check the timeline Color Adapter occurrences to see where Avid applies it.",
                        item.name,
                    ),
                    None => format!(
                        "{}: Avid source color transformation was retained in graph/2 only. Check the clip's source color settings in Premiere.",
                        item.name,
                    ),
                };
                loss(&mut losses, &item.id, "source_color_transform", reason);
            }
        }
        if let Some(observation) = raw.and_then(|v| key(v, "master_output_mix")) {
            if let Some(reason) = output_mix_notice(observation)? {
                loss(&mut losses, &item.id, "master_output_mix", reason);
            }
        }
        let mut declared = Vec::new();
        if let Some(groups) = raw.and_then(|v| key(v, "raw_groups")) {
            if !matches!(groups,Value::Arr(rows) if rows.is_empty()) {
                if clock == ClockProfile::VerifiedRewrap {
                    let Value::Arr(rows) = groups else { return Err(error("malformed native audio declarations")); };
                    if rows.len()>32 {return Err(error("too many native audio declarations"));}
                    let mut witnessed=HashSet::new();
                    for value in rows {
                        let group = str_value(value).and_then(declared_multichannel).ok_or_else(||error(format!("unsupported_native_audio_grouping {}: unknown native declaration",item.id)))?;
                        if !item.stream_groups.contains(&group.channels) { return Err(error(format!("unsupported_native_audio_grouping {}: incomplete or contradictory source group",item.id))); }
                        if !witnessed.insert(group.channels.clone()) {return Err(error("duplicate native audio declaration"));}
                        declared.push(group);
                    }
                } else {
                    loss(&mut losses,&item.id,"audio_grouping","unsupported native group declarations retained in source facts; only qualified mono/stereo routes prepared");
                }
            }
        }
        let drop = raw
            .and_then(|v| key(v, "drop"))
            .and_then(boolean)
            .unwrap_or(false);
        let mut markers = Vec::new();
        for (index, source) in item.markers.iter().enumerate() {
            if let Some(mut output) = marker(source, &mut losses, &item.id, index, false)? {
                let native = native_markers.get(&(item.id.as_str(), index)).copied();
                output.track = master_marker_track(source, native).map(|track| {
                    let prefix = if track.kind == StreamKind::Video { 'V' } else { 'A' };
                    format!("{prefix}{}", track.index)
                });
                markers.push(output);
            }
        }
        let mut stereo_pairs = Vec::new();
        let mut multichannel_groups = Vec::new();
        for group in &item.stream_groups {
            if let [l, r] = group.as_slice() {
                stereo_pairs.push((*l, *r));
            } else if let Some(g) = declared.iter().find(|g| &g.channels == group) {
                multichannel_groups.push(g.clone());
            } else {
                if clock == ClockProfile::VerifiedRewrap {
                    return Err(error(format!("unsupported_native_audio_grouping {}: {}-stream group is outside the qualified route",item.id,group.len())));
                }
                loss(
                    &mut losses,
                    &item.id,
                    "stream_groups",
                    format!(
                        "{}-stream group has no qualified Premiere pair interpretation",
                        group.len()
                    ),
                );
            }
        }
        if let Some(mut c) = original {
            if let Some(proxy) = sc.get(&("avid.proxy", item.id.as_str())) {
                let path = key(proxy,"path").and_then(str_value).ok_or_else(||error("missing Avid proxy path"))?;
                let v = c.video.as_ref().ok_or_else(||error("Avid proxy lacks its camera picture"))?;
                let frame = model::ticks_per_unit(v.rate);
                if key(proxy,"frames").and_then(integer) != Some(v.duration / frame)
                    || key(proxy,"start").and_then(integer) != Some(v.source_start / frame)
                    || match (key(proxy,"bits").and_then(integer), c.audio.as_ref().map(|a|&a.representation)) {
                        (None,None) | (Some(16),Some(SampleRepresentation::Pcm16)) | (Some(24),Some(SampleRepresentation::Pcm24)) => false,
                        _ => true,
                    }
                {
                    return Err(error("Avid proxy does not agree with camera original extent, clock or PCM"));
                }
                c.proxy_path = Some(path.into());
            }
            c.metadata.avid = avid;
            c.metadata.time_display = display(item.rate, drop);
            c.metadata.markers = markers;
            if !dual_mono && item.stream_groups.is_empty()
                && has_unmodified_original_stereo_pair(&c, raw)
                && !items_used_for_sequence_audio.contains(item.id.as_str())
            {
                c.groups.push(vec![1, 2]);
                diagnostics.push(format!(
                    "{}: linked original uses stereo left/right for new edits",
                    item.name
                ));
            } else {
                let mut grouped = HashSet::new();
                for group in &item.stream_groups {
                    if group.len() != 2 {
                        return Err(error("original-file wider groups require separate qualification"));
                    }
                    c.groups.push(group.clone());
                    grouped.extend(group.iter().copied());
                }
                for channel in &c.channels {
                    if !grouped.contains(&channel.avid_track) {
                        c.groups.push(vec![channel.avid_track]);
                    }
                }
            }
            let standalone = prproj::convert::prepared::validate_original(&c);
            let needs_native_import = item.media.iter().any(|link| {
                side(&sc, "avid.source", &link.media)
                    .and_then(|row| key(row, "verified_for_native_import"))
                    == Some(&Value::Bool(true))
            });
            if standalone.is_err() || needs_native_import {
                let reason = || standalone.as_ref().err().map_or_else(
                    || format!("{} requires Premiere to import its verified file", item.name),
                    ToString::to_string,
                );
                if !assisted || item.media.iter().any(|link| {
                    let source = media[link.media.as_str()];
                    source.online != Some(true) || source.original.is_none()
                        || source.locations.as_slice() != [c.path.as_str()]
                        || link.source_offset.is_none()
                }) {
                    return Err(error(reason()));
                }
                c.premiere_assisted = true;
                prproj::convert::prepared::validate_original(&c)
                    .map_err(|failure| error(format!("{}: {failure}", item.name)))?;
            }
            if same_rate_aac_channels != 0 {
                let stereo = matches!(c.groups.as_slice(), [pair] if pair.as_slice() == [1, 2]);
                let mono = matches!(c.groups.as_slice(), [left, right] if left.as_slice() == [1] && right.as_slice() == [2]);
                if same_rate_aac_channels != 2 || clock != ClockProfile::LegacyImport
                    || options.bin_mode != "separate"
                    || c.proxy_path.is_some() || !c.metadata.markers.is_empty()
                    || !c.video.as_ref().is_some_and(|v| {
                        let palette_rate = matches!((v.rate.num, v.rate.den), (25, 1) | (30_000, 1001));
                        v.codec == "avc1" && v.rate == Fraction::new(24, 1) && stereo
                            || v.codec == "avc1" && v.rate == Fraction::new(24_000, 1001)
                                && (v.width, v.height) == (3840, 2160) && (stereo || mono)
                                && std::path::Path::new(&c.path).extension()
                                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
                            || palette_rate && (stereo || mono) && matches!(v.codec.as_str(), "avc1" | "hvc1")
                    })
                    || !c.audio.as_ref().is_some_and(|a| a.representation == SampleRepresentation::Aac
                        && a.channels == 2 && a.sample_rate == 48000
                        && c.video.as_ref().is_some_and(|v| a.duration <= v.duration
                            && v.duration - a.duration < model::ticks_per_unit(Fraction::new(48_000, 1))))
                    || !c.channels.iter().all(|ch| ch.avid_track == ch.channel_index + 1)
                {
                    return Err(error(format!("{}: linked AAC file-track origin requires a verified same-rate source in Separate files mode", item.name)));
                }
                loss(&mut losses, &item.id, "original_audio_insertion",
                    "Existing edits use the saved Avid audio offset. Fresh insertions use the original file's own audio origin.");
            }
            let view = if same_rate_aac_channels != 0 && items_used_for_sequence_audio.contains(item.id.as_str()) {
                Some(&mut same_rate_aac_views)
            } else if c.is_same_rate_linked_pcm()
                && c.groups.iter().any(|g| g.len() == 2)
                && items_used_for_mono_audio.contains(item.id.as_str())
            {
                Some(&mut pcm_movie_mono_views)
            } else {
                None
            };
            if let Some(view) = view {
                // Preserve the saved stereo bin master and use independent mono
                // source controls for the existing edits that need them.
                let mut mono = c.clone();
                mono.metadata.id.push_str(":timeline-mono");
                mono.metadata.name.push_str(" [timeline mono]");
                mono.metadata.markers.clear();
                mono.groups = vec![vec![1], vec![2]];
                if !ids.insert(mono.metadata.id.clone()) {
                    return Err(error("timeline mono source identity collision"));
                }
                view.insert(item.id.as_str(), mono.metadata.id.clone());
                mono_view_ids.push(mono.metadata.id.clone());
                clips.push(PreparedClip::Original(mono));
            }
            if c.video.as_ref().is_some_and(|v| (v.width, v.height) != (1920, 1080))
                && c.supports_explicit_linked_raster()
            {
                explicit_linked_items.insert(item.id.as_str());
            }
            clips.push(PreparedClip::Original(c));
        } else {
        if timeline_stereo.contains(item.id.as_str()) && verified_stereo_siblings(item, &media, &sc) {
            stereo_pairs.push((1, 2));
            diagnostics.push(format!("{}: built a linked stereo source for the saved timeline combiner", item.name));
        }
        clips.push(PreparedClip::Legacy(Clip {
            id,
            name: item.name.clone(),
            material_umid: item.umid.clone(),
            tape_name: item.tape.clone(),
            video,
            audio,
            avid,
            time_display: display(item.rate, drop || direct_drop_frame_items.contains(item.id.as_str())),
            rate: item.rate.map(fraction),
            markers,
            stereo_pairs,
            multichannel_groups,
        }));
        }
    }
    context.occurrence = None;
    let media: HashMap<_, _> = g.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut routes = HashMap::new();
    for item in &g.items {
        let mut r = Routes {
            video: None,
            video_rate: None,
            video_raster: None,
            explicit_linked_raster: explicit_linked_items.contains(item.id.as_str()),
            video_duration: None,
            audio: HashMap::new(),
            first_audio: None,
        };
        for im in &item.media {
            let m = media[im.media.as_str()];
            match m.kind {
                StreamKind::Video => {
                    r.video = Some(im.source_offset);
                    r.video_rate = m.rate;
                    r.video_raster = m.frame_size;
                    r.video_duration = Some(m.duration);
                }
                StreamKind::Audio => {
                    let label = m.stream.unwrap_or(r.audio.len() as i64 + 1);
                    r.first_audio.get_or_insert(label);
                    r.audio.insert(label, im.source_offset);
                }
            }
        }
        routes.insert(item.id.as_str(), r);
    }
    let items_by_clip: HashMap<_, _> = g
        .items
        .iter()
        .map(|i| (i.id.strip_prefix("clip:").unwrap_or(&i.id), i))
        .collect();
    for b in &g.bins {
        if b.parent.is_some() {
            loss(
                &mut losses,
                &b.id,
                "hierarchy",
                "nested source bin flattened by the current destination model",
            );
        }
    }
    let mut model = PreparedProjectInput {
        project_name: g.project.name.clone(),
        options: options.clone(),
        clips,
        bins: g
            .bins
            .iter()
            .map(|b| Bin {
                name: b.name.clone(),
                clips: b.items.iter().filter(|id| !omitted_offline_items.contains(id.as_str()))
                    .map(|id| clip_id(id)).collect(),
            })
            .collect(),
        sequences: Vec::new(),
        autosync_bins: Default::default(),
        texts: Vec::new(),
        video_dissolves: Vec::new(),
        audio_edge_fades: Vec::new(),
        audio_crossfades: Vec::new(),
        audio_pans: Vec::new(),
        audio_mixers: Default::default(),
        track_states: Default::default(),
        timeline_marks: Default::default(),
        non_drop_sequences: Default::default(),
        fit_picture: Default::default(),
        picture_effects: Vec::new(),
        source_luts: Vec::new(),
        extended_sdr_grades: Default::default(),
        edit_labels: Vec::new(),
        disabled_edits: Default::default(),
        direct_dnxhr_origins,
        direct_dnxhd_pcm24,
        constant_speeds: Vec::new(),
        frame_holds: Default::default(),
        multicams: Vec::new(),
        multicam_uses: Vec::new(),
        multicam_tracks: Vec::new(),
        picture_subclips: Vec::new(),
        av_subclips: Vec::new(),
        av_subclip_uses: Vec::new(),
        picture_nests: Vec::new(),
        layered_submasters: Vec::new(),
        png_mattes: Default::default(),
    };
    if !mono_view_ids.is_empty() {
        model.bins.push(Bin { name: "Avid timeline audio".into(), clips: mono_view_ids });
    }
    if clock == ClockProfile::VerifiedRewrap || managed_rewrap.is_some_and(|(selected, _)| !selected.is_empty()) {
        let mut legacy=model.clips.iter().filter_map(|c|match c{PreparedClip::Legacy(c)=>Some(c.clone()),_=>None}).collect();
        normalize_rewrap_clocks(g, &sc, &mut legacy, &mut diagnostics, managed_rewrap.map(|(items, _)| items))?;
        for c in &mut model.clips {if let PreparedClip::Legacy(c)=c {*c=legacy[&c.id].clone();}}
    }
    let mut loss_index: LossIndex = HashMap::new();
    for l in &g.losses {
        loss_index
            .entry((&l.object, &l.aspect))
            .or_default()
            .push(&l.reason);
    }
    let mut source_marker_uniqueness = HashMap::new();
    for item in &g.items {
        for (mi, m) in item.markers.iter().enumerate() {
            let native = native_markers.get(&(item.id.as_str(), mi)).copied();
            if let (Some(track), Some(time), Some(created)) = (
                master_marker_track(m, native),
                m.start.and_then(|t| t.to_units(TICKS_PER_SECOND)),
                native.and_then(native_created),
            ) {
                source_marker_uniqueness
                    .entry(inherited_key(&item.id, m, track, time, created))
                    .and_modify(|unique| *unique = false)
                    .or_insert(true);
            }
        }
    }
    let mut editable_nested_motion = HashSet::new();
    if options.bin_mode == "separate" && g.sidecars.iter().any(|row| row.adapter == "avid.nested_motion")
    {
        let original_pictures: HashSet<_> = model.clips.iter().filter_map(|clip| match clip {
            PreparedClip::Original(c) if c.video.is_some() && c.proxy_path.is_none() =>
                Some(c.metadata.id.as_str()),
            _ => None,
        }).collect();
        for sequence in &g.sequences {
            for placement in &sequence.placements {
                if omitted_offline_items.contains(placement.item.as_str())
                    || side(&sc, "avid.nested_motion", &placement.id).is_none()
                {
                    continue;
                }
                let id = clip_id(&placement.item);
                let Some(item) = items_by_clip.get(id.as_str()) else { continue };
                let supported = match clock {
                    ClockProfile::LegacyImport => original_pictures.contains(id.as_str())
                        || model.direct_dnxhr_origins.contains_key(id.as_str())
                        || matches!(direct_opatom_clocks(item, &media, &sc, true, true, false), Some((0, 0, _, _))),
                    ClockProfile::VerifiedRewrap => placement.speed.as_ref().is_some_and(|speed| speed.ratio == (1, 2))
                        && matches!(&item.media[..], [binding]
                        if media.get(binding.media.as_str()).is_some_and(|picture|
                            picture.kind == StreamKind::Video && picture.codec.as_deref() == Some("apcs")
                                && picture.online == Some(true) && picture.original.is_none())),
                };
                if item.markers.is_empty() && supported {
                    editable_nested_motion.insert(placement.id.as_str());
                }
            }
        }
    }
    let facts = PreparationFacts {
        sidecars: sc,
        same_rate_aac_views,
        pcm_movie_mono_views,
        routes,
        items_by_clip,
        losses: loss_index,
        native_markers,
        source_marker_uniqueness,
        editable_nested_motion,
        omitted_offline_items,
        omitted_managed_items,
        omitted_picture_items,
    };
    let mut ownership = HashMap::new();
    let stereo_fade_sources = if options.bin_mode == "separate" && clock == ClockProfile::LegacyImport
        && allow_originals && has_avid_filler_fade(g)
    {
        avid_stereo_fade_sources(g, &media, &facts.sidecars)
    } else {
        HashSet::new()
    };
    // The mono source-layout Constant Gain has been tested on the direct Separate route.
    let linear_fade_clips: HashSet<_> = if options.bin_mode == "separate" {
        model.clips.iter().filter_map(|clip| match clip {
            PreparedClip::Legacy(clip) if model.direct_dnxhr_origins.contains_key(&clip.id)
                && clip.video.as_ref().map(|v| v.rate).or(clip.rate) == Some(Fraction::new(24_000, 1001))
                && clip.audio.iter().all(|a| a.bits == 24) =>
                Some(clip.id.as_str()),
            _ => None,
        }).collect()
    } else {
        HashSet::new()
    };
    let pan_clips: HashSet<_> = if allow_originals && clock == ClockProfile::LegacyImport {
        let managed = pan::managed_sources(g, &media, &facts.sidecars);
        model.clips.iter().filter_map(|clip| match clip {
            PreparedClip::Legacy(clip)
                if (options.bin_mode != "merged" || model.direct_dnxhr_origins.get(&clip.id) == Some(&0))
                    && facts.items_by_clip.get(clip.id.as_str())
                    .is_some_and(|item| managed.contains(item.id.as_str())) =>
                Some(clip.id.as_str()),
            PreparedClip::Original(clip) if options.bin_mode == "separate"
                && prproj::convert::prepared::qualifies_audio_pan(clip)
                && facts.items_by_clip.get(clip.metadata.id.as_str()).is_some_and(|item|
                    item.media.iter().all(|reference| media[reference.media.as_str()].online == Some(true))) =>
                Some(clip.metadata.id.as_str()),
            _ => None,
        }).collect()
    } else {
        HashSet::new()
    };
    let mut written_pan = HashSet::new();
    let source_gain_clips: HashMap<_, _> = if options.bin_mode == "separate" && clock == ClockProfile::LegacyImport {
        model.clips.iter().filter_map(|clip| match clip {
            PreparedClip::Original(clip) if prproj::convert::prepared::is_mono_pcm24_wav(clip)
                && clip.channels[0].gain_db != 0.0
                && clip.metadata.rate == Some(Fraction::new(24, 1))
                && facts.items_by_clip[clip.metadata.id.as_str()].media.iter()
                    .all(|reference| media[reference.media.as_str()].online == Some(true)) =>
                Some((clip.metadata.id.as_str(), clip.channels[0].avid_track)),
            _ => None,
        }).collect()
    } else {
        HashMap::new()
    };
    // Bound the extra sampled paths across the project, in addition to the
    // graph and destination writer limits on preserved source keys.
    let mut animatte_points_remaining = 500_000;
    let mut record_only_markers = HashSet::new();
    for (si, s) in g.sequences.iter().enumerate() {
        let needs_drop = s.placements.iter().any(|p| direct_drop_frame_items.contains(p.item.as_str()));
        let ndf_start = if !needs_drop && (s.rate.num(), s.rate.den()) == (30_000, 1001)
            && s.placements.iter().any(|p| facts.items_by_clip.get(p.item.strip_prefix("clip:").unwrap_or(&p.item))
                .is_some_and(|item| item.media.iter().any(|im| {
                    let stream = media[im.media.as_str()];
                    stream.rate == Some(s.rate) && stream.original.is_some()
                })))
        {
            direct_avid_tc1(s, 30, false, 30 * 60 * 60 * 24)
        } else { None };
        if ndf_start.is_some() { model.non_drop_sequences.insert(si); }
        let exact_start = ndf_start.or_else(|| qualified_sequence_start(s));
        if needs_drop && (s.rate != Rational::new(30_000, 1001).unwrap() || exact_start.is_none())
        {
            context.sequence = Some(&s.id);
            return Err(error(format!(
                "direct_dnxhd_sequence_clock {}: direct 29.97 DNxHD OP-Atom needs a qualified same-rate drop-frame TC1",
                s.id
            )));
        }
        if let Some(start) = exact_start {
            let rate_label = match (s.rate.num(), s.rate.den()) {
                (24, 1) => "24",
                (25, 1) => "25",
                (30_000, 1001) if ndf_start.is_some() => "29.97 NDF",
                (30_000, 1001) => "29.97 DF",
                _ => "23.976",
            };
            diagnostics.push(format!(
                "{}: sequence start from the qualified {rate_label} clock ({start} seconds)",
                s.name
            ));
        }
        let sequence = prepare_sequence(
            &facts,
            s,
            si,
            exact_start,
            &linear_fade_clips,
            &source_gain_clips,
            &stereo_fade_sources,
            &mut ownership,
            &mut losses,
            &mut model.video_dissolves,
            &mut model.audio_edge_fades,
            &mut model.picture_effects,
            &mut animatte_points_remaining,
            &mut record_only_markers,
            best_effort,
            &mut context.occurrence,
        ).inspect_err(|_| context.sequence = Some(&s.id))?;
        pan::prepare(&facts, s, &sequence, si, &ownership, &pan_clips,
            &mut model.audio_pans, &mut written_pan, &mut losses);
        let names = track_names::prepare(&facts, s, &sequence,
            allow_originals && clock == ClockProfile::LegacyImport, &mut losses);
        if !names.is_empty() {
            model.track_states.insert(si, names);
        }
        model.sequences.push(sequence);
    }
    multicam::prepare(g, &mut model, &ownership, clock, &mut diagnostics, &mut losses,
        best_effort, &facts.omitted_offline_items, context)?;
    let autosyncs = subclips::prepare(
        g, &media, &facts.sidecars, &mut model, &ownership, allow_originals, clock, &mut losses,
    )?;
    text::prepare(g, &mut model, &mut losses)?;
    nests::prepare(
        g, &media, &facts.sidecars, &mut model, &ownership, &record_only_markers,
        allow_originals, clock, &mut losses,
    );
    let restored_layered = nests::add_layered(layered_candidates, &mut model)?;
    if options.bin_mode == "separate"
        && (clock == ClockProfile::LegacyImport || !facts.editable_nested_motion.is_empty())
    {
        let mut qualified_items = HashMap::new();
        for source_sequence in &g.sequences {
            for placement in &source_sequence.placements {
                let nested = facts.editable_nested_motion.contains(placement.id.as_str());
                let (num, den) = if nested {
                    let Some(speed) = &placement.speed else {
                        context.at_placement(source_sequence, placement);
                        return Err(error("missing saved Avid nested constant-speed witness"));
                    };
                    let (den, num) = speed.ratio;
                    if num <= 0 || den <= 0 {
                        context.at_placement(source_sequence, placement);
                        return Err(error("invalid saved Avid nested constant-speed witness"));
                    }
                    (num, den)
                } else {
                    if clock != ClockProfile::LegacyImport {
                        continue;
                    }
                    let Some(Value::Arr(ratio)) = side(&facts.sidecars, "avid.source", &placement.id)
                        .and_then(|receipt| key(receipt, "constant_speed")) else { continue };
                    let [Value::Int(num), Value::Int(den)] = ratio.as_slice() else {
                        context.at_placement(source_sequence, placement);
                        return Err(error("invalid saved Avid constant-speed witness"));
                    };
                    (*num, *den)
                };
                let Some(&(sequence, event)) = ownership.get(&placement.id) else { continue };
                let seq = &model.sequences[sequence];
                let Some(item) = facts.items_by_clip.get(seq.events[event].clip.as_str()) else {
                    context.at_placement(source_sequence, placement);
                    return Err(error("saved Avid constant speed has no prepared source item"));
                };
                let eligible = nested || *qualified_items.entry(item.id.as_str()).or_insert_with(|| {
                    item.rate == Some(Rational::integer(24))
                        && matches!(direct_opatom_clocks(item, &media, &facts.sidecars, true, true, false), Some((0, 0, _, _)))
                });
                if !eligible {
                    continue;
                }
                if !item.markers.is_empty() {
                    loss(&mut losses, &placement.id, "constant_speed",
                        "Premiere 27 retime correction requires a separate source marker timing check; this edit uses the compatibility retime");
                    continue;
                }
                model.constant_speeds.push(prproj::convert::prepared::PreparedConstantSpeed {
                    sequence, event, ratio: (num, den),
                });
                diagnostics.push(format!("{}: editable Avid picture retime uses exact {num}/{den} speed and Premiere 27 source trims", placement.id));
            }
        }
        // A retimed edit has a qualified record span, but the clip marker's
        // source coordinates have not been qualified against Premiere playback.
        if !model.constant_speeds.is_empty() {
            let corrected: HashSet<_> = model.constant_speeds.iter()
                .map(|speed| (speed.sequence, speed.event)).collect();
            for (sequence, seq) in model.sequences.iter_mut().enumerate() {
                for marker in &mut seq.markers {
                    let event = marker.event.and_then(|event| usize::try_from(event).ok());
                    if marker.owner == "edit"
                        && event.is_some_and(|event| corrected.contains(&(sequence, event)))
                    {
                        marker.owner = "sequence".into();
                        marker.event = None;
                        marker.source = None;
                        marker.inherited = false;
                        loss(&mut losses, &g.sequences[sequence].id, "marker_ownership",
                            format!("marker {:?}: record time preserved on sequence; edit ownership on a corrected constant retime is unqualified", marker.name));
                    }
                }
            }
        }
    }
    if !allow_original_speed {
        let verified_speeds: HashSet<_> = model.constant_speeds.iter()
            .map(|speed| (speed.sequence, speed.event)).collect();
        let originals: HashSet<_> = model.clips.iter().filter_map(|clip| match clip {
            PreparedClip::Original(clip) => Some(clip.metadata.id.as_str()),
            _ => None,
        }).collect();
        let unsupported = model.sequences.iter().enumerate().find_map(|(si, sequence)| {
            sequence.events.iter().enumerate().find(|(ei, event)| {
                event.speed_ratio.is_some() && originals.contains(event.clip.as_str())
                    && !verified_speeds.contains(&(si, *ei))
            }).map(|(ei, _)| (si, ei))
        });
        if let Some((sequence, event)) = unsupported {
            context.at_prepared_event(g, sequence, Some(event), &ownership);
            return Err(error("original-file retiming is not qualified"));
        }
    }
    let restored_disabled = if allow_originals && options.bin_mode == "separate"
        && clock == ClockProfile::LegacyImport && has_disabled_edit_candidates(g)
    {
        disabled::prepare(g, &media, &facts.sidecars, &unsupported_stereo, &mut model, &mut diagnostics)
    } else {
        HashSet::new()
    };
    for (bin, sequence) in autosyncs {
        model.autosync_bins.insert(model.sequences.len(), bin);
        model.sequences.push(sequence);
    }
    model.validate_with_location(!check_encoding).map_err(|issue| {
        if let Some(location) = issue.location {
            context.at_prepared_event(g, location.sequence, location.event, &ownership);
        }
        error(issue.error.to_string())
    })?;
    let native_disabled_losses: HashSet<_> = g.losses.iter()
        .filter(|loss| loss.aspect == "disabled_clip")
        .filter_map(|loss| loss.reason.split_once(": ")
            .map(|(owner, _)| (loss.object.as_str(), owner)))
        .collect();
    for sequence in &g.sequences {
        for edit in &sequence.disabled_placements {
            let key = (sequence.id.as_str(), edit.id.as_str());
            if !restored_disabled.contains(&key) && !native_disabled_losses.contains(&key) {
                loss(&mut losses, &edit.id, "disabled_clip",
                    "Disabled source edit kept as a gap in this Premiere conversion");
            }
        }
    }
    losses.extend(g.losses.iter().filter(|loss| {
        if loss.aspect == LAYERED_LOSS && restored_layered.get(loss.object.as_str())
            .is_some_and(|reasons| reasons.contains(&loss.reason))
        {
            return false;
        }
        loss.aspect != "subclip_bin_item"
            && !(loss.aspect == "disabled_clip" && loss.reason.split_once(": ").is_some_and(
                |(owner, _)| restored_disabled.contains(&(loss.object.as_str(), owner))))
            && !(written_pan.contains(loss.object.as_str()) && loss.aspect == "pan_automation"
                && loss.reason == pan::SOURCE_OMISSION)
    }).cloned());
    Ok(PreparedPremiereInput {
        input:model,
        losses,
        diagnostics,
        ownership,
    })
}

#[derive(Clone, Copy)]
struct Trim {
    start: Rational,
    end: Rational,
}

fn direct_avid_tc1(s: &SourceSequence, base: u16, drop: bool, frames_per_day: u64) -> Option<Rational> {
    let clock = s.clock.as_ref()?;
    (clock.rate == s.rate
        && clock.display_base == base
        && clock.drop == drop
        && clock.provenance.route == "direct sequence TC1"
        && clock.provenance.timecode_track.starts_with("TC1/")
        && clock.provenance.source_rate == s.rate
        && clock.provenance.origin_count == clock.effective_count
        && clock.provenance.source_offset == 0
        && clock.effective_count < frames_per_day
        && clock.exact_start() == s.start)
        .then_some(s.start)
        .flatten()
}

fn qualified_sequence_start(s: &SourceSequence) -> Option<Rational> {
    if s.rate == Rational::integer(25) {
        return direct_avid_tc1(s, 25, false, 25 * 60 * 60 * 24);
    }
    if (s.rate.num(), s.rate.den()) == (30_000, 1001) {
        // The 1080p29.97 Premiere preset already uses drop-frame display 102.
        return direct_avid_tc1(s, 30, true, 2_589_408);
    }
    let clock = s.clock.as_ref()?;
    // The existing 23.976 Premiere sequence presets use non-drop display
    // code 110. This changes only the zero point, not any record-local times.
    ((s.rate.num() == 24000 && s.rate.den() == 1001
        || s.rate == Rational::integer(24)
            && matches!((clock.provenance.route.as_str(), clock.provenance.timecode_track.as_str()),
                ("Resolve archive clock", "Sm2Sequence.MediaExtents")
                    | ("Premiere sequence ticks", "MZ.ZeroPoint"))
            && clock.provenance.source_package == s.id
            && clock.provenance.origin_count == clock.effective_count
            && clock.provenance.source_offset == 0
            && clock.exact_start() == s.start)
        && clock.rate == s.rate
        && clock.display_base == 24
        && !clock.drop
        && clock.effective_count < 24 * 60 * 60 * 24)
        .then_some(s.start)
        .flatten()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FillerSide {
    Outgoing,
    Incoming,
}

struct FillerReceipt<'a> {
    effect: &'a str,
    kind: StreamKind,
    outgoing: Option<i64>,
    incoming: Option<i64>,
    same_rate: bool,
}

fn unoccupied_transition(
    s: &SourceSequence, t: &graph::source::SourceTransition, omitted: &HashSet<&str>,
    filler: Option<(&str, FillerSide)>,
) -> R<bool> {
    let start = t.cut.checked_sub(t.cut_offset).ok_or_else(|| error("transition start overflow"))?;
    let end = add(start, t.duration)?;
    for p in s.placements.iter().chain(&s.disabled_placements) {
        if omitted.contains(p.item.as_str()) || filler.is_some_and(|(id, _)| id == p.id) {
            continue;
        }
        let same_lane = match t.kind {
            StreamKind::Video => p.video_track == Some(t.track),
            StreamKind::Audio => p.audio_tracks.contains(&t.track)
                || p.grouped_audio.iter().any(|audio| audio.track == t.track),
        };
        if same_lane {
            let filler_side = filler.map(|(_, side)| side);
            if p.record_start < end {
                let last = add(p.record_start, p.duration)?;
                if last > start || (last == start && filler_side == Some(FillerSide::Outgoing)) {
                    return Ok(false);
                }
            } else if p.record_start == end && filler_side == Some(FillerSide::Incoming) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn isolated_filler_cut(
    s: &SourceSequence,
    t: &graph::source::SourceTransition,
    survivor: &Placement,
    filler: &graph::source::SourceBinding,
    side: FillerSide,
    omitted: &HashSet<&str>,
) -> R<bool> {
    let start = sub(t.cut, t.cut_offset)?;
    let end = add(start, t.duration)?;
    let on_lane = match t.kind {
        StreamKind::Video => survivor.video_track == Some(t.track)
            && survivor.audio_tracks.is_empty() && survivor.grouped_audio.is_empty(),
        StreamKind::Audio => survivor.video_track.is_none()
            && ((survivor.audio_tracks == [t.track] && survivor.grouped_audio.is_empty())
                || (survivor.audio_tracks.is_empty()
                    && matches!(survivor.grouped_audio.as_slice(), [group] if group.track == t.track))),
    };
    if !on_lane || survivor.duration < t.duration
        || [start, end].iter().any(|time| time.to_units(s.rate.num())
            .is_none_or(|units| units % s.rate.den() != 0))
        || filler.reason.as_deref() != Some("native owner produced no source placements")
        || !unoccupied_transition(s, t, omitted, Some((&survivor.id, side)))?
    {
        return Ok(false);
    }
    for other in &s.transitions {
        if other.id != t.id && other.kind == t.kind && other.track == t.track {
            let begin = sub(other.cut, other.cut_offset)?;
            let last = add(begin, other.duration)?;
            if (begin < end && last > start)
                || (side == FillerSide::Outgoing && last == start)
                || (side == FillerSide::Incoming && begin == end)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn prepare_sequence<'a>(
    facts: &PreparationFacts<'a>,
    s: &'a SourceSequence,
    si: usize,
    exact_start: Option<Rational>,
    linear_fade_clips: &HashSet<&str>,
    source_gain_clips: &HashMap<&str, i64>,
    stereo_fade_sources: &HashSet<&str>,
    ownership: &mut HashMap<String, (usize, usize)>,
    losses: &mut Vec<Loss>,
    video_dissolves: &mut Vec<PreparedVideoDissolve>,
    audio_edge_fades: &mut Vec<prproj::convert::prepared::PreparedAudioEdgeFade>,
    picture_effects: &mut Vec<prproj::convert::prepared::PreparedPictureEffect>,
    animatte_points_remaining: &mut usize,
    record_only_markers: &mut HashSet<(usize, usize)>,
    best_effort: bool,
    active_occurrence: &mut Option<PreparationOccurrence<&'a str>>,
) -> R<Sequence> {
    // Avid's actual sequence canvas is currently unqualified. Keep the exact
    // old first-picture/default raster only in the destination compatibility profile.
    let frame_size=match side(&facts.sidecars,"avid.source",&s.id).and_then(|v|key(v,"compatibility_frame_size")) {
        Some(Value::Arr(values)) if values.len()==2=>match (integer(&values[0]),integer(&values[1])) {
            (Some(w),Some(h)) if w>0&&h>0=>(w,h),
            _=>return Err(error(format!("{}: invalid compatibility frame size",s.id))),
        },
        Some(_)=>return Err(error(format!("{}: invalid compatibility frame size",s.id))),
        None=>s.frame_size.ok_or_else(||error(format!("unresolved_sequence_geometry {}: neither qualified canvas nor explicit compatibility raster is available",s.id)))?,
    };
    let source_gain = gain::SourceGain::new(facts, s, frame_size, source_gain_clips);
    let mut linear_audio = HashSet::new();
    let mut filler_edges = HashMap::new();
    let mut gain_gap_edges = HashMap::new();
    if let Some(raw) =
        side(&facts.sidecars, "avid.source", &s.id).and_then(|v| key(v, "native_transitions"))
    {
        let Value::Arr(native) = raw else {
            return Err(error(format!(
                "unresolved_transition_topology {}: malformed native transition receipt",
                s.id
            )));
        };
        let qualified: HashSet<_> = s.transitions.iter().map(|t| t.id.as_str()).collect();
        let mut receipt_ids = HashSet::new();
        let sequence_rate = s.rate.to_string();
        for transition in native {
            let id = key(transition, "id").and_then(str_value).ok_or_else(|| {
                error(format!(
                    "unresolved_transition_topology {}: missing native transition identity",
                    s.id
                ))
            })?;
            let Some(&saved_id) = qualified.get(id) else {
                *active_occurrence = Some(PreparationOccurrence::Transition(id));
                return Err(error(format!("unresolved_transition_topology {id}: native transition has no qualified timing/relationship; source retained unchanged")));
            };
            if !receipt_ids.insert(id) {
                *active_occurrence = Some(PreparationOccurrence::Transition(saved_id));
                return Err(error(format!("unresolved_transition_topology {id}: duplicate native transition receipt")));
            }
            if let Some(edges) = key(transition, "gain_gap_edges") {
                gain_gap_edges.insert(id, edges);
            }
            let (effect, kind) = match key(transition, "effect").and_then(str_value) {
                Some(effect @ "BLEND_AUDIO_DISSOLVE") => (effect, StreamKind::Audio),
                Some(effect @ ("EFF2_BLEND_DISSOLVE" | "EFF2_BLEND_DISSOLVE_LOG" | "EFF2_BLEND_DISSOLVE_LIN")) =>
                    (effect, StreamKind::Video),
                _ => continue,
            };
            let attribute = key(transition, "audio_curve_attribute").and_then(integer);
            let parameter = key(transition, "audio_curve_parameter").and_then(integer);
            if kind == StreamKind::Audio && attribute == Some(0) && parameter == Some(0) {
                linear_audio.insert(id);
            }
            let outgoing = key(transition, "outgoing_filler_frames").and_then(integer);
            let incoming = key(transition, "incoming_filler_frames").and_then(integer);
            if (outgoing.is_some() || incoming.is_some())
                && attribute.zip(parameter).is_none_or(|(a, b)| a == b)
            {
                let same_rate = key(transition, "rate").and_then(str_value) == Some(sequence_rate.as_str());
                filler_edges.insert(id, FillerReceipt { effect, kind, outgoing, incoming, same_rate });
            }
        }
    }
    let placements: HashMap<_, _> = s.placements.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut trims: HashMap<_, _> = s
        .placements
        .iter()
        .map(|p| {
            Ok((
                p.id.as_str(),
                Trim {
                    start: p.record_start,
                    end: add(p.record_start, p.duration)?,
                },
            ))
        })
        .collect::<R<_>>()?;
    let mut crossfades = Vec::new();
    let mut dissolve_endpoints = Vec::new();
    let mut stereo_tail_fades = Vec::new();
    let mut seen = HashSet::new();
    // Resolve against original spans before making any edit, including chains.
    // Keep the current ID on early return, and clear it before unrelated checks.
    for t in &s.transitions {
        *active_occurrence = Some(PreparationOccurrence::Transition(&t.id));
        let all_endpoints_omitted = t.outgoing.placements.iter().chain(&t.incoming.placements)
            .all(|id| facts.omitted_offline_items.contains(placements[id.as_str()].item.as_str()));
        if best_effort && t.style.starts_with("avid.")
            && all_endpoints_omitted
            && unoccupied_transition(s, t, &facts.omitted_offline_items, None)?
        {
            let lane = if t.kind == StreamKind::Video { "V" } else { "A" };
            loss(losses, &t.id, "transition_omitted", format!(
                "{} · {lane}{} at {} seconds: transition omitted over an empty range with neither endpoint retained.",
                s.name, t.track, t.cut));
            continue;
        }
        if let Some(edges) = gain_gap_edges.get(t.id.as_str()) {
            gain_gap::prepare_cut(s, t, edges, &placements, &facts.sidecars, &mut trims)?;
            loss(losses, &t.id, "transition_cut",
                format!("{} · A{} at {} seconds: saved dissolve omitted at its cut beside an unavailable AudioSuite Gain edit.",
                    s.name, t.track, t.cut));
            continue;
        }
        if let Some(saved) = filler_edges.get(t.id.as_str())
            .filter(|saved| t.kind == saved.kind && t.style.strip_prefix("avid.") == Some(saved.effect))
        {
            let filler = match (saved.outgoing, saved.incoming) {
                (Some(frames), None) if t.cut_offset == Rational::ZERO =>
                    Some((FillerSide::Outgoing, &t.outgoing, &t.incoming, frames)),
                (None, Some(frames)) if t.cut_offset == t.duration =>
                    Some((FillerSide::Incoming, &t.incoming, &t.outgoing, frames)),
                _ => None,
            };
            if let Some((side, filler, survivor, _)) = filler.filter(|(_, filler, survivor, frames)| {
                *frames > 0 && filler.placements.is_empty() && survivor.reason.is_none()
                    && Rational::new(i128::from(*frames) * i128::from(s.rate.den()), i128::from(s.rate.num()))
                        .is_some_and(|extent| extent >= t.duration)
            }) {
                if let [id] = survivor.placements.as_slice() {
                    let p = placements[id.as_str()];
                    let at_cut = match side {
                        FillerSide::Outgoing => p.record_start == t.cut,
                        FillerSide::Incoming => add(p.record_start, p.duration)? == t.cut,
                    };
                    if t.kind == StreamKind::Audio && side == FillerSide::Incoming
                        && linear_audio.contains(t.id.as_str())
                        && s.rate == Rational::new(24_000, 1001).unwrap()
                        && stereo_fade_sources.contains(p.item.as_str())
                        && matches!(p.grouped_audio.as_slice(), [st] if st.track == t.track
                            && st.streams == [1, 2] && st.pans.iter().all(Option::is_none))
                        && p.audio_tracks.is_empty() && p.video_track.is_none()
                        && p.speed.is_none() && p.frame_map.is_none() && p.duration >= t.duration
                        && at_cut
                    {
                        stereo_tail_fades.push((t, id.as_str()));
                        continue;
                    }
                    if best_effort && at_cut && saved.same_rate
                        && isolated_filler_cut(s, t, p, filler, side, &facts.omitted_offline_items)?
                    {
                        let action = match side {
                            FillerSide::Outgoing => "omitted the fade from saved filler and kept the incoming edit at its saved cut.",
                            FillerSide::Incoming => "omitted the fade into saved filler and kept the outgoing edit through its saved cut.",
                        };
                        let lane = if t.kind == StreamKind::Video { "V" } else { "A" };
                        loss(losses, &t.id, "transition_cut", format!(
                            "{} · {lane}{} at {} seconds: {action}", s.name, t.track, t.cut));
                        continue;
                    }
                }
            }
        }
        if t.outgoing.reason.is_some()
            || t.incoming.reason.is_some()
            || t.outgoing.placements.len() != 1
            || t.incoming.placements.len() != 1
        {
            return Err(error(format!(
                "unresolved_transition_topology {}: endpoints are not a qualified single pair",
                t.id
            )));
        }
        let out = placements[t.outgoing.placements[0].as_str()];
        let input = placements[t.incoming.placements[0].as_str()];
        if out.id == input.id {
            return Err(error(format!(
                "unresolved_transition_topology {}: identical endpoints",
                t.id
            )));
        }
        let managed_motion = t.kind == StreamKind::Audio
            && t.style == "avid.BLEND_AUDIO_DISSOLVE"
            && linear_audio.contains(t.id.as_str())
            && s.rate == Rational::new(24_000, 1001).unwrap()
            && t.cut_offset > Rational::ZERO && t.cut_offset < t.duration
            && [out, input].iter().all(|p| {
                p.video_track.is_none() && p.audio_tracks == [t.track]
                    && p.audio_streams == [1] && p.grouped_audio.is_empty()
                    && side(&facts.sidecars, "avid.motion_adapter", &p.id)
                        .and_then(|a| key(a, "managed_pcm16")).and_then(boolean) == Some(true)
            });
        if [out, input]
            .iter()
            .any(|p| p.speed.is_some() || p.frame_map.is_some()
                || (!managed_motion && side(&facts.sidecars, "avid.motion_adapter", &p.id).is_some()))
        {
            return Err(error(format!(
                "retimed_transition_endpoint {}: ordinary trim arithmetic is unqualified",
                t.id
            )));
        }
        let begin = sub(t.cut, t.cut_offset)?;
        let end = add(begin, t.duration)?;
        if add(out.record_start, out.duration)? != end || input.record_start != begin {
            return Err(error(format!("unqualified_transition_edges {}", t.id)));
        }
        if !seen.insert((
            t.kind,
            t.track,
            t.cut,
            t.duration,
            t.cut_offset,
            t.style.as_str(),
        )) {
            continue;
        }
        let ot = trims.get_mut(out.id.as_str()).unwrap();
        ot.end = ot.end.min(t.cut);
        let it = trims.get_mut(input.id.as_str()).unwrap();
        it.start = it.start.max(t.cut);
        if t.style.starts_with("avid.") && [out, input].iter().any(|p| facts.omits_placement(p))
        {
            loss(losses, &t.id, "transition_omitted",
                format!("{} at {} seconds: omitted dissolve at its saved cut beside an unavailable offline source.",
                    s.name, t.cut));
            continue;
        }
        if t.kind == StreamKind::Audio {
            let linear = managed_motion || t.style == "avid.BLEND_AUDIO_DISSOLVE"
                && linear_audio.contains(t.id.as_str())
                && s.rate == Rational::new(24000, 1001).unwrap()
                && t.cut_offset > Rational::ZERO
                && t.cut_offset < t.duration
                && [out, input].iter().all(|p| {
                    p.audio_tracks == [t.track]
                        && p.grouped_audio.is_empty()
                        && linear_fade_clips.contains(p.item.strip_prefix("clip:").unwrap_or(&p.item))
                });
            let (style, name) = if linear {
                ("constant_gain", "Constant Gain")
            } else {
                ("constant_power", "Constant Power")
            };
            crossfades.push(AudioCrossfade {
                track: t.track,
                cut: ticks(t.cut, "transition cut")?,
                duration: ticks(t.duration, "transition duration")?,
                cut_offset: ticks(t.cut_offset, "transition offset")?,
                style: style.into(),
            });
            loss(
                losses,
                &t.id,
                "transition_approximation",
                format!("native transition {:?} prepared as {name}", t.style),
            );
            continue;
        } else if t.style == "avid.EFF2_BLEND_DISSOLVE"
            && t.cut_offset > Rational::ZERO && t.cut_offset < t.duration
            && [out, input].iter().all(|p| facts.routes[p.item.as_str()].video_rate == Some(s.rate))
        {
            dissolve_endpoints.push((t, out.id.as_str(), input.id.as_str()));
            loss(losses, &t.id, "transition_approximation",
                "Avid Dissolve mapped to editable Premiere Cross Dissolve (Legacy); Avid blending and custom parameters are not reproduced");
            continue;
        }
        loss(
            losses,
            &t.id,
            "transition_approximation",
            format!("native transition {:?} prepared as a video cut", t.style),
        );
    }
    *active_occurrence = None;
    let edit_marker_owners: HashSet<_> = if facts.same_rate_aac_views.is_empty() {
        HashSet::new()
    } else {
        s.markers.iter().filter(|m| m.owner == MarkerOwner::Edit)
            .flat_map(|m| m.binding.placements.iter().map(String::as_str)).collect()
    };
    let mut raw = Vec::new();
    let mut picture_edits = Vec::new();
    for p in &s.placements {
        if facts.omits_placement(p) {
            continue;
        }
        *active_occurrence = Some(PreparationOccurrence::Placement(&p.id));
        if side(&facts.sidecars, "avid.nested_motion", &p.id).is_some()
            && !facts.editable_nested_motion.contains(p.id.as_str())
        {
            let message = if p.speed.as_ref().is_some_and(|speed| speed.ratio == (1, 2)) {
                "The nested 200% edit requires a verified same-rate picture link in Separate files, or the qualified managed ProRes LT rewrap. Its picture is omitted."
            } else {
                "The nested constant-speed edit requires a verified same-rate picture link in Separate files. Its picture is omitted."
            };
            loss(losses, &p.id, "nested_motion", message);
            continue;
        }
        if facts.editable_nested_motion.contains(p.id.as_str())
            && (p.source_in.to_units(TICKS_PER_SECOND).is_none()
                || p.speed.as_ref().is_some_and(|speed|
                    speed.source_duration.to_units(TICKS_PER_SECOND).is_none()))
        {
            loss(losses, &p.id, "nested_motion",
                "The nested constant-speed edit has a source trim that Premiere cannot represent exactly in ticks. Its picture is omitted.");
            continue;
        }
        let tr = trims[p.id.as_str()];
        if tr.end <= tr.start {
            return Err(error(format!(
                "transition_handles_unavailable {}: nonpositive prepared span",
                p.id
            )));
        }
        let delta = sub(tr.start, p.record_start)?;
        let route = &facts.routes[p.item.as_str()];
        let mut offsets = Vec::new();
        if p.video_track.is_some() {
            offsets.push(
                route
                    .video
                    .flatten()
                    .ok_or_else(|| error(format!("unresolved_source_offset {}: picture", p.id)))?,
            );
        } else {
            let labels: Vec<i64> = if !p.grouped_audio.is_empty() {
                p.grouped_audio
                    .iter()
                    .flat_map(|a| a.streams.iter().copied())
                    .collect()
            } else if p.audio_streams.is_empty() {
                route.first_audio.into_iter().collect()
            } else {
                p.audio_streams.clone()
            };
            for label in labels {
                offsets.push(route.audio.get(&label).copied().flatten().ok_or_else(|| {
                    error(format!("unresolved_source_offset {}: A{label}", p.id))
                })?);
            }
        }
        let offset = *offsets
            .first()
            .ok_or_else(|| error(format!("unresolved_route {}", p.id)))?;
        if offsets.iter().any(|v| *v != offset) {
            return Err(error(format!(
                "incompatible_source_offsets {}: atomic route has different file origins",
                p.id
            )));
        }
        if offset != Rational::ZERO {
            return Err(error(format!(
                "nonzero_source_offset {}: preserved, separate qualification required",
                p.id
            )));
        }
        // Frame maps already address absolute media frames. Never add source origin again.
        let unit = model::ticks_per_unit(fraction(s.rate));
        let duration_ticks = ticks(sub(tr.end, tr.start)?, "duration")?;
        if side(&facts.sidecars, "avid.mp3_presentation", &p.id)
            .and_then(|data| key(data, "sample_rate"))
            .and_then(integer) == Some(44_100)
        {
            loss(losses, &p.id, "mp3_resampling",
                "The linked MP3 trim is preserved on its 44.1 kHz source clock. Premiere and Avid can render slightly different audio when resampling to 48 kHz.");
        }
        let adapter = side(&facts.sidecars, "avid.motion_adapter", &p.id);
        let mut sample_tail = false;
        if let Some(a) = adapter {
            let managed = key(a, "managed_pcm16").and_then(boolean) == Some(true);
            let reverse_pcm = key(a, "managed_pcm24").and_then(boolean) == Some(true);
            let native = key(a, "native_record_frames").and_then(integer)
                .filter(|n| (2..=16_384).contains(n));
            let sample = model::TICKS_PER_SECOND / 48_000;
            let tail = key(a, "silent_audio_tail_samples").and_then(integer)
                .filter(|n| (0..unit / sample).contains(n));
            let extent = native.zip(tail).and_then(|(n,t)| n.checked_mul(unit)?.checked_sub(t * sample));
            if !matches!((s.rate.num(), s.rate.den()), (24, 1) | (24_000, 1001))
                || extent != Some(ticks(p.duration, "saved adapter duration")?)
                || (!managed && (delta != Rational::ZERO || extent != Some(duration_ticks)))
                || p.speed.is_some()
                || (managed && (s.rate != Rational::new(24_000, 1001).unwrap()
                    || p.video_track.is_some() || p.frame_map.is_some()
                    || tail != Some(0) || p.audio_streams != [1]))
                || (reverse_pcm && (managed || s.rate != Rational::integer(24)
                    || p.video_track.is_some() || p.frame_map.is_some()
                    || tail != Some(0) || !matches!(p.audio_streams.as_slice(), [1] | [2])))
                || (p.video_track.is_some() && (p.frame_map.is_none() || tail != Some(0)))
                || (p.video_track.is_none() && (p.audio_tracks.len() != 1 || !p.grouped_audio.is_empty()
                    || ticks(p.source_in, "adapter audio in")? % sample != 0))
            {
                return Err(error(format!("invalid Motion Adapter preparation facts {}", p.id)));
            }
            sample_tail = p.video_track.is_none() && tail.is_some_and(|n| n != 0);
        }
        if unit <= 0
            || ticks(tr.start, "record start")? % unit != 0
            || (!sample_tail && duration_ticks % unit != 0)
        {
            return Err(error(format!("destination_record_grid {}: exact source record time/span is not representable on the current Premiere sequence frame grid",p.id)));
        }
        let receipt = side(&facts.sidecars, "avid.same_rate_aac", &p.id);
        let aac_view = facts.same_rate_aac_views.get(p.item.as_str())
            .filter(|_| !p.audio_tracks.is_empty() || !p.grouped_audio.is_empty());
        if receipt.is_some() && aac_view.is_none() {
            return Err(error(format!("{}: linked AAC edit receipt has no matching original source", p.id)));
        }
        let clip = if let Some(view) = aac_view {
            let original = sub(p.source_in, Rational::new(s.rate.den().into(), s.rate.num().into())
                .ok_or_else(|| error("invalid linked AAC sequence clock"))?)?;
            let saved_source_in = original.to_string();
            if Some(s.rate) != route.video_rate || p.video_track.is_some()
                || p.audio_tracks.len() != 1 || !matches!(p.audio_streams.as_slice(), [1] | [2])
                || !p.grouped_audio.is_empty() || p.speed.is_some() || p.frame_map.is_some()
                || !p.effects.is_empty() || !p.volume_keys.is_empty()
                || delta != Rational::ZERO || tr.end != add(p.record_start, p.duration)?
                || original.is_negative() || edit_marker_owners.contains(p.id.as_str())
                || !route.video_duration.is_some_and(|extent|
                    p.source_in.checked_add(p.duration).is_some_and(|end| end <= extent))
                || receipt.and_then(|r| key(r, "file_track_start")).and_then(integer) != Some(1)
                || receipt.and_then(|r| key(r, "saved_source_in")).and_then(str_value) != Some(saved_source_in.as_str())
            {
                return Err(error(format!("{}: linked AAC audio needs a verified one-frame offset and a playable straight mono route", p.id)));
            }
            view.clone()
        } else if !p.audio_tracks.is_empty() && p.grouped_audio.is_empty()
            && facts.pcm_movie_mono_views.contains_key(p.item.as_str())
        {
            facts.pcm_movie_mono_views[p.item.as_str()].clone()
        } else {
            clip_id(&p.item)
        };
        let source = if p.frame_map.is_some() {
            p.source_in
        } else {
            add(p.source_in, delta)?
        };
        if p.frame_map.is_some() && side(&facts.sidecars, "avid.source", &p.id)
            .and_then(|v| key(v, "saved_timewarp")).and_then(boolean) == Some(true)
        {
            let reason = if (s.rate.num(), s.rate.den()) == (24_000, 1001) {
                "Calculated one Premiere Time Remapping key per frame from Avid's saved Position curve. Premiere uses frame sampling and leaves out the original handles and inherited Avid render settings."
            } else {
                "Saved Timewarp picture cadence becomes Premiere Time Remapping with one key per frame. Avid Speed keys and handles are not recreated."
            };
            loss(losses, &p.id, "timewarp_editability", reason);
        }
        let mut cc = None;
        let mut matte = None;
        let mut picture = None;
        let mut must_key = false;
        let mut chain_transform = None;
        let mut reflection_before_motion = [false; 2];
        let mut replace_transform = false;
        for e in &p.effects {
            match e.kind.as_str() {
                "picture.corner_pin" => {
                    let pin = picture::corner_pin(&e.params)
                        .ok_or_else(|| error(format!("invalid Corner Pin points or source crop for {}", p.id)))?;
                    if p.video_track.is_none() || p.effects.len() != 1 || p.transform.is_some()
                        || p.speed.is_some() || p.frame_map.is_some()
                        || !route.supports_corner_pin(s.rate, frame_size)
                    {
                        loss(losses, &p.id, "picture_effect",
                            "Corner Pin needs an isolated same-rate picture edit with matching verified geometry at a supported clock");
                    } else {
                        picture = Some(prproj::convert::prepared::PictureEffect::CornerPin(pin));
                    }
                }
                graph::picture_chain::KIND => {
                    if p.effects.len() != 1 || p.transform.is_some() {
                        return Err(error(format!("ordered picture chain needs one exclusive picture effect at {}", p.id)));
                    }
                    must_key = picture::has_spectramatte(e);
                    let full_span = tr.start == p.record_start && tr.end == add(p.record_start, p.duration)?;
                    (chain_transform, picture, reflection_before_motion) = picture::prepare_chain(
                        p, e, full_span, s.rate, frame_size, route, losses,
                    )?;
                }
                "avid.color_correction" if cc.is_none() => {
                    cc = Some(bridge::color_correction_from(
                        &e.params,
                        source_losses(&facts.losses, &p.id, "color_correction"),
                    )?)
                }
                "avid.animatte" if matte.is_none() => {
                    let mut parsed = bridge::matte_from(
                        &e.params,
                        source_losses(&facts.losses, &p.id, "matte"),
                    )?;
                    if parsed.inverted && parsed.shapes.len() > 1 {
                        loss(losses, &p.id, "matte",
                            "Premiere output omits this multi-shape AniMatte Key Out. The source graph retains its contours and keys.");
                    } else if key(&e.params, "acceleration_q30").and_then(integer) == Some(1 << 29) {
                        let qualified = !parsed.inverted
                            && parsed.opacities.iter().all(|opacity| *opacity == 100.0)
                            && parsed.feathers.iter().all(|feather| *feather == (0.0, 0.0))
                            && p.video_track.is_some()
                            && p.effects.len() == 1
                            && p.transform.is_none()
                            && p.speed.is_none()
                            && p.frame_map.is_none()
                            && route.video_rate == Some(s.rate)
                            && route.video_raster == Some((1920, 1080))
                            && frame_size == (1920, 1080)
                            && s.rate == Rational::integer(24)
                            && delta == Rational::ZERO
                            && duration_ticks % unit == 0
                            && tr.end == add(p.record_start, p.duration)?;
                        if qualified && sample_animatte_acceleration50(
                            &mut parsed, duration_ticks / unit, animatte_points_remaining,
                        ) {
                            loss(losses, &p.id, "matte_approximation",
                                "AniMatte Acceleration 50 approximated with editable per-frame Mask Path keys");
                        } else {
                            loss(losses, &p.id, "matte",
                                "AniMatte Acceleration 50 is outside the sampled Key In profile or exceeds the path budget. The saved paths use Premiere's default interpolation.");
                        }
                    }
                    if parsed.inverted && parsed.shapes.len() == 1
                        && parsed.opacities.iter().any(|opacity| *opacity > 0.0 && *opacity < 100.0)
                    {
                        loss(losses, &p.id, "matte_approximation",
                            "Partial AniMatte Key Out uses an editable inverted mask. Premiere can attenuate the picture outside the contour, where Avid preserves it in the tested case.");
                    }
                    matte = Some(parsed);
                }
                kind if mapping::picture_chain::is_reflection_kind(kind) && picture.is_none() => {
                    let axes = graph::picture_chain::Reflection::from_params(&e.params)
                        .filter(|_| p.video_track.is_some())
                        .ok_or_else(|| error(format!("invalid reflection effect for {}", p.id)))?;
                    if kind != "avid.flip" && (p.transform.is_some() || p.effects.len() != 1) {
                        loss(losses, &p.id, "picture_effect",
                            "Reflection with another picture control requires a qualified effect order");
                    } else {
                        picture = Some(prproj::convert::prepared::PictureEffect::Flip {
                            horizontal: axes.horizontal,
                            vertical: axes.vertical,
                        });
                    }
                }
                "avid.warp_roll_linear" | "avid.warp_roll_shelf"
                | "avid.warp_roll_bezier" | "avid.warp_roll_spline" if picture.is_none() => {
                    if p.effects.len() != 1 || route.chain_geometry(frame_size) == picture::ChainGeometry::Unverified
                        || !prproj::convert::prepared::supports_picture_motion_clock(fraction(s.rate))
                    {
                        loss(losses, &p.id, "picture_effect",
                            "Animated Warp Rotation requires one isolated edit on a supported clock with matching 1080p or verified linked geometry");
                        continue;
                    }
                    let full_span = tr.start == p.record_start && tr.end == add(p.record_start, p.duration)?;
                    let same_rate = route.video_rate == Some(s.rate);
                    picture = picture::prepare(p, &e.kind, &e.params, full_span, same_rate, unit, route.video_duration, losses)?;
                }
                "avid.resize_linear" | "avid.warp_linear"
                | "avid.resize_crop_linear" | "avid.warp_crop_linear" | "avid.warp_trim_linear"
                | "avid.resize_trim_linear" | "avid.resize_trim_crop_linear" | "avid.warp_trim_crop_linear" if picture.is_none() => {
                    let keyed_crop = picture::is_keyed_crop(&e.kind);
                    let trimmed = picture::is_trimmed_motion(&e.kind);
                    let non_hd = frame_size != (1920, 1080)
                        || route.video_raster.is_some_and(|raster| raster != (1920, 1080));
                    replace_transform |= keyed_crop || trimmed || e.kind == "avid.warp_linear" || non_hd;
                    if non_hd && !route.chain_geometry(frame_size).supports_linked_motion(&e.kind, fraction(s.rate)) {
                        loss(losses, &p.id, "picture_effect",
                            "Linear Motion outside 1080p needs matching verified linked geometry and a supported key clock");
                        continue;
                    }
                    if !picture::supports_motion_clock(&e.kind, fraction(s.rate)) {
                        loss(losses, &p.id, "picture_effect",
                            "Animated crop or trimmed Linear Motion requires a supported destination sequence clock");
                        continue;
                    }
                    let full_span = tr.start == p.record_start && tr.end == add(p.record_start, p.duration)?;
                    let same_rate = route.video_rate == Some(s.rate);
                    picture = picture::prepare(p, &e.kind, &e.params, full_span, same_rate, unit, route.video_duration, losses)?;
                }
                graph::audio_pan::LINEAR_KIND => {} // Prepared with the final audio lane binding.
                _ => loss(
                    losses,
                    &p.id,
                    "effect",
                    format!(
                        "unsupported or repeated effect {} retained in source",
                        e.kind
                    ),
                ),
            }
        }
        if must_key && !matches!(picture, Some(prproj::convert::prepared::PictureEffect::UltraKey { .. })) {
            // An opaque foreground would cover the underlying edit.
            continue;
        }
        if let Some(effect) = picture {
            picture_edits.push((p.id.as_str(), effect, reflection_before_motion));
        }
        raw.push((
            p.id.as_str(),
            Event {
                clip,
                record_start: ticks(tr.start, "record start")?,
                source_in: ticks(source, "source in")?,
                duration: duration_ticks,
                video_track: p.video_track,
                audio_tracks: p.audio_tracks.clone(),
                audio_channels: p.audio_streams.clone(),
                audio_gains: if source_gain.is_dry(p)
                    && tr.start == p.record_start && tr.end == add(p.record_start, p.duration)? {
                    vec![Some(0.0)]
                } else {
                    p.track_gains_db.clone()
                },
                audio_pans: p.track_pans.clone(),
                audio: EventAudio {
                    gain_db: p.gain_db,
                    volume_db: p.volume_db,
                    volume_keys: prepare_volume_keys(facts, p, losses)?,
                },
                stereo: p
                    .grouped_audio
                    .iter()
                    .map(|x| StereoEdit {
                        audio_track: x.track,
                        channels: x.streams.clone(),
                        gain_db: x.gain_db,
                        pans: x.pans.clone(),
                    })
                    .collect(),
                speed_ratio: p.speed.as_ref().map(|x| x.ratio),
                source_duration: p
                    .speed
                    .as_ref()
                    .map(|x| ticks(x.source_duration, "source duration"))
                    .transpose()?,
                time_remap: p.frame_map.clone(),
                color_correction: cc,
                matte,
                transform: chain_transform.as_ref().or_else(|| p.transform.as_ref()
                    .filter(|_| !replace_transform || picture.is_some())).map(|t| Transform {
                    position: t.position,
                    scale: t.scale,
                    rotation: t.rotation,
                    anchor: t.anchor,
                    crop: t.crop,
                    not_converted: source_losses(&facts.losses, &p.id, "transform"),
                }),
            },
        ));
    }
    *active_occurrence = None;
    // Assemble only compatible spans after all lane-specific trims. Preserve two
    // simultaneous picture lanes as separate events and genuine combiners atomically.
    let mut events: Vec<Event> = Vec::new();
    let mut source_groups: Vec<Vec<&str>> = Vec::new();
    let mut cohorts: HashMap<(String, i64, i64, i64, Option<(i64, i64)>, Option<i64>), Vec<usize>> =
        HashMap::new();
    for (id, mut e) in raw {
        let key = (
            e.clip.clone(),
            e.record_start,
            e.source_in,
            e.duration,
            e.speed_ratio,
            e.source_duration,
        );
        let candidates = cohorts.entry(key).or_default();
        let found = candidates.iter().copied().find(|&i| {
            let x = &events[i];
            x.time_remap == e.time_remap
                && (x.video_track.is_none() || e.video_track.is_none())
                && x.audio == e.audio
        });
        if let Some(i) = found {
            let x = &mut events[i];
            if e.video_track.is_some() {
                x.video_track = e.video_track;
                x.color_correction = e.color_correction;
                x.transform = e.transform;
                x.matte = e.matte;
            }
            if !x.audio_gains.is_empty() || !e.audio_gains.is_empty() {
                x.audio_gains.resize(x.audio_tracks.len(), None);
                e.audio_gains.resize(e.audio_tracks.len(), None);
            }
            if !x.audio_pans.is_empty() || !e.audio_pans.is_empty() {
                x.audio_pans.resize(x.audio_tracks.len(), None);
                e.audio_pans.resize(e.audio_tracks.len(), None);
            }
            x.audio_tracks.extend(e.audio_tracks);
            x.audio_channels.extend(e.audio_channels);
            x.audio_gains.extend(e.audio_gains);
            x.audio_pans.extend(e.audio_pans);
            x.stereo.extend(e.stereo);
            source_groups[i].push(id);
        } else {
            candidates.push(events.len());
            events.push(e);
            source_groups.push(vec![id]);
        }
    }
    let mut assembled: Vec<_> = events.into_iter().zip(source_groups).collect();
    assembled.sort_by_key(|(e, _)| (e.record_start, e.video_track.unwrap_or(0)));
    let mut sorted = Vec::with_capacity(assembled.len());
    for (mut event, sources) in assembled {
        let index = sorted.len();
        for id in sources {
            ownership.insert(id.into(), (si, index));
        }
        if event.audio_gains.iter().all(Option::is_none) {
            event.audio_gains.clear();
        }
        if event.audio_pans.iter().all(Option::is_none) {
            event.audio_pans.clear();
        }
        sorted.push(event);
    }
    for (t, outgoing, incoming) in dissolve_endpoints {
        *active_occurrence = Some(PreparationOccurrence::Transition(&t.id));
        video_dissolves.push(PreparedVideoDissolve {
            sequence: si,
            outgoing: ownership[outgoing].1,
            incoming: ownership[incoming].1,
            duration: ticks(t.duration, "dissolve duration")?,
            before_cut: ticks(t.cut_offset, "dissolve offset")?,
        });
    }
    for (t, outgoing) in stereo_tail_fades {
        *active_occurrence = Some(PreparationOccurrence::Transition(&t.id));
        audio_edge_fades.push(prproj::convert::prepared::PreparedAudioEdgeFade {
            sequence: si, event: ownership[outgoing].1, track: t.track, head: 0,
            tail: ticks(t.duration, "stereo fade duration")?,
        });
    }
    *active_occurrence = None;
    for (id, effect, reflection_before_motion) in picture_edits {
        picture_effects.push(prproj::convert::prepared::PreparedPictureEffect {
            sequence: si, event: ownership[id].1, effect, reflection_before_motion,
        });
    }
    let mut markers: Vec<Marker> = Vec::new();
    let mut edit_matches = HashMap::new();
    for (marker_index, m) in s.markers.iter().enumerate() {
        let Some(mut out) = marker(m, losses, &s.id, marker_index, true)? else {
            continue;
        };
        if m.owner == MarkerOwner::Edit {
            let candidates: HashSet<_> = m
                .binding
                .placements
                .iter()
                .filter_map(|id| ownership.get(id).map(|(_, i)| *i))
                .filter(|&i| {
                    sorted[i].record_start <= out.start
                        && sorted[i]
                            .record_start
                            .checked_add(sorted[i].duration)
                            .is_some_and(|end| {
                                out.start < end
                                    && out
                                        .start
                                        .checked_add(out.duration)
                                        .is_some_and(|marker_end| marker_end <= end)
                            })
                })
                .collect();
            if out.owner == MarkerOwner::Sequence.as_str() {
                if m.binding.reason.is_none() && candidates.len() == 1 {
                    record_only_markers.insert((si, marker_index));
                }
                markers.push(out);
                continue;
            }
            if m.binding.reason.is_none() && candidates.len() == 1 {
                let i = *candidates.iter().next().unwrap();
                out.event = Some(i as i64);
                if out.source.is_none() {
                    let e = &sorted[i];
                    if e.speed_ratio.is_none() && e.time_remap.is_none() {
                        out.source = e.source_in.checked_add(out.start - e.record_start);
                    }
                }
                let event = &sorted[i];
                if event.speed_ratio.is_none() && event.time_remap.is_none() {
                    let mut source_tracks = m.binding.placements.iter()
                        .filter(|id| ownership.get(*id) == Some(&(si, i)))
                        .filter_map(|id| {
                            let placement = placements[id.as_str()];
                            marker_source_track(m, placement, &facts.routes[placement.item.as_str()])
                        });
                    let track = source_tracks.next()
                        .filter(|track| source_tracks.all(|candidate| candidate == *track));
                    if let (Some(source), Some(created), Some(item), Some(track)) = (
                        out.source,
                        facts
                            .native_markers
                            .get(&(s.id.as_str(), marker_index))
                            .and_then(|v| native_created(v)),
                        facts.items_by_clip.get(event.clip.as_str()),
                        track,
                    ) {
                        out.inherited = false;
                        let key = inherited_key(&item.id, m, track, source, created);
                        if out.track.is_some()
                            && facts.source_marker_uniqueness.get(&key) == Some(&true)
                        {
                            // A unique source marker can match once per edit lane.
                            // Multiple matching edit markers need the conflict policy.
                            match edit_matches.entry((i, m.track, key)) {
                                Entry::Vacant(entry) => {
                                    entry.insert(Some(markers.len()));
                                    out.inherited = true;
                                }
                                Entry::Occupied(mut entry) => {
                                    if let Some(first) = entry.get_mut().take() {
                                        markers[first].inherited = false;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if out.event.is_none() || out.source.is_none() || out.track.is_none() {
                out.owner = "sequence".into();
                out.event = None;
                out.source = None;
                out.inherited = false;
                loss(losses,&s.id,"marker_ownership",format!("marker {:?}: record time preserved on sequence; edit ownership unavailable after preparation",m.name));
            }
        }
        markers.push(out);
    }
    offline_gap::append(facts, s, &trims, &sorted, losses, &mut markers)?;
    let start = exact_start.or_else(|| {
        side(&facts.sidecars, "avid.source", &s.id)
            .and_then(|v| key(v, "compatibility_start"))
            .and_then(str_value)
            .and_then(Rational::parse)
    })
        .ok_or_else(|| error(format!("{}: missing compatibility start provenance", s.id)))?;
    for t in &s.tracks {
        if t.kind == StreamKind::Video && t.level_db.is_some() {
            loss(
                losses,
                &s.id,
                "tracks",
                format!(
                    "picture track V{} level has no destination model representation",
                    t.index
                ),
            );
        }
    }
    Ok(Sequence {
        name: s.name.clone(),
        rate: fraction(s.rate),
        width: frame_size.0,
        height: frame_size.1,
        start: ticks(start, "sequence start")?,
        events: sorted,
        markers,
        track_levels: s
            .tracks
            .iter()
            .filter_map(|t| {
                if t.kind == StreamKind::Audio {
                    t.level_db.map(|db| TrackLevel { track: t.index, db })
                } else {
                    None
                }
            })
            .collect(),
        audio_crossfades: crossfades,
    })
}
