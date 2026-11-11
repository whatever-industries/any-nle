//! Port of `converter/src/avid2prproj/model.py`: the neutral edit model (`avid2prproj.model/1`),
//! the converter's input contract.
//!
//! All times are integer Premiere ticks (254016000000 per second); rates are exact fractions.
//! Loading is bounded and strict: unknown keys, wrong types, non-integer times and dangling
//! references are errors, not guesses. The validation order and the [`ModelError`] texts follow
//! the Python module exactly (the Python loader is the oracle, see `tests/convert_model.rs`).
//!
//! std-only: this file carries its own small JSON reader that reproduces what Python's
//! `json.loads(..., object_pairs_hook, parse_constant)` accepts and refuses, including the
//! `JSONDecodeError` texts (Python 3.14 wording), duplicate-key refusal and the `NaN`/`Infinity`
//! refusal. JSON object order is preserved everywhere (clips, options maps, columns, tone).
//!
//! Known differences from Python (none affect valid models written by the Avid reader):
//! - Python's dynamic typing quirks that would leak a non-int into a field (`"sample_rate": 48000.0`,
//!   `"bits": 16.0`, `"time_display": 100.0`) are accepted like Python but stored as integers;
//!   `true` where Python's `isinstance(x, int)` lets it through (track numbers) is stored as 1.
//! - Python would raise a bare `TypeError` on a few malformed shapes (an unhashable value used as a
//!   clip reference or `time_display`, a non-iterable `stereo`); here those are a [`ModelError`]
//!   whose text starts with `TypeError: `.
//! - A lone UTF-16 surrogate escape in a JSON string (Python keeps it) is refused.
//! - `\d` in the Python regexes also matches non-ASCII digits; here only ASCII digits match.
//! - Integers beyond `i64` in `speed_ratio` are refused (Python keeps them).

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use crate::pystr::py_strip;

pub const TICKS_PER_SECOND: i64 = 254_016_000_000;
pub const SCHEMA: &str = "avid2prproj.model/1";
pub const MAX_MODEL_BYTES: u64 = 16 << 20;
pub const MAX_ITEMS: usize = 100_000;
pub const BIN_MODES: [&str; 3] = ["separate", "grouped", "merged"];
pub const MARKER_OWNERS: [&str; 3] = ["sequence", "filler", "edit"];
pub const CURVE_NAMES: [&str; 4] = ["master", "red", "green", "blue"];
/// Premiere time display codes (docs/time-display-formats.md in the research repo).
pub const TIME_DISPLAY_CODES: [i64; 16] = [100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 200, 201];

/// Python `ModelError(ValueError)`: the model was refused; the text is the Python message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError(pub String);

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelError {}

impl From<ModelError> for crate::Error {
    fn from(e: ModelError) -> Self {
        crate::Error::Value(e.0)
    }
}

type R<T> = Result<T, ModelError>;

static EMPTY_ARR: Json = Json::Arr(Vec::new());
static EMPTY_STR: Json = Json::Str(String::new());

fn fail<T>(message: String) -> R<T> {
    Err(ModelError(message))
}

fn require(ok: bool, message: impl FnOnce() -> String) -> R<()> {
    if ok {
        Ok(())
    } else {
        fail(message())
    }
}

// --- Python text helpers ------------------------------------------------------------------------

/// Python 3.14 `str.isprintable()` for one character (table generated from Python itself by
/// `tools/make_printable_table.py`).
pub fn py_printable(c: char) -> bool {
    let u = c as u32;
    let table = super::printable::NOT_PRINTABLE;
    let i = table.partition_point(|&(_, end)| end < u);
    !(i < table.len() && table[i].0 <= u)
}

/// Python `repr(str)`.
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if py_printable(c) || c == ' ' => out.push(c),
            c => {
                let u = c as u32;
                if u < 0x100 {
                    out.push_str(&format!("\\x{u:02x}"));
                } else if u < 0x10000 {
                    out.push_str(&format!("\\u{u:04x}"));
                } else {
                    out.push_str(&format!("\\U{u:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

fn str_list_repr(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| py_str_repr(s)).collect::<Vec<_>>().join(", "))
}

fn tuple_repr(items: &[&str]) -> String {
    let parts: Vec<String> = items.iter().map(|s| py_str_repr(s)).collect();
    format!("({})", parts.join(", "))
}

// --- JSON ---------------------------------------------------------------------------------------

/// A parsed JSON value with Python's number model: `Int` is an exact integer, `Big` an integer
/// beyond `i128` (only its sign is kept), `Float` a double. Objects keep source order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i128),
    Big(bool),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "NoneType",
            Json::Bool(_) => "bool",
            Json::Int(_) | Json::Big(_) => "int",
            Json::Float(_) => "float",
            Json::Str(_) => "str",
            Json::Arr(_) => "list",
            Json::Obj(_) => "dict",
        }
    }

    /// Object member lookup (`None` when the key is absent; a `null` member is `Some(Null)`).
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

struct Scanner {
    s: Vec<char>,
    /// `json.loads` defaults (NaN/Infinity accepted, a repeated key keeps its last value) and
    /// Python's bare error messages, instead of the model loader's strictness.
    lenient: bool,
}

const UNSUPPORTED_SURROGATE: &str = "unsupported lone UTF-16 surrogate in a JSON string";

impl Scanner {
    fn err<T>(&self, msg: &str, pos: usize) -> R<T> {
        let before = &self.s[..pos.min(self.s.len())];
        let lineno = before.iter().filter(|&&c| c == '\n').count() + 1;
        let colno = match before.iter().rposition(|&c| c == '\n') {
            Some(i) => pos - i,
            None => pos + 1,
        };
        let prefix = if self.lenient { "" } else { "invalid JSON: " };
        fail(format!("{prefix}{msg}: line {lineno} column {colno} (char {pos})"))
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }

    fn word(&self, i: usize, w: &str) -> bool {
        w.chars().enumerate().all(|(k, c)| self.at(i + k) == Some(c))
    }

    fn string(&self, quote: usize) -> R<(String, usize)> {
        let begin = quote;
        let mut i = quote + 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.at(i) else { return self.err("Unterminated string starting at", begin) };
            match c {
                '"' => return Ok((out, i + 1)),
                '\\' => {
                    let Some(e) = self.at(i + 1) else { return self.err("Unterminated string starting at", begin) };
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let u = self.hex4(i + 1)?;
                            let mut next = i + 6;
                            let cp = if (0xd800..0xdc00).contains(&u) {
                                // A pair when a \uXXXX low surrogate follows.
                                if self.at(next) == Some('\\') && self.at(next + 1) == Some('u') && next + 6 < self.s.len() {
                                    let lo = self.hex4(next + 1)?;
                                    if (0xdc00..0xe000).contains(&lo) {
                                        next += 6;
                                        0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00)
                                    } else {
                                        return fail(UNSUPPORTED_SURROGATE.to_string());
                                    }
                                } else {
                                    return fail(UNSUPPORTED_SURROGATE.to_string());
                                }
                            } else if (0xdc00..0xe000).contains(&u) {
                                return fail(UNSUPPORTED_SURROGATE.to_string());
                            } else {
                                u
                            };
                            out.push(char::from_u32(cp).expect("scalar value"));
                            i = next;
                            continue;
                        }
                        _ => return self.err("Invalid \\escape", i),
                    }
                    i += 2;
                }
                c if (c as u32) < 0x20 => return self.err("Invalid control character at", i),
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
    }

    /// The four hex digits after the `u` at index `u` (Python: "Invalid \uXXXX escape" at the `u`).
    fn hex4(&self, u: usize) -> R<u32> {
        if u + 5 >= self.s.len() {
            return self.err("Invalid \\uXXXX escape", u);
        }
        let mut v = 0u32;
        for k in 1..=4 {
            match self.s[u + k].to_digit(16) {
                Some(d) => v = v * 16 + d,
                None => return self.err("Invalid \\uXXXX escape", u),
            }
        }
        Ok(v)
    }

    /// Python's `scan_once`; `Ok(None)` is its `StopIteration` (no value starts at `i`).
    fn value(&self, i: usize) -> R<Option<(Json, usize)>> {
        let Some(c) = self.at(i) else { return Ok(None) };
        match c {
            '"' => {
                let (s, end) = self.string(i)?;
                Ok(Some((Json::Str(s), end)))
            }
            '{' => self.object(i + 1).map(Some),
            '[' => self.array(i + 1).map(Some),
            'n' if self.word(i, "null") => Ok(Some((Json::Null, i + 4))),
            't' if self.word(i, "true") => Ok(Some((Json::Bool(true), i + 4))),
            'f' if self.word(i, "false") => Ok(Some((Json::Bool(false), i + 5))),
            'N' if self.word(i, "NaN") => self.constant(f64::NAN, "NaN", i),
            'I' if self.word(i, "Infinity") => self.constant(f64::INFINITY, "Infinity", i),
            '-' if self.word(i, "-Infinity") => self.constant(f64::NEG_INFINITY, "-Infinity", i),
            _ => Ok(self.number(i)),
        }
    }

    fn constant(&self, value: f64, name: &str, i: usize) -> R<Option<(Json, usize)>> {
        if self.lenient {
            Ok(Some((Json::Float(value), i + name.len())))
        } else {
            fail(format!("invalid JSON constant {name}"))
        }
    }

    fn number(&self, start: usize) -> Option<(Json, usize)> {
        let digit = |k: usize| self.at(k).is_some_and(|c| c.is_ascii_digit());
        let mut i = start;
        let neg = self.at(i) == Some('-');
        if neg {
            i += 1;
        }
        match self.at(i) {
            Some('1'..='9') => {
                while digit(i) {
                    i += 1;
                }
            }
            Some('0') => i += 1,
            _ => return None,
        }
        let mut is_float = false;
        if self.at(i) == Some('.') && digit(i + 1) {
            i += 2;
            while digit(i) {
                i += 1;
            }
            is_float = true;
        }
        if matches!(self.at(i), Some('e' | 'E')) {
            let mut k = i + 1;
            if matches!(self.at(k), Some('+' | '-')) {
                k += 1;
            }
            if digit(k) {
                while digit(k) {
                    k += 1;
                }
                i = k;
                is_float = true;
            }
        }
        let text: String = self.s[start..i].iter().collect();
        let v = if is_float {
            Json::Float(text.parse::<f64>().expect("validated float"))
        } else {
            match text.parse::<i128>() {
                Ok(n) => Json::Int(n),
                Err(_) => Json::Big(neg),
            }
        };
        Some((v, i))
    }

    fn object(&self, mut end: usize) -> R<(Json, usize)> {
        let mut pairs: Vec<(String, Json)> = Vec::new();
        let mut next = self.at(end);
        if next != Some('"') {
            end = self.ws(end);
            next = self.at(end);
            if next == Some('}') {
                return Ok((Json::Obj(pairs), end + 1));
            } else if next != Some('"') {
                return self.err("Expecting property name enclosed in double quotes", end);
            }
        }
        loop {
            let (key, e) = self.string(end)?;
            end = e;
            if self.at(end) != Some(':') {
                end = self.ws(end);
                if self.at(end) != Some(':') {
                    return self.err("Expecting ':' delimiter", end);
                }
            }
            end = self.ws(end + 1);
            let Some((value, e)) = self.value(end)? else { return self.err("Expecting value", end) };
            end = e;
            pairs.push((key, value));
            end = self.ws(end);
            let c = self.at(end);
            end += 1;
            if c == Some('}') {
                break;
            } else if c != Some(',') {
                return self.err("Expecting ',' delimiter", end - 1);
            }
            let comma = end - 1;
            end = self.ws(end);
            let c = self.at(end);
            end += 1;
            if c != Some('"') {
                if c == Some('}') {
                    return self.err("Illegal trailing comma before end of object", comma);
                }
                return self.err("Expecting property name enclosed in double quotes", end - 1);
            }
            end -= 1;
        }
        if self.lenient {
            // dict semantics: a repeated key keeps its first position and its last value.
            let mut unique: Vec<(String, Json)> = Vec::with_capacity(pairs.len());
            for (k, v) in pairs {
                match unique.iter_mut().find(|(seen, _)| *seen == k) {
                    Some(slot) => slot.1 = v,
                    None => unique.push((k, v)),
                }
            }
            return Ok((Json::Obj(unique), end));
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for (k, _) in &pairs {
            if !seen.insert(k) {
                return fail(format!("duplicate JSON key {}", py_str_repr(k)));
            }
        }
        Ok((Json::Obj(pairs), end))
    }

    fn array(&self, mut end: usize) -> R<(Json, usize)> {
        let mut items = Vec::new();
        end = self.ws(end);
        if self.at(end) == Some(']') {
            return Ok((Json::Arr(items), end + 1));
        }
        loop {
            let Some((value, e)) = self.value(end)? else { return self.err("Expecting value", end) };
            end = e;
            items.push(value);
            end = self.ws(end);
            let c = self.at(end);
            end += 1;
            if c == Some(']') {
                break;
            } else if c != Some(',') {
                return self.err("Expecting ',' delimiter", end - 1);
            }
            let comma = end - 1;
            end = self.ws(end);
            if self.at(end) == Some(']') {
                return self.err("Illegal trailing comma before end of array", comma);
            }
        }
        Ok((Json::Arr(items), end))
    }
}

/// `json.loads(text, object_pairs_hook=<refuse duplicates>, parse_constant=<refuse NaN...>)`,
/// with the `invalid JSON: ` prefix of the `JSONDecodeError` re-raise.
pub fn parse_json(text: &str) -> R<Json> {
    parse_json_with(text, false)
}

/// Plain `json.loads(text)`: NaN/Infinity accepted, repeated keys last-wins, Python's own
/// `JSONDecodeError` messages (no `invalid JSON: ` prefix).
pub fn parse_json_python(text: &str) -> R<Json> {
    parse_json_with(text, true)
}

fn parse_json_with(text: &str, lenient: bool) -> R<Json> {
    let scanner = Scanner { s: text.chars().collect(), lenient };
    if scanner.at(0) == Some('\u{feff}') {
        return scanner.err("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0);
    }
    let start = scanner.ws(0);
    let Some((value, end)) = scanner.value(start)? else { return scanner.err("Expecting value", start) };
    let end = scanner.ws(end);
    if end != scanner.s.len() {
        return scanner.err("Extra data", end);
    }
    Ok(value)
}

// --- the dataclasses ----------------------------------------------------------------------------

/// An exact rational rate (Python `Fraction`), always reduced, denominator positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fraction {
    pub num: i64,
    pub den: i64,
}

impl Fraction {
    pub fn new(num: i64, den: i64) -> Fraction {
        fn gcd(a: i64, b: i64) -> i64 {
            if b == 0 {
                a
            } else {
                gcd(b, a % b)
            }
        }
        let g = gcd(num.abs(), den.abs()).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        Fraction { num: sign * num / g, den: sign * den / g }
    }
}

impl fmt::Display for Fraction {
    /// Python `str(Fraction)`: `24` or `30000/1001`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

/// Ticks per frame/sample unit of a validated positive `rate` (Python `int(TICKS_PER_SECOND / rate)`).
/// The wide intermediate also permits an equivalent unreduced constructed fraction.
pub fn ticks_per_unit(rate: Fraction) -> i64 {
    (TICKS_PER_SECOND as i128 * rate.den as i128 / rate.num as i128) as i64
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoFile {
    pub path: String,
    pub codec: String,
    pub width: i64,
    pub height: i64,
    pub rate: Fraction,
    pub duration: i64,
    /// Media start timecode in ticks, from the MXF (Premiere media `<Start>`).
    pub start: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioFile {
    pub path: String,
    pub sample_rate: i64,
    pub duration: i64,
    pub bits: i64,
    pub avid_track: Option<i64>,
    pub start: i64,
    /// Avid master-clip gain on this track (PVOL), Premiere master-clip Audio Gain.
    pub gain_db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuxTimecode {
    /// Avid import-mob track label; Aux TC number = index - 2 is a hypothesis.
    pub track_index: i64,
    pub timecode: String,
    pub frames: i64,
    pub fps: i64,
}

/// Avid-side clip facts carried for metadata transfer (avid-reader `avid` block).
#[derive(Clone, Debug, PartialEq)]
pub struct AvidInfo {
    pub mob_id: String,
    pub columns: Vec<(String, String)>,
    pub color_rgb16: Option<[i64; 3]>,
    pub start_timecode: Option<String>,
    pub aux_timecodes: Vec<AuxTimecode>,
    pub source_file: Option<String>,
    pub source_path: Option<String>,
}

impl AvidInfo {
    pub fn column(&self, name: &str) -> Option<&str> {
        self.columns.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Explicit source speaker-group declaration. Channel order is the Avid group order,
/// not physical file order. A channel count alone never establishes this declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct MultichannelGroup {
    pub format: String,
    pub channels: Vec<i64>,
}
impl MultichannelGroup {
    pub fn width(&self) -> Option<usize> {
        match self.format.as_str() { "5.1" => Some(6), "7.1" => Some(8), _ => None }
    }
    /// Premiere's native 5.1 order is L,R,C,LFE,Ls,Rs. Avid's group declaration
    /// orders L,C,R,Ls,Rs,LFE. Adaptive 7.1 retains all eight Avid-ordered channels.
    pub fn premiere_channels(&self) -> Vec<i64> {
        if self.format == "5.1" && self.channels.len() == 6 {
            [0,2,1,5,3,4].into_iter().map(|i| self.channels[i]).collect()
        } else { self.channels.clone() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub material_umid: Option<String>,
    pub tape_name: Option<String>,
    pub video: Option<VideoFile>,
    pub audio: Vec<AudioFile>,
    pub avid: Option<AvidInfo>,
    /// Premiere TimeDisplay code for audio-only items (100 = 24 fps).
    pub time_display: Option<i64>,
    /// Avid stereo groups, by avid_track (left, right).
    pub stereo_pairs: Vec<(i64, i64)>,
    /// Optional extension; absent/empty preserves the original model/1 behavior.
    pub multichannel_groups: Vec<MultichannelGroup>,
    /// Avid clip rate (FPS); timebase for audio-only builds.
    pub rate: Option<Fraction>,
    /// Avid master-clip markers.
    pub markers: Vec<Marker>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VolumeKey {
    /// Clip source time, as Premiere keys clip effects (PRPROJ-0013).
    pub ticks: i64,
    pub db: f64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EventAudio {
    pub gain_db: f64,
    pub volume_db: Option<f64>,
    pub volume_keys: Vec<VolumeKey>,
}

/// Avid Color Correction as the reader interprets it (avid-reader src/color.rs).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ColorCorrection {
    /// Percent, 100 = unchanged.
    pub saturation: Option<f64>,
    /// Avid HSL Controls brightness/contrast/gain/gamma/setup, as stored.
    pub tone: Vec<(String, f64)>,
    /// master/red/green/blue, 0..1.
    pub curves: Vec<(String, Vec<(f64, f64)>)>,
    pub not_converted: Vec<String>,
}

/// Picture position, scale, rotation and crop (from Avid Resize / 3D Warp). Fractions are of the
/// frame, measured from its centre with x right and y down.
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    /// Where the source's anchor lands, sequence frame.
    pub position: (f64, f64),
    /// Percent, x and y.
    pub scale: (f64, f64),
    /// Degrees clockwise.
    pub rotation: f64,
    /// Pivot, source frame.
    pub anchor: (f64, f64),
    /// Left, top, right, bottom, of the source.
    pub crop: [f64; 4],
    pub not_converted: Vec<String>,
}

/// Avid AniMatte as the reader gives it: per shape, keys of (frame within the edit, points
/// `[x, y, in_x, in_y, out_x, out_y]`) as fractions of the source frame from its top-left,
/// y down, handles relative to the point.
#[derive(Clone, Debug, PartialEq)]
pub struct Matte {
    pub shapes: Vec<Vec<(i64, Vec<[f64; 6]>)>>,
    /// Avid Key Out.
    pub inverted: bool,
    /// Per shape, percent (100 when the reader omits it).
    pub opacities: Vec<f64>,
    /// Per shape, raw Avid values in stored order.
    pub feathers: Vec<(f64, f64)>,
    pub not_converted: Vec<String>,
}

/// One multichannel (stereo) edit: sequence audio track, clip audio tracks in channel order,
/// edit gain dB or None, per-channel pan percent or None.
#[derive(Clone, Debug, PartialEq)]
pub struct StereoEdit {
    pub audio_track: i64,
    pub channels: Vec<i64>,
    pub gain_db: Option<f64>,
    pub pans: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub clip: String,
    pub record_start: i64,
    pub source_in: i64,
    pub duration: i64,
    pub video_track: Option<i64>,
    pub audio_tracks: Vec<i64>,
    pub audio: EventAudio,
    /// Clip audio files (by avid_track) playing on each of audio_tracks; empty = the clip's
    /// first len(audio_tracks) files in model order.
    pub audio_channels: Vec<i64>,
    pub stereo: Vec<StereoEdit>,
    /// The edit's own Avid gain (dB) and pan (-100..100) per audio track; None = not set.
    pub audio_gains: Vec<Option<f64>>,
    pub audio_pans: Vec<Option<f64>>,
    /// Avid motion effect (KB AVB-0010): speed_ratio = timeline frames / source frames, negative =
    /// reverse; source_duration = the source span it plays. None = normal speed.
    pub speed_ratio: Option<(i64, i64)>,
    pub source_duration: Option<i64>,
    pub color_correction: Option<ColorCorrection>,
    pub transform: Option<Transform>,
    pub matte: Option<Matte>,
    /// Avid Timewarp as sampled by Avid (KB AVB-0010): the source frame shown at each record
    /// frame, in clip frames from the media start. None = no retiming.
    pub time_remap: Option<Vec<i64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackLevel {
    pub track: i64,
    pub db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioCrossfade {
    pub track: i64,
    /// Sequence ticks of the cut between two adjacent events on `track`.
    pub cut: i64,
    pub duration: i64,
    /// Ticks from the crossfade start to the cut (Premiere `Alignment`).
    pub cut_offset: i64,
    /// `constant_power` or `custom_fade`.
    pub style: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub name: String,
    pub comment: String,
    /// Clip markers: master-clip-relative ticks; sequence markers: sequence ticks.
    pub start: i64,
    pub duration: i64,
    pub user: String,
    /// Avid color name (16-name palette).
    pub color: String,
    /// Avid track it sits on ("V1", "A2"), None = the sequence's own.
    pub track: Option<String>,
    /// "sequence" (Avid's own sequence markers), "filler", or "edit" (on an edit; `event` indexes
    /// the sequence's events, `source` = master-clip-relative ticks, `inherited` = a copy of a
    /// marker the master clip carries).
    pub owner: String,
    pub event: Option<i64>,
    pub source: Option<i64>,
    pub inherited: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub name: String,
    pub rate: Fraction,
    pub width: i64,
    pub height: i64,
    pub events: Vec<Event>,
    pub markers: Vec<Marker>,
    pub track_levels: Vec<TrackLevel>,
    pub audio_crossfades: Vec<AudioCrossfade>,
    /// Absolute sequence zero point in Premiere ticks, independent of edit positions.
    pub start: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bin {
    pub name: String,
    pub clips: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// separate: linked source clips in the timeline; merged/grouped: the timeline uses the
    /// combined clip.
    pub bin_mode: String,
    /// Avid pans mono tracks odd-left/even-right by default; off keeps edits without an explicit
    /// Avid pan centred.
    pub match_avid_default_pan: bool,
    /// Where an edit's own marker and a source marker share a frame: "edit" (default) or "source".
    pub marker_conflict_winner: String,
    pub marker_conflict_loser_to_sequence: bool,
    /// Append "Avid: <user> · <color>" to Premiere marker comments.
    pub marker_avid_info: bool,
    /// Avid marker color name -> Premiere marker color index (0..7).
    pub marker_colors: Vec<(String, i64)>,
    /// Avid clip color "#RRGGBB" -> Premiere label index (0..15).
    pub clip_labels: Vec<(String, i64)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            bin_mode: "separate".to_string(),
            match_avid_default_pan: false,
            marker_conflict_winner: "edit".to_string(),
            marker_conflict_loser_to_sequence: false,
            marker_avid_info: false,
            marker_colors: Vec::new(),
            clip_labels: Vec::new(),
        }
    }
}

/// Python's `dict[str, Clip]`: insertion order kept, lookup by id.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClipMap {
    order: Vec<Clip>,
    index: HashMap<String, usize>,
}

impl ClipMap {
    pub fn get(&self, id: &str) -> Option<&Clip> {
        self.index.get(id).map(|&i| &self.order[i])
    }

    /// Mutable lookup (to edit a parsed model, e.g. in tests).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Clip> {
        self.index.get(id).map(|&i| &mut self.order[i])
    }

    pub fn contains_key(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Clips in model (insertion) order.
    pub fn values(&self) -> std::slice::Iter<'_, Clip> {
        self.order.iter()
    }

    /// `(id, clip)` pairs in model order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Clip)> {
        self.order.iter().map(|c| (c.id.as_str(), c))
    }

    fn insert(&mut self, clip: Clip) {
        self.index.insert(clip.id.clone(), self.order.len());
        self.order.push(clip);
    }
}

/// Clips in order, for building a model outside the parser (crates/bridge). Callers pass unique
/// ids (the parser refuses repeats; here a repeat would be kept twice, the lookup finding the later).
impl FromIterator<Clip> for ClipMap {
    fn from_iter<I: IntoIterator<Item = Clip>>(clips: I) -> Self {
        let mut map = ClipMap::default();
        for clip in clips {
            map.insert(clip);
        }
        map
    }
}

impl std::ops::Index<&str> for ClipMap {
    type Output = Clip;
    fn index(&self, id: &str) -> &Clip {
        self.get(id).unwrap_or_else(|| panic!("KeyError: {id:?}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub project_name: String,
    pub options: Options,
    pub clips: ClipMap,
    pub bins: Vec<Bin>,
    pub sequences: Vec<Sequence>,
}

// --- validation helpers -------------------------------------------------------------------------

/// A checked JSON object: member lookup like Python's `dict`.
struct Obj<'a>(&'a [(String, Json)]);

impl<'a> Obj<'a> {
    /// `o.get(key)` / `o[key]`: absent is `None`, a `null` member is `Some(&Json::Null)`.
    fn raw(&self, key: &str) -> Option<&'a Json> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// `o.get(key)` as a Python `None` check: absent and `null` are both `None`.
    fn opt(&self, key: &str) -> Option<&'a Json> {
        self.raw(key).filter(|v| !matches!(v, Json::Null))
    }

    fn has(&self, key: &str) -> bool {
        self.raw(key).is_some()
    }

    /// `o[key]` for a key `_keys` required.
    fn req(&self, key: &str) -> &'a Json {
        self.raw(key).unwrap_or_else(|| panic!("required key {key} checked by keys()"))
    }

    /// `o.get(key, default)`: the default only when absent.
    fn or<'b>(&self, key: &str, default: &'b Json) -> &'b Json
    where
        'a: 'b,
    {
        self.raw(key).unwrap_or(default)
    }
}

fn keys<'a>(obj: &'a Json, where_: &str, required: &[&str], optional: &[&str]) -> R<Obj<'a>> {
    let Json::Obj(pairs) = obj else { return fail(format!("{where_}: expected an object")) };
    let mut unknown: Vec<String> = pairs
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
        .collect();
    unknown.sort();
    require(unknown.is_empty(), || format!("{where_}: unknown keys {}", str_list_repr(&unknown)))?;
    let mut missing: Vec<String> =
        required.iter().filter(|k| !pairs.iter().any(|(pk, _)| pk == *k)).map(|k| k.to_string()).collect();
    missing.sort();
    require(missing.is_empty(), || format!("{where_}: missing keys {}", str_list_repr(&missing)))?;
    Ok(Obj(pairs))
}

pub(crate) fn check_text(s: &str, where_: &str, limit: usize, allow_empty: bool) -> R<()> {
    require((allow_empty || !py_strip(s).is_empty()) && s.chars().count() <= limit,
            || format!("{where_}: expected non-empty text"))?;
    require(!s.chars().any(|c| (c as u32) < 32 && c != '\t'), || format!("{where_}: control characters are not allowed"))
}

fn text(value: &Json, where_: &str, limit: usize, allow_empty: bool) -> R<String> {
    let Json::Str(s) = value else { return fail(format!("{where_}: expected non-empty text")) };
    check_text(s, where_, limit, allow_empty)?;
    Ok(s.clone())
}

fn text_default(value: &Json, where_: &str) -> R<String> {
    text(value, where_, 4096, false)
}

fn ticks(value: &Json, where_: &str, positive: bool) -> R<i64> {
    let (ok_type, n): (bool, Option<i128>) = match value {
        Json::Int(n) => (true, Some(*n)),
        Json::Big(_) => (true, None),
        _ => (false, None),
    };
    require(ok_type, || format!("{where_}: expected integer ticks"))?;
    let n = n.ok_or_else(|| ModelError(format!("{where_}: out of range")))?;
    check_ticks(n, where_, positive)?;
    Ok(n as i64)
}

pub(crate) fn check_ticks(n: i128, where_: &str, positive: bool) -> R<()> {
    require((if positive { n > 0 } else { n >= 0 }) && n <= 1i128 << 62,
            || format!("{where_}: out of range"))
}

/// `isinstance(value, int) and not isinstance(value, bool)`.
fn strict_int(v: &Json) -> Option<i128> {
    match v {
        Json::Int(n) => Some(*n),
        Json::Big(neg) => Some(if *neg { i128::MIN } else { i128::MAX }),
        _ => None,
    }
}

/// `isinstance(value, int)`: Python's bool is an int too (`true` counts as 1).
fn loose_int(v: &Json) -> Option<i128> {
    match v {
        Json::Bool(b) => Some(*b as i128),
        other => strict_int(other),
    }
}

/// `isinstance(value, (int, float)) and not isinstance(value, bool)` as a float for comparisons.
fn number(v: &Json) -> Option<f64> {
    match v {
        Json::Int(n) => Some(*n as f64),
        Json::Big(neg) => Some(if *neg { f64::NEG_INFINITY } else { f64::INFINITY }),
        Json::Float(f) => Some(*f),
        _ => None,
    }
}

fn int_in(v: &Json, low: i64, high: i64, strict: bool) -> Option<i64> {
    let n = if strict { strict_int(v) } else { loose_int(v) }?;
    (n >= low as i128 && n <= high as i128).then_some(n as i64)
}

fn int_range(value: &Json, where_: &str, low: i64, high: i64) -> R<i64> {
    int_in(value, low, high, true).ok_or_else(|| ModelError(format!("{where_}: expected an integer {low}..{high}")))
}

fn rate(value: &Json, where_: &str) -> R<Fraction> {
    let parsed = match value {
        Json::Str(s) => parse_rate_text(s),
        _ => None,
    };
    let (num, den) = match parsed {
        Some(p) => p,
        None => return fail(format!("{where_}: expected 'num/den'")),
    };
    let r = Fraction::new(num, den);
    check_rate(r, where_)?;
    Ok(r)
}

pub(crate) fn check_rate(r: Fraction, where_: &str) -> R<()> {
    require((1..=9_999_999).contains(&r.num) && (1..=9_999_999).contains(&r.den),
            || format!("{where_}: expected 'num/den'"))?;
    require(r.num >= r.den && r.num as i128 <= 1000 * r.den as i128, || format!("{where_}: rate out of range"))?;
    require((TICKS_PER_SECOND as i128 * r.den as i128) % r.num as i128 == 0,
            || format!("{where_}: rate does not divide the tick clock"))
}

/// `[1-9][0-9]{0,6}/[1-9][0-9]{0,6}` (fullmatch).
fn parse_rate_text(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once('/')?;
    let part = |p: &str| -> Option<i64> {
        let ok = (1..=7).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_digit()) && !p.starts_with('0');
        ok.then(|| p.parse().expect("digits"))
    };
    Some((part(a)?, part(b)?))
}

/// Python `PurePosixPath(p).is_absolute() or PureWindowsPath(p).is_absolute()`.
pub fn is_absolute_path(p: &str) -> bool {
    if p.starts_with('/') {
        return true;
    }
    // Windows (Python 3.13+ ntpath.isabs on the first three characters, `/` read as `\`).
    let head: Vec<char> = p.chars().take(3).map(|c| if c == '/' { '\\' } else { c }).collect();
    (head.len() == 3 && head[1] == ':' && head[2] == '\\') || (head.len() >= 2 && head[0] == '\\' && head[1] == '\\')
}

/// `PureWindowsPath(p).name` if `p` has a backslash, else `PurePosixPath(p).name`.
pub fn path_basename(p: &str) -> String {
    let last = |comps: &[&str]| -> String {
        comps.iter().rev().find(|c| !c.is_empty() && **c != ".").map(|c| c.to_string()).unwrap_or_default()
    };
    if p.contains('\\') {
        let n: String = p.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
        let (rest, unc) = if n.starts_with("\\\\") {
            (&n[2..], true)
        } else if n.chars().nth(1) == Some(':') {
            let skip = n.chars().next().map_or(0, |c| c.len_utf8()) + 1;
            (&n[skip..], false)
        } else {
            (&n[..], false)
        };
        let comps: Vec<&str> = rest.split('\\').collect();
        if unc {
            // The server and the share belong to the drive, not to the name.
            return if comps.len() > 2 { last(&comps[2..]) } else { String::new() };
        }
        last(&comps)
    } else {
        let comps: Vec<&str> = p.split('/').collect();
        last(&comps)
    }
}

fn abs_path(value: &Json, where_: &str) -> R<String> {
    let t = text(value, where_, 32768, false)?;
    require(is_absolute_path(&t), || format!("{where_}: media paths must be absolute"))?;
    Ok(t)
}

/// `re.search(r"A(\d{2})\.[^.]+\.mxf$", name, re.IGNORECASE)` -> the track number.
pub fn avid_track_from_name(name: &str) -> Option<i64> {
    let lower = name.to_lowercase();
    let body = lower.strip_suffix(".mxf")?;
    // body = <...>a<dd>.<id> with a non-empty, dot-free <id>.
    let (head, id) = body.rsplit_once('.')?;
    if id.is_empty() {
        return None;
    }
    let chars: Vec<char> = head.chars().collect();
    let n = chars.len();
    if n < 3 || chars[n - 3] != 'a' || !chars[n - 2].is_ascii_digit() || !chars[n - 1].is_ascii_digit() {
        return None;
    }
    Some((chars[n - 2].to_digit(10).unwrap() * 10 + chars[n - 1].to_digit(10).unwrap()) as i64)
}

fn video(obj: &Json, where_: &str) -> R<VideoFile> {
    let o = keys(obj, where_, &["path", "codec", "width", "height", "rate", "duration_ticks"], &["start_ticks"])?;
    let codec = match o.req("codec") {
        Json::Str(s) if s == "AVdn" || s == "apcs" => s.clone(),
        _ => return fail(format!("{where_}.codec: only 'AVdn' (Avid DNx) and 'apcs' (ProRes LT) OP-Atom MXF have a verified profile")),
    };
    let mut wh = [0i64; 2];
    for (slot, k) in wh.iter_mut().zip(["width", "height"]) {
        match int_in(o.req(k), 16, 16384, true) {
            Some(n) => *slot = n,
            None => return fail(format!("{where_}.{k}: out of range")),
        }
    }
    let r = rate(o.req("rate"), &format!("{where_}.rate"))?;
    let duration = ticks(o.req("duration_ticks"), &format!("{where_}.duration_ticks"), true)?;
    require(duration % ticks_per_unit(r) == 0, || format!("{where_}.duration_ticks: not a whole number of frames"))?;
    let path = abs_path(o.req("path"), &format!("{where_}.path"))?;
    let start = ticks(o.or("start_ticks", &Json::Int(0)), &format!("{where_}.start_ticks"), false)?;
    Ok(VideoFile { path, codec, width: wh[0], height: wh[1], rate: r, duration, start })
}

fn db_value(value: &Json, where_: &str, low: f64, high: f64) -> R<f64> {
    let Some(v) = number(value) else { return fail(format!("{where_}: expected a number of dB")) };
    check_db(v, where_, low, high)?;
    Ok(v)
}

pub(crate) fn check_db(v: f64, where_: &str, low: f64, high: f64) -> R<()> {
    require(low <= v && v <= high, || {
        format!("{where_}: out of range {}..{} dB", crate::build::fmt::py_repr(low), crate::build::fmt::py_repr(high))
    })
}

fn db(value: &Json, where_: &str) -> R<f64> {
    db_value(value, where_, -96.0, 96.0)
}

/// `value in (a, b, c)` for ints: Python's `==` also lets an integral float through.
fn int_equals_any(v: &Json, allowed: &[i64]) -> Option<i64> {
    let n = match v {
        Json::Int(n) => i64::try_from(*n).ok()?,
        Json::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => *f as i64,
        _ => return None,
    };
    allowed.contains(&n).then_some(n)
}

fn audio(obj: &Json, where_: &str) -> R<AudioFile> {
    let o = keys(obj, where_, &["path", "sample_rate", "duration_ticks", "bits"], &["avid_track", "start_ticks", "gain_db"])?;
    let Some(sample_rate) = int_equals_any(o.req("sample_rate"), &[44100, 48000, 96000]) else {
        return fail(format!("{where_}.sample_rate: unsupported"));
    };
    let Some(bits) = int_equals_any(o.req("bits"), &[16, 24]) else { return fail(format!("{where_}.bits: unsupported")) };
    let track = o.opt("avid_track");
    let mut avid_track = None;
    if let Some(t) = track {
        match int_in(t, 1, 64, true) {
            Some(n) => avid_track = Some(n),
            None => return fail(format!("{where_}.avid_track: expected 1..64")),
        }
    }
    let path = abs_path(o.req("path"), &format!("{where_}.path"))?;
    if track.is_none() {
        // Avid OP-Atom audio files are named <clip name, truncated>A01.<id>.mxf, A02., ... by track.
        avid_track = avid_track_from_name(&path_basename(&path));
    }
    let duration = ticks(o.req("duration_ticks"), &format!("{where_}.duration_ticks"), true)?;
    let start = ticks(o.or("start_ticks", &Json::Int(0)), &format!("{where_}.start_ticks"), false)?;
    let gain_db = db(o.or("gain_db", &Json::Float(0.0)), &format!("{where_}.gain_db"))?;
    Ok(AudioFile { path, sample_rate, duration, bits, avid_track, start, gain_db })
}

fn check_audio_tracks(files: &[AudioFile], where_: &str) -> R<()> {
    let mut tracks: Vec<i64> = files.iter().filter_map(|f| f.avid_track).collect();
    let unique: HashSet<i64> = tracks.iter().copied().collect();
    if unique.len() != tracks.len() {
        tracks.sort();
        let list: Vec<String> = tracks.iter().map(|t| t.to_string()).collect();
        return fail(format!("{where_}: duplicate Avid audio tracks [{}]", list.join(", ")));
    }
    Ok(())
}

/// The clip audio file playing on each of the event's audio tracks.
pub fn event_audio_files<'a>(clip: &'a Clip, event: &Event) -> R<Vec<&'a AudioFile>> {
    if event.audio_channels.is_empty() {
        return Ok(clip.audio.iter().take(event.audio_tracks.len()).collect());
    }
    event
        .audio_channels
        .iter()
        .map(|c| {
            clip.audio
                .iter()
                .find(|f| f.avid_track == Some(*c))
                .ok_or_else(|| ModelError(format!("KeyError: {c}")))
        })
        .collect()
}

/// Audio in Avid track order (A01 first), for combining a clip's video with its own audio into
/// one merged or grouped clip. Bin items and timeline placement keep model order. Refuses rather
/// than guesses when a track number is unknown.
pub fn audio_for_combined_clip(clip: &Clip) -> R<Vec<AudioFile>> {
    let files = &clip.audio;
    if files.len() > 1 {
        require(files.iter().all(|f| f.avid_track.is_some()), || {
            format!(
                "clip {}: every audio file needs an Avid track (avid_track or an A01-style name) to be combined",
                py_str_repr(&clip.name)
            )
        })?;
    }
    let mut sorted = files.clone();
    sorted.sort_by_key(|f| f.avid_track.unwrap_or(0));
    Ok(sorted)
}

fn is_timecode(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 11
        && b.iter().enumerate().all(|(i, c)| match i {
            2 | 5 => *c == b':',
            8 => *c == b':' || *c == b';',
            _ => c.is_ascii_digit(),
        })
}

fn timecode(value: &Json, where_: &str) -> R<String> {
    match value {
        Json::Str(s) if is_timecode(s) => Ok(s.clone()),
        _ => fail(format!("{where_}: expected HH:MM:SS:FF")),
    }
}

fn avid(obj: &Json, where_: &str) -> R<AvidInfo> {
    let o = keys(obj, where_, &["mob_id"], &["columns", "color_rgb16", "start", "aux_timecodes", "source_file", "source_path"])?;
    let empty = Json::Obj(Vec::new());
    let columns = o.or("columns", &empty);
    let pairs = match columns {
        Json::Obj(p) if p.len() <= 512 => p,
        _ => return fail(format!("{where_}.columns: expected an object of at most 512 columns")),
    };
    let mut cols = Vec::new();
    for (k, v) in pairs {
        let key = text(&Json::Str(k.clone()), &format!("{where_}.columns key"), 256, false)?;
        // Avid allows empty column values (Phase 21 "Shoot Date"); metadata skips them.
        let val = text(v, &format!("{where_}.columns[{}]", py_str_repr(k)), 4096, true)?;
        cols.push((key, val));
    }
    let mut color_rgb16 = None;
    if let Some(color) = o.opt("color_rgb16") {
        let ok = matches!(color, Json::Arr(a) if a.len() == 3);
        require(ok, || format!("{where_}.color_rgb16: expected [r, g, b]"))?;
        let Json::Arr(a) = color else { unreachable!() };
        let mut rgb = [0i64; 3];
        for (slot, c) in rgb.iter_mut().zip(a) {
            *slot = int_range(c, &format!("{where_}.color_rgb16"), 0, 65535)?;
        }
        color_rgb16 = Some(rgb);
    }
    let mut start_timecode = None;
    if let Some(start) = o.opt("start") {
        let st = keys(start, &format!("{where_}.start"), &["timecode", "frames", "fps"], &[])?;
        start_timecode = Some(timecode(st.req("timecode"), &format!("{where_}.start.timecode"))?);
    }
    let aux = o.or("aux_timecodes", &EMPTY_ARR);
    let aux = match aux {
        Json::Arr(a) if a.len() <= 64 => a,
        _ => return fail(format!("{where_}.aux_timecodes: expected a list")),
    };
    let mut auxes = Vec::new();
    for (i, a) in aux.iter().enumerate() {
        let w = format!("{where_}.aux_timecodes[{i}]");
        let a = keys(a, &w, &["track_index", "timecode", "start", "fps"], &["aux_tc_hypothesis", "position", "length"])?;
        let track_index = int_range(a.req("track_index"), &w, 0, 32767)?;
        let tc = timecode(a.req("timecode"), &w)?;
        let frames = int_range(a.req("start"), &w, 0, 1 << 32)?;
        let fps = int_range(a.req("fps"), &w, 1, 1000)?;
        auxes.push(AuxTimecode { track_index, timecode: tc, frames, fps });
    }
    let opt_text = |k: &str| -> R<Option<String>> {
        match o.opt(k) {
            Some(v) => text_default(v, &format!("{where_}.{k}")).map(Some),
            None => Ok(None),
        }
    };
    let mob_id = text_default(o.req("mob_id"), &format!("{where_}.mob_id"))?;
    let source_file = opt_text("source_file")?;
    let source_path = opt_text("source_path")?;
    Ok(AvidInfo { mob_id, columns: cols, color_rgb16, start_timecode, aux_timecodes: auxes, source_file, source_path })
}

fn is_umid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c))
}

fn clip(obj: &Json, where_: &str) -> R<Clip> {
    let o = keys(
        obj,
        where_,
        &["id", "name", "audio"],
        &["video", "material_umid", "tape_name", "avid", "time_display", "stereo_pairs", "multichannel_groups", "rate", "markers"],
    )?;
    let mut material_umid = None;
    if let Some(umid) = o.opt("material_umid") {
        require(matches!(umid, Json::Str(s) if is_umid(s)), || format!("{where_}.material_umid: expected 64 upper-case hex digits"))?;
        let Json::Str(s) = umid else { unreachable!() };
        material_umid = Some(s.clone());
    }
    let tape_name = match o.opt("tape_name") {
        Some(t) => Some(text_default(t, &format!("{where_}.tape_name"))?),
        None => None,
    };
    let audio_json = match o.req("audio") {
        Json::Arr(a) if a.len() <= 64 => a,
        _ => return fail(format!("{where_}.audio: expected a list of at most 64 files")),
    };
    let video = match o.opt("video") {
        Some(v) => Some(self::video(v, &format!("{where_}.video"))?),
        None => None,
    };
    let files: Vec<AudioFile> =
        audio_json.iter().enumerate().map(|(i, a)| audio(a, &format!("{where_}.audio[{i}]"))).collect::<R<_>>()?;
    check_audio_tracks(&files, &format!("{where_}.audio"))?;
    require(video.is_some() || !files.is_empty(), || format!("{where_}: a clip needs at least one media file"))?;
    let avid = match o.opt("avid") {
        Some(a) => Some(self::avid(a, &format!("{where_}.avid"))?),
        None => None,
    };
    let time_display = match o.opt("time_display") {
        None => None,
        Some(td) => {
            if matches!(td, Json::Arr(_) | Json::Obj(_)) {
                return fail("TypeError: unhashable type".to_string());
            }
            match int_equals_any(td, &TIME_DISPLAY_CODES) {
                Some(n) => Some(n),
                None => {
                    let codes: Vec<String> = TIME_DISPLAY_CODES.iter().map(|c| c.to_string()).collect();
                    return fail(format!("{where_}.time_display: expected one of [{}]", codes.join(", ")));
                }
            }
        }
    };
    let pairs_default = Json::Arr(Vec::new());
    let pairs_json = o.or("stereo_pairs", &pairs_default);
    let mut pairs = Vec::new();
    let pairs_ok = match pairs_json {
        Json::Arr(items) => items.iter().all(|p| match p {
            Json::Arr(p) if p.len() == 2 => {
                let a = int_in(&p[0], 1, 64, false);
                let b = int_in(&p[1], 1, 64, false);
                if let (Some(a), Some(b)) = (a, b) {
                    pairs.push((a, b));
                    true
                } else {
                    false
                }
            }
            _ => false,
        }),
        _ => false,
    };
    require(pairs_ok, || format!("{where_}.stereo_pairs: expected [[left, right], ...] Avid tracks"))?;
    let known: HashSet<Option<i64>> = files.iter().map(|f| f.avid_track).collect();
    require(pairs.iter().all(|(a, b)| known.contains(&Some(*a)) && known.contains(&Some(*b))), || {
        format!("{where_}.stereo_pairs: names an audio track the clip lacks")
    })?;
    let used: Vec<i64> = pairs.iter().flat_map(|(a, b)| [*a, *b]).collect();
    require(used.len() == used.iter().collect::<HashSet<_>>().len(), || format!("{where_}.stereo_pairs: a track is in two pairs"))?;
    let mut multichannel_groups = Vec::new();
    let group_values = match o.or("multichannel_groups", &EMPTY_ARR) {
        Json::Arr(a) if a.len() <= 32 => a,
        _ => return fail(format!("{where_}.multichannel_groups: expected at most 32 groups")),
    };
    let mut grouped: HashSet<i64> = used.iter().copied().collect();
    for (i, value) in group_values.iter().enumerate() {
        let w = format!("{where_}.multichannel_groups[{i}]");
        let g = keys(value, &w, &["format", "channels"], &[])?;
        let format = text(g.req("format"), &format!("{w}.format"), 16, false)?;
        let width = match format.as_str() { "5.1" => 6, "7.1" => 8, _ => return fail(format!("{w}.format: expected 5.1 or 7.1")) };
        let channels = match g.req("channels") {
            Json::Arr(a) if a.len() == width => a.iter().map(|v| int_in(v,1,64,false)).collect::<Option<Vec<_>>>(),
            _ => None,
        }.ok_or_else(|| ModelError(format!("{w}.channels: expected {width} Avid tracks")))?;
        require(channels.iter().all(|n| known.contains(&Some(*n)) && grouped.insert(*n)), || format!("{w}.channels: missing, repeated or overlapping member"))?;
        multichannel_groups.push(MultichannelGroup { format, channels });
    }
    let rate = match o.opt("rate") {
        Some(r) => Some(self::rate(r, &format!("{where_}.rate"))?),
        None => None,
    };
    let markers_json = match o.or("markers", &EMPTY_ARR) {
        Json::Arr(a) if a.len() <= MAX_ITEMS => a,
        _ => return fail(format!("{where_}.markers: expected a list")),
    };
    let id = text(o.req("id"), &format!("{where_}.id"), 256, false)?;
    let name = text_default(o.req("name"), &format!("{where_}.name"))?;
    let markers: Vec<Marker> =
        markers_json.iter().enumerate().map(|(i, m)| marker(m, &format!("{where_}.markers[{i}]"), false)).collect::<R<_>>()?;
    Ok(Clip { id, name, material_umid, tape_name, video, audio: files, avid, time_display, stereo_pairs: pairs, multichannel_groups, rate, markers })
}

fn event_audio(obj: &Json, where_: &str) -> R<EventAudio> {
    let o = keys(obj, where_, &[], &["gain_db", "volume_db", "volume_keys"])?;
    let keys_json = match o.or("volume_keys", &EMPTY_ARR) {
        Json::Arr(a) if a.len() <= 65536 => a,
        _ => return fail(format!("{where_}.volume_keys: expected a list")),
    };
    let mut parsed = Vec::new();
    for (i, k) in keys_json.iter().enumerate() {
        let w = format!("{where_}.volume_keys[{i}]");
        let ko = keys(k, &w, &["source_ticks", "db"], &[])?;
        let t = ticks(ko.req("source_ticks"), &format!("{w}.source_ticks"), false)?;
        let d = db_value(ko.req("db"), &format!("{w}.db"), -200.0, 24.0)?;
        parsed.push(VolumeKey { ticks: t, db: d });
    }
    require(parsed.windows(2).all(|w| w[0].ticks < w[1].ticks), || format!("{where_}.volume_keys: times must be strictly increasing"))?;
    require(!(!parsed.is_empty() && o.opt("volume_db").is_some()), || format!("{where_}: give volume_db or volume_keys, not both"))?;
    let gain_db = db(o.or("gain_db", &Json::Float(0.0)), &format!("{where_}.gain_db"))?;
    let volume_db = match o.opt("volume_db") {
        None => None,
        Some(v) => Some(db_value(v, &format!("{where_}.volume_db"), -200.0, 24.0)?),
    };
    Ok(EventAudio { gain_db, volume_db, volume_keys: parsed })
}

fn pan(value: &Json, where_: &str) -> R<f64> {
    match number(value) {
        Some(v) if (-100.0..=100.0).contains(&v) => Ok(v),
        _ => fail(format!("{where_}: expected a pan percent -100..100")),
    }
}

fn optional_list(value: &Json, length: usize, check: fn(&Json, &str) -> R<f64>, where_: &str) -> R<Vec<Option<f64>>> {
    let items = match value {
        Json::Arr(a) if a.len() == length => a,
        _ => return fail(format!("{where_}: expected {length} entries")),
    };
    items
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Json::Null => Ok(None),
            v => check(v, &format!("{where_}[{i}]")).map(Some),
        })
        .collect()
}

/// Iterating a JSON value like Python does (`for x in value`): list items, dict keys, str chars.
fn py_iter(v: &Json) -> R<Vec<Json>> {
    match v {
        Json::Arr(a) => Ok(a.clone()),
        Json::Obj(p) => Ok(p.iter().map(|(k, _)| Json::Str(k.clone())).collect()),
        Json::Str(s) => Ok(s.chars().map(|c| Json::Str(c.to_string())).collect()),
        other => fail(format!("TypeError: '{}' object is not iterable", other.type_name())),
    }
}

fn color_correction(obj: &Json, where_: &str) -> R<ColorCorrection> {
    let o = keys(obj, where_, &[], &["saturation", "tone", "curves", "not_converted"])?;
    let empty = Json::Obj(Vec::new());
    let tone_json = o.or("tone", &empty);
    let tone_pairs: &[(String, Json)] = match tone_json {
        Json::Obj(p) => p,
        _ => &[],
    };
    let tone_ok = matches!(tone_json, Json::Obj(_))
        && tone_pairs.iter().all(|(k, _)| ["hue", "brightness", "contrast", "gain", "gamma", "setup"].contains(&k.as_str()))
        && tone_pairs.iter().all(|(_, v)| number(v).is_some_and(|f| (-1000.0..=1000.0).contains(&f)));
    require(tone_ok, || format!("{where_}.tone: expected Avid control values"))?;
    let gamma = tone_pairs.iter().find(|(k, _)| k == "gamma").and_then(|(_, v)| number(v)).unwrap_or(1.0);
    require(gamma > 0.0, || format!("{where_}.tone.gamma: must be positive"))?;
    let sat = o.opt("saturation");
    let saturation = match sat {
        None => None,
        Some(s) => match number(s) {
            Some(f) if (0.0..=1000.0).contains(&f) => Some(f),
            _ => return fail(format!("{where_}.saturation: expected a percent")),
        },
    };
    let curves_json = o.or("curves", &empty);
    let curves_ok = matches!(curves_json, Json::Obj(p) if p.iter().all(|(k, _)| CURVE_NAMES.contains(&k.as_str())));
    require(curves_ok, || format!("{where_}.curves: keys from {}", tuple_repr(&CURVE_NAMES)))?;
    let Json::Obj(curve_pairs) = curves_json else { unreachable!() };
    let mut parsed = Vec::new();
    for name in CURVE_NAMES {
        let Some((_, pts)) = curve_pairs.iter().find(|(k, _)| k == name) else { continue };
        let mut points = Vec::new();
        let ok = match pts {
            Json::Arr(a) if (2..=32).contains(&a.len()) => a.iter().all(|p| match p {
                Json::Arr(p) if p.len() == 2 => match (number(&p[0]), number(&p[1])) {
                    (Some(x), Some(y)) if (-4.0..=4.0).contains(&x) && (-4.0..=4.0).contains(&y) => {
                        points.push((x, y));
                        true
                    }
                    _ => false,
                },
                _ => false,
            }),
            _ => false,
        };
        require(ok, || format!("{where_}.curves.{name}: expected 2..32 [x, y] points"))?;
        require(points.iter().enumerate().all(|(i, p)| points[..i].iter().all(|q| p.0 != q.0)),
                || format!("{where_}.curves.{name}: x coordinates must be distinct"))?;
        parsed.push((name.to_string(), points));
    }
    let nc_ok = match o.or("not_converted", &EMPTY_ARR) {
        Json::Arr(a) => a.iter().all(|n| matches!(n, Json::Str(_))),
        _ => false,
    };
    require(nc_ok, || format!("{where_}.not_converted: expected text"))?;
    let not_converted = match o.or("not_converted", &EMPTY_ARR) {
        Json::Arr(a) => a.iter().map(|n| if let Json::Str(s) = n { s.clone() } else { unreachable!() }).collect(),
        _ => unreachable!(),
    };
    let tone = tone_pairs.iter().map(|(k, v)| (k.clone(), number(v).expect("checked"))).collect();
    Ok(ColorCorrection { saturation, tone, curves: parsed, not_converted })
}

/// Python `_numbers`: `count` numbers within `low..=high`.
fn numbers(v: &Json, count: usize, low: f64, high: f64, range: &str, where_: &str) -> R<Vec<f64>> {
    let parsed: Option<Vec<f64>> = match v {
        Json::Arr(a) if a.len() == count => a.iter().map(|x| number(x).filter(|f| low <= *f && *f <= high)).collect(),
        _ => None,
    };
    match parsed {
        Some(p) => Ok(p),
        None => fail(format!("{where_}: expected {count} numbers {range}")),
    }
}

fn transform(obj: &Json, where_: &str) -> R<Transform> {
    let o = keys(obj, where_, &[], &["position", "scale", "rotation", "anchor", "crop", "not_converted"])?;
    let zero = Json::Int(0);
    let rotation = number(o.or("rotation", &zero)).filter(|r| (-32768.0..=32767.0).contains(r));
    require(rotation.is_some(), || format!("{where_}.rotation: expected degrees"))?;
    let nc_ok = match o.or("not_converted", &EMPTY_ARR) {
        Json::Arr(a) => a.iter().all(|n| matches!(n, Json::Str(_))),
        _ => false,
    };
    require(nc_ok, || format!("{where_}.not_converted: expected text"))?;
    let not_converted = match o.or("not_converted", &EMPTY_ARR) {
        Json::Arr(a) => a.iter().map(|n| if let Json::Str(s) = n { s.clone() } else { unreachable!() }).collect(),
        _ => unreachable!(),
    };
    let pair = |key: &str, default: i128, low: f64, high: f64, range: &str| -> R<(f64, f64)> {
        let d = Json::Arr(vec![Json::Int(default), Json::Int(default)]);
        let v = numbers(o.or(key, &d), 2, low, high, range, &format!("{where_}.{key}"))?;
        Ok((v[0], v[1]))
    };
    let position = pair("position", 0, -100.0, 100.0, "-100..100")?;
    let scale = pair("scale", 100, 0.0, 10000.0, "0..10000")?;
    let anchor = pair("anchor", 0, -100.0, 100.0, "-100..100")?;
    let no_crop = Json::Arr(vec![Json::Int(0); 4]);
    let c = numbers(o.or("crop", &no_crop), 4, -100.0, 100.0, "-100..100", &format!("{where_}.crop"))?;
    Ok(Transform { position, scale, rotation: rotation.unwrap(), anchor, crop: [c[0], c[1], c[2], c[3]], not_converted })
}

fn matte(obj: &Json, where_: &str) -> R<Matte> {
    let o = keys(obj, where_, &["shapes"], &["inverted", "not_converted"])?;
    let inverted = match o.or("inverted", &Json::Bool(false)) {
        Json::Bool(b) => *b,
        _ => return fail(format!("{where_}.inverted: expected true or false")),
    };
    let shapes_json = match o.req("shapes") {
        Json::Arr(a) if (1..=64).contains(&a.len()) => a,
        _ => return fail(format!("{where_}.shapes: expected 1..64 shapes")),
    };
    let nc_ok = match o.or("not_converted", &EMPTY_ARR) {
        Json::Arr(a) => a.iter().all(|n| matches!(n, Json::Str(_))),
        _ => false,
    };
    require(nc_ok, || format!("{where_}.not_converted: expected text"))?;
    let not_converted: Vec<String> = match o.or("not_converted", &EMPTY_ARR) {
        Json::Arr(a) => a.iter().map(|n| if let Json::Str(s) = n { s.clone() } else { unreachable!() }).collect(),
        _ => unreachable!(),
    };
    let mut shapes = Vec::new();
    let mut opacities = Vec::new();
    let mut feathers = Vec::new();
    for (i, shape) in shapes_json.iter().enumerate() {
        let so = keys(shape, &format!("{where_}.shapes[{i}]"), &["keyframes"], &["opacity", "feather"])?;
        let opacity = number(so.or("opacity", &Json::Int(100))).filter(|v| (0.0..=100.0).contains(v));
        require(opacity.is_some(), || format!("{where_}.shapes[{i}].opacity: expected a percent 0..100"))?;
        let no_feather = Json::Arr(vec![Json::Int(0), Json::Int(0)]);
        let feather: Option<Vec<f64>> = match so.or("feather", &no_feather) {
            Json::Arr(a) if a.len() == 2 => a.iter().map(|v| number(v).filter(|f| (0.0..=100000.0).contains(f))).collect(),
            _ => None,
        };
        require(feather.is_some(), || format!("{where_}.shapes[{i}].feather: expected two raw Avid values"))?;
        opacities.push(opacity.unwrap());
        let f = feather.unwrap();
        feathers.push((f[0], f[1]));
        let keys_json = match so.req("keyframes") {
            Json::Arr(a) if (1..=100000).contains(&a.len()) => a,
            _ => return fail(format!("{where_}.shapes[{i}].keyframes: expected 1.. keys")),
        };
        let mut parsed: Vec<(i64, Vec<[f64; 6]>)> = Vec::new();
        for (j, key) in keys_json.iter().enumerate() {
            let w = format!("{where_}.shapes[{i}].keyframes[{j}]");
            let ko = keys(key, &w, &["frame", "points"], &[])?;
            let frame = strict_int(ko.req("frame")).filter(|f| (0..(1i128 << 31)).contains(f) && parsed.last().is_none_or(|p| *f > p.0 as i128));
            require(frame.is_some(), || format!("{w}.frame: expected increasing frames from 0"))?;
            let points: Option<Vec<[f64; 6]>> = match ko.req("points") {
                Json::Arr(a) if (3..=4096).contains(&a.len()) && parsed.first().is_none_or(|p| p.1.len() == a.len()) => a
                    .iter()
                    .map(|p| match p {
                        Json::Arr(v) if v.len() == 6 => {
                            let mut out = [0.0; 6];
                            for (slot, x) in out.iter_mut().zip(v) {
                                *slot = number(x).filter(|f| (-100.0..=100.0).contains(f))?;
                            }
                            Some(out)
                        }
                        _ => None,
                    })
                    .collect(),
                _ => None,
            };
            require(points.is_some(), || format!("{w}.points: expected 3..4096 [x, y, in_x, in_y, out_x, out_y], same count in every key"))?;
            parsed.push((frame.unwrap() as i64, points.unwrap()));
        }
        shapes.push(parsed);
    }
    Ok(Matte { shapes, inverted, opacities, feathers, not_converted })
}

fn event(obj: &Json, where_: &str) -> R<Event> {
    let o = keys(
        obj,
        where_,
        &["clip", "record_start_ticks", "source_in_ticks", "duration_ticks"],
        &[
            "video_track", "audio_tracks", "audio", "audio_channels", "stereo", "audio_gains_db", "audio_pans", "speed_ratio",
            "source_duration_ticks", "color_correction", "transform", "matte", "time_remap",
        ],
    )?;
    let vt_json = o.opt("video_track");
    let video_track = match vt_json {
        None => None,
        Some(v) => match int_in(v, 1, 99, false) {
            Some(n) => Some(n),
            None => return fail(format!("{where_}.video_track: expected 1..99")),
        },
    };
    let at_json = o.or("audio_tracks", &EMPTY_ARR);
    let at: Vec<i64> = match at_json {
        Json::Arr(a) => {
            let parsed: Vec<Option<i64>> = a.iter().map(|t| int_in(t, 1, 99, false)).collect();
            if parsed.iter().all(|t| t.is_some()) {
                let v: Vec<i64> = parsed.into_iter().flatten().collect();
                if v.iter().collect::<HashSet<_>>().len() == v.len() {
                    v
                } else {
                    return fail(format!("{where_}.audio_tracks: expected distinct 1..99"));
                }
            } else {
                return fail(format!("{where_}.audio_tracks: expected distinct 1..99"));
            }
        }
        _ => return fail(format!("{where_}.audio_tracks: expected distinct 1..99")),
    };
    let mut stereo = Vec::new();
    let stereo_json = o.or("stereo", &EMPTY_ARR);
    for (i, x) in py_iter(stereo_json)?.iter().enumerate() {
        let w = format!("{where_}.stereo[{i}]");
        let xo = keys(x, &w, &["audio_track", "channels"], &["gain_db", "pans"])?;
        let chans = xo.req("channels");
        let audio_track = int_in(xo.req("audio_track"), 1, 99, false);
        require(audio_track.is_some(), || format!("{w}.audio_track: expected 1..99"))?;
        let channels: Option<Vec<i64>> = match chans {
            Json::Arr(c) if (2..=8).contains(&c.len()) => c.iter().map(|c| int_in(c, 1, 64, false)).collect(),
            _ => None,
        };
        require(channels.is_some(), || format!("{w}.channels: expected 2..8 Avid tracks"))?;
        let channels = channels.unwrap();
        let gain_db = match xo.opt("gain_db") {
            Some(g) => Some(db(g, &format!("{w}.gain_db"))?),
            None => None,
        };
        let default_pans = Json::Arr(vec![Json::Null; channels.len()]);
        let pans = optional_list(xo.or("pans", &default_pans), channels.len(), pan, &format!("{w}.pans"))?;
        stereo.push(StereoEdit { audio_track: audio_track.unwrap(), channels, gain_db, pans });
    }
    require(video_track.is_some() || !at.is_empty() || !stereo.is_empty(), || format!("{where_}: an event needs a video or audio track"))?;
    let audio_ev = if o.has("audio") { event_audio(o.req("audio"), &format!("{where_}.audio"))? } else { EventAudio::default() };
    require(!(audio_ev != EventAudio::default() && at.is_empty()), || format!("{where_}.audio: the event has no audio tracks"))?;
    let ch_ok = match o.or("audio_channels", &EMPTY_ARR) {
        Json::Arr(c) => {
            let v: Option<Vec<i64>> = c.iter().map(|c| int_in(c, 1, 64, true)).collect();
            v.filter(|v| v.is_empty() || v.len() == at.len())
        }
        _ => None,
    };
    require(ch_ok.is_some(), || format!("{where_}.audio_channels: expected one Avid track 1..64 per audio track"))?;
    let audio_channels = ch_ok.unwrap();
    let audio_gains = if o.has("audio_gains_db") {
        optional_list(o.req("audio_gains_db"), at.len(), db, &format!("{where_}.audio_gains_db"))?
    } else {
        Vec::new()
    };
    let audio_pans = if o.has("audio_pans") {
        optional_list(o.req("audio_pans"), at.len(), pan, &format!("{where_}.audio_pans"))?
    } else {
        Vec::new()
    };
    let mut speed_ratio = None;
    let mut source_duration = None;
    if let Some(ratio) = o.opt("speed_ratio") {
        let pair: Option<(i128, i128)> = match ratio {
            Json::Arr(r) if r.len() == 2 => match (strict_int(&r[0]), strict_int(&r[1])) {
                (Some(a), Some(b)) if a != 0 && b > 0 => Some((a, b)),
                _ => None,
            },
            _ => None,
        };
        require(pair.is_some(), || format!("{where_}.speed_ratio: expected [timeline, source] frames, timeline non-zero"))?;
        require(at.is_empty() && stereo.is_empty(), || {
            format!("{where_}: a speed change is only supported on picture (Avid does not retime audio here)")
        })?;
        let (a, b) = pair.unwrap();
        let (Ok(a), Ok(b)) = (i64::try_from(a), i64::try_from(b)) else {
            return fail(format!("{where_}.speed_ratio: out of range (this port keeps 64-bit ratios)"));
        };
        speed_ratio = Some((a, b));
        let sd = o.raw("source_duration_ticks").unwrap_or(&Json::Null);
        source_duration = Some(ticks(sd, &format!("{where_}.source_duration_ticks"), true)?);
    }
    let color_correction = match o.opt("color_correction") {
        Some(c) => Some(self::color_correction(c, &format!("{where_}.color_correction"))?),
        None => None,
    };
    require(color_correction.is_none() || video_track.is_some(), || format!("{where_}.color_correction: the event has no picture"))?;
    let transform = match o.opt("transform") {
        Some(t) => Some(self::transform(t, &format!("{where_}.transform"))?),
        None => None,
    };
    require(transform.is_none() || video_track.is_some(), || format!("{where_}.transform: the event has no picture"))?;
    let time_remap = match o.opt("time_remap") {
        None => None,
        Some(r) => {
            let frames: Option<Vec<i64>> = match r {
                Json::Arr(a) if (2..=100000).contains(&a.len()) => {
                    a.iter().map(|f| strict_int(f).filter(|n| (0..(1i128 << 40)).contains(n)).map(|n| n as i64)).collect()
                }
                _ => None,
            };
            require(frames.is_some(), || format!("{where_}.time_remap: expected 2.. source frames"))?;
            require(video_track.is_some() && at.is_empty() && stereo.is_empty() && o.opt("speed_ratio").is_none(), || {
                format!("{where_}.time_remap: only on a picture-only event without speed_ratio")
            })?;
            frames
        }
    };
    let matte = match o.opt("matte") {
        Some(m) => Some(self::matte(m, &format!("{where_}.matte"))?),
        None => None,
    };
    require(matte.is_none() || video_track.is_some(), || format!("{where_}.matte: the event has no picture"))?;
    let clip = text(o.req("clip"), &format!("{where_}.clip"), 256, false)?;
    let record_start = ticks(o.req("record_start_ticks"), &format!("{where_}.record_start_ticks"), false)?;
    let source_in = ticks(o.req("source_in_ticks"), &format!("{where_}.source_in_ticks"), false)?;
    let duration = ticks(o.req("duration_ticks"), &format!("{where_}.duration_ticks"), true)?;
    Ok(Event {
        clip,
        record_start,
        source_in,
        duration,
        video_track,
        audio_tracks: at,
        audio: audio_ev,
        audio_channels,
        stereo,
        audio_gains,
        audio_pans,
        speed_ratio,
        source_duration,
        color_correction,
        transform,
        matte,
        time_remap,
    })
}

fn is_track_label(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=3).contains(&b.len()) && (b[0] == b'V' || b[0] == b'A') && b[1..].iter().all(|c| c.is_ascii_digit())
}

fn marker(obj: &Json, where_: &str, in_sequence: bool) -> R<Marker> {
    let o = keys(
        obj,
        where_,
        &["name", "start_ticks"],
        &["comment", "user", "color", "rgb16", "duration_ticks", "avid_id", "track", "owner", "event", "source_ticks", "inherited"],
    )?;
    let comment = o.or("comment", &EMPTY_STR);
    require(matches!(comment, Json::Str(s) if s.chars().count() <= 4096), || format!("{where_}.comment: expected text"))?;
    let Json::Str(comment) = comment else { unreachable!() };
    require(!comment.chars().any(|c| ((c as u32) < 32 && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{fffe}' | '\u{ffff}'))
            && !(comment.ends_with('\t') && comment.trim_end_matches('\t').ends_with('\n')),
            || format!("{where_}.comment: text cannot be saved in XML"))?;
    let track = match o.opt("track") {
        None => None,
        Some(Json::Str(s)) if is_track_label(s) => Some(s.clone()),
        Some(_) => return fail(format!("{where_}.track: expected V1/A1-style")),
    };
    let owner_default = Json::Str("sequence".to_string());
    let owner = o.or("owner", &owner_default);
    let owner_ok = matches!(owner, Json::Str(s) if MARKER_OWNERS.contains(&s.as_str()) && (in_sequence || s == "sequence"));
    require(owner_ok, || format!("{where_}.owner: one of {} (sequences only)", tuple_repr(&MARKER_OWNERS)))?;
    let Json::Str(owner) = owner else { unreachable!() };
    let event = match o.opt("event") {
        None => None,
        Some(v) => match strict_int(v) {
            Some(n) if n >= 0 => Some(i64::try_from(n).unwrap_or(i64::MAX)),
            _ => return fail(format!("{where_}.event: expected an index")),
        },
    };
    let inherited = match o.or("inherited", &Json::Bool(false)) {
        Json::Bool(b) => *b,
        _ => return fail(format!("{where_}.inherited: expected true or false")),
    };
    let source = match o.opt("source_ticks") {
        Some(v) => Some(ticks(v, &format!("{where_}.source_ticks"), false)?),
        None => None,
    };
    require(owner != "edit" || (event.is_some() && source.is_some() && track.is_some()), || {
        format!("{where_}: an edit marker needs event, source_ticks and track")
    })?;
    let name = text(o.req("name"), &format!("{where_}.name"), 4096, true)?;
    let start = ticks(o.req("start_ticks"), &format!("{where_}.start_ticks"), false)?;
    let duration = ticks(o.or("duration_ticks", &Json::Int(0)), &format!("{where_}.duration_ticks"), false)?;
    let user = text(o.or("user", &EMPTY_STR), &format!("{where_}.user"), 256, true)?;
    let color = text(o.or("color", &EMPTY_STR), &format!("{where_}.color"), 64, true)?;
    Ok(Marker { name, comment: comment.clone(), start, duration, user, color, track, owner: owner.clone(), event, source, inherited })
}

fn sequence(obj: &Json, where_: &str) -> R<Sequence> {
    let o = keys(
        obj,
        where_,
        &["name", "rate", "width", "height", "events"],
        &["markers", "track_levels", "audio_crossfades", "start_ticks", "stereo_audio_tracks"],
    )?;
    let rate = rate(o.req("rate"), &format!("{where_}.rate"))?;
    let events_json = match o.req("events") {
        Json::Arr(a) if a.len() <= MAX_ITEMS => a,
        _ => return fail(format!("{where_}.events: expected a list")),
    };
    let markers_json = match o.or("markers", &EMPTY_ARR) {
        Json::Arr(a) if a.len() <= MAX_ITEMS => a,
        _ => return fail(format!("{where_}.markers: expected a list")),
    };
    let ms: Vec<Marker> =
        markers_json.iter().enumerate().map(|(i, m)| marker(m, &format!("{where_}.markers[{i}]"), true)).collect::<R<_>>()?;
    require(ms.iter().all(|m| m.event.is_none_or(|e| (e as i128) < events_json.len() as i128)), || {
        format!("{where_}.markers: event index out of range")
    })?;
    let mut wh = [0i64; 2];
    for (slot, k) in wh.iter_mut().zip(["width", "height"]) {
        match int_in(o.req(k), 16, 16384, true) {
            Some(n) => *slot = n,
            None => return fail(format!("{where_}.{k}: out of range")),
        }
    }
    let mut levels = Vec::new();
    for (i, t) in py_iter(o.or("track_levels", &EMPTY_ARR))?.iter().enumerate() {
        let w = format!("{where_}.track_levels[{i}]");
        let to = keys(t, &w, &["audio_track", "db"], &[])?;
        let Some(track) = int_in(to.req("audio_track"), 1, 99, false) else {
            return fail(format!("{w}.audio_track: expected 1..99"));
        };
        levels.push(TrackLevel { track, db: db_value(to.req("db"), &format!("{w}.db"), -200.0, 15.0)? });
    }
    let mut fades = Vec::new();
    for (i, f) in py_iter(o.or("audio_crossfades", &EMPTY_ARR))?.iter().enumerate() {
        let w = format!("{where_}.audio_crossfades[{i}]");
        let fo = keys(f, &w, &["audio_track", "cut_ticks", "duration_ticks", "cut_offset_ticks"], &["style"])?;
        let style_default = Json::Str("constant_power".to_string());
        let style = fo.or("style", &style_default);
        let style = match style {
            Json::Str(s) if s == "constant_power" || s == "custom_fade" => s.clone(),
            _ => return fail(format!("{w}.style: constant_power or custom_fade")),
        };
        let Some(track) = int_in(fo.req("audio_track"), 1, 99, false) else {
            return fail(format!("{w}.audio_track: expected 1..99"));
        };
        let duration = ticks(fo.req("duration_ticks"), &format!("{w}.duration_ticks"), true)?;
        let offset = ticks(fo.req("cut_offset_ticks"), &format!("{w}.cut_offset_ticks"), false)?;
        require(offset <= duration, || format!("{w}: cut_offset_ticks exceeds duration"))?;
        let cut = ticks(fo.req("cut_ticks"), &format!("{w}.cut_ticks"), false)?;
        fades.push(AudioCrossfade { track, cut, duration, cut_offset: offset, style });
    }
    let name = text_default(o.req("name"), &format!("{where_}.name"))?;
    let events: Vec<Event> = events_json.iter().enumerate().map(|(i, e)| event(e, &format!("{where_}.events[{i}]"))).collect::<R<_>>()?;
    let start = ticks(o.or("start_ticks", &Json::Int(0)), &format!("{where_}.start_ticks"), false)?;
    Ok(Sequence { name, rate, width: wh[0], height: wh[1], events, markers: ms, track_levels: levels, audio_crossfades: fades, start })
}

/// Python repr of a JSON scalar used as a dict key lookup (`unknown clip {ref!r}`).
fn scalar_repr(v: &Json) -> R<String> {
    Ok(match v {
        Json::Null => "None".to_string(),
        Json::Bool(true) => "True".to_string(),
        Json::Bool(false) => "False".to_string(),
        Json::Int(n) => n.to_string(),
        Json::Big(_) => "<huge int>".to_string(),
        Json::Float(f) => crate::build::fmt::py_repr(*f),
        Json::Str(s) => py_str_repr(s),
        other => return fail(format!("TypeError: unhashable type: '{}'", other.type_name())),
    })
}

/// `parse_model(data)`: validate a decoded JSON document into a [`Model`].
pub fn parse_model(data: &Json) -> R<Model> {
    let o = keys(data, "model", &["schema", "project", "clips", "bins"], &["options", "sequences"])?;
    require(matches!(o.req("schema"), Json::Str(s) if s == SCHEMA), || format!("model.schema: expected {SCHEMA}"))?;
    let project = keys(o.req("project"), "model.project", &["name"], &[])?;
    let empty = Json::Obj(Vec::new());
    let opts = keys(
        o.or("options", &empty),
        "model.options",
        &[],
        &["bin_mode", "match_avid_default_pan", "marker_conflict_winner", "marker_conflict_loser_to_sequence", "marker_avid_info", "marker_colors", "clip_labels"],
    )?;
    let default_mode = Json::Str("separate".to_string());
    let mode = opts.or("bin_mode", &default_mode);
    let mode_ok = matches!(mode, Json::Str(s) if BIN_MODES.contains(&s.as_str()));
    require(mode_ok, || format!("model.options.bin_mode: one of {}", tuple_repr(&BIN_MODES)))?;
    let Json::Str(mode) = mode else { unreachable!() };
    let flag = |k: &str| -> R<bool> {
        match opts.or(k, &Json::Bool(false)) {
            Json::Bool(b) => Ok(*b),
            _ => fail(format!("model.options.{k}: expected true or false")),
        }
    };
    let match_pan = flag("match_avid_default_pan")?;
    let default_winner = Json::Str("edit".to_string());
    let winner = opts.or("marker_conflict_winner", &default_winner);
    require(matches!(winner, Json::Str(s) if s == "edit" || s == "source"), || {
        "model.options.marker_conflict_winner: edit or source".to_string()
    })?;
    let Json::Str(winner) = winner else { unreachable!() };
    let loser_to_sequence = flag("marker_conflict_loser_to_sequence")?;
    let avid_info = flag("marker_avid_info")?;
    let mut maps: Vec<Vec<(String, i64)>> = Vec::new();
    for (k, high) in [("marker_colors", 7i64), ("clip_labels", 15)] {
        let m = opts.or(k, &empty);
        let ok = match m {
            Json::Obj(p) => p.len() <= 1024 && p.iter().all(|(_, v)| int_in(v, 0, high, true).is_some()),
            _ => false,
        };
        require(ok, || format!("model.options.{k}: expected {{name: index 0..{high}}}"))?;
        let Json::Obj(p) = m else { unreachable!() };
        let mut parsed = Vec::new();
        for (n, v) in p {
            parsed.push((text(&Json::Str(n.clone()), &format!("model.options.{k} key"), 64, false)?, int_in(v, 0, high, true).unwrap()));
        }
        maps.push(parsed);
    }
    let clip_label_map = maps.pop().unwrap();
    let marker_color_map = maps.pop().unwrap();
    let clips_json = match o.req("clips") {
        Json::Arr(a) if a.len() <= MAX_ITEMS => a,
        _ => return fail("model.clips: expected a list".to_string()),
    };
    let mut clips = ClipMap::default();
    for (i, c) in clips_json.iter().enumerate() {
        let parsed = clip(c, &format!("model.clips[{i}]"))?;
        require(!clips.contains_key(&parsed.id), || format!("model.clips[{i}].id: duplicate id {}", py_str_repr(&parsed.id)))?;
        clips.insert(parsed);
    }
    let bins_json = match o.req("bins") {
        Json::Arr(a) if a.len() <= MAX_ITEMS => a,
        _ => return fail("model.bins: expected a list".to_string()),
    };
    let mut bins = Vec::new();
    for (i, b) in bins_json.iter().enumerate() {
        let w = format!("model.bins[{i}]");
        let bo = keys(b, &w, &["name", "clips"], &[])?;
        let refs = match bo.req("clips") {
            Json::Arr(a) => a,
            _ => return fail(format!("{w}.clips: expected a list")),
        };
        for r in refs {
            let known = matches!(r, Json::Str(s) if clips.contains_key(s));
            if !known {
                return fail(format!("{w}.clips: unknown clip {}", scalar_repr(r)?));
            }
        }
        let name = text_default(bo.req("name"), &format!("{w}.name"))?;
        bins.push(Bin { name, clips: refs.iter().map(|r| if let Json::Str(s) = r { s.clone() } else { unreachable!() }).collect() });
    }
    let seqs_json = match o.or("sequences", &EMPTY_ARR) {
        Json::Arr(a) if a.len() <= 10_000 => a,
        _ => return fail("model.sequences: expected a list".to_string()),
    };
    let sequences: Vec<Sequence> =
        seqs_json.iter().enumerate().map(|(i, s)| sequence(s, &format!("model.sequences[{i}]"))).collect::<R<_>>()?;
    validate_sequence_links(&clips, &sequences)?;
    let options = Options {
        bin_mode: mode.clone(),
        match_avid_default_pan: match_pan,
        marker_conflict_winner: winner.clone(),
        marker_conflict_loser_to_sequence: loser_to_sequence,
        marker_avid_info: avid_info,
        marker_colors: marker_color_map,
        clip_labels: clip_label_map,
    };
    let project_name = text_default(project.req("name"), "model.project.name")?;
    Ok(Model { project_name, options, clips, bins, sequences })
}

// --- validation of constructed models -----------------------------------------------------------

/// Validate a constructed model without serializing it or allocating a JSON value tree.
/// This enforces the representable model/1 field, collection and cross-object rules; JSON-only
/// schema/type/presence checks remain with the parser. The file loader's 16 MiB byte ceiling is
/// an input-file limit, not a second encoded-size or resident-memory limit for typed models.
/// Callers must validate after mutations and before passing the model to a writer.
pub fn validate_model(model: &Model) -> R<()> {
    let opts = &model.options;
    require(BIN_MODES.contains(&opts.bin_mode.as_str()), || {
        format!("model.options.bin_mode: one of {}", tuple_repr(&BIN_MODES))
    })?;
    require(["edit", "source"].contains(&opts.marker_conflict_winner.as_str()), || {
        "model.options.marker_conflict_winner: edit or source".to_string()
    })?;
    for (key, entries, high) in [("marker_colors", &opts.marker_colors, 7), ("clip_labels", &opts.clip_labels, 15)] {
        require(entries.len() <= 1024 && entries.iter().all(|(_, v)| (0..=high).contains(v)), || {
            format!("model.options.{key}: expected {{name: index 0..{high}}}")
        })?;
        unique_keys(entries.iter().map(|(k, _)| k.as_str()))?;
        for (name, _) in entries {
            check_text(name, &format!("model.options.{key} key"), 64, false)?;
        }
    }
    require(model.clips.len() <= MAX_ITEMS, || "model.clips: expected a list".to_string())?;
    let mut ids = HashSet::new();
    for (i, clip) in model.clips.values().enumerate() {
        let w = format!("model.clips[{i}]");
        validate_clip(clip, &w)?;
        require(ids.insert(clip.id.as_str()), || {
            format!("{w}.id: duplicate id {}", py_str_repr(&clip.id))
        })?;
    }
    // Clip ids are public mutable fields. A renamed id must not leave the private lookup stale.
    require(
        model.clips.index.len() == model.clips.order.len()
            && model
                .clips
                .order
                .iter()
                .enumerate()
                .all(|(i, c)| model.clips.index.get(&c.id) == Some(&i)),
        || "model.clips: clip id lookup is inconsistent with clip ids".to_string(),
    )?;
    require(model.bins.len() <= MAX_ITEMS, || "model.bins: expected a list".to_string())?;
    for (i, bin) in model.bins.iter().enumerate() {
        let w = format!("model.bins[{i}]");
        for id in &bin.clips {
            require(model.clips.contains_key(id), || {
                format!("{w}.clips: unknown clip {}", py_str_repr(id))
            })?;
        }
        check_text(&bin.name, &format!("{w}.name"), 4096, false)?;
    }
    require(model.sequences.len() <= 10_000, || "model.sequences: expected a list".to_string())?;
    for (i, sequence) in model.sequences.iter().enumerate() {
        validate_sequence(sequence, &format!("model.sequences[{i}]"))?;
    }
    validate_sequence_links(&model.clips, &model.sequences)?;
    check_text(&model.project_name, "model.project.name", 4096, false)
}

fn unique_keys<'a>(keys: impl Iterator<Item = &'a str>) -> R<()> {
    let mut seen = HashSet::new();
    for key in keys {
        require(seen.insert(key), || format!("duplicate JSON key {}", py_str_repr(key)))?;
    }
    Ok(())
}

pub(crate) fn check_path(path: &str, w: &str) -> R<()> {
    check_text(path, w, 32768, false)?;
    require(is_absolute_path(path), || format!("{w}: media paths must be absolute"))
}

pub(crate) fn check_dimensions(width: i64, height: i64, w: &str) -> R<()> {
    for (key, value) in [("width", width), ("height", height)] {
        require((16..=16384).contains(&value), || format!("{w}.{key}: out of range"))?;
    }
    Ok(())
}

pub(crate) fn validate_clip(c: &Clip, w: &str) -> R<()> {
    if let Some(umid) = &c.material_umid {
        require(is_umid(umid), || format!("{w}.material_umid: expected 64 upper-case hex digits"))?;
    }
    if let Some(tape) = &c.tape_name {
        check_text(tape, &format!("{w}.tape_name"), 4096, false)?;
    }
    require(c.audio.len() <= 64, || format!("{w}.audio: expected a list of at most 64 files"))?;
    if let Some(v) = &c.video {
        let w = format!("{w}.video");
        require(["AVdn", "apcs", "apch"].contains(&v.codec.as_str()), || {
            format!("{w}.codec: only DNx, ProRes LT and the prepared ProRes HQ freeze profile are supported")
        })?;
        check_dimensions(v.width, v.height, &w)?;
        check_rate(v.rate, &format!("{w}.rate"))?;
        check_ticks(v.duration as i128, &format!("{w}.duration_ticks"), true)?;
        require(v.duration % ticks_per_unit(v.rate) == 0, || {
            format!("{w}.duration_ticks: not a whole number of frames")
        })?;
        check_path(&v.path, &format!("{w}.path"))?;
        check_ticks(v.start as i128, &format!("{w}.start_ticks"), false)?;
    }
    for (i, a) in c.audio.iter().enumerate() {
        let w = format!("{w}.audio[{i}]");
        require([44100, 48000, 96000].contains(&a.sample_rate), || {
            format!("{w}.sample_rate: unsupported")
        })?;
        require([16, 24].contains(&a.bits), || format!("{w}.bits: unsupported"))?;
        require(a.avid_track.is_none_or(|t| (1..=64).contains(&t)), || {
            format!("{w}.avid_track: expected 1..64")
        })?;
        check_path(&a.path, &format!("{w}.path"))?;
        check_ticks(a.duration as i128, &format!("{w}.duration_ticks"), true)?;
        check_ticks(a.start as i128, &format!("{w}.start_ticks"), false)?;
        check_db(a.gain_db, &format!("{w}.gain_db"), -96.0, 96.0)?;
    }
    check_audio_tracks(&c.audio, &format!("{w}.audio"))?;
    require(c.video.is_some() || !c.audio.is_empty(), || {
        format!("{w}: a clip needs at least one media file")
    })?;
    if let Some(a) = &c.avid {
        let w = format!("{w}.avid");
        require(a.columns.len() <= 512, || {
            format!("{w}.columns: expected an object of at most 512 columns")
        })?;
        unique_keys(a.columns.iter().map(|(k, _)| k.as_str()))?;
        for (k, v) in &a.columns {
            check_text(k, &format!("{w}.columns key"), 256, false)?;
            check_text(v, &format!("{w}.columns[{}]", py_str_repr(k)), 4096, true)?;
        }
        if let Some(rgb) = a.color_rgb16 {
            for v in rgb {
                require((0..=65535).contains(&v), || {
                    format!("{w}.color_rgb16: expected an integer 0..65535")
                })?;
            }
        }
        if let Some(tc) = &a.start_timecode {
            require(is_timecode(tc), || format!("{w}.start.timecode: expected HH:MM:SS:FF"))?;
        }
        require(a.aux_timecodes.len() <= 64, || format!("{w}.aux_timecodes: expected a list"))?;
        for (i, tc) in a.aux_timecodes.iter().enumerate() {
            let w = format!("{w}.aux_timecodes[{i}]");
            for (value, low, high) in [(tc.track_index, 0, 32767), (tc.frames, 0, 1i64 << 32), (tc.fps, 1, 1000)] {
                require((low..=high).contains(&value), || format!("{w}: expected an integer {low}..{high}"))?;
            }
            require(is_timecode(&tc.timecode), || format!("{w}: expected HH:MM:SS:FF"))?;
        }
        check_text(&a.mob_id, &format!("{w}.mob_id"), 4096, false)?;
        for (key, value) in [("source_file", &a.source_file), ("source_path", &a.source_path)] {
            if let Some(v) = value {
                check_text(v, &format!("{w}.{key}"), 4096, false)?;
            }
        }
    }
    if let Some(td) = c.time_display {
        require(TIME_DISPLAY_CODES.contains(&td), || {
            let codes: Vec<_> = TIME_DISPLAY_CODES.iter().map(|c| c.to_string()).collect();
            format!("{w}.time_display: expected one of [{}]", codes.join(", "))
        })?;
    }
    require(
        c.stereo_pairs.iter().all(|(a, b)| (1..=64).contains(a) && (1..=64).contains(b)),
        || format!("{w}.stereo_pairs: expected [[left, right], ...] Avid tracks"),
    )?;
    let known: HashSet<_> = c.audio.iter().filter_map(|a| a.avid_track).collect();
    require(c.stereo_pairs.iter().all(|(a, b)| known.contains(a) && known.contains(b)), || {
        format!("{w}.stereo_pairs: names an audio track the clip lacks")
    })?;
    let mut used = HashSet::new();
    require(c.stereo_pairs.iter().all(|(a, b)| used.insert(*a) && used.insert(*b)), || {
        format!("{w}.stereo_pairs: a track is in two pairs")
    })?;
    require(c.multichannel_groups.len() <= 32, || format!("{w}.multichannel_groups: expected at most 32 groups"))?;
    for (i, g) in c.multichannel_groups.iter().enumerate() {
        let at = format!("{w}.multichannel_groups[{i}]");
        require(g.width().is_some(), || format!("{at}.format: expected 5.1 or 7.1"))?;
        require(g.width() == Some(g.channels.len()) && g.channels.iter().all(|n| (1..=64).contains(n)), || format!("{at}.channels: expected {} Avid tracks",g.width().unwrap()))?;
        require(g.channels.iter().all(|n|known.contains(n)&&used.insert(*n)), || format!("{at}.channels: missing, repeated or overlapping member"))?;
    }
    if let Some(rate) = c.rate {
        check_rate(rate, &format!("{w}.rate"))?;
    }
    require(c.markers.len() <= MAX_ITEMS, || format!("{w}.markers: expected a list"))?;
    check_text(&c.id, &format!("{w}.id"), 256, false)?;
    check_text(&c.name, &format!("{w}.name"), 4096, false)?;
    for (i, m) in c.markers.iter().enumerate() {
        validate_marker(m, &format!("{w}.markers[{i}]"), false)?;
    }
    Ok(())
}

fn check_optional_values(values: &[Option<f64>], count: usize, w: &str, is_pan: bool, allow_absent: bool) -> R<()> {
    require(values.len() == count || (allow_absent && values.is_empty()), || {
        format!("{w}: expected {count} entries")
    })?;
    for (i, value) in values.iter().enumerate() {
        if let Some(v) = value {
            let w = format!("{w}[{i}]");
            if is_pan {
                require((-100.0..=100.0).contains(v), || format!("{w}: expected a pan percent -100..100"))?;
            } else {
                check_db(*v, &w, -96.0, 96.0)?;
            }
        }
    }
    Ok(())
}

fn validate_event(e: &Event, w: &str) -> R<()> {
    require(e.video_track.is_none_or(|t| (1..=99).contains(&t)), || {
        format!("{w}.video_track: expected 1..99")
    })?;
    let distinct: HashSet<_> = e.audio_tracks.iter().collect();
    require(
        e.audio_tracks.iter().all(|t| (1..=99).contains(t)) && distinct.len() == e.audio_tracks.len(),
        || format!("{w}.audio_tracks: expected distinct 1..99"),
    )?;
    for (i, s) in e.stereo.iter().enumerate() {
        let w = format!("{w}.stereo[{i}]");
        require((1..=99).contains(&s.audio_track), || format!("{w}.audio_track: expected 1..99"))?;
        require(
            (2..=8).contains(&s.channels.len()) && s.channels.iter().all(|t| (1..=64).contains(t)),
            || format!("{w}.channels: expected 2..8 Avid tracks"),
        )?;
        if let Some(g) = s.gain_db {
            check_db(g, &format!("{w}.gain_db"), -96.0, 96.0)?;
        }
        check_optional_values(&s.pans, s.channels.len(), &format!("{w}.pans"), true, false)?;
    }
    require(
        e.video_track.is_some() || !e.audio_tracks.is_empty() || !e.stereo.is_empty(),
        || format!("{w}: an event needs a video or audio track"),
    )?;
    let aw = format!("{w}.audio");
    require(e.audio.volume_keys.len() <= 65536, || format!("{aw}.volume_keys: expected a list"))?;
    for (i, k) in e.audio.volume_keys.iter().enumerate() {
        check_ticks(k.ticks as i128, &format!("{aw}.volume_keys[{i}].source_ticks"), false)?;
        check_db(k.db, &format!("{aw}.volume_keys[{i}].db"), -200.0, 24.0)?;
    }
    require(e.audio.volume_keys.windows(2).all(|v| v[0].ticks < v[1].ticks), || {
        format!("{aw}.volume_keys: times must be strictly increasing")
    })?;
    require(e.audio.volume_keys.is_empty() || e.audio.volume_db.is_none(), || {
        format!("{aw}: give volume_db or volume_keys, not both")
    })?;
    check_db(e.audio.gain_db, &format!("{aw}.gain_db"), -96.0, 96.0)?;
    if let Some(v) = e.audio.volume_db {
        check_db(v, &format!("{aw}.volume_db"), -200.0, 24.0)?;
    }
    require(e.audio == EventAudio::default() || !e.audio_tracks.is_empty(), || {
        format!("{w}.audio: the event has no audio tracks")
    })?;
    require(
        (e.audio_channels.is_empty() || e.audio_channels.len() == e.audio_tracks.len())
            && e.audio_channels.iter().all(|t| (1..=64).contains(t)),
        || format!("{w}.audio_channels: expected one Avid track 1..64 per audio track"),
    )?;
    check_optional_values(&e.audio_gains, e.audio_tracks.len(), &format!("{w}.audio_gains_db"), false, true)?;
    check_optional_values(&e.audio_pans, e.audio_tracks.len(), &format!("{w}.audio_pans"), true, true)?;
    if let Some((timeline, source)) = e.speed_ratio {
        require(timeline != 0 && source > 0, || {
            format!("{w}.speed_ratio: expected [timeline, source] frames, timeline non-zero")
        })?;
        require(e.audio_tracks.is_empty() && e.stereo.is_empty(), || {
            format!("{w}: a speed change is only supported on picture (Avid does not retime audio here)")
        })?;
        let sd = e
            .source_duration
            .ok_or_else(|| ModelError(format!("{w}.source_duration_ticks: expected integer ticks")))?;
        check_ticks(sd as i128, &format!("{w}.source_duration_ticks"), true)?;
    } else {
        require(e.source_duration.is_none(), || {
            format!("{w}.source_duration_ticks: requires speed_ratio")
        })?;
    }
    if let Some(c) = &e.color_correction {
        validate_color(c, &format!("{w}.color_correction"))?;
    }
    require(e.color_correction.is_none() || e.video_track.is_some(), || {
        format!("{w}.color_correction: the event has no picture")
    })?;
    if let Some(t) = &e.transform {
        let w = format!("{w}.transform");
        require((-32768.0..=32767.0).contains(&t.rotation), || {
            format!("{w}.rotation: expected degrees")
        })?;
        for (key, values, low, high, range) in [
            ("position", &[t.position.0, t.position.1][..], -100.0, 100.0, "-100..100"),
            ("scale", &[t.scale.0, t.scale.1][..], 0.0, 10000.0, "0..10000"),
            ("anchor", &[t.anchor.0, t.anchor.1][..], -100.0, 100.0, "-100..100"),
            ("crop", &t.crop[..], -100.0, 100.0, "-100..100"),
        ] {
            require(values.iter().all(|v| low <= *v && *v <= high), || {
                format!("{w}.{key}: expected {} numbers {range}", values.len())
            })?;
        }
    }
    require(e.transform.is_none() || e.video_track.is_some(), || {
        format!("{w}.transform: the event has no picture")
    })?;
    if let Some(frames) = &e.time_remap {
        require(
            (2..=100000).contains(&frames.len()) && frames.iter().all(|f| (0..(1i64 << 40)).contains(f)),
            || format!("{w}.time_remap: expected 2.. source frames"),
        )?;
        require(
            e.video_track.is_some() && e.audio_tracks.is_empty() && e.stereo.is_empty() && e.speed_ratio.is_none(),
            || format!("{w}.time_remap: only on a picture-only event without speed_ratio"),
        )?;
    }
    if let Some(m) = &e.matte {
        validate_matte(m, &format!("{w}.matte"))?;
    }
    require(e.matte.is_none() || e.video_track.is_some(), || {
        format!("{w}.matte: the event has no picture")
    })?;
    check_text(&e.clip, &format!("{w}.clip"), 256, false)?;
    check_ticks(e.record_start as i128, &format!("{w}.record_start_ticks"), false)?;
    check_ticks(e.source_in as i128, &format!("{w}.source_in_ticks"), false)?;
    check_ticks(e.duration as i128, &format!("{w}.duration_ticks"), true)
}

fn validate_color(c: &ColorCorrection, w: &str) -> R<()> {
    unique_keys(c.tone.iter().map(|(k, _)| k.as_str()))?;
    require(
        c.tone.iter().all(|(k, v)| {
            ["hue", "brightness", "contrast", "gain", "gamma", "setup"].contains(&k.as_str()) && (-1000.0..=1000.0).contains(v)
        }),
        || format!("{w}.tone: expected Avid control values"),
    )?;
    require(c.tone.iter().find(|(k, _)| k == "gamma").is_none_or(|(_, v)| *v > 0.0), || {
        format!("{w}.tone.gamma: must be positive")
    })?;
    require(c.saturation.is_none_or(|s| (0.0..=1000.0).contains(&s)), || {
        format!("{w}.saturation: expected a percent")
    })?;
    unique_keys(c.curves.iter().map(|(k, _)| k.as_str()))?;
    require(c.curves.iter().all(|(k, _)| CURVE_NAMES.contains(&k.as_str())), || {
        format!("{w}.curves: keys from {}", tuple_repr(&CURVE_NAMES))
    })?;
    for (name, points) in &c.curves {
        require(
            (2..=32).contains(&points.len()) && points.iter().all(|(x, y)| (-4.0..=4.0).contains(x) && (-4.0..=4.0).contains(y)),
            || format!("{w}.curves.{name}: expected 2..32 [x, y] points"),
        )?;
        require(
            points.iter().enumerate().all(|(i, p)| points[..i].iter().all(|q| p.0 != q.0)),
            || format!("{w}.curves.{name}: x coordinates must be distinct"),
        )?;
    }
    Ok(())
}

fn validate_matte(m: &Matte, w: &str) -> R<()> {
    require((1..=64).contains(&m.shapes.len()), || format!("{w}.shapes: expected 1..64 shapes"))?;
    require(m.opacities.len() == m.shapes.len() && m.feathers.len() == m.shapes.len(), || {
        format!("{w}.shapes: opacity/feather sidecar count differs from shapes")
    })?;
    for (i, keys) in m.shapes.iter().enumerate() {
        require((0.0..=100.0).contains(&m.opacities[i]), || {
            format!("{w}.shapes[{i}].opacity: expected a percent 0..100")
        })?;
        let f = m.feathers[i];
        require([f.0, f.1].iter().all(|v| (0.0..=100000.0).contains(v)), || {
            format!("{w}.shapes[{i}].feather: expected two raw Avid values")
        })?;
        require((1..=100000).contains(&keys.len()), || {
            format!("{w}.shapes[{i}].keyframes: expected 1.. keys")
        })?;
        for (j, (frame, points)) in keys.iter().enumerate() {
            let w = format!("{w}.shapes[{i}].keyframes[{j}]");
            require((0..(1i64 << 31)).contains(frame) && (j == 0 || keys[j - 1].0 < *frame), || {
                format!("{w}.frame: expected increasing frames from 0")
            })?;
            require(
                (3..=4096).contains(&points.len())
                    && points.len() == keys[0].1.len()
                    && points.iter().flatten().all(|v| (-100.0..=100.0).contains(v)),
                || format!("{w}.points: expected 3..4096 [x, y, in_x, in_y, out_x, out_y], same count in every key"),
            )?;
        }
    }
    Ok(())
}

pub(crate) fn validate_marker(m: &Marker, w: &str, in_sequence: bool) -> R<()> {
    require(m.comment.chars().count() <= 4096, || format!("{w}.comment: expected text"))?;
    require(
        !m.comment
            .chars()
            .any(|c| ((c as u32) < 32 && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{fffe}' | '\u{ffff}'))
            && !(m.comment.ends_with('\t') && m.comment.trim_end_matches('\t').ends_with('\n')),
        || format!("{w}.comment: text cannot be saved in XML"),
    )?;
    require(m.track.as_ref().is_none_or(|s| is_track_label(s)), || {
        format!("{w}.track: expected V1/A1-style")
    })?;
    require(
        MARKER_OWNERS.contains(&m.owner.as_str()) && (in_sequence || m.owner == "sequence"),
        || format!("{w}.owner: one of {} (sequences only)", tuple_repr(&MARKER_OWNERS)),
    )?;
    require(m.event.is_none_or(|e| e >= 0), || format!("{w}.event: expected an index"))?;
    if let Some(source) = m.source {
        check_ticks(source as i128, &format!("{w}.source_ticks"), false)?;
    }
    require(
        m.owner != "edit" || (m.event.is_some() && m.source.is_some() && m.track.is_some()),
        || format!("{w}: an edit marker needs event, source_ticks and track"),
    )?;
    check_text(&m.name, &format!("{w}.name"), 4096, true)?;
    check_ticks(m.start as i128, &format!("{w}.start_ticks"), false)?;
    check_ticks(m.duration as i128, &format!("{w}.duration_ticks"), false)?;
    check_text(&m.user, &format!("{w}.user"), 256, true)?;
    check_text(&m.color, &format!("{w}.color"), 64, true)
}

pub(crate) fn validate_sequence(s: &Sequence, w: &str) -> R<()> {
    check_rate(s.rate, &format!("{w}.rate"))?;
    require(s.events.len() <= MAX_ITEMS, || format!("{w}.events: expected a list"))?;
    require(s.markers.len() <= MAX_ITEMS, || format!("{w}.markers: expected a list"))?;
    for (i, m) in s.markers.iter().enumerate() {
        validate_marker(m, &format!("{w}.markers[{i}]"), true)?;
    }
    require(
        s.markers
            .iter()
            .all(|m| m.event.is_none_or(|e| e >= 0 && (e as usize) < s.events.len())),
        || format!("{w}.markers: event index out of range"),
    )?;
    check_dimensions(s.width, s.height, w)?;
    for (i, t) in s.track_levels.iter().enumerate() {
        let w = format!("{w}.track_levels[{i}]");
        require((1..=99).contains(&t.track), || format!("{w}.audio_track: expected 1..99"))?;
        check_db(t.db, &format!("{w}.db"), -200.0, 15.0)?;
    }
    for (i, f) in s.audio_crossfades.iter().enumerate() {
        let w = format!("{w}.audio_crossfades[{i}]");
        require(["constant_power", "custom_fade"].contains(&f.style.as_str()), || {
            format!("{w}.style: constant_power or custom_fade")
        })?;
        require((1..=99).contains(&f.track), || format!("{w}.audio_track: expected 1..99"))?;
        check_ticks(f.duration as i128, &format!("{w}.duration_ticks"), true)?;
        check_ticks(f.cut_offset as i128, &format!("{w}.cut_offset_ticks"), false)?;
        require(f.cut_offset <= f.duration, || format!("{w}: cut_offset_ticks exceeds duration"))?;
        check_ticks(f.cut as i128, &format!("{w}.cut_ticks"), false)?;
    }
    check_text(&s.name, &format!("{w}.name"), 4096, false)?;
    for (i, e) in s.events.iter().enumerate() {
        validate_event(e, &format!("{w}.events[{i}]"))?;
    }
    check_ticks(s.start as i128, &format!("{w}.start_ticks"), false)
}
/// Validate model/1's cross-object placement rules, shared by JSON input and graph projection.
/// Use [`validate_model`] for complete constructed-model validation. Rates are checked here as
/// well so this narrower legacy helper cannot divide by zero during frame-grid arithmetic.
pub fn validate_sequence_links(clips: &ClipMap, sequences: &[Sequence]) -> R<()> {
    for s in sequences {
        check_rate(s.rate, &format!("sequence {}.rate", py_str_repr(&s.name)))?;
        let unit = ticks_per_unit(s.rate);
        let sname = py_str_repr(&s.name);
        for (i, e) in s.events.iter().enumerate() {
            let pre = format!("sequence {sname} event {i}");
            let Some(c) = clips.get(&e.clip) else { return fail(format!("{pre}: unknown clip {}", py_str_repr(&e.clip))) };
            require(e.video_track.is_none() || c.video.is_some(), || format!("{pre}: clip has no video"))?;
            require(e.audio_tracks.len() <= c.audio.len(), || format!("{pre}: more audio tracks than clip audio files"))?;
            let known: HashSet<Option<i64>> = c.audio.iter().map(|f| f.avid_track).collect();
            require(e.audio_channels.iter().all(|ch| known.contains(&Some(*ch))), || format!("{pre}: audio_channels names a track the clip lacks"))?;
            require(e.stereo.iter().all(|st| st.channels.iter().all(|ch| known.contains(&Some(*ch)))), || {
                format!("{pre}: stereo names a track the clip lacks")
            })?;
            for (label, v) in [("record_start", e.record_start), ("duration", e.duration)] {
                require(v % unit == 0, || format!("{pre}: {label} is not on the sequence frame grid"))?;
            }
            let media = c.audio.iter().map(|f| f.duration).chain(c.video.iter().map(|v| v.duration)).max()
                .ok_or_else(|| ModelError(format!("{pre}: clip has no media")))?;
            let span = e.source_duration.unwrap_or(e.duration);
            require(e.source_in as i128 + span as i128 <= media as i128, || format!("{pre}: source range exceeds media"))?;
        }
    }
    Ok(())
}

/// Decode and validate a model from JSON text (Python: `parse_model(json.loads(text, ...))`).
pub fn parse_model_str(text: &str) -> R<Model> {
    parse_model(&parse_json(text)?)
}

/// `load_model(path)`: read, decode (UTF-8, universal newlines like Python's `read_text`) and
/// validate a model file.
/// Python's `OSError` text: `[Errno 2] No such file or directory: 'missing.json'` (the path as
/// `str(Path)` shows it).
fn os_error(error: &std::io::Error, path: &Path) -> ModelError {
    let text = error.to_string();
    let strerror = text.rfind(" (os error ").map_or(text.as_str(), |at| &text[..at]);
    let name = py_str_repr(&crate::model::project::py_path(&path.to_string_lossy()).to_string_lossy());
    ModelError(match error.raw_os_error() {
        Some(code) => format!("[Errno {code}] {strerror}: {name}"),
        None => format!("{strerror}: {name}"),
    })
}

/// `load_model(path)`. Where Python crashes with a traceback (exit 1) — a directory path
/// (`IsADirectoryError`), a file that is not UTF-8 (`UnicodeDecodeError`), integers of more
/// than 4300 digits (`ValueError`) — this reports a [`ModelError`] instead (the CLI exits 2).
pub fn load_model(path: &Path) -> R<Model> {
    let size = std::fs::metadata(path).map_err(|e| os_error(&e, path))?.len();
    if size > MAX_MODEL_BYTES {
        return fail("model file exceeds 16 MiB".to_string());
    }
    let bytes = std::fs::read(path).map_err(|e| os_error(&e, path))?;
    let text = String::from_utf8(bytes).map_err(|e| ModelError(format!("UnicodeDecodeError: {e}")))?;
    let text = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text };
    parse_model_str(&text)
}
