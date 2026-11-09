//! Bounded AVB primitive writer, ported from pyavb `ioctx.py` at
//! ff64e06b60077ecabe5db3fef239898b08363898 (MIT; see NOTICE).
//! This does not allocate object IDs, serialize classes, or establish Avid compatibility.

use crate::io::{Error, MobId, Result, Uuid};

/// A bounded byte accumulator. Methods check the complete append before mutation.
pub struct Writer {
    bytes: Vec<u8>,
    pub big_endian: bool,
    limit: usize,
}

impl Writer {
    pub fn new(big_endian: bool) -> Self {
        Self::with_limit(big_endian, usize::MAX)
    }

    pub fn with_limit(big_endian: bool, limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            big_endian,
            limit,
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn bytes(&mut self, value: &[u8]) -> Result<()> {
        if value.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(Error::new("writer byte limit exceeded", self.bytes.len()));
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    pub fn u8(&mut self, value: u8) -> Result<()> {
        self.bytes(&[value])
    }
    pub fn s8(&mut self, value: i8) -> Result<()> {
        self.bytes(&[value as u8])
    }
    pub fn bool(&mut self, value: bool) -> Result<()> {
        self.u8(u8::from(value))
    }
    pub fn u16(&mut self, value: u16) -> Result<()> {
        self.bytes(&if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        })
    }
    pub fn s16(&mut self, value: i16) -> Result<()> {
        self.u16(value as u16)
    }
    pub fn u32(&mut self, value: u32) -> Result<()> {
        self.bytes(&if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        })
    }
    pub fn s32(&mut self, value: i32) -> Result<()> {
        self.u32(value as u32)
    }
    pub fn u64(&mut self, value: u64) -> Result<()> {
        self.bytes(&if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        })
    }
    pub fn s64(&mut self, value: i64) -> Result<()> {
        self.u64(value as u64)
    }

    /// Standard file-order f64, symmetric with this crate's `Reader::f64`.
    pub fn f64(&mut self, value: f64) -> Result<()> {
        self.bytes(&if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        })
    }

    /// Match pyavb `write_double_{le,be}`. Both emit little-endian bytes in the
    /// pinned revision. This conflicts with `Reader::f64` on big-endian input.
    pub fn f64_pyavb(&mut self, value: f64) -> Result<()> {
        self.bytes(&value.to_le_bytes())
    }

    pub fn fourcc(&mut self, value: [u8; 4]) -> Result<()> {
        let mut value = value;
        if !self.big_endian {
            value.reverse();
        }
        self.bytes(&value)
    }

    /// Reproduce pyavb's decimal mantissa/exponent choice for finite values.
    /// Small fractions may be truncated at exponent -6; no precision guarantee.
    pub fn exp10_float(&mut self, mut value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::new("non-finite exp10 value", self.len()));
        }
        let mut exponent: i16 = 0;
        while value.trunc() != value {
            if (value * 10.0).abs() >= i32::MAX as f64 || exponent <= -6 {
                break;
            }
            value *= 10.0;
            exponent -= 1;
        }
        let (mantissa, exponent) = if value == 5994.0 {
            (59940, -3)
        } else {
            if value < i32::MIN as f64 || value > i32::MAX as f64 {
                return Err(Error::new("exp10 mantissa outside i32", self.len()));
            }
            (value.trunc() as i32, exponent)
        };
        let mut pending = Writer::with_limit(self.big_endian, 6);
        pending.s32(mantissa)?;
        pending.s16(exponent)?;
        self.bytes(pending.as_bytes())
    }

    fn length_prefixed(&mut self, value: &[u8]) -> Result<()> {
        // 0xFFFF is an empty sentinel in the reader, not a usable length.
        if value.len() >= u16::MAX as usize {
            return Err(Error::new(
                "string length reaches reserved 0xFFFF",
                self.len(),
            ));
        }
        let mut pending = Writer::with_limit(self.big_endian, value.len() + 2);
        pending.u16(value.len() as u16)?;
        pending.bytes(value)?;
        self.bytes(pending.as_bytes())
    }

    pub fn string_macroman(&mut self, value: &str) -> Result<()> {
        let mut encoded = Vec::with_capacity(value.len());
        for ch in value.chars() {
            if (ch as u32) < 0x80 {
                encoded.push(ch as u8);
            } else if let Some(index) = MACROMAN_HIGH.iter().position(|c| *c == ch) {
                encoded.push(0x80 + index as u8);
            } else {
                return Err(Error::new(
                    format!("unrepresentable MacRoman character U+{:04X}", ch as u32),
                    self.len(),
                ));
            }
        }
        self.length_prefixed(&encoded)
    }

    /// pyavb prepends two NUL bytes to nonempty UTF-8 strings.
    pub fn string_utf8(&mut self, value: &str) -> Result<()> {
        if value.is_empty() {
            return self.length_prefixed(&[]);
        }
        if value.len() > (u16::MAX as usize - 3) {
            return Err(Error::new(
                "UTF-8 string length reaches reserved 0xFFFF",
                self.len(),
            ));
        }
        let mut encoded = Vec::with_capacity(value.len() + 2);
        encoded.extend_from_slice(&[0, 0]);
        encoded.extend_from_slice(value.as_bytes());
        self.length_prefixed(&encoded)
    }

    /// Zero is pyavb's null reference. Mapping live objects to IDs is a caller concern.
    pub fn object_ref_index(&mut self, index: Option<u32>) -> Result<()> {
        self.u32(index.unwrap_or(0))
    }

    pub fn raw_uuid(&mut self, value: &Uuid) -> Result<()> {
        let mut pending = Writer::with_limit(self.big_endian, 16);
        pending.u32(value.data1)?;
        pending.u16(value.data2)?;
        pending.u16(value.data3)?;
        pending.bytes(&value.data4)?;
        self.bytes(pending.as_bytes())
    }

    pub fn tagged_uuid(&mut self, value: &Uuid) -> Result<()> {
        let mut pending = Writer::with_limit(self.big_endian, 30);
        pending.u8(72)?;
        pending.u32(value.data1)?;
        pending.u8(70)?;
        pending.u16(value.data2)?;
        pending.u8(70)?;
        pending.u16(value.data3)?;
        pending.u8(65)?;
        pending.s32(8)?;
        pending.bytes(&value.data4)?;
        self.bytes(pending.as_bytes())
    }

    pub fn mob_id(&mut self, value: &MobId) -> Result<()> {
        let mut pending = Writer::with_limit(self.big_endian, 51);
        pending.u8(65)?;
        pending.s32(12)?;
        pending.bytes(&value.label)?;
        for v in [
            value.length,
            value.instance[0],
            value.instance[1],
            value.instance[2],
        ] {
            pending.u8(68)?;
            pending.u8(v)?;
        }
        pending.tagged_uuid(&value.material)?;
        self.bytes(pending.as_bytes())
    }

    pub fn rect(&mut self, value: [i16; 4]) -> Result<()> {
        let mut pending = Writer::with_limit(self.big_endian, 10);
        pending.s16(1)?;
        for v in value {
            pending.s16(v)?;
        }
        self.bytes(pending.as_bytes())
    }

    pub fn rgb_color(&mut self, value: [u16; 3]) -> Result<()> {
        let mut pending = Writer::with_limit(self.big_endian, 8);
        pending.s16(1)?;
        for v in value {
            pending.u16(v)?;
        }
        self.bytes(pending.as_bytes())
    }
}

// Same exact table as `Reader` (Mac OS Roman mapping). Kept local to avoid changing
// the established read API while adding only the writer slice.
const MACROMAN_HIGH: [char; 128] = [
    'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', 'ê', 'ë', 'í',
    'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü', '†', '°', '¢', '£', '§', '•',
    '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø', '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏',
    'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø', '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{a0}',
    'À', 'Ã', 'Õ', 'Œ', 'œ', '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›',
    'ﬁ', 'ﬂ', '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô',
    '\u{f8ff}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ',
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Reader;

    // Expected bytes generated with pinned pyavb AVBIOContext using BytesIO.
    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn sample_uuid() -> Uuid {
        Uuid {
            data1: 0x12345678,
            data2: 0x9abc,
            data3: 0xdef0,
            data4: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
        }
    }

    #[test]
    fn pyavb_scalar_vectors_both_orders() {
        for (big, expected) in [
            (false, "fffe0134122efb78563412b29e43ffefcdab8967452301eb7e16820befddee000000000000f83f44434241"),
            (true, "fffe011234fb2e12345678ff439eb20123456789abcdefeeddef0b82167eeb000000000000f83f41424344"),
        ] {
            let mut w = Writer::new(big);
            w.u8(255).unwrap(); w.s8(-2).unwrap(); w.bool(true).unwrap();
            w.u16(0x1234).unwrap(); w.s16(-1234).unwrap();
            w.u32(0x12345678).unwrap(); w.s32(-12345678).unwrap();
            w.u64(0x0123456789abcdef).unwrap(); w.s64(-1234567890123456789).unwrap();
            w.f64_pyavb(1.5).unwrap(); w.fourcc(*b"ABCD").unwrap();
            assert_eq!(w.as_bytes(), hex(expected));
        }
    }

    #[test]
    fn standard_big_endian_f64_roundtrips_reader_but_pyavb_vector_differs() {
        let mut standard = Writer::new(true);
        standard.f64(1.5).unwrap();
        assert_eq!(standard.as_bytes(), hex("3ff8000000000000"));
        assert_eq!(
            Reader::new(standard.as_bytes(), 0, true).f64().unwrap(),
            1.5
        );
        let mut source_compatible = Writer::new(true);
        source_compatible.f64_pyavb(1.5).unwrap();
        assert_eq!(source_compatible.as_bytes(), hex("000000000000f83f"));
        assert_ne!(standard.as_bytes(), source_compatible.as_bytes());
    }

    #[test]
    fn pyavb_uuid_and_mob_vectors_both_orders() {
        let uuid = sample_uuid();
        for (big, tagged, raw, mob) in [
            (false, "487856341246bc9a46f0de41080000001122334455667788",
             "78563412bc9af0de1122334455667788",
             "410c000000000102030405060708090a0b4413440144024403487856341246bc9a46f0de41080000001122334455667788"),
            (true, "4812345678469abc46def041000000081122334455667788",
             "123456789abcdef01122334455667788",
             "410000000c000102030405060708090a0b44134401440244034812345678469abc46def041000000081122334455667788"),
        ] {
            let mut w = Writer::new(big); w.tagged_uuid(&uuid).unwrap();
            assert_eq!(w.as_bytes(), hex(tagged));
            assert_eq!(Reader::new(w.as_bytes(), 0, big).tagged_uuid().unwrap(), uuid);
            let mut w = Writer::new(big); w.raw_uuid(&uuid).unwrap();
            assert_eq!(w.as_bytes(), hex(raw));
            assert_eq!(Reader::new(w.as_bytes(), 0, big).raw_uuid().unwrap(), uuid);
            let id = MobId { label: [0,1,2,3,4,5,6,7,8,9,10,11], length: 19,
                instance: [1,2,3], material: uuid };
            let mut w = Writer::new(big); w.mob_id(&id).unwrap();
            assert_eq!(w.as_bytes(), hex(mob));
            assert_eq!(Reader::new(w.as_bytes(), 0, big).mob_id().unwrap(), id);
        }
    }

    #[test]
    fn pyavb_rect_rgb_strings_exp10_and_refs() {
        for (big, rect_rgb, strings, exps, reference) in [
            (
                false,
                "0100feff0300fcff0500010000000080ffff",
                "0300419aa50800000041c3b6e280a20000",
                "a85d0000fdff24ea0000fdff0f000000ffff640000000000",
                "0000000078563412",
            ),
            (
                true,
                "0001fffe0003fffc0005000100008000ffff",
                "0003419aa50008000041c3b6e280a20000",
                "00005da8fffd0000ea24fffd0000000fffff000000640000",
                "0000000012345678",
            ),
        ] {
            let mut w = Writer::new(big);
            w.rect([-2, 3, -4, 5]).unwrap();
            w.rgb_color([0, 32768, 65535]).unwrap();
            assert_eq!(w.as_bytes(), hex(rect_rgb));
            let mut r = Reader::new(w.as_bytes(), 0, big);
            assert_eq!(r.rect().unwrap(), [-2, 3, -4, 5]);
            assert_eq!(r.rgb_color().unwrap(), [0, 32768, 65535]);

            let mut w = Writer::new(big);
            w.string_macroman("Aö•").unwrap();
            w.string_utf8("Aö•").unwrap();
            w.string_macroman("").unwrap();
            assert_eq!(w.as_bytes(), hex(strings));
            let mut r = Reader::new(w.as_bytes(), 0, big);
            assert_eq!(r.string().unwrap(), "Aö•");
            assert_eq!(r.string_utf8().unwrap(), "Aö•");
            assert_eq!(r.string().unwrap(), "");

            let mut w = Writer::new(big);
            for v in [23.976, 5.994, 1.5, 100.0] {
                w.exp10_float(v).unwrap();
            }
            assert_eq!(w.as_bytes(), hex(exps));

            let mut w = Writer::new(big);
            w.object_ref_index(None).unwrap();
            w.object_ref_index(Some(0x12345678)).unwrap();
            assert_eq!(w.as_bytes(), hex(reference));
        }
    }

    #[test]
    fn malformed_inputs_are_rejected_without_partial_append() {
        let mut w = Writer::with_limit(false, 5);
        w.u32(7).unwrap();
        assert!(w.u16(9).is_err());
        assert_eq!(w.as_bytes(), &[7, 0, 0, 0]);
        assert!(w.string_utf8("long").is_err());
        assert_eq!(w.len(), 4);
        assert!(w.string_macroman("🙂").is_err());
        assert_eq!(w.len(), 4);
        assert!(w.exp10_float(f64::NAN).is_err());
        assert_eq!(w.len(), 4);
        assert!(w.exp10_float(f64::INFINITY).is_err());
        assert_eq!(w.len(), 4);
        let mut other = Writer::new(false);
        assert!(other.string_utf8(&"x".repeat(65533)).is_err());
        assert!(other.exp10_float(2147483648.0).is_err());
    }
}
