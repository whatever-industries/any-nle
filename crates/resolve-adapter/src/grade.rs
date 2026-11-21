//! Witnessed Resolve 21.2 clip-local Color nodes.
use crate::{linked::SidecarLookup, Result};
use avid_reader::source::{saved_color_transforms_match, SourceColorRange};
use graph::{source::{SourceGraph, SourceMedia, SourceSequence}, Placement, StreamKind, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub(crate) const ADAPTER: &str = "resolve.primary_gain";
pub(crate) const CDL_ADAPTER: &str = "resolve.cdl_slope";
pub(crate) const RANGE_ADAPTER: &str = "resolve.cdl_range";
pub(crate) const OPAQUE_ADAPTER: &str = "resolve.grade";
pub(crate) const AVID_RANGE_LOSS: &str = "source_color_adapter";
const MASTER_VALUE: std::ops::Range<usize> = 141..145;
const NODE_FINGERPRINT: &str = "1eecb1c098d6a5195c07e02f9cc9f4a15d129f6f66156f63115ea769350e05a3";
// Clip CDL saved by Resolve 21.2 after Save Project As. The 409-byte body
// carries the three slopes without the extra fields in the native API export.
const CDL_SAVE_AS_BODY: &str = concat!(
    "0af702080110011a4808800f10b8081d0000803f20800f28b808350000803f38800f40b80848ffffffff",
    "0f7a240000803f0000000000000000000000000000803f0000000000000000000000000000803f3a8602",
    "0801100120be0128b4013801402c4ae7010add010801180132d60112d30108011a0c088580803012050d",
    "0000803f1a0c089880803012050d000000001a0c089980803012050d000000001a0c089a80803012050d",
    "000000001a0c089d80803012050df304b53f1a0c089e80803012050df304b53f1a0c089f80803012050d",
    "f304b53f1a0c08a280803012050d000000001a0c08a380803012050d000000001a0c08a480803012050d",
    "000000001a0c08fd80803012050d000000001a0c08fe80803012050d000000001a0c08ff80803012050d",
    "000000001a0d088b8080b00812050d000000001a0a089b8180b0081202100212050304050612520c0a0a",
    "08818080800c120210024a0e080110501a080801104018012001520e080210401a080802104018022001",
    "1a1d20be0128b401380140544a03120106520c0a0a08818080800c12021002",
);
const CDL_SLOPE_OFFSETS: [usize; 3] = [182, 196, 210];
const CDL_LIFT_OFFSETS: [usize; 3] = [140, 154, 168];
// Native SetCDL writes three variable-width integers outside the fixed
// controls. Hash the witnessed node with those integers removed and the
// enclosing lengths normalized. The hash still covers every RGB control.
const NATIVE_RANGE_FULL_TO_VIDEO: &str =
    "52f0c43534c021c4112cc66388176bd86ec520d49878c7cf5950a39c8190fa07";
const NATIVE_RANGE_VIDEO_TO_FULL: &str =
    "2b1e357b0c9eff0f270b0bd75a22ade556448a63e68106fecf8a6d6951c9a79b";
const NATIVE_EXPOSURE_ONE_FINGERPRINT: &str =
    "5a0f84f3ed9d8ce7ffecc7e78e1c63799db5e3f163f39b52227e3621bcb21ebd";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Operation {
    PrimaryGain(f32),
    ExposureCdl(f32),
    RangeCdl(SourceColorRange),
}

// These are the six native control values saved by Resolve's SetCDL operation
// for the two Avid mappings. Resolve evaluates its own Lift and Gain encoding.
fn range_body(range: SourceColorRange) -> Result<Vec<u8>> {
    let (lift, gain): (u32, u32) = match range {
        SourceColorRange::VideoToFull => (0xbd88acfb, 0x3f8b4d58),
        SourceColorRange::FullToVideo => (0x3d8b7034, 0x3f69d7f5),
    };
    let mut body = hex::decode(CDL_SAVE_AS_BODY)?;
    for (offsets, value) in [(CDL_LIFT_OFFSETS, lift), (CDL_SLOPE_OFFSETS, gain)] {
        for start in offsets {
            body[start..start + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    Ok(body)
}

pub(crate) fn decode_range_cdl(body: &[u8]) -> Option<SourceColorRange> {
    if body.len() > 1024 {
        return None;
    }
    let data = match body {
        [0x80, bytes @ ..] => bytes.to_vec(),
        [0x81, bytes @ ..] => zstd::bulk::decompress(bytes, 409).ok()?,
        _ => return None,
    };
    if data.len() != 409 {
        return match native_cdl_fingerprint(&data)?.as_str() {
            NATIVE_RANGE_FULL_TO_VIDEO => Some(SourceColorRange::FullToVideo),
            NATIVE_RANGE_VIDEO_TO_FULL => Some(SourceColorRange::VideoToFull),
            _ => None,
        };
    }
    [SourceColorRange::VideoToFull, SourceColorRange::FullToVideo]
        .into_iter().find(|&range| range_body(range).is_ok_and(|expected| expected == data))
}

fn native_cdl_fingerprint(data: &[u8]) -> Option<String> {
    use crate::binary::take_varint;

    let data = data.strip_prefix(&[0x0a])?;
    let mut start = 0;
    let bytes = usize::try_from(take_varint(data, &mut start).ok()?).ok()?;
    let (node, tail) = data.get(start..)?.split_at_checked(bytes)?;
    let tail = tail.strip_prefix(&[0x20])?;
    let mut tail_end = 0;
    let third = take_varint(tail, &mut tail_end).ok()?;
    if tail_end != tail.len() {
        return None;
    }
    let (prefix, payload) = node.split_at_checked(344)?;
    let mut at = 0;
    let first = take_varint(payload, &mut at).ok()?;
    let middle = payload.get(at..at + 33)?;
    let mut end = at + middle.len();
    let second = take_varint(payload, &mut end).ok()?;
    if !(3..=5).contains(&at) || prefix[79] != 0x8a + (at as u8 - 3)
        || end != payload.len() || first.checked_mul(2) != Some(second) || third > second
    {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(&prefix[..79]);
    hash.update([0x8a]);
    hash.update(&prefix[80..]);
    hash.update(middle);
    hash.update([0x20]);
    Some(hex::encode(hash.finalize()))
}
fn prepare_range(range: SourceColorRange) -> Result<(Operation, String)> {
    let mut encoded = vec![0x81];
    encoded.extend(zstd::bulk::compress(&range_body(range)?, 1)?);
    Ok((Operation::RangeCdl(range), hex::encode(encoded)))
}

pub(crate) fn range_direction(range: SourceColorRange) -> &'static str {
    match range {
        SourceColorRange::FullToVideo => "full_to_video",
        SourceColorRange::VideoToFull => "video_to_full",
    }
}

fn text<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    match row.get(key)? {
        Value::Str(text) => Some(text),
        _ => None,
    }
}

pub(crate) struct AvidRanges<'a> {
    rows: HashMap<&'a str, &'a Value>,
    transition_ends: HashSet<&'a str>,
}

impl<'a> AvidRanges<'a> {
    pub(crate) fn new(sidecars: SidecarLookup<'a>, sequence: &'a SourceSequence) -> Result<Self> {
        let mut rows = HashMap::new();
        if let Some(Value::Arr(issues)) = sidecars.get("avid.source", &sequence.id)
            .and_then(|s| s.get("unresolved"))
        {
            for issue in issues {
                if text(issue, "effect_id") != Some("EFF2_LUTSFX") {
                    continue;
                }
                let Some(facts) = issue.get("color_adapter") else { continue };
                let Some(id) = text(facts, "placement") else { continue };
                if rows.insert(id, facts).is_some() {
                    return Err(format!("{id}: duplicate Avid Color Adapter owners").into());
                }
            }
        }
        let transition_ends = sequence.transitions.iter()
            .filter(|transition| transition.kind == StreamKind::Video)
            .flat_map(|transition| transition.outgoing.placements.iter().chain(&transition.incoming.placements))
            .map(String::as_str).collect();
        Ok(Self { rows, transition_ends })
    }

    pub(crate) fn has_ranges(&self) -> bool {
        self.rows.values().any(|row| text(row, "text")
            .and_then(SourceColorRange::from_saved_xml).is_some())
    }

    pub(crate) fn prepare(
        &self, sidecars: SidecarLookup<'_>, edit: &Placement, media: &SourceMedia,
    ) -> Result<Option<(Operation, String)>> {
        let Some(row) = self.rows.get(edit.id.as_str()) else { return Ok(None) };
        let Some(xml) = text(row, "text") else { return Ok(None) };
        let Some(range) = SourceColorRange::from_saved_xml(xml) else { return Ok(None) };
        let Some(package) = text(row, "picture_file") else { return Ok(None) };
        let Some(package_hex) = package.strip_prefix("urn:smpte:umid:").map(|id| id.replace('.', "")) else {
            return Ok(None);
        };
        let native = sidecars.get("avid.source", &edit.id);
        let plain = matches!(native.and_then(|r| r.get("via")), Some(Value::Arr(via))
            if matches!(via.as_slice(), [Value::Str(stage)] if stage == "effect EFF2_LUTSFX not converted"));
        let same_package = sidecars.get("avid.source", &media.id)
            .and_then(|record| text(record, "file_package"))
            .is_some_and(|id| id.eq_ignore_ascii_case(&package_hex));
        let source_matches = matches!(sidecars.get("avid.source", &edit.item)
            .and_then(|record| record.get("input_color_transforms")), Some(Value::Arr(rows))
            if matches!(rows.as_slice(), [saved] if text(saved, "owner_mob") == Some(package)
                && text(saved, "text").is_some_and(|text| saved_color_transforms_match(xml, text))
                && saved.get("track") == Some(&Value::Int(1))));
        if row.get("neutral_wrapper") != Some(&Value::Bool(true))
            || row.get("subclip_motion").is_some() || edit.video_track != Some(1)
            || edit.speed.is_some() || edit.frame_map.is_some()
            || edit.transform.is_some() || !edit.effects.is_empty()
            || self.transition_ends.contains(edit.id.as_str())
            || !plain || !same_package || !source_matches
        {
            return Ok(None);
        }
        prepare_range(range).map(Some)
    }
}

// Only equal RGB slopes on the qualified clip node are editable here.
// The rest of the body includes zero offset, unity power and saturation.
pub(crate) fn decode_exposure_cdl(body: &[u8]) -> Option<f32> {
    if body.len() > 1024 {
        return None;
    }
    let mut data = match body {
        [0x80, bytes @ ..] => bytes.to_vec(),
        [0x81, bytes @ ..] => zstd::bulk::decompress(bytes, 409).ok()?,
        _ => return None,
    };
    let first = CDL_SLOPE_OFFSETS[0];
    let value = data.get(first..first + 4)?.try_into().ok()?;
    let slope = f32::from_le_bytes(value);
    if !supported_slope(slope) {
        return None;
    }
    for offset in CDL_SLOPE_OFFSETS {
        let bytes = data.get_mut(offset..offset + 4)?;
        if bytes != value {
            return None;
        }
        bytes.copy_from_slice(&std::f32::consts::SQRT_2.to_le_bytes());
    }
    let matches = if data.len() == 409 {
        data == hex::decode(CDL_SAVE_AS_BODY).ok()?
    } else {
        native_cdl_fingerprint(&data)?.as_str() == NATIVE_EXPOSURE_ONE_FINGERPRINT
    };
    matches.then_some(slope)
}

fn supported_slope(slope: f32) -> bool {
    ((-2.5f64).exp2() as f32..=2.5f64.exp2() as f32).contains(&slope)
}

fn encode_exposure_cdl(slope: f32) -> Result<String> {
    let mut payload = hex::decode(CDL_SAVE_AS_BODY)?;
    for offset in CDL_SLOPE_OFFSETS {
        payload[offset..offset + 4].copy_from_slice(&slope.to_le_bytes());
    }
    let mut encoded = vec![0x81];
    encoded.extend(zstd::bulk::compress(&payload, 1)?);
    Ok(hex::encode(encoded))
}

pub(crate) fn decode(body: &[u8]) -> Option<(f32, Vec<u8>)> {
    if body.len() > 1024 {
        return None;
    }
    let data = match body {
        [0x80, bytes @ ..] => bytes.to_vec(),
        [0x81, bytes @ ..] => zstd::bulk::decompress(bytes, 268).ok()?,
        _ => return None,
    };
    if data.len() != 268 {
        return None;
    }
    let value = f32::from_le_bytes(data[MASTER_VALUE].try_into().ok()?);
    if !(0.0..=2.0).contains(&value) {
        return None;
    }
    let mut neutral = data;
    neutral[MASTER_VALUE].copy_from_slice(&1.0f32.to_le_bytes());
    (hex::encode(Sha256::digest(&neutral)) == NODE_FINGERPRINT).then_some((value, neutral))
}

fn read_receipt(row: &Value) -> Result<(f32, f32, Vec<u8>)> {
    let gain = match row.get("gain") {
        Some(Value::Float(gain)) => *gain,
        Some(Value::Int(gain @ 0..=2)) => *gain as f64,
        _ => return Err("incomplete Resolve primary gain value".into()),
    };
    let Some(Value::Str(body)) = row.get("body") else {
        return Err("missing Resolve primary gain body".into());
    };
    if row.get("node_id") != Some(&Value::Int(1))
        || !matches!(row.get("value_space"), Some(Value::Str(space)) if space == "resolve.drx")
        || !(0.0..=2.0).contains(&gain)
        || f64::from(gain as f32) != gain
        || body.len() > 2048
    {
        return Err("unsupported Resolve primary gain value or node".into());
    }
    let (saved, payload) =
        decode(&hex::decode(body)?).ok_or("unqualified Resolve grade node body")?;
    Ok((gain as f32, saved, payload))
}

/// Unlike a writer request, a saved observation must agree with its node body.
pub(crate) fn saved_gain(row: &Value) -> Result<f32> {
    let (gain, saved, _) = read_receipt(row)?;
    if gain != saved {
        return Err("Resolve primary gain disagrees with the saved node body".into());
    }
    Ok(gain)
}

pub(crate) fn prepare(
    graph: &SourceGraph,
    occurrence: &str,
) -> Result<Option<(Operation, String)>> {
    prepare_with_sidecars(SidecarLookup::Linear(graph), occurrence)
}

pub(crate) fn prepare_with_sidecars(
    sidecars: SidecarLookup<'_>,
    occurrence: &str,
) -> Result<Option<(Operation, String)>> {
    let premiere = sidecars
        .get(crate::premiere::color_mode::ADAPTER, occurrence)
        .and_then(crate::premiere::color_mode::qualified_exposure)
        .filter(|exposure| *exposure != 0.0);
    let cdl = sidecars.get(CDL_ADAPTER, occurrence);
    if let Some(row) = sidecars.get(RANGE_ADAPTER, occurrence) {
        if premiere.is_some() || cdl.is_some() || sidecars.get(ADAPTER, occurrence).is_some()
            || row.get("node_id") != Some(&Value::Int(1))
        {
            return Err("conflicting or invalid Resolve range CDL".into());
        }
        return match text(row, "direction") {
            Some("full_to_video") => prepare_range(SourceColorRange::FullToVideo).map(Some),
            Some("video_to_full") => prepare_range(SourceColorRange::VideoToFull).map(Some),
            _ => Err("unsupported Resolve range CDL direction".into()),
        };
    }
    if (premiere.is_some() && cdl.is_some())
        || ((premiere.is_some() || cdl.is_some()) && sidecars.get(ADAPTER, occurrence).is_some())
    {
        return Err("conflicting Resolve grade owners".into());
    }
    let slope = if let Some(row) = cdl {
        let Some(Value::Float(saved)) = row.get("slope") else {
            return Err("missing Resolve exposure CDL slope".into());
        };
        let slope = *saved as f32;
        if row.get("node_id") != Some(&Value::Int(1))
            || !supported_slope(slope)
            || f64::from(slope) != *saved
            || !matches!(row.get("value_space"), Some(Value::Str(space)) if space == "resolve.cdl")
        {
            return Err("unsupported Resolve exposure CDL receipt".into());
        }
        Some(slope)
    } else {
        premiere.map(|exposure| (exposure / 2.0).exp2() as f32)
    };
    if let Some(slope) = slope {
        return Ok(Some((
            Operation::ExposureCdl(slope),
            encode_exposure_cdl(slope)?,
        )));
    }
    let Some(row) = sidecars.get(ADAPTER, occurrence) else {
        return Ok(None);
    };
    let (gain, _, mut payload) = read_receipt(row)?;
    payload[MASTER_VALUE].copy_from_slice(&gain.to_le_bytes());
    let mut encoded = vec![0x81];
    encoded.extend(zstd::bulk::compress(&payload, 1)?);
    Ok(Some((Operation::PrimaryGain(gain), hex::encode(encoded))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_cdl_requires_the_complete_witnessed_node() {
        for range in [SourceColorRange::VideoToFull, SourceColorRange::FullToVideo] {
            let (_, body) = prepare_range(range).unwrap();
            assert_eq!(decode_range_cdl(&hex::decode(body).unwrap()), Some(range));
            let mut changed = vec![0x80];
            changed.extend(range_body(range).unwrap());
            changed[1 + CDL_LIFT_OFFSETS[1]] ^= 1;
            assert_eq!(decode_range_cdl(&changed), None);
        }
    }

    #[test]
    fn exposure_cdl_requires_all_three_saved_channel_slopes() {
        for exposure in [-5.0f64, -2.0, 0.0, 0.5, 2.0, 5.0] {
            let slope = (exposure / 2.0).exp2() as f32;
            let saved = hex::decode(encode_exposure_cdl(slope).unwrap()).unwrap();
            assert_eq!(decode_exposure_cdl(&saved), Some(slope));
        }
        for invalid in [0.0, 6.0, f32::NAN] {
            let saved = hex::decode(encode_exposure_cdl(invalid).unwrap()).unwrap();
            assert!(decode_exposure_cdl(&saved).is_none());
        }
        let saved = hex::decode(encode_exposure_cdl(2.0).unwrap()).unwrap();
        let mut changed = vec![0x80];
        changed.extend(zstd::bulk::decompress(&saved[1..], 409).unwrap());
        assert_eq!(decode_exposure_cdl(&changed), Some(2.0));
        let offset = 1 + CDL_SLOPE_OFFSETS[0];
        changed[offset..offset + 4].copy_from_slice(&1.0f32.to_le_bytes());
        assert!(decode_exposure_cdl(&changed).is_none());
    }
}
