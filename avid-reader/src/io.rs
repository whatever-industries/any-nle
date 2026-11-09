//! Bounded, byte-order-aware primitives for AVB payloads.
//! Ported from pyavb `ioctx.py` (MIT, Mark Reid; see NOTICE).

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub message: String,
    pub offset: usize,
}

impl Error {
    pub fn new(message: impl Into<String>, offset: usize) -> Error {
        Error { message: message.into(), offset }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A cursor over one payload. Every read is bounds-checked; nothing allocates more than
/// the payload already holds.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Absolute file offset of `data[0]`, for error messages.
    base: usize,
    pub big_endian: bool,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], base: usize, big_endian: bool) -> Reader<'a> {
        Reader { data, pos: 0, base, big_endian }
    }

    pub fn offset(&self) -> usize {
        self.base + self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn err<T>(&self, message: impl Into<String>) -> Result<T> {
        Err(Error::new(message, self.offset()))
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return self.err(format!("truncated: need {} bytes, {} left", n, self.remaining()));
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn peek_u8(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? == 0x01)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.array::<2>()?;
        Ok(if self.big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
    }

    pub fn s16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.array::<4>()?;
        Ok(if self.big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }

    pub fn s32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.array::<8>()?;
        Ok(if self.big_endian { u64::from_be_bytes(b) } else { u64::from_le_bytes(b) })
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub fn s64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }

    /// Little-endian files store four-character codes reversed.
    pub fn fourcc(&mut self) -> Result<[u8; 4]> {
        let mut b = self.array::<4>()?;
        if !self.big_endian {
            b.reverse();
        }
        Ok(b)
    }

    pub fn assert_tag(&mut self, expected: u8) -> Result<()> {
        let at = self.offset();
        let tag = self.u8()?;
        if tag != expected {
            return Err(Error::new(format!("expected tag 0x{:02X}, found 0x{:02X}", expected, tag), at));
        }
        Ok(())
    }

    /// pyavb `iter_ext`: while the next byte is 0x01, consume it and yield the following tag.
    pub fn next_ext(&mut self) -> Result<Option<u8>> {
        if self.peek_u8() != Some(0x01) {
            return Ok(None);
        }
        self.pos += 1;
        Ok(Some(self.u8()?))
    }

    /// s32 mantissa * 10^s16 exponent.
    pub fn exp10_float(&mut self) -> Result<f64> {
        let mantissa = self.s32()? as f64;
        let exponent = self.s16()? as i32;
        // Use a negative power directly. Dividing by 10^(-exponent) loses
        // subnormal results because the denominator itself overflows first.
        let result = mantissa * 10f64.powf(f64::from(exponent));
        if !result.is_finite() {
            return self.err("exp10 value exceeds finite f64 range");
        }
        Ok(result)
    }

    /// u16 length + bytes, NULs stripped from both ends; 0xFFFF means empty and reads nothing.
    pub fn string(&mut self) -> Result<String> {
        Ok(decode_macroman(self.raw_string()?))
    }

    pub fn string_utf8(&mut self) -> Result<String> {
        let at = self.offset();
        let raw = self.raw_string()?;
        String::from_utf8(raw.to_vec()).map_err(|_| Error::new("invalid UTF-8 string", at))
    }

    fn raw_string(&mut self) -> Result<&'a [u8]> {
        let size = self.u16()?;
        if size == 0xFFFF {
            return Ok(&[]);
        }
        let raw = self.bytes(size as usize)?;
        let start = raw.iter().position(|&b| b != 0).unwrap_or(raw.len());
        let end = raw.iter().rposition(|&b| b != 0).map_or(start, |i| i + 1);
        Ok(&raw[start..end])
    }

    pub fn object_ref(&mut self) -> Result<u32> {
        self.u32()
    }

    /// 16 bytes as (u32, u16, u16, [u8; 8]) in file byte order, like `UUID(bytes_le=...)`.
    pub fn raw_uuid(&mut self) -> Result<Uuid> {
        let data1 = self.u32()?;
        let data2 = self.u16()?;
        let data3 = self.u16()?;
        let data4 = self.array::<8>()?;
        Ok(Uuid { data1, data2, data3, data4 })
    }

    pub fn tagged_uuid(&mut self) -> Result<Uuid> {
        self.assert_tag(72)?;
        let data1 = self.u32()?;
        self.assert_tag(70)?;
        let data2 = self.u16()?;
        self.assert_tag(70)?;
        let data3 = self.u16()?;
        self.assert_tag(65)?;
        if self.s32()? != 8 {
            return self.err("UUID data4 length is not 8");
        }
        let data4 = self.array::<8>()?;
        Ok(Uuid { data1, data2, data3, data4 })
    }

    pub fn mob_id(&mut self) -> Result<MobId> {
        self.assert_tag(65)?;
        if self.s32()? != 12 {
            return self.err("MobID label length is not 12");
        }
        let label = self.array::<12>()?;
        let mut tail = [0u8; 4];
        for b in tail.iter_mut() {
            self.assert_tag(68)?;
            *b = self.u8()?;
        }
        let material = self.tagged_uuid()?;
        Ok(MobId { label, length: tail[0], instance: [tail[1], tail[2], tail[3]], material })
    }

    pub fn rect(&mut self) -> Result<[i16; 4]> {
        if self.s16()? != 1 {
            return self.err("rect version is not 1");
        }
        Ok([self.s16()?, self.s16()?, self.s16()?, self.s16()?])
    }

    pub fn rgb_color(&mut self) -> Result<[u16; 3]> {
        if self.s16()? != 1 {
            return self.err("RGB color version is not 1");
        }
        Ok([self.u16()?, self.u16()?, self.u16()?])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = &self.data4;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MobId {
    pub label: [u8; 12],
    pub length: u8,
    pub instance: [u8; 3],
    pub material: Uuid,
}

impl MobId {
    /// Material written UUID-first unless it holds a half-swapped SMPTE label (pyavb `MobID.urn`).
    fn swapped(&self) -> bool {
        let d = &self.material.data4;
        self.label[11] == 0 && d[..6] == [0x06, 0x0E, 0x2B, 0x34, 0x7F, 0x7F]
    }

    pub fn urn(&self) -> String {
        let l = &self.label;
        let mut s = format!(
            "urn:smpte:umid:{:02x}{:02x}{:02x}{:02x}.{:02x}{:02x}{:02x}{:02x}.{:02x}{:02x}{:02x}{:02x}.{:02x}{:02x}{:02x}{:02x}.",
            l[0], l[1], l[2], l[3], l[4], l[5], l[6], l[7], l[8], l[9], l[10], l[11],
            self.length, self.instance[0], self.instance[1], self.instance[2]
        );
        let m = &self.material;
        let d = &m.data4;
        if self.swapped() {
            s += &format!(
                "{:02x}{:02x}{:02x}{:02x}.{:02x}{:02x}{:02x}{:02x}.{:08x}.{:04x}{:04x}",
                d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], m.data1, m.data2, m.data3
            );
        } else {
            s += &format!(
                "{:08x}.{:04x}{:04x}.{:02x}{:02x}{:02x}{:02x}.{:02x}{:02x}{:02x}{:02x}",
                m.data1, m.data2, m.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
            );
        }
        s
    }

    /// The 32-byte UMID as 64 uppercase hex digits, in URN order (what MXF stores as the
    /// material package UID, and what the converter model calls `material_umid`).
    pub fn umid_hex(&self) -> String {
        self.urn()["urn:smpte:umid:".len()..].replace('.', "").to_uppercase()
    }
}

/// Mac OS Roman, 0x80..=0xFF.
const MACROMAN_HIGH: [char; 128] = [
    'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è',
    'ê', 'ë', 'í', 'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü',
    '†', '°', '¢', '£', '§', '•', '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø',
    '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏', 'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø',
    '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{a0}', 'À', 'Ã', 'Õ', 'Œ', 'œ',
    '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›', 'ﬁ', 'ﬂ',
    '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô',
    '\u{f8ff}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ',
];

pub fn decode_macroman(raw: &[u8]) -> String {
    raw.iter().map(|&b| if b < 0x80 { b as char } else { MACROMAN_HIGH[(b - 0x80) as usize] }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macroman_matches_python_codec_samples() {
        assert_eq!(decode_macroman(&[0x41, 0x8E, 0xA5, 0xD0, 0xF0]), "Aé•–\u{f8ff}");
    }

    #[test]
    fn strings_strip_nuls_and_ffff_is_empty() {
        let data = [3, 0, 0, b'a', 0, 0xFF, 0xFF, 9];
        let mut r = Reader::new(&data, 0, false);
        assert_eq!(r.string().unwrap(), "a");
        assert_eq!(r.string().unwrap(), "");
        assert_eq!(r.remaining(), 1);
    }

    #[test]
    fn truncated_reads_fail_without_panicking() {
        let mut r = Reader::new(&[1, 2, 3], 10, false);
        let e = r.u32().unwrap_err();
        assert_eq!(e.offset, 10);
        let mut r = Reader::new(&[200, 0], 0, false);
        assert!(r.string().is_err());
    }

    #[test]
    fn exp10_float() {
        let data = [0x18, 0, 0, 0, 0, 0, 0xA8, 0x5D, 0, 0, 0xFD, 0xFF];
        let mut r = Reader::new(&data, 0, false);
        assert_eq!(r.exp10_float().unwrap(), 24.0);
        assert_eq!(r.exp10_float().unwrap(), 23.976);
    }
}
