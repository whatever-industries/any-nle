//! Drop an Avid dissolve when its saved Gain endpoint is a marked gap.
use super::*;
use graph::source::SourceTransition;

pub(super) fn prepare_cut(
    sequence: &SourceSequence, transition: &SourceTransition, edges: &Value,
    placements: &HashMap<&str, &Placement>, sidecars: &Sidecars<'_>,
    trims: &mut HashMap<&str, Trim>,
) -> R<()> {
    let invalid = |detail| error(format!(
        "unresolved_transition_topology {}: AudioSuite Gain gap {detail}", transition.id));
    let Value::Arr(edges) = edges else { return Err(invalid("requires two saved endpoints")) };
    let [outgoing, incoming] = edges.as_slice() else {
        return Err(invalid("requires two saved endpoints"));
    };
    if transition.kind != StreamKind::Audio || transition.style != "avid.BLEND_AUDIO_DISSOLVE"
        || !edges.iter().any(|edge| matches!(edge, Value::Str(_)))
    {
        return Err(invalid("requires a saved audio dissolve and a Gain occurrence"));
    }
    let start = sub(transition.cut, transition.cut_offset)?;
    let end = add(start, transition.duration)?;
    let gap_name = format!("AudioSuite Gain omitted · A{}", transition.track);
    let mut kept = None;
    for (out, edge, binding) in [
        (true, outgoing, &transition.outgoing),
        (false, incoming, &transition.incoming),
    ] {
        match edge {
            Value::Str(owner) if binding.placements.is_empty() && binding.reason.is_some() => {
                let mut markers = sequence.markers.iter().filter(|marker| {
                    marker.owner == MarkerOwner::Sequence && marker.owner_occurrence == *owner
                        && marker.name == gap_name
                });
                let marker = markers.next().ok_or_else(|| invalid("is missing its range marker"))?;
                let first = marker.start.ok_or_else(|| invalid("range marker has no start"))?;
                let last = add(first, marker.duration)?;
                if markers.next().is_some() || first >= last
                    || if out { last != transition.cut } else { first != transition.cut }
                {
                    return Err(invalid("range marker disagrees with the saved cut"));
                }
            }
            Value::Bool(true) if binding.placements.is_empty()
                && binding.reason.as_deref() == Some("native owner produced no source placements") => {}
            Value::Null if binding.reason.is_none() => {
                let [id] = binding.placements.as_slice() else {
                    return Err(invalid("requires a single surviving placement"));
                };
                let p = placements[id.as_str()];
                let p_end = add(p.record_start, p.duration)?;
                let on_lane = p.audio_tracks == [transition.track]
                    || matches!(p.grouped_audio.as_slice(), [group] if group.track == transition.track);
                if kept.is_some() || p.video_track.is_some() || !on_lane
                    || p.speed.is_some() || p.frame_map.is_some()
                    || !p.volume_keys.is_empty() || !p.effects.is_empty()
                    || side(sidecars, "avid.motion_adapter", &p.id).is_some()
                    || if out { p_end != end || p.record_start >= transition.cut }
                        else { p.record_start != start || p_end <= transition.cut }
                {
                    return Err(invalid("cannot preserve the neighboring trim at its cut"));
                }
                kept = Some((out, id.as_str()));
            }
            _ => return Err(invalid("has an unresolved neighboring endpoint")),
        }
    }
    if let Some((out, id)) = kept {
        let trim = trims.get_mut(id).unwrap();
        if out {
            trim.end = trim.end.min(transition.cut);
        } else {
            trim.start = trim.start.max(transition.cut);
        }
    }
    Ok(())
}
