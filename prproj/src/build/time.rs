//! `Time` (port of `models/time.py`): a tick count, 254016000000 ticks per second.
//!
//! Python ints are unbounded; ticks here are `i64` (about 36 million hours at the tick rate),
//! with overflow-checked arithmetic that reports an error rather than wrapping.

use std::cmp::Ordering;
use std::ops::{Add, Neg, Sub};

use super::fmt::py_repr;
use crate::error::{value, Result};

/// Premiere's tick rate.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// ExtendScript's sentinel for an unset time (-400000 s).
pub const UNSET_TICKS: i64 = -400_000 * TICKS_PER_SECOND;

/// Python `round(x)` for a float: nearest integer, ties to even.
pub fn round_half_even(x: f64) -> Result<i64> {
    if !x.is_finite() {
        return value("cannot round a non-finite number");
    }
    let r = x.round_ties_even();
    if r >= -9.223_372_036_854_775_8e18 && r < 9.223_372_036_854_775_8e18 {
        Ok(r as i64)
    } else {
        value("number too large to express in ticks")
    }
}

/// Python `a // b` (floor division); `b` must be nonzero.
pub fn floor_div(a: i64, b: i64) -> Result<i64> {
    if b == 0 {
        return value("integer division or modulo by zero");
    }
    let q = a.checked_div(b).ok_or_else(|| crate::Error::Value("integer overflow".into()))?;
    let r = a - q * b;
    Ok(if r != 0 && ((r < 0) != (b < 0)) { q - 1 } else { q })
}

/// `_timebase_ticks`: ticks per frame, which must be positive.
fn timebase_ticks(timebase: i64) -> Result<i64> {
    if timebase <= 0 {
        return value("timebase must be a positive number of ticks per frame");
    }
    Ok(timebase)
}

/// `_nominal_fps`: `round(TICKS_PER_SECOND / timebase)`.
fn nominal_fps(timebase: i64) -> Result<i64> {
    round_half_even(TICKS_PER_SECOND as f64 / timebase as f64)
}

/// `_drop_count`: frame numbers dropped per minute (29.97 -> 2, 59.94 -> 4).
fn drop_count(timebase: i64) -> Result<i64> {
    let nominal = nominal_fps(timebase)?;
    if nominal % 30 != 0 || nominal * timebase == TICKS_PER_SECOND {
        return value("drop-frame timecode requires a 29.97/59.94 family timebase");
    }
    Ok(nominal / 15)
}

/// A time in ticks (Python `Time`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Time {
    /// The tick count.
    pub ticks: i64,
}

impl Time {
    /// `Time(ticks)`.
    pub const fn new(ticks: i64) -> Time {
        Time { ticks }
    }

    /// `Time.seconds`: ticks / TICKS_PER_SECOND.
    pub fn seconds(&self) -> f64 {
        crate::pynum::true_div(self.ticks as i128, TICKS_PER_SECOND as i128)
    }

    /// `Time.from_seconds`: rounded to the nearest tick (ties to even).
    pub fn from_seconds(seconds: f64) -> Result<Time> {
        if !seconds.is_finite() {
            return value(format!("must be a finite number, got {}", py_repr(seconds)));
        }
        let ticks = seconds * TICKS_PER_SECOND as f64;
        if !ticks.is_finite() {
            return value(format!("{} seconds is too large to express in ticks", py_repr(seconds)));
        }
        Ok(Time::new(round_half_even(ticks)?))
    }

    /// `Time.from_frames(frames, timebase)`; `timebase` is ticks per frame.
    pub fn from_frames(frames: i64, timebase: i64) -> Result<Time> {
        let tb = timebase_ticks(timebase)?;
        frames
            .checked_mul(tb)
            .map(Time::new)
            .ok_or_else(|| crate::Error::Value("integer overflow".into()))
    }

    /// `Time.to_frames(timebase)`: floored to a whole frame.
    pub fn to_frames(&self, timebase: i64) -> Result<i64> {
        floor_div(self.ticks, timebase_ticks(timebase)?)
    }

    /// `Time.from_timecode`: `HH:MM:SS:FF`, or `HH:MM:SS;FF` for drop-frame.
    pub fn from_timecode(timecode: &str, timebase: i64) -> Result<Time> {
        if timecode.is_empty() {
            return value("expected a non-empty string");
        }
        let bad = || crate::Error::Value(format!("not a HH:MM:SS:FF timecode: {}", py_str_repr(timecode)));
        let drop = timecode.contains(';');
        let normalized = timecode.replace(';', ":");
        let parts: Vec<&str> = normalized.split(':').collect();
        if parts.len() != 4 || !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            return Err(bad());
        }
        let mut fields = [0i64; 4];
        for (slot, part) in fields.iter_mut().zip(&parts) {
            *slot = part.parse().map_err(|_| bad())?;
        }
        let [hours, minutes, seconds, frames] = fields;
        let ticks = timebase_ticks(timebase)?;
        let nominal = nominal_fps(ticks)?;
        if minutes >= 60 || seconds >= 60 || frames >= nominal {
            return value(format!("timecode field out of range: {}", py_str_repr(timecode)));
        }
        let mut number = ((hours * 60 + minutes) * 60 + seconds) * nominal + frames;
        if drop {
            let dropped = drop_count(ticks)?;
            let total_minutes = hours * 60 + minutes;
            if seconds == 0 && minutes % 10 != 0 && frames < dropped {
                return value(format!("dropped frame number: {}", py_str_repr(timecode)));
            }
            number -= dropped * (total_minutes - total_minutes / 10);
        }
        Ok(Time::new(number * ticks))
    }

    /// `Time.to_timecode(timebase, drop_frame)`.
    pub fn to_timecode(&self, timebase: i64, drop_frame: bool) -> Result<String> {
        let ticks = timebase_ticks(timebase)?;
        if self.ticks < 0 {
            return value("cannot format a negative time as timecode");
        }
        let mut number = self.ticks / ticks;
        let nominal = nominal_fps(ticks)?;
        if nominal == 0 {
            return value("integer division or modulo by zero");
        }
        if drop_frame {
            let dropped = drop_count(ticks)?;
            let per_ten_minutes = nominal * 600 - dropped * 9;
            let per_minute = nominal * 60 - dropped;
            let (tens, rest) = (number / per_ten_minutes, number % per_ten_minutes);
            if rest > dropped {
                number += dropped * 9 * tens + dropped * ((rest - dropped) / per_minute);
            } else {
                number += dropped * 9 * tens;
            }
        }
        let (seconds_total, frames) = (number / nominal, number % nominal);
        let (minutes_total, seconds) = (seconds_total / 60, seconds_total % 60);
        let (hours, minutes) = (minutes_total / 60, minutes_total % 60);
        let sep = if drop_frame { ';' } else { ':' };
        Ok(format!("{hours:02}:{minutes:02}:{seconds:02}{sep}{frames:02}"))
    }

    /// `Time * int`.
    pub fn mul_int(self, factor: i64) -> Result<Time> {
        self.ticks
            .checked_mul(factor)
            .map(Time::new)
            .ok_or_else(|| crate::Error::Value("integer overflow".into()))
    }

    /// `Time * float`: `round(ticks * factor)`.
    pub fn mul_float(self, factor: f64) -> Result<Time> {
        Ok(Time::new(round_half_even(self.ticks as f64 * factor)?))
    }

    /// `Time // int` (floor division).
    pub fn floor_div(self, divisor: i64) -> Result<Time> {
        Ok(Time::new(floor_div(self.ticks, divisor)?))
    }

    /// `Time / number`: `round(ticks / divisor)`.
    pub fn div_float(self, divisor: f64) -> Result<Time> {
        if divisor == 0.0 {
            return value("float division by zero");
        }
        Ok(Time::new(round_half_even(self.ticks as f64 / divisor)?))
    }

    /// `abs(Time)`.
    pub fn abs(self) -> Time {
        Time::new(self.ticks.abs())
    }
}

/// Python `repr(str)` for the error messages (single quotes unless the text holds one).
fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

impl Add for Time {
    type Output = Time;
    fn add(self, other: Time) -> Time {
        Time::new(self.ticks + other.ticks)
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, other: Time) -> Time {
        Time::new(self.ticks - other.ticks)
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time::new(-self.ticks)
    }
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Time) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Time) -> Ordering {
        self.ticks.cmp(&other.ticks)
    }
}
