//! One witnessed Resolve 21.2 clip-local primary gain node.
use crate::Result;
use graph::{source::SourceGraph, Value};
use sha2::{Digest, Sha256};

pub(crate) const ADAPTER: &str = "resolve.primary_gain";
pub(crate) const CDL_ADAPTER: &str = "resolve.cdl_slope";
pub(crate) const OPAQUE_ADAPTER: &str = "resolve.grade";
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
const NATIVE_EXPOSURE_ONE_FINGERPRINT: &str =
    "2ced4c337c3084a92208917ac43a950d5d6e29b4a87c9548d45a6ec8828dbe3b";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Operation {
    PrimaryGain(f32),
    ExposureCdl(f32),
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
    if data.len() == 393 && hex::encode(Sha256::digest(&data)) == NATIVE_EXPOSURE_ONE_FINGERPRINT {
        return Some(std::f32::consts::SQRT_2);
    }
    if data.len() != 409 {
        return None;
    }
    let first = CDL_SLOPE_OFFSETS[0];
    let value = data[first..first + 4].try_into().ok()?;
    let slope = f32::from_le_bytes(value);
    if !supported_slope(slope) {
        return None;
    }
    for offset in CDL_SLOPE_OFFSETS {
        let bytes = &mut data[offset..offset + 4];
        if bytes != value {
            return None;
        }
        bytes.copy_from_slice(&std::f32::consts::SQRT_2.to_le_bytes());
    }
    (data == hex::decode(CDL_SAVE_AS_BODY).ok()?).then_some(slope)
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
    let premiere = graph
        .sidecar(crate::premiere::color_mode::ADAPTER, occurrence)
        .and_then(crate::premiere::color_mode::qualified_exposure)
        .filter(|exposure| *exposure != 0.0);
    let cdl = graph.sidecar(CDL_ADAPTER, occurrence);
    if (premiere.is_some() && cdl.is_some())
        || ((premiere.is_some() || cdl.is_some()) && graph.sidecar(ADAPTER, occurrence).is_some())
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
    let Some(row) = graph.sidecar(ADAPTER, occurrence) else {
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
