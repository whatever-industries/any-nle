//! Avid copies the source gain into a newly created edit on the qualified mono WAV
//! route. An older dry cut of the same master must stay dry.
use super::*;

pub(super) struct SourceGain<'a> {
    events: HashMap<&'a str, Option<&'a Value>>,
    masters: &'a HashMap<&'a str, i64>,
    sidecars: &'a Sidecars<'a>,
}

impl<'a> SourceGain<'a> {
    pub(super) fn new(
        facts: &'a PreparationFacts<'a>,
        seq: &SourceSequence,
        frame_size: (i64, i64),
        masters: &'a HashMap<&'a str, i64>,
    ) -> Self {
        let mut events = HashMap::new();
        if !masters.is_empty() && seq.rate == Rational::integer(24) && frame_size == (1920, 1080) {
            if let Some(Value::Arr(rows)) = side(&facts.sidecars, "avid.source", &seq.id)
                .and_then(|v| key(v, "native_events"))
            {
                for row in rows {
                    if let Some(id) = key(row, "placement_id").and_then(str_value) {
                        events.entry(id).and_modify(|value| *value = None).or_insert(Some(row));
                    }
                }
            }
        }
        Self { events, masters, sidecars: &facts.sidecars }
    }

    pub(super) fn is_dry(&self, p: &Placement) -> bool {
        let (Some(&lane), [track], Some(Some(event))) = (
            self.masters.get(p.item.strip_prefix("clip:").unwrap_or(&p.item)),
            p.audio_tracks.as_slice(),
            self.events.get(p.id.as_str()),
        ) else {
            return false;
        };
        let Some(mob) = side(self.sidecars, "avid", &p.item)
            .and_then(|v| key(v, "mob_id")).and_then(str_value)
        else {
            return false;
        };
        p.video_track.is_none() && p.grouped_audio.is_empty() && p.audio_streams == [lane]
            && p.track_gains_db.iter().all(Option::is_none)
            && p.gain_db == 0.0 && p.volume_db.is_none() && p.volume_keys.is_empty()
            && p.effects.is_empty() && p.speed.is_none() && p.frame_map.is_none()
            && side(self.sidecars, "avid.motion_adapter", &p.id).is_none()
            && dry_receipt(event, p, mob, *track, lane)
    }
}

fn dry_receipt(event: &Value, p: &Placement, mob: &str, track: i64, lane: i64) -> bool {
    let neutral_pan = match (p.track_pans.as_slice(), key(event, "edit_pan")) {
        ([] | [None], Some(Value::Null)) => true,
        ([Some(pan)], Some(Value::Float(saved))) => *pan == 0.0 && *saved == *pan,
        ([Some(pan)], Some(Value::Int(0))) => *pan == 0.0,
        _ => false,
    };
    if key(event, "id").and_then(str_value) != Some(p.id.as_str())
        || key(event, "master").and_then(str_value) != Some(mob)
        || key(event, "kind").and_then(integer) != Some(2)
        || key(event, "track").and_then(integer) != Some(track)
        || key(event, "master_track").and_then(integer) != Some(lane)
        || !neutral_pan
        || ["combined", "edit_gain_db", "reason"].into_iter()
            .any(|field| key(event, field) != Some(&Value::Null))
        || !matches!(key(event, "via"), Some(Value::Arr(v)) if v.is_empty())
        || !matches!(key(event, "controls"), Some(Value::Obj(v)) if !v.is_empty()
            && v.iter().all(|(_, value)| *value == Value::Null))
    {
        return false;
    }
    let Some(range) = key(event, "range") else { return false };
    if key(range, "record_rate").and_then(str_value) != Some("24")
        || key(range, "source_rate").and_then(str_value) != Some("24")
        || ![("record_start", p.record_start), ("source_in", p.source_in),
            ("record_length", p.duration), ("source_length", p.duration)]
            .into_iter().all(|(field, time)| time.to_units(24).is_some_and(|frames|
                key(range, field).and_then(integer) == Some(frames)))
    {
        return false;
    }
    matches!(key(range, "route"), Some(Value::Arr(routes)) if matches!(routes.as_slice(), [route]
        if key(route, "mob").and_then(str_value) == Some(mob)
        && key(route, "track").and_then(integer) == Some(lane)
        && key(route, "rate").and_then(str_value) == Some("24")
        && p.source_in.to_units(24).is_some_and(|frames|
            key(route, "source_offset").and_then(|v| key(v, "value")).and_then(integer) == Some(frames))
        && p.duration.to_units(24).is_some_and(|frames|
            key(route, "length").and_then(|v| key(v, "value")).and_then(integer) == Some(frames))))
}
