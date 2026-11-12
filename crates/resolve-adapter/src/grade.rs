//! One witnessed Resolve 21.2 clip-local primary gain node.
use crate::Result;
use graph::{source::SourceGraph, Value};
use sha2::{Digest, Sha256};

pub(crate) const ADAPTER: &str = "resolve.primary_gain";
const MASTER_VALUE: std::ops::Range<usize> = 141..145;
const NODE_FINGERPRINT: &str = "1eecb1c098d6a5195c07e02f9cc9f4a15d129f6f66156f63115ea769350e05a3";

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

pub(crate) fn prepare(graph: &SourceGraph, occurrence: &str) -> Result<Option<(f32, String)>> {
    let Some(row) = graph.sidecar(ADAPTER, occurrence) else {
        return Ok(None);
    };
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
    let (_, mut payload) =
        decode(&hex::decode(body)?).ok_or("unqualified Resolve grade node body")?;
    let gain = gain as f32;
    payload[MASTER_VALUE].copy_from_slice(&gain.to_le_bytes());
    let mut encoded = vec![0x81];
    encoded.extend(zstd::bulk::compress(&payload, 1)?);
    Ok(Some((gain, hex::encode(encoded))))
}
