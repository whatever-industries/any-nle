//! Place qualified graph track names on the corresponding Premiere timeline lane.
use super::*;
use prproj::convert::prepared::{PreparedTrackLane, PreparedTrackState};

pub(super) const ADAPTER: &str = "avid.track_names";

struct Name<'a> {
    lane: PreparedTrackLane,
    name: &'a str,
    empty: bool,
    ambiguous: bool,
}

fn saved_lane(entry: &Value) -> Option<PreparedTrackLane> {
    let label = key(entry, "track").and_then(integer)?;
    match key(entry, "kind").and_then(integer)? {
        1 => Some(PreparedTrackLane::Picture(label)),
        2 => Some(PreparedTrackLane::Audio(label)),
        _ => None,
    }
}

fn legacy_names<'a>(
    facts: &PreparationFacts<'a>,
    source: &SourceSequence,
    losses: &mut Vec<Loss>,
) -> Vec<Name<'a>> {
    let Some(receipt) = side(&facts.sidecars, ADAPTER, &source.id) else {
        return Vec::new();
    };
    let Value::Arr(entries) = receipt else {
        loss(losses, &source.id, "track_name", "invalid saved track name list");
        return Vec::new();
    };
    if entries.len() > 198 {
        loss(losses, &source.id, "track_name", "saved track name list exceeds the timeline lane limit");
        return Vec::new();
    }
    entries.iter().filter_map(|entry| {
        Some(Name {
            lane: saved_lane(entry)?,
            name: key(entry, "name").and_then(str_value)?,
            empty: key(entry, "plain_empty").and_then(boolean) == Some(true),
            ambiguous: key(entry, "ambiguous").and_then(boolean) != Some(false),
        })
    }).collect()
}

pub(super) fn prepare(
    facts: &PreparationFacts,
    source: &SourceSequence,
    destination: &Sequence,
    enabled: bool,
    losses: &mut Vec<Loss>,
) -> Vec<PreparedTrackState> {
    let mut entries = legacy_names(facts, source, losses);
    entries.extend(source.tracks.iter().filter_map(|track| Some(Name {
        lane: match track.kind {
            StreamKind::Video => PreparedTrackLane::Picture(track.index),
            StreamKind::Audio => PreparedTrackLane::Audio(track.index),
        },
        name: track.name.as_deref()?,
        empty: track.empty,
        ambiguous: false,
    })));
    if entries.is_empty() {
        return Vec::new();
    }
    let mut layouts = HashMap::new();
    if let Some(Value::Arr(saved)) = side(&facts.sidecars, "avid.plain_track_layouts", &source.id) {
        if saved.len() <= 198 {
            for lane in saved.iter().filter_map(saved_lane) {
                *layouts.entry(lane).or_insert(0usize) += 1;
            }
        }
    }
    let mut counts = HashMap::new();
    for entry in &entries {
        *counts.entry(entry.lane).or_insert(0usize) += 1;
    }
    let populated = PreparedTrackLane::populated(destination);
    entries.into_iter().filter_map(|entry| {
        let Name { lane, name, empty, ambiguous } = entry;
        let (prefix, label) = match lane {
            PreparedTrackLane::Picture(label) => ("V", label),
            PreparedTrackLane::Audio(label) => ("A", label),
        };
        let has_placements = populated.contains(&lane);
        let create_empty = !has_placements && (empty || layouts.get(&lane) == Some(&1));
        let omitted = if ambiguous || counts[&lane] != 1 {
            Some("saved track identity or name is ambiguous")
        } else if !enabled {
            Some("this conversion route does not write timeline track names")
        } else if !(1..=99).contains(&label) || (!has_placements && !create_empty) {
            Some("the named track has no supported placement or verified empty layout")
        } else if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
            Some("the saved track name is empty, too long, or contains a control character")
        } else {
            None
        };
        if let Some(reason) = omitted {
            loss(losses, &source.id, "track_name", format!("{prefix}{label}: {reason}"));
            return None;
        }
        Some(PreparedTrackState {
            lane,
            name: Some(name.into()),
            locked: false,
            picture_disabled: false,
            create_empty,
        })
    }).collect()
}
