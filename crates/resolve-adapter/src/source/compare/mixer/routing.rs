//! Saved pan and direct stereo bus assignments for two mono lanes.
use super::{db, known, mono_row, Diff, Timeline};
use crate::source::mixer::{valid_ticks, DIRECT_PROFILE, FAIRLIGHT_ADAPTER, LINKED_TRACK_PANS};
use graph::Value;
use serde_json::{json, Value as Json};

pub(super) fn compare(
    left: &Timeline<'_>,
    right: &Timeline<'_>,
    scope: &Json,
    diff: &mut Diff,
) -> (Option<usize>, Option<usize>) {
    let a = Routing::read(left);
    let b = Routing::read(right);
    known(
        diff,
        "stereo_bus_count",
        scope,
        a.buses.map(|n| json!(n)),
        b.buses.map(|n| json!(n)),
    );
    for slot in 0..2 {
        let lane = json!({"kind":"audio", "track":slot + 1});
        known(
            diff,
            "track_pan_percent",
            &lane,
            a.pan[slot].map(percent),
            b.pan[slot].map(percent),
        );
        known(
            diff,
            "track_pan_bypassed",
            &lane,
            a.bypass[slot].map(|v| json!(v)),
            b.bypass[slot].map(|v| json!(v)),
        );
        known(
            diff,
            "track_output_bus",
            &lane,
            a.output[slot].map(|n| json!(n)),
            b.output[slot].map(|n| json!(n)),
        );
    }
    // The bus-count row already reports creation/removal. Gain and mute
    // need two known Bus 2 owners before we can compare their settings.
    if (a.buses, b.buses) == (Some(2), Some(2)) {
        let bus = json!({"bus":2});
        known(
            diff,
            "bus_volume_db",
            &bus,
            a.bus_2_gain.map(db),
            b.bus_2_gain.map(db),
        );
        known(
            diff,
            "bus_mute",
            &bus,
            a.bus_2_mute.map(|v| json!(v)),
            b.bus_2_mute.map(|v| json!(v)),
        );
    }
    (a.buses, b.buses)
}

#[derive(Default)]
struct Routing {
    buses: Option<usize>,
    pan: [Option<i32>; 2],
    bypass: [Option<bool>; 2],
    output: [Option<i64>; 2],
    bus_2_gain: Option<i32>,
    bus_2_mute: Option<bool>,
}

impl Routing {
    fn read(timeline: &Timeline<'_>) -> Self {
        let mut result = Self::default();
        let Some(data) = timeline.graph.sidecar(FAIRLIGHT_ADAPTER, &timeline.seq.id) else {
            return result;
        };
        let Some(Value::Arr(buses)) = data.get("buses") else {
            return result;
        };
        let direct = match data.get(DIRECT_PROFILE) {
            Some(Value::Int(1)) if buses.len() == 1 => true,
            Some(Value::Int(2)) if buses.len() == 2 => true,
            None => false,
            _ => return result,
        };
        if !(1..=2).contains(&buses.len())
            || data.get("main_bus") != buses.first()
            || !buses.iter().enumerate().all(|(i, bus)| {
                let i = i as i64;
                number(bus, "index") == Some(i)
                    && number(bus, "strip_id") == Some(i + 3)
                    && number(bus, "channels") == Some(2)
                    && number(bus, "output_connected_code") == Some(0)
                    && number(bus, "output_bus_index") == Some(-1)
            })
        {
            return result;
        }
        result.buses = Some(buses.len());
        if let [_, bus] = buses.as_slice() {
            // A primary raw mute cell alone does not establish the mirrored
            // state. Direct profile qualification also verifies both copies.
            if direct {
                result.bus_2_gain = number(bus, "fader_tenths_db")
                    .and_then(|n| i32::try_from(n).ok())
                    .filter(|&n| valid_ticks(n));
                result.bus_2_mute = switch(number(bus, "mute_code"));
            }
        }
        let linked = match data.get(LINKED_TRACK_PANS) {
            Some(Value::Arr(pans)) => match pans.as_slice() {
                [Value::Int(a), Value::Int(b)] if buses.len() == 1 => Some([*a, *b]),
                _ => return result,
            },
            None => None,
            _ => return result,
        };
        for (slot, track) in [1, 2].into_iter().enumerate() {
            let Some(row) = mono_row(Some(data), track) else {
                continue;
            };
            if number(row, "strip_id") != Some(track - 1) {
                continue;
            }
            let output = number(row, "output_bus_index");
            if number(row, "output_connected_code") != Some(1)
                || !output.is_some_and(|n| (0..buses.len() as i64).contains(&n))
            {
                continue;
            }
            let bypass = number(row, "pan_bypass_code");
            let pan = number(row, "pan_tenths_percent");
            if let Some(pans) = linked {
                if pan != Some(pans[slot]) || bypass != Some(0) || output != Some(0) {
                    continue;
                }
            }
            // A raw row can witness neutral settings. Nonneutral values
            // need a qualified direct or linked profile.
            if direct || output == Some(0) {
                result.output[slot] = output.map(|n| n + 1);
            }
            result.bypass[slot] = match bypass {
                Some(1) if direct && buses.len() == 1 => Some(true),
                Some(0) => Some(false),
                _ => None,
            };
            if result.bypass[slot].is_some() && (direct || linked.is_some() || pan == Some(0)) {
                result.pan[slot] = pan.filter(|n| (-1000..=1000).contains(n)).map(|n| n as i32);
            }
        }
        result
    }
}

fn number(row: &Value, key: &str) -> Option<i64> {
    match row.get(key) {
        Some(Value::Int(n)) => Some(*n),
        _ => None,
    }
}

fn switch(raw: Option<i64>) -> Option<bool> {
    match raw {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

fn percent(tenths: i32) -> Json {
    json!(f64::from(tenths) / 10.0)
}
