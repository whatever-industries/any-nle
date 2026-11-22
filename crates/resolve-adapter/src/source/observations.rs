//! Bounded source facts whose destination behavior is still unqualified.
use super::{encoding, omitted, seconds, text, timing::FrameField, Grid, Node};
use crate::{dissolve, Result};
use graph::{source::SourceGraph, Sidecar, StreamKind, Value};
use sha2::{Digest, Sha256};

const UNBOUND_PICTURE: &str = "resolve.unbound_picture";
const INLINE_OBSERVATION_BYTES: usize = 4096;

/// Retain a picture occurrence with saved effects and no Media Pool identity.
/// A locator on the edit is evidence only and establishes no source binding.
#[allow(clippy::too_many_arguments)]
pub(super) fn unbound_picture(
    graph: &mut SourceGraph,
    sequence: &str,
    node: &Node,
    track_id: &str,
    track: i64,
    grid: Grid,
    filters: &str,
    blobs: &mut encoding::Blobs,
) -> Result<()> {
    let id = node.id()?;
    let mut fields = vec![
        ("occurrence".into(), text(id)),
        ("track_id".into(), text(track_id)),
        ("track".into(), Value::Int(track)),
        ("record_rate".into(), text(&grid.rate.to_string())),
        ("media_ref".into(), Value::Null),
    ];
    for (tag, key) in [
        ("Name", "name"),
        ("PrettyType", "type_label"),
        ("Flags", "saved_flags"),
        ("Start", "saved_start"),
        ("Duration", "saved_duration"),
        ("In", "saved_in"),
        ("MediaFrameRate", "saved_media_rate"),
        ("MediaStartTime", "saved_media_start_time"),
        ("MediaFilePath", "saved_locator"),
        ("MediaReelNumber", "saved_reel"),
        ("CurrentSelectorIdx", "saved_selector_index"),
        ("IsForceConformed", "saved_force_conformed"),
        ("IsPreConformed", "saved_preconformed"),
    ] {
        if let Some(saved) = node.maybe(tag)? {
            fields.push((key.into(), text(saved)));
        }
    }
    let whole = |tag| -> Result<Option<i64>> {
        node.maybe(tag)?
            .map(|saved| FrameField::parse(saved).map(|field| field.integer()))
            .transpose()
            .map(Option::flatten)
    };
    if let (Some(start), Some(duration)) = (whole("Start")?, whole("Duration")?) {
        let start = grid.record_start(start, duration)?;
        fields.push(("record_start".into(), text(&seconds(start, grid.rate)?.to_string())));
        fields.push(("record_duration".into(), text(&seconds(duration, grid.rate)?.to_string())));
    }
    fields.push(("effect_filters".into(), saved_bytes(filters, blobs)?));
    for (tag, key) in [
        ("FieldsBlob", "fields_blob"),
        ("MarkersBA", "markers_blob"),
        ("ImportExportMetadataBA", "import_export_metadata"),
        ("PreConformMediaExtents", "preconform_media_extents"),
    ] {
        if let Some(saved) = node.maybe(tag)?.filter(|value| !value.is_empty()) {
            fields.push((key.into(), saved_bytes(saved, blobs)?));
        }
    }
    if let Some(map) = node.maybe("MediaTimemapBA")?.filter(|value| !value.is_empty()) {
        fields.push(("time_map".into(), time_map(map, blobs)?));
        if map.len() <= 2 * INLINE_OBSERVATION_BYTES {
            fields.push(("saved_time_map".into(), text(map)));
        }
    }
    append(graph, UNBOUND_PICTURE, sequence, Value::Obj(fields));
    omitted(graph, sequence, id, "media",
        "Resolve picture edit has saved effects and no MediaRef; source binding is unresolved");
    Ok(())
}

pub(super) fn saved_bytes(encoded: &str, blobs: &mut encoding::Blobs) -> Result<Value> {
    let bytes = encoding::hex_bytes(encoded)?;
    blobs.charge(bytes.len())?;
    let mut fields = vec![
        ("byte_count".into(), Value::Int(bytes.len() as i64)),
        ("sha256".into(), text(&hex::encode(Sha256::digest(&bytes)))),
    ];
    if bytes.len() <= INLINE_OBSERVATION_BYTES {
        fields.push(("saved_hex".into(), text(encoded)));
    }
    Ok(Value::Obj(fields))
}

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
                Some((_, _, _, interpretation)) => interpretation,
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
) -> Result<Option<(i64, &'a str, &'a str, &'static str)>> {
    let [Some(left), Some(right)] = neighbors else {
        return Ok(None);
    };
    let clip_class = if audio { "Sm2TiAudioClip" } else { "Sm2TiVideoClip" };
    if left.name != clip_class || right.name != clip_class {
        return Ok(None);
    }
    let Some(style) = node.maybe("PrettyType")?.and_then(dissolve::Style::from_label) else {
        return Ok(None);
    };
    let Some(duration) = node.maybe("Duration")?.and_then(|value| value.parse().ok()) else {
        return Ok(None);
    };
    if style.audio() != audio || (timeline_export && duration != dissolve::LEGACY_DURATION) {
        return Ok(None);
    }
    let Some(alignment) = node.maybe("AlignmentType")? else {
        return Ok(None);
    };
    let Some(before) = style.offset(duration, alignment) else {
        return Ok(None);
    };
    for (field, expected) in [
        ("Position", dissolve::POSITION),
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
    let effect_matches = if audio {
        effect.is_empty()
    } else {
        effect.len() == dissolve::EFFECT.len()
            && dissolve::video_effect(duration).is_ok_and(|expect| hex::encode(expect) == effect)
    };
    if !effect_matches
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
    Ok(Some((relative, left.id()?, right.id()?, style.interpretation(duration, alignment))))
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

pub(super) enum PictureTiming {
    Speed(crate::retime::PictureSpeed),
    Hold(i64),
    Linear(Vec<[i64; 2]>),
}

pub(super) fn picture_timing(
    encoded: &str,
    available: i64,
    source_rate: graph::Rational,
    record_rate: graph::Rational,
    blobs: &mut encoding::Blobs,
) -> Result<Option<PictureTiming>> {
    use crate::retime::{PictureSpeed, HOLD_MAP_END};
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
    let linear = (|| {
        let saved = map.bytes("KeyframesBA").ok()?;
        let payload = match saved {
            [0x80, bytes @ ..] => bytes.into(),
            [0x81, 0x28, 0xb5, 0x2f, 0xfd, ..] => keyframe_payload(saved)?,
            _ => return None,
        };
        use encoding::Wire;
        let fields = encoding::fields(&payload).ok()?;
        if !(3..=crate::retime::MAX_LINEAR_KEYS).contains(&fields.len())
            || fields.iter().any(|(tag, wire)| *tag != 1 || !matches!(wire, Wire::Bytes(_)))
        {
            return None;
        }
        let points = Value::Obj(map_key_fields(fields)?);
        let Value::Arr(points) = points.get("keyframes")? else { return None };
        let coordinate = |point: &Value, name, rate| match point.get(name)? {
            Value::Float(value) => encoding::frames(*value, rate).ok(),
            _ => None,
        };
        let decoded = points.iter().map(|point| {
            if point.get("saved_handles").is_some() {
                return None;
            }
            Some([coordinate(point, "record_seconds", record_rate)?, coordinate(point, "source_seconds", source_rate)?])
        }).collect::<Option<Vec<_>>>()?;
        let &[record_end, source_end] = decoded.last()?;
        if encoding::frames(map.real("XMax").ok()?, record_rate).ok()? != record_end
            || encoding::frames(map.real("LastValidYOffset").ok()?, source_rate).ok()? != source_end
        {
            return None;
        }
        crate::retime::LinearMap::validate_keys(&decoded, available).ok()?;
        Some(PictureTiming::Linear(decoded))
    })();
    if linear.is_some() || source_rate != record_rate || record_rate != graph::Rational::integer(24) {
        return Ok(linear);
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
        let [first, last_record, last_source, source_end, x_max] = observed;
        if last_record == HOLD_MAP_END && x_max == HOLD_MAP_END && first == last_source {
            let frame = encoding::frames(first, graph::Rational::integer(24)).ok()?;
            let last = encoding::frames(source_end, graph::Rational::integer(24)).ok()?;
            return ((0..available).contains(&frame) && last == available - 1)
                .then_some(PictureTiming::Hold(frame));
        }
        [PictureSpeed::Half, PictureSpeed::Double, PictureSpeed::Reverse].into_iter().find(|mode| {
            mode.coordinates(available).is_some_and(|(first, last_record, last_source, source_end)| {
                [first, last_record, last_source, source_end, last_record]
                    .into_iter()
                    .zip(observed)
                    .all(|(expected, saved)| (expected - saved).abs() * 24.0 <= 1e-7)
            })
        }).map(PictureTiming::Speed)
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

pub(super) fn still_hold(encoded: &str, blobs: &mut encoding::Blobs) -> Result<bool> {
    if !encoded.starts_with("00000001") {
        return Ok(false);
    }
    let map = blobs.qt(encoded)?;
    let end = crate::retime::HOLD_MAP_END;
    if map.keys().count() != 6
        || map.text("DbType")? != Some("Sm2TimeMap")
        || map.real("YMin")? != -1.0 || map.real("YMax")? != -1.0 || map.real("XMax")? != end
        || map.text("UniqueId")?.and_then(|id| uuid::Uuid::parse_str(id).ok()).is_none()
    {
        return Ok(false);
    }
    let payload = map.bytes("KeyframesBA")?;
    let keys = Value::Obj(map_keys(payload).unwrap_or_default());
    let Some(Value::Arr(points)) = keys.get("keyframes") else { return Ok(false) };
    let matches = |point: &Value, at: f64| {
        point.get("record_seconds") == Some(&Value::Float(at))
            && point.get("source_seconds") == Some(&Value::Float(0.0))
            && match point.get("saved_handles") {
                None => true,
                Some(Value::Arr(handles)) => handles.iter().all(|value| *value == Value::Float(0.0)),
                _ => false,
            }
    };
    Ok(keys.get("source_origin_seconds").is_none() && match points.as_slice() {
        // The witnessed compact encoding begins with the field-160 marker
        // and saves only the terminal key. Source zero remains implicit.
        [last] => payload.starts_with(&[0x80, 0x0a, 0x00, 0x0a]) && matches(last, end),
        [first, last] => matches(first, 0.0) && matches(last, end),
        _ => false,
    })
}

fn keyframe_payload(bytes: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    if bytes.starts_with(&[0x81, 0x28, 0xb5, 0x2f, 0xfd]) {
        return Some(zstd::bulk::decompress(&bytes[1..], 16 << 10).ok()?.into());
    }
    // Resolve's uncompressed envelope starts with 0x80. The documented
    // bare protobuf marker (field 160 = 0) starts with the same byte.
    Some(if bytes.starts_with(&[0x80, 0x0a, 0x00, 0x0a]) {
        bytes.into()
    } else {
        bytes.strip_prefix(&[0x80]).unwrap_or(bytes).into()
    })
}

fn map_keys(bytes: &[u8]) -> Option<Vec<(String, Value)>> {
    let payload = keyframe_payload(bytes)?;
    map_key_fields(encoding::fields(&payload).ok()?)
}

fn map_key_fields(fields: Vec<(u64, encoding::Wire<'_>)>) -> Option<Vec<(String, Value)>> {
    use encoding::Wire;
    let mut keys = Vec::new();
    let mut origin = None;
    for (id, wire) in fields {
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
    fn unbound_picture_blob_keeps_small_payloads_and_hashes_larger_ones() {
        let mut blobs = encoding::Blobs::default();
        let small = saved_bytes("A1b2", &mut blobs).unwrap();
        assert_eq!(small.get("saved_hex"), Some(&text("A1b2")));
        assert_eq!(small.get("byte_count"), Some(&Value::Int(2)));
        let bytes = vec![0xab; INLINE_OBSERVATION_BYTES + 1];
        let large = saved_bytes(&hex::encode(&bytes), &mut blobs).unwrap();
        assert_eq!(large.get("byte_count"), Some(&Value::Int(bytes.len() as i64)));
        assert_eq!(large.get("sha256"), Some(&text(&hex::encode(Sha256::digest(&bytes)))));
        assert!(large.get("saved_hex").is_none());
        assert!(saved_bytes("not_hex", &mut blobs).is_err());
    }

    #[test]
    fn linear_map_keys_round_trip_on_the_saved_movie_clock() {
        let mut blobs = encoding::Blobs::default();
        let id = "b27cdc8f-3d5a-4fe5-89b1-5226e9c4b506";
        let keys = vec![[0, 0], [12, 6], [24, 24], [47, 47]];
        let base = graph::Rational::integer(24);
        for (num, den) in [(24_000, 1001), (25, 1), (30_000, 1001)] {
            let rate = graph::Rational::new(num, den).unwrap();
            let map = crate::retime::LinearMap::new(6, keys.clone(), 48, rate, rate).unwrap();
            let saved = hex::encode(map.encode(id));
            assert!(matches!(picture_timing(&saved, 48, rate, rate, &mut blobs).unwrap(),
                Some(PictureTiming::Linear(decoded)) if decoded == keys));
            assert!(picture_timing(&saved, 48, base, base, &mut blobs).unwrap().is_none());
        }
        let source = crate::prepare::rate();
        let map = crate::retime::LinearMap::new(6, keys.clone(), 48, source, base).unwrap();
        let saved = hex::encode(map.encode(id));
        assert!(matches!(picture_timing(&saved, 48, source, base, &mut blobs).unwrap(),
            Some(PictureTiming::Linear(decoded)) if decoded == keys));
        assert!(picture_timing(&saved, 48, base, base, &mut blobs).unwrap().is_none());
        assert!(picture_timing(&saved, 48, source, source, &mut blobs).unwrap().is_none());

        // Resolve zstd-compresses the keyframes in some reopened saves.
        use crate::binary::{qt, Qt};
        let native = blobs.qt(&saved).unwrap();
        let raw = native.bytes("KeyframesBA").unwrap().strip_prefix(&[0x80]).unwrap();
        let mut compressed = vec![0x81];
        compressed.extend(zstd::bulk::compress(raw, 1).unwrap());
        let saved = hex::encode(qt(&[
            ("YMin", Qt::Double(-1.0)),
            ("YMax", Qt::Double(-1.0)),
            ("XMax", Qt::Double(native.real("XMax").unwrap())),
            ("UniqueId", Qt::Text(id)),
            ("LastValidYOffset", Qt::Double(native.real("LastValidYOffset").unwrap())),
            ("KeyframesBA", Qt::Bytes(&compressed)),
            ("DbType", Qt::Text("Sm2TimeMap")),
        ]));
        assert!(matches!(picture_timing(&saved, 48, source, base, &mut blobs).unwrap(),
            Some(PictureTiming::Linear(decoded)) if decoded == keys));
    }

    #[test]
    fn constant_hold_map_requires_exact_frame_and_original_extent() {
        let mut blobs = encoding::Blobs::default();
        let id = "b27cdc8f-3d5a-4fe5-89b1-5226e9c4b506";
        let rate = graph::Rational::integer(24);
        for frame in [0, 17, 95] {
            let saved = hex::encode(crate::retime::encode_hold(frame, 96, id).unwrap());
            assert!(matches!(
                picture_timing(&saved, 96, rate, rate, &mut blobs).unwrap(),
                Some(PictureTiming::Hold(selected)) if selected == frame
            ));
            assert!(picture_timing(&saved, 97, rate, rate, &mut blobs).unwrap().is_none());
        }
        assert!(crate::retime::encode_hold(96, 96, id).is_err());
    }

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
