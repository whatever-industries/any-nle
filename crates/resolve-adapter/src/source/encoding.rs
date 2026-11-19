//! Read the saved Resolve encodings used by the source profile.
use crate::{binary::take_varint, Result};
use graph::Rational;
use std::collections::BTreeMap;

pub(super) const MAX_BLOB: usize = 1 << 16;

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
    ByteList(Vec<Vec<u8>>),
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

    pub fn byte_list(&self, key: &str) -> Result<Option<&[Vec<u8>]>> {
        match self.0.get(key) {
            None => Ok(None),
            Some(Qt::ByteList(rows)) => Ok(Some(rows)),
            _ => Err(format!("invalid Resolve byte list {key}").into()),
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
            9 => {
                let count = u32::from_be_bytes(number(&mut data)?);
                if count > 256 { return Err("Resolve list exceeds entry limit".into()); }
                let mut rows = Vec::new();
                for _ in 0..count {
                    if u32::from_be_bytes(number(&mut data)?) != 12 || take(&mut data,1)?[0] != 0 {
                        return Err("unsupported Resolve list entry".into());
                    }
                    rows.push(sized(&mut data)?.to_vec());
                }
                Qt::ByteList(rows)
            }
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

pub(super) fn default_picture_settings(
    project: &super::Node,
    blobs: &mut Blobs,
) -> Result<(bool, Option<&'static str>)> {
    let Some(config) = project.optional("CommonConfig")? else { return Ok((false, None)); };
    let encoded = blobs.qt(config.entry()?.get("FieldsBlob")?)?;
    let setup = encoded.bytes("SetupBA")?;
    if setup.len() < 9 || setup[..4] != 1u32.to_be_bytes()
        || u32::from_be_bytes(setup[4..8].try_into()?) as usize != setup.len() - 8
        || setup[8] != 0x81
    {
        return Ok((false, None));
    }
    let decoded = zstd::bulk::decompress(&setup[9..], MAX_BLOB)?;
    blobs.charge(decoded.len())?;
    // Resolve Studio 21.2 omits field 180 for the measured Nearest
    // default. The same setting changed to Frame Blend stores value 2.
    let config = fields(&decoded)?;
    let nearest = !config.iter().any(|(field, _)| *field == 180);
    // The default Scale entire image to fit omits field 98. Choosing
    // Center crop with no resizing in the same project saved zero there.
    let mut sizing_fields = config.iter().filter(|(field, _)| *field == 98);
    let sizing = match (sizing_fields.next(), sizing_fields.next()) {
        (None, None) => Some(crate::input_scaling::FIT),
        (Some((_, Wire::Int(0))), None) => Some(crate::input_scaling::CENTER_CROP),
        _ => None,
    };
    Ok((nearest, sizing))
}

pub(super) fn valid_clip_sizing_state(property: &[u8]) -> Result<bool> {
    // Reapplying the control changes the second integer. The actual
    // choice is identified separately by EffectFiltersBA.
    let properties = fields(property)?;
    let [(3, Wire::Bytes(state))] = properties.as_slice() else { return Ok(false) };
    Ok(matches!(fields(state)?.as_slice(),
        [(1, Wire::Int(1)), (2, Wire::Int(saved))] if *saved > 0))
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
    Ok(frame_rate(data)?.0)
}

pub(super) fn frame_rate(data: &[u8]) -> Result<(Rational, bool)> {
    // Media uses the non-drop representation. Native 29.97 DF timelines
    // store a one in the final byte of the otherwise identical rate field.
    let [bytes @ .., 0, 0, 0, 0, 0, 0, 0, drop @ (0 | 1)] = data else {
        return Err("unsupported Resolve frame-rate flags".into());
    };
    let fps = f64::from_le_bytes(bytes.try_into()?);
    let (n, d) = match fps {
        x if x.to_bits() == crate::RATE.to_bits() => (24000, 1001),
        24.0 => (24, 1),
        25.0 => (25, 1),
        x if x == 30000.0 / 1001.0 => (30000, 1001),
        _ => return Err("unsupported Resolve source frame rate".into()),
    };
    if *drop == 1 && n != 30000 {
        return Err("unqualified drop-frame flag for Resolve frame rate".into());
    }
    Ok((Rational::new(n, d).expect("known rate"), *drop == 1))
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
    fn project_sizing_does_not_inherit_an_unknown_or_duplicate_value() {
        use crate::input_scaling::{CENTER_CROP, FIT};

        let settings = |body: &[u8]| {
            let mut setup = hex_bytes(&crate::binary::pack(body).unwrap()).unwrap();
            setup[3] = 1; // Project SetupBA uses the version-one envelope.
            let qt = crate::binary::qt(&[("SetupBA", crate::binary::Qt::Bytes(&setup))]);
            let project = super::super::xml::parse(&format!(
                "<SM_Project><CommonConfig><SM_Config><FieldsBlob>{}</FieldsBlob></SM_Config></CommonConfig></SM_Project>",
                hex::encode(qt),
            ), &mut 0).unwrap();
            default_picture_settings(&project, &mut Blobs::default()).unwrap()
        };
        assert_eq!(settings(&[]), (true, Some(FIT)));
        assert_eq!(settings(&[0x90, 0x06, 0]), (true, Some(CENTER_CROP)));
        assert_eq!(settings(&[0x90, 0x06, 1]), (true, None));
        assert_eq!(settings(&[0x90, 0x06, 0, 0x90, 0x06, 0]), (true, None));
        assert_eq!(settings(&[0xa0, 0x0b, 2]), (false, Some(FIT)));
    }

    #[test]
    fn clip_sizing_state_accepts_native_variation_in_the_opaque_value() {
        for saved in ["1a07080110c4839806", "1a080801108fd7f19601"] {
            assert!(valid_clip_sizing_state(&hex_bytes(saved).unwrap()).unwrap());
        }
        assert!(!valid_clip_sizing_state(&[0x1a, 4, 8, 1, 16, 0]).unwrap());
        assert!(!valid_clip_sizing_state(&[0x1a, 4, 8, 2, 16, 1]).unwrap());
    }

    #[test]
    fn rate_fields_distinguish_the_2997_drop_frame_flag_from_media_rate() {
        let mut bytes = hex::decode("286b55e253f83d400000000000000001").unwrap();
        let ntsc = Rational::new(30000, 1001).unwrap();
        assert_eq!(frame_rate(&bytes).unwrap(), (ntsc, true));
        bytes[15] = 0;
        assert_eq!(frame_rate(&bytes).unwrap(), (ntsc, false));
        bytes[15] = 2;
        assert!(frame_rate(&bytes).is_err());
        bytes[..8].copy_from_slice(&25_f64.to_le_bytes());
        bytes[15] = 1;
        assert!(frame_rate(&bytes).is_err());
    }

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
