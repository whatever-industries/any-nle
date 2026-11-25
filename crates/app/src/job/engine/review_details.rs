//! Project saved Avid evidence into the Review report.
use super::{media_blocker, BlockerKind, ClipMode, ReportNote, ReviewBlocker, ReviewGroup};
use crate::premiere_preparation::{timeline_subject, timeline_subject_with_exact_offset};
use avid_reader::source::SourceInput;
use crate::diagnostics::SavedLocation;
use media_blocker::{append_audio_group_details, append_media_edit_details, append_unresolved_uses};
use std::{collections::{HashMap, HashSet}, path::PathBuf};

pub(super) fn append_source_blocker_details(
    issue: &mut ReviewBlocker,
    read: &mut avid_adapter::ReadResult,
    inputs: &[SourceInput],
    bins: &[PathBuf],
    matte: &crate::png_mattes::Plan,
) {
    issue.affected.append(&mut read.scan_issues);
    append_audio_group_details(issue, &read.audio_group_issues, &read.audio_group_edits, inputs);
    append_media_edit_details(issue, &read.package_conflict_edits, inputs, "Duplicate MXF package", true);
    let missing = &read.missing_managed_edits;
    append_media_edit_details(issue, missing, inputs, "Missing MXF package", true);
    let reported_edits = [&read.package_conflict_edits, missing].into_iter()
        .flat_map(|details| &details.timelines).flat_map(|sequence| &sequence.edits)
        .map(|edit| edit.id.as_str()).collect();
    append_unresolved_uses(issue, &read.graph, &read.sequence_bins, inputs, matte, &reported_edits);
    if !read.ambiguous_managed_packages.is_empty() && !read.missing_group_audio.is_empty() {
        let missing = read.missing_group_audio.iter()
            .map(|(id, members)| (id.as_str(), members.as_slice()));
        append_group_preparation_details(issue, &read.graph, &read.sequence_bins, bins, missing);
        if read.missing_group_audio_truncated {
            issue.affected.push("Missing Group audio details reached the diagnostic size limit.".into());
        }
    }
}

pub(super) fn job_preparation_blocker(
    error: premiere_adapter::PreparationIssue,
    read: &avid_adapter::ReadResult,
    bins: &[PathBuf],
    for_review: bool,
) -> ReviewBlocker {
    let missing_group = match &error.occurrence {
        Some(premiere_adapter::PreparationOccurrence::Group(id)) if for_review =>
            read.missing_group_audio.get(id).map(|missing| (id, missing)),
        _ => None,
    };
    if let Some((id, missing)) = missing_group {
        let mut issue = ReviewBlocker::new(BlockerKind::AvidMedia, error.message);
        append_group_preparation_details(
            &mut issue, &read.graph, &read.sequence_bins, bins,
            std::iter::once((id.as_str(), missing.as_slice())),
        );
        if read.missing_group_audio_truncated {
            issue.affected.push("Missing Group audio details reached the diagnostic size limit.".into());
        }
        return issue;
    }
    premiere_preparation_blocker(error, &read.graph,
        for_review.then_some((&read.sequence_bins, bins)))
}

pub(super) fn source_read_blocker(
    issue: avid_adapter::SourceReadIssue,
    linked: bool,
    inputs: &[SourceInput],
) -> ReviewBlocker {
    use avid_adapter::{OriginalLocatorKind, SourceReadIssue};
    use BlockerKind::*;

    let in_bin = |kind, message, bin: usize| {
        let mut blocker = ReviewBlocker::new(kind, message);
        if let Some(input) = inputs.get(bin) {
            blocker.affected.push(format!("Avid bin {}", input.path.display()));
        }
        blocker
    };
    match issue {
        SourceReadIssue::OriginalLocator(issue) => {
            let edit_reason = if issue.kind == OriginalLocatorKind::LegacyMxfPackage {
                "Unresolved MXF package"
            } else {
                "Unresolved saved path"
            };
            let kind = match issue.kind {
                OriginalLocatorKind::LegacyMxfPackage => LegacyMxfPackage,
                OriginalLocatorKind::MissingOriginal if !linked => RewrapMissingOriginal,
                _ => OriginalLocator,
            };
            let mut blocker = in_bin(kind, issue.message, issue.bin);
            if issue.kind == OriginalLocatorKind::MissingOriginal {
                blocker.diagnostic_reason = Some(crate::diagnostics::Reason::MissingOriginal);
            }
            blocker.affected.extend(issue.additional.into_iter().map(|(bin, mut detail)| {
                if let Some(input) = inputs.get(bin) {
                    detail.push_str(&format!(" · {}", input.path.display()));
                }
                detail
            }));
            if issue.details_truncated {
                blocker.message.push_str("\nAdditional locator checks reached the diagnostic size limit.");
            }
            append_media_edit_details(&mut blocker, &issue.edits, inputs, edit_reason, false);
            blocker
        }
        SourceReadIssue::Bin(message) => ReviewBlocker::new(ProjectSource, message),
        SourceReadIssue::OriginalMedia(message) => ReviewBlocker::new(OriginalMedia, message),
        SourceReadIssue::OriginalMediaTracks(issue) => {
            let mut blocker = in_bin(OriginalMedia, issue.message, issue.bin);
            append_media_edit_details(
                &mut blocker, &issue.edits, inputs, "Original media verification failed", false,
            );
            blocker
        }
        SourceReadIssue::MediaScan(message) => ReviewBlocker::new(AvidMedia, message),
        SourceReadIssue::SourceFacts(message) => ReviewBlocker::new(SourceContent, message),
    }
}

pub(super) fn media_scan_blocker(
    scan_issues: &[String],
    audio_group_issues: &[avid_adapter::AudioGroupIssue],
    for_review: bool,
) -> ReviewBlocker {
    let kind = if scan_issues.is_empty() {
        BlockerKind::SourceContent
    } else {
        BlockerKind::AvidMedia
    };
    let summary = "Media scan or channel routing needs resolution";
    if for_review {
        ReviewBlocker::new(kind, summary)
    } else {
        let details = scan_issues
            .iter()
            .chain(audio_group_issues.iter().map(|issue| &issue.message))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        ReviewBlocker::new(kind, format!("{summary}: {details}"))
    }
}

pub(crate) fn sequence_timing_blocker(
    source: &graph::source::SourceGraph,
    review_bins: Option<(&HashMap<String, usize>, &[PathBuf])>,
) -> Option<ReviewBlocker> {
    let mut blocked = source
        .losses
        .iter()
        .filter(|loss| loss.aspect == avid_adapter::UNQUALIFIED_ROUTE_TIMING)
        .peekable();
    blocked.peek()?;
    let names: HashMap<&str, &str> = source
        .sequences
        .iter()
        .map(|sequence| (sequence.id.as_str(), sequence.name.as_str()))
        .collect();
    let mut issue = ReviewBlocker::new(
        BlockerKind::SequenceTiming,
        "Conversion would omit edits with unsupported Avid sequence timing",
    );
    let mut seen = HashSet::new();
    let mut affected_owners = Vec::new();
    for loss in blocked {
        if seen.insert((loss.object.as_str(), loss.reason.as_str())) {
            let name = names.get(loss.object.as_str()).copied().unwrap_or("Selected sequence");
            issue.affected.push(format!("{name} · {}", loss.reason));
            if review_bins.is_some() {
                affected_owners.push(loss.object.as_str());
            }
        }
    }
    if let Some((sequence_bins, bins)) = review_bins {
        issue.timeline_groups = review_timeline_groups(source, affected_owners.iter().copied(), sequence_bins, bins);
    }
    Some(issue)
}

pub(super) fn premiere_preparation_blocker(
    error: premiere_adapter::PreparationIssue,
    source: &graph::source::SourceGraph,
    review_bins: Option<(&HashMap<String, usize>, &[PathBuf])>,
) -> ReviewBlocker {
    use premiere_adapter::PreparationOccurrence::{CombinedPlacements, Group, GroupUse, Item, Placement, Transition};

    let mut issue = ReviewBlocker::new(BlockerKind::SourceContent, error.message);
    let Some((sequence_bins, bins)) = review_bins else { return issue };
    if let Some(Item(id)) = &error.occurrence {
        append_preparation_clip_uses(&mut issue, id, source, sequence_bins, bins);
        return issue;
    }
    if let Some(Group(id)) = &error.occurrence {
        append_group_preparation_details(
            &mut issue, source, sequence_bins, bins, std::iter::once((id.as_str(), &[][..])),
        );
        return issue;
    }
    if let Some(id) = error.sequence {
        if let Some(sequence) = source.sequences.iter().find(|sequence| sequence.id == id) {
            let placement_detail = |edit: &graph::Placement, role| {
                let (kind, track) = placement_lane(edit)?;
                let subject = timeline_subject_with_exact_offset(
                    sequence, kind, track, Some(edit.record_start),
                );
                Some((format!("{subject} · {role} {}", edit.id),
                    SavedLocation::from_sequence(sequence, kind, track, Some(edit.record_start))))
            };
            let mut truncated = false;
            match error.occurrence {
                Some(Transition(id)) => {
                    if let Some(t) = sequence.transitions.iter().find(|t| t.id == id) {
                        let subject = timeline_subject(sequence, t.kind, t.track, Some(t.cut));
                        issue.push_diagnostic_edit(format!("{subject} · transition {id}"),
                            SavedLocation::from_sequence(sequence, t.kind, t.track, Some(t.cut)));
                    } else if let Some((subject, location)) = native_transition_subject(source, sequence, &id) {
                        issue.push_diagnostic_edit(subject, location);
                    }
                }
                Some(Placement(id)) => {
                    if let Some((detail, location)) = sequence.placements.iter()
                        .find(|edit| edit.id == id).and_then(|edit| placement_detail(edit, "edit"))
                    {
                        issue.push_diagnostic_edit(detail, location);
                    }
                }
                Some(CombinedPlacements { ids, truncated: more }) => {
                    let ids: HashSet<_> = ids.iter().map(String::as_str).collect();
                    for (detail, location) in sequence.placements.iter()
                        .filter(|edit| ids.contains(edit.id.as_str()))
                        .filter_map(|edit| placement_detail(edit, "combined edit member"))
                    {
                        issue.push_diagnostic_edit(detail, location);
                    }
                    truncated = more;
                }
                Some(GroupUse(id)) => {
                    if let Some(usage) = source.multicam_uses.iter().find(|u| u.id == id && u.sequence == sequence.id) {
                        issue.push_diagnostic_edit(group_edit_subject(sequence, usage),
                            SavedLocation::from_sequence(sequence, usage.kind, usage.track, usage.record_start));
                    }
                }
                Some(Item(_) | Group(_)) | None => {}
            }
            if issue.affected.is_empty() {
                issue.affected.push(format!("{} · saved sequence {id}", sequence.name));
            }
            if truncated {
                issue.affected.push("Combined edit members reached the diagnostic size limit.".into());
            }
            issue.timeline_groups.push(ReviewGroup {
                bin: sequence_bins.get(&id).and_then(|&index| bins.get(index)).cloned(),
                id,
                name: sequence.name.clone(),
                note_indices: (0..issue.affected.len()).collect(),
            });
        }
    }
    issue
}

fn native_transition_subject(
    source: &graph::source::SourceGraph,
    sequence: &graph::source::SourceSequence,
    id: &str,
) -> Option<(String, SavedLocation)> {
    use graph::Value;

    if !id.strip_prefix(&sequence.id).is_some_and(|suffix| suffix.starts_with('/')) {
        return None;
    }
    let Value::Arr(rows) = source.sidecar("avid.source", &sequence.id)?.get("native_transitions")? else {
        return None;
    };
    let mut matches = rows.iter().filter(|row| {
        matches!(row.get("id"), Some(Value::Str(candidate)) if candidate == id)
    });
    let (Some(row), None) = (matches.next(), matches.next()) else { return None };
    let (kind, track) = native_review_lane(row)?;
    let start = native_review_offset(sequence, row.get("rate"), row.get("position").and_then(|pos| pos.get("value")));
    let subject = timeline_subject_with_exact_offset(sequence, kind, track, start);
    let position_label = if start.is_some() { " (saved start)" } else { "" };
    Some((format!("{subject} · transition {id}{position_label}"),
        SavedLocation::from_sequence(sequence, kind, track, start)))
}

fn group_edit_subject(sequence: &graph::source::SourceSequence, usage: &graph::source::MulticamUse) -> String {
    let subject = timeline_subject_with_exact_offset(sequence, usage.kind, usage.track, usage.record_start);
    format!("{subject} · Group edit {}", usage.id)
}

pub(super) fn append_group_preparation_details<'a>(
    issue: &mut ReviewBlocker,
    source: &graph::source::SourceGraph,
    sequence_bins: &HashMap<String, usize>,
    bins: &[PathBuf],
    groups: impl IntoIterator<Item = (&'a str, &'a [avid_adapter::MissingGroupAudio])>,
) {
    let groups: HashMap<_, _> = groups.into_iter().collect();
    if groups.is_empty() {
        return;
    }
    let mut group_bins = HashMap::new();
    for bin in &source.bins {
        let path = saved_bin_path(&bin.id, bins);
        for id in bin.items.iter().filter(|id| groups.contains_key(id.as_str())) {
            group_bins.entry(id.as_str()).and_modify(|owner| *owner = None).or_insert(path);
        }
    }
    let mut by_alternative = HashMap::new();
    for group in &source.multicams {
        let id = group.id.as_str();
        let Some(&missing_audio) = groups.get(id) else { continue };
        let mut detail = format!("{:?} · saved Group {id}", group.name);
        if let Some(path) = group_bins.get(id).copied().flatten() {
            detail.push_str(&format!(" · Avid bin {}", path.display()));
        }
        issue.affected.push(detail);

        let mut packages = HashSet::new();
        for missing in missing_audio {
            by_alternative.insert((id, missing.lane, missing.alternative), missing);
            if packages.insert(missing.package.as_str()) {
                issue.affected.push(format!(
                    "Missing MXF package {} · {:?} A{}",
                    missing.package, missing.source, missing.source_track,
                ));
            }
        }
    }
    let sequences: HashMap<_, _> = source.sequences.iter()
        .map(|sequence| (sequence.id.as_str(), sequence)).collect();
    let mut uses = source.multicam_uses.iter()
        .filter_map(|usage| {
            let [id] = usage.candidates.as_slice() else { return None };
            let missing_audio = groups.get(id.as_str())?;
            let sequence = *sequences.get(usage.sequence.as_str())?;
            if missing_audio.is_empty() {
                return Some((sequence, usage, None));
            }
            if usage.reason.is_some() || usage.kind != graph::StreamKind::Audio {
                return None;
            }
            let missing = by_alternative.get(&(id.as_str(), usage.lane?, usage.selected?))?;
            let start = usage.source_in?;
            let end = start.checked_add(usage.duration?)?;
            (start < missing.end && missing.start < end).then_some((sequence, usage, Some(*missing)))
        });
    let mut owners = vec![""; issue.affected.len()];
    for (sequence, usage, missing) in uses.by_ref().take(media_blocker::MAX_EDIT_DETAILS) {
        let mut detail = group_edit_subject(sequence, usage);
        if let Some(missing) = missing {
            detail.push_str(&format!(" · missing MXF package {}", missing.package));
        }
        issue.push_diagnostic_edit(detail,
            SavedLocation::from_sequence(sequence, usage.kind, usage.track, usage.record_start));
        owners.push(usage.id.as_str());
    }
    if uses.next().is_some() {
        issue.affected.push("Affected edits reached the diagnostic size limit.".into());
    }
    let mut timeline_groups = media_blocker::TimelineGroups::new(&mut issue.timeline_groups);
    for group in review_timeline_groups(source, owners.iter().copied(), sequence_bins, bins) {
        timeline_groups.extend(&group.id, &group.name, group.bin.as_deref(), group.note_indices);
    }
}

fn append_preparation_clip_uses(
    issue: &mut ReviewBlocker,
    id: &str,
    source: &graph::source::SourceGraph,
    sequence_bins: &HashMap<String, usize>,
    bins: &[PathBuf],
) {
    let Some(item) = source.items.iter().find(|item| item.id == id) else { return };
    let mut detail = format!("{:?} · saved clip {id}", item.name);
    if let Some(path) = unique_saved_item_bin(source, id, bins) {
        detail.push_str(&format!(" · Avid bin {}", path.display()));
    }
    issue.affected.push(detail);

    let selected = media_blocker::selected_group_uses(source, |piece, _| {
        piece.item.as_deref() == Some(id)
    });
    let mut remaining = media_blocker::MAX_EDIT_DETAILS;
    for sequence in &source.sequences {
        let direct_uses = sequence.placements.iter().map(|edit| (edit, "edit"))
            .chain(sequence.disabled_placements.iter().map(|edit| (edit, "disabled edit")))
            .filter(|(edit, _)| edit.item == id && !selected.bound_placements.contains(edit.id.as_str()))
            .filter_map(|(edit, state)| {
                let (kind, track) = placement_lane(edit)?;
                let subject = timeline_subject_with_exact_offset(
                    sequence, kind, track, Some(edit.record_start),
                );
                Some((format!("{subject} · {state} {}", edit.id),
                    SavedLocation::from_sequence(sequence, kind, track, Some(edit.record_start))))
            });
        let group_uses = selected.by_sequence.get(sequence.id.as_str()).into_iter().flatten()
            .filter_map(|(group, usage)| {
                let subject = timeline_subject_with_exact_offset(
                    sequence, usage.kind, usage.track, Some(usage.record_start?),
                );
                Some((format!("{subject} · Group {:?} · selected edit {}", group.name, usage.id),
                    SavedLocation::from_sequence(sequence, usage.kind, usage.track, usage.record_start)))
            });
        let mut uses = direct_uses.chain(group_uses);
        let start = issue.affected.len();
        for (detail, location) in uses.by_ref().take(remaining) {
            issue.push_diagnostic_edit(detail, location);
        }
        let end = issue.affected.len();
        remaining -= end - start;
        if end > start {
            issue.timeline_groups.push(ReviewGroup {
                id: sequence.id.clone(),
                name: sequence.name.clone(),
                bin: sequence_bins.get(&sequence.id).and_then(|&index| bins.get(index)).cloned(),
                note_indices: (start..end).collect(),
            });
        }
        if uses.next().is_some() {
            issue.affected.push("Affected edits reached the diagnostic size limit.".into());
            break;
        }
    }
}

fn placement_lane(placement: &graph::Placement) -> Option<(graph::StreamKind, i64)> {
    // Graph/2 placements occupy one lane, including a multichannel audio group.
    if let Some(track) = placement.video_track {
        Some((graph::StreamKind::Video, track))
    } else {
        placement.audio_tracks.first().copied()
            .or_else(|| placement.grouped_audio.first().map(|group| group.track))
            .map(|track| (graph::StreamKind::Audio, track))
    }
}

pub(super) fn group_clip_mode_blocker(
    source: &graph::source::SourceGraph,
    mode: ClipMode,
    review_bins: Option<(&HashMap<String, usize>, &[PathBuf])>,
) -> Option<ReviewBlocker> {
    if !matches!(mode, ClipMode::Grouped | ClipMode::Merged)
        || (source.multicams.is_empty() && source.multicam_uses.is_empty())
    {
        return None;
    }
    let mut issue = ReviewBlocker::new(
        BlockerKind::GroupClipMode,
        "Linked editable Groups require Separate files",
    );
    issue.affected.extend(source.multicams.iter().map(|group| {
        if group.name.is_empty() { group.id.clone() } else { group.name.clone() }
    }));
    if let Some((sequence_bins, bins)) = review_bins {
        let names: HashMap<_, _> = source.multicams.iter()
            .filter(|group| !group.name.is_empty())
            .map(|group| (group.id.as_str(), group.name.as_str()))
            .collect();
        let sequences: HashMap<_, _> = source.sequences.iter()
            .map(|sequence| (sequence.id.as_str(), sequence))
            .collect();
        let mut owners = vec![""; issue.affected.len()];
        let mut uses = source.multicam_uses.iter().filter_map(|usage| {
            sequences.get(usage.sequence.as_str()).map(|sequence| (sequence, usage))
        });
        for (sequence, usage) in uses.by_ref().take(media_blocker::MAX_EDIT_DETAILS) {
            let subject = timeline_subject(sequence, usage.kind, usage.track, usage.record_start);
            let group = match usage.candidates.as_slice() {
                [id] => names.get(id.as_str()).copied().unwrap_or(id),
                _ => "Unresolved Group",
            };
            issue.push_diagnostic_edit(format!("{subject} · {group} · edit {}", usage.id),
                SavedLocation::from_sequence(sequence, usage.kind, usage.track, usage.record_start));
            owners.push(&usage.id);
        }
        if uses.next().is_some() {
            issue.message.push_str("\nAffected edits reached the diagnostic size limit.");
        }
        issue.timeline_groups = review_timeline_groups(source, owners.into_iter(), sequence_bins, bins);
    }
    Some(issue)
}

pub(super) fn review_subjects(
    source: &graph::source::SourceGraph,
    notes: &[ReportNote],
    diagnostic_locations: Option<&mut HashMap<usize, SavedLocation>>,
) -> HashMap<String, String> {
    let wanted: HashSet<&str> = notes
        .iter()
        .filter(|n| matches!(n.severity.as_str(), "loss" | "warning"))
        .map(|n| n.object.as_str())
        .collect();
    let mut subjects = HashMap::new();
    let mut saved = HashMap::new();
    let mut locate = |id: &str, sequence, kind: crate::diagnostics::TrackKind, track, offset| {
        if diagnostic_locations.is_some() {
            saved.insert(id.to_owned(), SavedLocation::from_sequence(sequence, kind, track, offset));
        }
    };
    let mut names: HashMap<&str, &str> = HashMap::new();
    for item in &source.items {
        names.insert(&item.id, &item.name);
    }
    for sequence in &source.sequences {
        names.insert(&sequence.id, &sequence.name);
        for transition in &sequence.transitions {
            if wanted.contains(transition.id.as_str()) {
                locate(&transition.id, sequence, transition.kind.into(), transition.track, Some(transition.cut));
                subjects.insert(
                    transition.id.clone(),
                    timeline_subject(sequence, transition.kind, transition.track, Some(transition.cut)),
                );
            }
        }
        for text in &sequence.texts {
            if wanted.contains(text.id.as_str()) {
                let subject = timeline_subject_with_exact_offset(
                    sequence, graph::StreamKind::Video, text.track, text.record_start,
                );
                let (kind, lane) = match text.kind {
                    graph::source::TextKind::Caption => ("caption", crate::diagnostics::TrackKind::Caption),
                    graph::source::TextKind::Graphic => ("title", crate::diagnostics::TrackKind::Video),
                };
                locate(&text.id, sequence, lane, text.track, text.record_start);
                subjects.insert(text.id.clone(), format!("{subject} · {kind}"));
            }
        }
        for placement in &sequence.placements {
            if !wanted.contains(placement.id.as_str()) {
                continue;
            }
            if let Some((kind, track)) = placement_lane(placement) {
                locate(&placement.id, sequence, kind.into(), track, Some(placement.record_start));
                subjects.insert(
                    placement.id.clone(),
                    timeline_subject(sequence, kind, track, Some(placement.record_start)),
                );
            }
        }
        for edit in &sequence.disabled_placements {
            if wanted.contains(edit.id.as_str()) {
                if let Some((kind, track)) = placement_lane(edit) {
                    locate(&edit.id, sequence, kind.into(), track, Some(edit.record_start));
                    let subject = timeline_subject_with_exact_offset(
                        sequence, kind, track, Some(edit.record_start),
                    );
                    subjects.insert(edit.id.clone(), format!("{subject} · disabled edit"));
                }
            }
        }
    }
    let sequences: HashMap<_, _> = source.sequences.iter()
        .map(|sequence| (sequence.id.as_str(), sequence)).collect();
    for usage in &source.multicam_uses {
        if wanted.contains(usage.id.as_str()) {
            if let Some(&sequence) = sequences.get(usage.sequence.as_str()) {
                locate(&usage.id, sequence, usage.kind.into(), usage.track, usage.record_start);
                subjects.insert(
                    usage.id.clone(),
                    timeline_subject(sequence, usage.kind, usage.track, usage.record_start),
                );
            }
        }
    }
    for group in &source.multicams {
        names.insert(&group.id, &group.name);
    }
    for bin in &source.bins {
        names.insert(&bin.id, &bin.name);
    }
    let native_effects = notes.iter()
        .filter(|note| note.why == "picture_effect"
            && wanted.contains(note.object.as_str()) && !subjects.contains_key(&note.object))
        .map(|note| note.object.as_str())
        .collect();
    for (id, (subject, location)) in native_picture_effect_subjects(source, &sequences, native_effects) {
        if diagnostic_locations.is_some() {
            saved.insert(id.clone(), location);
        }
        subjects.insert(id, subject);
    }
    if let Some(locations) = diagnostic_locations {
        for (index, note) in notes.iter().enumerate() {
            if let Some(&location) = saved.get(&note.object) {
                locations.entry(index).or_insert(location);
            }
        }
    }
    for id in wanted {
        if subjects.contains_key(id) {
            continue;
        }
        // Native child occurrence IDs extend their sequence's ID with slash
        // components. Use the closest enclosing name, never a different ID.
        let mut object = id;
        loop {
            if let Some(name) = names.get(object).filter(|name| !name.is_empty()) {
                subjects.insert(id.to_owned(), (*name).to_owned());
                break;
            }
            match object.rsplit_once('/') {
                Some((parent, _)) => object = parent,
                None => break,
            }
        }
    }
    subjects
}

fn native_picture_effect_subjects(
    source: &graph::source::SourceGraph,
    sequences: &HashMap<&str, &graph::source::SourceSequence>,
    wanted: HashSet<&str>,
) -> HashMap<String, (String, SavedLocation)> {
    use graph::{StreamKind, Value};

    let mut owners = HashMap::new();
    if !wanted.is_empty() {
        for receipt in source.sidecars.iter().filter(|row| row.adapter == "avid.source") {
            let Some(&sequence) = sequences.get(receipt.object.as_str()) else { continue };
            let Some(Value::Arr(events)) = receipt.data.get("unresolved") else { continue };
            for event in events {
                let Some(Value::Str(owner)) = event.get("owner") else { continue };
                if wanted.contains(owner.as_str())
                    && owner.strip_prefix(&sequence.id).is_some_and(|suffix| suffix.starts_with('/'))
                {
                    owners.entry(owner).and_modify(|row| *row = None).or_insert(Some((sequence, event)));
                }
            }
        }
    }
    owners.into_iter().filter_map(|(owner, row)| {
        let (sequence, event) = row?;
        let (StreamKind::Video, track) = native_review_lane(event)? else { return None };
        let frames = event.get("record_start").and_then(|start| start.get("value"));
        let offset = native_review_offset(sequence, event.get("record_rate"), frames);
        let subject = timeline_subject_with_exact_offset(sequence, StreamKind::Video, track, offset);
        let location = SavedLocation::from_sequence(sequence, StreamKind::Video, track, offset);
        Some((owner.clone(), (format!("{subject} · picture effect"), location)))
    }).collect()
}

pub(super) fn locate_native_review_notes(
    source: &graph::source::SourceGraph,
    notes: &mut [ReportNote],
    mut diagnostic_locations: Option<&mut HashMap<usize, crate::diagnostics::SavedLocation>>,
) -> HashMap<String, String> {
    use graph::Value;

    let mut subjects = HashMap::new();
    // Legacy sequence-owned losses carry the native owner before the prose.
    // Use it as a receipt key. Routing and timing come from saved fields.
    fn legacy_owner(note: &ReportNote) -> Option<&str> {
        note.message.split_once(": ").map(|(owner, _)| owner)
    }
    let mut wanted = HashMap::<_, HashSet<_>>::new();
    for note in notes.iter().filter(|note| matches!(note.why.as_str(), "disabled_clip" | "source_route")) {
        if let Some(owner) = legacy_owner(note) {
            wanted.entry((note.object.as_str(), note.why.as_str())).or_default().insert(owner);
        }
    }
    if wanted.is_empty() {
        return subjects;
    }
    let sequences: HashMap<_, _> = source.sequences.iter()
        .map(|sequence| (sequence.id.as_str(), sequence)).collect();
    let mut owners = HashMap::new();
    for receipt in source.sidecars.iter().filter(|row| row.adapter == "avid.source") {
        let Some(&sequence) = sequences.get(receipt.object.as_str()) else { continue };
        for (aspect, field, id_field) in [
            ("disabled_clip", "unresolved", "owner"),
            ("source_route", "native_events", "id"),
            ("source_route", "unresolved", "owner"),
        ] {
            let Some(wanted_ids) = wanted.get(&(sequence.id.as_str(), aspect)) else { continue };
            let Some(Value::Arr(events)) = receipt.data.get(field) else { continue };
            for event in events {
                let Some(Value::Str(owner)) = event.get(id_field) else { continue };
                if !wanted_ids.contains(owner.as_str())
                    || !owner.strip_prefix(&sequence.id).is_some_and(|suffix| suffix.starts_with('/'))
                {
                    continue;
                }
                // Count every row that claims this ID before qualifying its fields.
                owners.entry((aspect, owner.as_str())).and_modify(|row| *row = None)
                    .or_insert(Some((sequence, event, field)));
            }
        }
    }
    for (index, note) in notes.iter_mut().enumerate() {
        let Some(owner) = legacy_owner(note) else { continue };
        let Some((sequence, event, field)) = owners.get(&(note.why.as_str(), owner)).copied().flatten() else { continue };
        if sequence.id != note.object {
            continue;
        }
        let observed_start = || event.get("record_start").and_then(|start| start.get("value"));
        let (timing, frames, label) = match (note.why.as_str(), field) {
            ("disabled_clip", "unresolved") if event.get("disabled_clip") == Some(&Value::Bool(true)) =>
                (Some(event), observed_start(), "disabled edit"),
            ("source_route", "native_events") if event.get("placement_id") == Some(&Value::Null) => {
                let range = event.get("range");
                (range, range.and_then(|row| row.get("record_start")), "omitted edit")
            }
            ("source_route", "unresolved")
                if matches!(event.get("reason"), Some(Value::Str(reason)) if reason == &note.message) =>
                (Some(event), observed_start(), "source route"),
            _ => continue,
        };
        let Some((kind, track)) = native_review_lane(event) else { continue };
        let offset = native_review_offset(sequence, timing.and_then(|row| row.get("record_rate")), frames);
        if let Some(locations) = diagnostic_locations.as_deref_mut() {
            locations.insert(index, crate::diagnostics::SavedLocation::from_sequence(sequence, kind, track, offset));
        }
        let subject = timeline_subject_with_exact_offset(sequence, kind, track, offset);
        note.object = owner.to_owned();
        subjects.insert(note.object.clone(), format!("{subject} · {label}"));
    }
    subjects
}

fn native_review_lane(event: &graph::Value) -> Option<(graph::StreamKind, i64)> {
    use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};
    use graph::{StreamKind, Value};

    let kind = match event.get("kind") {
        Some(Value::Int(KIND_PICTURE)) => StreamKind::Video,
        Some(Value::Int(KIND_SOUND)) => StreamKind::Audio,
        _ => return None,
    };
    let Some(Value::Int(track)) = event.get("track") else { return None };
    (*track > 0).then_some((kind, *track))
}

fn native_review_offset(
    sequence: &graph::source::SourceSequence,
    rate: Option<&graph::Value>,
    frames: Option<&graph::Value>,
) -> Option<graph::Rational> {
    use graph::{Rational, Value};

    let (Some(Value::Str(rate)), Some(Value::Int(frames))) = (rate, frames) else { return None };
    if *frames < 0 || Rational::parse(rate) != Some(sequence.rate) {
        return None;
    }
    Rational::new(i128::from(*frames) * i128::from(sequence.rate.den()), i128::from(sequence.rate.num()))
}

pub(super) fn locate_source_media_gaps(
    source: &graph::source::SourceGraph,
    notes: &mut [ReportNote],
    mut diagnostic_locations: Option<&mut HashMap<usize, crate::diagnostics::SavedLocation>>,
) -> HashMap<String, String> {
    use graph::{MarkerOwner, Rational, StreamKind, Value};

    let wanted: HashSet<_> = notes.iter()
        .filter(|note| note.why == "media_gap" && note.severity == "loss")
        .map(|note| (note.object.as_str(), note.message.as_str()))
        .collect();
    let mut subjects = HashMap::new();
    if wanted.is_empty() {
        return subjects;
    }
    // A source-capture gap repeats the same diagnostic in the sequence's
    // generated marker. Its start includes any trim made to remove a transition.
    let mut gaps = HashMap::new();
    let mut wanted_events = HashMap::<_, HashSet<_>>::new();
    for sequence in &source.sequences {
        for marker in &sequence.markers {
            let key = (sequence.id.as_str(), marker.comment.as_str());
            if wanted.contains(&key) && marker.owner == MarkerOwner::Sequence
                && marker.track.is_none() && marker.source.is_none() && !marker.inherited
                && marker.binding.reason.is_none() && marker.binding.placements.is_empty()
                && marker.name.starts_with("Media omitted · ")
                && marker.owner_occurrence.strip_prefix(&sequence.id).is_some_and(|rest| rest.starts_with('/'))
            {
                wanted_events.entry(sequence.id.as_str()).or_default().insert(marker.owner_occurrence.as_str());
                gaps.entry(key).and_modify(|gap| *gap = None).or_insert(Some((sequence, marker)));
            }
        }
    }
    if gaps.is_empty() {
        return subjects;
    }
    let mut events = HashMap::new();
    for sidecar in source.sidecars.iter().filter(|row| row.adapter == "avid.source") {
        let Some(ids) = wanted_events.get(sidecar.object.as_str()) else { continue };
        let Some(Value::Arr(native)) = sidecar.data.get("native_events") else { continue };
        for row in native {
            let Some(Value::Str(id)) = row.get("id") else { continue };
            if ids.contains(id.as_str()) {
                events.entry((sidecar.object.as_str(), id.as_str()))
                    .and_modify(|event| *event = None).or_insert(Some(row));
            }
        }
    }
    for (index, note) in notes.iter_mut().enumerate().filter(|(_, note)| note.why == "media_gap" && note.severity == "loss") {
        let key = (note.object.as_str(), note.message.as_str());
        let Some(Some((sequence, marker))) = gaps.get(&key) else { continue };
        let id = marker.owner_occurrence.as_str();
        let Some(Some(event)) = events.get(&(sequence.id.as_str(), id)) else { continue };
        let Some((kind, track)) = native_review_lane(event) else { continue };
        let lane = if kind == StreamKind::Video { 'V' } else { 'A' };
        let Some(start) = marker.start else { continue };
        if event.get("placement_id") != Some(&Value::Null)
            || marker.name != format!("Media omitted · {lane}{track}")
            || marker.owner_local != Some(start) || marker.owner_rate != Some(sequence.rate)
            || start < Rational::ZERO || marker.duration <= Rational::ZERO
        {
            continue;
        }
        let subject = timeline_subject_with_exact_offset(sequence, kind, track, Some(start));
        if let Some(locations) = diagnostic_locations.as_deref_mut() {
            locations.insert(index, crate::diagnostics::SavedLocation::from_sequence(sequence, kind, track, Some(start)));
        }
        note.object = id.to_owned();
        subjects.insert(note.object.clone(), format!("{subject} · media gap"));
    }
    subjects
}

pub(super) fn review_timeline_groups<'a>(
    source: &graph::source::SourceGraph,
    objects: impl Iterator<Item = &'a str> + Clone,
    sequence_bins: &HashMap<String, usize>,
    bins: &[PathBuf],
) -> Vec<ReviewGroup> {
    let wanted: HashSet<_> = objects.clone().collect();
    let sequences: HashMap<_, _> = source.sequences.iter().enumerate()
        .map(|(index, sequence)| (sequence.id.as_str(), index))
        .collect();
    let mut owners = HashMap::new();
    for (index, sequence) in source.sequences.iter().enumerate() {
        let ids = sequence.placements.iter().chain(&sequence.disabled_placements).map(|edit| edit.id.as_str())
            .chain(sequence.transitions.iter().map(|transition| transition.id.as_str()))
            .chain(sequence.texts.iter().map(|text| text.id.as_str()));
        owners.extend(ids.filter(|id| wanted.contains(id)).map(|id| (id, index)));
    }
    for usage in &source.multicam_uses {
        if !wanted.contains(usage.id.as_str()) {
            continue;
        }
        if let Some(&index) = sequences.get(usage.sequence.as_str()) {
            owners.insert(usage.id.as_str(), index);
        }
    }
    let mut indices = vec![Vec::new(); source.sequences.len()];
    for (note_index, mut id) in objects.enumerate() {
        loop {
            if let Some(&index) = owners.get(id).or_else(|| sequences.get(id)) {
                indices[index].push(note_index);
                break;
            }
            // A native occurrence may have been omitted by the graph. Avid
            // extends the owner's exact ID with slash-delimited components.
            let Some((parent, _)) = id.rsplit_once('/') else { break };
            id = parent;
        }
    }
    source.sequences.iter().zip(indices)
        .filter(|(_, note_indices)| !note_indices.is_empty())
        .map(|(sequence, note_indices)| ReviewGroup {
            id: sequence.id.clone(),
            name: sequence.name.clone(),
            bin: sequence_bins.get(&sequence.id).and_then(|&index| bins.get(index)).cloned(),
            note_indices,
        })
        .collect()
}

pub(super) fn review_source_groups(
    source: &graph::source::SourceGraph,
    notes: &[ReportNote],
    bins: &[PathBuf],
) -> Vec<ReviewGroup> {
    let wanted: HashSet<_> = notes.iter().map(|note| note.object.as_str()).collect();
    let items: HashMap<_, _> = source.items.iter().enumerate()
        .map(|(index, item)| (item.id.as_str(), index)).collect();
    let referenced: HashMap<_, _> = source.sequences.iter()
        .flat_map(|sequence| sequence.placements.iter().chain(&sequence.disabled_placements))
        .filter(|edit| wanted.contains(edit.id.as_str()))
        .filter_map(|edit| Some((edit.id.as_str(), *items.get(edit.item.as_str())?)))
        .collect();
    let mut indices: HashMap<usize, Vec<usize>> = HashMap::new();
    for (note_index, note) in notes.iter().enumerate() {
        let id = note.object.as_str();
        if let Some(&index) = items.get(id).or_else(|| referenced.get(id)) {
            indices.entry(index).or_default().push(note_index);
        }
    }
    let mut owners = HashMap::new();
    for bin in &source.bins {
        let path = saved_bin_path(&bin.id, bins);
        for id in &bin.items {
            let Some(&index) = items.get(id.as_str()) else { continue };
            if indices.contains_key(&index) {
                // Membership in multiple bins cannot establish one owner.
                owners.entry(index).and_modify(|path| *path = None).or_insert(path);
            }
        }
    }
    source.items.iter().enumerate()
        .filter_map(|(index, item)| {
            let note_indices = indices.remove(&index)?;
            Some(ReviewGroup {
                id: item.id.clone(),
                name: item.name.clone(),
                bin: owners.get(&index).copied().flatten().cloned(),
                note_indices,
            })
        })
        .collect()
}

fn saved_bin_path<'a>(id: &str, bins: &'a [PathBuf]) -> Option<&'a PathBuf> {
    let index = id.strip_prefix("bin:")?.parse::<usize>().ok()?;
    bins.get(index)
}

fn unique_saved_item_bin<'a>(
    source: &graph::source::SourceGraph, id: &str, bins: &'a [PathBuf],
) -> Option<&'a PathBuf> {
    let mut members = source.bins.iter().filter(|bin| bin.items.iter().any(|item| item == id));
    let (Some(bin), None) = (members.next(), members.next()) else { return None };
    saved_bin_path(&bin.id, bins)
}
