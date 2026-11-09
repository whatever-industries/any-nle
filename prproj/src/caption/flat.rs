//! The styled-text payload (`FormattedTextData`): port of the module-level functions of
//! `models/caption.py`.
//!
//! The payload is a FlatBuffer. Every function here reads and rewrites it the way the Python
//! does, including the places where a corrupt buffer makes Python raise: [`PyError::Value`] for
//! the `malformed caption payload` reads and writes, [`PyError::Struct`] where `struct` refuses a
//! value, [`PyError::Index`] for a byte store past the buffer, [`PyError::NotImplemented`] for the
//! shapes py does not model.
//!
//! Positions are `i64` (Python ints are unbounded; offsets read from a buffer are `u32`, so sums
//! of a few of them cannot overflow `i64`).

use std::collections::{BTreeSet, HashMap, HashSet};

use super::{PyError, PyResult};
use crate::base64::b64encode;
use crate::build::fmt::py_repr;
use crate::convert::model::py_printable;
use crate::pystr::py_strip;
use crate::xml::{NodeId, PremiereDocument};

/// `FLOAT32_MAX` (`models/validators.py`).
pub const FLOAT32_MAX: f64 = 3.4028234663852886e38;

/// An RGBA colour with 8-bit channels (Python `Color`, a `NamedTuple`; alpha defaults to 255).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub red: i64,
    pub green: i64,
    pub blue: i64,
    pub alpha: i64,
}

impl Color {
    /// `Color(red, green, blue)` with alpha 255.
    pub fn rgb(red: i64, green: i64, blue: i64) -> Color {
        Color { red, green, blue, alpha: 255 }
    }

    /// `Color(red, green, blue, alpha)`.
    pub fn rgba(red: i64, green: i64, blue: i64, alpha: i64) -> Color {
        Color { red, green, blue, alpha }
    }

    /// `validate_color`: every channel an integer 0-255.
    pub fn validate(&self) -> PyResult<()> {
        for channel in [self.red, self.green, self.blue, self.alpha] {
            if !(0..=255).contains(&channel) {
                return super::value_err(format!("channel out of range 0-255: {channel}"));
            }
        }
        Ok(())
    }
}

/// `validate_number` for a float: finite.
pub fn validate_number(value: f64) -> PyResult<()> {
    if !value.is_finite() {
        let shown = if value.is_nan() {
            "nan".to_string()
        } else if value > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
        return super::value_err(format!("must be a finite number, got {shown}"));
    }
    Ok(())
}

/// `validate_positive_number` (`min=0.0`) for a float.
pub fn validate_positive_number(value: f64) -> PyResult<()> {
    validate_number(value)?;
    if value < 0.0 {
        return super::value_err(format!("must be >= 0.0, got {}", py_repr(value)));
    }
    Ok(())
}

/// Whitespace real caption and title text carries (`str.isprintable()` calls all three
/// unprintable).
const TEXT_WHITESPACE: [char; 3] = ['\n', '\r', '\t'];

/// `is_payload_text`: whether a string decoded out of a binary payload looks like text.
pub fn is_payload_text(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| py_printable(c) || TEXT_WHITESPACE.contains(&c))
}

/// The FlatBuffer proper starts after the u64 byte-length prefix and the u32 `0x11223344` magic.
pub const FB_START: i64 = 12;
/// `_TEXT_MAGIC`: bytes 8..12 of a `FormattedTextData` payload.
pub const TEXT_MAGIC: [u8; 4] = [0x44, 0x33, 0x22, 0x11];

/// Document-table slots: the run vector and the font-name vector.
const RUNS_SLOT: usize = 0;
const FONT_SLOT: usize = 1;
/// A run table's slot holding its style table.
const RUN_STYLE_SLOT: usize = 1;

/// Style-table (per-run) slots, named by their sentinel values in 82_caption_style_sweep.
pub const STYLE_SIZE_SLOT: usize = 1;
pub const STYLE_FILL_SLOT: usize = 2;
pub const STYLE_STROKE_COLOR_SLOT: usize = 4;
pub const STYLE_STROKE_ENABLED_SLOT: usize = 5;
pub const STYLE_STROKE_WIDTH_SLOT: usize = 6;
pub const STYLE_TRACKING_SLOT: usize = 8;
/// Document-table (per-block) slots.
pub const BLOCK_LEADING_SLOT: usize = 6;
pub const BLOCK_SHADOW_COLOR_SLOT: usize = 10;
pub const BLOCK_SHADOW_OPACITY_SLOT: usize = 12;
pub const BLOCK_SHADOW_ANGLE_SLOT: usize = 13;
pub const BLOCK_SHADOW_DISTANCE_SLOT: usize = 14;
pub const BLOCK_SHADOW_SIZE_SLOT: usize = 15;
pub const BLOCK_SHADOW_BLUR_SLOT: usize = 16;
pub const BLOCK_BACKGROUND_COLOR_SLOT: usize = 17;
pub const BLOCK_BACKGROUND_ENABLED_SLOT: usize = 18;
pub const BLOCK_BACKGROUND_OPACITY_SLOT: usize = 19;
pub const BLOCK_BACKGROUND_SIZE_SLOT: usize = 20;
pub const BLOCK_BACKGROUND_CORNER_SLOT: usize = 34;

/// The size a run with no override renders at (64_caption_style).
pub const DEFAULT_FONT_SIZE: f64 = 100.0;

// ---- bounded reads and writes -------------------------------------------------------------------

fn read_at(payload: &[u8], offset: i64, size: usize) -> PyResult<&[u8]> {
    if offset < 0 || offset + size as i64 > payload.len() as i64 {
        return super::value_err(format!(
            "malformed caption payload: cannot read {size} bytes at {offset} of {}",
            payload.len()
        ));
    }
    let start = offset as usize;
    Ok(&payload[start..start + size])
}

fn u16_at(payload: &[u8], offset: i64) -> PyResult<i64> {
    let b = read_at(payload, offset, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]) as i64)
}

fn u32_at(payload: &[u8], offset: i64) -> PyResult<i64> {
    let b = read_at(payload, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64)
}

fn i32_at(payload: &[u8], offset: i64) -> PyResult<i64> {
    let b = read_at(payload, offset, 4)?;
    Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64)
}

fn u64_at(payload: &[u8], offset: i64) -> PyResult<i128> {
    let b = read_at(payload, offset, 8)?;
    Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as i128)
}

fn f32_at(payload: &[u8], offset: i64) -> PyResult<f64> {
    let b = read_at(payload, offset, 4)?;
    Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64)
}

/// `_pack_into` bounds check.
fn write_at(buffer: &mut [u8], offset: i64, size: usize) -> PyResult<usize> {
    if offset < 0 || offset + size as i64 > buffer.len() as i64 {
        return super::value_err(format!(
            "malformed caption payload: cannot write {size} bytes at {offset} of {}",
            buffer.len()
        ));
    }
    Ok(offset as usize)
}

fn struct_range<T>(code: char, low: &str, high: &str) -> PyResult<T> {
    Err(PyError::Struct(format!("'{code}' format requires {low} <= number <= {high}")))
}

fn put_u16(buffer: &mut [u8], offset: i64, value: i128) -> PyResult<()> {
    let at = write_at(buffer, offset, 2)?;
    if !(0..=65535).contains(&value) {
        return struct_range('H', "0", "65535");
    }
    buffer[at..at + 2].copy_from_slice(&(value as u16).to_le_bytes());
    Ok(())
}

fn put_u32(buffer: &mut [u8], offset: i64, value: i128) -> PyResult<()> {
    let at = write_at(buffer, offset, 4)?;
    if !(0..=4294967295).contains(&value) {
        return struct_range('I', "0", "4294967295");
    }
    buffer[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
    Ok(())
}

fn put_i32(buffer: &mut [u8], offset: i64, value: i128) -> PyResult<()> {
    let at = write_at(buffer, offset, 4)?;
    if !(-2147483648..=2147483647).contains(&value) {
        return struct_range('i', "-2147483648", "2147483647");
    }
    buffer[at..at + 4].copy_from_slice(&(value as i32).to_le_bytes());
    Ok(())
}

fn put_u64(buffer: &mut [u8], offset: i64, value: i128) -> PyResult<()> {
    let at = write_at(buffer, offset, 8)?;
    if !(0..=18446744073709551615).contains(&value) {
        return struct_range('Q', "0", "18446744073709551615");
    }
    buffer[at..at + 8].copy_from_slice(&(value as u64).to_le_bytes());
    Ok(())
}

/// `_pack_into("<f", ...)`: bounds, then the float32 range.
fn put_f32(buffer: &mut [u8], offset: i64, value: f64) -> PyResult<()> {
    let at = write_at(buffer, offset, 4)?;
    if value.abs() > FLOAT32_MAX {
        return super::value_err(format!("value out of float32 range: {}", py_repr(value)));
    }
    buffer[at..at + 4].copy_from_slice(&(value as f32).to_le_bytes());
    Ok(())
}

/// `_pack_float32`: a float32 for splicing in, with the range check.
pub fn pack_float32(value: f64) -> PyResult<[u8; 4]> {
    if value.abs() > FLOAT32_MAX {
        return super::value_err(format!("value out of float32 range: {}", py_repr(value)));
    }
    Ok((value as f32).to_le_bytes())
}

/// Python `bytes.decode("utf-8")` as a `UnicodeDecodeError`.
pub fn decode_utf8(bytes: &[u8]) -> PyResult<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_string()),
        Err(error) => {
            let start = error.valid_up_to();
            let first = bytes[start];
            let (end, reason) = match error.error_len() {
                None => (bytes.len() - 1, "unexpected end of data"),
                Some(len) => {
                    let reason = if matches!(first, 0x80..=0xC1 | 0xF5..=0xFF) {
                        "invalid start byte"
                    } else {
                        "invalid continuation byte"
                    };
                    (start + len - 1, reason)
                }
            };
            let message = if end == start {
                format!("'utf-8' codec can't decode byte 0x{first:02x} in position {start}: {reason}")
            } else {
                format!("'utf-8' codec can't decode bytes in position {start}-{end}: {reason}")
            };
            Err(PyError::Unicode(message))
        }
    }
}

// ---- the table walk ---------------------------------------------------------------------------------

/// The vtable position of a table at `position`, or `None` when the bytes there cannot be a
/// table (alignment gates included).
fn valid_vtable(payload: &[u8], position: i64) -> PyResult<Option<i64>> {
    if position % 4 != 0 || position + 4 > payload.len() as i64 {
        return Ok(None);
    }
    let soffset = i32_at(payload, position)?;
    let vtable = position - soffset;
    if vtable < 0 || vtable % 2 != 0 || vtable + 4 > payload.len() as i64 {
        return Ok(None);
    }
    let vtable_length = u16_at(payload, vtable)?;
    if vtable_length < 4 || vtable_length % 2 != 0 || vtable + vtable_length > payload.len() as i64 {
        return Ok(None);
    }
    Ok(Some(vtable))
}

/// Byte length of the length-prefixed text string at `position`, or `None` when it is not one.
fn last_string_at(payload: &[u8], position: i64) -> PyResult<Option<usize>> {
    if position + 4 > payload.len() as i64 {
        return Ok(None);
    }
    let length = u32_at(payload, position)?;
    let end = position + 4 + length;
    if length == 0 || end >= payload.len() as i64 || payload[end as usize] != 0 {
        return Ok(None);
    }
    let Ok(text) = std::str::from_utf8(&payload[(position + 4) as usize..end as usize]) else {
        return Ok(None);
    };
    Ok(if is_payload_text(text) { Some(length as usize) } else { None })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    U32,
    I32,
}

type Word = (i64, Kind, i64);

struct Walk<'a> {
    payload: &'a [u8],
    words: Vec<Word>,
    seen: HashSet<i64>,
    vtables: HashSet<i64>,
}

impl Walk<'_> {
    fn walk(&mut self, position: i64) -> PyResult<()> {
        if !self.seen.insert(position) {
            return Ok(());
        }
        let payload = self.payload;
        let Some(vtable) = valid_vtable(payload, position)? else { return Ok(()) };
        self.words.push((position, Kind::I32, vtable));
        self.vtables.insert(vtable);
        let vtable_length = u16_at(payload, vtable)?;
        let object_size = u16_at(payload, vtable + 2)?;
        let mut slots = Vec::new();
        for index in 0..(vtable_length - 4) / 2 {
            let relative = u16_at(payload, vtable + 4 + index * 2)?;
            if relative != 0 {
                slots.push(relative);
            }
        }
        // A field only has room for a 32-bit offset if the next field starts at least 4 bytes
        // later.
        let mut edges: Vec<i64> = slots.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        edges.push(object_size);
        let width: HashMap<i64, i64> = edges[..edges.len() - 1]
            .iter()
            .enumerate()
            .map(|(index, &edge)| (edge, edges[index + 1] - edge))
            .collect();
        let len = payload.len() as i64;
        for &relative in &slots {
            if width.get(&relative).copied().unwrap_or(0) < 4 {
                continue;
            }
            let field = position + relative;
            if field + 4 > len {
                continue;
            }
            let value = u32_at(payload, field)?;
            if !(0 < value && value <= len - field - 4) {
                continue;
            }
            let target = field + value;
            if last_string_at(payload, target)?.is_some() {
                self.words.push((field, Kind::U32, target));
            } else if valid_vtable(payload, target)?.is_some() {
                self.words.push((field, Kind::U32, target));
                self.walk(target)?;
            } else {
                let count = u32_at(payload, target)?;
                if !(0 < count && count <= 64) || target + 4 + count * 4 > len {
                    continue;
                }
                self.words.push((field, Kind::U32, target));
                for item in 0..count {
                    let slot = target + 4 + item * 4;
                    let offset = u32_at(payload, slot)?;
                    let item_target = slot + offset;
                    if !(0 < offset && offset < len - slot) {
                        continue;
                    }
                    if last_string_at(payload, item_target)?.is_some() {
                        self.words.push((slot, Kind::U32, item_target));
                    } else if valid_vtable(payload, item_target)?.is_some() {
                        self.words.push((slot, Kind::U32, item_target));
                        self.walk(item_target)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Every offset word in the buffer: `(word position, kind, target)`; found by walking from the
/// root plus a scan for ORPHAN tables.
fn collect_offset_words(payload: &[u8]) -> PyResult<Vec<Word>> {
    let root = FB_START + u32_at(payload, FB_START)?;
    let mut walk = Walk {
        payload,
        words: vec![(FB_START, Kind::U32, root)],
        seen: HashSet::new(),
        vtables: HashSet::new(),
    };
    walk.walk(root)?;
    let known: HashSet<i64> = walk.words.iter().map(|w| w.0).collect();
    let mut position = FB_START;
    while position < payload.len() as i64 - 4 {
        if !known.contains(&position) {
            let value = i32_at(payload, position)?;
            if value < 0 && walk.vtables.contains(&(position - value)) {
                walk.words.push((position, Kind::I32, position - value));
            }
        }
        position += 4;
    }
    Ok(walk.words)
}

/// `(offset of the u32 length word, string byte length)` of the buffer's LAST length-prefixed
/// UTF-8 string - the caption text.
fn last_string_span(payload: &[u8]) -> PyResult<Option<(i64, usize)>> {
    if (payload.len() as i64) < FB_START + 4 {
        return Ok(None);
    }
    let mut found = None;
    let targets: BTreeSet<i64> =
        collect_offset_words(payload)?.into_iter().filter(|w| w.1 == Kind::U32).map(|w| w.2).collect();
    for offset in targets {
        if let Some(length) = last_string_at(payload, offset)? {
            found = Some((offset, length));
        }
    }
    Ok(found)
}

/// `decode_caption_text`: the caption text out of a `FormattedTextData` payload ("" if none).
pub fn decode_caption_text(payload: &[u8]) -> PyResult<String> {
    let Some((offset, length)) = last_string_span(payload)? else { return Ok(String::new()) };
    decode_utf8(slice(payload, offset + 4, offset + 4 + length as i64))
}

/// Python slice `payload[start:end]` (clamped).
fn slice(payload: &[u8], start: i64, end: i64) -> &[u8] {
    let len = payload.len() as i64;
    let s = start.clamp(0, len) as usize;
    let e = end.clamp(0, len) as usize;
    if s >= e {
        &[]
    } else {
        &payload[s..e]
    }
}

/// `_splice_bytes`: replace `payload[start:end]`, re-deriving every stored offset.
fn splice_bytes(payload: &[u8], start: i64, end: i64, replacement: &[u8]) -> PyResult<Vec<u8>> {
    let delta = replacement.len() as i64 - (end - start);
    let shifted = |position: i64| if position >= end { position + delta } else { position };
    let words = collect_offset_words(payload)?;
    let mut buffer = slice(payload, 0, start).to_vec();
    buffer.extend_from_slice(replacement);
    buffer.extend_from_slice(slice(payload, end, payload.len() as i64));
    let prefix = u64_at(&buffer, 0)?;
    put_u64(&mut buffer, 0, prefix + delta as i128)?;
    for (position, kind, target) in words {
        match kind {
            Kind::U32 => put_u32(&mut buffer, shifted(position), (shifted(target) - shifted(position)) as i128)?,
            Kind::I32 => put_i32(&mut buffer, shifted(position), (shifted(position) - shifted(target)) as i128)?,
        }
    }
    Ok(buffer)
}

/// `_splice_string`: replace the length-prefixed padded string at `offset`.
fn splice_string(payload: &[u8], offset: i64, length: i64, text: &str) -> PyResult<Vec<u8>> {
    let span_end = offset + 4 + (length + 4) / 4 * 4;
    let encoded = text.as_bytes();
    let padded = (encoded.len() + 4) / 4 * 4;
    let mut replacement = (encoded.len() as u32).to_le_bytes().to_vec();
    replacement.extend_from_slice(encoded);
    replacement.resize(replacement.len() + padded - encoded.len(), 0);
    splice_bytes(payload, offset, span_end, &replacement)
}

/// `replace_payload_text`: the payload with its text string replaced by `text`.
pub fn replace_payload_text(payload: &[u8], text: &str) -> PyResult<Vec<u8>> {
    if text.is_empty() {
        return super::value_err("caption text cannot be empty");
    }
    let Some((offset, length)) = last_string_span(payload)? else {
        return super::value_err("payload holds no caption text");
    };
    splice_string(payload, offset, length as i64, text)
}

/// `_document_table`: the buffer's root table holds one field, the document table.
fn document_table(payload: &[u8]) -> PyResult<i64> {
    let root = FB_START + u32_at(payload, FB_START)?;
    let Some(vtable) = valid_vtable(payload, root)? else {
        return super::value_err("payload has no root table");
    };
    let field = root + u16_at(payload, vtable + 4)?;
    Ok(field + u32_at(payload, field)?)
}

/// `_font_string_span`: `(string position, length)` of the single font name.
fn font_string_span(payload: &[u8]) -> PyResult<(i64, i64)> {
    let document = document_table(payload)?;
    let Some(vtable) = valid_vtable(payload, document)? else {
        return super::value_err("payload has no document table");
    };
    let relative = u16_at(payload, vtable + 4 + FONT_SLOT as i64 * 2)?;
    if relative == 0 {
        return super::value_err("payload carries no font name");
    }
    let field = document + relative;
    let vector = field + u32_at(payload, field)?;
    let count = u32_at(payload, vector)?;
    if count == 0 {
        return super::value_err("malformed caption payload: font name vector is empty");
    }
    if count != 1 {
        return Err(PyError::NotImplemented(format!("payload carries {count} font names; only one is supported")));
    }
    let slot = vector + 4;
    let string = slot + u32_at(payload, slot)?;
    let length = u32_at(payload, string)?;
    Ok((string, length))
}

/// `_field_offset`: the absolute position of a table's field, `None` when the slot is absent.
fn field_offset(payload: &[u8], table: i64, slot: usize) -> PyResult<Option<i64>> {
    let Some(vtable) = valid_vtable(payload, table)? else { return Ok(None) };
    let vtable_length = u16_at(payload, vtable)?;
    if slot as i64 >= (vtable_length - 4) / 2 {
        return Ok(None);
    }
    let relative = u16_at(payload, vtable + 4 + slot as i64 * 2)?;
    Ok(if relative != 0 { Some(table + relative) } else { None })
}

/// `_style_table`: the style table of the payload's FIRST run.
fn style_table(payload: &[u8]) -> PyResult<Option<i64>> {
    let document = document_table(payload)?;
    let Some(field) = field_offset(payload, document, RUNS_SLOT)? else { return Ok(None) };
    let vector = field + u32_at(payload, field)?;
    let count = u32_at(payload, vector)?;
    if count == 0 {
        return Ok(None);
    }
    let run = vector + 4 + u32_at(payload, vector + 4)?;
    let Some(style_field) = field_offset(payload, run, RUN_STYLE_SLOT)? else { return Ok(None) };
    Ok(Some(style_field + u32_at(payload, style_field)?))
}

fn document_locate(payload: &[u8]) -> PyResult<Option<i64>> {
    Ok(Some(document_table(payload)?))
}

fn read_float(payload: &[u8], table: Option<i64>, slot: usize) -> PyResult<Option<f64>> {
    let Some(table) = table else { return Ok(None) };
    let Some(field) = field_offset(payload, table, slot)? else { return Ok(None) };
    if field + 4 > payload.len() as i64 {
        return Ok(None);
    }
    Ok(Some(f32_at(payload, field)?))
}

/// A colour is a nested table of three `ubyte` slots.
fn read_rgb(payload: &[u8], table: Option<i64>, slot: usize) -> PyResult<Option<Color>> {
    let Some(table) = table else { return Ok(None) };
    let Some(field) = field_offset(payload, table, slot)? else { return Ok(None) };
    if field + 4 > payload.len() as i64 {
        return Ok(None);
    }
    let nested = field + u32_at(payload, field)?;
    let mut channels = [0i64; 3];
    for (channel, out) in channels.iter_mut().enumerate() {
        let Some(position) = field_offset(payload, nested, channel)? else { return Ok(None) };
        if position >= payload.len() as i64 {
            return Ok(None);
        }
        *out = payload[position as usize] as i64;
    }
    Ok(Some(Color::rgb(channels[0], channels[1], channels[2])))
}

/// `_add_table_field`: give `table` a field it lacks, returning the rewritten payload.
fn add_table_field(payload: &[u8], table: i64, slot: usize, data: &[u8]) -> PyResult<Vec<u8>> {
    let Some(vtable) = valid_vtable(payload, table)? else { return super::value_err("not a table") };
    let vtable_length = u16_at(payload, vtable)?;
    let object_size = u16_at(payload, vtable + 2)?;
    let count = (vtable_length - 4) / 2;
    let mut entries = Vec::new();
    for index in 0..count {
        entries.push(u16_at(payload, vtable + 4 + index * 2)?);
    }
    let wanted = (slot as i64 + 1 - count).max(0);
    entries.extend(std::iter::repeat_n(0, wanted as usize));
    if entries[slot] != 0 {
        return super::value_err("table already carries that field");
    }
    let align = data.len() as i64;
    let field_relative = (object_size + align - 1) / align * align;
    entries[slot] = field_relative;
    let padding = field_relative - object_size;

    // 1. The value itself, at the end of the object, padded to a multiple of 4.
    let inserted = padding + data.len() as i64;
    let mut run = vec![0u8; padding as usize];
    run.extend_from_slice(data);
    run.resize(run.len() + (-inserted).rem_euclid(4) as usize, 0);
    let grown = splice_bytes(payload, table + object_size, table + object_size, &run)?;

    // 2. A private vtable immediately before the table, padded so the table stays 4-aligned.
    let mut new_vtable = Vec::new();
    for value in [4 + 2 * entries.len() as i64, field_relative + data.len() as i64] {
        if value > 65535 {
            return struct_range('H', "0", "65535");
        }
        new_vtable.extend_from_slice(&(value as u16).to_le_bytes());
    }
    for entry in &entries {
        new_vtable.extend_from_slice(&(*entry as u16).to_le_bytes());
    }
    let lead = (-(new_vtable.len() as i64)).rem_euclid(4) as usize;
    let mut block = vec![0u8; lead];
    block.extend_from_slice(&new_vtable);
    let mut result = splice_bytes(&grown, table, table, &block)?;
    let moved = table + block.len() as i64;
    put_i32(&mut result, moved, new_vtable.len() as i128)?;
    Ok(result)
}

fn write_table_float(payload: &[u8], table: i64, slot: usize, value: f64) -> PyResult<Vec<u8>> {
    match field_offset(payload, table, slot)? {
        None => add_table_field(payload, table, slot, &pack_float32(value)?),
        Some(field) => {
            let mut buffer = payload.to_vec();
            put_f32(&mut buffer, field, value)?;
            Ok(buffer)
        }
    }
}

/// `_color_vtable`: colour tables all share one vtable (three `ubyte` slots); borrow it.
fn color_vtable(payload: &[u8]) -> PyResult<Option<i64>> {
    let document = document_table(payload)?;
    let style = style_table(payload)?;
    for (table, slot) in [
        (Some(document), BLOCK_SHADOW_COLOR_SLOT),
        (Some(document), BLOCK_BACKGROUND_COLOR_SLOT),
        (style, STYLE_STROKE_COLOR_SLOT),
    ] {
        let Some(table) = table else { continue };
        let Some(field) = field_offset(payload, table, slot)? else { continue };
        let nested = field + u32_at(payload, field)?;
        if let Some(vtable) = valid_vtable(payload, nested)? {
            return Ok(Some(vtable));
        }
    }
    Ok(None)
}

type Locate = fn(&[u8]) -> PyResult<Option<i64>>;

fn write_table_color(payload: &[u8], locate: Locate, slot: usize, color: Color) -> PyResult<Vec<u8>> {
    if color.alpha != 255 {
        return super::value_err(format!(
            "caption colours are opaque: the payload stores red, green and blue only, so alpha must be 255 (got {})",
            color.alpha
        ));
    }
    let channels = [color.red, color.green, color.blue];
    let Some(table) = locate(payload)? else { return super::value_err("payload has no such table") };
    if let Some(field) = field_offset(payload, table, slot)? {
        let nested = field + u32_at(payload, field)?;
        let mut buffer = payload.to_vec();
        for (channel, value) in channels.iter().enumerate() {
            let Some(position) = field_offset(payload, nested, channel)? else {
                return Err(PyError::NotImplemented("colour table is missing a channel".into()));
            };
            if position >= buffer.len() as i64 {
                return Err(PyError::Index("bytearray index out of range".into()));
            }
            buffer[position as usize] = *value as u8;
        }
        return Ok(buffer);
    }

    let Some(vtable) = color_vtable(payload)? else {
        return Err(PyError::NotImplemented("payload carries no colour table to model".into()));
    };
    // Append the new colour table at the very end, so nothing shifts, then point a fresh field
    // at it.
    let object_size = u16_at(payload, vtable + 2)?;
    let mut body = vec![0u8; object_size as usize];
    put_i32(&mut body, 0, (payload.len() as i64 - vtable) as i128)?;
    for (channel, value) in channels.iter().enumerate() {
        let relative = u16_at(payload, vtable + 4 + channel as i64 * 2)?;
        if relative >= body.len() as i64 {
            return Err(PyError::Index("bytearray index out of range".into()));
        }
        body[relative as usize] = *value as u8;
    }
    let appended = splice_bytes(payload, payload.len() as i64, payload.len() as i64, &body)?;
    let located = locate(&appended)?.filter(|&t| t != 0).unwrap_or(0);
    let mut with_field = add_table_field(&appended, located, slot, &0u32.to_le_bytes())?;
    let moved = locate(&with_field)?;
    let field = match moved {
        None => None,
        Some(moved) => field_offset(&with_field, moved, slot)?,
    };
    let Some(field) = field else { return super::value_err("failed to add the colour field") };
    let target = with_field.len() as i64 - object_size;
    put_u32(&mut with_field, field, (target - field) as i128)?;
    Ok(with_field)
}

/// `_write_table_flag`: a group's ubyte enable flag (the Appearance panel checkbox).
fn write_table_flag(payload: &[u8], locate: Locate, slot: usize) -> PyResult<Vec<u8>> {
    let Some(table) = locate(payload)? else { return super::value_err("payload has no such table") };
    match field_offset(payload, table, slot)? {
        None => add_table_field(payload, table, slot, &[1]),
        Some(field) => {
            let mut buffer = payload.to_vec();
            if field >= buffer.len() as i64 {
                return Err(PyError::Index("bytearray index out of range".into()));
            }
            buffer[field as usize] = 1;
            Ok(buffer)
        }
    }
}

/// Raise a style group's enable flag (`_write_table_flag(payload, _style_table, slot)`).
pub fn write_style_flag(payload: &[u8], slot: usize) -> PyResult<Vec<u8>> {
    write_table_flag(payload, style_table, slot)
}

/// Raise a block group's enable flag (`_write_table_flag(payload, _document_table, slot)`).
pub fn write_block_flag(payload: &[u8], slot: usize) -> PyResult<Vec<u8>> {
    write_table_flag(payload, document_locate, slot)
}

/// `write_style_float`: set a float slot of the first run's style table.
pub fn write_style_float(payload: &[u8], slot: usize, value: f64) -> PyResult<Vec<u8>> {
    let Some(table) = style_table(payload)? else { return super::value_err("payload has no run style table") };
    write_table_float(payload, table, slot, value)
}

/// `write_style_color`: set a colour slot of the first run's style table.
pub fn write_style_color(payload: &[u8], slot: usize, color: Color) -> PyResult<Vec<u8>> {
    write_table_color(payload, style_table, slot, color)
}

/// `write_block_float`: set a float slot of the block-level document table.
pub fn write_block_float(payload: &[u8], slot: usize, value: f64) -> PyResult<Vec<u8>> {
    write_table_float(payload, document_table(payload)?, slot, value)
}

/// `write_block_color`: set a colour slot of the block-level document table.
pub fn write_block_color(payload: &[u8], slot: usize, color: Color) -> PyResult<Vec<u8>> {
    write_table_color(payload, document_locate, slot, color)
}

/// `read_style_float`: a float slot of the first run's style table, `None` if absent.
pub fn read_style_float(payload: &[u8], slot: usize) -> PyResult<Option<f64>> {
    read_float(payload, style_table(payload)?, slot)
}

/// `read_style_color`.
pub fn read_style_color(payload: &[u8], slot: usize) -> PyResult<Option<Color>> {
    read_rgb(payload, style_table(payload)?, slot)
}

/// `read_block_float`.
pub fn read_block_float(payload: &[u8], slot: usize) -> PyResult<Option<f64>> {
    read_float(payload, Some(document_table(payload)?), slot)
}

/// `read_block_color`.
pub fn read_block_color(payload: &[u8], slot: usize) -> PyResult<Option<Color>> {
    read_rgb(payload, Some(document_table(payload)?), slot)
}

/// `read_font_family`: the font family the payload's text is set in.
pub fn read_font_family(payload: &[u8]) -> PyResult<String> {
    let (offset, length) = font_string_span(payload)?;
    decode_utf8(slice(payload, offset + 4, offset + 4 + length))
}

/// `write_font_family`: the payload with its font family replaced by `name`.
pub fn write_font_family(payload: &[u8], name: &str) -> PyResult<Vec<u8>> {
    if name.is_empty() {
        return super::value_err("font family cannot be empty");
    }
    let (offset, length) = font_string_span(payload)?;
    splice_string(payload, offset, length, name)
}

// ---- BinaryHash of a rewritten payload ------------------------------------------------------------

/// `_PAYLOAD_NAMESPACE`: `6f0f2f24-2d8e-5a3a-9a0f-0f4a2f0d9c11`.
const PAYLOAD_NAMESPACE: [u8; 16] =
    [0x6f, 0x0f, 0x2f, 0x24, 0x2d, 0x8e, 0x5a, 0x3a, 0x9a, 0x0f, 0x0f, 0x4a, 0x2f, 0x0d, 0x9c, 0x11];

/// `write_payload_element`: store a rewritten payload on its element under a fresh
/// (content-derived) hash. Refuses an element that is the STORED copy of a hash others reference.
pub fn write_payload_element(document: &mut PremiereDocument, element: NodeId, payload: &[u8]) -> PyResult<()> {
    let tree = &document.tree;
    let old_hash = tree.get(element, "BinaryHash").map(str::to_string);
    if !py_strip(tree[element].text_or_empty()).is_empty() && old_hash.as_deref().is_some_and(|h| !h.is_empty()) {
        let old_hash = old_hash.expect("checked");
        for other in tree.iter(document.root) {
            if other != element
                && tree.get(other, "BinaryHash") == Some(old_hash.as_str())
                && py_strip(tree[other].text_or_empty()).is_empty()
            {
                return Err(PyError::NotImplemented(
                    "this element carries the stored payload others reference; rewriting it is not supported yet".into(),
                ));
            }
        }
    }
    let hash = crate::hash::uuid5(&PAYLOAD_NAMESPACE, &crate::hash::hex(payload));
    document.tree.set(element, "BinaryHash", hash);
    document.tree[element].text = Some(b64encode(payload) + "\n\t\t");
    document.invalidate_payloads();
    Ok(())
}

// ---- font size -------------------------------------------------------------------------------------

/// Head offsets of the FormattedTextData fields the font-size forms touch: the four backward
/// relative-offset words that cross the run struct.
const CROSSING_WORDS: [i64; 4] = [0xCC, 0xD0, 0x100, 0x108];
/// Tail-relative offsets (from the end of the padded text string).
const TAIL_OBJECT_SIZE: i64 = 4;
const TAIL_SIZE_SLOT: i64 = 8;
const TAIL_FORWARD_WORDS: [i64; 3] = [64, 68, 76];
const TAIL_SIZE_FLOAT: i64 = 80;
/// The run struct's object sizes with and without the override field, and the override's
/// offset within the struct.
const RUN_WITH_OVERRIDE: i64 = 0x1C;
const RUN_WITHOUT_OVERRIDE: i64 = 0x18;
const SIZE_FIELD_OFFSET: i64 = 0x18;

/// `_tail_offset`: the end of the padded text string.
fn tail_offset(payload: &[u8]) -> PyResult<i64> {
    let Some((offset, length)) = last_string_span(payload)? else {
        return super::value_err("payload holds no caption text");
    };
    Ok(offset + 4 + (length as i64 + 4) / 4 * 4)
}

/// `_has_default_run_layout`: the as-imported payload family, whose run style table sits right
/// after the text and holds only the size override.
fn has_default_run_layout(payload: &[u8]) -> PyResult<bool> {
    let Some(style) = style_table(payload)? else { return Ok(false) };
    let Some(vtable) = valid_vtable(payload, style)? else { return Ok(false) };
    if vtable != tail_offset(payload)? + 2 {
        return Ok(false);
    }
    let object_size = u16_at(payload, vtable + 2)?;
    if object_size != RUN_WITH_OVERRIDE && object_size != RUN_WITHOUT_OVERRIDE {
        return Ok(false);
    }
    // `CROSSING_WORDS` are ABSOLUTE positions: confirm they are all still offset words.
    let positions: HashSet<i64> = collect_offset_words(payload)?.into_iter().map(|w| w.0).collect();
    Ok(CROSSING_WORDS.iter().all(|p| positions.contains(p)))
}

/// `read_font_size`: the effective font size (the run's override, else the default 100).
pub fn read_font_size(payload: &[u8]) -> PyResult<f64> {
    Ok(read_style_float(payload, STYLE_SIZE_SLOT)?.unwrap_or(DEFAULT_FONT_SIZE))
}

/// `write_font_size`: the payload with its font size set to `value`.
pub fn write_font_size(payload: &[u8], value: f64) -> PyResult<Vec<u8>> {
    if !has_default_run_layout(payload)? {
        return write_style_float(payload, STYLE_SIZE_SLOT, value);
    }
    let tail = tail_offset(payload)?;
    let object_size = u16_at(payload, tail + TAIL_OBJECT_SIZE)?;
    let base = DEFAULT_FONT_SIZE;
    let mut buffer = payload.to_vec();
    if object_size == RUN_WITH_OVERRIDE {
        if value != base {
            put_f32(&mut buffer, tail + TAIL_SIZE_FLOAT, value)?;
            return Ok(buffer);
        }
        let length = u64_at(&buffer, 0)?;
        put_u64(&mut buffer, 0, length - 4)?;
        for position in CROSSING_WORDS {
            let word = u32_at(&buffer, position)?;
            put_u32(&mut buffer, position, ((word + 4) & 0xFFFF_FFFF) as i128)?;
        }
        for offset in TAIL_FORWARD_WORDS {
            let word = u32_at(&buffer, tail + offset)?;
            put_u32(&mut buffer, tail + offset, (word - 4) as i128)?;
        }
        put_u16(&mut buffer, tail + TAIL_OBJECT_SIZE, RUN_WITHOUT_OVERRIDE as i128)?;
        put_u16(&mut buffer, tail + TAIL_SIZE_SLOT, 0)?;
        let from = ((tail + TAIL_SIZE_FLOAT).max(0) as usize).min(buffer.len());
        let to = ((tail + TAIL_SIZE_FLOAT + 4).max(0) as usize).min(buffer.len());
        buffer.drain(from..to);
        return Ok(buffer);
    }
    if object_size == RUN_WITHOUT_OVERRIDE {
        if value == base {
            return Ok(buffer);
        }
        let length = u64_at(&buffer, 0)?;
        put_u64(&mut buffer, 0, length + 4)?;
        for position in CROSSING_WORDS {
            let word = u32_at(&buffer, position)?;
            put_u32(&mut buffer, position, ((word - 4) & 0xFFFF_FFFF) as i128)?;
        }
        for offset in TAIL_FORWARD_WORDS {
            let word = u32_at(&buffer, tail + offset)?;
            put_u32(&mut buffer, tail + offset, (word + 4) as i128)?;
        }
        put_u16(&mut buffer, tail + TAIL_OBJECT_SIZE, RUN_WITH_OVERRIDE as i128)?;
        put_u16(&mut buffer, tail + TAIL_SIZE_SLOT, SIZE_FIELD_OFFSET as i128)?;
        let at = ((tail + TAIL_SIZE_FLOAT).max(0) as usize).min(buffer.len());
        let float = pack_float32(value)?;
        buffer.splice(at..at, float);
        return Ok(buffer);
    }
    super::value_err(format!("unknown caption run layout (object size {:#x})", object_size))
}
