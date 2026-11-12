//! Narrow Resolve-to-Resolve Fairlight preparation for the turnover archive.
use crate::{
    source::mixer::{valid_ticks, DIRECT_PROFILE, FAIRLIGHT_ADAPTER, FAIRLIGHT_LOSS},
    Result,
};
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirectMixer {
    pub(crate) buses: i64,
    pub(crate) pan: [i32; 2],
    pub(crate) output: [i32; 2],
    pub(crate) bus_2_gain: i32,
    pub(crate) bus_2_muted: bool,
}

impl DirectMixer {
    pub(crate) fn recover(loss: &Loss, owner: &str) -> bool {
        loss.object == owner && loss.aspect == FAIRLIGHT_LOSS.0 && loss.reason == FAIRLIGHT_LOSS.1
    }

    pub(crate) fn prepare(graph: &SourceGraph, sequence: &SourceSequence) -> Result<Option<Self>> {
        let Some(data) = graph.sidecar(FAIRLIGHT_ADAPTER, &sequence.id) else {
            return Ok(None);
        };
        if data.get(DIRECT_PROFILE).is_none() {
            // Unqualified raw observations retain their original source loss.
            return Ok(None);
        }
        if !sequence
            .clock
            .as_ref()
            .is_some_and(|clock| clock.provenance.route == "Resolve archive clock")
        {
            return Err("direct Fairlight controls require a native Resolve clock".into());
        }
        let mixer = Self::from_receipt(data)?;
        if mixer.buses == 1 && mixer.pan == [0; 2] {
            // Keep neutral legacy output unchanged.
            return Ok(None);
        }
        Ok(Some(mixer))
    }

    fn from_receipt(data: &Value) -> Result<Self> {
        let buses = number(data, DIRECT_PROFILE)?;
        if data.get("plugins").is_some() {
            return Err("direct Fairlight profile does not map mixer plug-ins".into());
        }
        let Some(Value::Arr(outputs)) = data.get("buses") else {
            return Err("direct Fairlight mixer is missing its persisted outputs".into());
        };
        if !matches!(buses, 1 | 2) || outputs.len() as i64 != buses {
            return Err(
                "direct Fairlight mixer requires one or two persisted stereo outputs".into(),
            );
        }
        let mut bus_2_gain = 0;
        let mut bus_2_muted = false;
        for (index, bus) in (0i64..).zip(outputs) {
            if number(bus, "index")? != index
                || number(bus, "strip_id")? != 3 + index
                || number(bus, "channels")? != 2
                || number(bus, "output_connected_code")? != 0
                || number(bus, "output_bus_index")? != -1
                || !neutral_when_present(
                    bus,
                    &[
                        "raw_pan",
                        "raw_pan_bypass",
                        "raw_solo",
                        "pre_dynamics_gain_tenths_db",
                        "effects_in_code",
                    ],
                )
            {
                return Err("unsupported or ambiguous direct Fairlight stereo bus".into());
            }
            let gain = number(bus, "fader_tenths_db")?;
            let mute = number(bus, "mute_code")?;
            if index == 0 {
                if gain != 0 || mute != 0 {
                    return Err(
                        "direct Fairlight stereo Bus 1 requires neutral gain and mute".into(),
                    );
                }
            } else {
                bus_2_gain = i32::try_from(gain)
                    .ok()
                    .filter(|&ticks| valid_ticks(ticks))
                    .ok_or("unsupported direct Fairlight Bus 2 gain")?;
                if !matches!(mute, 0 | 1) {
                    return Err("unsupported direct Fairlight Bus 2 mute code".into());
                }
                bus_2_muted = mute == 1;
            }
        }
        if data.get("main_bus") != outputs.first() {
            return Err("direct Fairlight main bus receipt disagrees with bus identity".into());
        }
        let Some(Value::Arr(tracks)) = data.get("tracks") else {
            return Err("direct Fairlight mixer is missing its two mono tracks".into());
        };
        if tracks.len() != 2 {
            return Err("direct Fairlight mixer requires two independent mono tracks".into());
        }
        let mut pan = [0; 2];
        let mut output = [0; 2];
        for (column, row) in tracks.iter().enumerate() {
            let position = number(row, "pan_tenths_percent")?;
            let bus = number(row, "output_bus_index")?;
            if number(row, "track")? != column as i64 + 1
                || number(row, "strip_id")? != column as i64
                || number(row, "channel_index")? != 0
                || number(row, "pan_bypass_code")? != 0
                || number(row, "output_connected_code")? != 1
                || number(row, "fader_tenths_db")? != 0
                || number(row, "mute_code")? != 0
                || !neutral_when_present(
                    row,
                    &[
                        "solo_code",
                        "pre_dynamics_gain_tenths_db",
                        "effects_in_code",
                    ],
                )
                || !(-1000..=1000).contains(&position)
                || !(0..buses).contains(&bus)
            {
                return Err("unsupported direct Fairlight mono pan or output assignment".into());
            }
            pan[column] = position as i32;
            output[column] = bus as i32;
        }
        Ok(Self {
            buses,
            pan,
            output,
            bus_2_gain,
            bus_2_muted,
        })
    }
}

fn number(row: &Value, name: &str) -> Result<i64> {
    match row.get(name) {
        Some(Value::Int(value)) => Ok(*value),
        _ => Err(format!("direct Fairlight receipt is missing integer {name}").into()),
    }
}

fn neutral_when_present(row: &Value, names: &[&str]) -> bool {
    names
        .iter()
        .all(|name| matches!(row.get(name), None | Some(Value::Int(0))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value as Json};

    fn receipt() -> Json {
        let bus = |index| {
            json!({
                "index": index, "strip_id": 3 + index, "channels": 2,
                "output_connected_code": 0, "output_bus_index": -1,
                "fader_tenths_db": 0, "mute_code": 0
            })
        };
        let track = |strip, pan, output| {
            json!({
                "track": 1 + strip, "strip_id": strip, "channel_index": 0,
                "pan_tenths_percent": pan, "pan_bypass_code": 0, "output_connected_code": 1,
                "output_bus_index": output, "fader_tenths_db": 0, "mute_code": 0
            })
        };
        json!({
            (DIRECT_PROFILE): 2, "buses": [bus(0), bus(1)], "main_bus": bus(0),
            "tracks": [track(0, -1000, 0), track(1, 500, 1)]
        })
    }

    fn prepare(value: Json) -> Result<DirectMixer> {
        DirectMixer::from_receipt(&graph::json::parse(&value.to_string())?)
    }

    #[test]
    fn preserves_signed_pan_and_independent_direct_output_by_strip() {
        let prepared = prepare(receipt()).unwrap();
        assert_eq!(prepared.pan, [-1000, 500]);
        assert_eq!(prepared.output, [0, 1]);
    }

    #[test]
    fn refuses_ambiguous_missing_or_disconnected_outputs_and_unmapped_processing() {
        for (pointer, change) in [
            ("/buses/1/strip_id", json!(3)),
            ("/buses/1/channels", json!(1)),
            ("/buses/1/output_connected_code", json!(1)),
            ("/buses/0/fader_tenths_db", json!(-60)),
            ("/buses/0/mute_code", json!(1)),
            ("/buses/1/fader_tenths_db", json!(-61)),
            ("/buses/1/mute_code", json!(2)),
            ("/main_bus/index", json!(1)),
            ("/tracks/1/channel_index", json!(1)),
            ("/tracks/1/output_bus_index", json!(2)),
            ("/tracks/1/output_connected_code", json!(0)),
            ("/tracks/1/pan_bypass_code", json!(1)),
            ("/tracks/1/pan_tenths_percent", json!(1001)),
        ] {
            let mut value = receipt();
            *value.pointer_mut(pointer).unwrap() = change;
            assert!(prepare(value).is_err(), "accepted {pointer}");
        }
        let mut value = receipt();
        value["plugins"] = json!([{"track":1,"slot":6}]);
        assert!(prepare(value).is_err());
        let mut value = receipt();
        value["buses"].as_array_mut().unwrap().pop();
        assert!(prepare(value).is_err());
    }

    #[test]
    fn preserves_bus_2_gain_and_mute_independently_of_pan_and_routing() {
        for (gain, mute) in [(-60, 0), (0, 1), (-37, 1)] {
            let mut value = receipt();
            value["buses"][1]["fader_tenths_db"] = json!(gain);
            value["buses"][1]["mute_code"] = json!(mute);
            let prepared = prepare(value).unwrap();
            assert_eq!(
                (prepared.bus_2_gain, prepared.bus_2_muted),
                (gain, mute == 1)
            );
            assert_eq!(prepared.pan, [-1000, 500]);
            assert_eq!(prepared.output, [0, 1]);
        }
    }
}
