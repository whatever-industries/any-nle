//! Saved frame coordinates and decimal clip clocks.
use super::*;

const ADAPTER: &str = "resolve.fractional_timing";
pub(super) const SUBFRAME_LOSS: &str = "Resolve saved subframe edit has no qualified placement";

pub(super) struct FrameField {
    pub whole: i64,
    pub fraction: f64,
}

impl FrameField {
    pub fn parse(text: &str) -> Result<Self> {
        let Some((whole, suffix)) = text.split_once('|') else {
            return Ok(Self {
                whole: text.parse()?,
                fraction: 0.0,
            });
        };
        if suffix.len() != 16 {
            return Err("invalid Resolve frame fraction byte count".into());
        }
        let fraction = f64::from_le_bytes(
            hex::decode(suffix)?
                .try_into()
                .map_err(|_| "invalid Resolve frame fraction byte count")?,
        );
        // Native rounding residue can occur just below zero.
        if !(-1e-9..=1.0).contains(&fraction) {
            return Err("invalid Resolve frame fraction".into());
        }
        Ok(Self {
            whole: whole.parse()?,
            fraction,
        })
    }

    pub fn integer(&self) -> Option<i64> {
        (self.fraction == 0.0).then_some(self.whole)
    }
}

/// Record the native coordinate even when its clock interpretation or playback
/// is unqualified. The signed significand times two to the saved power is exact.
pub(super) fn observe_edit(
    graph: &mut SourceGraph,
    owner: &str,
    node: &Node,
    record_rate: Rational,
) -> Result<()> {
    let mut fields = Vec::new();
    for (tag, key) in [("Start", "start"), ("Duration", "duration"), ("In", "in")] {
        let Some(raw) = node.maybe(tag)?.filter(|value| value.contains('|')) else {
            continue;
        };
        let frame = FrameField::parse(raw)?;
        let (mantissa, power) =
            encoding::binary64_parts(frame.fraction.to_bits()).expect("parsed finite fraction");
        fields.push((
            key.into(),
            Value::Obj(vec![
                ("saved".into(), text(raw)),
                ("whole_frames".into(), Value::Int(frame.whole)),
                ("fraction_significand".into(), Value::Int(mantissa)),
                (
                    "fraction_binary_exponent".into(),
                    Value::Int(i64::from(power)),
                ),
            ]),
        ));
    }
    if fields.is_empty() {
        return Ok(());
    }
    let mut row = vec![
        ("occurrence".into(), text(node.id()?)),
        ("record_rate".into(), text(&record_rate.to_string())),
        ("fields".into(), Value::Obj(fields)),
    ];
    if let Some(target) = node.maybe("MediaRef")? {
        row.push(("media_ref".into(), text(target)));
    }
    observations::append(graph, ADAPTER, owner, Value::Obj(row));
    Ok(())
}

/// Compare with the known clock at the precision of the saved decimal. A
/// rounded value must still distinguish adjacent ticks of the owning clock.
pub(super) fn clock_matches(saved: &str, expected: Rational, tick_rate: Rational) -> Result<bool> {
    let (count, scale) = decimal(saved).ok_or("invalid saved Resolve decimal media clock")?;
    let overflow = || "Resolve decimal media clock exceeds comparison bounds";
    let saved_scaled = count
        .checked_mul(i128::from(expected.den()))
        .ok_or_else(overflow)?;
    let clock_scaled = i128::from(expected.num())
        .checked_mul(scale)
        .ok_or_else(overflow)?;
    let error = saved_scaled
        .checked_sub(clock_scaled)
        .ok_or_else(overflow)?
        .unsigned_abs();
    if error == 0 {
        return Ok(true);
    }
    if scale
        .checked_mul(i128::from(tick_rate.den()))
        .ok_or_else(overflow)?
        <= i128::from(tick_rate.num())
    {
        return Ok(false);
    }
    if error <= expected.den() as u128 / 2 {
        return Ok(true);
    }
    // A shortest string can represent the native double rather than the
    // mathematical fraction. Require the same double as the known clock.
    Ok(saved.parse::<f64>()? == expected.num() as f64 / expected.den() as f64)
}

fn decimal(saved: &str) -> Option<(i128, i128)> {
    let (mantissa, exponent) = match saved.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (saved, 0),
    };
    let (value, negative) = if let Some(value) = mantissa.strip_prefix('-') {
        (value, true)
    } else {
        (mantissa.strip_prefix('+').unwrap_or(mantissa), false)
    };
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let digits = whole.bytes().chain(fraction.bytes());
    let len = whole.len().checked_add(fraction.len())?;
    if !(1..=36).contains(&len) || !digits.clone().all(|n| n.is_ascii_digit()) {
        return None;
    }
    let magnitude = digits.fold(0i128, |n, digit| n * 10 + i128::from(digit - b'0'));
    let count = if negative { -magnitude } else { magnitude };
    let places = (fraction.len() as i32).checked_sub(exponent)?;
    let power = 10i128.checked_pow(places.unsigned_abs())?;
    if places >= 0 {
        Some((count, power))
    } else {
        Some((count.checked_mul(power)?, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_precision_checks_the_known_tick() {
        let rate = Rational::integer(24);
        let expected = Rational::new(7, 24).unwrap();
        assert!(clock_matches("0.291667", expected, rate).unwrap());
        assert!(clock_matches("2.91667E-1", expected, rate).unwrap());
        assert!(clock_matches("0.2916666666666667", expected, rate).unwrap());
        assert!(!clock_matches("0.291668", expected, rate).unwrap());
        assert!(!clock_matches("0.333333", expected, rate).unwrap());
        assert!(!clock_matches("0.3", expected, rate).unwrap());
        assert!(clock_matches("0.2917", expected, rate).unwrap());
        assert!(!clock_matches("0.2917", expected, Rational::integer(48000)).unwrap());
        let samples = Rational::integer(48000);
        let sample_start = Rational::new(91, 48000).unwrap();
        assert!(clock_matches("0.001896", sample_start, samples).unwrap());
        assert!(!clock_matches("0.001917", sample_start, samples).unwrap());
        assert!(clock_matches("NaN", expected, rate).is_err());
    }

    #[test]
    fn frame_fraction_retains_signed_binary64_and_rejects_bad_payloads() {
        let frame = FrameField::parse("7|000000000000e03f").unwrap();
        assert_eq!(
            (frame.whole, frame.fraction, frame.integer()),
            (7, 0.5, None)
        );
        assert_eq!(
            encoding::binary64_parts(frame.fraction.to_bits()),
            Some((1, -1))
        );
        assert_eq!(
            FrameField::parse("7|0000000000000000").unwrap().integer(),
            Some(7)
        );
        assert!(FrameField::parse("7|000000000000f87f").is_err());
        assert!(FrameField::parse("7|0012").is_err());
    }

    #[test]
    fn fractional_group_duration_has_an_exact_source_extent() {
        let mut extent = 3600.0f64.to_le_bytes().to_vec();
        extent.extend_from_slice(&1.03125f64.to_le_bytes());
        let node = xml::parse(
            &format!(
                "<Sm2Sequence><FrameRate>00000000000038400000000000000000</FrameRate>\
             <Resolution>00000000000007800000000000000438</Resolution>\
             <MediaExtents>{}</MediaExtents></Sm2Sequence>",
                hex::encode(extent),
            ),
            &mut 0,
        )
        .unwrap();
        assert!(Grid::read(&node).is_err());
        let (grid, duration) = Grid::read_with_fractional_duration(&node).unwrap();
        assert_eq!((grid.origin, grid.length), (86400, 24));
        assert_eq!(duration, Rational::new(33, 32));
    }
}
