//! Gzip framing for `.prproj` files (port of `py_premiere/xml/gzip_io.py`).
//!
//! Premiere writes a fixed 10-byte gzip header (flags 0, mtime 0, xfl 0; the
//! OS byte varies by writer) followed by a stock zlib level-6 raw deflate
//! stream and the CRC-32/size trailer. The header read from a file is kept
//! verbatim (including optional FEXTRA/FNAME/FCOMMENT/FHCRC fields) and
//! re-emitted on save. Compression goes through flate2's `zlib` backend
//! (system libz on macOS, the same library Python's `zlib` module uses), at
//! level 6, window bits -15, memLevel 8 - Python's
//! `zlib.compressobj(6, zlib.DEFLATED, -15)`.

use crate::error::{Error, Result};
use flate2::{Compress, Compression, Crc, Decompress, FlushCompress, FlushDecompress, Status};

/// The gzip magic bytes.
pub const GZIP_MAGIC: &[u8] = b"\x1f\x8b";
/// The OS byte current Premiere builds write (older ones wrote 19).
pub const DEFAULT_OS_BYTE: u8 = 10;

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;

/// Raw gzip header of a parsed file, re-emitted verbatim on save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GzipFraming {
    /// The header bytes exactly as read (or [`GzipFraming::default`]).
    pub header: Vec<u8>,
}

impl Default for GzipFraming {
    /// For files created from scratch: mtime 0 and the current Premiere OS byte.
    fn default() -> Self {
        GzipFraming { header: vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, DEFAULT_OS_BYTE] }
    }
}

fn truncated<T>() -> Result<T> {
    Err(Error::Gzip("truncated gzip header".into()))
}

fn header_length(data: &[u8]) -> Result<usize> {
    if data.len() < 10 {
        return truncated();
    }
    let flags = data[3];
    let mut pos = 10usize;
    if flags & FEXTRA != 0 {
        if pos + 2 > data.len() {
            return truncated();
        }
        let xlen = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            // Python's bytes.index from a position past the end also fails.
            let Some(rel) = data.get(pos..).and_then(|s| s.iter().position(|&b| b == 0)) else {
                return truncated();
            };
            pos += rel + 1;
        }
    }
    if flags & FHCRC != 0 {
        pos += 2;
    }
    if pos > data.len() {
        return truncated();
    }
    Ok(pos)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

/// Python `decompress_prproj`: `(xml payload, framing)`; framing is `None`
/// for an uncompressed file (returned as is).
pub fn decompress_prproj(data: &[u8]) -> Result<(Vec<u8>, Option<GzipFraming>)> {
    if !data.starts_with(GZIP_MAGIC) {
        return Ok((data.to_vec(), None));
    }
    let header_len = header_length(data)?;
    let body = &data[header_len..];
    let mut inflater = Decompress::new(false);
    let mut xml: Vec<u8> = Vec::with_capacity(body.len() * 8 + 1024);
    let mut ended = false;
    loop {
        if xml.len() == xml.capacity() {
            xml.reserve(xml.capacity().max(64 * 1024));
        }
        let consumed = inflater.total_in() as usize;
        let before_out = inflater.total_out();
        let status = inflater
            .decompress_vec(&body[consumed..], &mut xml, FlushDecompress::None)
            .map_err(|e| Error::Gzip(format!("corrupt deflate stream: {e}")))?;
        if status == Status::StreamEnd {
            ended = true;
            break;
        }
        let progressed = inflater.total_in() as usize != consumed || inflater.total_out() != before_out;
        if !progressed && xml.len() < xml.capacity() {
            break; // input exhausted mid-stream
        }
    }
    let trailer = if ended { &body[inflater.total_in() as usize..] } else { &[][..] };
    if trailer.len() < 8 {
        return Err(Error::Gzip("truncated gzip trailer".into()));
    }
    if trailer.len() > 8 {
        return Err(Error::Gzip("multi-member gzip files are not supported".into()));
    }
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    if crc != crc32(&xml) || isize != (xml.len() as u64 & 0xFFFF_FFFF) as u32 {
        return Err(Error::Gzip("gzip CRC/length mismatch".into()));
    }
    Ok((xml, Some(GzipFraming { header: data[..header_len].to_vec() })))
}

/// Raw deflate at zlib level 6 (Python `compressobj(6, DEFLATED, -15)`,
/// `compress(data) + flush()`).
pub fn deflate_raw(data: &[u8]) -> Vec<u8> {
    let mut deflater = Compress::new(Compression::new(6), false);
    let mut out: Vec<u8> = Vec::with_capacity(data.len() / 4 + 1024);
    // compress(xml): all input with Z_NO_FLUSH...
    while (deflater.total_in() as usize) < data.len() {
        if out.len() == out.capacity() {
            out.reserve(out.capacity().max(64 * 1024));
        }
        let consumed = deflater.total_in() as usize;
        deflater
            .compress_vec(&data[consumed..], &mut out, FlushCompress::None)
            .expect("deflate cannot fail on valid state");
    }
    // ...then flush(): Z_FINISH until the stream ends.
    loop {
        if out.len() == out.capacity() {
            out.reserve(out.capacity().max(64 * 1024));
        }
        let status = deflater
            .compress_vec(&[], &mut out, FlushCompress::Finish)
            .expect("deflate cannot fail on valid state");
        if status == Status::StreamEnd {
            break;
        }
    }
    out
}

/// Python `compress_prproj`: frame an XML payload the way Premiere would have
/// written it (no framing: the XML itself).
pub fn compress_prproj(xml: &[u8], framing: Option<&GzipFraming>) -> Vec<u8> {
    let Some(framing) = framing else {
        return xml.to_vec();
    };
    let deflated = deflate_raw(xml);
    let mut out = Vec::with_capacity(framing.header.len() + deflated.len() + 8);
    out.extend_from_slice(&framing.header);
    out.extend_from_slice(&deflated);
    out.extend_from_slice(&crc32(xml).to_le_bytes());
    out.extend_from_slice(&((xml.len() as u64 & 0xFFFF_FFFF) as u32).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_header_fields() {
        let xml = b"<a>hello hello hello</a>".repeat(100);
        let file = compress_prproj(&xml, Some(&GzipFraming::default()));
        let (back, framing) = decompress_prproj(&file).unwrap();
        assert_eq!(back, xml);
        assert_eq!(framing.unwrap(), GzipFraming::default());
        // FNAME header field preserved.
        let mut header = vec![0x1f, 0x8b, 8, FNAME, 0, 0, 0, 0, 0, 3];
        header.extend_from_slice(b"x.xml\0");
        let framing = GzipFraming { header };
        let file = compress_prproj(&xml, Some(&framing));
        assert_eq!(decompress_prproj(&file).unwrap().1.unwrap(), framing);
        // Errors.
        assert!(decompress_prproj(&file[..file.len() - 3]).is_err());
        let mut two = file.clone();
        two.extend_from_slice(&file);
        assert!(decompress_prproj(&two).is_err());
        assert!(decompress_prproj(b"\x1f\x8b\x08").is_err());
        assert_eq!(decompress_prproj(b"<a/>").unwrap(), (b"<a/>".to_vec(), None));
    }
}
