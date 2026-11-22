//! Editable picture maps witnessed in Resolve 21.2 saves and playback.
use crate::{
    binary::{pb_bytes, qt, Qt},
    prepare::frames_on,
    Result,
};
use graph::{Placement, Rational, Sidecar, Speed, Value};

pub(crate) const NEAREST: &str =
    "0000000200000019800a16082c38124a08085b1a040a0220014a004a004a004a00";
pub(crate) const NEAREST_SECTION: &[u8] =
    b"\x08\x2c\x38\x12\x4a\x08\x08\x5b\x1a\x04\x0a\x02\x20\x01\x4a\x00\x4a\x00\x4a\x00\x4a\x00";

const HALF_FRAME: &str = "000000000000e03f";
pub(crate) const HOLD_IN: &str = "86400";
pub(crate) const HOLD_MAP_END: f64 = 60_000.0;
pub(crate) const LINEAR_ADAPTER: &str = "picture.retime_linear";
pub(crate) const MAX_LINEAR_KEYS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinearMap {
    pub(crate) record_in: i64,
    pub(crate) source_rate: Rational,
    record_rate: Rational,
    keys: Vec<[i64; 2]>,
}

impl LinearMap {
    pub(crate) fn new(
        record_in: i64,
        keys: Vec<[i64; 2]>,
        available: i64,
        source_rate: Rational,
        record_rate: Rational,
    ) -> Result<Self> {
        Self::validate_keys(&keys, available)?;
        if record_in < 0 {
            return Err("Linear retime requires a nonnegative record trim".into());
        }
        if source_rate <= Rational::ZERO || record_rate <= Rational::ZERO {
            return Err("Linear retime requires positive source and timeline clocks".into());
        }
        Ok(Self { record_in, source_rate, record_rate, keys })
    }

    pub(crate) fn validate_keys(keys: &[[i64; 2]], available: i64) -> Result<()> {
        let valid = (3..=MAX_LINEAR_KEYS).contains(&keys.len())
            && keys.first() == Some(&[0, 0])
            && keys.last().is_some_and(|&[x, y]| {
                (0..=10_000_000).contains(&x) && y.checked_add(1) == Some(available)
            })
            && keys.windows(2).all(|w| w[0][0] < w[1][0] && w[0][1] < w[1][1]);
        if !valid {
            return Err("expected a bounded, forward Linear retime map from zero to the last source frame".into());
        }
        let [x, y] = keys[1];
        if keys.iter().all(|&[r, s]| i128::from(r) * i128::from(y) == i128::from(s) * i128::from(x)) {
            return Err("expected a piecewise Linear retime with changing speed".into());
        }
        Ok(())
    }

    pub(crate) fn samples(&self, length: i64) -> Result<Vec<i64>> {
        let count = hold_count(length).ok_or("invalid Linear retime edit length")?;
        let end = self.record_in.checked_add(length).ok_or("Linear retime edit overflows")?;
        if end > self.keys.last().ok_or("empty Linear retime map")?[0] {
            return Err("Linear retime does not cover the full saved edit".into());
        }
        let mut segments = self.keys.windows(2).peekable();
        let mut samples = Vec::with_capacity(count);
        for record in self.record_in..end {
            while segments.peek().is_some_and(|pair| pair[1][0] <= record) {
                segments.next();
            }
            let pair = segments.peek().ok_or("missing Linear retime segment")?;
            let (a, b) = (pair[0], pair[1]);
            let dx = i128::from(b[0] - a[0]);
            let dy = i128::from(b[1] - a[1]);
            let offset = i128::from(record - a[0]) * dy;
            let fraction = offset % dx;
            if record == self.record_in && fraction != 0 {
                return Err("Linear retime source In must fall on a whole source frame".into());
            }
            // Mixed-rate sampling can advance a value within 0.002 frames of
            // the next source frame. Require agreement with the exact floor
            // even with a 1/400-frame advance, including a rounding margin.
            if self.source_rate != self.record_rate && (dx - fraction) * 400 <= dx {
                return Err("mixed-rate Linear retime samples too close to a source frame boundary".into());
            }
            samples.push(a[1] + i64::try_from(offset / dx)?);
        }
        Ok(samples)
    }

    pub(crate) fn source_time(&self, frame: i64) -> Result<Rational> {
        Rational::new(
            i128::from(frame) * i128::from(self.source_rate.den()),
            self.source_rate.num().into(),
        ).ok_or_else(|| "invalid Linear retime source trim".into())
    }

    pub(crate) fn receipt(&self, owner: &str) -> Sidecar {
        Sidecar {
            adapter: LINEAR_ADAPTER.into(),
            object: owner.into(),
            data: Value::Obj(vec![
                ("record_in".into(), Value::Int(self.record_in)),
                ("keys".into(), Value::Arr(self.keys.iter().map(|[record, source]| {
                    Value::Arr(vec![Value::Int(*record), Value::Int(*source)])
                }).collect())),
            ]),
        }
    }

    pub(crate) fn from_receipt(
        value: &Value,
        available: i64,
        source_rate: Rational,
        record_rate: Rational,
    ) -> Result<Self> {
        let invalid = || "malformed graph/2 Linear retime receipt";
        let (Some(Value::Int(record_in)), Some(Value::Arr(keys))) =
            (value.get("record_in"), value.get("keys"))
        else {
            return Err(invalid().into());
        };
        if keys.len() > MAX_LINEAR_KEYS {
            return Err(invalid().into());
        }
        let keys = keys.iter().map(|key| match key {
            Value::Arr(pair) => match pair.as_slice() {
                [Value::Int(record), Value::Int(source)] => Ok([*record, *source]),
                _ => Err(invalid()),
            },
            _ => Err(invalid()),
        }).collect::<std::result::Result<_, _>>()?;
        Self::new(*record_in, keys, available, source_rate, record_rate)
    }

    pub(crate) fn encode(&self, id: &str) -> Vec<u8> {
        let record_fps = self.record_rate.num() as f64 / self.record_rate.den() as f64;
        let source_fps = self.source_rate.num() as f64 / self.source_rate.den() as f64;
        encode_points(self.keys.iter().map(|&[record, source]| {
            (record as f64 / record_fps, source as f64 / source_fps)
        }), self.keys.last().expect("validated retime keys")[1] as f64 / source_fps, id)
    }
}

pub(crate) fn hold_count(length: i64) -> Option<usize> {
    usize::try_from(length).ok().filter(|n| (2..=graph::MAX_LIST).contains(n))
}

pub(crate) fn encode_hold(frame: i64, available: i64, id: &str) -> Result<Vec<u8>> {
    if !(0..available).contains(&frame) {
        return Err("held source picture is outside the verified original".into());
    }
    let source = frame as f64 / 24.0;
    Ok(encode_map(source, HOLD_MAP_END, source, (available - 1) as f64 / 24.0, id))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PictureSpeed {
    Half,
    Double,
    Reverse,
}

impl PictureSpeed {
    pub(crate) fn from_ratio(ratio: (i64, i64)) -> Option<Self> {
        match ratio {
            (2, 1) => Some(Self::Half),
            (1, 2) => Some(Self::Double),
            (-1, 1) => Some(Self::Reverse),
            _ => None,
        }
    }

    pub(crate) fn graph(self, record: i64) -> Option<Speed> {
        if record <= 0 {
            return None;
        }
        let (ratio, denominator) = match self {
            Self::Half => ((2, 1), 48),
            Self::Double => ((1, 2), 12),
            Self::Reverse => ((-1, 1), 24),
        };
        Some(Speed {
            ratio,
            source_duration: Rational::new(record.into(), denominator)?,
        })
    }

    pub(crate) fn prepare(edit: &Placement, rate: Rational, length: i64) -> Result<Option<Self>> {
        let Some(speed) = &edit.speed else {
            return Ok(None);
        };
        let mode =
            Self::from_ratio(speed.ratio).filter(|mode| mode.graph(length).as_ref() == Some(speed));
        if rate != Rational::integer(24)
            || edit.video_track != Some(1)
            || !edit.audio_tracks.is_empty()
            || edit.transform.is_some()
            || !edit.effects.is_empty()
        {
            return Err("expected a plain same-rate 24 fps V1 picture retime".into());
        }
        mode.map(Some)
            .ok_or_else(|| "unsupported Resolve constant speed or source span".into())
    }

    pub(crate) fn record_source_in(
        self,
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Option<String> {
        if source_in < 0 {
            return None;
        }
        let whole = match self {
            Self::Half => source_in.checked_mul(2)?,
            Self::Double => source_in / 2,
            Self::Reverse => available.checked_sub(source_in)?.checked_sub(length)?,
        };
        if self == Self::Double && source_in % 2 != 0 {
            return Some(format!("{whole}|{HALF_FRAME}"));
        }
        (whole >= 0).then(|| whole.to_string())
    }

    pub(crate) fn file_source_in(self, saved_in: &str, length: i64, available: i64) -> Option<i64> {
        let saved_in = if saved_in.is_empty() { "0" } else { saved_in };
        if let Some((whole, fraction)) = saved_in.split_once('|') {
            if self != Self::Double || !fraction.eq_ignore_ascii_case(HALF_FRAME) {
                return None;
            }
            let whole: i64 = whole.parse().ok()?;
            return whole.checked_mul(2)?.checked_add(1).filter(|frame| *frame >= 0);
        }
        let saved_in: i64 = saved_in.parse().ok()?;
        if saved_in < 0 {
            return None;
        }
        match self {
            Self::Half if saved_in % 2 == 0 => Some(saved_in / 2),
            Self::Double => saved_in.checked_mul(2),
            Self::Reverse => available.checked_sub(saved_in)?.checked_sub(length).filter(|frame| *frame >= 0),
            _ => None,
        }
    }

    pub(crate) fn coordinates(self, available: i64) -> Option<(f64, f64, f64, f64)> {
        if available < 2 {
            return None;
        }
        let source_end = (available - 1) as f64 / 24.0;
        Some(match self {
            Self::Half => {
                let end = available.checked_mul(2)?.checked_sub(1)? as f64 / 24.0;
                (0.0, end, end / 2.0, source_end)
            }
            Self::Double => {
                if available <= 2 {
                    return None;
                }
                // Resolve's last map key precedes the final source frame.
                let end = (available - 2) as f64 / 24.0;
                (0.0, end / 2.0, end, source_end)
            }
            Self::Reverse => (source_end, source_end, 0.0, source_end),
        })
    }

    pub(crate) fn encode(self, available: i64, id: &str) -> Result<Vec<u8>> {
        let (first, last_record, last_source, source_end) = self
            .coordinates(available)
            .ok_or("invalid Resolve retime media extent")?;
        Ok(encode_map(first, last_record, last_source, source_end, id))
    }

    pub(crate) fn source_length(self, length: i64) -> Result<i64> {
        let speed = self.graph(length).ok_or("invalid Resolve retime length")?;
        frames_on(speed.source_duration, Rational::integer(24))
    }

    /// Source extent required by the saved nearest-frame map.
    /// For an odd half-speed edit the final picture is displayed once,
    /// although the time range is only half a source frame into it.
    pub(crate) fn sampled_source_frames(self, length: i64) -> Result<i64> {
        if length <= 0 {
            return Err("invalid Resolve retime length".into());
        }
        match self {
            Self::Half => Ok(length / 2 + length % 2),
            Self::Double => self.source_length(length),
            Self::Reverse => Ok(length),
        }
    }
}

fn encode_map(first: f64, last_record: f64, last_source: f64, source_end: f64, id: &str) -> Vec<u8> {
    encode_points([(0.0, first), (last_record, last_source)], source_end, id)
}

fn encode_points(points: impl IntoIterator<Item = (f64, f64)>, source_end: f64, id: &str) -> Vec<u8> {
    let point = |record: f64, source: f64| {
        let mut point = Vec::new();
        for (tag, value) in [(9, record), (17, source)] {
            if value != 0.0 {
                point.push(tag);
                point.extend_from_slice(&value.to_le_bytes());
            }
        }
        point
    };
    let mut keys = vec![0x80];
    let mut last_record = 0.0;
    for (record, source) in points {
        pb_bytes(1, &point(record, source), &mut keys);
        last_record = record;
    }
    qt(&[
        ("YMin", Qt::Double(-1.0)),
        ("YMax", Qt::Double(-1.0)),
        ("XMax", Qt::Double(last_record)),
        ("UniqueId", Qt::Text(id)),
        ("LastValidYOffset", Qt::Double(source_end)),
        ("KeyframesBA", Qt::Bytes(&keys)),
        ("DbType", Qt::Text("Sm2TimeMap")),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_map_uses_record_trim_and_floor_sampling_between_keys() {
        let rate = Rational::integer(24);
        let keys = vec![[0, 0], [8, 4], [12, 10], [21, 19]];
        let map = LinearMap::new(6, keys.clone(), 20, rate, rate).unwrap();
        assert_eq!(map.samples(8).unwrap(), [3, 3, 4, 5, 7, 8, 10, 11]);
        assert_eq!(LinearMap::from_receipt(&map.receipt("edit").data, 20, rate, rate).unwrap(), map);
        assert!(LinearMap::from_receipt(&map.receipt("edit").data, 19, rate, rate).is_err());
        assert!(map.samples(16).is_err());
        assert!(LinearMap::new(7, keys, 20, rate, rate).unwrap().samples(8).is_err());
        assert!(LinearMap::new(0, vec![[0, 0], [8, 4], [12, 4], [21, 19]], 20, rate, rate).is_err());
    }

    #[test]
    fn mixed_linear_trim_uses_file_clock_and_requires_stable_frame_selections() {
        let source = crate::prepare::rate();
        let record = Rational::integer(24);
        let keys = vec![[0, 0], [8, 4], [12, 10], [21, 19]];
        let map = LinearMap::new(6, keys.clone(), 20, source, record).unwrap();
        let samples = map.samples(8).unwrap();
        assert_eq!(samples, [3, 3, 4, 5, 7, 8, 10, 11]);
        assert_eq!(map.source_time(samples[0]).unwrap(), Rational::new(1001, 8000).unwrap());
        let reverse_pair = LinearMap::new(6, keys.clone(), 20, record, source).unwrap();
        assert_eq!(reverse_pair.samples(8).unwrap(), samples);
        assert_eq!(reverse_pair.source_time(samples[0]).unwrap(), Rational::new(1, 8).unwrap());
        for file_rate in [Rational::integer(25), Rational::new(30000, 1001).unwrap()] {
            let map = LinearMap::new(6, keys.clone(), 20, file_rate, source).unwrap();
            assert_eq!(map.samples(8).unwrap(), samples);
            assert_eq!(frames_on(map.source_time(samples[0]).unwrap(), file_rate).unwrap(), 3);
        }
        assert!(LinearMap::new(6, keys, 20, Rational::ZERO, source).is_err());

        let near_boundary = vec![[0, 0], [1001, 1000], [1003, 1004]];
        let mixed = LinearMap::new(0, near_boundary.clone(), 1005, source, record).unwrap();
        let reverse_pair = LinearMap::new(0, near_boundary.clone(), 1005, record, source).unwrap();
        let same = LinearMap::new(0, near_boundary, 1005, record, record).unwrap();
        assert!(mixed.samples(2).is_err());
        assert!(reverse_pair.samples(2).is_err());
        assert_eq!(same.samples(2).unwrap(), [0, 0]);
    }

    #[test]
    fn native_trim_conventions_are_invertible_and_refuse_fractional_half_speed() {
        let half = PictureSpeed::Half;
        assert_eq!(half.record_source_in(12, 24, 96).as_deref(), Some("24"));
        assert_eq!(half.file_source_in("24", 24, 96), Some(12));
        assert_eq!(half.file_source_in("25", 24, 96), None);
        assert_eq!(half.source_length(24).unwrap(), 12);
        assert!(half.source_length(23).is_err());
        assert_eq!(half.graph(23).unwrap().source_duration, Rational::new(23, 48).unwrap());
        assert_eq!(half.sampled_source_frames(23).unwrap(), 12);
        assert_eq!(half.sampled_source_frames(24).unwrap(), 12);
        assert_eq!(half.sampled_source_frames(25).unwrap(), 13);
        assert_eq!(half.sampled_source_frames(1).unwrap(), 1);
        assert!(half.sampled_source_frames(0).is_err());
        let reverse = PictureSpeed::Reverse;
        assert_eq!(reverse.record_source_in(12, 24, 96).as_deref(), Some("60"));
        assert_eq!(reverse.file_source_in("60", 24, 96), Some(12));
        assert_eq!(reverse.file_source_in("73", 24, 96), None);
        let double = PictureSpeed::Double;
        assert_eq!(double.record_source_in(12, 24, 96).as_deref(), Some("6"));
        let odd = double.record_source_in(13, 24, 96).unwrap();
        assert_eq!(odd, "6|000000000000e03f");
        assert_eq!(double.file_source_in(&odd, 24, 96), Some(13));
        assert_eq!(double.file_source_in("6|000000000000d03f", 24, 96), None);
        assert_eq!(double.source_length(24).unwrap(), 48);
        assert_eq!(double.coordinates(95).unwrap().1 * 24.0, 46.5);
    }
}
