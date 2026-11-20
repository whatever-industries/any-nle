//! Mark exact record spans lost when Premiere cannot construct an offline master.
use super::*;

fn placement_lanes(p: &Placement) -> impl Iterator<Item = (bool, i64)> + '_ {
    p.video_track.into_iter().map(|track| (true, track)).chain(
        p.audio_tracks.iter().copied().chain(p.grouped_audio.iter().map(|a| a.track))
            .map(|track| (false, track)),
    )
}

pub(super) fn append(
    facts: &PreparationFacts<'_>,
    sequence: &SourceSequence,
    trims: &HashMap<&str, Trim>,
    events: &[Event],
    losses: &mut Vec<Loss>,
    markers: &mut Vec<Marker>,
) -> R<()> {
    let omitted = |p: &Placement| facts.omits_placement(p);
    if !sequence.placements.iter().any(omitted) {
        return Ok(());
    }
    let mut occupied: HashMap<_, Vec<(i64, i64)>> = HashMap::new();
    for event in events {
        let end = event.record_start.checked_add(event.duration)
            .ok_or_else(|| error("prepared record end overflows"))?;
        let lanes = event.video_track.into_iter().map(|track| (true, track)).chain(
            event.audio_tracks.iter().copied().chain(event.stereo.iter().map(|a| a.audio_track))
                .map(|track| (false, track)),
        );
        for lane in lanes {
            occupied.entry(lane).or_default().push((event.record_start, end));
        }
    }
    for p in sequence.disabled_placements.iter().filter(|p| !omitted(p)) {
        let start = ticks(p.record_start, "disabled record start");
        let end = ticks(add(p.record_start, p.duration)?, "disabled record end");
        let span = start.ok().zip(end.ok()).unwrap_or((i64::MIN, i64::MAX));
        for lane in placement_lanes(p) {
            occupied.entry(lane).or_default().push(span);
        }
    }
    // The last prefix before the candidate end can answer overlap in log time.
    for spans in occupied.values_mut() {
        spans.sort_unstable();
        let mut last = i64::MIN;
        for (_, end) in spans {
            last = last.max(*end);
            *end = last;
        }
    }
    let unit = model::ticks_per_unit(fraction(sequence.rate));
    for p in sequence.placements.iter().filter(|p| omitted(p)) {
        let trim = trims[p.id.as_str()];
        let span = ticks(trim.start, "offline record start").ok()
            .zip(ticks(trim.end, "offline record end").ok())
            .filter(|&(start, end)| unit > 0 && start >= 0 && end > start && end <= 1_i64 << 62
                && start % unit == 0 && end % unit == 0);
        for lane @ (picture, track) in placement_lanes(p) {
            let label = format!("{}{track}", if picture { 'V' } else { 'A' });
            let description = format!("{} · {label} · {} at {}–{} seconds",
                sequence.name, p.id, trim.start, trim.end);
            let safe = span.filter(|&(start, end)| {
                occupied.get(&lane).is_none_or(|spans| {
                    let count = spans.partition_point(|(other_start, _)| *other_start < end);
                    count == 0 || spans[count - 1].1 <= start
                })
            });
            let Some((start, end)) = safe else {
                loss(losses, &p.id, "offline_edit_omitted", format!(
                    "{description}: offline source omitted; a frame-aligned vacant span was not established."));
                continue;
            };
            let reason = if facts.omitted_managed_items.contains(p.item.as_str()) {
                "managed media has no selected Premiere media route"
            } else {
                "unqualified offline managed media omitted"
            };
            let comment = format!("{description}: {reason}.");
            loss(losses, &p.id, "media_gap", comment.clone());
            markers.push(Marker {
                name: format!("Media omitted · {label}"),
                comment: comment.chars().filter(|&c| !c.is_control() && !matches!(c, '\u{fffe}' | '\u{ffff}'))
                    .take(4096).collect(),
                start,
                duration: end - start,
                user: String::new(),
                color: String::new(),
                track: None,
                owner: "sequence".into(),
                event: None,
                source: None,
                inherited: false,
            });
        }
    }
    Ok(())
}
