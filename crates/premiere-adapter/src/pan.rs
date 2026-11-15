//! Prepare qualified Linear pan on the final Premiere mono edit.
use super::*;
use graph::audio_pan::{self, PanKey};
use prproj::convert::prepared::{AudioPanKey, MAX_AUDIO_PAN_KEYS};

pub(super) const SOURCE_OMISSION: &str =
    "Avid clip pan keys are retained in source facts and omitted from the converted edit";

/// Find saved PCM16/48 kHz mono edits over a verified zero-origin DNxHD master.
pub(super) fn managed_sources<'a>(
    graph: &'a SourceGraph,
    media: &HashMap<&str, &SourceMedia>,
    sidecars: &Sidecars,
) -> HashSet<&'a str> {
    let controlled: HashSet<_> = graph.losses.iter()
        .filter(|loss| loss.aspect == "pan_automation" && loss.reason == SOURCE_OMISSION)
        .map(|loss| loss.object.as_str())
        .collect();
    let selected: HashSet<_> = graph.sequences.iter()
        .filter(|sequence| sequence.rate == Rational::integer(24))
        .flat_map(|sequence| &sequence.placements)
        .filter(|edit| (controlled.contains(edit.id.as_str())
            || edit.effects.iter().any(|effect| effect.kind == audio_pan::LINEAR_KIND))
            && edit.video_track.is_none() && edit.audio_tracks.len() == 1
            && edit.grouped_audio.is_empty()
            && edit.speed.is_none() && edit.frame_map.is_none())
        .map(|edit| edit.item.as_str())
        .collect();
    graph.items.iter()
        .filter(|item| selected.contains(item.id.as_str())
            && item.rate == Some(Rational::integer(24))
            && item.media.iter().all(|binding|
                binding.gain_db == 0.0 && media.contains_key(binding.media.as_str()))
            && matches!(
                direct_opatom_clocks(item, media, sidecars, true, true, false),
                Some((0, 0, _, _))
            ))
        .map(|item| item.id.as_str())
        .collect()
}

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
    let native = match side(&facts.sidecars, "avid.source", &sequence.id)
        .and_then(|v| key(v, "native_events"))
    {
        Some(Value::Arr(events)) => events.as_slice(),
        _ => &[],
    };
    let mut legacy = HashMap::new();
    for occurrence in native {
        if let Some(id) = key(occurrence, "placement_id").and_then(str_value) {
            let has_pan = key(occurrence, "controls").and_then(|c| key(c, "pvol_pan")).is_some();
            legacy.entry(id).and_modify(|entry| *entry = None).or_insert(has_pan.then_some(occurrence));
        }
    }
    for placement in &sequence.placements {
        let linear = placement.effects.iter().find(|effect| effect.kind == audio_pan::LINEAR_KIND);
        let occurrence = legacy.get(placement.id.as_str()).copied().flatten();
        if linear.is_none() && occurrence.is_none() {
            continue;
        }
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
                || facts.losses.get(&(placement.id.as_str(), "pan_automation"))
                    .into_iter().flatten().any(|&reason| reason != SOURCE_OMISSION)
            {
                return None;
            }
            let (keys, rounded) = match linear {
                Some(effect) => prepare_keys(
                    &audio_pan::read_linear(effect, placement.duration)?, event.source_in,
                ),
                None => legacy_keys(occurrence?, placement, event, *track),
            }?;
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

fn legacy_keys(
    occurrence: &Value, placement: &Placement, event: &Event, track: i64,
) -> Option<(Vec<AudioPanKey>, bool)> {
    if key(occurrence, "id").and_then(str_value) != Some(placement.id.as_str())
        || key(occurrence, "kind").and_then(integer) != Some(2)
        || key(occurrence, "track").and_then(integer) != Some(track)
        || key(occurrence, "combined") != Some(&Value::Null)
        || !matches!(key(occurrence, "via"), Some(Value::Arr(v)) if v.is_empty())
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
    let pan = key(occurrence, "controls").and_then(|v| key(v, "pvol_pan"))?;
    if key(pan, "unqualified").is_some()
        || key(pan, "control_code").and_then(integer) != Some(2000)
        || key(pan, "saved_interp_kind").and_then(integer) != Some(3)
    {
        return None;
    }
    let Value::Arr(points) = key(pan, "keys")? else { return None };
    project_keys(points.iter().map(|point| {
        let num = i128::from(key(point, "frame_numerator").and_then(integer)?);
        let den = i128::from(key(point, "frame_denominator").and_then(integer)?);
        let raw = key(point, "raw_pan").and_then(integer)?;
        let scaled = num * i128::from(frame_ticks);
        if num < 0 || den <= 0 || scaled > i128::from(event.duration) * den
            || !(0..=32_768).contains(&raw)
        {
            return None;
        }
        Some((scaled, den, raw as f64 / 32_768.0))
    }), event.source_in)
}

fn prepare_keys(points: &[PanKey], source_in: i64) -> Option<(Vec<AudioPanKey>, bool)> {
    project_keys(points.iter().map(|point| {
        let scaled = i128::from(point.at.num()) * i128::from(TICKS_PER_SECOND);
        let denominator = i128::from(point.at.den());
        let value = (point.pan.num() as f64 / point.pan.den() as f64 + 1.0) * 0.5;
        Some((scaled, denominator, value))
    }), source_in)
}

fn project_keys(
    points: impl ExactSizeIterator<Item = Option<(i128, i128, f64)>>,
    source_in: i64,
) -> Option<(Vec<AudioPanKey>, bool)> {
    if !(2..=MAX_AUDIO_PAN_KEYS).contains(&points.len()) {
        return None;
    }
    let mut keys: Vec<AudioPanKey> = Vec::with_capacity(points.len());
    let mut rounded = false;
    for point in points {
        let (scaled, denominator, value) = point?;
        rounded |= scaled % denominator != 0;
        let ticks = source_in.checked_add(i64::try_from((scaled + denominator / 2) / denominator).ok()?)?;
        if keys.last().is_some_and(|previous| previous.ticks >= ticks) {
            return None;
        }
        keys.push(AudioPanKey { ticks, value });
    }
    Some((keys, rounded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_edit_local_time_once_and_refuses_colliding_ticks() {
        let left = PanKey { at: Rational::ZERO, pan: Rational::integer(-1) };
        let right = |at| PanKey { at, pan: Rational::integer(1) };
        let start = 2 * TICKS_PER_SECOND;
        let fractional = right(Rational::new(25_641, 2048 * 24).unwrap());
        let (keys, rounded) = prepare_keys(&[left, fractional], start).unwrap();
        assert!(rounded);
        assert_eq!(keys[1], AudioPanKey { ticks: 640_543_886_719, value: 1.0 });
        let too_close = right(Rational::new(1, 1 << 40).unwrap());
        assert!(prepare_keys(&[left, too_close], start).is_none());
    }
}
