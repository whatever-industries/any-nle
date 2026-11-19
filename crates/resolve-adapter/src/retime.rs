//! Constant picture maps witnessed in native Resolve 21.2 saves.
use crate::{
    binary::{pb_bytes, qt, Qt},
    prepare::frames_on,
    Result,
};
use graph::{Placement, Rational, Speed};

pub(crate) const NEAREST: &str =
    "0000000200000019800a16082c38124a08085b1a040a0220014a004a004a004a00";

const HALF_FRAME: &str = "000000000000e03f";
pub(crate) const HOLD_IN: &str = "86400";
pub(crate) const HOLD_MAP_END: f64 = 60_000.0;

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
    pb_bytes(1, &point(0.0, first), &mut keys);
    pb_bytes(1, &point(last_record, last_source), &mut keys);
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
