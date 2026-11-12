//! Offline checks of saved source frames and supplied pictures at V1 cuts.
mod renders;

use super::{compare, metadata, read_drp_bytes, report, ADAPTER, OMITTED_TRACKS_ADAPTER};
use crate::{
    dissolve,
    prepare::frames_on,
    relink::{self, Destinations},
    Result,
};
use graph::{
    source::{SourceGraph, SourceMedia, SourceSequence},
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

pub use renders::Renders;

pub struct MoveOptions<'a> {
    pub moves: &'a Path,
    pub checksums: Option<&'a Path>,
}

#[derive(Default)]
pub struct Options<'a> {
    pub renders: Option<Renders<'a>>,
    pub moves: Option<MoveOptions<'a>>,
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
    let mut result = compare(&left, &right, moves);
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
    move_verification: Option<&'static str>,
    excluded_dissolve_windows: Vec<Range<i64>>,
    samples: Vec<Sample<'a>>,
    picture: Json,
}

#[derive(Serialize)]
struct Sample<'a> {
    cut: i64,
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
    locator: &'a str,
    source_frame: i64,
    physical_stream: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_timecode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reel: Option<&'a str>,
}

struct Edit<'a> {
    placement: &'a Placement,
    range: Range<i64>,
    source: std::result::Result<Source<'a>, &'static str>,
}

struct Source<'a> {
    locator: &'a str,
    first_frame: i64,
    physical_stream: Option<i64>,
    source_timecode: Option<&'a str>,
    reel: Option<&'a str>,
}

struct Timeline<'a> {
    sequence: &'a SourceSequence,
    edits: Vec<Edit<'a>>,
    extent: Option<i64>,
    unqualified: Vec<Range<i64>>,
    dissolve: Option<Range<i64>>,
    issue: Option<&'static str>,
}

fn timeline<'a>(graph: &'a SourceGraph, id: &str) -> Result<Timeline<'a>> {
    let seq = graph
        .sequences
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("conform timeline {id} was not found"))?;
    let mut result = Timeline {
        sequence: seq,
        edits: Vec::new(),
        extent: None,
        unqualified: Vec::new(),
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
    let items: HashMap<_, _> = graph
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let media: HashMap<_, _> = graph
        .media
        .iter()
        .map(|source| (source.id.as_str(), source))
        .collect();
    let saved_metadata: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|row| row.adapter == metadata::ADAPTER)
        .map(|row| (row.object.as_str(), &row.data))
        .collect();
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
                let item = items[placement.item.as_str()];
                let mut video = item.media.iter().filter_map(|link| {
                    let source = media[link.media.as_str()];
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
                        saved_metadata.get(item.id.as_str()).copied(),
                    ),
                    _ => Err("picture binding does not identify one video stream"),
                };
                result.edits.push(Edit {
                    placement,
                    range,
                    source: binding,
                });
            }
            Err(_) => result.issue = Some("a V1 edit is outside the exact timeline frame grid"),
        }
    }
    result.edits.sort_by_key(|edit| edit.range.start);
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
    edit: &Placement,
    range: &Range<i64>,
    media: &'a SourceMedia,
    offset: Option<Rational>,
    rate: Rational,
    saved_metadata: Option<&'a Value>,
) -> std::result::Result<Source<'a>, &'static str> {
    if edit.speed.is_some() || edit.frame_map.is_some() || media.rate != Some(rate) {
        return Err("unsupported picture timing or source rate");
    }
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
    let offset = offset.ok_or("unknown item-to-file source offset")?;
    let source_time = edit
        .source_in
        .checked_add(offset)
        .ok_or("source offset overflow")?;
    let first_frame = frames_on(source_time, rate)
        .map_err(|_| "source time is outside the exact picture frame grid")?;
    let count = frames_on(media.duration, rate).map_err(|_| "unknown whole-frame source extent")?;
    if first_frame
        .checked_add(range.end - range.start)
        .is_none_or(|end| end > count)
    {
        return Err("picture edit exceeds the saved source extent");
    }
    let saved = |field| match saved_metadata.and_then(|data| data.get(field)) {
        Some(Value::Str(value)) if !value.is_empty() => Some(value.as_str()),
        _ => None,
    };
    Ok(Source {
        locator: path,
        first_frame,
        physical_stream: media.original.as_ref().map(|file| file.stream_index),
        source_timecode: saved(metadata::TIMECODE),
        reel: saved(metadata::REEL),
    })
}

fn track_unknowns(graph: &SourceGraph, timeline: &mut Timeline<'_>) -> Result<()> {
    let seq = timeline.sequence;
    let rate = seq.rate;
    let mut identified = HashSet::new();
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
        for row in rows {
            if let Some(Value::Str(id)) = row.get("occurrence") {
                identified.insert(("timing", id.as_str()));
            }
            if let Some(range) = saved_range(row, origin).filter(|_| {
                matches!(row.get("record_rate"), Some(Value::Str(saved)) if Rational::parse(saved) == Some(rate))
            }) {
                timeline.unqualified.push(range);
            } else {
                timeline.issue = Some("unqualified saved edit timing");
            }
        }
    }
    for group in graph
        .multicam_uses
        .iter()
        .filter(|u| u.sequence == seq.id && u.kind == StreamKind::Video && u.track == 1)
    {
        match group
            .record_start
            .zip(group.duration)
            .and_then(|(start, duration)| frame_range(start, duration, rate).ok())
        {
            Some(range) => timeline.unqualified.push(range),
            None => timeline.issue = Some("unqualified multicam timing"),
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
                .is_some_and(|(id, _)| identified.contains(&(loss.aspect.as_str(), id)))
    }) {
        timeline.issue = Some("unlocated timeline observations may affect V1");
    }
    if timeline.unqualified.len() > MAX_EDITS {
        return Err("unqualified interval count exceeds conform report limit".into());
    }
    timeline.unqualified.sort_by_key(|range| range.start);
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
    ) -> (std::result::Result<Mapping<'g>, &'static str>, bool) {
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
                Err("unqualified transition, edit timing, or multicam covers this frame"),
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
            return (Err("no saved V1 edit covers this frame"), false);
        };
        let edit = &track.edits[index];
        let mapping = edit
            .source
            .as_ref()
            .map(|source| Mapping {
                edit: &edit.placement.id,
                locator: source.locator,
                source_frame: source.first_frame + (frame - edit.range.start),
                physical_stream: source.physical_stream,
                source_timecode: source.source_timecode.filter(|_| include_identity),
                reel: source.reel.filter(|_| include_identity),
            })
            .map_err(|reason| *reason);
        (mapping, true)
    }
}

fn compare<'g>(
    left: &Timeline<'g>,
    right: &Timeline<'g>,
    moves: Option<&'g Destinations>,
) -> Report<'g> {
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
        move_verification: None,
        excluded_dissolve_windows: Vec::new(),
        samples: Vec::new(),
        picture: Json::Null,
    };
    if a.rate != b.rate || !matches!((a.rate.num(), a.rate.den()), (24, 1) | (24000, 1001)) {
        result.mapping_reason = Some("expected matching supported timeline rates");
        return result;
    }
    let origin = |seq: &SourceSequence| seq.start.and_then(|start| frames_on(start, seq.rate).ok());
    match (origin(a), origin(b)) {
        (Some(x), Some(y)) if x == y => {}
        (Some(_), Some(_)) => {
            result.mapping_status = "different";
            result.mapping_reason = Some("saved timeline origins differ");
            return result;
        }
        _ => {
            result.mapping_reason = Some("timeline origin is unknown or off-grid");
            return result;
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
    let end = [left, right]
        .into_iter()
        .flat_map(|timeline| {
            timeline
                .edits
                .iter()
                .map(|edit| edit.range.end)
                .chain(timeline.extent)
        })
        .max()
        .unwrap_or(0);
    let cuts: BTreeSet<_> = left
        .edits
        .iter()
        .chain(&right.edits)
        .flat_map(|edit| [edit.range.start, edit.range.end])
        .filter(|&frame| frame > 0 && frame < end)
        .collect();
    let (mut reference, mut candidate) = (Cursor::new(left), Cursor::new(right));
    for cut in cuts {
        for frame in [cut - 1, cut] {
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
                let (a, reference_picture) = reference.at(frame, moves.is_some());
                let (b, candidate_picture) = candidate.at(frame, moves.is_some());
                sample.picture_eligible = reference_picture && candidate_picture;
                sample.expected_candidate_locator = moves.and_then(|moves| {
                    a.as_ref()
                        .ok()
                        .map(|a| moves.get(a.locator).map_or(a.locator, String::as_str))
                });
                match (&a, &b) {
                    (Ok(a), Ok(b)) => {
                        if sample.expected_candidate_locator.unwrap_or(a.locator) != b.locator {
                            sample.differences.push("locator");
                        }
                        if a.source_frame != b.source_frame {
                            sample.differences.push("source_frame");
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
                    _ => sample.status = "inconclusive",
                }
                (sample.reference, sample.reference_reason) = match a {
                    Ok(mapping) => (Some(mapping), None),
                    Err(reason) => (None, Some(reason)),
                };
                (sample.candidate, sample.candidate_reason) = match b {
                    Ok(mapping) => (Some(mapping), None),
                    Err(reason) => (None, Some(reason)),
                };
            }
            result.samples.push(sample);
        }
    }
    let has = |status| result.samples.iter().any(|sample| sample.status == status);
    result.mapping_status = if has("different") {
        "different"
    } else if has("inconclusive") {
        "inconclusive"
    } else if has("match") {
        "match"
    } else {
        result.mapping_reason = Some("no comparable V1 samples");
        "inconclusive"
    };
    result
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
    fn checks_file_relative_frames_and_retains_differences_alongside_gaps() {
        let reference = graph();
        let left = timeline(&reference, "timeline").unwrap();
        let baseline = compare(&left, &left, None);
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
        let report = compare(&left, &right, None);
        assert_eq!(report.mapping_status, "different");
        assert_eq!(report.samples[0].status, "inconclusive");
        assert!(report.samples[0].picture_eligible);
        assert_eq!(report.samples[1].differences, ["locator"]);
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
        let report = compare(&left, &right, Some(&moves));
        assert_eq!(report.mapping_status, "match");
        assert_eq!(
            report.samples[1].expected_candidate_locator,
            Some(destination)
        );

        candidate.sidecars.clear();
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, Some(&moves));
        assert_eq!(report.mapping_status, "inconclusive");
        assert_eq!(report.samples[1].unresolved_identity, ["source_timecode"]);
        assert!(report.samples[1].picture_eligible);

        candidate.sequences[0].placements[1].source_in = Rational::new(5, 24).unwrap();
        let right = timeline(&candidate, "timeline").unwrap();
        let report = compare(&left, &right, Some(&moves));
        assert_eq!(report.mapping_status, "different");
        assert_eq!(report.samples[1].differences, ["source_frame"]);
        assert_eq!(report.samples[1].unresolved_identity, ["source_timecode"]);
    }
}
