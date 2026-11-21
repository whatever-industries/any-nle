//! Read the saved Resolve encodings used by the source profile.
use crate::{binary::take_varint, Result};
use graph::Rational;
use std::collections::BTreeMap;

pub(super) const MAX_BLOB: usize = 1 << 16;
const MAX_LOCKABLE_FIELDS_BYTES: usize = 256 << 10;
const MAX_LOCKABLE_BODY_BYTES: usize = 512 << 10;

pub(super) struct Blobs {
    used: usize,
    limit: usize,
}

impl Default for Blobs {
    fn default() -> Self {
        Self::new(32 << 20)
    }
}

impl Blobs {
    pub(super) fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    pub(super) fn charge(&mut self, bytes: usize) -> Result<()> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or("Resolve decoded blob budget exceeded")?;
        Ok(())
    }

    pub fn qt(&mut self, encoded: &str) -> Result<Map> {
        let data = hex_bytes(encoded)?;
        self.charge(data.len())?;
        qt(&data)
    }

    pub fn lockable_qt(&mut self, encoded: &str) -> Result<Map> {
        if encoded.len() > MAX_LOCKABLE_FIELDS_BYTES * 2 {
            return Err("Resolve lockable metadata field exceeds stored size limit".into());
        }
        self.charge(encoded.len() / 2)?;
        qt_with_limit(&hex::decode(encoded)?, MAX_LOCKABLE_FIELDS_BYTES)
    }

    pub fn marker_data(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.lockable_body(data, MAX_BLOB)
    }

    pub fn lockable_metadata(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.lockable_body(data, MAX_LOCKABLE_BODY_BYTES)
    }

    fn lockable_body(&mut self, data: &[u8], limit: usize) -> Result<Vec<u8>> {
        if data.len() < 9
            || data[..4] != 10001u32.to_be_bytes()
            || u32::from_be_bytes(data[4..8].try_into()?) as usize != data.len() - 8
        {
            return Err("malformed Resolve marker blob header".into());
        }
        let remaining = (self.limit - self.used).min(limit);
        let payload = &data[9..];
        let raw = match data[8] {
            0x80 if payload.len() <= remaining => payload.to_vec(),
            0x80 => {
                return Err("Resolve lockable metadata exceeds decoded size limit or budget".into());
            }
            0x81 => {
                let size = zstd::zstd_safe::get_frame_content_size(payload)
                    .map_err(|_| "malformed Resolve lockable metadata zstd frame")?;
                if size.is_some_and(|size| size > remaining as u64) {
                    return Err(
                        "Resolve lockable metadata exceeds decoded size limit or budget".into(),
                    );
                }
                zstd::bulk::decompress(payload, remaining)?
            }
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
    PixelAspect(u64),
    Unused,
}
pub(super) struct Map(BTreeMap<String, Qt>);

pub(super) struct PixelAspect {
    pub ratio: Rational,
    pub saved_bits: u64,
}

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

    pub fn pixel_aspect(&self) -> Result<Option<PixelAspect>> {
        let saved_bits = match self.0.get("PixelAspectRatio") {
            None | Some(Qt::Unused) => return Ok(None),
            Some(Qt::PixelAspect(bits)) => *bits,
            _ => return Err("unsupported Resolve pixel aspect ratio encoding".into()),
        };
        let value = f64::from_bits(saved_bits);
        if !value.is_finite() || value <= 0.0 {
            return Err("invalid Resolve pixel aspect ratio".into());
        }
        let ratio = binary64_rational(saved_bits).ok_or("Resolve pixel aspect ratio exceeds graph bounds")?;
        Ok(Some(PixelAspect { ratio, saved_bits }))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// Decode a finite IEEE 754 value into a signed significand and a power of two.
pub(super) fn binary64_parts(bits: u64) -> Option<(i64, i32)> {
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0x7ff {
        return None;
    }
    let significand = (bits & ((1 << 52) - 1)) | if exponent == 0 { 0 } else { 1 << 52 };
    if significand == 0 {
        return Some((0, 0));
    }
    let zeros = significand.trailing_zeros();
    let magnitude = (significand >> zeros) as i64;
    let signed = if bits >> 63 == 0 { magnitude } else { -magnitude };
    Some((signed, exponent.max(1) - 1023 - 52 + zeros as i32))
}

pub(super) fn binary64_rational(bits: u64) -> Option<Rational> {
    let (significand, power) = binary64_parts(bits)?;
    let numerator = i128::from(significand);
    match power {
        -62..=-1 => Rational::new(numerator, 1 << -power),
        0..=63 => Rational::new(numerator << power, 1),
        _ => None,
    }
}

pub(super) fn qt(data: &[u8]) -> Result<Map> {
    qt_with_limit(data, MAX_BLOB)
}

fn qt_with_limit(mut data: &[u8], limit: usize) -> Result<Map> {
    if data.len() > limit || u32::from_be_bytes(number(&mut data)?) != 1 {
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
            135 if key == "PixelAspectRatio" => Qt::PixelAspect(u64::from_be_bytes(number(&mut data)?)),
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
    let mut fields = Vec::new();
    visit_fields(data, 4096, |field, value| {
        fields.push((field, value));
        Ok(())
    })?;
    Ok(fields)
}

pub(super) fn visit_fields<'a>(
    data: &'a [u8],
    limit: usize,
    mut visit: impl FnMut(u64, Wire<'a>) -> Result<()>,
) -> Result<()> {
    let mut at = 0;
    let mut count = 0;
    while at < data.len() {
        let key = take_varint(data, &mut at)?;
        let field = key >> 3;
        if field == 0 || field > (1 << 29) - 1 || count == limit {
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
        visit(field, value)?;
        count += 1;
    }
    Ok(())
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
        x if x == 30000.0 / 1001.0 => (30000, 1001),
        x if x == 48000.0 / 1001.0 => (48000, 1001),
        x if x == 60000.0 / 1001.0 => (60000, 1001),
        // A two-digit saved frame label can represent indices 0 through 99.
        x if (1.0..=100.0).contains(&x) && x.fract() == 0.0 => (x as i128, 1),
        _ => return Err("unsupported Resolve source frame rate".into()),
    };
    if *drop == 1 && n != 30000 {
        return Err("unqualified drop-frame flag for Resolve frame rate".into());
    }
    Ok((Rational::new(n, d).expect("known rate"), *drop == 1))
}

pub(super) fn display_base(rate: Rational) -> Result<u16> {
    match (rate.num(), rate.den()) {
        (n @ 1..=100, 1) => Ok(n as u16),
        (n @ (24000 | 30000 | 48000 | 60000), 1001) => Ok((n / 1000) as u16),
        _ => Err("unsupported Resolve source timecode base".into()),
    }
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

pub(super) fn sample_start(seconds: f64, sample_rate: i64) -> Result<Rational> {
    let samples = seconds * sample_rate as f64;
    let rounded = samples.round();
    if !(0.0..i64::MAX as f64).contains(&samples) || (samples - rounded).abs() > 1e-6 {
        return Err("Resolve audio starts outside its sample grid".into());
    }
    Rational::from_units(rounded as i64, sample_rate).ok_or_else(|| "invalid audio start".into())
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
    let (start, length) = extent_seconds(data)?;
    Ok((frames(start, rate)?, frames(length, rate)?))
}

pub(super) fn extent_seconds(data: &[u8]) -> Result<(f64, f64)> {
    let (start, length) = data.split_at_checked(8).ok_or("truncated Resolve extent")?;
    Ok((
        f64::from_le_bytes(start.try_into()?),
        f64::from_le_bytes(length.try_into()?),
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
    fn source_lockable_metadata_has_separate_stored_and_decoded_limits() {
        use crate::binary::{qt as encode_qt, Qt as Field};

        let payload = vec![0; MAX_BLOB + 1];
        let stored = hex::encode(encode_qt(&[("BlobData", Field::Bytes(&payload))]));
        let mut blobs = Blobs::default();
        assert!(blobs.qt(&stored).is_err());
        assert_eq!(blobs.lockable_qt(&stored).unwrap().bytes("BlobData").unwrap(), payload);
        assert!(blobs.lockable_qt(&"00".repeat(MAX_LOCKABLE_FIELDS_BYTES + 1)).is_err());
        assert!(blobs.lockable_qt("00").is_err());

        let compress = |raw: &[u8]| {
            let compressed = zstd::bulk::compress(raw, 1).unwrap();
            let mut envelope = 10001u32.to_be_bytes().to_vec();
            envelope.extend_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
            envelope.push(0x81);
            envelope.extend(compressed);
            envelope
        };
        let envelope = compress(&payload);
        assert!(blobs.marker_data(&envelope).is_err());
        assert_eq!(blobs.lockable_metadata(&envelope).unwrap(), payload);
        assert!(blobs.lockable_metadata(&compress(&vec![0; MAX_LOCKABLE_BODY_BYTES + 1])).is_err());

        let mut budget = Blobs::new(payload.len());
        budget.charge(1).unwrap();
        assert!(budget.lockable_metadata(&envelope).is_err());
        assert!(blobs.lockable_metadata(&envelope[..envelope.len() - 1]).is_err());
    }

    #[test]
    fn saved_pixel_aspect_is_an_exact_positive_binary64_ratio() {
        use crate::binary::{qt as encode_qt, Qt as Field};
        let encode = |key, value| {
            let mut map = encode_qt(&[(key, Field::Double(value))]);
            let type_start = map.len() - 13;
            map[type_start..type_start + 4].copy_from_slice(&135u32.to_be_bytes());
            map
        };
        for (value, exact) in [(1.0, "1"), (1.125, "9/8"), (4.0 / 3.0, "6004799503160661/4503599627370496")] {
            let field = qt(&encode("PixelAspectRatio", value)).unwrap().pixel_aspect().unwrap().unwrap();
            assert_eq!(field.ratio.to_string(), exact);
            assert_eq!(field.saved_bits, value.to_bits());
        }
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MIN_POSITIVE, f64::MAX] {
            assert!(qt(&encode("PixelAspectRatio", value)).unwrap().pixel_aspect().is_err());
        }
        assert!(qt(&encode("DifferentField", 1.125)).is_err());
        let mut truncated = encode("PixelAspectRatio", 1.125);
        truncated.pop();
        assert!(qt(&truncated).is_err());
    }

    #[test]
    fn saved_thirty_is_distinct_from_the_fractional_drop_frame_rate() {
        let mut field = [0; 16];
        field[..8].copy_from_slice(&30.0f64.to_le_bytes());
        let exact = Rational::integer(30);
        assert_eq!(frame_rate(&field).unwrap(), (exact, false));
        assert_eq!(timecode("00:00:59:29", i64::from(display_base(exact).unwrap())).unwrap(), 1799);
        field[15] = 1;
        assert!(frame_rate(&field).is_err());
        field[..8].copy_from_slice(&(30_000.0f64 / 1001.0).to_le_bytes());
        assert_eq!(frame_rate(&field).unwrap(), (Rational::new(30_000, 1001).unwrap(), true));
    }

    #[test]
    fn observed_integral_and_ntsc_picture_rates_keep_exact_clocks() {
        for (saved, numerator, denominator, base) in [
            (15.0, 15, 1, 15), (16.0, 16, 1, 16), (20.0, 20, 1, 20),
            (27.0, 27, 1, 27), (45.0, 45, 1, 45), (56.0, 56, 1, 56),
            (48000.0 / 1001.0, 48000, 1001, 48), (60000.0 / 1001.0, 60000, 1001, 60),
        ] {
            let mut field = [0; 16];
            field[..8].copy_from_slice(&f64::to_le_bytes(saved));
            let exact = Rational::new(numerator, denominator).unwrap();
            assert_eq!(frame_rate(&field).unwrap(), (exact, false));
            assert_eq!(display_base(exact).unwrap(), base);
            assert_eq!(timecode(&format!("01:02:03:{:02}", base - 1), i64::from(base)).unwrap(),
                3724 * i64::from(base) - 1);
            field[15] = 1;
            assert!(frame_rate(&field).is_err());
        }
        for saved in [0.0, -15.0, 101.0, 47.952, 59.94, f64::NAN, f64::INFINITY] {
            let mut field = [0; 16];
            field[..8].copy_from_slice(&saved.to_le_bytes());
            assert!(frame_rate(&field).is_err());
        }
    }

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
