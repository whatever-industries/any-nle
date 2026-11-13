//! Compare static saved controls separately from clip volume and wider Fairlight state.
mod processing;
mod routing;

use super::{known, Diff, Timeline};
use crate::{
    source::mixer::{
        fader_ticks, parse_track_mutes, FAIRLIGHT_ADAPTER, MAIN_BUS_LEVEL, MAIN_BUS_LOSS,
        MUTED_AUDIO_TRACKS, TRACK_MUTE_LOSS,
    },
    track_state,
};
use graph::{Loss, StreamKind, Value};
use serde_json::{json, Value as Json};

pub(super) fn compare(
    left: &Timeline<'_>,
    right: &Timeline<'_>,
    scope: &Json,
    diff: &mut Diff,
) -> bool {
    if !in_scope(left) || !in_scope(right) {
        return false;
    }
    let a = SavedMixer::read(left);
    let b = SavedMixer::read(right);
    for (slot, track) in [1, 2].into_iter().enumerate() {
        let lane = json!({"kind":"audio", "track":track});
        known(
            diff,
            "track_volume_db",
            &lane,
            a.track_db[slot].map(db),
            b.track_db[slot].map(db),
        );
        known(
            diff,
            "track_mute",
            &lane,
            a.mute[slot].map(|v| json!(v)),
            b.mute[slot].map(|v| json!(v)),
        );
    }
    known(
        diff,
        "main_bus_volume_db",
        scope,
        a.main_db.map(db),
        b.main_db.map(db),
    );
    let (a_buses, b_buses) = routing::compare(left, right, scope, diff);
    processing::compare(left, right, a_buses, b_buses, diff);
    true
}

pub(super) fn compared_loss(t: &Timeline<'_>, loss: &Loss) -> bool {
    loss.object == t.seq.id
        && matches!(
            (loss.aspect.as_str(), loss.reason.as_str()),
            MAIN_BUS_LOSS | TRACK_MUTE_LOSS
        )
}

fn in_scope(t: &Timeline<'_>) -> bool {
    t.seq
        .tracks
        .iter()
        .map(|t| (t.kind, t.index))
        .eq(track_state::LANES)
        && !t.losses.iter().any(|loss| {
            loss.object == t.seq.id && matches!(loss.aspect.as_str(), "track" | "tracks")
        })
}

#[derive(Default)]
struct SavedMixer {
    track_db: [Option<i32>; 2],
    mute: [Option<bool>; 2],
    main_db: Option<i32>,
}

impl SavedMixer {
    fn read(t: &Timeline<'_>) -> Self {
        let facts = t.graph.sidecar(FAIRLIGHT_ADAPTER, &t.seq.id);
        let raw_tracks = [1, 2].map(|track| mono_row(facts, track));
        let raw_main = facts
            .and_then(|v| v.get("main_bus"))
            .filter(|row| row.get("index") == Some(&Value::Int(0)));
        let unresolved = |aspect| {
            t.losses.iter().any(|loss| {
                loss.object == t.seq.id && loss.aspect == aspect && !compared_loss(t, loss)
            })
        };
        let mut result = Self::default();
        if !unresolved("track_volume") {
            for (slot, lane) in t
                .seq
                .tracks
                .iter()
                .filter(|t| t.kind == StreamKind::Audio)
                .enumerate()
            {
                result.track_db[slot] = match lane.level_db {
                    Some(value) => fader_ticks(value),
                    None => observed_zero(raw_tracks[slot], "fader_tenths_db"),
                };
            }
        }
        if !unresolved(MAIN_BUS_LOSS.0) {
            result.main_db = match t.receipt(&t.seq.id, MAIN_BUS_LEVEL) {
                Some(Value::Float(value)) => fader_ticks(*value),
                Some(Value::Int(value)) => fader_ticks(*value as f64),
                None => observed_zero(raw_main, "fader_tenths_db"),
                _ => None,
            };
        }
        if !unresolved(TRACK_MUTE_LOSS.0) {
            result.mute = match t.receipt(&t.seq.id, MUTED_AUDIO_TRACKS) {
                Some(value) => parse_track_mutes(value)
                    .map(|mutes| mutes.map(Some))
                    .unwrap_or_default(),
                // An absent nonneutral receipt is only evidence of an unmuted
                // lane when its saved mixer row confirms zero and the reader
                // did not report a conflict with the timeline flags.
                None => raw_tracks.map(|row| observed_zero(row, "mute_code").map(|_| false)),
            };
        }
        result
    }
}

fn mono_row(facts: Option<&Value>, track: i64) -> Option<&Value> {
    let Value::Arr(rows) = facts?.get("tracks")? else {
        return None;
    };
    let mut rows = rows
        .iter()
        .filter(|row| row.get("track") == Some(&Value::Int(track)));
    let row = rows.next()?;
    (row.get("channel_index") == Some(&Value::Int(0)) && rows.next().is_none()).then_some(row)
}

fn observed_zero(row: Option<&Value>, field: &str) -> Option<i32> {
    (row?.get(field) == Some(&Value::Int(0))).then_some(0)
}

fn db(ticks: i32) -> Json {
    json!(f64::from(ticks) / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{mixer::DIRECT_PROFILE, ADAPTER};
    use graph::{
        source::{SourceGraph, SourceSequence},
        Project, Rational, Sidecar, Track,
    };

    const ID: &str = "00000000-0000-0000-0000-000000000001";

    fn fixture() -> SourceGraph {
        SourceGraph {
            project: Project {
                name: "Test".into(),
            },
            bins: vec![],
            media: vec![],
            items: vec![],
            sequences: vec![SourceSequence {
                id: ID.into(),
                name: "Test".into(),
                rate: Rational::new(24, 1).unwrap(),
                frame_size: None,
                start: None,
                clock: None,
                tracks: track_state::LANES
                    .map(|(kind, index)| Track {
                        kind,
                        index,
                        level_db: None,
                    })
                    .into(),
                placements: vec![],
                transitions: vec![],
                markers: vec![],
                texts: vec![],
            }],
            losses: vec![],
            sidecars: vec![Sidecar {
                adapter: FAIRLIGHT_ADAPTER.into(),
                object: ID.into(),
                data: graph::json::parse(
                    r#"{
                    "tracks":[
                        {"track":2,"strip_id":1,"channel_index":0,"fader_tenths_db":0,"mute_code":0,
                         "pan_tenths_percent":0,"pan_bypass_code":0,"output_connected_code":1,"output_bus_index":0},
                        {"track":1,"strip_id":0,"channel_index":0,"fader_tenths_db":0,"mute_code":0,
                         "pan_tenths_percent":0,"pan_bypass_code":0,"output_connected_code":1,"output_bus_index":0}
                    ],
                    "main_bus":{"index":0,"strip_id":3,"channels":2,"output_connected_code":0,"output_bus_index":-1,"fader_tenths_db":0},
                    "buses":[{"index":0,"strip_id":3,"channels":2,"output_connected_code":0,"output_bus_index":-1,"fader_tenths_db":0}]
                }"#,
                )
                .unwrap(),
            }],
            multicams: vec![],
            multicam_uses: vec![],
        }
    }

    fn compare_graphs(left: &SourceGraph, right: &SourceGraph) -> Diff {
        let mut diff = Diff::default();
        assert!(compare(
            &Timeline::new(left, ID).unwrap(),
            &Timeline::new(right, ID).unwrap(),
            &Json::Null,
            &mut diff,
        ));
        diff
    }

    #[test]
    fn track_gain_mute_and_main_bus_compare_independently() {
        let left = fixture();
        let mut right = left.clone();
        right.sequences[0].tracks[1].level_db = Some(-3.0);
        right.sidecars.push(Sidecar {
            adapter: ADAPTER.into(),
            object: ID.into(),
            data: graph::json::parse(r#"{"muted_audio_tracks":[2],"main_bus_level_db":-3.7}"#)
                .unwrap(),
        });
        let diff = compare_graphs(&left, &right);
        assert_eq!(
            diff.differences,
            [
                json!({"field":"track_volume_db","context":{"kind":"audio","track":1},"left":0.0,"right":-3.0}),
                json!({"field":"track_mute","context":{"kind":"audio","track":2},"left":false,"right":true}),
                json!({"field":"main_bus_volume_db","context":null,"left":0.0,"right":-3.7}),
            ]
        );
        assert!(diff.gaps.is_empty());

        right.losses.push(Loss {
            object: ID.into(),
            aspect: TRACK_MUTE_LOSS.0.into(),
            reason: "Conflicting saved mixer state".into(),
        });
        let diff = compare_graphs(&left, &right);
        assert_eq!(diff.differences.len(), 2);
        assert_eq!(diff.gaps.len(), 2);
        assert!(diff.gaps.iter().all(|row| row["field"] == "track_mute"));
    }

    #[test]
    fn missing_mixer_does_not_prove_neutral_controls() {
        let left = fixture();
        let mut right = left.clone();
        right.sidecars.clear();
        let diff = compare_graphs(&left, &right);
        assert!(diff.differences.is_empty());
        assert_eq!(diff.gaps.len(), 12);
        assert!(diff
            .gaps
            .iter()
            .all(|row| row["reason"] == "unresolved on the right"));
    }

    #[test]
    fn routing_pan_and_secondary_bus_controls_are_separate() {
        let left = fixture();
        let mut right = left.clone();
        right.sidecars[0].data = graph::json::parse(
            r#"{
                "direct_mono_stereo_profile":2,
                "tracks":[
                    {"track":1,"strip_id":0,"channel_index":0,"fader_tenths_db":0,"mute_code":0,
                     "pan_tenths_percent":-500,"pan_bypass_code":0,"output_connected_code":1,"output_bus_index":0},
                    {"track":2,"strip_id":1,"channel_index":0,"fader_tenths_db":0,"mute_code":0,
                     "pan_tenths_percent":0,"pan_bypass_code":0,"output_connected_code":1,"output_bus_index":1}
                ],
                "main_bus":{"index":0,"strip_id":3,"channels":2,"output_connected_code":0,"output_bus_index":-1,"fader_tenths_db":0},
                "buses":[
                    {"index":0,"strip_id":3,"channels":2,"output_connected_code":0,"output_bus_index":-1,"fader_tenths_db":0},
                    {"index":1,"strip_id":4,"channels":2,"output_connected_code":0,"output_bus_index":-1,"fader_tenths_db":-60,"mute_code":1}
                ]
            }"#,
        )
        .unwrap();
        let diff = compare_graphs(&left, &right);
        assert_eq!(
            diff.differences,
            [
                json!({"field":"stereo_bus_count","context":null,"left":1,"right":2}),
                json!({"field":"track_pan_percent","context":{"kind":"audio","track":1},"left":0.0,"right":-50.0}),
                json!({"field":"track_output_bus","context":{"kind":"audio","track":2},"left":1,"right":2}),
            ]
        );
        assert!(diff.gaps.is_empty());

        let mut neutral = right.clone();
        let Value::Obj(fields) = &mut neutral.sidecars[0].data else {
            unreachable!()
        };
        let Value::Arr(buses) = &mut fields.iter_mut().find(|(key, _)| key == "buses").unwrap().1
        else {
            unreachable!()
        };
        let Value::Obj(bus2) = &mut buses[1] else {
            unreachable!()
        };
        for (key, value) in bus2 {
            if matches!(key.as_str(), "fader_tenths_db" | "mute_code") {
                *value = Value::Int(0);
            }
        }
        let diff = compare_graphs(&neutral, &right);
        assert_eq!(
            diff.differences,
            [
                json!({"field":"bus_volume_db","context":{"bus":2},"left":0.0,"right":-6.0}),
                json!({"field":"bus_mute","context":{"bus":2},"left":false,"right":true}),
            ]
        );
        assert!(diff.gaps.is_empty());

        let Value::Obj(fields) = &mut right.sidecars[0].data else {
            unreachable!()
        };
        fields.retain(|(key, _)| key != DIRECT_PROFILE);
        let diff = compare_graphs(&neutral, &right);
        assert!(diff.differences.is_empty());
        for field in [
            "track_pan_percent",
            "track_output_bus",
            "bus_volume_db",
            "bus_mute",
        ] {
            assert!(diff.gaps.iter().any(|row| row["field"] == field), "{field}");
        }
    }
}
