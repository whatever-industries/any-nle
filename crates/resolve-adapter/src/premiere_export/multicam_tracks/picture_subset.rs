//! Keep existing picture choices when the archive proves which other angles
//! are missing from the portable Group.
use super::{Multicam, MulticamUse, SourceGraph, StreamKind, Value};
use crate::Result;
use graph::{source::MulticamLane, Loss};
use std::collections::{HashMap, HashSet};

pub(super) fn omitted_angles(
    graph: &SourceGraph,
    group: &Multicam,
    receipt: &Value,
    picture: Option<&MulticamLane>,
    uses: &[&MulticamUse],
) -> Result<Vec<Loss>> {
    let invalid = || {
        format!(
            "{}: incomplete or ambiguous saved Resolve picture angles",
            group.id
        )
    };
    let picture = picture.ok_or_else(invalid)?;
    let Some(Value::Arr(tracks)) = receipt.get("tracks") else {
        return Err(invalid().into());
    };
    let mut by_alternative = HashMap::new();
    let mut angle_ids = HashSet::new();
    let mut saved = Vec::new();
    for track in tracks {
        let native_kind = field(track, "kind").ok_or_else(invalid)?;
        if native_kind == "audio" {
            continue;
        }
        if native_kind != "video" {
            return Err(invalid().into());
        }
        let id = field(track, "track_id").ok_or_else(invalid)?;
        let angle = field(track, "angle_id").ok_or_else(invalid)?;
        let alternative = format!("{}/{id}/video/1", group.id);
        if !angle_ids.insert(angle)
            || by_alternative
                .insert(alternative.clone(), (id, angle))
                .is_some()
        {
            return Err(invalid().into());
        }
        saved.push((alternative, angle));
    }
    let kept = picture
        .alternatives
        .iter()
        .map(|alternative| {
            let entry = by_alternative.remove(&alternative.id).ok_or_else(invalid)?;
            if alternative.reason.is_some() {
                return Err(invalid());
            }
            Ok(entry)
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if kept.is_empty()
        || by_alternative.is_empty()
        || !uses.iter().any(|edit| edit.kind == StreamKind::Video)
    {
        return Err(invalid().into());
    }
    for edit in uses.iter().filter(|edit| edit.kind == StreamKind::Video) {
        let selected = edit
            .selected
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| kept.get(index));
        let saved = graph.sidecar(crate::source::ADAPTER, &edit.id);
        if edit.lane != Some(picture.index)
            || selected.is_none_or(|&(track, angle)| {
                saved.and_then(|row| field(row, "saved_angle_track_id")) != Some(track)
                    || saved.and_then(|row| field(row, "angle_id")) != Some(angle)
            })
        {
            return Err(format!(
                "{}: saved Resolve picture angle contradicts its binding",
                edit.id
            )
            .into());
        }
    }
    Ok(saved.into_iter().filter_map(|(alternative, angle)| {
        by_alternative.contains_key(&alternative).then(|| Loss {
            object: alternative,
            aspect: "multicam_angle_omitted".into(),
            reason: format!("Resolve picture angle {angle} omitted from the editable Group because its source binding is unqualified. All saved selections of this Group on the timeline are retained."),
        })
    }).collect())
}

fn field<'a>(row: &'a Value, name: &str) -> Option<&'a str> {
    match row.get(name)? {
        Value::Str(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}
