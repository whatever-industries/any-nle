//! Qualified direct-mono Resolve clip envelopes.
use crate::{
    binary::{pack, pb_bytes, pb_int},
    prepare::{frames, Edit},
    Result,
};
use graph::{Placement, Value};

pub(crate) const FADE_LOSS: (&str, &str) = (
    "clip_fades",
    "Resolve clip fades are retained in source metadata",
);
pub(crate) const KEY_LOSS: (&str, &str) = (
    "volume_interpolation",
    "Resolve volume curve law is retained in source metadata",
);
pub(crate) const EFFECT: u64 = 124;
pub(crate) const VOLUME: u64 = 95;
pub(crate) const HEAD: u64 = 97;
pub(crate) const TAIL: u64 = 98;
pub(crate) const KEY_FLAGS: u32 = 0x000c_0000;
pub(crate) const VOLUME_DB: std::ops::RangeInclusive<f64> = -6.0..=3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Envelope {
    StaticVolume(f64),
    Fades { head: i64, tail: i64 },
    VolumeKeys([(u32, f64); 2]),
}

pub(crate) fn prepare(p: &Placement, edit: Edit, data: Option<&Value>) -> Result<Option<Envelope>> {
    let unsupported = || format!("unsupported Resolve clip envelope at {}", p.id);
    if let Some(db) = p.volume_db {
        return if data.is_none() && p.volume_keys.is_empty() && VOLUME_DB.contains(&db) {
            Ok(Some(Envelope::StaticVolume(db)))
        } else {
            Err(unsupported().into())
        };
    }
    let Some(data) = data else {
        return if p.volume_keys.is_empty() {
            Ok(None)
        } else {
            Err("Resolve volume keys require the qualified source curve receipt".into())
        };
    };
    let Value::Obj(fields) = data else {
        return Err(unsupported().into());
    };
    let get = |key| data.get(key);
    let is_text =
        |key, expected: &str| matches!(get(key), Some(Value::Str(value)) if value == expected);
    if p.volume_keys.is_empty() {
        let (Some(Value::Int(head)), Some(Value::Int(tail))) =
            (get("fade_in_frames"), get("fade_out_frames"))
        else {
            return Err(unsupported().into());
        };
        if fields.len() == 4
            && is_text("fade_clock", "clip_edges")
            && is_text("fade_shape", "linear_amplitude")
            && *head >= 0
            && *tail >= 0
            && head
                .checked_add(*tail)
                .is_some_and(|n| n > 0 && n <= edit.length)
        {
            return Ok(Some(Envelope::Fades {
                head: *head,
                tail: *tail,
            }));
        }
    } else if fields.len() == 3
        && is_text("volume_key_clock", "item_source")
        && is_text("volume_interpolation", "linear_db")
        && is_text("volume_extrapolation", "hold")
    {
        if let [first, last] = p.volume_keys.as_slice() {
            let points = [frames(first.at)?, frames(last.at)?];
            if points[0] >= edit.source_in
                && points[0] < points[1]
                && points[1] < edit.source_in + edit.length
                && VOLUME_DB.contains(&first.db)
                && VOLUME_DB.contains(&last.db)
            {
                return Ok(Some(Envelope::VolumeKeys([
                    (u32::try_from(points[0])?, first.db),
                    (u32::try_from(points[1])?, last.db),
                ])));
            }
        }
    }
    Err(unsupported().into())
}

impl Envelope {
    pub(crate) fn recovered_loss(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::StaticVolume(_) => None,
            Self::Fades { .. } => Some(FADE_LOSS),
            Self::VolumeKeys(_) => Some(KEY_LOSS),
        }
    }

    pub(crate) fn encode(self) -> Result<String> {
        let mut section = Vec::new();
        pb_int(1, EFFECT, &mut section);
        pb_int(7, 0, &mut section);
        match self {
            Self::StaticVolume(db) => {
                pb_bytes(9, &scalar(VOLUME, db), &mut section);
                pb_bytes(9, &[], &mut section);
                pb_bytes(9, &[], &mut section);
            }
            Self::Fades { head, tail } => {
                pb_bytes(9, &[], &mut section);
                for (id, count) in [(HEAD, head), (TAIL, tail)] {
                    let param = if count == 0 {
                        Vec::new()
                    } else {
                        scalar(id, count as f64)
                    };
                    pb_bytes(9, &param, &mut section);
                }
            }
            Self::VolumeKeys(points) => {
                let mut param = scalar(VOLUME, points[0].1);
                let mut keys = Vec::with_capacity(36);
                keys.extend_from_slice(&(-32i32).to_be_bytes());
                for (frame, db) in points {
                    keys.extend_from_slice(&frame.to_le_bytes());
                    keys.extend_from_slice(&KEY_FLAGS.to_le_bytes());
                    keys.extend_from_slice(&db.to_le_bytes());
                }
                pb_bytes(10, &keys, &mut param);
                pb_bytes(9, &param, &mut section);
                pb_bytes(9, &[], &mut section);
                pb_bytes(9, &[], &mut section);
            }
        }
        pb_bytes(9, &[], &mut section);
        pb_bytes(9, &[], &mut section);
        let mut body = Vec::new();
        pb_bytes(1, &section, &mut body);
        pack(&body)
    }
}

fn scalar(id: u64, value: f64) -> Vec<u8> {
    let mut point = vec![0x11]; // Field 2, fixed64.
    point.extend_from_slice(&value.to_le_bytes());
    let mut curve = Vec::new();
    pb_bytes(1, &point, &mut curve);
    let mut param = Vec::new();
    pb_int(1, id, &mut param);
    pb_bytes(3, &curve, &mut param);
    param
}
