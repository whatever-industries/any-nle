//! Bounded source facts whose destination behavior is still unqualified.
use super::{encoding, omitted, text, Grid, Node};
use crate::Result;
use graph::{source::SourceGraph, Sidecar, StreamKind, Value};
use sha2::{Digest, Sha256};

/// Preserve the on-disk codes independently. The native video witness has
/// different AlignmentType and Position values, so neither implies the cut.
pub(super) fn transition(
    graph: &mut SourceGraph,
    node: &Node,
    sequence: &str,
    track_id: &str,
    kind: StreamKind,
    index: i64,
    grid: Grid,
) -> Result<()> {
    let id = node.id()?;
    let mut fields = vec![
        ("id", text(id)),
        ("native_track_id", text(track_id)),
        (
            "kind",
            text(if kind == StreamKind::Video {
                "video"
            } else {
                "audio"
            }),
        ),
        ("track", Value::Int(index)),
        ("track_rate_numerator", Value::Int(grid.rate.num())),
        ("track_rate_denominator", Value::Int(grid.rate.den())),
        ("timeline_origin_frames", Value::Int(grid.origin)),
        ("interpretation", text("saved_fields_only")),
    ];
    for (tag, field) in [
        ("PrettyType", "type_label"),
        ("Name", "name"),
        ("Start", "saved_start"),
        ("Duration", "saved_duration"),
        ("AlignmentType", "alignment_type"),
        ("Position", "position"),
        ("Flags", "flags"),
    ] {
        if let Some(value) = node.maybe(tag)? {
            fields.push((field, text(value)));
        }
    }
    for (tag, field) in [
        ("EffectFiltersBA", "effect_filters_sha256"),
        ("FieldsBlob", "fields_blob_sha256"),
    ] {
        if let Some(encoded) = node.maybe(tag)?.filter(|value| !value.is_empty()) {
            // Hash the decoded field. The archive reader already caps each XML value.
            let bytes = hex::decode(encoded)?;
            fields.push((field, Value::Str(hex::encode(Sha256::digest(bytes)))));
        }
    }
    append(
        graph,
        "resolve.transitions",
        sequence,
        Value::Obj(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        ),
    );
    omitted(
        graph,
        sequence,
        id,
        "transition",
        "Resolve transition fields retained; cut and handle mapping are unqualified",
    );
    Ok(())
}

pub(super) fn append(graph: &mut SourceGraph, adapter: &str, owner: &str, value: Value) {
    if let Some(Sidecar {
        data: Value::Arr(records),
        ..
    }) = graph
        .sidecars
        .iter_mut()
        .rev()
        .find(|row| row.adapter == adapter && row.object == owner)
    {
        records.push(value);
    } else {
        graph.sidecars.push(Sidecar {
            adapter: adapter.into(),
            object: owner.into(),
            data: Value::Arr(vec![value]),
        });
    }
}

/// Capture the saved map coordinates. The interpretation of the clip's In
/// field and the frame sampling law remain a separate conversion decision.
pub(super) fn time_map(encoded: &str, blobs: &mut encoding::Blobs) -> Result<Value> {
    let bytes = encoding::hex_bytes(encoded)?;
    blobs.charge(bytes.len())?;
    let mut row = vec![
        ("byte_count".into(), Value::Int(bytes.len() as i64)),
        (
            "sha256".into(),
            Value::Str(hex::encode(Sha256::digest(&bytes))),
        ),
    ];
    if let Some(values) = compact_map(&bytes) {
        row.push(("form".into(), text("compact")));
        row.push(("saved_seconds".into(), Value::Arr(values)));
    } else if bytes.starts_with(&1u32.to_be_bytes()) {
        if let Ok(map) = encoding::qt(&bytes) {
            if map.text("DbType").ok().flatten() == Some("Sm2TimeMap") {
                row.push(("form".into(), text("Sm2TimeMap")));
                row.push((
                    "provenance".into(),
                    text("drp-format:1e2bf034 documented fields"),
                ));
                for (key, label) in [
                    ("XMax", "x_max_seconds"),
                    ("LastValidYOffset", "last_valid_y_offset_seconds"),
                ] {
                    if let Ok(value) = map.real(key) {
                        row.push((label.into(), Value::Float(value)));
                    }
                }
                if let Ok(mode) = map.int("Mode") {
                    row.push(("saved_mode".into(), Value::Int(mode)));
                }
                if let Ok(keyframes) = map.bytes("KeyframesBA") {
                    if let Some(keys) = map_keys(keyframes) {
                        if let Some((_, Value::Arr(points))) = keys.first() {
                            blobs.charge(512 * points.len())?;
                        }
                        row.push(("points".into(), Value::Obj(keys)));
                    }
                }
            }
        }
    }
    Ok(Value::Obj(row))
}

fn compact_map(bytes: &[u8]) -> Option<Vec<Value>> {
    let [2, payload @ ..] = bytes else {
        return None;
    };
    if !matches!(payload.len(), 8 | 40) {
        return None;
    }
    payload
        .chunks_exact(8)
        .map(|raw| {
            let value = f64::from_be_bytes(raw.try_into().ok()?);
            value.is_finite().then_some(Value::Float(value))
        })
        .collect()
}

fn map_keys(bytes: &[u8]) -> Option<Vec<(String, Value)>> {
    use encoding::Wire;
    let decoded;
    let bytes = if bytes.starts_with(&[0x81, 0x28, 0xb5, 0x2f, 0xfd]) {
        decoded = zstd::bulk::decompress(&bytes[1..], 16 << 10).ok()?;
        &decoded
    } else {
        bytes
    };
    let mut keys = Vec::new();
    let mut origin = None;
    for (id, wire) in encoding::fields(bytes).ok()? {
        match (id, wire) {
            (160, Wire::Int(0)) => {}
            (2, Wire::Fixed(bytes)) if origin.is_none() => origin = Some(double(bytes)?),
            (1, Wire::Bytes(bytes)) if keys.len() < 256 => {
                let mut coordinates = [None, None];
                let mut handles = None;
                for (tag, wire) in encoding::fields(bytes).ok()? {
                    match (tag, wire) {
                        (tag @ (1 | 2), Wire::Fixed(bytes)) => {
                            let slot = &mut coordinates[(tag - 1) as usize];
                            if slot.replace(double(bytes)?).is_some() {
                                return None;
                            }
                        }
                        (3, Wire::Bytes(bytes)) if bytes.len() == 48 && handles.is_none() => {
                            handles = Some(
                                bytes
                                    .chunks_exact(8)
                                    .map(double)
                                    .collect::<Option<Vec<_>>>()?,
                            );
                        }
                        _ => return None,
                    }
                }
                // In the protobuf representation an omitted coordinate is zero.
                let mut fields = vec![
                    (
                        "record_seconds".into(),
                        Value::Float(coordinates[0].unwrap_or(0.0)),
                    ),
                    (
                        "source_seconds".into(),
                        Value::Float(coordinates[1].unwrap_or(0.0)),
                    ),
                    (
                        "record_explicit".into(),
                        Value::Bool(coordinates[0].is_some()),
                    ),
                    (
                        "source_explicit".into(),
                        Value::Bool(coordinates[1].is_some()),
                    ),
                ];
                if let Some(handles) = handles {
                    fields.push((
                        "saved_handles".into(),
                        Value::Arr(handles.into_iter().map(Value::Float).collect()),
                    ));
                }
                keys.push(Value::Obj(fields));
            }
            _ => return None,
        }
    }
    if keys.is_empty() {
        return None;
    }
    let mut fields = vec![("keyframes".into(), Value::Arr(keys))];
    if let Some(origin) = origin {
        fields.push(("source_origin_seconds".into(), Value::Float(origin)));
    }
    Some(fields)
}

fn double(bytes: &[u8]) -> Option<f64> {
    let value = f64::from_le_bytes(bytes.try_into().ok()?);
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_time_map_retains_missing_coordinate_and_explicit_zero() {
        let mut saved = vec![0x0a, 0x09, 0x11];
        saved.extend_from_slice(&4.0f64.to_le_bytes());
        saved.extend_from_slice(&[0x0a, 0x12, 0x09]);
        saved.extend_from_slice(&2.0f64.to_le_bytes());
        saved.push(0x11);
        saved.extend_from_slice(&0.0f64.to_le_bytes());
        let keys = Value::Obj(map_keys(&saved).unwrap());
        let Some(Value::Arr(rows)) = keys.get("keyframes") else {
            panic!("missing keyframes")
        };
        assert_eq!(rows[0].get("record_explicit"), Some(&Value::Bool(false)));
        assert_eq!(rows[0].get("source_seconds"), Some(&Value::Float(4.0)));
        assert_eq!(rows[1].get("record_seconds"), Some(&Value::Float(2.0)));
        assert_eq!(rows[1].get("source_explicit"), Some(&Value::Bool(true)));
        saved.pop();
        assert!(map_keys(&saved).is_none());
    }
}
