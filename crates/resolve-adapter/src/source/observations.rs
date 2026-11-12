//! Bounded source facts whose destination behavior is still unqualified.
use super::{encoding, omitted, text, Grid, Node};
use crate::{dissolve, Result};
use graph::{source::SourceGraph, Sidecar, StreamKind, Value};
use sha2::{Digest, Sha256};

/// Preserve the native codes independently and attach endpoint evidence for the
/// qualified profile only when the saved item order and ranges agree.
#[allow(clippy::too_many_arguments)]
pub(super) fn transition(
    graph: &mut SourceGraph,
    node: &Node,
    sequence: &str,
    track_id: &str,
    kind: StreamKind,
    index: i64,
    grid: Grid,
    neighbors: [Option<&Node>; 2],
    timeline_export: bool,
) -> Result<()> {
    let id = node.id()?;
    let audio = kind == StreamKind::Audio;
    let qualified = if index == 1
        && (!audio || (!timeline_export && grid.rate == graph::Rational::integer(24)))
        && dissolve::supported_rate(grid.rate)
        && grid.size == (1920, 1080)
    {
        read_transition(node, neighbors, grid, timeline_export, audio)?
    } else {
        None
    };
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
        (
            "interpretation",
            text(match qualified {
                Some(_) if audio => dissolve::AUDIO_INTERPRETATION,
                Some((_, _, _, 4)) => dissolve::CENTER_INTERPRETATION,
                Some(_) => dissolve::INTERPRETATION,
                None => "saved_fields_only",
            }),
        ),
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
    if let Some((cut, outgoing, incoming, _)) = qualified {
        fields.extend([
            ("cut_frames", Value::Int(cut)),
            ("outgoing", text(outgoing)),
            ("incoming", text(incoming)),
        ]);
    }
    append(
        graph,
        dissolve::ADAPTER,
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
        if timeline_export && qualified.is_some() {
            dissolve::TIMELINE_EXPORT_LOSS
        } else {
            dissolve::LOSS
        },
    );
    Ok(())
}

fn read_transition<'a>(
    node: &Node,
    neighbors: [Option<&'a Node>; 2],
    grid: Grid,
    timeline_export: bool,
    audio: bool,
) -> Result<Option<(i64, &'a str, &'a str, i64)>> {
    let [Some(left), Some(right)] = neighbors else {
        return Ok(None);
    };
    let clip_class = if audio { "Sm2TiAudioClip" } else { "Sm2TiVideoClip" };
    if left.name != clip_class || right.name != clip_class {
        return Ok(None);
    }
    let before = match node.maybe("AlignmentType")? {
        Some(dissolve::ALIGNMENT) if !audio => dissolve::DURATION,
        Some("2") => 4,
        _ => return Ok(None),
    };
    for (field, expected) in [
        ("PrettyType", if audio { dissolve::AUDIO_TYPE } else { dissolve::TYPE }),
        ("Position", dissolve::POSITION),
        ("Duration", "8"),
        ("Flags", "0"),
        ("WasDisbanded", "false"),
        ("PriorityIndex", "0"),
    ] {
        if node.maybe(field)? != Some(expected) {
            return Ok(None);
        }
    }
    for field in ["LinkedItemSync", "MarkersBA", "ImportExportMetadataBA"] {
        if node.maybe(field)?.is_some_and(|value| !value.is_empty()) {
            return Ok(None);
        }
    }
    let effect = node.maybe("EffectFiltersBA")?.unwrap_or("");
    if effect != if audio { "" } else { dissolve::EFFECT }
        || !node
            .maybe("FieldsBlob")?
            .is_some_and(|fields| {
                if audio { fields == dissolve::AUDIO_FIELDS }
                else if timeline_export { dissolve::timeline_export_fields_match(fields) }
                else { dissolve::native_fields_match(fields) }
            })
    {
        return Ok(None);
    }
    let number = |node: &Node, key| -> Result<Option<i64>> {
        Ok(node.maybe(key)?.and_then(|s| s.parse().ok()))
    };
    let (Some(start), Some(left_start), Some(left_length), Some(cut), Some(incoming_in)) = (
        number(node, "Start")?,
        number(left, "Start")?,
        number(left, "Duration")?,
        number(right, "Start")?,
        number(right, "In")?,
    ) else {
        return Ok(None);
    };
    if start < grid.origin
        || left_length < before
        || incoming_in < before
        || start.checked_add(before) != Some(cut)
        || left_start.checked_add(left_length) != Some(cut)
    {
        return Ok(None);
    }
    let Some(relative) = cut.checked_sub(grid.origin) else {
        return Ok(None);
    };
    Ok(Some((relative, left.id()?, right.id()?, before)))
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

pub(super) fn constant_picture_speed(
    encoded: &str,
    available: i64,
    blobs: &mut encoding::Blobs,
) -> Result<Option<crate::retime::PictureSpeed>> {
    use crate::retime::PictureSpeed;
    if !encoded.starts_with("00000001") {
        return Ok(None);
    }
    let map = blobs.qt(encoded)?;
    if map.keys().count() != 7
        || map.text("DbType").ok().flatten() != Some("Sm2TimeMap")
        || map.text("UniqueId").ok().flatten().and_then(|id| uuid::Uuid::parse_str(id).ok()).is_none()
        || map.real("YMin").ok() != Some(-1.0)
        || map.real("YMax").ok() != Some(-1.0)
    {
        return Ok(None);
    }
    let observed = (|| {
        let payload = map.bytes("KeyframesBA").ok()?.strip_prefix(&[0x80])?;
        use encoding::Wire;
        let fields = encoding::fields(payload).ok()?;
        let points = Value::Obj(map_keys(payload)?);
        let Value::Arr(points) = points.get("keyframes")? else { return None };
        let get = |point: &Value, key| match point.get(key)? {
            Value::Float(value) => Some(*value),
            _ => None,
        };
        let (first_source, last) = match (fields.as_slice(), points.as_slice()) {
            ([(1, Wire::Bytes(_)), (1, Wire::Bytes(_))], [first, last])
                if get(first, "record_seconds")? == 0.0 && first.get("saved_handles").is_none() =>
            {
                (get(first, "source_seconds")?, last)
            }
            _ => return None,
        };
        if last.get("saved_handles").is_some() {
            return None;
        }
        Some([
            first_source,
            get(last, "record_seconds")?,
            get(last, "source_seconds")?,
            map.real("LastValidYOffset").ok()?,
            map.real("XMax").ok()?,
        ])
    })();
    Ok(observed.and_then(|observed| {
        [PictureSpeed::Half, PictureSpeed::Reverse].into_iter().find(|mode| {
            mode.coordinates(available).is_some_and(|(first, last_record, last_source, source_end)| {
                [first, last_record, last_source, source_end, last_record]
                    .into_iter()
                    .zip(observed)
                    .all(|(expected, saved)| (expected - saved).abs() * 24.0 <= 1e-7)
            })
        })
    }))
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
        // Resolve's uncompressed envelope starts with 0x80. The documented
        // bare protobuf marker (field 160 = 0) starts with the same byte.
        if bytes.starts_with(&[0x80, 0x0a, 0x00, 0x0a]) {
            bytes
        } else {
            bytes.strip_prefix(&[0x80]).unwrap_or(bytes)
        }
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
