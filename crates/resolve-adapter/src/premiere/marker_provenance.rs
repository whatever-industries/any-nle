//! Read the edit-marker ownership record written on a dedicated Premiere item.
use super::markers::{SavedMarkerColor, SOURCE_MARKER_COLORS_ADAPTER};
use crate::Result;
use graph::{
    source::{SourceBinding, SourceGraph, SourceItem, SourceMarker},
    Label, MarkerOwner, Placement, Rational, Sidecar, StreamKind, TrackRef, Value,
};
use prproj::{
    convert::markers::{EDIT_MARKER_PROVENANCE, MAX_MARKER_CUSTOM_DATA},
    model::{marker::marker_guid, ItemId, Project},
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

pub(crate) const ADAPTER: &str = "premiere.edit_marker_provenance";
const OUTSIDE_TRIM: &str = "outside_current_trim";
pub(crate) const CROSSES_SPLIT: &str = "crosses_split";
pub(crate) const CROSSES_SPLIT_REASON: &str = "Premiere edit marker range crosses the split";

pub(super) struct Record {
    pub original: ItemId,
    pub rate: Rational,
    sequence_index: usize,
    pub receipts: Vec<EditReceipt>,
}

pub(super) struct EditReceipt {
    placement_index: usize,
    pub sidecar: Sidecar,
}

struct EditWindow {
    placement_index: usize,
    source: Range<Rational>,
    record: Range<i64>,
}

pub(super) fn read(project: &Project, graph: &SourceGraph) -> Result<Vec<Record>> {
    let tree = &project.doc.tree;
    let mut records = Vec::new();
    let mut items = HashMap::new();
    let mut placements_by_item: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for (sequence_index, sequence) in graph.sequences.iter().enumerate() {
        for (placement_index, edit) in sequence.placements.iter().enumerate() {
            placements_by_item
                .entry(&edit.item)
                .or_default()
                .push((sequence_index, placement_index));
        }
    }
    for item in project.walk_items() {
        let data = project.item(item);
        if let Some(uid) = tree
            .get(data.element, "ObjectUID")
            .filter(|_| data.master.is_some())
        {
            items
                .entry(uid)
                .and_modify(|id| *id = None)
                .or_insert(Some(item));
        }
    }
    for item in &graph.items {
        let Some(uid) = item.id.strip_prefix("premiere:item:") else {
            continue;
        };
        let Some(copy) = items.get(uid) else {
            continue;
        };
        let copy =
            copy.ok_or_else(|| format!("{}: ambiguous Premiere project item UID", item.id))?;
        let Some(properties) =
            tree.find1(project.item(copy).element, "ProjectItem/Node/Properties")
        else {
            continue;
        };
        let mut saved = tree.iter_tag(properties, EDIT_MARKER_PROVENANCE);
        let Some(node) = saved.next() else { continue };
        let invalid = |reason| format!("{}: Premiere edit marker provenance {reason}", item.id);
        if saved.next().is_some() {
            return Err(invalid("is duplicated").into());
        }
        let body = tree
            .text(node)
            .filter(|text| text.len() <= 8192)
            .ok_or_else(|| invalid("is empty or exceeds its size limit"))?;
        let fields: Vec<_> = body.split_ascii_whitespace().collect();
        let [version @ ("1" | "2" | "3"), parent, sequence, start, source_in, duration, guids] =
            fields.as_slice()
        else {
            return Err(invalid("has an invalid format").into());
        };
        if parent == &uid || !valid_uuid(parent) || !valid_uuid(sequence) {
            return Err(invalid("has an invalid item or sequence identity").into());
        }
        let uses = placements_by_item
            .get(item.id.as_str())
            .ok_or_else(|| invalid("has no enabled edit"))?;
        let sequence_index = uses[0].0;
        let timeline = &graph.sequences[sequence_index];
        let invalid_trim = || {
            invalid(if uses.len() == 1 {
                "does not identify one matching V1 edit and trim"
            } else {
                "requires contiguous V1 pieces of the saved trim in source and record order"
            })
        };
        if timeline.id != *sequence
            || !crate::source::supported_marker_rate(timeline.rate)
            || timeline.frame_size != Some((1920, 1080))
            || item.rate != Some(timeline.rate)
        {
            return Err(invalid_trim().into());
        }
        let saved_in = parse_ticks(source_in).ok_or_else(invalid_trim)?;
        let saved_duration = parse_ticks(duration).ok_or_else(invalid_trim)?;
        let saved_window =
            frame_range(saved_in, saved_duration, timeline.rate).ok_or_else(invalid_trim)?;
        let saved_record = parse_ticks(start)
            .and_then(|time| record_range(time, saved_duration, timeline.rate))
            .ok_or_else(invalid_trim)?;
        let mut windows: Vec<_> = uses
            .iter()
            .map(|&(owner, placement_index)| {
                if owner != sequence_index {
                    return None;
                }
                let edit = &timeline.placements[placement_index];
                if edit.video_track != Some(1)
                    || !edit.audio_tracks.is_empty()
                    || edit.speed.is_some()
                    || edit.frame_map.is_some()
                {
                    return None;
                }
                Some(EditWindow {
                    placement_index,
                    source: frame_range(edit.source_in, edit.duration, timeline.rate)?,
                    record: record_range(edit.record_start, edit.duration, timeline.rate)?,
                })
            })
            .collect::<Option<_>>()
            .ok_or_else(invalid_trim)?;
        windows.sort_by_key(|window| window.record.start);
        let current_window = covered_source(&windows)
            .filter(|current| contains_range(&saved_window, current))
            .ok_or_else(invalid_trim)?;
        if (windows.len() > 1 || current_window != saved_window)
            && !item.media.iter().any(|link| {
                link.source_offset == Some(Rational::ZERO)
                    && graph.media.iter().any(|media| {
                        media.id == link.media
                            && media.kind == StreamKind::Video
                            && media.duration >= saved_window.end
                    })
            })
        {
            return Err(invalid_trim().into());
        }
        let parent_item = match items.get(parent) {
            Some(Some(item)) => *item,
            Some(None) => return Err(invalid("has an ambiguous original item").into()),
            None => return Err(invalid("cannot find the original item").into()),
        };
        let parent_media = project
            .media_element(parent_item)?
            .ok_or_else(|| invalid("original has no linked media"))?;
        let copy_media = project
            .media_element(copy)?
            .ok_or_else(|| invalid("copy has no linked media"))?;
        for field in ["FilePath", "ActualMediaFilePath"] {
            let original = tree.findtext(parent_media, field)?;
            if (field == "FilePath" && original.as_deref().is_none_or(str::is_empty))
                || original != tree.findtext(copy_media, field)?
            {
                return Err(
                    invalid("original and copy have different saved media locators").into(),
                );
            }
        }
        let mut attached = HashMap::new();
        for (index, &marker) in project.item_markers(copy).iter().enumerate() {
            let id = marker_guid(tree, marker)?;
            if attached.insert(id, index + 1).is_some() {
                return Err(invalid("contains duplicate attached marker IDs").into());
            }
        }
        let mut occurrences = vec![Vec::new(); windows.len()];
        let mut seen = HashSet::new();
        let mut budget = MAX_MARKER_CUSTOM_DATA;
        for entry in guids.split(',') {
            let (guid, encoded, color) = parse_entry(version, entry).map_err(invalid)?;
            let custom = encoded
                .map(|value| decode_custom_data(value, &mut budget).map_err(invalid))
                .transpose()?;
            if !valid_uuid(guid) || seen.len() == 128 || !seen.insert(guid) {
                return Err(invalid("has invalid, duplicate or too many marker IDs").into());
            }
            let ordinal = attached
                .get(guid)
                .ok_or_else(|| invalid("references a missing marker"))?;
            let occurrence = format!("{}/marker/{ordinal}", item.id);
            let marker = item
                .markers
                .iter()
                .find(|marker| marker.owner_occurrence == occurrence)
                .ok_or_else(|| invalid("references an unsupported attached marker"))?;
            let source = marker
                .start
                .ok_or_else(|| invalid("marker has no exact source time"))?;
            let span = source
                ..source
                    .checked_add(marker.duration)
                    .ok_or("edit marker extent overflow")?;
            if !contains_range(&saved_window, &span) {
                return Err(invalid("marker falls outside the saved edit").into());
            }
            let mut fields = vec![
                ("marker_guid".into(), Value::Str(guid.into())),
                ("source_marker".into(), Value::Str(occurrence)),
            ];
            if let Some(custom) = custom {
                fields.push(("custom_data".into(), Value::Str(custom)));
            }
            if let Some((original, preview)) = color {
                let color = SavedMarkerColor::read(marker, original, preview).map_err(invalid)?;
                fields.extend(color.receipt());
            }
            // A range spanning the split remains one unresolved occurrence.
            // Its prospective owner is the piece containing its start.
            let owner = if source < current_window.start {
                0
            } else {
                windows
                    .partition_point(|window| window.source.end <= source)
                    .min(windows.len() - 1)
            };
            if !contains_range(&current_window, &span) {
                fields.push((OUTSIDE_TRIM.into(), Value::Bool(true)));
            } else if !contains_range(&windows[owner].source, &span) {
                fields.push((CROSSES_SPLIT.into(), Value::Bool(true)));
            }
            occurrences[owner].push(Value::Obj(fields));
        }
        let receipts = windows
            .into_iter()
            .zip(occurrences)
            .map(|(window, markers)| {
                let mut fields = vec![
                    ("sequence".into(), Value::Str(timeline.id.clone())),
                    (
                        "original_item".into(),
                        Value::Str(format!("premiere:item:{parent}")),
                    ),
                    ("dedicated_item".into(), Value::Str(item.id.clone())),
                    ("markers".into(), Value::Arr(markers)),
                ];
                if saved_record.start != window.record.start {
                    fields.extend([
                        (
                            "record_start_at_export_ticks".into(),
                            Value::Int(saved_record.start),
                        ),
                        (
                            "record_start_at_capture_ticks".into(),
                            Value::Int(window.record.start),
                        ),
                    ]);
                }
                if window.source != saved_window {
                    for (field, range) in [
                        ("source_range_at_export", &saved_window),
                        ("source_range_at_capture", &window.source),
                    ] {
                        fields.push((
                            field.into(),
                            Value::Arr(vec![
                                Value::Str(range.start.to_string()),
                                Value::Str(range.end.to_string()),
                            ]),
                        ));
                    }
                }
                EditReceipt {
                    placement_index: window.placement_index,
                    sidecar: Sidecar {
                        adapter: ADAPTER.into(),
                        object: timeline.placements[window.placement_index].id.clone(),
                        data: Value::Obj(fields),
                    },
                }
            })
            .collect();
        records.push(Record {
            original: parent_item,
            rate: timeline.rate,
            sequence_index,
            receipts,
        });
    }
    Ok(records)
}

fn parse_ticks(value: &str) -> Option<Rational> {
    Rational::from_units(value.parse().ok()?, super::TICKS_PER_SECOND)
}

fn record_range(start: Rational, duration: Rational, rate: Rational) -> Option<Range<i64>> {
    let range = frame_range(start, duration, rate)?;
    Some(
        range.start.to_units(super::TICKS_PER_SECOND)?
            ..range.end.to_units(super::TICKS_PER_SECOND)?,
    )
}

fn covered_source(windows: &[EditWindow]) -> Option<Range<Rational>> {
    let adjacent = windows.windows(2).all(|pair| {
        pair[0].record.end == pair[1].record.start && pair[0].source.end == pair[1].source.start
    });
    adjacent.then_some(windows.first()?.source.start..windows.last()?.source.end)
}

fn frame_range(start: Rational, duration: Rational, rate: Rational) -> Option<Range<Rational>> {
    let end = start.checked_add(duration)?;
    crate::prepare::frames_on(start, rate).ok()?;
    crate::prepare::frames_on(end, rate).ok()?;
    (start < end).then_some(start..end)
}

fn contains_range(window: &Range<Rational>, span: &Range<Rational>) -> bool {
    window.contains(&span.start) && span.end <= window.end
}

type SavedEntry<'a> = (&'a str, Option<&'a str>, Option<(&'a str, &'a str)>);

fn parse_entry<'a>(
    version: &str,
    entry: &'a str,
) -> std::result::Result<SavedEntry<'a>, &'static str> {
    if version == "3" {
        let pieces: Vec<_> = entry.split(':').collect();
        return match pieces.as_slice() {
            [guid] => Ok((guid, None, None)),
            [guid, encoded] => Ok((guid, Some(encoded), None)),
            [guid, encoded, original, preview] => Ok((
                guid,
                (!encoded.is_empty()).then_some(*encoded),
                Some((original, preview)),
            )),
            _ => Err("has an invalid marker entry"),
        };
    }
    match (version, entry.split_once(':')) {
        ("2", Some((guid, encoded))) => Ok((guid, Some(encoded), None)),
        _ => Ok((entry, None, None)),
    }
}

pub(super) fn decode_custom_data(
    encoded: &str,
    budget: &mut usize,
) -> std::result::Result<String, &'static str> {
    if encoded.is_empty() || encoded.len() > *budget * 2 {
        return Err("has empty or over-limit custom data");
    }
    let decoded = hex::decode(encoded).map_err(|_| "has invalid custom-data hex")?;
    *budget -= decoded.len();
    String::from_utf8(decoded).map_err(|_| "has invalid custom-data UTF-8")
}

pub(super) fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|uuid| uuid.hyphenated().to_string() == value)
}

pub(super) fn is_dedicated_item(project: &Project, item: ItemId) -> Result<bool> {
    let tree = &project.doc.tree;
    let element = project.item(item).element;
    Ok(tree
        .findall(element, "ProjectItem/Node/Properties")?
        .into_iter()
        .any(|bag| tree.iter_tag(bag, EDIT_MARKER_PROVENANCE).next().is_some()))
}

pub(super) fn matches_original(
    graph: &SourceGraph,
    original: &SourceGraph,
    record: &Record,
) -> Result<bool> {
    Ok(restoration(graph, original, record)?.is_some())
}

/// Collapse a dedicated item only when the verified original and every source
/// marker on the copy agree. Keep the copy and its review loss otherwise.
pub(super) fn restore(graph: &mut SourceGraph, records: Vec<Record>) -> Result<HashSet<String>> {
    let copies: HashSet<_> = records
        .iter()
        .filter_map(
            |record| match record.receipts.first()?.sidecar.data.get("dedicated_item") {
                Some(Value::Str(id)) => Some(id.clone()),
                _ => None,
            },
        )
        .collect();
    let mut restored = HashSet::new();
    let mut removed_media = HashSet::new();
    for record in records {
        let Some(found) =
            restoration(graph, graph, &record)?.filter(|found| !copies.contains(&found.original))
        else {
            for EditReceipt { sidecar, .. } in record.receipts {
                graph.losses.push(graph::Loss {
                    object: sidecar.object.clone(),
                    aspect: "premiere.marker".into(),
                    reason: "Edit-marker restoration requires a verified original and matching copied source markers and metadata".into(),
                });
                graph.sidecars.push(sidecar);
            }
            continue;
        };
        let Restoration {
            copy,
            original,
            markers,
        } = found;
        let mut saved = graph.sidecar("premiere.clip_markers", &copy).cloned();
        let timeline = &mut graph.sequences[record.sequence_index];
        for (receipt, markers) in record.receipts.into_iter().zip(markers) {
            let mut sidecar = receipt.sidecar;
            timeline.placements[receipt.placement_index]
                .item
                .clone_from(&original);
            timeline.markers.extend(markers);
            if let Value::Obj(fields) = &mut sidecar.data {
                if let Some(saved) = saved.take() {
                    fields.push(("dedicated_clip_markers".into(), saved));
                }
                fields.push(("restored".into(), Value::Bool(true)));
            }
            graph.sidecars.push(sidecar);
        }
        removed_media.extend(
            graph
                .items
                .iter()
                .filter(|item| item.id == copy)
                .flat_map(|item| item.media.iter().map(|link| link.media.clone())),
        );
        restored.insert(copy);
    }
    graph.items.retain(|item| !restored.contains(&item.id));
    for bin in &mut graph.bins {
        bin.items.retain(|item| !restored.contains(item));
    }
    graph.media.retain(|media| {
        !removed_media.contains(&media.id)
            || graph
                .items
                .iter()
                .any(|item| item.media.iter().any(|link| link.media == media.id))
    });
    let retained_media: HashSet<_> = graph.media.iter().map(|media| media.id.as_str()).collect();
    graph.sidecars.retain(|row| {
        !restored.contains(&row.object)
            && (!removed_media.contains(&row.object)
                || retained_media.contains(row.object.as_str()))
    });
    Ok(restored)
}

struct Restoration {
    copy: String,
    original: String,
    markers: Vec<Vec<SourceMarker>>,
}

fn restoration(
    graph: &SourceGraph,
    original_graph: &SourceGraph,
    record: &Record,
) -> Result<Option<Restoration>> {
    let first = record
        .receipts
        .first()
        .ok_or("missing Premiere marker provenance")?;
    let (Some(Value::Str(copy_id)), Some(Value::Str(original_id))) = (
        first.sidecar.data.get("dedicated_item"),
        first.sidecar.data.get("original_item"),
    ) else {
        return Err("invalid verified Premiere marker provenance".into());
    };
    let copy = graph.items.iter().find(|item| item.id == *copy_id);
    let original = original_graph
        .items
        .iter()
        .find(|item| item.id == *original_id);
    let (Some(copy), Some(original)) = (copy, original) else {
        return Ok(None);
    };
    if graph
        .sequences
        .iter()
        .flat_map(|s| &s.disabled_placements)
        .any(|p| p.item == *copy_id)
        || !same_picture(graph, copy, original_graph, original)
        || graph.losses.iter().any(|loss| {
            loss.object == *copy_id || copy.media.iter().any(|link| loss.object == link.media)
        })
        || graph
            .sidecars
            .iter()
            .filter(|row| row.object == *copy_id && row.adapter != "premiere.clip_markers")
            .any(|row| original_graph.sidecar(&row.adapter, original_id) != Some(&row.data))
    {
        return Ok(None);
    }
    let mut marked = HashSet::new();
    let mut markers = Vec::with_capacity(record.receipts.len());
    let timeline = graph
        .sequences
        .get(record.sequence_index)
        .ok_or("missing verified Premiere marker sequence")?;
    for receipt in &record.receipts {
        let Some(Value::Arr(rows)) = receipt.sidecar.data.get("markers") else {
            return Err("missing verified Premiere marker rows".into());
        };
        let edit = timeline
            .placements
            .get(receipt.placement_index)
            .filter(|edit| edit.id == receipt.sidecar.object && edit.item == *copy_id)
            .ok_or("missing verified Premiere marker placement")?;
        markers.push(restore_markers(copy, edit, rows, &mut marked)?);
    }
    if !copied_source_markers_match(original_graph, original, copy, &marked) {
        return Ok(None);
    }
    Ok(Some(Restoration {
        copy: copy_id.clone(),
        original: original_id.clone(),
        markers,
    }))
}

fn restore_markers<'a>(
    copy: &SourceItem,
    edit: &Placement,
    rows: &'a [Value],
    marked: &mut HashSet<&'a str>,
) -> Result<Vec<SourceMarker>> {
    let mut markers = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(Value::Str(occurrence)) = row.get("source_marker") else {
            return Err("missing verified Premiere source marker occurrence".into());
        };
        let marker = copy
            .markers
            .iter()
            .find(|m| m.owner_occurrence == *occurrence)
            .ok_or("verified Premiere source marker is absent")?;
        let source = marker
            .start
            .ok_or("verified Premiere marker has no source time")?;
        let mut restored = marker.clone();
        if let (Some(Value::Bool(true)), Some(Value::Str(name))) =
            (row.get("restored"), row.get("resolve_color"))
        {
            restored.color = Some(Label {
                palette: "resolve".into(),
                name: name.clone(),
            });
        }
        restored.owner = MarkerOwner::Edit;
        restored.source = Some(source);
        restored.track = Some(TrackRef {
            kind: StreamKind::Video,
            index: 1,
        });
        restored.binding = SourceBinding {
            placements: vec![edit.id.clone()],
            reason: None,
        };
        let unavailable = if row.get(OUTSIDE_TRIM) == Some(&Value::Bool(true)) {
            Some("Premiere trim leaves the edit marker outside its visible range")
        } else if row.get(CROSSES_SPLIT) == Some(&Value::Bool(true)) {
            Some(CROSSES_SPLIT_REASON)
        } else {
            None
        };
        if let Some(reason) = unavailable {
            restored.start = None;
            restored.owner_local = None;
            restored.binding.reason = Some(reason.into());
        } else {
            let local = source
                .checked_sub(edit.source_in)
                .ok_or("edit marker position overflow")?;
            restored.start = Some(
                edit.record_start
                    .checked_add(local)
                    .ok_or("edit marker position overflow")?,
            );
            restored.owner_local = Some(local);
        }
        markers.push(restored);
        marked.insert(occurrence.as_str());
    }
    Ok(markers)
}

fn copied_source_markers_match(
    original_graph: &SourceGraph,
    original: &SourceItem,
    copy: &SourceItem,
    edit_markers: &HashSet<&str>,
) -> bool {
    // The original's graph markers already have restored Resolve colors.
    // A dedicated copy carries the Premiere colors saved in the project.
    // Compare on that saved palette while leaving the original graph intact.
    let rows = match original_graph.sidecar(SOURCE_MARKER_COLORS_ADAPTER, &original.id) {
        Some(Value::Arr(rows)) => rows.as_slice(),
        _ => &[],
    };
    let mut remaining = original.markers.clone();
    for marker in &mut remaining {
        let Some(color) = marker
            .color
            .as_mut()
            .filter(|color| color.palette == "resolve")
        else {
            continue;
        };
        let observed = rows.iter().find_map(|row| {
            let (
                Some(Value::Str(occurrence)),
                Some(Value::Str(original_color)),
                Some(Value::Str(saved)),
            ) = (
                row.get("source_marker"),
                row.get("resolve_color"),
                row.get("premiere_observed"),
            )
            else {
                return None;
            };
            (occurrence == &marker.owner_occurrence
                && original_color == &color.name
                && row.get("restored") == Some(&Value::Bool(true))
                && row.get("premiere_fallback") == row.get("premiere_observed"))
            .then_some(saved)
        });
        if let Some(saved) = observed {
            color.palette = "premiere".into();
            color.name.clone_from(saved);
        }
    }
    for copied in copy
        .markers
        .iter()
        .filter(|m| !edit_markers.contains(m.owner_occurrence.as_str()))
    {
        let Some(index) = remaining.iter_mut().position(|original| {
            original
                .owner_occurrence
                .clone_from(&copied.owner_occurrence);
            *original == *copied
        }) else {
            return false;
        };
        remaining.swap_remove(index);
    }
    remaining.is_empty()
}

fn same_picture(
    graph: &SourceGraph,
    copy: &SourceItem,
    original_graph: &SourceGraph,
    original: &SourceItem,
) -> bool {
    let ([left], [right]) = (copy.media.as_slice(), original.media.as_slice()) else {
        return false;
    };
    if left.gain_db != right.gain_db
        || left.source_offset != Some(Rational::ZERO)
        || right.source_offset != Some(Rational::ZERO)
        || copy.tape != original.tape
        || copy.rate != original.rate
        || copy.umid != original.umid
        || copy.color_rgb16 != original.color_rgb16
        || copy.stream_groups != original.stream_groups
    {
        return false;
    }
    let left = graph.media.iter().find(|media| media.id == left.media);
    let right = original_graph
        .media
        .iter()
        .find(|media| media.id == right.media);
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    let mut candidate = left.clone();
    candidate.id.clone_from(&right.id);
    if let Some(clock) = &mut candidate.clock {
        if clock.provenance.source_package == copy.id {
            clock.provenance.source_package.clone_from(&original.id);
        }
    }
    left.kind == StreamKind::Video && candidate == *right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::premiere::source_capture::GraphCapture;

    #[test]
    fn edit_marker_record_ranges_require_the_exact_frame_grid() {
        let rate = Rational::new(24000, 1001).unwrap();
        let duration = Rational::new(1001, 1000).unwrap();
        assert_eq!(
            record_range(duration, duration, rate),
            Some(254_270_016_000..508_540_032_000),
        );
        assert!(record_range(parse_ticks("1").unwrap(), duration, rate).is_none());
        assert!(record_range(parse_ticks("-10594584000").unwrap(), duration, rate).is_none());
        assert!(record_range(duration, parse_ticks("1").unwrap(), rate).is_none());
    }

    #[test]
    fn edit_marker_trim_requires_a_contained_frame_range_and_excludes_the_out_point() {
        let rate = Rational::new(24000, 1001).unwrap();
        let at = |frame: i128| Rational::new(frame * 1001, 24000).unwrap();
        let original = frame_range(at(5), at(24), rate).unwrap();
        let trimmed = frame_range(at(9), at(15), rate).unwrap();
        assert!(contains_range(&original, &trimmed));
        assert!(contains_range(&trimmed, &(at(9)..at(9))));
        assert!(contains_range(&trimmed, &(at(23)..at(24))));
        assert!(!contains_range(&trimmed, &(at(24)..at(24))));
        assert!(!contains_range(&trimmed, &(at(23)..at(25))));
        assert!(!contains_range(&original, &(at(6)..at(30))));
        assert!(frame_range(at(9), Rational::ZERO, rate).is_none());
        assert!(frame_range(Rational::new(1, 254_016_000_000).unwrap(), at(15), rate).is_none());
    }

    #[test]
    fn edit_marker_chain_checks_every_source_and_record_join() {
        let mut windows: Vec<_> = (0..512)
            .map(|frame| EditWindow {
                placement_index: frame as usize,
                source: Rational::integer(frame)..Rational::integer(frame + 1),
                record: frame..frame + 1,
            })
            .collect();
        assert_eq!(
            covered_source(&windows),
            Some(Rational::ZERO..Rational::integer(512))
        );
        windows[511].source = Rational::integer(512)..Rational::integer(513);
        assert!(covered_source(&windows).is_none());
        windows[511].source = Rational::integer(511)..Rational::integer(512);
        windows[511].record = 510..511;
        assert!(covered_source(&windows).is_none());
    }

    #[test]
    fn edit_marker_color_entries_keep_custom_data_optional_and_older_records_readable() {
        assert_eq!(parse_entry("1", "guid"), Ok(("guid", None, None)));
        assert_eq!(parse_entry("2", "guid:41"), Ok(("guid", Some("41"), None)));
        assert_eq!(parse_entry("3", "guid:41"), Ok(("guid", Some("41"), None)));
        assert_eq!(
            parse_entry("3", "guid::128:0"),
            Ok(("guid", None, Some(("128", "0"))))
        );
        assert_eq!(
            parse_entry("3", "guid:41:128:0"),
            Ok(("guid", Some("41"), Some(("128", "0"))))
        );
        assert!(parse_entry("3", "guid:41:128").is_err());
        assert!(parse_entry("3", "guid:41:128:0:extra").is_err());
    }

    #[test]
    fn dedicated_source_markers_match_the_saved_palette_on_the_original() {
        let original = SourceItem {
            id: "original".into(),
            name: "Picture".into(),
            media: vec![],
            umid: None,
            tape: None,
            rate: Some(Rational::integer(24)),
            color_rgb16: None,
            stream_groups: vec![],
            markers: vec![SourceMarker {
                name: "Note".into(),
                comment: "Source note".into(),
                user: String::new(),
                color: Some(Label {
                    palette: "resolve".into(),
                    name: "Mint".into(),
                }),
                start: Some(Rational::ZERO),
                duration: Rational::new(1, 24).unwrap(),
                track: None,
                owner: MarkerOwner::Sequence,
                owner_occurrence: "original/marker/2".into(),
                owner_local: Some(Rational::ZERO),
                owner_rate: Some(Rational::integer(24)),
                binding: SourceBinding {
                    placements: vec![],
                    reason: None,
                },
                source: None,
                inherited: false,
            }],
        };
        let mut copy = original.clone();
        copy.markers[0].owner_occurrence = "copy/marker/1".into();
        copy.markers[0].color = Some(Label {
            palette: "premiere".into(),
            name: "Green".into(),
        });
        let mut graph = GraphCapture::empty_graph("Test".into());
        let edits = HashSet::new();
        assert!(!copied_source_markers_match(
            &graph, &original, &copy, &edits
        ));
        graph.sidecars.push(Sidecar {
            adapter: SOURCE_MARKER_COLORS_ADAPTER.into(),
            object: original.id.clone(),
            data: Value::Arr(vec![Value::Obj(vec![
                (
                    "source_marker".into(),
                    Value::Str("original/marker/2".into()),
                ),
                ("resolve_color".into(), Value::Str("Mint".into())),
                ("premiere_fallback".into(), Value::Str("Green".into())),
                ("premiere_observed".into(), Value::Str("Green".into())),
                ("restored".into(), Value::Bool(true)),
            ])]),
        });
        assert!(copied_source_markers_match(
            &graph, &original, &copy, &edits
        ));
        assert_eq!(original.markers[0].color.as_ref().unwrap().name, "Mint");
        copy.markers[0].color.as_mut().unwrap().name = "Red".into();
        assert!(!copied_source_markers_match(
            &graph, &original, &copy, &edits
        ));
        copy.markers[0].color.as_mut().unwrap().name = "Green".into();
        copy.markers[0].comment = "Changed note".into();
        assert!(!copied_source_markers_match(
            &graph, &original, &copy, &edits
        ));
    }
}
