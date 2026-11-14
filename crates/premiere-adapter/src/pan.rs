//! Translate qualified Avid PVOL points from the sequence's native occurrence receipts.
use super::*;
use prproj::convert::prepared::{AudioPanKey, MAX_AUDIO_PAN_KEYS};

pub(super) const SOURCE_OMISSION: &str =
    "Avid clip pan keys are retained in source facts and omitted from the converted edit";

pub(super) fn prepare(
    facts: &PreparationFacts,
    sequence: &SourceSequence,
    prepared: &Sequence,
    sequence_index: usize,
    ownership: &HashMap<String, (usize, usize)>,
    clips: &HashSet<&str>,
    output: &mut Vec<PreparedAudioPan>,
    written: &mut HashSet<String>,
    losses: &mut Vec<Loss>,
) {
    let Some(Value::Arr(native)) = side(&facts.sidecars, "avid.source", &sequence.id)
        .and_then(|v| key(v, "native_events"))
    else {
        return;
    };
    let mut by_placement = HashMap::new();
    for occurrence in native {
        if let Some(id) = key(occurrence, "placement_id").and_then(str_value) {
            by_placement.entry(id)
                .and_modify(|entry: &mut Option<&Value>| *entry = None)
                .or_insert(Some(occurrence));
        }
    }
    for placement in &sequence.placements {
        let Some(Some(occurrence)) = by_placement.get(placement.id.as_str()) else {
            continue;
        };
        let Some(pan) = key(occurrence, "controls").and_then(|v| key(v, "pvol_pan")) else {
            continue;
        };
        let result = (|| {
            let (owner, index) = *ownership.get(&placement.id)?;
            let event = prepared.events.get(index)?;
            let [track] = placement.audio_tracks.as_slice() else {
                return None;
            };
            if owner != sequence_index || sequence.rate != Rational::integer(24)
                || (prepared.width, prepared.height) != (1920, 1080)
                || !clips.contains(event.clip.as_str()) || event.audio_tracks != [*track]
                || event.video_track.is_some() || !event.stereo.is_empty()
                || event.speed_ratio.is_some() || event.time_remap.is_some()
                || event.audio != EventAudio::default() || event.audio_gains.iter().flatten().any(|g| *g != 0.0)
                || event.audio_pans.iter().flatten().any(|p| *p != 0.0)
                || prepared.audio_crossfades.iter().any(|fade| fade.track == *track)
                || placement.source_in.to_units(TICKS_PER_SECOND) != Some(event.source_in)
                || placement.record_start.to_units(TICKS_PER_SECOND) != Some(event.record_start)
                || placement.duration.to_units(TICKS_PER_SECOND) != Some(event.duration)
                || key(occurrence, "id").and_then(str_value) != Some(placement.id.as_str())
                || key(occurrence, "kind").and_then(integer) != Some(2)
                || key(occurrence, "track").and_then(integer) != Some(*track)
                || key(occurrence, "combined") != Some(&Value::Null)
                || !matches!(key(occurrence, "via"), Some(Value::Arr(v)) if v.is_empty())
                || key(pan, "unqualified").is_some()
                || facts.losses.get(&(placement.id.as_str(), "pan_automation"))
                    .into_iter().flatten().any(|&reason| reason != SOURCE_OMISSION)
            {
                return None;
            }
            let range = key(occurrence, "range")?;
            let frame_ticks = TICKS_PER_SECOND / 24;
            if key(range, "record_rate").and_then(str_value) != Some("24")
                || key(range, "source_rate").and_then(str_value) != Some("24")
                || ![("record_start", event.record_start), ("source_in", event.source_in),
                    ("record_length", event.duration), ("source_length", event.duration)]
                    .into_iter().all(|(name, ticks)| key(range, name).and_then(integer)
                        .and_then(|frame| frame.checked_mul(frame_ticks)) == Some(ticks))
            {
                return None;
            }
            let (keys, rounded) = prepare_keys(pan, event.source_in, event.duration)?;
            Some((PreparedAudioPan { sequence: owner, event: index, track: *track, keys }, rounded))
        })();
        if let Some((pan, rounded)) = result {
            output.push(pan);
            written.insert(placement.id.clone());
            loss(losses, &placement.id, "pan_interpolation",
                "Avid clip pan points are editable Linear keys in Premiere. The native pan envelope differs between editors");
            if rounded {
                loss(losses, &placement.id, "pan_key_clock",
                    "Avid subframe pan times rounded to the nearest Premiere tick. Exact times remain in source facts");
            }
        } else if !facts.losses.contains_key(&(placement.id.as_str(), "pan_automation")) {
            loss(losses, &placement.id, "pan_automation", SOURCE_OMISSION);
        }
    }
}

fn prepare_keys(pan: &Value, source_in: i64, duration: i64) -> Option<(Vec<AudioPanKey>, bool)> {
    if key(pan, "control_code").and_then(integer) != Some(2000)
        || key(pan, "saved_interp_kind").and_then(integer) != Some(3)
    {
        return None;
    }
    let Value::Arr(points) = key(pan, "keys")? else {
        return None;
    };
    if !(2..=MAX_AUDIO_PAN_KEYS).contains(&points.len()) {
        return None;
    }
    let mut keys: Vec<AudioPanKey> = Vec::with_capacity(points.len());
    let mut rounded = false;
    for point in points {
        let n = i128::from(key(point, "frame_numerator").and_then(integer)?);
        let d = i128::from(key(point, "frame_denominator").and_then(integer)?);
        let raw = key(point, "raw_pan").and_then(integer)?;
        let scaled = n * i128::from(TICKS_PER_SECOND / 24);
        if d <= 0 || n < 0 || scaled > i128::from(duration) * d || !(0..=32768).contains(&raw) {
            return None;
        }
        rounded |= scaled % d != 0;
        let ticks = source_in.checked_add(i64::try_from((scaled + d / 2) / d).ok()?)?;
        if keys.last().is_some_and(|previous| previous.ticks >= ticks) {
            return None;
        }
        keys.push(AudioPanKey { ticks, value: raw as f64 / 32768.0 });
    }
    Some((keys, rounded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_to_a_tick_and_refuses_colliding_or_out_of_range_points() {
        let control = |points: &[(i64, i64, i64)]| Value::Obj(vec![
            ("control_code".into(), Value::Int(2000)),
            ("saved_interp_kind".into(), Value::Int(3)),
            ("keys".into(), Value::Arr(points.iter().map(|&(num, den, raw)| Value::Obj(vec![
                ("frame_numerator".into(), Value::Int(num)),
                ("frame_denominator".into(), Value::Int(den)),
                ("raw_pan".into(), Value::Int(raw)),
            ])).collect())),
        ]);
        let start = 2 * TICKS_PER_SECOND;
        let duration = 2 * TICKS_PER_SECOND;
        let (keys, rounded) = prepare_keys(&control(&[(0, 1, 0), (25641, 2048, 32768)]),
            start, duration).unwrap();
        assert!(rounded);
        assert_eq!(keys[1], AudioPanKey { ticks: 640_543_886_719, value: 1.0 });
        assert!(prepare_keys(&control(&[(0, 1, 0), (1, 1 << 40, 32768)]), start, duration).is_none());
        assert!(prepare_keys(&control(&[(0, 1, 0), (1, 0, 32768)]), start, duration).is_none());
        assert!(prepare_keys(&control(&[(0, 1, 0), (49, 1, 32768)]), start, duration).is_none());
        assert!(prepare_keys(&control(&[(0, 1, 0), (12, 1, 32769)]), start, duration).is_none());
    }
}
