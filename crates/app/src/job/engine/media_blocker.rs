//! Attribute blocked Avid media to saved edit occurrences for Review.
use super::{ReviewBlocker, ReviewGroup};
use crate::{png_mattes, premiere_preparation::{timeline_subject, timeline_subject_at}};
use avid_adapter::MediaEditDetails;
use avid_reader::{
    extract::{KIND_PICTURE, KIND_SOUND},
    io::MobId,
    source::SourceInput,
};
use graph::{source::SourceGraph, Rational, StreamKind, Value};
use std::collections::{HashMap, HashSet};

const MAX_EDIT_DETAILS: usize = 1024;

pub(super) fn append_media_edit_details(
    blocker: &mut ReviewBlocker,
    details: &MediaEditDetails,
    inputs: &[SourceInput],
    reason: &str,
    show_package: bool,
) {
    let existing_groups: HashMap<_, _> = blocker.timeline_groups.iter().enumerate()
        .map(|(index, group)| (group.id.clone(), index)).collect();
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
            blocker.affected.push(note);
        }
        let indices = start..blocker.affected.len();
        if let Some(&index) = existing_groups.get(&timeline.id) {
            blocker.timeline_groups[index].note_indices.extend(indices);
        } else {
            blocker.timeline_groups.push(ReviewGroup {
                id: timeline.id.clone(),
                name: timeline.name.clone(),
                bin: inputs.get(timeline.bin).map(|input| input.path.clone()),
                note_indices: indices.collect(),
            });
        }
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
    let existing_groups: HashMap<_, _> = blocker.timeline_groups.iter().enumerate()
        .map(|(index, group)| (group.id.clone(), index)).collect();
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
            blocker.affected.push(format!(
                "{subject} · {name:?} · edit {id} references unresolved source {master_id}"
            ));
        }
        let end = blocker.affected.len();
        if end > start {
            if let Some(&index) = existing_groups.get(&sequence.id) {
                blocker.timeline_groups[index].note_indices.extend(start..end);
            } else {
                blocker.timeline_groups.push(ReviewGroup {
                    id: sequence.id.clone(),
                    name: sequence.name.clone(),
                    bin: sequence_bins
                        .get(&sequence.id)
                        .and_then(|&index| inputs.get(index))
                        .map(|input| input.path.clone()),
                    note_indices: (start..end).collect(),
                });
            }
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
        source::{SourceItem, SourceSequence},
        Project, Sidecar,
    };
    use serde_json::json;

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
