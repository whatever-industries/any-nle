//! Preserve the editable boundary of a proven inline Avid Submaster.
use super::*;
use prproj::convert::prepared::{plain_picture_edit, PreparedPictureNest};

pub(super) fn prepare(
    graph: &SourceGraph, sidecars: &Sidecars<'_>, input: &mut PreparedProjectInput,
    ownership: &HashMap<String, (usize, usize)>, typed: bool, clock: ClockProfile, losses: &mut Vec<Loss>,
) {
    let rate = Rational::new(24_000, 1001).unwrap();
    let online: HashSet<_> = graph.media.iter().filter(|m| m.online == Some(true))
        .map(|m| m.id.as_str()).collect();
    let linked: HashSet<_> = graph.items.iter().filter(|item| {
        item.media.iter().all(|reference| online.contains(reference.media.as_str()))
    }).map(|item| item.id.as_str()).collect();
    let busy: HashSet<_> = input.picture_effects.iter().map(|e| (e.sequence, e.event))
        .chain(input.video_dissolves.iter().flat_map(|e| [(e.sequence, e.incoming), (e.sequence, e.outgoing)]))
        .chain(input.multicam_uses.iter().map(|e| (e.sequence, e.event)))
        .chain(input.av_subclip_uses.iter().map(|e| (e.sequence, e.event)))
        .chain(input.edit_labels.iter().map(|e| (e.sequence, e.event)))
        .collect();
    for sequence in &graph.sequences {
        let used_markers: HashSet<_> = sequence.markers.iter()
            .flat_map(|m| m.binding.placements.iter().map(String::as_str)).collect();
        for placement in &sequence.placements {
            let Some(facts) = side(sidecars, "avid.source", &placement.id) else { continue };
            let Some(Value::Arr(via)) = key(facts, "via") else { continue };
            if !via.iter().any(|entry| str_value(entry) == Some("effect EFF_SUBMASTER not converted")) {
                continue;
            }
            let admitted = ownership.get(&placement.id).filter(|&&(si, ei)| {
                let target = &input.sequences[si];
                let edit = &target.events[ei];
                typed && clock == ClockProfile::LegacyImport && input.options.bin_mode == "separate"
                    && sequence.rate == rate && qualified_sequence_start(sequence).is_some_and(|s| s > Rational::ZERO)
                    && (target.width, target.height) == (1920, 1080)
                    && linked.contains(placement.item.as_str())
                    && input.direct_dnxhr_origins.contains_key(&edit.clip)
                    && key(facts, "plain_submaster").and_then(boolean) == Some(true) && via.len() == 1
                    && plain_picture_edit(edit) && !busy.contains(&(si, ei))
                    && !used_markers.contains(placement.id.as_str())
                    && ticks(placement.source_in, "Submaster source").ok() == Some(edit.source_in)
                    && ticks(placement.record_start, "Submaster record").ok() == Some(edit.record_start)
                    && ticks(placement.duration, "Submaster length").ok() == Some(edit.duration)
            });
            if let Some(&(sequence, event)) = admitted {
                input.picture_nests.push(PreparedPictureNest { sequence, event });
            } else {
                loss(losses, &placement.id, "picture_nest",
                    "Avid Submaster boundary flattened; editable output requires one plain same-rate linked V1 input and an exact clock");
            }
        }
    }
}
