//! Media Composer bin structure for the qualified managed master.
use super::{BoundFile, Essence, Result, RATE, SAMPLE_RATE};
use crate::avid_media::avb::canonical_descriptor;
use avid_reader::{
    document::{AvbDocument, MobKind},
    io::{MobId, Uuid},
    objects::{Node, Value},
    primitives_extra::{new_unique_mob_id, AvbTimestamp, UtcDateTime},
};
use std::time::{SystemTime, UNIX_EPOCH};

const EDIT_RATE: f64 = RATE as f64;
const PHYSICAL_SOURCE_EXTENT: u32 = 5 * 24 * 60 * 60 * RATE as u32;

pub(super) fn build(
    name: &str,
    volume: &str,
    start: u32,
    frames: u32,
    files: &[BoundFile; 3],
) -> Result<AvbDocument> {
    let timestamp = u32::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let bin_uid = u64::from_be_bytes(new_unique_mob_id()?.material.data4);
    let master_uid = new_unique_mob_id()?;
    let UtcDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    } = AvbTimestamp(timestamp).to_utc();
    let saved_at = format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}:{second:02}");
    let mut doc = AvbDocument::new_empty_bin(bin_uid, saved_at, timestamp, false);
    let mut tracks = Vec::with_capacity(4);
    let null_id = MobId::from_pyavb_bytes_le(&[0; 32])?;
    for file in files {
        let kind = file.track_kind();
        let upstream_descriptor = doc.add_object(Node {
            class: *b"MDES",
            type_name: "MediaDescriptor",
            fields: base_descriptor(5, Value::Null).into(),
        })?;
        let origin =
            doc.create_source_clip(EDIT_RATE, kind, PHYSICAL_SOURCE_EXTENT, null_id, 0, 0)?;
        let tape_clock = doc.create_timecode(EDIT_RATE, PHYSICAL_SOURCE_EXTENT, RATE as u16, 0)?;
        let tape = composition(
            &mut doc,
            "",
            MobKind::Source,
            file.upstream,
            PHYSICAL_SOURCE_EXTENT,
            timestamp,
        )?;
        doc.set_field(tape, "descriptor", Value::Ref(upstream_descriptor))?;
        doc.append_track(tape, 1, origin)?;
        doc.append_track(tape, 1, tape_clock)?;
        doc.add_bin_source(tape)?;

        let locator = doc.add_object(Node {
            class: *b"MSML",
            type_name: "MSMLocator",
            fields: vec![
                ("last_known_volume", Value::Str(volume.to_owned())),
                ("domain_type", Value::Int(4)),
                ("mob_id", Value::MobId(file.package)),
                ("last_known_volume_utf8", Value::Str(volume.to_owned())),
            ],
        })?;
        let descriptor = doc.add_object(file_descriptor(file, locator)?)?;
        let source_clip = doc.create_source_clip(
            EDIT_RATE,
            kind,
            frames,
            file.upstream,
            1,
            i32::try_from(start)?,
        )?;
        let source_clock = doc.create_timecode(EDIT_RATE, frames, RATE as u16, start)?;
        let source = composition(
            &mut doc,
            "",
            MobKind::Source,
            file.package,
            frames,
            timestamp,
        )?;
        doc.set_field(source, "descriptor", Value::Ref(descriptor))?;
        doc.append_track(source, 1, source_clip)?;
        doc.append_track(source, 1, source_clock)?;
        doc.add_bin_source(source)?;

        let clip = doc.create_source_clip(EDIT_RATE, kind, frames, file.package, 1, 0)?;
        let head = doc.create_filler(EDIT_RATE, kind, 0)?;
        let tail = doc.create_filler(EDIT_RATE, kind, 0)?;
        let segment = doc.create_sequence(EDIT_RATE, kind, &[head, clip, tail])?;
        tracks.push((file.lane, segment));
    }
    tracks.push((
        1,
        doc.create_timecode(EDIT_RATE, frames, RATE as u16, start)?,
    ));
    let master = composition(
        &mut doc,
        name,
        MobKind::Master,
        master_uid,
        frames,
        timestamp,
    )?;
    for (lane, segment) in tracks {
        doc.append_track(master, lane, segment)?;
    }
    doc.add_bin_item(master)?;
    doc.finish_bin_copy()?;
    let decoded = AvbDocument::from_bytes(doc.to_bytes()?)?;
    check_written(&decoded, master, start, frames, files)?;
    Ok(decoded)
}

fn composition(
    doc: &mut AvbDocument,
    name: &str,
    kind: MobKind,
    id: MobId,
    length: u32,
    timestamp: u32,
) -> Result<u32> {
    let mob = doc.create_composition(
        if name.is_empty() { "Source" } else { name },
        kind,
        id,
        length,
        timestamp,
    )?;
    if name.is_empty() {
        doc.set_field(mob, "name", Value::Null)?;
    }
    doc.set_field(mob, "edit_rate", Value::Float(EDIT_RATE))?;
    Ok(mob)
}

fn base_descriptor(kind: i64, locator: Value) -> [(&'static str, Value); 4] {
    [
        ("mob_kind", Value::Int(kind)),
        ("locator", locator),
        ("intermediate", Value::Bool(false)),
        ("physical_media", Value::Null),
    ]
}

fn file_descriptor(file: &BoundFile, locator: u32) -> Result<Node> {
    let mut fields = Vec::from(base_descriptor(1, Value::Ref(locator)));
    fields.extend([
        ("is_omfi", Value::Int(0)),
        ("data_offset", Value::Int(file.offset)),
    ]);
    let (class, type_name) = match file.kind {
        Essence::Video { coding } => {
            let uuid = Uuid {
                data1: u32::from_be_bytes(coding[8..12].try_into().unwrap()),
                data2: u16::from_be_bytes(coding[12..14].try_into().unwrap()),
                data3: u16::from_be_bytes(coding[14..16].try_into().unwrap()),
                data4: coding[..8].try_into().unwrap(),
            };
            fields.extend([
                ("edit_rate", Value::Float(EDIT_RATE)),
                (
                    "aspect_ratio",
                    Value::List(vec![Value::Int(16), Value::Int(9)]),
                ),
                ("line_map", Value::List(vec![Value::Int(0), Value::Int(0)])),
                ("compress_method", Value::Bytes(b"RI00".to_vec())),
                ("essence_compression", Value::Uuid(uuid)),
                ("uniformness", Value::Bool(false)),
                ("next_did_desc", Value::Null),
                ("frame_checked_with_mapper", Value::Bool(false)),
            ]);
            fields.extend(
                [
                    ("length", 0),
                    ("stored_height", 1088),
                    ("stored_width", 1920),
                    ("sampled_height", 1080),
                    ("sampled_width", 1920),
                    ("sampled_x_offset", 0),
                    ("sampled_y_offset", 0),
                    ("display_height", 1080),
                    ("display_width", 1920),
                    ("display_x_offset", 0),
                    ("display_y_offset", 0),
                    ("frame_layout", 0),
                    ("alpha_transparency", 0),
                    ("did_image_size", 0),
                    ("resolution_id", 0),
                    ("image_alignment_factor", 0),
                    ("frame_index_byte_order", 0x4949),
                    ("frame_sample_size", 0),
                    ("first_frame_offset", 0),
                    ("client_fill_start", 0),
                    ("client_fill_end", 0),
                    ("offset_to_rle_frame_index", -1),
                    ("horizontal_subsampling", 2),
                    ("vertical_subsampling", 1),
                    ("component_width", 8),
                    ("color_sitting", 0),
                    ("black_ref_level", 16),
                    ("white_ref_level", 235),
                    ("color_range", 225),
                    ("frame_index_offset", 0),
                    ("alpha_sampled_width", 0),
                    ("ignore_bw", 0),
                ]
                .into_iter()
                .map(|(name, value)| (name, Value::Int(value))),
            );
            (*b"CDCI", "CDCIDescriptor")
        }
        Essence::Sound { samples } => {
            fields.extend([
                ("edit_rate", Value::Float(SAMPLE_RATE as f64)),
                ("sample_rate", Value::Float(SAMPLE_RATE as f64)),
                ("locked", Value::Bool(true)),
                ("has_peak_envelope_data", Value::Bool(false)),
                ("peak_of_peaks_offset", Value::UInt(u64::MAX)),
            ]);
            fields.extend(
                [
                    ("length", samples),
                    ("channels", 1),
                    ("quantization_bits", 24),
                    ("audio_ref_level", -20),
                    ("electro_spatial_formulation", 2),
                    ("dial_norm", 27),
                    ("coding_format", 1),
                    ("block_align", 3),
                    ("sequence_offset", 0),
                    ("average_bps", SAMPLE_RATE * 3),
                    ("peak_envelope_version", 0),
                    ("peak_envelope_format", 0),
                    ("points_per_peak_value", 0),
                    ("peak_envelope_block_size", 0),
                    ("peak_channel_count", 0),
                    ("peak_frame_count", 0),
                    ("peak_envelope_timestamp", 0),
                ]
                .into_iter()
                .map(|(name, value)| (name, Value::Int(value))),
            );
            (*b"PCMA", "PCMADescriptor")
        }
    };
    let descriptor = Node {
        class,
        type_name,
        fields,
    };
    canonical_descriptor(
        &descriptor,
        false,
        locator,
        "missing saved managed descriptor",
        "managed descriptor changed during encoding",
    )
}

fn check_written(
    doc: &AvbDocument,
    master_id: u32,
    start: u32,
    frames: u32,
    files: &[BoundFile; 3],
) -> Result<()> {
    let err = "managed master failed serialized AVB read-back";
    let node = |id| doc.node(id).ok_or(err);
    let reference = |value: Option<&Value>| value.and_then(Value::as_index).ok_or(err);
    let Value::List(items) = node(doc.root_id())?.get("items").ok_or(err)? else {
        return Err(err.into());
    };
    let mut mobs = std::collections::HashMap::new();
    for item in items {
        let Value::Node(item) = item else {
            return Err(err.into());
        };
        let id = reference(item.get("mob"))?;
        let mob = node(id)?;
        let Some(Value::MobId(uid)) = mob.get("mob_id") else {
            return Err(err.into());
        };
        if item.get("user_placed") != Some(&Value::Bool(id == master_id))
            || mobs.insert(*uid, mob).is_some()
        {
            return Err(err.into());
        }
    }
    let master = node(master_id)?;
    let Some(Value::List(tracks)) = master.get("tracks") else {
        return Err(err.into());
    };
    if mobs.len() != 7
        || tracks.len() != 4
        || master.get("length") != Some(&Value::Int(frames.into()))
    {
        return Err(err.into());
    }
    for (track, file) in tracks.iter().zip(files) {
        let Value::Node(track) = track else {
            return Err(err.into());
        };
        let seq = node(reference(track.get("component"))?)?;
        let Some(Value::RefList(components)) = seq.get("components") else {
            return Err(err.into());
        };
        let [_, clip, _] = components.as_slice() else {
            return Err(err.into());
        };
        let clip = node(reference(Some(clip))?)?;
        if track.get("index") != Some(&Value::Int(file.lane.into()))
            || clip.get("media_kind_id") != Some(&Value::Int(file.track_kind()))
            || clip.get("mob_id") != Some(&Value::MobId(file.package))
            || clip.get("track_id") != Some(&Value::Int(1))
            || clip.get("start_time") != Some(&Value::Int(0))
            || clip.get("length") != Some(&Value::Int(frames.into()))
            || !mobs.contains_key(&file.upstream)
        {
            return Err(err.into());
        }
        let source = mobs.get(&file.package).ok_or(err)?;
        let descriptor = node(reference(source.get("descriptor"))?)?;
        let locator = node(reference(descriptor.get("locator"))?)?;
        if locator.get("mob_id") != Some(&Value::MobId(file.package))
            || locator.get("domain_type") != Some(&Value::Int(4))
        {
            return Err(err.into());
        }
    }
    let Value::Node(clock_track) = &tracks[3] else {
        return Err(err.into());
    };
    let clock = node(reference(clock_track.get("component"))?)?;
    if clock.class != *b"TCCP"
        || clock.get("start") != Some(&Value::Int(start.into()))
        || clock.get("fps") != Some(&Value::Int(RATE))
    {
        return Err(err.into());
    }
    Ok(())
}
