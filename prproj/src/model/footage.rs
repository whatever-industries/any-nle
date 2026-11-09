//! `FootageInterpretation` (port of `py_premiere/models/project_item.py`): how a clip item's
//! footage is interpreted, backed by the media's `VideoStream` (`Is*Overridden` flag plus
//! `Overridden*` value; note Premiere's own `OveriddenFrameRate` spelling). A frame-rate
//! override also rewrites the media source's `OriginalDuration`.
//!
//! The struct holds the two element handles; reads take the [`Tree`], writes `&mut Tree`.

use super::enums::{AlphaUsage, VideoFieldType};
use super::project::py_int;
use super::validators::{Num, NUMBER};
use crate::build::time::Time;
use crate::error::{type_err, value, Result};
use crate::xml::mutations::{append_leaf, insert_before, remove_child};
use crate::xml::{NodeId, Tree};

/// `_VIDEO_STREAM_ORDER`: the schema order of `VideoStream` children.
pub const VIDEO_STREAM_ORDER: [&str; 24] = [
    "FrameRate",
    "Duration",
    "OverriddenPAR",
    "OverriddenFieldType",
    "FrameRect",
    "CodecType",
    "IsStill",
    "IsFrameRateOverridden",
    "OveriddenFrameRate",
    "OriginalColorSpace",
    "IsFrameRectOverridden",
    "OverriddenFrameRect",
    "IsPAROverridden",
    "AlphaType",
    "IsAlphaTypeOverridden",
    "OverriddenAlphaType",
    "IsIgnoreAlphaOverridden",
    "OverriddenIgnoreAlpha",
    "IsInvertAlphaOverridden",
    "OverriddenInvertAlpha",
    "IsFieldTypeOverridden",
    "AlphaInfoIsUncertain",
    "FieldTypeIsUncertain",
    "OriginalImageOrientationType",
];

/// `_PAR_MAX_DENOMINATOR`.
const PAR_MAX_DENOMINATOR: i128 = 1000;

/// What `pixel_aspect_ratio = value` is given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParInput {
    /// `None`: clear the override.
    Clear,
    /// A ratio (`int` or `float`), converted to the nearest exact fraction.
    Ratio(Num),
    /// The exact `(numerator, denominator)` pair.
    Pair(Num, Num),
}

/// A clip item's footage interpretation (`ProjectItem.footage_interpretation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FootageInterpretation {
    /// The media's `VideoStream` object.
    pub stream: NodeId,
    /// The `*MediaSource` object (whose `OriginalDuration` a rate override rewrites).
    pub source: Option<NodeId>,
}

fn findtext(tree: &Tree, element: NodeId, tag: &str) -> Option<String> {
    tree.child_by_tag(element, tag).map(|e| tree[e].text_or_empty().to_string())
}

fn int_or_zero(text: Option<String>) -> Result<i64> {
    match text {
        Some(t) if !t.is_empty() => py_int(&t),
        _ => Ok(0),
    }
}

impl FootageInterpretation {
    fn overridden(&self, tree: &Tree, flag: &str, value_tag: &str) -> Option<String> {
        if findtext(tree, self.stream, flag).as_deref() != Some("true") {
            return None;
        }
        // Python `findtext` of an absent value tag is None.
        findtext(tree, self.stream, value_tag)
    }

    fn set_leaf(&self, tree: &mut Tree, tag: &str, text: &str) -> Result<()> {
        if let Some(existing) = tree.child_by_tag(self.stream, tag) {
            tree.set_text(existing, Some(text));
            return Ok(());
        }
        let position = VIDEO_STREAM_ORDER.iter().position(|t| *t == tag).expect("ordered tag");
        let children = tree.children(self.stream).to_vec();
        for sibling in children {
            let sibling_tag = tree.tag(sibling).to_string();
            if let Some(p) = VIDEO_STREAM_ORDER.iter().position(|t| *t == sibling_tag) {
                if p > position {
                    let element = tree.new_element(tag);
                    tree.set_text(element, Some(text));
                    return insert_before(tree, self.stream, &sibling_tag, element);
                }
            }
        }
        append_leaf(tree, self.stream, tag, text);
        Ok(())
    }

    fn clear_leaf(&self, tree: &mut Tree, tag: &str) -> Result<()> {
        if let Some(existing) = tree.child_by_tag(self.stream, tag) {
            remove_child(tree, self.stream, existing)?;
        }
        Ok(())
    }

    /// `frame_rate`: effective ticks per frame.
    pub fn frame_rate(&self, tree: &Tree) -> Result<Time> {
        if let Some(o) = self.overridden(tree, "IsFrameRateOverridden", "OveriddenFrameRate") {
            return Ok(Time::new(py_int(&o)?));
        }
        Ok(Time::new(int_or_zero(findtext(tree, self.stream, "FrameRate"))?))
    }

    /// `frame_rate = value` (`None` or the native rate clears the override); rescales the
    /// source duration to the same frame count, except for stills.
    pub fn set_frame_rate(&self, tree: &mut Tree, rate: Option<Time>) -> Result<()> {
        if let Some(v) = rate {
            if v.ticks <= 0 {
                return value("frame rate must be positive ticks per frame");
            }
        }
        let native = int_or_zero(findtext(tree, self.stream, "FrameRate"))?;
        if native == 0 {
            return value("stream has no native frame rate");
        }
        let frames = self.frame_count(tree)?;
        match rate {
            Some(v) if v.ticks != native => {
                self.set_leaf(tree, "IsFrameRateOverridden", "true")?;
                self.set_leaf(tree, "OveriddenFrameRate", &v.ticks.to_string())?;
                if let Some(f) = frames {
                    self.set_duration(tree, f * v.ticks as i128)?;
                }
            }
            _ => {
                self.clear_leaf(tree, "IsFrameRateOverridden")?;
                self.clear_leaf(tree, "OveriddenFrameRate")?;
                if let Some(f) = frames {
                    self.set_duration(tree, f * native as i128)?;
                }
            }
        }
        Ok(())
    }

    fn frame_count(&self, tree: &Tree) -> Result<Option<i128>> {
        let Some(source) = self.source else { return value("item has no media source to rewrite") };
        let duration = int_or_zero(findtext(tree, source, "OriginalDuration"))? as i128;
        let rate = self.frame_rate(tree)?.ticks as i128;
        if rate == 0 {
            return value("stream has no frame rate");
        }
        let (frames, remainder) = (duration.div_euclid(rate), duration.rem_euclid(rate));
        if remainder == 0 {
            return Ok(Some(frames));
        }
        if findtext(tree, self.stream, "IsStill").as_deref() == Some("true") {
            return Ok(None);
        }
        value("media duration is not a whole number of frames")
    }

    fn set_duration(&self, tree: &mut Tree, ticks: i128) -> Result<()> {
        let element = self.source.and_then(|s| tree.child_by_tag(s, "OriginalDuration"));
        let Some(element) = element else { return value("media source has no OriginalDuration") };
        tree.set_text(element, Some(ticks.to_string()));
        Ok(())
    }

    /// `alpha_usage` (`NONE` unless overridden).
    pub fn alpha_usage(&self, tree: &Tree) -> Result<AlphaUsage> {
        match self.overridden(tree, "IsAlphaTypeOverridden", "OverriddenAlphaType") {
            Some(o) => AlphaUsage::from_value(py_int(&o)?),
            None => Ok(AlphaUsage::None),
        }
    }

    /// `alpha_usage = value`.
    pub fn set_alpha_usage(&self, tree: &mut Tree, usage: AlphaUsage) -> Result<()> {
        if usage == AlphaUsage::None {
            self.clear_leaf(tree, "IsAlphaTypeOverridden")?;
            return self.clear_leaf(tree, "OverriddenAlphaType");
        }
        self.set_leaf(tree, "IsAlphaTypeOverridden", "true")?;
        self.set_leaf(tree, "OverriddenAlphaType", &usage.value().to_string())
    }

    /// `alpha_usage = value` given a raw int, as Python behaves for one: validated, but never
    /// clearing - `value is AlphaUsage.NONE` is false for the int `0`, so the override is
    /// written (`OverriddenAlphaType` 0).
    pub fn set_alpha_usage_int(&self, tree: &mut Tree, value: i64) -> Result<()> {
        let usage = AlphaUsage::from_value(value)
            .map_err(|_| crate::Error::Value(format!("{value} is not a valid AlphaUsage value")))?;
        self.set_leaf(tree, "IsAlphaTypeOverridden", "true")?;
        self.set_leaf(tree, "OverriddenAlphaType", &usage.value().to_string())
    }

    fn native_ignore_alpha(&self, tree: &Tree) -> bool {
        findtext(tree, self.stream, "IgnoreAlpha").as_deref() == Some("true")
    }

    /// `ignore_alpha` (the native `IgnoreAlpha` unless overridden).
    pub fn ignore_alpha(&self, tree: &Tree) -> bool {
        match self.overridden(tree, "IsIgnoreAlphaOverridden", "OverriddenIgnoreAlpha") {
            Some(o) => o == "true",
            None => self.native_ignore_alpha(tree),
        }
    }

    /// `ignore_alpha = value` (the native answer drops the override).
    pub fn set_ignore_alpha(&self, tree: &mut Tree, ignore: bool) -> Result<()> {
        if ignore == self.native_ignore_alpha(tree) {
            self.clear_leaf(tree, "IsIgnoreAlphaOverridden")?;
            return self.clear_leaf(tree, "OverriddenIgnoreAlpha");
        }
        self.set_leaf(tree, "IsIgnoreAlphaOverridden", "true")?;
        self.set_leaf(tree, "OverriddenIgnoreAlpha", if ignore { "true" } else { "false" })
    }

    /// `invert_alpha`.
    pub fn invert_alpha(&self, tree: &Tree) -> bool {
        self.overridden(tree, "IsInvertAlphaOverridden", "OverriddenInvertAlpha").as_deref() == Some("true")
    }

    /// `invert_alpha = value`.
    pub fn set_invert_alpha(&self, tree: &mut Tree, invert: bool) -> Result<()> {
        if invert {
            self.set_leaf(tree, "IsInvertAlphaOverridden", "true")?;
            self.set_leaf(tree, "OverriddenInvertAlpha", "true")
        } else {
            self.clear_leaf(tree, "IsInvertAlphaOverridden")?;
            self.clear_leaf(tree, "OverriddenInvertAlpha")
        }
    }

    /// `field_type` (`DEFAULT` unless overridden).
    pub fn field_type(&self, tree: &Tree) -> Result<VideoFieldType> {
        match self.overridden(tree, "IsFieldTypeOverridden", "OverriddenFieldType") {
            Some(o) => VideoFieldType::from_value(py_int(&o)?),
            None => Ok(VideoFieldType::Default),
        }
    }

    /// `field_type = value`.
    pub fn set_field_type(&self, tree: &mut Tree, field: VideoFieldType) -> Result<()> {
        if field == VideoFieldType::Default {
            self.clear_leaf(tree, "IsFieldTypeOverridden")?;
            return self.clear_leaf(tree, "OverriddenFieldType");
        }
        self.set_leaf(tree, "OverriddenFieldType", &field.value().to_string())?;
        self.set_leaf(tree, "IsFieldTypeOverridden", "true")
    }

    /// `field_type = value` given a raw int (Python's `is VideoFieldType.DEFAULT` is false for
    /// the int `-1`, so the override is written rather than cleared).
    pub fn set_field_type_int(&self, tree: &mut Tree, value: i64) -> Result<()> {
        let field = VideoFieldType::from_value(value)
            .map_err(|_| crate::Error::Value(format!("{value} is not a valid VideoFieldType value")))?;
        self.set_leaf(tree, "OverriddenFieldType", &field.value().to_string())?;
        self.set_leaf(tree, "IsFieldTypeOverridden", "true")
    }

    /// `pixel_aspect_ratio`: the override, else the native `OriginalPAR`, else 1.0.
    pub fn pixel_aspect_ratio(&self, tree: &Tree) -> Result<f64> {
        let stored = match self.overridden(tree, "IsPAROverridden", "OverriddenPAR") {
            Some(s) => Some(s),
            None => findtext(tree, self.stream, "OriginalPAR"),
        };
        if let Some((n, d)) = stored.as_deref().and_then(|s| s.split_once(',')) {
            let (n, d) = (py_int(n)?, py_int(d)?);
            if d == 0 {
                return Err(crate::Error::ZeroDivision("division by zero".into()));
            }
            // Python `int / int` is correctly rounded; so is this for operands below 2^53.
            return Ok(n as f64 / d as f64);
        }
        Ok(1.0)
    }

    /// `pixel_aspect_ratio = value`.
    pub fn set_pixel_aspect_ratio(&self, tree: &mut Tree, input: ParInput) -> Result<()> {
        match input {
            ParInput::Clear => {
                self.clear_leaf(tree, "IsPAROverridden")?;
                self.clear_leaf(tree, "OverriddenPAR")
            }
            ParInput::Ratio(v) => {
                NUMBER.check(v)?;
                if v.py_cmp(Num::Int(0)) != Some(std::cmp::Ordering::Greater) {
                    return value("pixel aspect ratio must be positive");
                }
                let (n, d) = limit_denominator(v, PAR_MAX_DENOMINATOR);
                self.write_par(tree, n, d)
            }
            ParInput::Pair(n, d) => {
                super::validators::validate_vector2([n, d])?;
                let (Num::Int(n), Num::Int(d)) = (n, d) else {
                    return type_err("pixel aspect ratio must be two integers");
                };
                if n <= 0 || d <= 0 {
                    return value("pixel aspect ratio must be positive");
                }
                self.write_par(tree, n, d)
            }
        }
    }

    fn write_par(&self, tree: &mut Tree, numerator: i128, denominator: i128) -> Result<()> {
        let native = findtext(tree, self.stream, "OriginalPAR").filter(|t| !t.is_empty()).unwrap_or_else(|| "1,1".into());
        if let Some((a, b)) = native.split_once(',') {
            let (a, b) = (py_int(a)? as i128, py_int(b)? as i128);
            if numerator * b == a * denominator {
                self.clear_leaf(tree, "IsPAROverridden")?;
                return self.clear_leaf(tree, "OverriddenPAR");
            }
        }
        self.set_leaf(tree, "OverriddenPAR", &format!("{numerator},{denominator}"))?;
        self.set_leaf(tree, "IsPAROverridden", "true")
    }

    /// `repr(footage_interpretation)`.
    pub fn repr(&self, tree: &Tree) -> Result<String> {
        Ok(format!(
            "FootageInterpretation(alpha_usage={}, field_type={})",
            self.alpha_usage(tree)?.name(),
            self.field_type(tree)?.name()
        ))
    }
}

/// `Fraction(value).limit_denominator(max_denominator)` -> `(numerator, denominator)`.
fn limit_denominator(value: Num, max_denominator: i128) -> (i128, i128) {
    // Fraction(value): an int is n/1; a float is its exact binary value.
    let (mut p, mut q) = match value {
        Num::Int(i) => (i, 1i128),
        Num::Float(f) => float_as_ratio(f),
    };
    let g = gcd(p.abs(), q);
    p /= g;
    q /= g;
    if q <= max_denominator {
        return (p, q);
    }
    // CPython's algorithm (fractions.Fraction.limit_denominator).
    let (mut p0, mut q0, mut p1, mut q1) = (0i128, 1i128, 1i128, 0i128);
    let (mut n, mut d) = (p, q);
    loop {
        let a = n.div_euclid(d);
        let q2 = q0 + a * q1;
        if q2 > max_denominator {
            break;
        }
        (p0, q0, p1, q1) = (p1, q1, p0 + a * p1, q2);
        (n, d) = (d, n - a * d);
    }
    let k = (max_denominator - q0).div_euclid(q1);
    if 2 * d * (q0 + k * q1) <= q {
        (p1, q1)
    } else {
        (p0 + k * p1, q0 + k * q1)
    }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// The exact ratio of a finite float (`float.as_integer_ratio`), reduced to fit `i128`: the
/// pixel aspect ratios given here are small, so mantissa/2^k stays in range; a value whose
/// exponent would overflow is clamped (such ratios are refused upstream by Premiere anyway).
fn float_as_ratio(f: f64) -> (i128, i128) {
    let bits = f.to_bits();
    let sign = if bits >> 63 == 1 { -1i128 } else { 1 };
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = (bits & ((1u64 << 52) - 1)) as i128;
    let (m, e) = if exponent == 0 { (mantissa, -1074) } else { (mantissa | (1i128 << 52), exponent - 1075) };
    if m == 0 {
        return (0, 1);
    }
    let mut m = m;
    let mut e = e;
    while m % 2 == 0 && e < 0 {
        m /= 2;
        e += 1;
    }
    if e >= 0 {
        let shifted = if e > 70 { i128::MAX / 2 } else { m << e };
        (sign * shifted, 1)
    } else if -e > 120 {
        (sign * m, i128::MAX / 2)
    } else {
        (sign * m, 1i128 << (-e))
    }
}
