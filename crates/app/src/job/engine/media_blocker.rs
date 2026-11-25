//! Attribute blocked Avid media to saved edit occurrences for Review.
use super::{CancelToken, ReviewBlocker, ReviewGroup};
use crate::{png_mattes, premiere_preparation::{
    timeline_subject, timeline_subject_at, timeline_subject_with_exact_offset,
}};
use avid_adapter::{AudioGroupIssue, MediaEditDetails};
use avid_reader::{
    extract::{KIND_PICTURE, KIND_SOUND},
    io::MobId,
    source::SourceInput,
};
use graph::{
    source::{Multicam, MulticamPiece, MulticamUse, SourceGraph},
    Rational, StreamKind, Value,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

pub(super) const MAX_EDIT_DETAILS: usize = 1024;

/// Group detail indices by saved timeline while preserving the first row's ownership.
pub(super) struct TimelineGroups<'a> {
    groups: &'a mut Vec<ReviewGroup>,
    positions: HashMap<String, usize>,
}

impl<'a> TimelineGroups<'a> {
    pub(super) fn new(groups: &'a mut Vec<ReviewGroup>) -> Self {
        let positions = groups.iter().enumerate()
            .map(|(index, group)| (group.id.clone(), index)).collect();
        Self { groups, positions }
    }

    pub(super) fn extend(
        &mut self,
        id: &str,
        name: &str,
        bin: Option<&Path>,
        indices: impl IntoIterator<Item = usize>,
    ) {
        if let Some(&index) = self.positions.get(id) {
            self.groups[index].note_indices.extend(indices);
        } else {
            self.positions.insert(id.to_owned(), self.groups.len());
            self.groups.push(ReviewGroup {
                id: id.to_owned(),
                name: name.to_owned(),
                bin: bin.map(Path::to_owned),
                note_indices: indices.into_iter().collect(),
            });
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum FileKind {
    Managed,
    Original,
}

pub(super) fn append_png_edit(
    blocker: &mut ReviewBlocker,
    source: &SourceGraph,
    sequence_bins: &HashMap<String, usize>,
    bins: &[PathBuf],
    edit: png_mattes::FailedPngEdit,
    cancel: &CancelToken,
) {
    // Bound these details independently of the source-graph and media limits.
    const MAX_PNG_EDIT_DETAILS: usize = 128;
    let related = png_mattes::matching_overlay_failures(
        source, &edit, MAX_PNG_EDIT_DETAILS, || cancel.is_cancelled(),
    );
    if related.len() == MAX_PNG_EDIT_DETAILS {
        blocker.message.push_str("\nAffected PNG edits reached the diagnostic size limit.");
    }
    let mut groups = TimelineGroups::new(&mut blocker.timeline_groups);
    for edit in std::iter::once(edit).chain(related.into_iter().take(MAX_PNG_EDIT_DETAILS - 1)) {
        let Some(sequence) = source.sequences.iter().find(|seq| seq.id == edit.sequence) else {
            continue;
        };
        let subject = timeline_subject_with_exact_offset(
            sequence, StreamKind::Video, edit.track, edit.record_start,
        );
        let mut note = format!("{subject} · PNG source preparation failed · edit {}", edit.id);
        if !edit.saved_locators.is_empty() {
            note.push_str(&format!(" · saved PNG {:?}", edit.saved_locators));
        }
        let bin = sequence_bins.get(&sequence.id).and_then(|&index| bins.get(index));
        groups.extend(
            &sequence.id, &sequence.name, bin.map(PathBuf::as_path),
            std::iter::once(blocker.affected.len()),
        );
        blocker.diagnostic_locations.insert(blocker.affected.len(),
            crate::diagnostics::SavedLocation::from_sequence(sequence, StreamKind::Video, edit.track, edit.record_start));
        blocker.affected.push(note);
    }
}

/// Follow up on an early verification failure using file metadata only.
pub(super) fn append_unavailable_files<'a>(
    blocker: &mut ReviewBlocker,
    source: &SourceGraph,
    sequence_bins: &HashMap<String, usize>,
    bins: &[PathBuf],
    remaining_paths: impl Iterator<Item = &'a Path>,
    kind: FileKind,
    cancel: &CancelToken,
) {
    const MAX_PATH_CHECKS: usize = 256;
    const MAX_ADDITIONAL_FILES: usize = 8;
    let mut failures = 0;
    let (label, expected_file, limit_notice) = match kind {
        FileKind::Managed => (
            "Managed file",
            "expected a regular managed file",
            "\nAdditional managed file checks reached the diagnostic limit.",
        ),
        FileKind::Original => (
            "Original file",
            "original is not a file",
            "\nAdditional original file checks reached the diagnostic limit.",
        ),
    };
    for (index, path) in remaining_paths.enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        if index == MAX_PATH_CHECKS || failures == MAX_ADDITIONAL_FILES {
            blocker.message.push_str(limit_notice);
            break;
        }
        let reason = match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => continue,
            Ok(_) => expected_file.into(),
            Err(error) => error.to_string(),
        };
        failures += 1;
        let finding = format!("{label} {}: {reason}", path.display());
        let first = blocker.affected.len();
        append_file_uses(blocker, source, sequence_bins, bins, path, kind);
        if blocker.affected.len() == first {
            blocker.affected.push(finding);
        } else {
            for note in &mut blocker.affected[first..] {
                note.push_str(" · ");
                note.push_str(&finding);
            }
        }
    }
}

fn selected_group_has_piece(
    group: &Multicam,
    usage: &MulticamUse,
    matches: impl Fn(&MulticamPiece, StreamKind) -> bool,
) -> bool {
    let (None, Some(selected), Some(start), Some(duration)) =
        (&usage.reason, usage.selected, usage.source_in, usage.duration)
    else {
        return false;
    };
    let Some(end) = start.checked_add(duration) else { return false };
    let alternative = group.lanes.iter()
        .find(|lane| lane.kind == usage.kind && Some(lane.index) == usage.lane)
        .and_then(|lane| lane.alternatives.get(usize::try_from(selected).ok()?));
    let Some(alternative) = alternative.filter(|alt| alt.reason.is_none()) else { return false };
    let first = alternative.pieces.partition_point(|piece| {
        piece.start.checked_add(piece.duration).is_some_and(|end| end <= start)
    });
    alternative.pieces[first..]
        .iter()
        .take_while(|piece| piece.start < end)
        .any(|piece| matches(piece, usage.kind))
}

#[derive(Default)]
pub(super) struct SelectedGroupUses<'a> {
    pub(super) by_sequence: HashMap<&'a str, Vec<(&'a Multicam, &'a MulticamUse)>>,
    pub(super) bound_placements: HashSet<&'a str>,
}

pub(super) fn selected_group_uses(
    source: &SourceGraph,
    matches: impl Fn(&MulticamPiece, StreamKind) -> bool,
) -> SelectedGroupUses<'_> {
    let mut selected = SelectedGroupUses::default();
    if source.multicam_uses.is_empty() {
        return selected;
    }
    let groups: HashMap<_, _> = source.multicams.iter()
        .map(|group| (group.id.as_str(), group))
        .collect();
    for usage in &source.multicam_uses {
        let [id] = usage.candidates.as_slice() else { continue };
        let Some(group) = groups.get(id.as_str()) else { continue };
        if !selected_group_has_piece(group, usage, &matches) {
            continue;
        }
        // The selected Group use is more specific than its bound leaf placements.
        if usage.binding.reason.is_none() {
            selected.bound_placements.extend(usage.binding.placements.iter().map(String::as_str));
        }
        selected.by_sequence.entry(usage.sequence.as_str()).or_default().push((*group, usage));
    }
    selected
}

pub(super) fn append_file_uses(
    blocker: &mut ReviewBlocker,
    source: &SourceGraph,
    sequence_bins: &HashMap<String, usize>,
    bins: &[PathBuf],
    path: &Path,
    kind: FileKind,
) {
    let failed: HashSet<_> = source
        .media
        .iter()
        .filter(|media| {
            media.original.is_some() == matches!(kind, FileKind::Original)
                && media.online == Some(true)
                && matches!(media.locations.as_slice(), [location] if Path::new(location) == path)
        })
        .map(|media| media.id.as_str())
        .collect();
    let items: HashMap<_, _> = source
        .items
        .iter()
        .filter(|item| {
            item.media
                .iter()
                .any(|link| failed.contains(link.media.as_str()))
        })
        .map(|item| (item.id.as_str(), item))
        .collect();
    if items.is_empty() {
        return;
    }
    let media: HashMap<_, _> = source
        .media
        .iter()
        .map(|media| (media.id.as_str(), media))
        .collect();
    let selected = selected_group_uses(source, |piece, kind| {
        let Some(item) = piece.item.as_deref().and_then(|id| items.get(id)) else {
            return false;
        };
        item.media.iter()
            .filter_map(|link| media.get(link.media.as_str()))
            .any(|stream| {
                failed.contains(stream.id.as_str())
                    && stream.kind == kind
                    && (stream.stream == piece.stream
                        || (kind == StreamKind::Video
                            && stream.stream.is_none()
                            && piece.stream == Some(1)))
            })
    });
    const TRUNCATED: &str = "\nAffected edits reached the diagnostic size limit.";
    let mut groups = TimelineGroups::new(&mut blocker.timeline_groups);
    for sequence in &source.sequences {
        let direct_uses = sequence
            .placements
            .iter()
            .map(|edit| (edit, "edit"))
            .chain(
                sequence
                    .disabled_placements
                    .iter()
                    .map(|edit| (edit, "disabled edit")),
            )
            .filter_map(|(edit, state)| {
                if selected.bound_placements.contains(edit.id.as_str()) {
                    return None;
                }
                let item = items.get(edit.item.as_str())?;
                let streams = |kind| {
                    item.media
                        .iter()
                        .filter_map(|link| media.get(link.media.as_str()).copied())
                        .filter(move |media| media.kind == kind)
                };
                let (kind, track, affected) = if let Some(track) = edit.video_track {
                    (
                        StreamKind::Video,
                        track,
                        streams(StreamKind::Video)
                            .any(|stream| failed.contains(stream.id.as_str())),
                    )
                } else {
                    let mut audio = streams(StreamKind::Audio);
                    let (track, labels) = if let [group] = edit.grouped_audio.as_slice() {
                        (group.track, group.streams.as_slice())
                    } else if let [track] = edit.audio_tracks.as_slice() {
                        (*track, edit.audio_streams.as_slice())
                    } else {
                        return None;
                    };
                    let affected = if labels.is_empty() {
                        audio
                            .next()
                            .is_some_and(|stream| failed.contains(stream.id.as_str()))
                    } else {
                        audio.any(|stream| {
                            failed.contains(stream.id.as_str())
                                && stream.stream.is_some_and(|label| labels.contains(&label))
                        })
                    };
                    (StreamKind::Audio, track, affected)
                };
                affected.then(|| {
                    let subject = timeline_subject(sequence, kind, track, Some(edit.record_start));
                    format!(
                        "{subject} · Media verification failed for {:?} · {state} {}",
                        item.name, edit.id,
                    )
                })
            });
        let group_uses = selected.by_sequence.get(sequence.id.as_str()).into_iter().flatten()
            .map(|(group, usage)| {
                let subject = timeline_subject(sequence, usage.kind, usage.track, usage.record_start);
                format!(
                    "{subject} · Media verification failed for Group {:?} · selected edit {}",
                    group.name, usage.id,
                )
            });
        let mut uses = direct_uses.chain(group_uses);
        let start = blocker.affected.len();
        let remaining = MAX_EDIT_DETAILS.saturating_sub(start);
        blocker.affected.extend(uses.by_ref().take(remaining));
        let end = blocker.affected.len();
        if end > start {
            let bin = sequence_bins.get(&sequence.id).and_then(|&index| bins.get(index));
            groups.extend(&sequence.id, &sequence.name, bin.map(PathBuf::as_path), start..end);
        }
        if uses.next().is_some() {
            if !blocker.message.ends_with(TRUNCATED) {
                blocker.message.push_str(TRUNCATED);
            }
            break;
        }
    }
}

pub(super) fn append_audio_group_details(
    blocker: &mut ReviewBlocker,
    issues: &[AudioGroupIssue],
    details: &MediaEditDetails,
    inputs: &[SourceInput],
) {
    for issue in issues {
        let mut note = format!("{} · source {}", issue.message, issue.source_id);
        if let Some(input) = inputs.get(issue.bin) {
            note.push_str(&format!(" · Avid bin {}", input.path.display()));
        }
        blocker.affected.push(note);
    }
    append_media_edit_details(blocker, details, inputs, "Unresolved audio group", false);
}

pub(super) fn append_media_edit_details(
    blocker: &mut ReviewBlocker,
    details: &MediaEditDetails,
    inputs: &[SourceInput],
    reason: &str,
    show_package: bool,
) {
    let mut groups = TimelineGroups::new(&mut blocker.timeline_groups);
    for timeline in &details.timelines {
        let start = blocker.affected.len();
        for edit in &timeline.edits {
            let subject = timeline_subject_at(
                &timeline.name, edit.kind, edit.track, timeline.rate, edit.offset, timeline.clock,
            );
            let lane = match edit.kind {
                StreamKind::Video => "V",
                StreamKind::Audio => "A",
            };
            let mut note = format!(
                "{subject} · {reason} for {:?} {lane}{} · edit {} · source {}",
                edit.master_name, edit.master_track, edit.id, edit.master_id,
            );
            if show_package {
                note.push_str(" · package ");
                note.push_str(&edit.file_package);
            }
            blocker.diagnostic_locations.insert(blocker.affected.len(),
                crate::diagnostics::SavedLocation::new(edit.kind, edit.track, edit.offset, timeline.rate, timeline.clock));
            blocker.affected.push(note);
        }
        let indices = start..blocker.affected.len();
        let bin = inputs.get(timeline.bin).map(|input| input.path.as_path());
        groups.extend(&timeline.id, &timeline.name, bin, indices);
    }
    const TRUNCATED: &str = "\nAffected edits reached the diagnostic size limit.";
    if details.truncated && !blocker.message.ends_with(TRUNCATED) {
        blocker.message.push_str(TRUNCATED);
    }
}

struct UnresolvedMaster<'a> {
    bin: &'a str,
    name: &'a str,
    reason: &'a str,
    occurrence: Option<&'a str>,
    mob: Option<&'a str>,
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value.get(key) {
        Some(Value::Str(value)) => Some(value),
        _ => None,
    }
}

fn records<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    match value.get(key) {
        Some(Value::Arr(values)) => values,
        _ => &[],
    }
}

fn master_facts<'a>(
    source: &'a SourceGraph,
    matte: &'a png_mattes::Plan,
) -> impl Iterator<Item = UnresolvedMaster<'a>> {
    source
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.source")
        .flat_map(move |s| {
            records(&s.data, "unresolved_masters")
                .iter()
                .filter_map(move |record| {
                    let reason = text(record, "reason")?;
                    let occurrence = text(record, "occurrence");
                    if occurrence
                        .is_some_and(|id| !matte.keeps_unresolved_master(&s.object, id, reason))
                    {
                        return None;
                    }
                    Some(UnresolvedMaster {
                        bin: &s.object,
                        name: text(record, "name")?,
                        reason,
                        occurrence,
                        mob: text(record, "mob_id"),
                    })
                })
        })
}

pub(super) fn unresolved_masters(
    source: &SourceGraph,
    inputs: &[SourceInput],
    matte: &png_mattes::Plan,
) -> Vec<String> {
    master_facts(source, matte)
        .map(|master| {
            let mut detail = format!("{}: {}", master.name, master.reason);
            if let Some(occurrence) = master.occurrence {
                detail.push_str(&format!(" · {occurrence}"));
            }
            if let Some(bin) = master
                .bin
                .strip_prefix("bin:")
                .and_then(|i| i.parse::<usize>().ok())
                .and_then(|index| inputs.get(index))
            {
                detail.push_str(&format!(" · {}", bin.path.display()));
            }
            detail
        })
        .collect()
}

pub(super) fn append_unresolved_uses(
    blocker: &mut ReviewBlocker,
    source: &SourceGraph,
    sequence_bins: &HashMap<String, usize>,
    inputs: &[SourceInput],
    matte: &png_mattes::Plan,
    reported_edits: &HashSet<&str>,
) {
    let mut by_mob = HashMap::new();
    for master in master_facts(source, matte) {
        if let Some(mob) = master.mob {
            by_mob
                .entry(mob)
                .and_modify(|owner| *owner = None)
                .or_insert(master.occurrence.map(|id| (master.name, id)));
        }
    }
    if by_mob.is_empty() {
        return;
    }
    let portable: HashSet<_> = source
        .items
        .iter()
        .filter_map(|item| item.umid.as_deref())
        .collect();
    by_mob.retain(|mob, master| {
        master.is_some()
            && MobId::from_urn(mob)
                .is_ok_and(|native| !portable.contains(native.umid_hex().as_str()))
    });
    if by_mob.is_empty() {
        return;
    }
    // Only selected sequence receipts with retained native events can establish
    // an edit reference. Reuse them without expanding staged graph receipts.
    let receipts: HashMap<_, _> = source
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.source" && s.data.get("native_events").is_some())
        .map(|s| (s.object.as_str(), &s.data))
        .collect();
    let mut count = 0;
    let mut groups = TimelineGroups::new(&mut blocker.timeline_groups);
    for sequence in &source.sequences {
        let Some(receipt) = receipts.get(sequence.id.as_str()) else {
            continue;
        };
        let start = blocker.affected.len();
        let rate = sequence.rate.to_string();
        for event in records(receipt, "native_events") {
            if !matches!(event.get("placement_id"), Some(Value::Null)) {
                continue;
            }
            let (Some(mob), Some(id), Some(Value::Int(track))) =
                (text(event, "master"), text(event, "id"), event.get("track"))
            else {
                continue;
            };
            if reported_edits.contains(id) {
                continue;
            }
            let Some(Some((name, master_id))) = by_mob.get(mob) else {
                continue;
            };
            let kind = match event.get("kind") {
                Some(Value::Int(KIND_PICTURE)) => StreamKind::Video,
                Some(Value::Int(KIND_SOUND)) => StreamKind::Audio,
                _ => continue,
            };
            if *track < 1 {
                continue;
            }
            count += 1;
            if count > MAX_EDIT_DETAILS {
                continue;
            }
            let position = event.get("range").and_then(|range| {
                if text(range, "record_rate") != Some(rate.as_str()) {
                    return None;
                }
                let Some(Value::Int(frame)) = range.get("record_start") else {
                    return None;
                };
                Rational::new(
                    i128::from(*frame) * i128::from(sequence.rate.den()),
                    sequence.rate.num().into(),
                )
            });
            let subject = timeline_subject(sequence, kind, *track, position);
            blocker.diagnostic_locations.insert(
                blocker.affected.len(),
                crate::diagnostics::SavedLocation::from_sequence(sequence, kind, *track, position),
            );
            blocker.affected.push(format!(
                "{subject} · {name:?} · edit {id} references unresolved source {master_id}"
            ));
        }
        let end = blocker.affected.len();
        if end > start {
            let bin = sequence_bins.get(&sequence.id)
                .and_then(|&index| inputs.get(index)).map(|input| input.path.as_path());
            groups.extend(&sequence.id, &sequence.name, bin, start..end);
        }
    }
    if count > MAX_EDIT_DETAILS {
        blocker.affected.push(format!(
            "Showing {MAX_EDIT_DETAILS} of {count} edits that reference unresolved sources."
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::BlockerKind;
    use graph::{
        source::{MulticamAlternative, MulticamLane, SourceBinding, SourceItem, SourceItemMedia, SourceMedia, SourceSequence},
        Bin, Placement, Project, Sidecar,
    };
    use serde_json::json;

    #[test]
    fn failed_group_file_is_attributed_to_the_exact_selected_piece() {
        let second = Rational::integer(1);
        let two_seconds = Rational::integer(2);
        let additional_path = std::env::temp_dir().join(format!(
            "anynle-group-review-{}-absent.mxf",
            std::process::id(),
        ));
        let media = |id: &str, path: &str, stream| SourceMedia {
            id: id.into(), kind: StreamKind::Audio, locations: vec![path.into()],
            codec: None, frame_size: None, rate: None, sample_rate: Some(48_000), bits: Some(24),
            duration: two_seconds, start: None, clock: None, stream: Some(stream),
            online: Some(true), original: None,
        };
        let capture = |id: &str, media: &[&str]| SourceItem {
            id: id.into(), name: "Same camera name".into(), tape: None, rate: None,
            umid: None, color_rgb16: None, stream_groups: vec![], markers: vec![],
            media: media.iter().map(|id| SourceItemMedia {
                media: (*id).into(), gain_db: 0.0, source_offset: Some(Rational::ZERO),
            }).collect(),
        };
        let piece = |item: &str, stream, start, duration| MulticamPiece {
            item: Some(item.into()), stream: Some(stream), source_in: Some(Rational::ZERO),
            start, duration,
        };
        let usage = |id: &str, lane, selected, record_start, source_in| MulticamUse {
            id: id.into(), sequence: "sequence".into(), candidates: vec!["group".into()],
            kind: StreamKind::Audio, lane: Some(lane), track: lane, selected: Some(selected),
            record_start: Some(record_start), source_in: Some(source_in), duration: Some(second),
            binding: SourceBinding { placements: vec![], reason: None }, reason: None,
        };
        let mut bound = usage("selected", 3, 1, second, second);
        bound.binding.placements.push("flattened".into());
        let source = SourceGraph {
            project: Project { name: "Synthetic Group".into() },
            media: vec![
                media("normal", "/one/A03.mxf", 3),
                media("failed", "/two/A03.mxf", 3),
                media("other-lane", additional_path.to_str().unwrap(), 2),
            ],
            items: vec![capture("camera-one", &["normal"]), capture("camera-two", &["failed", "other-lane"])],
            bins: vec![Bin {
                id: "bin".into(), name: "Group bin".into(), parent: None,
                items: ["camera-one", "camera-two", "group"].map(String::from).to_vec(),
            }],
            sequences: vec![SourceSequence {
                id: "sequence".into(), name: "Timeline".into(), rate: Rational::integer(24),
                frame_size: None, start: None, clock: None, tracks: vec![],
                placements: vec![Placement {
                    id: "flattened".into(), item: "camera-two".into(), record_start: second,
                    source_in: Rational::ZERO, duration: second, video_track: None,
                    audio_tracks: vec![3], audio_streams: vec![3], grouped_audio: vec![],
                    gain_db: 0.0, volume_db: None, volume_keys: vec![], track_gains_db: vec![],
                    track_pans: vec![], speed: None, frame_map: None, transform: None, effects: vec![],
                }],
                disabled_placements: vec![], transitions: vec![], markers: vec![], texts: vec![],
            }],
            multicams: vec![Multicam {
                id: "group".into(), name: "Group".into(), rate: Some(Rational::integer(24)),
                duration: Some(two_seconds), reason: None,
                lanes: vec![
                    MulticamLane {
                        kind: StreamKind::Audio, index: 2, selected: Some(0), reason: None,
                        alternatives: vec![MulticamAlternative {
                            id: "a2-alternative".into(), reason: None,
                            pieces: vec![piece("camera-two", 2, Rational::ZERO, two_seconds)],
                        }],
                    },
                    MulticamLane {
                        kind: StreamKind::Audio, index: 3, selected: Some(0), reason: None,
                        alternatives: vec![
                            MulticamAlternative {
                                id: "default".into(), reason: None,
                                pieces: vec![piece("camera-one", 3, Rational::ZERO, two_seconds)],
                            },
                            MulticamAlternative {
                                id: "switched".into(), reason: None,
                                pieces: vec![
                                    piece("camera-one", 3, Rational::ZERO, second),
                                    piece("camera-two", 3, second, second),
                                ],
                            },
                        ],
                    },
                ],
            }],
            multicam_uses: vec![
                usage("ends-at-boundary", 3, 1, Rational::ZERO, Rational::ZERO),
                bound,
                usage("unbound-selection", 3, 1, two_seconds, second),
                usage("unswitched", 3, 0, Rational::integer(3), second),
                usage("different-stream", 2, 0, second, second),
            ],
            losses: vec![],
            sidecars: vec![],
        };
        source.validate().unwrap();
        let bins = [PathBuf::from("Timelines.avb")];
        let sequence_bins = HashMap::from([("sequence".into(), 0)]);
        let mut issue = ReviewBlocker::new(BlockerKind::AvidMedia, "Failed to verify A03.mxf");
        append_file_uses(
            &mut issue, &source, &sequence_bins, &bins,
            Path::new("/two/A03.mxf"), FileKind::Managed,
        );
        assert_eq!(issue.affected, [
            "Timeline · A3 · frame 24 from sequence start · Media verification failed for Group \"Group\" · selected edit selected",
            "Timeline · A3 · frame 48 from sequence start · Media verification failed for Group \"Group\" · selected edit unbound-selection",
        ]);
        assert_eq!(issue.timeline_groups.len(), 1);
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&bins[0]));
        assert_eq!(issue.timeline_groups[0].note_indices, [0, 1]);

        let cancel = CancelToken::default();
        append_unavailable_files(
            &mut issue, &source, &sequence_bins, &bins,
            std::iter::once(additional_path.as_path()), FileKind::Managed, &cancel,
        );
        assert_eq!(issue.message, "Failed to verify A03.mxf");
        assert_eq!(issue.affected.len(), 3);
        assert!(issue.affected[2].contains("A2 · frame 24 from sequence start"));
        assert!(issue.affected[2].contains("selected edit different-stream"));
        assert!(issue.affected[2].contains(additional_path.to_str().unwrap()));
        assert_eq!(issue.timeline_groups.len(), 1);
        assert_eq!(issue.timeline_groups[0].note_indices, [0, 1, 2]);

        cancel.cancel();
        let before = issue.clone();
        append_unavailable_files(
            &mut issue, &source, &sequence_bins, &bins,
            std::iter::once(additional_path.as_path()), FileKind::Managed, &cancel,
        );
        assert_eq!(issue, before);

        let mut source = source;
        source.bins[0].id = "bin:0".into();
        let mut unknown = usage("unknown-selection", 3, 1, second, second);
        unknown.reason = Some("Saved selection is unresolved".into());
        source.multicam_uses.push(unknown);
        let missing = avid_adapter::MissingGroupAudio {
            lane: 3, alternative: 1, start: second, end: two_seconds,
            source: "Same camera name".into(), source_track: 3, package: "missing-package".into(),
        };
        let mut issue = ReviewBlocker::new(BlockerKind::AvidMedia, "Group media unavailable");
        issue.affected.push("Duplicate package detail".into());
        issue.timeline_groups.push(ReviewGroup {
            id: "sequence".into(), name: "Timeline".into(),
            bin: Some(bins[0].clone()), note_indices: vec![0],
        });
        super::super::append_group_preparation_details(
            &mut issue, &source, &sequence_bins, &bins, [("group", &[missing][..])],
        );
        assert_eq!(issue.affected, [
            "Duplicate package detail",
            "\"Group\" · saved Group group · Avid bin Timelines.avb",
            "Missing MXF package missing-package · \"Same camera name\" A3",
            "Timeline · A3 · frame 24 from sequence start · Group edit selected · missing MXF package missing-package",
            "Timeline · A3 · frame 48 from sequence start · Group edit unbound-selection · missing MXF package missing-package",
        ]);
        assert_eq!(issue.timeline_groups.len(), 1);
        assert_eq!(issue.timeline_groups[0].note_indices, [0, 3, 4]);
        source.multicam_uses.pop();
        source.sidecars.push(Sidecar {
            adapter: "avid.source".into(),
            object: "normal".into(),
            data: graph::json::parse(r#"{"legacy_clock":null}"#).unwrap(),
        });
        source.items[1].media[0].source_offset = None;
        let refusal = premiere_adapter::prepare_avid_job(
            &source, &prproj::convert::model::Options::default(),
            premiere_adapter::ClockProfile::LegacyImport, false, false, None,
        ).err().expect("unresolved source origin refuses clip preparation");
        assert!(refusal.message.starts_with("unresolved_source_offset camera-two -> failed"));
        let issue = super::super::premiere_preparation_blocker(
            refusal, &source, Some((&sequence_bins, &bins)),
        );
        assert_eq!(issue.affected, [
            "\"Same camera name\" · saved clip camera-two · Avid bin Timelines.avb",
            "Timeline · A3 · frame 24 from sequence start · Group \"Group\" · selected edit selected",
            "Timeline · A3 · frame 48 from sequence start · Group \"Group\" · selected edit unbound-selection",
            "Timeline · A2 · frame 24 from sequence start · Group \"Group\" · selected edit different-stream",
        ]);
        let [group] = issue.timeline_groups.as_slice() else { panic!("expected one timeline") };
        assert_eq!(group.id, "sequence");
        assert_eq!(group.bin.as_ref(), Some(&bins[0]));
        assert_eq!(group.note_indices, [1, 2, 3]);
    }

    #[test]
    fn missing_media_review_uses_exact_identity_and_skips_ambiguous_copies() {
        let first = MobId::from_urn(&"01".repeat(32)).unwrap();
        let second = MobId::from_urn(&"02".repeat(32)).unwrap();
        let master = |id: &str, mob: &MobId| {
            json!({
                "occurrence": id, "name": "Same name", "reason": "Missing picture", "mob_id": mob.urn(),
            })
        };
        let sidecar = |id: &str, data: serde_json::Value| Sidecar {
            adapter: "avid.source".into(),
            object: id.into(),
            data: graph::json::parse(&data.to_string()).unwrap(),
        };
        let event = |id: &str, mob: &MobId, placement: Option<&str>| {
            json!({
                "id": id, "placement_id": placement, "master": mob.urn(),
                "kind": KIND_PICTURE, "track": 1,
                "range": { "record_start": 12, "record_rate": "24" },
            })
        };
        let mut source = SourceGraph {
            project: Project {
                name: "Project".into(),
            },
            media: vec![],
            items: vec![],
            bins: vec![],
            losses: vec![],
            multicams: vec![],
            multicam_uses: vec![],
            sequences: vec![SourceSequence {
                id: "seq".into(),
                name: "Sequence".into(),
                rate: Rational::integer(24),
                frame_size: None,
                start: None,
                clock: None,
                tracks: vec![],
                placements: vec![],
                disabled_placements: vec![],
                transitions: vec![],
                markers: vec![],
                texts: vec![],
            }],
            sidecars: vec![
                sidecar(
                    "bin:0",
                    json!({ "unresolved_masters": [
                    master("master:unique", &first), master("master:copy-a", &second),
                ] }),
                ),
                sidecar(
                    "bin:1",
                    json!({ "unresolved_masters": [master("master:copy-b", &second)] }),
                ),
                sidecar(
                    "seq",
                    json!({ "native_events": [
                    event("missing edit", &first, None), event("ambiguous edit", &second, None),
                    event("converted edit", &first, Some("placement")),
                ] }),
                ),
            ],
        };
        let inputs = [SourceInput {
            path: "Timelines.avb".into(),
            revision: String::new(),
        }];
        let sequence_bins = HashMap::from([("seq".into(), 0)]);
        let plan = png_mattes::Plan::default();
        let mut issue = ReviewBlocker::new(BlockerKind::RewrapMissingMedia, "Required files");
        issue.affected.push("Prior media finding".into());
        append_unresolved_uses(&mut issue, &source, &sequence_bins, &inputs, &plan, &HashSet::new());
        assert_eq!(issue.affected.len(), 2);
        assert!(issue.affected[1].contains("V1 · frame 12 from sequence start"));
        assert!(issue.affected[1]
            .ends_with("edit missing edit references unresolved source master:unique"));
        assert_eq!(issue.timeline_groups[0].note_indices, [1]);
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&inputs[0].path));

        source.sidecars[2] = sidecar("seq", json!({ "native_events": [
            event("missing edit", &first, None), event("other missing edit", &first, None),
        ] }));
        let mut issue = ReviewBlocker::new(BlockerKind::AvidMedia, "Duplicate media");
        issue.affected.push("Specific finding for missing edit".into());
        issue.timeline_groups.push(ReviewGroup {
            id: "seq".into(), name: "Sequence".into(), bin: Some(inputs[0].path.clone()),
            note_indices: vec![0],
        });
        append_unresolved_uses(&mut issue, &source, &sequence_bins, &inputs, &plan,
            &HashSet::from(["missing edit"]));
        assert_eq!(issue.timeline_groups.len(), 1);
        assert_eq!(issue.timeline_groups[0].note_indices, [0, 1]);
        assert_eq!(issue.affected.len(), 2);
        assert!(issue.affected[1].contains("edit other missing edit"));

        source.items.push(SourceItem {
            id: "available".into(),
            name: "Same name".into(),
            media: vec![],
            tape: None,
            rate: None,
            umid: Some(first.umid_hex()),
            color_rgb16: None,
            stream_groups: vec![],
            markers: vec![],
        });
        let mut issue = ReviewBlocker::new(BlockerKind::RewrapMissingMedia, "Required files");
        append_unresolved_uses(&mut issue, &source, &sequence_bins, &inputs, &plan, &HashSet::new());
        assert!(issue.affected.is_empty());
        assert!(issue.timeline_groups.is_empty());
    }
}
