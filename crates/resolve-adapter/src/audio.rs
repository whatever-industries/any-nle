//! Qualified direct-mono Resolve clip envelopes.
use crate::{
    binary::{pack, pb_bytes, pb_int},
    prepare::frames_on,
    Result,
};
use graph::{Placement, Rational, Value};

pub(crate) const FADE_LOSS: (&str, &str) = (
    "clip_fades",
    "Resolve clip fades are retained in source metadata",
);
pub(crate) const CLIP_PAN_LOSS: (&str, &str) = (
    "clip_pan",
    "Resolve clip pan is retained in source metadata",
);
pub(crate) const FRACTIONAL_FADE_LOSS: (&str, &str) = (
    "fractional_clip_fades",
    "Resolve fractional clip fade lengths are retained in source metadata",
);
pub(crate) const KEY_LOSS: (&str, &str) = (
    "volume_interpolation",
    "Resolve volume curve law is retained in source metadata",
);
pub(crate) const CUSTOM_FADE_LOSS: (&str, &str) = (
    "clip_fade_shape",
    "Resolve custom clip fade points require a destination mapping",
);
pub(crate) const KEYED_FADE_LOSS: (&str, &str) = (
    "clip_envelope",
    "Resolve volume keys and clip fades require a combined destination mapping",
);
pub(crate) const UNQUALIFIED_FADE_ADAPTER: &str = "resolve.clip_fades";
pub(crate) const EFFECT: u64 = 124;
pub(crate) const VOLUME: u64 = 95;
pub(crate) const HEAD: u64 = 97;
pub(crate) const TAIL: u64 = 98;
pub(crate) const HEAD_POINT: u64 = 99;
pub(crate) const TAIL_POINT: u64 = 100;
pub(crate) const KEY_FLAGS: u32 = 0x000c_0000;
pub(crate) const VOLUME_DB: std::ops::RangeInclusive<f64> = -6.0..=3.0;

pub(crate) type VolumeKeys = [(u32, f64); 2];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Volume {
    Static(f64),
    Keys(VolumeKeys),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Envelope {
    pub(crate) volume: Option<Volume>,
    pub(crate) fades: Option<Fades>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FadePoint {
    position_percent: f64,
    relative_tenths_db: f64,
}

impl FadePoint {
    pub(crate) fn new(position_percent: f64, relative_tenths_db: f64) -> Option<Self> {
        ((1.0..=99.0).contains(&position_percent) && (-60.0..=0.0).contains(&relative_tenths_db))
            .then_some(Self {
                position_percent,
                relative_tenths_db,
            })
    }

    pub(crate) fn receipt(self) -> Value {
        Value::Obj(vec![
            (
                "position_percent".into(),
                Value::Float(self.position_percent),
            ),
            (
                "relative_tenths_db".into(),
                Value::Float(self.relative_tenths_db),
            ),
        ])
    }

    fn from_receipt(value: &Value) -> Option<Self> {
        let Value::Obj(fields) = value else {
            return None;
        };
        if fields.len() != 2 {
            return None;
        }
        let (Some(Value::Float(position)), Some(Value::Float(level))) = (
            value.get("position_percent"),
            value.get("relative_tenths_db"),
        ) else {
            return None;
        };
        Self::new(*position, *level)
    }

    fn encode(self, id: u64) -> Vec<u8> {
        let mut payload = self.position_percent.to_be_bytes().to_vec();
        payload.extend_from_slice(&self.relative_tenths_db.to_be_bytes());
        let mut point = Vec::new();
        pb_bytes(7, &payload, &mut point);
        let mut curve = Vec::new();
        pb_bytes(1, &point, &mut curve);
        let mut param = Vec::new();
        pb_int(1, id, &mut param);
        pb_bytes(3, &curve, &mut param);
        param
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fades {
    pub(crate) head: i64,
    pub(crate) tail: i64,
    pub(crate) head_point: Option<FadePoint>,
    pub(crate) tail_point: Option<FadePoint>,
}

impl Fades {
    pub(crate) fn linear(head: i64, tail: i64) -> Self {
        Self {
            head,
            tail,
            head_point: None,
            tail_point: None,
        }
    }

    fn custom(self) -> bool {
        self.head_point.is_some() || self.tail_point.is_some()
    }

    fn shape(self) -> &'static str {
        if self.custom() {
            "resolve_control_point"
        } else {
            "linear_amplitude"
        }
    }

    pub(crate) fn valid_for(self, length: i64) -> bool {
        self.head >= 0
            && self.tail >= 0
            && self
                .head
                .checked_add(self.tail)
                .is_some_and(|n| n > 0 && n <= length)
            && (self.head_point.is_none() || self.head > 0)
            && (self.tail_point.is_none() || self.tail > 0)
    }

    fn receipt(self) -> Vec<(&'static str, Value)> {
        let mut data = vec![
            ("fade_in_frames", Value::Int(self.head)),
            ("fade_out_frames", Value::Int(self.tail)),
            ("fade_clock", Value::Str("clip_edges".into())),
            ("fade_shape", Value::Str(self.shape().into())),
        ];
        for (key, point) in [
            ("fade_in_point", self.head_point),
            ("fade_out_point", self.tail_point),
        ] {
            if let Some(point) = point {
                data.push((key, point.receipt()));
            }
        }
        data
    }
}

pub(crate) fn prepare(
    p: &Placement,
    source_in: i64,
    length: i64,
    rate: Rational,
    data: Option<&Value>,
) -> Result<Option<Envelope>> {
    let unsupported = || format!("unsupported Resolve clip envelope at {}", p.id);
    let volume = match (p.volume_db, p.volume_keys.as_slice()) {
        (None, []) => None,
        (Some(db), []) if VOLUME_DB.contains(&db) => Some(Volume::Static(db)),
        (None, [first, last]) => {
            let points = [frames_on(first.at, rate)?, frames_on(last.at, rate)?];
            if points[0] < source_in
                || points[0] >= points[1]
                || source_in
                    .checked_add(length)
                    .is_none_or(|end| points[1] >= end)
                || !VOLUME_DB.contains(&first.db)
                || !VOLUME_DB.contains(&last.db)
            {
                return Err(unsupported().into());
            }
            Some(Volume::Keys([
                (u32::try_from(points[0])?, first.db),
                (u32::try_from(points[1])?, last.db),
            ]))
        }
        _ => return Err(unsupported().into()),
    };
    let fades = if let Some(data) = data.filter(|data| data.get("fade_shape").is_some()) {
        let (Some(Value::Int(head)), Some(Value::Int(tail))) =
            (data.get("fade_in_frames"), data.get("fade_out_frames"))
        else {
            return Err(unsupported().into());
        };
        let point = |key| match data.get(key) {
            Some(value) => FadePoint::from_receipt(value)
                .map(Some)
                .ok_or_else(unsupported),
            None => Ok(None),
        };
        let fades = Fades {
            head: *head,
            tail: *tail,
            head_point: point("fade_in_point")?,
            tail_point: point("fade_out_point")?,
        };
        if !fades.valid_for(length) {
            return Err(unsupported().into());
        }
        Some(fades)
    } else {
        None
    };
    let envelope = Envelope { volume, fades };
    let expected = envelope.receipt();
    match data {
        None if expected.is_empty() => {}
        Some(data @ Value::Obj(fields))
            if fields.len() == expected.len()
                && !fields.is_empty()
                && expected.iter().all(|(key, value)| match *key {
                    "fade_in_point" | "fade_out_point" => data.get(key).is_some(),
                    _ => data.get(key) == Some(value),
                }) => {}
        _ => return Err(unsupported().into()),
    }
    Ok((volume.is_some() || fades.is_some()).then_some(envelope))
}

impl Envelope {
    pub(crate) fn static_volume(db: f64) -> Self {
        Self {
            volume: Some(Volume::Static(db)),
            fades: None,
        }
    }

    pub(crate) fn receipt(self) -> Vec<(&'static str, Value)> {
        let mut fields = Vec::new();
        if matches!(self.volume, Some(Volume::Keys(_))) {
            fields.extend([
                ("volume_key_clock", Value::Str("item_source".into())),
                ("volume_interpolation", Value::Str("linear_db".into())),
                ("volume_extrapolation", Value::Str("hold".into())),
            ]);
        }
        if let Some(fades) = self.fades {
            fields.extend(fades.receipt());
        }
        fields
    }

    pub(crate) fn recovered_loss(self) -> Option<(&'static str, &'static str)> {
        match (self.volume, self.fades) {
            (Some(Volume::Keys(_)), Some(_)) => Some(KEYED_FADE_LOSS),
            (Some(Volume::Keys(_)), None) => Some(KEY_LOSS),
            (_, Some(fades)) if fades.custom() => Some(CUSTOM_FADE_LOSS),
            (_, Some(_)) => Some(FADE_LOSS),
            _ => None,
        }
    }

    pub(crate) fn encode(self) -> Result<String> {
        let mut section = Vec::new();
        pb_int(1, EFFECT, &mut section);
        pb_int(7, 0, &mut section);
        let volume = match self.volume {
            None => Vec::new(),
            Some(Volume::Static(db)) => scalar(VOLUME, db),
            Some(Volume::Keys(points)) => {
                let mut param = scalar(VOLUME, points[0].1);
                let mut keys = Vec::with_capacity(36);
                keys.extend_from_slice(&(-32i32).to_be_bytes());
                for (frame, db) in points {
                    keys.extend_from_slice(&frame.to_le_bytes());
                    keys.extend_from_slice(&KEY_FLAGS.to_le_bytes());
                    keys.extend_from_slice(&db.to_le_bytes());
                }
                pb_bytes(10, &keys, &mut param);
                param
            }
        };
        let fades = self.fades.unwrap_or_else(|| Fades::linear(0, 0));
        for slot in [
            volume,
            fade_length(HEAD, fades.head),
            fade_length(TAIL, fades.tail),
            fades
                .head_point
                .map_or_else(Vec::new, |point| point.encode(HEAD_POINT)),
            fades
                .tail_point
                .map_or_else(Vec::new, |point| point.encode(TAIL_POINT)),
        ] {
            pb_bytes(9, &slot, &mut section);
        }
        let mut body = Vec::new();
        pb_bytes(1, &section, &mut body);
        pack(&body)
    }
}

fn fade_length(id: u64, frames: i64) -> Vec<u8> {
    if frames == 0 {
        Vec::new()
    } else {
        scalar(id, frames as f64)
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
