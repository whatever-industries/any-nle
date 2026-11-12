//! The two constant picture maps witnessed in native Resolve 21.2 saves.
use crate::{
    binary::{pb_bytes, qt, Qt},
    prepare::frames_on,
    Result,
};
use graph::{Placement, Rational, Speed};

pub(crate) const NEAREST: &str =
    "0000000200000019800a16082c38124a08085b1a040a0220014a004a004a004a00";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PictureSpeed {
    Half,
    Reverse,
}

impl PictureSpeed {
    pub(crate) fn from_ratio(ratio: (i64, i64)) -> Option<Self> {
        match ratio {
            (2, 1) => Some(Self::Half),
            (-1, 1) => Some(Self::Reverse),
            _ => None,
        }
    }

    pub(crate) fn graph(self, record: i64) -> Option<Speed> {
        let (ratio, frames) = match self {
            Self::Half if record % 2 == 0 => ((2, 1), record / 2),
            Self::Reverse => ((-1, 1), record),
            _ => return None,
        };
        (frames > 0).then_some(Speed {
            ratio,
            source_duration: Rational::new(frames.into(), 24)?,
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
    ) -> Option<i64> {
        match self {
            Self::Half => source_in.checked_mul(2),
            Self::Reverse => available.checked_sub(source_in)?.checked_sub(length),
        }
        .filter(|value| *value >= 0)
    }

    pub(crate) fn file_source_in(self, saved_in: i64, length: i64, available: i64) -> Option<i64> {
        match self {
            Self::Half if saved_in % 2 == 0 => Some(saved_in / 2),
            Self::Reverse => self.record_source_in(saved_in, length, available),
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
            Self::Reverse => (source_end, source_end, 0.0, source_end),
        })
    }

    pub(crate) fn encode(self, available: i64, id: &str) -> Result<Vec<u8>> {
        let (first, last_record, last_source, source_end) = self
            .coordinates(available)
            .ok_or("invalid Resolve retime media extent")?;
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
        Ok(qt(&[
            ("YMin", Qt::Double(-1.0)),
            ("YMax", Qt::Double(-1.0)),
            ("XMax", Qt::Double(last_record)),
            ("UniqueId", Qt::Text(id)),
            ("LastValidYOffset", Qt::Double(source_end)),
            ("KeyframesBA", Qt::Bytes(&keys)),
            ("DbType", Qt::Text("Sm2TimeMap")),
        ]))
    }

    pub(crate) fn source_length(self, length: i64) -> Result<i64> {
        let speed = self.graph(length).ok_or("invalid Resolve retime length")?;
        frames_on(speed.source_duration, Rational::integer(24))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_trim_conventions_are_invertible_and_refuse_fractional_half_speed() {
        let half = PictureSpeed::Half;
        assert_eq!(half.record_source_in(12, 24, 96), Some(24));
        assert_eq!(half.file_source_in(24, 24, 96), Some(12));
        assert_eq!(half.file_source_in(25, 24, 96), None);
        assert_eq!(half.source_length(24).unwrap(), 12);
        assert!(half.source_length(23).is_err());
        let reverse = PictureSpeed::Reverse;
        assert_eq!(reverse.record_source_in(12, 24, 96), Some(60));
        assert_eq!(reverse.file_source_in(60, 24, 96), Some(12));
        assert_eq!(reverse.file_source_in(73, 24, 96), None);
    }
}
