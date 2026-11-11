//! Read the saved Resolve encodings used by the source profile.
use crate::{binary::take_varint, Result};
use graph::Rational;
use std::collections::BTreeMap;

const MAX_BLOB: usize = 1 << 16;

#[derive(Default)]
pub(super) struct Blobs(usize);

impl Blobs {
    pub(super) fn charge(&mut self, bytes: usize) -> Result<()> {
        self.0 = self
            .0
            .checked_add(bytes)
            .filter(|n| *n <= 32 << 20)
            .ok_or("Resolve decoded blob budget exceeded")?;
        Ok(())
    }

    pub fn qt(&mut self, encoded: &str) -> Result<Map> {
        let data = hex_bytes(encoded)?;
        self.charge(data.len())?;
        qt(&data)
    }

    pub fn marker_data(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 9
            || data[..4] != 10001u32.to_be_bytes()
            || u32::from_be_bytes(data[4..8].try_into()?) as usize != data.len() - 8
        {
            return Err("malformed Resolve marker blob header".into());
        }
        let raw = match data[8] {
            0x80 => data[9..].to_vec(),
            0x81 => zstd::bulk::decompress(&data[9..], MAX_BLOB)?,
            _ => return Err("unsupported Resolve marker blob encoding".into()),
        };
        self.charge(raw.len())?;
        Ok(raw)
    }

    pub fn envelope(&mut self, encoded: &str) -> Result<Vec<u8>> {
        if encoded.len() > MAX_BLOB * 2 {
            return Err("Resolve encoded blob exceeds limit".into());
        }
        let data = crate::binary::unpack(encoded)?;
        self.charge(data.len())?;
        Ok(data)
    }
}

fn take<'a>(data: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    let (value, rest) = data.split_at_checked(len).ok_or("truncated Resolve blob")?;
    *data = rest;
    Ok(value)
}

fn number<const N: usize>(data: &mut &[u8]) -> Result<[u8; N]> {
    Ok(take(data, N)?.try_into()?)
}

fn sized<'a>(data: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = u32::from_be_bytes(number(data)?);
    if len == u32::MAX {
        return Ok(&[]);
    }
    take(data, len as usize)
}

fn utf16(bytes: &[u8]) -> Result<String> {
    if bytes.len() % 2 != 0 {
        return Err("invalid Resolve UTF-16 byte count".into());
    }
    Ok(char::decode_utf16(
        bytes
            .chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]])),
    )
    .collect::<std::result::Result<String, _>>()?)
}

#[derive(Debug)]
enum Qt {
    Int(i64),
    Real(f64),
    Text(String),
    Bytes(Vec<u8>),
    Unused,
}
pub(super) struct Map(BTreeMap<String, Qt>);

impl Map {
    pub fn int(&self, key: &str) -> Result<i64> {
        match self.0.get(key) {
            Some(Qt::Int(n)) => Ok(*n),
            _ => Err(format!("missing or invalid Resolve integer {key}").into()),
        }
    }

    pub fn real(&self, key: &str) -> Result<f64> {
        match self.0.get(key) {
            Some(Qt::Real(n)) if n.is_finite() => Ok(*n),
            _ => Err(format!("missing or invalid Resolve real {key}").into()),
        }
    }

    pub fn text(&self, key: &str) -> Result<Option<&str>> {
        match self.0.get(key) {
            None => Ok(None),
            Some(Qt::Text(value)) => Ok(Some(value)),
            _ => Err(format!("invalid Resolve text {key}").into()),
        }
    }

    pub fn bytes(&self, key: &str) -> Result<&[u8]> {
        match self.0.get(key) {
            Some(Qt::Bytes(value)) => Ok(value),
            _ => Err(format!("missing or invalid Resolve bytes {key}").into()),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

pub(super) fn qt(mut data: &[u8]) -> Result<Map> {
    if data.len() > MAX_BLOB || u32::from_be_bytes(number(&mut data)?) != 1 {
        return Err("unsupported Resolve Qt map".into());
    }
    let count = u32::from_be_bytes(number(&mut data)?);
    if count > 256 {
        return Err("Resolve Qt map exceeds entry limit".into());
    }
    let mut result = BTreeMap::new();
    for _ in 0..count {
        let key = utf16(sized(&mut data)?)?;
        let ty = u32::from_be_bytes(number(&mut data)?);
        let null = take(&mut data, 1)?[0];
        let value = match ty {
            1 => {
                take(&mut data, 1)?;
                Qt::Unused
            }
            2 => Qt::Int(i32::from_be_bytes(number(&mut data)?).into()),
            3 => Qt::Int(u32::from_be_bytes(number(&mut data)?).into()),
            4 => Qt::Int(i64::from_be_bytes(number(&mut data)?)),
            5 => {
                take(&mut data, 8)?;
                Qt::Unused
            }
            6 => Qt::Real(f64::from_be_bytes(number(&mut data)?)),
            10 => Qt::Text(utf16(sized(&mut data)?)?),
            12 => Qt::Bytes(sized(&mut data)?.to_vec()),
            38 => {
                take(&mut data, 4)?;
                Qt::Unused
            }
            _ => return Err("unsupported Resolve Qt field type".into()),
        };
        if null > 1
            || result
                .insert(key, if null == 0 { value } else { Qt::Unused })
                .is_some()
        {
            return Err("duplicate or malformed Resolve Qt field".into());
        }
    }
    if !data.is_empty() {
        return Err("unexpected trailing Resolve Qt bytes".into());
    }
    Ok(Map(result))
}

#[derive(Clone, Copy)]
pub(super) enum Wire<'a> {
    Int(u64),
    Bytes(&'a [u8]),
    Fixed(&'a [u8]),
}

pub(super) fn fields(data: &[u8]) -> Result<Vec<(u64, Wire<'_>)>> {
    let mut at = 0;
    let mut fields = Vec::new();
    while at < data.len() {
        let key = take_varint(data, &mut at)?;
        let field = key >> 3;
        if field == 0 || field > (1 << 29) - 1 || fields.len() == 4096 {
            return Err("invalid or over-budget Resolve protobuf field".into());
        }
        let wire = key & 7;
        let value = if wire == 0 {
            Wire::Int(take_varint(data, &mut at)?)
        } else {
            let size = match wire {
                1 => 8,
                2 => usize::try_from(take_varint(data, &mut at)?)?,
                5 => 4,
                _ => return Err("unsupported Resolve protobuf wire type".into()),
            };
            let end = at
                .checked_add(size)
                .filter(|end| *end <= data.len())
                .ok_or("truncated Resolve protobuf field")?;
            let bytes = &data[at..end];
            at = end;
            if wire == 2 {
                Wire::Bytes(bytes)
            } else {
                Wire::Fixed(bytes)
            }
        };
        fields.push((field, value));
    }
    Ok(fields)
}

pub(super) fn pb_bytes(data: &[u8], wanted: u64) -> Result<Option<&[u8]>> {
    let mut found = None;
    for (key, value) in fields(data)? {
        if key == wanted {
            match value {
                Wire::Bytes(bytes) if found.is_none() => found = Some(bytes),
                _ => return Err("ambiguous Resolve protobuf field".into()),
            }
        }
    }
    Ok(found)
}

pub(super) fn hex_bytes(encoded: &str) -> Result<Vec<u8>> {
    if encoded.len() > MAX_BLOB * 2 {
        return Err("Resolve hex field exceeds limit".into());
    }
    Ok(hex::decode(encoded)?)
}

pub(super) fn rate(data: &[u8]) -> Result<Rational> {
    // The known non-drop encoding stores the exact binary64 rate and zero flags.
    let [bytes @ .., 0, 0, 0, 0, 0, 0, 0, 0] = data else {
        return Err("unsupported Resolve frame-rate flags".into());
    };
    let fps = f64::from_le_bytes(bytes.try_into()?);
    let (n, d) = match fps {
        x if x.to_bits() == crate::RATE.to_bits() => (24000, 1001),
        24.0 => (24, 1),
        _ => return Err("unsupported Resolve source frame rate".into()),
    };
    Ok(Rational::new(n, d).expect("known rate"))
}

pub(super) fn frames(seconds: f64, rate: Rational) -> Result<i64> {
    let count = seconds * rate.num() as f64 / rate.den() as f64;
    if !count.is_finite()
        || count < 0.0
        || count > 10_000_000.0
        || (count - count.round()).abs() > 1e-7
    {
        return Err("Resolve extent is outside the supported frame grid".into());
    }
    Ok(count.round() as i64)
}

pub(super) fn timecode(text: &str, base: i64) -> Result<i64> {
    let parts = text.as_bytes();
    if parts.len() != 11 || [2, 5, 8].into_iter().any(|i| parts[i] != b':') {
        return Err("expected Resolve non-drop timecode".into());
    }
    let mut values = [0; 4];
    for (slot, pair) in values.iter_mut().zip(parts.split(|&c| c == b':')) {
        if pair.len() != 2 || !pair.iter().all(u8::is_ascii_digit) {
            return Err("malformed Resolve timecode".into());
        }
        *slot = i64::from((pair[0] - b'0') * 10 + pair[1] - b'0');
    }
    let [h, m, s, f] = values;
    if h >= 24 || m >= 60 || s >= 60 || f >= base {
        return Err("Resolve timecode component outside range".into());
    }
    Ok((h * 3600 + m * 60 + s) * base + f)
}

pub(super) fn extent(data: &[u8], rate: Rational) -> Result<(i64, i64)> {
    let (start, length) = data.split_at_checked(8).ok_or("truncated Resolve extent")?;
    Ok((
        frames(f64::from_le_bytes(start.try_into()?), rate)?,
        frames(f64::from_le_bytes(length.try_into()?), rate)?,
    ))
}

pub(super) fn raster(data: &[u8]) -> Result<(i64, i64)> {
    let (width, height) = data.split_at_checked(8).ok_or("truncated Resolve raster")?;
    let size = (
        i64::from_be_bytes(width.try_into()?),
        i64::from_be_bytes(height.try_into()?),
    );
    if size.0 <= 0 || size.1 <= 0 {
        return Err("nonpositive Resolve raster".into());
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_encodings_reject_ambiguous_fields_and_off_grid_clocks() {
        assert!(pb_bytes(&[10, 0, 10, 0], 1).is_err());
        assert!(pb_bytes(&[10, 2, 1], 1).is_err());
        assert!(pb_bytes(&[0], 1).is_err());
        assert_eq!(pb_bytes(&[10, 1, 42, 16, 9], 1).unwrap(), Some(&[42][..]));
        assert!(qt(&[0, 0, 0, 1, 0, 0, 1, 1]).is_err());
        let r = Rational::new(24000, 1001).unwrap();
        assert_eq!(frames(3603.6, r).unwrap(), 86400);
        assert!(frames(3603.61, r).is_err());
        assert!(timecode("01:00:00;00", 24).is_err());
    }
}
