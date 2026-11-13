//! Offline checks of saved source frames and supplied pictures at V1 cuts.
mod renders;
mod nests;
mod multicam;

use super::{compare, metadata, read_drp_bytes, report, ADAPTER, OMITTED_TRACKS_ADAPTER};
use crate::{
    dissolve,
    prepare::frames_on,
    relink::{self, Destinations},
    retime::PictureSpeed,
    Result,
};
use graph::{
    source::{SourceGraph, SourceMedia, SourceSequence, TextKind},
    Placement, Rational, StreamKind, Value,
};
use serde::Serialize;
use serde_json::{json, Value as Json};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{Read, Write},
    ops::Range,
    path::Path,
};

const MAX_EDITS: usize = 8192;
const MAX_FULL_SCAN_FRAMES: i64 = 16_384;
const NO_V1_EDIT: &str = "no saved V1 edit covers this frame";
const SAVED_V1_GAP: &str = "saved V1 gap";
const UNQUALIFIED_V1: &str = "unqualified V1 coverage at this frame";

pub use renders::Renders;

pub struct MoveOptions<'a> {
    pub moves: &'a Path,
    pub checksums: Option<&'a Path>,
}

#[derive(Default)]
pub struct Options<'a> {
    pub renders: Option<Renders<'a>>,
    pub moves: Option<MoveOptions<'a>>,
    pub all_frames: bool,
}

pub fn write(
    reference: &Path,
    reference_id: &str,
    candidate: &Path,
    candidate_id: &str,
    options: Options<'_>,
    output: &mut impl Write,
) -> Result<()> {
    let snapshots = options
        .moves
        .map(|options_moves| -> Result<_> {
            let reference_bytes = relink::resolve_project_bytes(reference)?;
            let candidate_bytes = relink::resolve_project_bytes(candidate)?;
            let ffprobe = options
                .renders
                .as_ref()
                .map_or(Path::new("ffprobe"), |r| r.ffprobe);
            let verified = relink::verify_resolve_moves(
                &reference_bytes,
                &relink::read_moves(options_moves.moves)?,
                ffprobe,
                options_moves.checksums,
            )?;
            let mode = if options_moves.checksums.is_some() {
                "sha256"
            } else {
                "path"
            };
            Ok((reference_bytes, candidate_bytes, verified, mode))
        })
        .transpose()?;
    let (left, right) = if let Some((a, b, _, _)) = &snapshots {
        (read_drp_bytes(a)?, read_drp_bytes(b)?)
    } else {
        (read_graph(reference)?, read_graph(candidate)?)
    };
    let left = timeline(&left, reference_id)?;
    let right = timeline(&right, candidate_id)?;
    let moves = snapshots
        .as_ref()
        .map(|(_, _, verified, _)| &verified.destinations);
    let mut result = compare(&left, &right, moves, options.all_frames)?;
    result.move_verification = snapshots.as_ref().map(|(_, _, _, mode)| *mode);
    result.picture = match options.renders {
        None => json!({"status": "not_requested"}),
        Some(renders) => renders::compare(renders, &left, &right, &result.samples),
    };
    report::write(output, &result)?;
    if let Some((a, b, verified, _)) = &snapshots {
        if relink::resolve_project_bytes(reference)? != *a {
            return Err("reference project changed during conform verification".into());
        }
        if relink::resolve_project_bytes(candidate)? != *b {
            return Err("candidate project changed during conform verification".into());
        }
        verified.recheck()?;
    }
    Ok(())
}

fn read_graph(path: &Path) -> Result<SourceGraph> {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        if !fs::metadata(path)?.is_file() {
            return Err("expected a regular graph/2 file".into());
        }
        let file = File::open(path)?;
        let mut bytes = Vec::new();
        file.take(graph::MAX_GRAPH_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > graph::MAX_GRAPH_BYTES {
            return Err("saved graph exceeds 64 MiB".into());
        }
        Ok(SourceGraph::from_json_text(std::str::from_utf8(&bytes)?)?)
    } else {
        compare::read_graph(path)
    }
}

#[derive(Serialize)]
struct Report<'a> {
    schema: &'static str,
    reference_id: &'a str,
    candidate_id: &'a str,
    rate: String,
    reference_origin: Option<String>,
    candidate_origin: Option<String>,
    mapping_status: &'static str,
    mapping_reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sampling: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    move_verification: Option<&'static str>,
    excluded_dissolve_windows: Vec<Range<i64>>,
    samples: Vec<Sample<'a>>,
    picture: Json,
}

#[derive(Serialize)]
struct Sample<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    cut: Option<i64>,
    record_frame: i64,
    status: &'static str,
    differences: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unresolved_identity: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_candidate_locator: Option<&'a str>,
    reference: Option<Mapping<'a>>,
    candidate: Option<Mapping<'a>>,
    reference_reason: Option<&'static str>,
    candidate_reason: Option<&'static str>,
    #[serde(skip)]
    picture_eligible: bool,
}

#[derive(Serialize)]
struct Mapping<'a> {
    edit: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    nest: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multicam_use: Option<&'a str>,
    locator: &'a str,
    source_frame: i64,
    #[serde(serialize_with = "serialize_rate")]
    source_rate: Rational,
    physical_stream: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_timecode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reel: Option<&'a str>,
}

fn serialize_rate<S: serde::Serializer>(rate: &Rational, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_str(rate)
}

struct Edit<'a> {
    nest: Option<&'a str>,
    multicam_use: Option<&'a str>,
    range: Range<i64>,
    content: EditContent<'a>,
}

#[derive(Clone, Copy)]
enum EditContent<'a> {
    Media {
        placement: &'a Placement,
        elapsed_offset: i64,
        source: std::result::Result<Source<'a>, &'static str>,
    },
    Gap,
}

#[derive(Clone, Copy)]
struct Source<'a> {
    locator: &'a str,
    frames: SourceFrames<'a>,
    rate: Rational,
    physical_stream: Option<i64>,
    source_timecode: Option<&'a str>,
    reel: Option<&'a str>,
}

#[derive(Clone, Copy)]
enum SourceFrames<'a> {
    Linear {
        first: i64,
        step: i64,
        divisor: i64,
    },
    Explicit(&'a [i64]),
}

impl SourceFrames<'_> {
    fn at(&self, elapsed: i64) -> i64 {
        match self {
            Self::Linear { first, step, divisor } => first + step * (elapsed / divisor),
            Self::Explicit(frames) => frames[elapsed as usize],
        }
    }
}

struct Timeline<'a> {
    sequence: &'a SourceSequence,
    edits: Vec<Edit<'a>>,
    extent: Option<i64>,
    unqualified: Vec<Range<i64>>,
    coverage_boundaries: Vec<i64>,
    dissolve: Option<Range<i64>>,
    issue: Option<&'static str>,
}

fn timeline<'a>(graph: &'a SourceGraph, id: &str) -> Result<Timeline<'a>> {
    let seq = graph
        .sequences
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("conform timeline {id} was not found"))?;
    let bindings = Bindings::new(graph);
    let mut result = direct_timeline(graph, seq, &bindings)?;
    nests::add(graph, &bindings, &mut result)?;
    result.edits.sort_by_key(|edit| edit.range.start);
    result.unqualified.sort_by_key(|range| range.start);
    Ok(result)
}

struct Bindings<'a> {
    items: HashMap<&'a str, &'a graph::source::SourceItem>,
    media: HashMap<&'a str, &'a SourceMedia>,
    metadata: HashMap<&'a str, &'a Value>,
}

impl<'a> Bindings<'a> {
    fn new(graph: &'a SourceGraph) -> Self {
        Self {
            items: graph.items.iter().map(|item| (item.id.as_str(), item)).collect(),
            media: graph.media.iter().map(|source| (source.id.as_str(), source)).collect(),
            metadata: graph.sidecars.iter().filter(|row| row.adapter == metadata::ADAPTER)
                .map(|row| (row.object.as_str(), &row.data)).collect(),
        }
    }
}

fn direct_timeline<'a>(
    graph: &'a SourceGraph,
    seq: &'a SourceSequence,
    bindings: &Bindings<'a>,
) -> Result<Timeline<'a>> {
    let id = seq.id.as_str();
    let mut result = Timeline {
        sequence: seq,
        edits: Vec::new(),
        extent: None,
        unqualified: Vec::new(),
        coverage_boundaries: Vec::new(),
        dissolve: None,
        issue: None,
    };
    if !matches!((seq.rate.num(), seq.rate.den()), (24, 1) | (24000, 1001)) {
        result.issue = Some("unsupported timeline rate");
        return Ok(result);
    }
    let picture = seq.placements.iter().filter(|p| p.video_track == Some(1));
    if picture.clone().count() > MAX_EDITS {
        return Err("V1 edit count exceeds conform report limit".into());
    }
    let conflicting_locators: HashSet<_> = graph
        .losses
        .iter()
        .filter(|loss| loss.object == id && loss.aspect == "locator")
        .filter_map(|loss| loss.reason.split_once(": ").map(|(edit, _)| edit))
        .collect();
    let range = |start, length| frame_range(start, length, seq.rate);
    for placement in picture {
        match range(placement.record_start, placement.duration) {
            Ok(range) => {
                let item = bindings.items[placement.item.as_str()];
                let mut video = item.media.iter().filter_map(|link| {
                    let source = bindings.media[link.media.as_str()];
                    (source.kind == StreamKind::Video).then_some((source, link.source_offset))
                });
                let binding = match (video.next(), video.next()) {
                    _ if conflicting_locators.contains(placement.id.as_str()) => {
                        Err("timeline locator disagrees with the saved media cache")
                    }
                    (Some((source, offset)), None) => saved_source(
                        placement,
                        &range,
                        source,
                        offset,
                        seq.rate,
                        bindings.metadata.get(item.id.as_str()).copied(),
                    ),
                    _ => Err("picture binding does not identify one video stream"),
                };
                result.edits.push(Edit {
                    nest: None,
                    multicam_use: None,
                    range,
                    content: EditContent::Media {
                        placement,
                        elapsed_offset: 0,
                        source: binding,
                    },
                });
            }
            Err(_) => result.issue = Some("a V1 edit is outside the exact timeline frame grid"),
        }
    }
    let inferred_extent = seq
        .placements
        .iter()
        .try_fold(0, |end, p| -> Result<i64> {
            let time = p
                .record_start
                .checked_add(p.duration)
                .ok_or("timeline extent overflow")?;
            // Compare the maximum end as a rational before requiring a whole picture frame.
            Ok(end.max(ceil_frame(time, seq.rate)?))
        })
        .ok();
    result.extent = match graph
        .sidecar(ADAPTER, id)
        .and_then(|v| v.get("record_length"))
    {
        Some(Value::Int(end)) if *end >= 0 && inferred_extent.is_some_and(|n| n <= *end) => {
            Some(*end)
        }
        Some(_) => None,
        None => inferred_extent,
    };
    track_unknowns(graph, &mut result)?;
    multicam::add(graph, &mut result)?;
    result.edits.sort_by_key(|edit| edit.range.start);
    result.unqualified.sort_by_key(|range| range.start);
    Ok(result)
}

fn ceil_frame(time: Rational, rate: Rational) -> Result<i64> {
    let n = i128::from(time.num()) * i128::from(rate.num());
    let d = i128::from(time.den()) * i128::from(rate.den());
    if n < 0 {
        return Err("negative timeline extent".into());
    }
    Ok(i64::try_from(n / d + i128::from(n % d != 0))?)
}

fn frame_range(start: Rational, length: Rational, rate: Rational) -> Result<Range<i64>> {
    let start = frames_on(start, rate)?;
    let length = frames_on(length, rate)?;
    let end = start
        .checked_add(length)
        .filter(|&end| end > start)
        .ok_or("invalid frame interval")?;
    Ok(start..end)
}

fn saved_source<'a>(
    edit: &'a Placement,
    range: &Range<i64>,
    media: &'a SourceMedia,
    offset: Option<Rational>,
    rate: Rational,
    saved_metadata: Option<&'a Value>,
) -> std::result::Result<Source<'a>, &'static str> {
    let [path] = media.locations.as_slice() else {
        return Err("picture binding does not identify one saved locator");
    };
    let windows_absolute = path
        .as_bytes()
        .get(1..3)
        .is_some_and(|bytes| matches!(bytes, b":\\" | b":/"))
        && path.as_bytes()[0].is_ascii_alphabetic();
    if !(Path::new(path).is_absolute() || windows_absolute || path.starts_with("\\\\")) {
        return Err("picture binding lacks a full saved locator");
    }
    if media
        .original
        .as_ref()
        .is_some_and(|file| file.saved_locators != media.locations)
    {
        return Err("picture binding uses a relocated original");
    }
    let length = range.end - range.start;
    let source_rate = media.rate.ok_or("unknown source picture rate")?;
    let frames = if let Some(map) = &edit.frame_map {
        // Graph/2 validates these as absolute file frames in the picture clock.
        if edit.speed.is_some() || usize::try_from(length).ok() != Some(map.len()) {
            return Err("ambiguous explicit picture timing");
        }
        SourceFrames::Explicit(map)
    } else {
        if source_rate != rate {
            return Err("unsupported picture timing or source rate");
        }
        let count = frames_on(media.duration, source_rate)
            .map_err(|_| "unknown whole-frame source extent")?;
        let offset = offset.ok_or("unknown item-to-file source offset")?;
        let source_time = edit.source_in.checked_add(offset).ok_or("source offset overflow")?;
        let first = frames_on(source_time, rate)
            .map_err(|_| "source time is outside the exact picture frame grid")?;
        let (span, step, divisor) = match &edit.speed {
            None => (length, 1, 1),
            Some(speed) => {
                let mode = PictureSpeed::from_ratio(speed.ratio)
                    .filter(|mode| rate == Rational::integer(24) && mode.graph(length).as_ref() == Some(speed))
                    .ok_or("unsupported constant picture speed or source span")?;
                let sampled = mode.sampled_source_frames(length)
                    .map_err(|_| "invalid constant picture speed extent")?;
                match mode {
                    PictureSpeed::Half => (sampled, 1, 2),
                    PictureSpeed::Reverse => (sampled, -1, 1),
                }
            }
        };
        let end = first.checked_add(span)
            .filter(|&end| first >= 0 && end <= count)
            .ok_or("picture edit exceeds the saved source extent")?;
        let first = if step < 0 { end - 1 } else { first };
        SourceFrames::Linear { first, step, divisor }
    };
    let saved = |field| match saved_metadata.and_then(|data| data.get(field)) {
        Some(Value::Str(value)) if !value.is_empty() => Some(value.as_str()),
        _ => None,
    };
    Ok(Source {
        locator: path,
        frames,
        rate: source_rate,
        physical_stream: media.original.as_ref().map(|file| file.stream_index),
        source_timecode: saved(metadata::TIMECODE),
        reel: saved(metadata::REEL),
    })
}

fn track_unknowns(graph: &SourceGraph, timeline: &mut Timeline<'_>) -> Result<()> {
    let seq = timeline.sequence;
    let rate = seq.rate;
    let mut identified = HashSet::new();
    let nest_ids: HashSet<_> = match graph.sidecar(crate::nests::ADAPTER, &seq.id) {
        Some(Value::Arr(rows)) => rows.iter().filter_map(|row| crate::nests::string(row, "id")).collect(),
        _ => HashSet::new(),
    };
    if let Some(Value::Arr(rows)) = graph.sidecar(OMITTED_TRACKS_ADAPTER, &seq.id) {
        for row in rows {
            if matches!(row.get("kind"), Some(Value::Str(kind)) if kind == "audio") {
                if let Some(Value::Str(id)) = row.get("id") {
                    identified.insert(("track", id.as_str()));
                }
            }
        }
    }
    for t in seq
        .transitions
        .iter()
        .filter(|t| t.kind == StreamKind::Video && t.track == 1)
    {
        match t
            .cut
            .checked_sub(t.cut_offset)
            .and_then(|start| frame_range(start, t.duration, rate).ok())
        {
            Some(range) => timeline.unqualified.push(range),
            None => timeline.issue = Some("unqualified V1 transition coverage"),
        }
    }
    let origin = seq.start.and_then(|start| frames_on(start, rate).ok());
    if let Some(data) = graph.sidecar(dissolve::ADAPTER, &seq.id) {
        if let Value::Arr(rows) = data {
            let qualified = dissolve::prepare(graph, seq).ok().flatten();
            if let Some(known) = qualified.as_ref().filter(|d| !d.audio) {
                timeline.dissolve = Some(known.window());
            }
            for row in rows {
                if let Some(Value::Str(id)) = row.get("id") {
                    identified.insert(("transition", id.as_str()));
                }
                if matches!(row.get("kind"), Some(Value::Str(kind)) if kind == "audio")
                    || matches!(row.get("track"), Some(Value::Int(track)) if *track > 1)
                    || qualified.is_some()
                {
                    continue;
                }
                let grid_agrees = row.get("timeline_origin_frames")
                    == origin.as_ref().map(|origin| Value::Int(*origin)).as_ref()
                    && row.get("track_rate_numerator") == Some(&Value::Int(rate.num()))
                    && row.get("track_rate_denominator") == Some(&Value::Int(rate.den()));
                if let Some(range) = saved_range(row, origin).filter(|_| grid_agrees) {
                    timeline.unqualified.push(range);
                } else {
                    timeline.issue = Some("unqualified V1 transition coverage");
                }
            }
        } else {
            timeline.issue = Some("unqualified V1 transition coverage");
        }
    }
    if let Some(Value::Arr(rows)) = graph.sidecar("resolve.timing", &seq.id) {
        let qualified: HashMap<_, _> = timeline.edits.iter().filter_map(|edit| {
            match edit.content {
                EditContent::Media { placement, source: Ok(_), .. } if placement.speed.is_some() => {
                    Some((placement.id.as_str(), &edit.range))
                }
                _ => None,
            }
        }).collect();
        let unresolved: HashSet<_> = graph.losses.iter()
            .filter(|loss| loss.object == seq.id && loss.aspect == "timing")
            .filter_map(|loss| loss.reason.split_once(": ").map(|(id, _)| id))
            .collect();
        for row in rows {
            let id = match row.get("occurrence") {
                Some(Value::Str(id)) => {
                    identified.insert(("timing", id.as_str()));
                    Some(id.as_str())
                }
                _ => None,
            };
            if let Some(range) = saved_range(row, origin).filter(|_| {
                matches!(row.get("record_rate"), Some(Value::Str(saved)) if Rational::parse(saved) == Some(rate))
            }) {
                if id.is_some_and(|id| {
                    !unresolved.contains(id)
                        && qualified.get(id).is_some_and(|qualified| **qualified == range)
                        && matches!(row.get("source_rate"), Some(Value::Str(saved)) if Rational::parse(saved) == Some(rate))
                }) {
                    continue;
                }
                timeline.unqualified.push(range);
            } else {
                timeline.issue = Some("unqualified saved edit timing");
            }
        }
    }
    // Resolve stores generators and titles outside the file-backed placements.
    // An interval they occupy cannot be treated as empty V1.
    if let Some(data) = graph.sidecar(crate::generator::ADAPTER, &seq.id) {
        match data {
            Value::Arr(rows) => {
                for row in rows {
                    match row.get("video_track") {
                        Some(Value::Int(track)) if *track > 1 => continue,
                        Some(Value::Int(1)) => {
                            if let (Some(Value::Int(start)), Some(Value::Int(length))) =
                                (row.get("start_frames"), row.get("duration_frames"))
                            {
                                if let Some(end) = start.checked_add(*length)
                                    .filter(|end| *start >= 0 && *end > *start)
                                {
                                    timeline.unqualified.push(*start..end);
                                    continue;
                                }
                            }
                        }
                        _ => {}
                    }
                    timeline.issue = Some("unqualified V1 generator coverage");
                }
            }
            _ => timeline.issue = Some("unqualified V1 generator coverage"),
        }
    }
    for title in seq.texts.iter().filter(|text| text.kind == TextKind::Graphic && text.track == 1) {
        match title.record_start.zip(title.duration)
            .and_then(|(start, duration)| frame_range(start, duration, rate).ok())
        {
            Some(range) => timeline.unqualified.push(range),
            None => timeline.issue = Some("unqualified V1 title coverage"),
        }
    }
    if graph.losses.iter().any(|loss| {
        loss.object == seq.id
            && matches!(
                loss.aspect.as_str(),
                "track" | "tracks" | "placement" | "media" | "timing" | "transition"
            )
            && !loss
                .reason
                .split_once(": ")
                .is_some_and(|(id, reason)| {
                    identified.contains(&(loss.aspect.as_str(), id))
                        || (loss.aspect == "placement" && reason == crate::nests::LOSS && nest_ids.contains(id))
                })
    }) {
        timeline.issue = Some("unlocated timeline observations may affect V1");
    }
    Ok(())
}

fn saved_range(row: &Value, origin: Option<i64>) -> Option<Range<i64>> {
    let (Some(Value::Str(start)), Some(Value::Str(duration))) =
        (row.get("saved_start"), row.get("saved_duration"))
    else {
        return None;
    };
    let start = start.parse::<i64>().ok()?.checked_sub(origin?)?;
    let end = start.checked_add(duration.parse::<i64>().ok()?)?;
    (start >= 0 && end > start).then_some(start..end)
}

struct Cursor<'a, 'g> {
    timeline: &'a Timeline<'g>,
    next: usize,
    active: BTreeSet<(i64, usize)>,
    next_unknown: usize,
    unknown_until: i64,
}

impl<'a, 'g> Cursor<'a, 'g> {
    fn new(timeline: &'a Timeline<'g>) -> Self {
        Self {
            timeline,
            next: 0,
            active: BTreeSet::new(),
            next_unknown: 0,
            unknown_until: 0,
        }
    }

    fn at(
        &mut self,
        frame: i64,
        include_identity: bool,
    ) -> (std::result::Result<Option<Mapping<'g>>, &'static str>, bool) {
        let track = self.timeline;
        if let Some(reason) = track.issue {
            return (Err(reason), false);
        }
        while let Some(range) = track
            .unqualified
            .get(self.next_unknown)
            .filter(|r| r.start <= frame)
        {
            self.unknown_until = self.unknown_until.max(range.end);
            self.next_unknown += 1;
        }
        if frame < self.unknown_until {
            return (
                Err(UNQUALIFIED_V1),
                false,
            );
        }
        while let Some(edit) = track
            .edits
            .get(self.next)
            .filter(|e| e.range.start <= frame)
        {
            self.active.insert((edit.range.end, self.next));
            self.next += 1;
        }
        while self.active.first().is_some_and(|&(end, _)| end <= frame) {
            self.active.pop_first();
        }
        if self.active.len() > 1 {
            return (Err("ambiguous overlapping V1 edits"), false);
        }
        let Some(&(_, index)) = self.active.first() else {
            return if track.extent.is_some_and(|end| frame < end) {
                (Ok(None), false)
            } else {
                (Err(NO_V1_EDIT), false)
            };
        };
        let edit = &track.edits[index];
        let EditContent::Media { placement, elapsed_offset, source } = edit.content else {
            return (Ok(None), false);
        };
        let mapping = source
            .map(|source| Some(Mapping {
                edit: &placement.id,
                nest: edit.nest,
                multicam_use: edit.multicam_use,
                locator: source.locator,
                source_frame: source.frames.at(elapsed_offset + (frame - edit.range.start)),
                source_rate: source.rate,
                physical_stream: source.physical_stream,
                source_timecode: source.source_timecode.filter(|_| include_identity),
                reel: source.reel.filter(|_| include_identity),
            }));
        (mapping, true)
    }
}

fn compare<'g>(
    left: &Timeline<'g>,
    right: &Timeline<'g>,
    moves: Option<&'g Destinations>,
    all_frames: bool,
) -> Result<Report<'g>> {
    let a = left.sequence;
    let b = right.sequence;
    let mut result = Report {
        schema: "anynle.resolve-conform/1",
        reference_id: &a.id,
        candidate_id: &b.id,
        rate: a.rate.to_string(),
        reference_origin: a.start.map(|t| t.to_string()),
        candidate_origin: b.start.map(|t| t.to_string()),
        mapping_status: "inconclusive",
        mapping_reason: None,
        sampling: all_frames.then_some("all_frames"),
        move_verification: None,
        excluded_dissolve_windows: Vec::new(),
        samples: Vec::new(),
        picture: Json::Null,
    };
    if a.rate != b.rate || !matches!((a.rate.num(), a.rate.den()), (24, 1) | (24000, 1001)) {
        result.mapping_reason = Some("expected matching supported timeline rates");
        return Ok(result);
    }
    let origin = |seq: &SourceSequence| seq.start.and_then(|start| frames_on(start, seq.rate).ok());
    match (origin(a), origin(b)) {
        (Some(x), Some(y)) if x == y => {}
        (Some(_), Some(_)) => {
            result.mapping_status = "different";
            result.mapping_reason = Some("saved timeline origins differ");
            return Ok(result);
        }
        _ => {
            result.mapping_reason = Some("timeline origin is unknown or off-grid");
            return Ok(result);
        }
    }
    for range in [&left.dissolve, &right.dissolve].into_iter().flatten() {
        if !result.excluded_dissolve_windows.contains(range) {
            result.excluded_dissolve_windows.push(range.clone());
        }
    }
    result
        .excluded_dissolve_windows
        .sort_by_key(|range| range.start);
    let (different_extent, samples) = if all_frames {
        let (Some(a), Some(b)) = (left.extent, right.extent) else {
            result.mapping_reason = Some("unknown full timeline extent");
            return Ok(result);
        };
        let end = a.max(b);
        if end > MAX_FULL_SCAN_FRAMES {
            return Err(format!("full conform scan exceeds {MAX_FULL_SCAN_FRAMES} timeline frames").into());
        }
        let samples: Box<dyn Iterator<Item = (Option<i64>, i64)>> =
            Box::new((0..end).map(|frame| (None, frame)));
        (a != b, samples)
    } else {
        let end = [left, right]
            .into_iter()
            .flat_map(|timeline| {
                timeline.edits.iter().map(|edit| edit.range.end).chain(timeline.extent)
            })
            .max()
            .unwrap_or(0);
        let cuts: BTreeSet<_> = left
            .edits
            .iter()
            .chain(&right.edits)
            .flat_map(|edit| [edit.range.start, edit.range.end])
            .chain(left.coverage_boundaries.iter().chain(&right.coverage_boundaries).copied())
            .filter(|&frame| frame > 0 && frame < end)
            .collect();
        let samples: Box<dyn Iterator<Item = (Option<i64>, i64)>> =
            Box::new(cuts.into_iter().flat_map(|cut| [(Some(cut), cut - 1), (Some(cut), cut)]));
        (false, samples)
    };
    let (mut reference, mut candidate) = (Cursor::new(left), Cursor::new(right));
    for (cut, frame) in samples {
        let mut sample = Sample {
            cut,
            record_frame: frame,
            status: "excluded_dissolve",
            differences: Vec::new(),
            unresolved_identity: Vec::new(),
            expected_candidate_locator: None,
            reference: None,
            candidate: None,
            reference_reason: None,
            candidate_reason: None,
            picture_eligible: false,
        };
        if !result
            .excluded_dissolve_windows
            .iter()
            .any(|range| range.contains(&frame))
        {
            let (a, reference_edit) = reference.at(frame, moves.is_some());
            let (b, candidate_edit) = candidate.at(frame, moves.is_some());
            let reference_gap = all_frames && matches!(a, Ok(None));
            let candidate_gap = all_frames && matches!(b, Ok(None));
            sample.picture_eligible = (reference_edit || reference_gap)
                && (candidate_edit || candidate_gap);
            sample.expected_candidate_locator = moves.and_then(|moves| {
                a.as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .map(|a| moves.get(a.locator).map_or(a.locator, String::as_str))
            });
            match (&a, &b) {
                (Ok(Some(a)), Ok(Some(b))) => {
                    if sample.expected_candidate_locator.unwrap_or(a.locator) != b.locator {
                        sample.differences.push("locator");
                    }
                    if a.source_frame != b.source_frame {
                        sample.differences.push("source_frame");
                    }
                    if a.source_rate != b.source_rate {
                        sample.differences.push("source_rate");
                    }
                    if matches!((a.physical_stream, b.physical_stream), (Some(x), Some(y)) if x != y)
                    {
                        sample.differences.push("physical_stream");
                    }
                    for (field, reference, candidate) in [
                        ("source_timecode", a.source_timecode, b.source_timecode),
                        ("reel", a.reel, b.reel),
                    ] {
                        if let Some(reference) = reference {
                            match candidate {
                                Some(candidate) if candidate != reference => {
                                    sample.differences.push(field)
                                }
                                Some(_) => {}
                                None => sample.unresolved_identity.push(field),
                            }
                        }
                    }
                    sample.status = if !sample.differences.is_empty() {
                        "different"
                    } else if !sample.unresolved_identity.is_empty() {
                        "inconclusive"
                    } else {
                        "match"
                    };
                }
                _ if reference_gap && candidate_gap => sample.status = "match",
                _ if (reference_gap && candidate_edit) || (candidate_gap && reference_edit) => {
                    sample.differences.push("v1_coverage");
                    sample.status = "different";
                }
                _ => sample.status = "inconclusive",
            }
            (sample.reference, sample.reference_reason) = match a {
                Ok(Some(mapping)) => (Some(mapping), None),
                Ok(None) => (None, Some(if reference_gap { SAVED_V1_GAP } else { NO_V1_EDIT })),
                Err(reason) => (None, Some(reason)),
            };
            (sample.candidate, sample.candidate_reason) = match b {
                Ok(Some(mapping)) => (Some(mapping), None),
                Ok(None) => (None, Some(if candidate_gap { SAVED_V1_GAP } else { NO_V1_EDIT })),
                Err(reason) => (None, Some(reason)),
            };
        }
        result.samples.push(sample);
    }
    let has = |status| result.samples.iter().any(|sample| sample.status == status);
    result.mapping_status = if different_extent {
        result.mapping_reason = Some("saved timeline extents differ");
        "different"
    } else if has("different") {
        "different"
    } else if has("inconclusive") {
        "inconclusive"
    } else if has("match") {
        "match"
    } else {
        result.mapping_reason = Some("no comparable V1 samples");
        "inconclusive"
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> SourceGraph {
        SourceGraph::from_json_text(
            r#"{
                "schema":"anynle.graph/2","project":{"name":"Conform"},
                "media":[
                    {"id":"a","kind":"video","locations":["C:/A/Counter.mov"],"rate":"24","duration":"4"},
                    {"id":"b","kind":"video","locations":["C:/B/Counter.mov"],"rate":"24","duration":"4"}
                ],
                "items":[
                    {"id":"a-item","name":"A","media":[{"media":"a","source_offset":"1/24"}]},
                    {"id":"b-item","name":"B","media":[{"media":"b","source_offset":"1/12"}]}
                ],
                "bins":[],
                "sequences":[{"id":"timeline","name":"V1","rate":"24","start":"3600","tracks":[],
                    "placements":[
                        {"id":"out","item":"a-item","record_start":"0","source_in":"1/24","duration":"1","video_track":1},
                        {"id":"in","item":"b-item","record_start":"1","source_in":"1/6","duration":"1","video_track":1}
                    ]
                }]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn nested_trim_preserves_child_speed_phase_and_internal_cut() {
        let mut graph = graph();
        let child_id = "00000000-0000-0000-0000-000000000001";
        let nest_id = "00000000-0000-0000-0000-000000000002";
        let mut child = graph.sequences[0].clone();
        child.id = child_id.into();
        child.frame_size = Some((1920, 1080));
        child.start = Some(Rational::integer(0));
        child.clock = Some(super::super::clock(
            child_id, "Sm2Sequence.MediaExtents", 0, child.rate,
        ).unwrap());
        child.placements[0].speed = PictureSpeed::Half.graph(24);
        graph.sequences[0].frame_size = child.frame_size;
        graph.sequences[0].placements.clear();
        graph.sequences.push(child);
        for (adapter, object, data) in [
            (ADAPTER, "timeline", r#"{"record_length":25}"#.to_owned()),
            (ADAPTER, child_id, r#"{"kind":"registered_timeline","media_pool_clip":"pool","record_origin":0,"record_length":48}"#.to_owned()),
            (crate::nests::ADAPTER, "timeline", format!(
                r#"[{{"id":"{nest_id}","profile":"{}","kind":"timeline","track_kind":"video","track":1,
                    "record_rate":"24","child_rate":"24","child_origin_frames":0,"pool_id":"pool",
                    "child_sequence":"{child_id}","record_start_frames":0,"record_duration_frames":25,"saved_source_in_frames":1}}]"#,
                crate::nests::PROFILE,
            )),
        ] {
            graph.sidecars.push(graph::Sidecar {
                adapter: adapter.into(), object: object.into(), data: graph::json::parse(&data).unwrap(),
            });
        }
        let left = timeline(&graph, "timeline").unwrap();
        let full = compare(&left, &left, None, true).unwrap();
        assert_eq!(full.mapping_status, "match");
        let source = |index: usize| full.samples[index].reference.as_ref().unwrap();
        assert_eq!((source(0).source_frame, source(1).source_frame), (2, 3));
        assert_eq!((source(22).source_frame, source(23).source_frame), (13, 6));
        assert_eq!(source(23).nest, Some(nest_id));
        let cuts = compare(&left, &left, None, false).unwrap();
        assert_eq!(cuts.samples.iter().map(|row| row.record_frame).collect::<Vec<_>>(), [22, 23]);

        let mut gap = graph.clone();
        gap.sequences[1].placements[0].duration = Rational::new(20, 24).unwrap();
        gap.sequences[1].placements[0].speed = PictureSpeed::Half.graph(20);
        let with_gap = timeline(&gap, "timeline").unwrap();
        let full = compare(&with_gap, &with_gap, None, true).unwrap();
        assert_eq!(full.mapping_status, "match");
        assert!(full.samples[19..23].iter().all(|sample| {
            sample.reference_reason == Some(SAVED_V1_GAP) && sample.picture_eligible
        }));
        let changed = compare(&with_gap, &left, None, true).unwrap();
        assert_eq!(changed.mapping_status, "different");
        assert_eq!(
            changed.samples.iter().filter(|sample| sample.differences == ["v1_coverage"])
                .map(|sample| sample.record_frame).collect::<Vec<_>>(),
            [19, 20, 21, 22],
        );
        gap.sidecars.push(graph::Sidecar {
            adapter: crate::generator::ADAPTER.into(),
            object: child_id.into(),
            data: Value::Arr(vec![crate::generator::Solid {
                start: 21, duration: 2, rgb: [0, 0, 0],
            }.receipt("child-solid", 1)]),
        });
        let with_generator = timeline(&gap, "timeline").unwrap();
        let full = compare(&with_generator, &with_generator, None, true).unwrap();
        assert_eq!(full.samples[19].reference_reason, Some(SAVED_V1_GAP));
        assert!(full.samples[20..22].iter().all(|sample| {
            sample.reference_reason == Some(UNQUALIFIED_V1) && !sample.picture_eligible
        }));
        assert_eq!(full.samples[22].reference_reason, Some(SAVED_V1_GAP));
        let cuts = compare(&with_generator, &with_generator, None, false).unwrap();
        assert!(cuts.samples.iter().any(|sample| sample.cut == Some(20)));

        // A known interval with unqualified timing must never appear empty.
        let Value::Arr(rows) = &mut graph.sidecars[2].data else { unreachable!() };
        let Value::Obj(row) = &mut rows[0] else { unreachable!() };
        row.retain(|(name, _)| name != "profile");
        let unknown = timeline(&graph, "timeline").unwrap();
        let full = compare(&unknown, &unknown, None, true).unwrap();
        assert_eq!(full.mapping_status, "inconclusive");
        assert!(full.samples.iter().all(|s| s.reference_reason == Some(UNQUALIFIED_V1)));
    }

    #[test]
    fn checks_file_relative_frames_and_retains_differences_alongside_gaps() {
        let reference = graph();
        let left = timeline(&reference, "timeline").unwrap();
        let baseline = compare(&left, &left, None, false).unwrap();
        assert_eq!(baseline.mapping_status, "match");
        assert_eq!(
            baseline.samples[0].reference.as_ref().unwrap().source_frame,
            25
        );
        assert_eq!(
            baseline.samples[1].reference.as_ref().unwrap().source_frame,
            6
        );

        let mut candidate = reference.clone();
        candidate.items[0].media[0].source_offset = None;
        candidate.media[1].locations[0] = "C:/Replacement/Counter.mov".into();
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, None, false).unwrap();
        assert_eq!(report.mapping_status, "different");
        assert_eq!(report.samples[0].status, "inconclusive");
        assert!(report.samples[0].picture_eligible);
        assert_eq!(report.samples[1].differences, ["locator"]);
    }

    #[test]
    fn full_scan_detects_an_interior_frame_change_that_cut_sampling_misses() {
        let reference = graph();
        let mut candidate = reference.clone();
        let mut map: Vec<_> = (2..26).collect();
        map[11] = 14;
        candidate.sequences[0].placements[0].frame_map = Some(map);
        candidate.validate().unwrap();
        let left = timeline(&reference, "timeline").unwrap();
        let mut right = timeline(&candidate, "timeline").unwrap();

        let cuts = compare(&left, &right, None, false).unwrap();
        assert_eq!(cuts.mapping_status, "match");
        assert_eq!(cuts.samples[0].cut, Some(24));

        let full = compare(&left, &right, None, true).unwrap();
        assert_eq!(full.sampling, Some("all_frames"));
        assert_eq!(full.mapping_status, "different");
        assert_eq!(full.samples.len(), 48);
        let changed: Vec<_> = full.samples.iter().filter(|row| row.status == "different").collect();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].record_frame, 11);
        assert_eq!(changed[0].cut, None);
        assert_eq!(changed[0].differences, ["source_frame"]);

        right.extent = Some(49);
        let different_length = compare(&left, &right, None, true).unwrap();
        assert_eq!(different_length.mapping_reason, Some("saved timeline extents differ"));
        right.extent = Some(MAX_FULL_SCAN_FRAMES + 1);
        assert!(compare(&left, &right, None, true).is_err());
    }

    #[test]
    fn full_scan_distinguishes_saved_gaps_from_missing_or_unresolved_edits() {
        let mut reference = graph();
        reference.sequences[0].placements[1].record_start = Rational::new(5, 4).unwrap();
        let left = timeline(&reference, "timeline").unwrap();
        let same = compare(&left, &left, None, true).unwrap();
        assert_eq!(same.mapping_status, "match");
        assert!(same.samples[24..30].iter().all(|row| {
            row.status == "match" && row.picture_eligible
                && row.reference_reason == Some(SAVED_V1_GAP)
                && row.candidate_reason == Some(SAVED_V1_GAP)
        }));

        let mut candidate = reference.clone();
        candidate.sequences[0].placements[0].duration = Rational::new(5, 4).unwrap();
        candidate.items[0].media[0].source_offset = None;
        candidate.validate().unwrap();
        let right = timeline(&candidate, "timeline").unwrap();
        let full = compare(&left, &right, None, true).unwrap();
        assert_eq!(full.mapping_status, "different");
        assert!(full.samples[24..30].iter().all(|row| {
            row.differences == ["v1_coverage"] && row.picture_eligible && row.candidate.is_none()
        }));
        let cuts = compare(&left, &right, None, false).unwrap();
        assert_eq!(cuts.mapping_status, "inconclusive");
        assert_eq!(cuts.samples[1].reference_reason, Some(NO_V1_EDIT));

        let mut generated = reference.clone();
        generated.sidecars.push(graph::Sidecar {
            adapter: crate::generator::ADAPTER.into(),
            object: "timeline".into(),
            data: Value::Arr(vec![crate::generator::Solid {
                start: 24, duration: 6, rgb: [0, 0, 0],
            }.receipt("black-solid", 1)]),
        });
        let uncertain = timeline(&generated, "timeline").unwrap();
        let full = compare(&left, &uncertain, None, true).unwrap();
        assert_eq!(full.mapping_status, "inconclusive");
        assert!(full.samples[24..30].iter().all(|row| {
            row.status == "inconclusive" && !row.picture_eligible && row.differences.is_empty()
        }));
    }

    #[test]
    fn checks_odd_half_speed_reverse_and_explicit_file_frames_at_a_cut() {
        let mut reference = graph();
        let edits = &mut reference.sequences[0].placements;
        edits[0].duration = Rational::new(23, 24).unwrap();
        edits[0].speed = PictureSpeed::Half.graph(23);
        edits[1].record_start = edits[0].duration;
        edits[1].speed = PictureSpeed::Reverse.graph(24);
        reference.validate().unwrap();
        let left = timeline(&reference, "timeline").unwrap();
        let report = compare(&left, &left, None, false).unwrap();
        assert_eq!(report.mapping_status, "match");
        let source_frame = |row: &Sample<'_>| row.reference.as_ref().unwrap().source_frame;
        assert_eq!(source_frame(&report.samples[0]), 13);
        assert_eq!(source_frame(&report.samples[1]), 29);

        let mut candidate = reference.clone();
        let edit = &mut candidate.sequences[0].placements[0];
        edit.speed = None;
        edit.frame_map = Some((0..23).map(|i| 2 + i / 2).collect());
        candidate.validate().unwrap();
        let right = timeline(&candidate, "timeline").unwrap();
        assert_eq!(compare(&left, &right, None, false).unwrap().mapping_status, "match");
        candidate.sequences[0].placements[0].frame_map.as_mut().unwrap()[22] = 14;
        let right = timeline(&candidate, "timeline").unwrap();
        assert_eq!(compare(&left, &right, None, false).unwrap().samples[0].differences, ["source_frame"]);
        candidate.sequences[0].placements[0].frame_map.as_mut().unwrap()[22] = 13;
        candidate.media[0].rate = Rational::new(24000, 1001);
        candidate.validate().unwrap();
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, None, false).unwrap();
        assert_eq!(report.samples[0].candidate.as_ref().unwrap().source_frame, 13);
        assert_eq!(report.samples[0].differences, ["source_rate"]);
    }

    #[test]
    fn saved_retime_receipt_must_agree_with_the_qualified_edit() {
        let mut reference = graph();
        reference.sequences[0].placements[0].speed = PictureSpeed::Half.graph(24);
        reference.sidecars.push(graph::Sidecar {
            adapter: "resolve.timing".into(),
            object: "timeline".into(),
            data: Value::Arr(vec![Value::Obj(vec![
                ("occurrence".into(), Value::Str("out".into())),
                ("saved_start".into(), Value::Str("86400".into())),
                ("saved_duration".into(), Value::Str("24".into())),
                ("record_rate".into(), Value::Str("24".into())),
                ("source_rate".into(), Value::Str("24".into())),
            ])]),
        });
        let left = timeline(&reference, "timeline").unwrap();
        assert_eq!(compare(&left, &left, None, false).unwrap().samples[0].status, "match");

        let mut candidate = reference.clone();
        candidate.losses.push(graph::Loss {
            object: "timeline".into(),
            aspect: "timing".into(),
            reason: "out: unqualified Resolve rate or retime map".into(),
        });
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, None, false).unwrap();
        assert_eq!(report.samples[0].status, "inconclusive");
        assert_eq!(report.samples[1].status, "match");
    }

    #[test]
    fn verified_moves_do_not_mask_slips_or_missing_saved_identity() {
        let mut reference = graph();
        reference.sidecars.push(graph::Sidecar {
            adapter: metadata::ADAPTER.into(),
            object: "b-item".into(),
            data: Value::Obj(vec![(
                metadata::TIMECODE.into(),
                Value::Str("11:00:02:00".into()),
            )]),
        });
        let left = timeline(&reference, "timeline").unwrap();
        let mut candidate = reference.clone();
        let destination = "C:/Moved/B/Counter.mov";
        candidate.media[1].locations[0] = destination.into();
        let moves = Destinations::from([("C:/B/Counter.mov".into(), destination.into())]);
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, Some(&moves), false).unwrap();
        assert_eq!(report.mapping_status, "match");
        assert_eq!(
            report.samples[1].expected_candidate_locator,
            Some(destination)
        );

        candidate.sidecars.clear();
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, Some(&moves), false).unwrap();
        assert_eq!(report.mapping_status, "inconclusive");
        assert_eq!(report.samples[1].unresolved_identity, ["source_timecode"]);
        assert!(report.samples[1].picture_eligible);

        candidate.sequences[0].placements[1].source_in = Rational::new(5, 24).unwrap();
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, Some(&moves), false).unwrap();
        assert_eq!(report.mapping_status, "different");
        assert_eq!(report.samples[1].differences, ["source_frame"]);
        assert_eq!(report.samples[1].unresolved_identity, ["source_timecode"]);
    }
}
