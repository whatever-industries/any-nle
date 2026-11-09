//! Port of the media-file readers of py_premiere `models/media_import.py` (stills, movie
//! containers, audio files) and `models/project.py` `_probe_movie`.
//!
//! Every reader takes the whole file as bytes, like Python's, and gives Python's result or
//! Python's exception class and message ([`Error::py_class`]). Offsets come from the file, so
//! all offset arithmetic is done in `u128` (no sum of a few file fields can overflow it) and
//! every read is bounds-checked the way `_unpack_from` checks it; slices clamp like Python
//! slicing. Iteration over ISO boxes is lazy, as Python's generator is, so the first error
//! raised is the same one. Integer results are `i128` (Python ints): the largest a reader can
//! produce (a WMA duration in samples) stays far below its range.
//!
//! The one place Python's unbounded ints cannot be followed is an MXF frame size, which is
//! `int.from_bytes` of a value of any length: one that does not fit in an `i64` is refused with
//! an `OverflowError` here where Python would carry the huge number on.

use crate::error::{Error, Result};
use std::collections::HashMap;

/// `TICKS_PER_SECOND`.
pub const TICKS_PER_SECOND: i128 = 254_016_000_000;

type Off = u128;

fn len(data: &[u8]) -> Off {
    data.len() as Off
}

/// Python `data[a:b]` for non-negative `a`, `b`.
fn sl(data: &[u8], a: Off, b: Off) -> &[u8] {
    let n = data.len();
    let a = a.min(n as Off) as usize;
    let b = b.min(n as Off) as usize;
    if a >= b {
        &[]
    } else {
        &data[a..b]
    }
}

/// Python `data[i]`.
fn byte(data: &[u8], i: Off) -> Result<u8> {
    if i < len(data) {
        Ok(data[i as usize])
    } else {
        Err(Error::Index("index out of range".into()))
    }
}

/// `_unpack_from`'s bounds check: the `size` bytes at `offset`.
fn read(data: &[u8], size: usize, offset: Off) -> Result<&[u8]> {
    if offset + size as Off > len(data) {
        return Err(Error::Value(format!(
            "malformed media file: cannot read {size} bytes at {offset} of {}",
            data.len()
        )));
    }
    let at = offset as usize;
    Ok(&data[at..at + size])
}

fn be(bytes: &[u8]) -> u128 {
    bytes.iter().fold(0u128, |acc, &b| (acc << 8) | b as u128)
}

fn le(bytes: &[u8]) -> u128 {
    bytes.iter().rev().fold(0u128, |acc, &b| (acc << 8) | b as u128)
}

fn u16be(data: &[u8], at: Off) -> Result<u128> {
    read(data, 2, at).map(be)
}
fn u32be(data: &[u8], at: Off) -> Result<u128> {
    read(data, 4, at).map(be)
}
fn u64be(data: &[u8], at: Off) -> Result<u128> {
    read(data, 8, at).map(be)
}
fn u16le(data: &[u8], at: Off) -> Result<u128> {
    read(data, 2, at).map(le)
}
fn u32le(data: &[u8], at: Off) -> Result<u128> {
    read(data, 4, at).map(le)
}
fn u64le(data: &[u8], at: Off) -> Result<u128> {
    read(data, 8, at).map(le)
}
fn i32le(data: &[u8], at: Off) -> Result<i128> {
    Ok(read(data, 4, at).map(le)? as u32 as i32 as i128)
}

/// `bytes.decode("ascii", "replace")`.
fn ascii_replace(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| if b < 0x80 { b as char } else { '\u{FFFD}' }).collect()
}

/// `bytes.hex()`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `bytes.index(needle, start)`.
fn index_of(data: &[u8], needle: &[u8], start: Off) -> Result<Off> {
    let not_found = || Error::Value("subsection not found".into());
    if start > len(data) {
        return Err(not_found());
    }
    let s = start as usize;
    data[s..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| (s + p) as Off)
        .ok_or_else(not_found)
}

fn int(v: u128) -> i128 {
    v as i128
}

/// Python floor division for a positive or negative divisor (`ZeroDivisionError` on zero).
pub fn py_floordiv(a: i128, b: i128) -> Result<i128> {
    if b == 0 {
        return Err(Error::ZeroDivision("division by zero".into()));
    }
    Ok(a.div_euclid(b) - if b < 0 && a.rem_euclid(b) != 0 { 1 } else { 0 })
}

/// Python `a % b` (sign of the divisor; `ZeroDivisionError` on zero).
pub fn py_mod(a: i128, b: i128) -> Result<i128> {
    Ok(a - b * py_floordiv(a, b)?)
}

// ---- stills -------------------------------------------------------------------------------------

/// `read_bmp_size`: `(width, height)` of a BMP (BITMAPINFOHEADER).
pub fn read_bmp_size(data: &[u8]) -> Result<(i128, i128)> {
    if sl(data, 0, 2) != b"BM" || data.len() < 26 {
        return Err(Error::Value("not a BMP file".into()));
    }
    Ok((i32le(data, 18)?, i32le(data, 22)?.abs()))
}

/// `read_png_size`: `(width, height)` of a PNG (IHDR).
pub fn read_png_size(data: &[u8]) -> Result<(i128, i128)> {
    if sl(data, 0, 8) != b"\x89PNG\r\n\x1a\n" || data.len() < 24 {
        return Err(Error::Value("not a PNG file".into()));
    }
    Ok((int(u32be(data, 16)?), int(u32be(data, 20)?)))
}

/// `read_tiff_size`: `(width, height)` of a TIFF (its first IFD).
pub fn read_tiff_size(data: &[u8]) -> Result<(i128, i128)> {
    if data.len() < 8 {
        return Err(Error::Value("not a TIFF file".into()));
    }
    let big = match sl(data, 0, 2) {
        b"II" => false,
        b"MM" => true,
        _ => return Err(Error::Value("not a TIFF file".into())),
    };
    let r16 = |at: Off| if big { u16be(data, at) } else { u16le(data, at) };
    let r32 = |at: Off| if big { u32be(data, at) } else { u32le(data, at) };
    let ifd = r32(4)?;
    if ifd + 2 > len(data) {
        return Err(Error::Value("truncated TIFF file".into()));
    }
    let count = r16(ifd)?;
    let mut sizes: HashMap<u128, u128> = HashMap::new();
    for index in 0..count {
        let entry = ifd + 2 + index * 12;
        if entry + 12 > len(data) {
            return Err(Error::Value("truncated TIFF file".into()));
        }
        read(data, 4, entry)?;
        let tag = r16(entry)?;
        let kind = r16(entry + 2)?;
        if tag == 0x0100 || tag == 0x0101 {
            let value = if kind == 3 { r16(entry + 8)? } else { r32(entry + 8)? };
            sizes.insert(tag, value);
        }
    }
    match (sizes.get(&0x0100), sizes.get(&0x0101)) {
        (Some(&w), Some(&h)) => Ok((int(w), int(h))),
        _ => Err(Error::Value("TIFF file has no image dimensions".into())),
    }
}

/// `read_psd_size`: `(width, height)` of a PSD/PSB (its file header).
pub fn read_psd_size(data: &[u8]) -> Result<(i128, i128)> {
    if sl(data, 0, 4) != b"8BPS" || data.len() < 26 {
        return Err(Error::Value("not a PSD file".into()));
    }
    let height = u32be(data, 14)?;
    let width = u32be(data, 18)?;
    Ok((int(width), int(height)))
}

/// `read_gif_size`: `(width, height)` of a GIF (its logical screen descriptor).
pub fn read_gif_size(data: &[u8]) -> Result<(i128, i128)> {
    let magic = sl(data, 0, 6);
    if (magic != b"GIF87a" && magic != b"GIF89a") || data.len() < 10 {
        return Err(Error::Value("not a GIF file".into()));
    }
    Ok((int(u16le(data, 6)?), int(u16le(data, 8)?)))
}

/// `read_jpeg_size`: `(width, height)` of a JPEG (its start-of-frame marker).
pub fn read_jpeg_size(data: &[u8]) -> Result<(i128, i128)> {
    if sl(data, 0, 2) != b"\xff\xd8" {
        return Err(Error::Value("not a JPEG file".into()));
    }
    let mut offset: Off = 2;
    while offset + 4 <= len(data) {
        if byte(data, offset)? != 0xFF {
            return Err(Error::Value("malformed JPEG segment".into()));
        }
        let marker = byte(data, offset + 1)?;
        let length = u16be(data, offset + 2)?;
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            if offset + 9 > len(data) {
                return Err(Error::Value("truncated JPEG start-of-frame marker".into()));
            }
            read(data, 4, offset + 5)?;
            let height = u16be(data, offset + 5)?;
            let width = u16be(data, offset + 7)?;
            return Ok((int(width), int(height)));
        }
        offset += 2 + length;
    }
    Err(Error::Value("JPEG file has no start-of-frame marker".into()))
}

// ---- AVI ----------------------------------------------------------------------------------------

/// `read_avi_info`'s `(width, height, fps, frames, codec)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AviInfo {
    pub width: i128,
    pub height: i128,
    pub fps: i128,
    pub frames: i128,
    pub codec: String,
}

/// `read_avi_info`.
pub fn read_avi_info(data: &[u8]) -> Result<AviInfo> {
    if sl(data, 0, 4) != b"RIFF" || sl(data, 8, 12) != b"AVI " {
        return Err(Error::Value("not an AVI file".into()));
    }
    let strh = index_of(data, b"strh", 0)? + 8;
    if sl(data, strh, strh + 4) != b"vids" {
        return Err(Error::Value("first AVI stream is not video".into()));
    }
    let codec = ascii_replace(sl(data, strh + 4, strh + 8));
    read(data, 8, strh + 20)?;
    let scale = u32le(data, strh + 20)?;
    let rate = u32le(data, strh + 24)?;
    let frames = u32le(data, strh + 32)?;
    let strf = index_of(data, b"strf", strh)? + 8;
    read(data, 8, strf + 4)?;
    let width = i32le(data, strf + 4)?;
    let height = i32le(data, strf + 8)?;
    if scale == 0 || rate % scale != 0 {
        return Err(Error::NotImplemented("non-integer AVI frame rates are not supported".into()));
    }
    Ok(AviInfo { width, height: height.abs(), fps: int(rate / scale), frames: int(frames), codec })
}

// ---- ISO base media (MP4 / MOV / M4A) -----------------------------------------------------------

/// `MovieAudioInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieAudioInfo {
    pub sample_rate: i128,
    pub channels: i128,
    pub frames: i128,
}

/// `MovieInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieInfo {
    pub width: i128,
    pub height: i128,
    pub timescale: i128,
    pub frame_duration: i128,
    pub frames: i128,
    pub codec: String,
    pub start_timecode: i128,
    pub drop_frame: bool,
    pub field_order: i128,
    pub audio: Option<MovieAudioInfo>,
    pub clip_id: String,
}

/// `_boxes`, lazily: each `next` parses one box header.
struct Boxes<'a> {
    data: &'a [u8],
    offset: Off,
    end: Off,
}

/// One box: `(kind, body offset, box end)`.
struct BoxSpan<'a> {
    kind: &'a [u8],
    body: Off,
    end: Off,
}

impl<'a> Boxes<'a> {
    fn new(data: &'a [u8], start: Off, end: Off) -> Self {
        Boxes { data, offset: start, end }
    }

    fn next_box(&mut self) -> Result<Option<BoxSpan<'a>>> {
        let (data, offset, end) = (self.data, self.offset, self.end);
        if offset + 8 > end {
            return Ok(None);
        }
        let mut size = u32be(data, offset)?;
        let kind = sl(data, offset + 4, offset + 8);
        let mut body = offset + 8;
        if size == 1 {
            size = u64be(data, body)?;
            body += 8;
        } else if size == 0 {
            size = end - offset;
        }
        if size < 8 || offset + size > end {
            return Err(Error::Value("malformed box in movie container".into()));
        }
        self.offset = offset + size;
        Ok(Some(BoxSpan { kind, body, end: offset + size }))
    }
}

/// `_box`: a nested box by path, as `(body offset, box end)`.
fn find_box(data: &[u8], start: Off, end: Off, path: &[&[u8]]) -> Result<Option<(Off, Off)>> {
    let mut span = (start, end);
    for kind in path {
        let mut boxes = Boxes::new(data, span.0, span.1);
        let mut found = None;
        while let Some(b) = boxes.next_box()? {
            if b.kind == *kind {
                found = Some((b.body, b.end));
                break;
            }
        }
        match found {
            Some(f) => span = f,
            None => return Ok(None),
        }
    }
    Ok(Some(span))
}

/// `_read_field_order`.
fn read_field_order(data: &[u8], entry: Off, entry_end: Off) -> Result<i128> {
    let mut boxes = Boxes::new(data, entry + 86, entry_end);
    while let Some(b) = boxes.next_box()? {
        if b.kind != b"fiel" {
            continue;
        }
        let fields = byte(data, b.body)?;
        let detail = byte(data, b.body + 1)?;
        if fields != 2 {
            return Ok(0);
        }
        return match detail {
            1 | 9 => Ok(1),
            6 | 14 => Ok(2),
            _ => Err(Error::NotImplemented(format!("unknown fiel field detail {detail}"))),
        };
    }
    Ok(0)
}

/// `_Track`.
struct Track<'a> {
    handler: &'a [u8],
    timescale: u128,
    format: &'a [u8],
    samples: u128,
    sample_delta: u128,
    width: u128,
    height: u128,
    channels: u128,
    sample_rate: u128,
    first_chunk: u128,
    field_order: i128,
    drop_frame: bool,
    sample_delta_runs: u128,
}

/// `_read_track`.
fn read_track(data: &[u8], start: Off, end: Off) -> Result<Track<'_>> {
    let mut t = Track {
        handler: b"",
        timescale: 0,
        format: b"",
        samples: 0,
        sample_delta: 0,
        width: 0,
        height: 0,
        channels: 0,
        sample_rate: 0,
        first_chunk: 0,
        field_order: 0,
        drop_frame: false,
        sample_delta_runs: 0,
    };
    let Some(media) = find_box(data, start, end, &[b"mdia"])? else {
        return Err(Error::Value("movie track has no mdia box".into()));
    };
    if let Some(hdlr) = find_box(data, media.0, media.1, &[b"hdlr"])? {
        t.handler = sl(data, hdlr.0 + 8, hdlr.0 + 12);
    }
    if let Some(mdhd) = find_box(data, media.0, media.1, &[b"mdhd"])? {
        let version = byte(data, mdhd.0)?;
        t.timescale = u32be(data, mdhd.0 + if version != 0 { 20 } else { 12 })?;
    }
    let Some(table) = find_box(data, media.0, media.1, &[b"minf", b"stbl"])? else {
        return Err(Error::Value("movie track has no sample table".into()));
    };
    if let Some(stsd) = find_box(data, table.0, table.1, &[b"stsd"])? {
        let entry = stsd.0 + 8;
        let entry_end = entry + u32be(data, entry)?;
        t.format = sl(data, entry + 4, entry + 8);
        if t.handler == b"vide" {
            read(data, 4, entry + 32)?;
            t.width = u16be(data, entry + 32)?;
            t.height = u16be(data, entry + 34)?;
            t.field_order = read_field_order(data, entry, entry_end)?;
        } else if t.handler == b"soun" {
            if u16be(data, entry + 16)? == 2 {
                return Err(Error::NotImplemented(
                    "version 2 audio sample descriptions are not supported".into(),
                ));
            }
            t.channels = u16be(data, entry + 24)?;
            t.sample_rate = u16be(data, entry + 32)?;
        } else if t.handler == b"tmcd" {
            t.drop_frame = u32be(data, entry + 20)? & 1 != 0;
        }
    }
    if let Some(stts) = find_box(data, table.0, table.1, &[b"stts"])? {
        t.sample_delta_runs = u32be(data, stts.0 + 4)?;
        t.sample_delta = u32be(data, stts.0 + 12)?;
    }
    if let Some(stsz) = find_box(data, table.0, table.1, &[b"stsz"])? {
        t.samples = u32be(data, stsz.0 + 8)?;
    }
    if let Some(chunks) = find_box(data, table.0, table.1, &[b"stco"])? {
        t.first_chunk = u32be(data, chunks.0 + 8)?;
    } else if let Some(chunks) = find_box(data, table.0, table.1, &[b"co64"])? {
        t.first_chunk = u64be(data, chunks.0 + 8)?;
    }
    Ok(t)
}

/// `read_mp4_info`: the `moov` metadata of an MP4/MOV.
pub fn read_mp4_info(data: &[u8]) -> Result<MovieInfo> {
    let Some(moov) = find_box(data, 0, len(data), &[b"moov"])? else {
        return Err(Error::Value("not an MP4/MOV file".into()));
    };
    let mut video: Option<Track> = None;
    let mut audio: Option<Track> = None;
    let mut start_timecode = 0u128;
    let mut drop_frame = false;
    let mut boxes = Boxes::new(data, moov.0, moov.1);
    while let Some(b) = boxes.next_box()? {
        if b.kind != b"trak" {
            continue;
        }
        let track = read_track(data, b.body, b.end)?;
        if track.handler == b"vide" && video.is_none() {
            video = Some(track);
        } else if track.handler == b"soun" && audio.is_none() {
            audio = Some(track);
        } else if track.handler == b"tmcd" && track.first_chunk != 0 {
            start_timecode = u32be(data, track.first_chunk)?;
            drop_frame = track.drop_frame;
        }
    }
    let Some(video) = video else {
        return Err(Error::Value("movie container has no video track".into()));
    };
    if video.sample_delta == 0 || video.timescale == 0 {
        return Err(Error::Value("movie video track has no frame duration".into()));
    }
    if video.sample_delta_runs > 1 {
        return Err(Error::NotImplemented(format!(
            "variable frame rate movies are not supported ({} stts runs)",
            video.sample_delta_runs
        )));
    }
    Ok(MovieInfo {
        width: int(video.width),
        height: int(video.height),
        timescale: int(video.timescale),
        frame_duration: int(video.sample_delta),
        frames: int(video.samples),
        codec: ascii_replace(video.format),
        start_timecode: int(start_timecode),
        drop_frame,
        field_order: video.field_order,
        audio: audio.map(|a| MovieAudioInfo {
            sample_rate: int(a.sample_rate),
            channels: int(a.channels),
            frames: int(a.samples),
        }),
        clip_id: String::new(),
    })
}

// ---- MXF ----------------------------------------------------------------------------------------

const MXF_STORED_WIDTH: u16 = 0x3203;
const MXF_STORED_HEIGHT: u16 = 0x3202;
const MXF_DISPLAY_WIDTH: u16 = 0x3205;
const MXF_DISPLAY_HEIGHT: u16 = 0x3208;
const MXF_SAMPLE_RATE: u16 = 0x3001;
const MXF_CONTAINER_DURATION: u16 = 0x3002;
const MXF_DURATION: u16 = 0x0202;
const MXF_PICTURE_CODING: u16 = 0x3201;
const MXF_PACKAGE_UID: u16 = 0x4401;

/// `MXF_PICTURE_CODINGS`: codec per picture-essence coding key (`coding[8:14].hex()`).
pub fn mxf_picture_coding(key: &str) -> Option<&'static str> {
    match key {
        "040102020101" => Some("MPEG"),
        _ => None,
    }
}

/// A BER length beyond this compares like Python's (huge) int against any offset in a file;
/// capping keeps the sums in range.
const BER_CAP: u128 = 1 << 100;

/// `_mxf_ber`: `(length, offset after it)`.
fn mxf_ber(data: &[u8], offset: Off) -> Result<(u128, Off)> {
    let first = byte(data, offset)?;
    if first < 0x80 {
        return Ok((first as u128, offset + 1));
    }
    let count = (first & 0x7F) as u128;
    let bytes = sl(data, offset + 1, offset + 1 + count);
    let mut value: u128 = 0;
    for &b in bytes {
        value = if value >= BER_CAP >> 8 { BER_CAP } else { ((value << 8) | b as u128).min(BER_CAP) };
    }
    Ok((value, offset + 1 + count))
}

/// `int.from_bytes(value, "big")`, refused when it does not fit an `i64`.
fn from_bytes_i64(value: &[u8], what: &str) -> Result<i128> {
    let significant: &[u8] = {
        let skip = value.iter().take_while(|&&b| b == 0).count();
        &value[skip..]
    };
    if significant.len() > 8 || (significant.len() == 8 && significant[0] >= 0x80) {
        return Err(Error::Overflow(format!("MXF {what} does not fit in 64 bits")));
    }
    Ok(be(significant) as i128)
}

/// `read_mxf_info`: the header metadata of an MXF.
pub fn read_mxf_info(data: &[u8]) -> Result<MovieInfo> {
    if sl(data, 0, 4) != b"\x06\x0e\x2b\x34" {
        return Err(Error::Value("not an MXF file".into()));
    }
    let mut picture: Option<HashMap<u16, &[u8]>> = None;
    let mut frames: u128 = 0;
    let mut clip_id = String::new();
    // `_mxf_local_sets`, consumed as it is produced.
    let mut offset: Off = 0;
    while offset + 17 < len(data) {
        if sl(data, offset, offset + 4) != b"\x06\x0e\x2b\x34" {
            break;
        }
        let (length, value) = mxf_ber(data, offset + 16)?;
        if sl(data, offset + 4, offset + 6) == b"\x02\x53" {
            let mut tags: HashMap<u16, &[u8]> = HashMap::new();
            let mut position = value;
            let end = value + length;
            while position + 4 <= end {
                read(data, 4, position)?;
                let tag = u16be(data, position)? as u16;
                let size = u16be(data, position + 2)?;
                tags.insert(tag, sl(data, position + 4, position + 4 + size));
                position += 4 + size;
            }
            if tags.contains_key(&MXF_STORED_WIDTH) && picture.is_none() {
                picture = Some(tags.clone());
            }
            if let Some(duration) = tags.get(&MXF_DURATION) {
                if duration.len() == 8 {
                    frames = frames.max(be(duration));
                }
            }
            if let Some(uid) = tags.get(&MXF_PACKAGE_UID) {
                if clip_id.is_empty() {
                    clip_id = hex(uid).to_uppercase();
                }
            }
        }
        offset = value + length;
    }
    let Some(picture) = picture else {
        return Err(Error::Value("MXF file has no picture descriptor".into()));
    };
    let pick = |display: u16, stored: u16| -> Result<&[u8]> {
        match picture.get(&display) {
            Some(v) if !v.is_empty() => Ok(v),
            _ => picture.get(&stored).copied().ok_or_else(|| Error::Key(stored.to_string())),
        }
    };
    let width = from_bytes_i64(pick(MXF_DISPLAY_WIDTH, MXF_STORED_WIDTH)?, "frame width")?;
    let height = from_bytes_i64(pick(MXF_DISPLAY_HEIGHT, MXF_STORED_HEIGHT)?, "frame height")?;
    let rate = match picture.get(&MXF_SAMPLE_RATE) {
        Some(r) if r.len() == 8 => *r,
        _ => return Err(Error::Value("MXF picture descriptor has no sample rate".into())),
    };
    let timescale = be(&rate[..4]);
    let frame_duration = be(&rate[4..]);
    if let Some(container) = picture.get(&MXF_CONTAINER_DURATION) {
        if container.len() == 8 {
            frames = be(container);
        }
    }
    let Some(coding) = picture.get(&MXF_PICTURE_CODING) else {
        return Err(Error::Value("MXF picture descriptor has no essence coding label".into()));
    };
    let key = hex(sl(coding, 8, 14));
    let Some(codec) = mxf_picture_coding(&key) else {
        return Err(Error::NotImplemented(format!(
            "no verified codec for MXF picture essence coding {}",
            hex(coding)
        )));
    };
    Ok(MovieInfo {
        width,
        height,
        timescale: int(timescale),
        frame_duration: int(frame_duration),
        frames: int(frames),
        codec: codec.into(),
        start_timecode: 0,
        drop_frame: false,
        field_order: 0,
        audio: None,
        clip_id,
    })
}

// ---- audio --------------------------------------------------------------------------------------

/// `WavInfo`: what an audio import needs from any audio file's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WavInfo {
    pub sample_rate: i128,
    pub frames: i128,
    pub channels: i128,
    pub sample_width: i128,
    pub format_tag: i128,
    pub channel_mask: i128,
}

impl WavInfo {
    fn compressed(sample_rate: i128, frames: i128, channels: i128) -> Self {
        WavInfo { sample_rate, frames, channels, sample_width: 0, format_tag: 0, channel_mask: 0 }
    }
}

pub const WAVE_FORMAT_PCM: i128 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: i128 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: i128 = 0xFFFE;

/// `read_wav_info`: a WAV's `fmt `/`data` chunks.
pub fn read_wav_info(data: &[u8]) -> Result<WavInfo> {
    if sl(data, 0, 4) != b"RIFF" || sl(data, 8, 12) != b"WAVE" {
        return Err(Error::Value("not a WAV file".into()));
    }
    let mut fmt_at: Off = 0;
    let mut fmt_size: u128 = 0;
    let mut data_bytes: u128 = 0;
    let mut offset: Off = 12;
    while offset + 8 <= len(data) {
        let kind = sl(data, offset, offset + 4);
        let size = u32le(data, offset + 4)?;
        let body = offset + 8;
        if kind == b"fmt " {
            fmt_at = body;
            fmt_size = size;
        } else if kind == b"data" {
            data_bytes = size.min(len(data).saturating_sub(body));
        }
        offset = body + size + (size % 2);
    }
    if fmt_size < 16 {
        return Err(Error::Value("WAV file has no fmt chunk".into()));
    }
    read(data, 16, fmt_at)?;
    let mut tag = u16le(data, fmt_at)?;
    let channels = u16le(data, fmt_at + 2)?;
    let sample_rate = u32le(data, fmt_at + 4)?;
    let mut block_align = u16le(data, fmt_at + 12)?;
    let bits = u16le(data, fmt_at + 14)?;
    let mut mask = 0u128;
    if tag as i128 == WAVE_FORMAT_EXTENSIBLE {
        if fmt_size < 40 {
            return Err(Error::Value("truncated WAVE_FORMAT_EXTENSIBLE fmt chunk".into()));
        }
        mask = u32le(data, fmt_at + 20)?;
        tag = u32le(data, fmt_at + 24)?;
    }
    if block_align == 0 {
        block_align = channels * (bits / 8);
    }
    if block_align == 0 {
        return Err(Error::Value(format!(
            "WAV fmt chunk declares no frame size ({channels} channels, {bits} bits)"
        )));
    }
    if sample_rate == 0 {
        return Err(Error::Value("WAV fmt chunk declares a zero sample rate".into()));
    }
    Ok(WavInfo {
        sample_rate: int(sample_rate),
        frames: int(data_bytes / block_align),
        channels: int(channels),
        sample_width: int(bits / 8),
        format_tag: int(tag),
        channel_mask: int(mask),
    })
}

/// `_read_extended`: an 80-bit IEEE extended float, as a whole number.
fn read_extended(raw: &[u8]) -> Result<i128> {
    if raw.len() < 10 {
        return Err(Error::Value("truncated AIFF sample-rate field".into()));
    }
    let exponent = (((raw[0] & 0x7F) as i64) << 8) | raw[1] as i64;
    let mantissa = be(&raw[2..10]);
    if exponent == 0 && mantissa == 0 {
        return Ok(0);
    }
    let shift = 16383 + 63 - exponent;
    if shift < 0 {
        return Err(Error::Value("negative shift count".into()));
    }
    Ok(if shift >= 128 { 0 } else { int(mantissa >> shift) })
}

/// `read_aiff_info`: an AIFF `COMM` chunk, shaped like a WAV header.
pub fn read_aiff_info(data: &[u8]) -> Result<WavInfo> {
    if sl(data, 0, 4) != b"FORM" || sl(data, 8, 12) != b"AIFF" {
        return Err(Error::Value("not an AIFF file".into()));
    }
    let mut offset: Off = 12;
    while offset + 8 <= len(data) {
        let kind = sl(data, offset, offset + 4);
        let size = u32be(data, offset + 4)?;
        let body = offset + 8;
        if kind == b"COMM" {
            read(data, 8, body)?;
            let channels = u16be(data, body)?;
            let frames = u32be(data, body + 2)?;
            let bits = u16be(data, body + 6)?;
            let rate = read_extended(sl(data, body + 8, body + 18))?;
            return Ok(WavInfo {
                sample_rate: rate,
                frames: int(frames),
                channels: int(channels),
                sample_width: int(bits / 8),
                format_tag: WAVE_FORMAT_PCM,
                channel_mask: 0,
            });
        }
        offset = body + size + (size % 2);
    }
    Err(Error::Value("AIFF file has no COMM chunk".into()))
}

/// `_skip_id3`: where the audio starts after a leading ID3v2 tag.
fn skip_id3(data: &[u8]) -> Off {
    if sl(data, 0, 3) != b"ID3" || data.len() < 10 {
        return 0;
    }
    let s = &data[6..10];
    10 + (((s[0] & 0x7F) as Off) << 21 | ((s[1] & 0x7F) as Off) << 14 | ((s[2] & 0x7F) as Off) << 7 | (s[3] & 0x7F) as Off)
}

/// `_mp3_trim`: the encoder delay plus padding an MP3's encoder tag declares.
fn mp3_trim(data: &[u8], tag_at: Off, flags: u128) -> i128 {
    let mut offset = tag_at + 8;
    for (flag, size) in [(0x1, 4), (0x2, 4), (0x4, 100), (0x8, 4)] {
        if flags & flag != 0 {
            offset += size;
        }
    }
    if len(data) < offset + 24 {
        return 0;
    }
    let at = offset as usize;
    let (first, second, third) = (data[at + 21] as i128, data[at + 22] as i128, data[at + 23] as i128);
    let delay = (first << 4) | (second >> 4);
    let padding = ((second & 0x0F) << 8) | third;
    delay + padding
}

const MP3_BITRATES_V1: [i128; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const MP3_BITRATES_V2: [i128; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

/// `read_mp3_info`: the first frame header, and the Xing tag when there is one.
pub fn read_mp3_info(data: &[u8]) -> Result<WavInfo> {
    let start = skip_id3(data);
    if len(data) < start + 4 {
        return Err(Error::Value("not an MP3 file".into()));
    }
    let header = u32be(data, start)?;
    if (header >> 21) & 0x7FF != 0x7FF {
        return Err(Error::Value("not an MP3 file (no frame sync)".into()));
    }
    let version = (header >> 19) & 0x3;
    let rate_index = ((header >> 10) & 0x3) as usize;
    let rates: [i128; 3] = match version {
        3 => [44100, 48000, 32000],
        2 => [22050, 24000, 16000],
        0 => [11025, 12000, 8000],
        _ => return Err(Error::Value("unsupported MPEG audio version or sample rate".into())),
    };
    if rate_index > 2 {
        return Err(Error::Value("unsupported MPEG audio version or sample rate".into()));
    }
    let rate = rates[rate_index];
    let mono = (header >> 6) & 0x3 == 3;
    let channels = if mono { 1 } else { 2 };
    let samples_per_frame: i128 = if version == 3 { 1152 } else { 576 };
    let side: Off = if version == 3 {
        if mono { 17 } else { 32 }
    } else if mono {
        9
    } else {
        17
    };
    let tag_at = start + 4 + side;
    let tag = sl(data, tag_at, tag_at + 4);
    if tag == b"Xing" || tag == b"Info" {
        let flags = u32be(data, tag_at + 4)?;
        if flags & 0x1 != 0 {
            let frames = int(u32be(data, tag_at + 8)?);
            let samples = frames * samples_per_frame - mp3_trim(data, tag_at, flags);
            return Ok(WavInfo::compressed(rate, samples.max(0), channels));
        }
    }
    let table = if version == 3 { &MP3_BITRATES_V1 } else { &MP3_BITRATES_V2 };
    let bitrate = table[((header >> 12) & 0xF) as usize] * 1000;
    if bitrate == 0 {
        return Err(Error::Value("MP3 frame declares a free or reserved bitrate".into()));
    }
    let padding = ((header >> 9) & 0x1) as i128;
    let frame_size = samples_per_frame / 8 * bitrate / rate + padding;
    let frames = int(len(data) - start) / frame_size;
    Ok(WavInfo::compressed(rate, frames * samples_per_frame, channels))
}

const ADTS_RATES: [i128; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
/// One AAC frame of decoder priming, which Premiere discards.
const AAC_PRIMING: i128 = 1024;

fn adts_length(data: &[u8], at: usize) -> u128 {
    ((data[at + 3] & 0x03) as u128) << 11 | (data[at + 4] as u128) << 3 | (data[at + 5] >> 5) as u128
}

/// `read_aac_info`: walk a raw ADTS stream, summing its frames' samples.
pub fn read_aac_info(data: &[u8]) -> Result<WavInfo> {
    let mut position = skip_id3(data);
    let is_header = |at: Off| -> bool {
        if at + 7 > len(data) {
            return false;
        }
        let a = at as usize;
        data[a] == 0xFF
            && data[a + 1] & 0xF0 == 0xF0
            && (data[a + 1] >> 1) & 0x3 == 0
            && (data[a + 2] >> 2) & 0xF < 13
            && adts_length(data, a) >= 7
    };
    while position + 7 <= len(data) && !is_header(position) {
        position += 1;
    }
    if position + 7 > len(data) {
        return Err(Error::Value("not an ADTS stream".into()));
    }
    let p = position as usize;
    let rate = ADTS_RATES[((data[p + 2] >> 2) & 0xF) as usize];
    let channels = ((data[p + 2] & 0x1) << 2 | (data[p + 3] >> 6) & 0x3) as i128;
    let mut samples: i128 = 0;
    while position + 7 <= len(data) && is_header(position) {
        let p = position as usize;
        samples += 1024 * ((data[p + 6] & 0x03) as i128 + 1);
        position += adts_length(data, p);
    }
    Ok(WavInfo::compressed(rate, (samples - AAC_PRIMING).max(0), channels.max(1)))
}

const ASF_HEADER_GUID: [u8; 16] = *b"\x30\x26\xB2\x75\x8E\x66\xCF\x11\xA6\xD9\x00\xAA\x00\x62\xCE\x6C";
const ASF_FILE_PROPS_GUID: [u8; 16] = *b"\xA1\xDC\xAB\x8C\x47\xA9\xCF\x11\x8E\xE4\x00\xC0\x0C\x20\x53\x65";
const ASF_STREAM_PROPS_GUID: [u8; 16] = *b"\x91\x07\xDC\xB7\xB7\xA9\xCF\x11\x8E\xE6\x00\xC0\x0C\x20\x53\x65";
const ASF_AUDIO_PREFIX: [u8; 4] = *b"\x40\x9E\x69\xF8";

/// Python `round(x)` of a finite float, as an int (ties to even).
fn py_round_float(x: f64) -> i128 {
    x.round_ties_even() as i128
}

/// `read_wma_info`: an ASF header's audio stream format and duration.
pub fn read_wma_info(data: &[u8]) -> Result<WavInfo> {
    if sl(data, 0, 16) != ASF_HEADER_GUID {
        return Err(Error::Value("not an ASF/WMA file".into()));
    }
    let header_size = u64le(data, 16)?;
    let body = sl(data, 30, header_size);
    let mut rate: u128 = 0;
    let mut channels: u128 = 0;
    let mut seconds: f64 = 0.0;
    let mut position: Off = 0;
    while position + 24 <= len(body) {
        let guid = sl(body, position, position + 16);
        let size = u64le(body, position + 16)?;
        if size < 24 || position + size > len(body) {
            break;
        }
        let payload = sl(body, position + 24, position + size);
        if guid == ASF_FILE_PROPS_GUID && payload.len() >= 64 {
            let send = u64le(payload, 48)?;
            if send != 0 {
                seconds = send as f64 / 1e7;
            } else {
                let play = u64le(payload, 40)?;
                let preroll = u64le(payload, 56)?;
                let value = play as f64 / 1e7 - preroll as f64 / 1e3;
                seconds = if value > 0.0 { value } else { 0.0 };
            }
        } else if guid == ASF_STREAM_PROPS_GUID && payload.len() >= 72 && payload[..4] == ASF_AUDIO_PREFIX {
            channels = u16le(payload, 56)?;
            rate = u32le(payload, 58)?;
        }
        position += size;
    }
    if rate == 0 {
        return Err(Error::Value("ASF file has no audio stream properties".into()));
    }
    Ok(WavInfo::compressed(int(rate), py_round_float(seconds * rate as f64), int(channels.max(1))))
}

/// `read_m4a_info`: the audio track of an MP4 container (its edit list when there is one).
pub fn read_m4a_info(data: &[u8]) -> Result<WavInfo> {
    let Some(moov) = find_box(data, 0, len(data), &[b"moov"])? else {
        return Err(Error::Value("not an MP4/M4A file".into()));
    };
    let mut movie_timescale = 0u128;
    if let Some(mvhd) = find_box(data, moov.0, moov.1, &[b"mvhd"])? {
        let version = byte(data, mvhd.0)?;
        movie_timescale = u32be(data, mvhd.0 + if version != 0 { 20 } else { 12 })?;
    }
    let mut boxes = Boxes::new(data, moov.0, moov.1);
    while let Some(b) = boxes.next_box()? {
        if b.kind != b"trak" {
            continue;
        }
        let track = read_track(data, b.body, b.end)?;
        if track.handler != b"soun" {
            continue;
        }
        let rate = if track.sample_rate != 0 { track.sample_rate } else { track.timescale };
        let edit = find_box(data, b.body, b.end, &[b"edts", b"elst"])?;
        if let (Some(edit), true) = (edit, movie_timescale != 0) {
            let span = u32be(data, edit.0 + 8)?;
            return Ok(WavInfo::compressed(int(rate), int(span * rate / movie_timescale), int(track.channels)));
        }
        return Ok(WavInfo::compressed(int(rate), int(track.samples * track.sample_delta), int(track.channels)));
    }
    Err(Error::Value("MP4 container has no audio track".into()))
}

/// `read_audio_info`: the reader for `suffix` (lower-case, with the dot).
pub fn read_audio_info(data: &[u8], suffix: &str) -> Result<WavInfo> {
    match suffix {
        ".aac" => read_aac_info(data),
        ".aiff" => read_aiff_info(data),
        ".m4a" => read_m4a_info(data),
        ".mp3" => read_mp3_info(data),
        ".wav" => read_wav_info(data),
        ".wma" => read_wma_info(data),
        _ => Err(Error::NotImplemented(format!("no audio reader for {suffix}"))),
    }
}

// ---- _probe_movie -------------------------------------------------------------------------------

/// `_MovieProbe`: what the movie readers yield, shared by import and proxy synthesis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieProbe {
    pub width: i128,
    pub height: i128,
    /// Ticks per frame.
    pub frame_rate: i128,
    /// Ticks.
    pub duration: i128,
    pub container: &'static str,
    pub codec: String,
    pub audio: Option<MovieAudioInfo>,
    pub field_order: i128,
    pub clip_id: String,
    pub start_ticks: i128,
    pub timecode_format: i64,
}

/// `TIMECODE_FORMATS.get((ticks_per_frame, drop_frame))`.
pub fn timecode_format(ticks_per_frame: i128, drop_frame: bool) -> Option<i64> {
    let ticks = i64::try_from(ticks_per_frame).ok()?;
    crate::build::media::timecode_format(ticks, drop_frame)
}

/// `_probe_movie` on a file's bytes; `suffix` is `path.suffix.lower()`.
pub fn probe_movie_data(data: &[u8], suffix: &str) -> Result<MovieProbe> {
    let Some(container) = crate::build::media::movie_container(suffix) else {
        return Err(Error::Key(crate::convert::model::py_str_repr(suffix)));
    };
    let mut audio = None;
    let mut start_ticks = 0;
    let mut drop_frame = false;
    let mut field_order = 0;
    let mut clip_id = String::new();
    let (width, height, frames, codec, frame_rate);
    if suffix == ".avi" {
        let info = read_avi_info(data)?;
        width = info.width;
        height = info.height;
        frames = info.frames;
        codec = info.codec;
        frame_rate = py_floordiv(TICKS_PER_SECOND, info.fps)?;
    } else {
        let movie = if suffix == ".mxf" { read_mxf_info(data)? } else { read_mp4_info(data)? };
        if py_mod(TICKS_PER_SECOND * movie.frame_duration, movie.timescale)? != 0 {
            return Err(Error::NotImplemented(format!(
                "frame rate {}/{} is not a whole number of ticks",
                movie.timescale, movie.frame_duration
            )));
        }
        frame_rate = py_floordiv(TICKS_PER_SECOND * movie.frame_duration, movie.timescale)?;
        width = movie.width;
        height = movie.height;
        frames = movie.frames;
        codec = movie.codec;
        audio = movie.audio;
        drop_frame = movie.drop_frame;
        field_order = movie.field_order;
        clip_id = movie.clip_id;
        start_ticks = movie.start_timecode * frame_rate;
    }
    let Some(timecode_format) = timecode_format(frame_rate, drop_frame) else {
        return Err(Error::NotImplemented(format!(
            "no verified timecode format for a {}-tick {}frame",
            frame_rate,
            if drop_frame { "drop-" } else { "" }
        )));
    };
    Ok(MovieProbe {
        width,
        height,
        frame_rate,
        duration: frames * frame_rate,
        container,
        codec,
        audio,
        field_order,
        clip_id,
        start_ticks,
        timecode_format,
    })
}

/// `_probe_movie(path)`: reads the file (the suffix is taken from the path as Python does).
pub fn probe_movie(path: &std::path::Path) -> Result<MovieProbe> {
    let data = std::fs::read(path)?;
    probe_movie_data(&data, &py_suffix_lower(path))
}

// ---- paths --------------------------------------------------------------------------------------

/// Python 3.14 `PurePath(path).suffix`: `os.path.splitext` of the final component (leading dots
/// of the name are not a suffix; a trailing dot is).
pub fn py_suffix(path: &std::path::Path) -> String {
    let normal = crate::model::project::py_path(&path.to_string_lossy());
    let text = normal.to_string_lossy();
    let name = match text.rfind('/') {
        Some(i) => &text[i + 1..],
        None => &text[..],
    };
    if name == "." || name == ".." {
        return String::new();
    }
    let Some(dot) = name.rfind('.') else { return String::new() };
    if name[..dot].bytes().all(|b| b == b'.') {
        return String::new();
    }
    name[dot..].to_string()
}

/// `path.suffix.lower()`.
pub fn py_suffix_lower(path: &std::path::Path) -> String {
    py_suffix(path).to_lowercase()
}
