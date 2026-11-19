//! Verified mixed-rate Frame Sampling and Resolve default Nearest conform.
use crate::Result;
use graph::Rational;

#[derive(Clone, Copy)]
pub(crate) enum Cadence {
    DropFrames,
    RepeatFrames,
    DropFrom25,
    DropFromNtsc,
}

const REPEAT_ZERO_FRACTION: &str = "fca9f1d24d6260bf"; // -0.002 in little-endian double.

/// Resolve's default Nearest conform from a verified whole source-frame trim.
#[derive(Clone, Copy)]
pub(crate) struct DefaultConform {
    cadence: Cadence,
    file_in: i64,
    resolve_in: f64,
}

pub(crate) enum ResolveTrim {
    Adjusted(Cadence, i64),
    DefaultConform(DefaultConform),
}

impl DefaultConform {
    fn from_resolve_in(cadence: Cadence, saved: &str) -> Option<Self> {
        if saved.is_empty() {
            return Some(Self {
                cadence,
                file_in: 0,
                resolve_in: 0.0,
            });
        }
        let (whole, fraction) = saved
            .split_once('|')
            .map_or((saved, None), |(whole, fraction)| (whole, Some(fraction)));
        let whole: i32 = whole.parse().ok()?;
        let frac = if let Some(fraction) = fraction {
            let mut bytes = [0; 8];
            hex::decode_to_slice(fraction, &mut bytes).ok()?;
            f64::from_le_bytes(bytes)
        } else {
            0.0
        };
        if whole < 0 || !(0.0..1.0).contains(&frac) {
            return None;
        }
        let resolve_in = f64::from(whole) + frac;
        let (num, den) = cadence.ratio();
        let file_in = (resolve_in * num as f64 / den as f64).round();
        // Native doubles can land one representable value below a whole frame.
        ((resolve_in - file_in * den as f64 / num as f64).abs() <= 1e-6).then_some(Self {
            cadence,
            file_in: file_in as i64,
            resolve_in,
        })
    }

    pub(crate) fn source_in(self) -> Rational {
        let rate = self.cadence.source_rate();
        Rational::new(
            i128::from(self.file_in) * i128::from(rate.den()),
            i128::from(rate.num()),
        )
        .expect("bounded source frame")
    }

    fn sample(self, offset: i64) -> i64 {
        match self.cadence {
            // Resolve evaluates the saved record-frame coordinate in seconds
            // before converting to the file clock. Keep this order at skips.
            Cadence::DropFrames | Cadence::DropFrom25 | Cadence::DropFromNtsc => {
                let rate = self.cadence.source_rate();
                let rate = rate.num() as f64 / rate.den() as f64;
                (((self.resolve_in + offset as f64) / (24_000.0 / 1001.0)) * rate + 0.002).floor()
                    as i64
            }
            // At integer record offsets the measured advance contributes two
            // units to the 1000/1001 ratio's numerator.
            Cadence::RepeatFrames => self.file_in + (offset * 1000 + 2) / 1001,
        }
    }

    pub(crate) fn last_frame(self, length: i64) -> Option<i64> {
        bounded_length(length).then(|| self.sample(length - 1))
    }

    pub(crate) fn capture(self, length: i64) -> Vec<i64> {
        (0..length).map(|offset| self.sample(offset)).collect()
    }

    pub(crate) fn agrees(self, map: &[i64], length: i64) -> bool {
        bounded_length(length) && map.iter().copied().eq((0..length).map(|i| self.sample(i)))
    }
}

fn bounded_length(length: i64) -> bool {
    (1..=graph::MAX_LIST as i64).contains(&length)
}

impl Cadence {
    pub(crate) fn between(source: Rational, record: Rational) -> Option<Self> {
        let rate_24 = Rational::integer(24);
        let rate_23976 = crate::prepare::rate();
        if (source, record) == (rate_24, rate_23976) {
            Some(Self::DropFrames)
        } else if (source, record) == (rate_23976, rate_24) {
            Some(Self::RepeatFrames)
        } else {
            None
        }
    }

    pub(crate) fn for_resolve_source(source: Rational, record: Rational) -> Option<Self> {
        if record == crate::prepare::rate() {
            match (source.num(), source.den()) {
                (25, 1) => return Some(Self::DropFrom25),
                (30000, 1001) => return Some(Self::DropFromNtsc),
                _ => {}
            }
        }
        Self::between(source, record)
    }

    pub(crate) fn source_rate(self) -> Rational {
        match self {
            Self::DropFrames => Rational::integer(24),
            Self::RepeatFrames => crate::prepare::rate(),
            Self::DropFrom25 => Rational::integer(25),
            Self::DropFromNtsc => Rational::new(30000, 1001).expect("known rate"),
        }
    }

    fn ratio(self) -> (i64, i64) {
        match self {
            Self::DropFrames => (1001, 1000),
            Self::RepeatFrames => (1000, 1001),
            Self::DropFrom25 => (1001, 960),
            Self::DropFromNtsc => (5, 4),
        }
    }

    pub(crate) fn last_frame(self, start: i64, length: i64) -> Option<i64> {
        let end = start.checked_add(length)?.checked_sub(1)?;
        let (numerator, denominator) = self.ratio();
        let minimum = match self {
            Self::DropFrames => 1,
            Self::RepeatFrames => 0,
            // These pairings are qualified only for a native Resolve trim.
            Self::DropFrom25 | Self::DropFromNtsc => return None,
        };
        (start >= minimum && bounded_length(length)).then_some(())?;
        end.checked_mul(numerator).map(|n| n / denominator)
    }

    fn samples(self, start: i64, length: i64) -> impl Iterator<Item = i64> {
        let (numerator, denominator) = self.ratio();
        (0..length).map(move |offset| (start + offset) * numerator / denominator)
    }

    pub(crate) fn capture(self, start: i64, length: i64, used: &mut usize) -> Result<Vec<i64>> {
        self.last_frame(start, length)
            .ok_or("mixed-rate picture needs a verified trim and bounded frame map")?;
        *used = used
            .checked_add(length as usize)
            .filter(|n| *n <= graph::MAX_GRAPH_BYTES / size_of::<i64>())
            .ok_or("sampled-frame map budget exceeded")?;
        Ok(self.samples(start, length).collect())
    }

    pub(crate) fn agrees(self, map: &[i64], start: i64, length: i64) -> bool {
        self.last_frame(start, length)
            .is_some_and(|_| map.iter().copied().eq(self.samples(start, length)))
    }

    fn resolve_fraction(self) -> Option<&'static str> {
        match self {
            Self::DropFrames => Some("2b8716d9cef7ef3f"), // 0.999 in little-endian double.
            Self::RepeatFrames => Some("560e2db29defef3f"), // 0.998 in little-endian double.
            Self::DropFrom25 | Self::DropFromNtsc => None,
        }
    }

    pub(crate) fn encode_in(self, start: i64) -> Option<String> {
        // Shift the saved trim by the verified fraction of a record frame.
        // The graph retains the exact, unadjusted source time.
        if start == 0 && matches!(self, Self::RepeatFrames) {
            Some(format!("0|{REPEAT_ZERO_FRACTION}"))
        } else {
            Some(format!("{}|{}", start - 1, self.resolve_fraction()?))
        }
    }

    pub(crate) fn decode_in(self, saved: &str) -> Option<i64> {
        let (whole, fraction) = saved.split_once('|')?;
        let whole: i64 = whole.parse().ok()?;
        if whole == 0
            && matches!(self, Self::RepeatFrames)
            && fraction.eq_ignore_ascii_case(REPEAT_ZERO_FRACTION)
        {
            return Some(0);
        }
        if !fraction.eq_ignore_ascii_case(self.resolve_fraction()?) {
            return None;
        }
        (whole >= 0).then_some(whole.checked_add(1)?)
    }

    pub(crate) fn resolve_trim(self, saved: &str, nearest: bool) -> Option<ResolveTrim> {
        if let Some(record_in) = self.decode_in(saved) {
            return Some(ResolveTrim::Adjusted(self, record_in));
        }
        if nearest {
            return DefaultConform::from_resolve_in(self, saved).map(ResolveTrim::DefaultConform);
        }
        None
    }

    pub(crate) fn premiere_default_in(
        self,
        source_in: Rational,
        map: &[i64],
        length: i64,
    ) -> Option<Rational> {
        let rate = self.source_rate();
        let file_in = crate::prepare::frames_on(source_in, rate).ok()?;
        if !(0..=i64::from(i32::MAX)).contains(&file_in) || !bounded_length(length) {
            return None;
        }
        let (num, den) = self.ratio();
        // The phases are in 1/2000 of a file frame. Each must reproduce the
        // complete map with Premiere's ordinary Frame Sampling.
        let phases: &[i64] = match self {
            Self::DropFrames => &[3, 5],
            Self::RepeatFrames => &[4],
            Self::DropFrom25 => &[3],
            Self::DropFromNtsc => &[0],
        };
        let phase = phases.iter().copied().find(|&phase| {
            map.iter().copied().eq((0..length)
                .map(|offset| file_in + (offset * num * 2000 + phase * den) / (den * 2000)))
        })?;
        Rational::new(
            (i128::from(file_in) * 2000 + i128::from(phase)) * i128::from(rate.den()),
            i128::from(rate.num()) * 2000,
        )
    }

    pub(crate) fn time_map(self, frames: i64) -> Vec<u8> {
        let rate = self.source_rate();
        let end = (frames - 1) as f64 / (rate.num() as f64 / rate.den() as f64);
        let middle = match self {
            Self::DropFrames => end - 1.0 / 24_000.0,
            Self::RepeatFrames => end + 1.0 / 24_000.0,
            Self::DropFrom25 => end - 41.0 / 24_000.0,
            Self::DropFromNtsc => end - 1001.0 / 120_000.0,
        };
        let mut map = vec![2];
        for time in [end, 0.0, middle, 0.0, end] {
            map.extend_from_slice(&time.to_be_bytes());
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_repeats_preserve_source_grid_and_phase() {
        let cadence = Cadence::RepeatFrames;
        let trim = DefaultConform::from_resolve_in(cadence, "24|007c6abc7493983f").unwrap();
        assert_eq!(trim.source_in(), Rational::parse("1001/1000").unwrap());
        let map = trim.capture(1059);
        assert_eq!(
            cadence
                .premiere_default_in(trim.source_in(), &map, 1059)
                .and_then(|time| time.to_units(254_016_000_000)),
            Some(254_291_205_168)
        );
        assert_eq!(&map[..5], &[24, 25, 26, 26, 27]);
        assert_eq!(&map[1002..1006], &[1025, 1026, 1026, 1027]);
        assert_eq!(trim.last_frame(1059), map.last().copied());
        assert!(DefaultConform::from_resolve_in(cadence, "23|560e2db29defef3f").is_none());
        assert_eq!(
            DefaultConform::from_resolve_in(cadence, "")
                .unwrap()
                .source_in(),
            Rational::ZERO
        );
        assert_eq!(
            DefaultConform::from_resolve_in(cadence, "1000|00fcffffffffef3f")
                .unwrap()
                .source_in(),
            Rational::parse("1001/24").unwrap()
        );
    }

    #[test]
    fn native_skips_follow_the_saved_double_at_cadence_boundaries() {
        let cadence = Cadence::DropFrames;
        let trim = DefaultConform::from_resolve_in(cadence, "22|e022b8b0c543ef3f").unwrap();
        assert_eq!(trim.source_in(), Rational::parse("23/24").unwrap());
        let map = trim.capture(3097);
        let skipped: Vec<_> = map
            .windows(2)
            .enumerate()
            .filter_map(|(i, pair)| (pair[1] - pair[0] > 1).then_some(i + 1))
            .collect();
        assert_eq!(skipped, [998, 1999, 2999]);
        assert_eq!(
            cadence
                .premiere_default_in(trim.source_in(), &map[..1058], 1058)
                .and_then(|time| time.to_units(254_016_000_000)),
            Some(243_458_460_000)
        );
        assert!(cadence
            .premiere_default_in(trim.source_in(), &map, 3097)
            .is_none());
        let zero = DefaultConform::from_resolve_in(cadence, "")
            .unwrap()
            .capture(1058);
        assert_eq!((&zero[997..1001]), &[997, 998, 1000, 1001]);
        assert_eq!(
            cadence
                .premiere_default_in(Rational::ZERO, &zero, 1058)
                .and_then(|time| time.to_units(254_016_000_000)),
            Some(15_876_000)
        );
    }

    #[test]
    fn native_25_fps_conform_uses_the_whole_file_trim_and_verified_phase() {
        let source = Rational::integer(25);
        let record = crate::prepare::rate();
        assert!(Cadence::between(source, record).is_none());
        let cadence = Cadence::for_resolve_source(source, record).unwrap();
        assert_eq!(
            hex::encode(cadence.time_map(3240)),
            "02406031eb851eb8520000000000000000406031dd867c3ece0000000000000000406031eb851eb852"
        );
        let saved = "22|0022d9d794aaad3f";
        assert!(cadence.resolve_trim(saved, false).is_none());
        let Some(ResolveTrim::DefaultConform(trim)) = cadence.resolve_trim(saved, true) else {
            panic!("expected a native Resolve trim");
        };
        assert_eq!(trim.source_in(), Rational::new(23, 25).unwrap());
        let map = trim.capture(2950);
        assert_eq!((map[0], map[2949]), (23, 3097));
        assert!(map
            .iter()
            .enumerate()
            .all(|(i, &frame)| { frame == 23 + (i as i64 * 1001 + 1) / 960 }));
        assert_eq!(
            cadence.premiere_default_in(trim.source_in(), &map, 2950),
            Rational::new(46_003, 50_000)
        );
        let mut wrong = map;
        wrong[100] += 1;
        assert!(cadence
            .premiere_default_in(trim.source_in(), &wrong, 2950)
            .is_none());
    }

    #[test]
    fn native_2997_conform_preserves_the_five_to_four_cadence_and_file_trim() {
        let source = Rational::new(30000, 1001).unwrap();
        let record = crate::prepare::rate();
        assert!(Cadence::between(source, record).is_none());
        let cadence = Cadence::for_resolve_source(source, record).unwrap();
        assert_eq!(
            hex::encode(cadence.time_map(3240)),
            "02405b04c6cae376ec0000000000000000405b043e1f67152a0000000000000000405b04c6cae376ec"
        );
        let saved = "18|c09999999999d93f";
        assert!(cadence.resolve_trim(saved, false).is_none());
        let Some(ResolveTrim::DefaultConform(trim)) = cadence.resolve_trim(saved, true) else {
            panic!("expected a native Resolve trim");
        };
        assert_eq!(trim.source_in(), Rational::new(23_023, 30_000).unwrap());
        let map = trim.capture(2500);
        assert_eq!((map[0], map[2499]), (23, 3146));
        assert!(map
            .iter()
            .enumerate()
            .all(|(offset, &frame)| { frame == 23 + offset as i64 * 5 / 4 }));
        assert_eq!(
            cadence.premiere_default_in(trim.source_in(), &map, 2500),
            Some(trim.source_in())
        );
    }

    #[test]
    fn samples_and_refusals_use_exact_trim_phase() {
        let cadence = Cadence::DropFrames;
        let mut total = 0;
        let map = cadence.capture(23, 3136, &mut total).unwrap();
        assert_eq!(
            (map.len(), total, map[0], map[976], map[977], map[1058]),
            (3136, 3136, 23, 999, 1001, 1082)
        );
        assert_eq!(
            (map[1976], map[1977], map[2976], map[2977]),
            (2000, 2002, 3001, 3003)
        );
        assert!(cadence.agrees(&map, 23, 3136));
        assert!(!cadence.agrees(&map, 24, 3136));
        assert!(cadence.capture(0, 1, &mut total).is_err());
        assert!(cadence
            .capture(1, graph::MAX_LIST as i64 + 1, &mut total)
            .is_err());
        assert_eq!(cadence.decode_in(&cadence.encode_in(23).unwrap()), Some(23));
        assert_eq!(cadence.decode_in("23"), None);
    }

    #[test]
    fn slower_picture_repeats_on_its_exact_frame_boundaries() {
        let cadence = Cadence::between(crate::prepare::rate(), Rational::integer(24)).unwrap();
        let mut total = 0;
        let map = cadence.capture(24, 3136, &mut total).unwrap();
        assert_eq!(
            (map[0], map[977], map[978], map[1978], map[1979]),
            (23, 1000, 1000, 2000, 2000)
        );
        assert!(cadence.agrees(&map, 24, 3136));
        assert_eq!(cadence.last_frame(24, 3136), map.last().copied());
        assert_eq!(cadence.decode_in(&cadence.encode_in(24).unwrap()), Some(24));
        assert_eq!(
            Cadence::DropFrames.decode_in(&cadence.encode_in(24).unwrap()),
            None
        );
        let zero = cadence.capture(0, 1003, &mut total).unwrap();
        assert_eq!(
            (&zero[..3], &zero[1000..]),
            (&[0, 0, 1][..], &[999, 1000, 1000][..])
        );
        assert_eq!(cadence.decode_in(&cadence.encode_in(0).unwrap()), Some(0));
        assert_eq!(
            Cadence::DropFrames.decode_in(&cadence.encode_in(0).unwrap()),
            None
        );
    }
}
