//! Validators (port of `py_premiere/models/validators.py` and `time.validate_time`).
//!
//! Python's validators also refuse values of the wrong *type* (`TypeError`); Rust's types rule
//! most of those out, so the functions here take typed values. Where a Python setter accepts
//! "an int or a float" the Rust API takes a [`Num`], which keeps the distinction Python's
//! messages and integer-only validators depend on.
//!
//! Messages are Python's, character for character (`must be >= 0, got -1`).

use std::cmp::Ordering;
use std::fmt;
use std::path::Path;

use crate::convert::model::py_str_repr;
use crate::error::{Error, Result};

/// A Python number: an `int` or a `float`. `Int` covers every value py_premiere's validators
/// and setters accept (up to `2**64 - 1` for packed colours).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    /// A Python `int`.
    Int(i128),
    /// A Python `float`.
    Float(f64),
}

impl Num {
    /// The value as a float (Python `float(value)`; ints round to nearest).
    pub fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    /// Whether this is an `int`.
    pub fn is_int(self) -> bool {
        matches!(self, Num::Int(_))
    }

    /// Python `type(value).__name__`.
    pub fn type_name(self) -> &'static str {
        match self {
            Num::Int(_) => "int",
            Num::Float(_) => "float",
        }
    }

    /// Exact comparison, as Python compares an int with a float. `None` when a NaN is involved.
    pub fn py_cmp(self, other: Num) -> Option<Ordering> {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Some(a.cmp(&b)),
            (Num::Float(a), Num::Float(b)) => a.partial_cmp(&b),
            (Num::Int(a), Num::Float(b)) => cmp_int_float(a, b),
            (Num::Float(a), Num::Int(b)) => cmp_int_float(b, a).map(Ordering::reverse),
        }
    }
}

impl From<f64> for Num {
    fn from(v: f64) -> Num {
        Num::Float(v)
    }
}

impl From<i64> for Num {
    fn from(v: i64) -> Num {
        Num::Int(v as i128)
    }
}

/// Python `str(value)`.
impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Num::Int(i) => write!(f, "{i}"),
            Num::Float(v) => f.write_str(&crate::build::fmt::py_repr(*v)),
        }
    }
}

fn cmp_int_float(i: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    const LIMIT: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0; // 2^127
    if f >= LIMIT {
        return Some(Ordering::Less);
    }
    if f < -LIMIT {
        return Some(Ordering::Greater);
    }
    let t = f.trunc();
    let ti = t as i128;
    if i != ti {
        return Some(i.cmp(&ti));
    }
    Some(if f > t {
        Ordering::Less
    } else if f < t {
        Ordering::Greater
    } else {
        Ordering::Equal
    })
}

/// The bounds of one of Python's `_validate_number(...)` validators.
#[derive(Clone, Copy, Debug)]
pub struct NumberRule {
    /// Minimum (inclusive unless `exclusive_min`).
    pub min: Option<Num>,
    /// Maximum (inclusive).
    pub max: Option<Num>,
    /// Refuse non-`int` values.
    pub integer: bool,
    /// The value must be strictly greater than `min`.
    pub exclusive_min: bool,
}

impl NumberRule {
    /// Python's validator call: `TypeError` for a float where an int is required, `ValueError`
    /// for a non-finite float or a value out of bounds.
    pub fn check(&self, value: Num) -> Result<()> {
        if self.integer && !value.is_int() {
            return Err(Error::Type(format!("expected an integer, got {}", value.type_name())));
        }
        if let Num::Float(f) = value {
            if !f.is_finite() {
                return Err(Error::Value(format!("must be a finite number, got {value}")));
            }
        }
        if let Some(lo) = self.min {
            let ord = value.py_cmp(lo);
            if self.exclusive_min && ord != Some(Ordering::Greater) {
                return Err(Error::Value(format!("must be > {lo}, got {value}")));
            }
            if !self.exclusive_min && ord == Some(Ordering::Less) {
                return Err(Error::Value(format!("must be >= {lo}, got {value}")));
            }
        }
        if let Some(hi) = self.max {
            if value.py_cmp(hi) == Some(Ordering::Greater) {
                return Err(Error::Value(format!("must be <= {hi}, got {value}")));
            }
        }
        Ok(())
    }
}

const fn rule(min: Option<Num>, max: Option<Num>, integer: bool) -> NumberRule {
    NumberRule { min, max, integer, exclusive_min: false }
}

/// `FLOAT32_MAX`: the largest magnitude a float32 param value can hold.
pub const FLOAT32_MAX: f64 = 3.4028234663852886e38;

/// `validate_number`: any finite number.
pub const NUMBER: NumberRule = rule(None, None, false);
/// `validate_positive_number`: `>= 0.0`.
pub const POSITIVE_NUMBER: NumberRule = rule(Some(Num::Float(0.0)), None, false);
/// `validate_int`.
pub const INT: NumberRule = rule(None, None, true);
/// `validate_positive_int`: an int `>= 0`.
pub const POSITIVE_INT: NumberRule = rule(Some(Num::Int(0)), None, true);
/// `validate_track_count`: an int `>= 1`.
pub const TRACK_COUNT: NumberRule = rule(Some(Num::Int(1)), None, true);
/// `validate_color_label`: an int in `0..=15`.
pub const COLOR_LABEL: NumberRule = rule(Some(Num::Int(0)), Some(Num::Int(15)), true);
/// `validate_marker_color_index`: an int in `0..=7`.
pub const MARKER_COLOR_INDEX: NumberRule = rule(Some(Num::Int(0)), Some(Num::Int(7)), true);
/// `validate_float32`: a number within float32 range.
pub const FLOAT32: NumberRule = rule(Some(Num::Float(-FLOAT32_MAX)), Some(Num::Float(FLOAT32_MAX)), false);
/// `validate_packed_color`: an int in `0..=2**64-1`.
pub const PACKED_COLOR: NumberRule = rule(Some(Num::Int(0)), Some(Num::Int(u64::MAX as i128)), true);

/// `validate_number(value)` for a float.
pub fn validate_number(value: f64) -> Result<()> {
    NUMBER.check(Num::Float(value))
}

/// `validate_positive_number(value)` for a float.
pub fn validate_positive_number(value: f64) -> Result<()> {
    POSITIVE_NUMBER.check(Num::Float(value))
}

/// `validate_string(allow_empty, max_length)`: text that can survive a save. Rust strings are
/// always valid UTF-8, so Python's unpaired-surrogate refusal cannot arise.
pub fn validate_string_with(value: &str, allow_empty: bool, max_length: Option<usize>) -> Result<()> {
    if !allow_empty && value.is_empty() {
        return Err(Error::Value("must not be empty".into()));
    }
    if let Some(max) = max_length {
        let n = value.chars().count();
        if n > max {
            return Err(Error::Value(format!("must be at most {max} characters, got {n}")));
        }
    }
    crate::build::marker::validate_string(value)
}

/// `validate_string()`.
pub fn validate_string(value: &str) -> Result<()> {
    validate_string_with(value, true, None)
}

/// `validate_string(allow_empty=False)`.
pub fn validate_nonempty_string(value: &str) -> Result<()> {
    validate_string_with(value, false, None)
}

/// `validate_one_of(allowed)`.
pub fn validate_one_of(value: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(Error::Value(format!("must be one of [{}], got {}", allowed.join(", "), py_str_repr(value))))
}

/// `validate_enum(Enum)` for a raw int: the member must exist.
pub fn validate_enum_value(value: i64, enum_name: &str, valid: bool) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Error::Value(format!("{value} is not a valid {enum_name} value")))
    }
}

/// What `validate_path(must_exist=...)` requires of the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathRule {
    /// `must_exist=True` (and `must_be_file`).
    MustExist {
        /// Also refuse an existing path that is not a regular file.
        must_be_file: bool,
    },
    /// `must_exist=False`: refuse a path that exists (`FileExistsError`).
    MustNotExist,
    /// `must_exist=None`.
    Any,
}

/// `validate_path(...)`. Messages print the path as `str(Path(value))` does.
pub fn validate_path(path: &Path, rule: PathRule) -> Result<()> {
    let shown = crate::model::project::py_path(&path.to_string_lossy());
    let shown = shown.display();
    match rule {
        PathRule::MustExist { must_be_file } => {
            if !path.exists() {
                return Err(Error::Value(format!("path does not exist: {shown}")));
            }
            if must_be_file && !path.is_file() {
                return Err(Error::Value(format!("path is not a file: {shown}")));
            }
        }
        PathRule::MustNotExist => {
            if path.exists() {
                return Err(Error::FileExists(format!("path already exists: {shown}")));
            }
        }
        PathRule::Any => {}
    }
    Ok(())
}

/// `validate_vector2` for two floats: both finite.
pub fn validate_vector2(value: [Num; 2]) -> Result<()> {
    for component in value {
        if let Num::Float(f) = component {
            if !f.is_finite() {
                return Err(Error::Value(format!("must be finite numbers, got {component}")));
            }
        }
    }
    Ok(())
}

/// `validate_color`: each channel 0..=255 (channels are checked in order red, green, blue,
/// alpha, as the `Color` tuple iterates).
pub fn validate_color(channels: [i64; 4]) -> Result<()> {
    for channel in channels {
        if !(0..=255).contains(&channel) {
            return Err(Error::Value(format!("channel out of range 0-255: {channel}")));
        }
    }
    Ok(())
}
