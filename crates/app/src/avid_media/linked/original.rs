//! Generate the verified macOS UME MOV and WaveAiff source-record layouts.
use super::{markers, native_bin, EditRate, Plan, Result, Route, RASTER, TIMECODE_BASE};
use crate::avid_media::avb::canonical_descriptor;
use avid_reader::{
    document::{AvbDocument, MobKind},
    document_extra::{create_file_locator, create_msm_locator, FileLocatorKind, FileLocatorPaths},
    io::{MobId, Uuid},
    objects::{Node, Value},
    primitives_extra::{new_unique_mob_id, AvbTimestamp},
};
use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const SAMPLE_RATE: i64 = 48_000;
const WAV_PROVIDER: Uuid = Uuid {
    data1: 0x3711d3cc,
    data2: 0x62d0,
    data3: 0x49d7,
    data4: [0xb0, 0xae, 0xc1, 0x18, 0x10, 0x1d, 0x1a, 0x16],
};
pub(super) const UME_PROVIDER: Uuid = Uuid {
    data1: 0x87a0584d,
    data2: 0xcafa,
    data3: 0x41f5,
    data4: [0x9a, 0x68, 0xf0, 0xca, 0xde, 0xfd, 0xbb, 0x71],
};
pub(super) const TERMINAL_SOURCE: MobId = MobId {
    label: [6, 10, 43, 52, 1, 1, 1, 1, 1, 1, 15, 0],
    length: 0x13,
    instance: [0; 3],
    material: Uuid {
        data1: 0x060e2b34,
        data2: 0x7f7f,
        data3: 0x2a80,
        data4: [0; 8],
    },
};

pub(super) fn new_bin() -> Result<(AvbDocument, u32)> {
    let timestamp = unix_seconds(SystemTime::now())?;
    let saved = AvbTimestamp(timestamp).to_utc();
    let saved_at = format!(
        "{:04}/{:02}/{:02} {:02}:{:02}:{:02}",
        saved.year, saved.month, saved.day, saved.hour, saved.minute, saved.second
    );
    let uid = u64::from_be_bytes(new_unique_mob_id()?.material.data4);
    Ok((
        AvbDocument::new_empty_bin(uid, saved_at, timestamp, false),
        timestamp,
    ))
}

pub(super) fn build(plan: &Plan<'_>, avc: Option<&super::mp4::Encoding>) -> Result<AvbDocument> {
    let (mut doc, timestamp) = new_bin()?;
    if let Some(avc) = avc {
        let masters =
            super::mp4::source_chain(&mut doc, &plan.sources[0], plan.rate, avc, timestamp)?;
        native_bin::write_timeline(&mut doc, plan, &[masters], timestamp)?;
        return Ok(doc);
    }
    let video = source_chain(
        &mut doc,
        &plan.sources[0].routes[0],
        plan.rate,
        timestamp,
        true,
        &plan.sources[0].picture_markers,
    )?;
    let audio = source_chain(
        &mut doc,
        &plan.sources[0].routes[1],
        plan.rate,
        timestamp,
        false,
        &[],
    )?;
    native_bin::write_timeline(&mut doc, plan, &[[video, audio, audio]], timestamp)?;
    Ok(doc)
}

pub(super) fn unix_seconds(time: SystemTime) -> Result<u32> {
    Ok(time.duration_since(UNIX_EPOCH)?.as_secs().try_into()?)
}

fn source_chain(
    doc: &mut AvbDocument,
    route: &Route<'_>,
    rate: EditRate,
    timestamp: u32,
    picture: bool,
    marker_plan: &[markers::Marker<'_>],
) -> Result<MobId> {
    let edit_rate = rate.saved();
    let path = Path::new(route.path);
    let volume = path
        .strip_prefix("/Volumes")
        .ok()
        .and_then(|relative| relative.components().next())
        .and_then(|first| first.as_os_str().to_str())
        .ok_or("original link generation uses mounted macOS media volumes")?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid original filename")?;
    let modified = unix_seconds(fs::metadata(path)?.modified()?)?;
    let kind = if picture { 1 } else { 2 };
    let lanes = if picture { 1 } else { 2 };
    let uuid = if picture { UME_PROVIDER } else { WAV_PROVIDER };
    let mut master_id = new_unique_mob_id()?;
    master_id.label = TERMINAL_SOURCE.label;
    let (file_id, physical_id) = if picture {
        master_id.label[7] = 0;
        let mut file = new_unique_mob_id()?;
        let mut physical = new_unique_mob_id()?;
        file.label = master_id.label;
        physical.label = master_id.label;
        (file, physical)
    } else {
        // WaveAiff stores master, file and physical roles under one material ID.
        let file = MobId {
            instance: [0xb1, 0, 0],
            ..master_id
        };
        let physical = MobId {
            instance: [0xa1, 0, 0],
            ..master_id
        };
        (file, physical)
    };

    let original = create_file_locator(
        doc,
        FileLocatorKind::Mac,
        FileLocatorPaths {
            path: "",
            path_posix: Some(route.path),
            path_utf8: Some(route.path),
            path2_utf8: None,
        },
    )?;
    let upstream = physical_descriptor(doc, original, uuid, 5)?;
    let embedded = physical_descriptor(doc, original, uuid, 1)?;
    let file_locator = create_msm_locator(doc, volume, file_id, Some(0), Some(volume))?;

    let samples = if picture {
        0
    } else {
        i64::from(route.extent) * rate.samples_per_frame()
    };
    let audio_headers = || {
        [
            (
                "_SAVED_AAF_AUDIO_RATE_NUM",
                Value::Int(if picture { 0 } else { SAMPLE_RATE }),
            ),
            ("_SAVED_AAF_AUDIO_RATE_DEN", Value::Int(1)),
        ]
    };
    let physical = composition(
        doc,
        filename,
        MobKind::Source,
        physical_id,
        route.extent,
        edit_rate,
        timestamp,
    )?;
    doc.set_field(physical, "descriptor", Value::Ref(upstream))?;
    set_attrs(
        doc,
        physical,
        audio_headers()
            .into_iter()
            .chain([("_SAVED_AAF_AUDIO_LENGTH", Value::Int(0))]),
    )?;
    let clock = doc.create_timecode(edit_rate, route.extent, TIMECODE_BASE as u16, route.clock)?;
    let clock = bracket(doc, edit_rate, 3, clock)?;
    set_attrs(doc, clock, [("_SAVED_AAF_SLOTID", Value::Int(1))])?;
    doc.append_track(physical, 1, clock)?;
    for lane in 1..=lanes {
        let clip = doc.create_source_clip(edit_rate, kind, route.extent, TERMINAL_SOURCE, 0, 0)?;
        let component = bracket(doc, edit_rate, kind, clip)?;
        set_attrs(
            doc,
            component,
            [
                ("_SAVED_AAF_SLOTID", Value::Int(i64::from(lane) + 1)),
                (
                    "_SAVED_AAF_PHYSICAL_TRACK_NUMBER",
                    Value::Int(i64::from(lane)),
                ),
            ],
        )?;
        doc.append_track(physical, lane, component)?;
    }
    doc.add_bin_source(physical)?;

    let desc = if picture {
        picture_descriptor(doc, file_locator, embedded, route.extent, edit_rate)?
    } else {
        audio_descriptor(
            doc,
            file_locator,
            embedded,
            samples,
            route.bits.ok_or("missing WAV bit depth")?,
        )?
    };
    let file = composition(
        doc,
        filename,
        MobKind::Source,
        file_id,
        route.extent,
        edit_rate,
        timestamp,
    )?;
    doc.set_field(file, "descriptor", Value::Ref(desc))?;
    set_attrs(
        doc,
        file,
        audio_headers().into_iter().chain([
            ("_SAVED_AAF_AUDIO_LENGTH", Value::Int(samples)),
            ("_AMA_FILE_DATE_TIME", Value::Int(i64::from(modified))),
        ]),
    )?;
    for lane in 1..=lanes {
        let clip = doc.create_source_clip(edit_rate, kind, route.extent, physical_id, lane, 0)?;
        let component = bracket(doc, edit_rate, kind, clip)?;
        let mut attributes = vec![
            ("_SAVED_AAF_SLOTID", Value::Int(if picture { 1 } else { 2 })),
            ("_SAVED_AAF_PHYSICAL_TRACK_NUMBER", Value::Int(1)),
        ];
        if !picture {
            attributes.push(("_SAVED_AAF_MONOSLOTID", Value::Int(i64::from(lane) + 1)));
        }
        set_attrs(doc, component, attributes)?;
        doc.append_track(file, lane, component)?;
    }
    doc.add_bin_source(file)?;

    let marker_list = markers::write(doc, marker_plan)?;
    let master = composition(
        doc,
        route.name,
        MobKind::Master,
        master_id,
        route.extent,
        edit_rate,
        timestamp,
    )?;
    let user = attrs(
        doc,
        route
            .tape
            .map(|tape| ("TapeID", Value::Str(tape.to_owned()))),
    )?;
    let settings = attrs(
        doc,
        [
            ("_IGNOREALPHA", Value::Int(0)),
            ("_INVERTALPHA", Value::Int(1)),
        ],
    )?;
    set_attrs(
        doc,
        master,
        audio_headers().into_iter().chain([
            ("_VERSION", Value::Int(2)),
            ("_USER", Value::Ref(user)),
            ("_IMPORTSETTING", Value::Ref(settings)),
            ("_IMPORTED_AMA", Value::Int(1)),
            ("_SPANNEDUSINGFILLER", Value::Int(1)),
            ("_SAVED_AAF_AUDIO_LENGTH", Value::Int(samples)),
        ]),
    )?;
    let master_attrs = doc
        .node(master)
        .and_then(|node| node.get("attributes"))
        .and_then(Value::as_index)
        .ok_or("new master lacks attributes")?;
    markers::attach(doc, master_attrs, marker_list)?;
    for lane in 1..=lanes {
        let clip = doc.create_source_clip(edit_rate, kind, route.extent, file_id, lane, 0)?;
        let component = bracket(doc, edit_rate, kind, clip)?;
        if picture {
            set_attrs(doc, component, [("_SAVED_AAF_SLOTID", Value::Int(1))])?;
        } else {
            let lane = i64::from(lane);
            set_attrs(
                doc,
                component,
                [
                    ("_SAVED_AAF_SOURCESLOTID", Value::Int(2)),
                    ("_SAVED_AAF_CHANNELID", Value::Int(lane)),
                    ("_SAVED_AAF_SLOTID", Value::Int(lane + 1)),
                    ("_SAVED_AAF_PHYSICAL_TRACK_NUMBER", Value::Int(lane)),
                ],
            )?;
        }
        doc.append_track(master, lane, component)?;
    }
    doc.add_bin_item(master)?;
    Ok(master_id)
}

pub(super) fn composition(
    doc: &mut AvbDocument,
    name: &str,
    kind: MobKind,
    id: MobId,
    length: u32,
    edit_rate: f64,
    timestamp: u32,
) -> Result<u32> {
    let object = doc.create_composition(name, kind, id, length, timestamp)?;
    doc.set_field(object, "edit_rate", Value::Float(edit_rate))?;
    Ok(object)
}

pub(super) fn bracket(doc: &mut AvbDocument, edit_rate: f64, kind: i64, clip: u32) -> Result<u32> {
    let head = doc.create_filler(edit_rate, kind, 0)?;
    let tail = doc.create_filler(edit_rate, kind, 0)?;
    doc.create_sequence(edit_rate, kind, &[head, clip, tail])
        .map_err(Into::into)
}

fn entries<'a>(fields: impl IntoIterator<Item = (&'a str, Value)>) -> Value {
    Value::Attrs(
        fields
            .into_iter()
            .map(|(key, val)| (key.to_owned(), val))
            .collect(),
    )
}

pub(super) fn attrs<'a>(
    doc: &mut AvbDocument,
    fields: impl IntoIterator<Item = (&'a str, Value)>,
) -> Result<u32> {
    Ok(doc.add_object(Node {
        class: *b"ATTR",
        type_name: "Attributes",
        fields: vec![("entries", entries(fields))],
    })?)
}

pub(super) fn set_attrs<'a>(
    doc: &mut AvbDocument,
    owner: u32,
    fields: impl IntoIterator<Item = (&'a str, Value)>,
) -> Result<()> {
    let attributes = doc
        .node(owner)
        .and_then(|node| node.get("attributes"))
        .and_then(Value::as_index)
        .ok_or("new native record lacks attributes")?;
    Ok(doc.set_field(attributes, "entries", entries(fields))?)
}

pub(super) fn common_descriptor(
    kind: i64,
    locator: u32,
    physical: Value,
    uuid: Uuid,
) -> Vec<(&'static str, Value)> {
    vec![
        ("mob_kind", Value::Int(kind)),
        ("locator", Value::Ref(locator)),
        ("intermediate", Value::Bool(false)),
        ("physical_media", physical),
        ("uuid", Value::Uuid(uuid)),
    ]
}

pub(super) fn physical_descriptor(
    doc: &mut AvbDocument,
    locator: u32,
    uuid: Uuid,
    kind: i64,
) -> Result<u32> {
    add_descriptor(
        doc,
        *b"MDES",
        "MediaDescriptor",
        common_descriptor(kind, locator, Value::Null, uuid),
    )
}

fn picture_descriptor(
    doc: &mut AvbDocument,
    locator: u32,
    physical: u32,
    frames: u32,
    edit_rate: f64,
) -> Result<u32> {
    let handler = attrs(
        doc,
        [(
            "_CONTAINER_HANDLER_GUID",
            Value::Bytes(vec![
                0x73, 0xdc, 0x06, 0x3c, 0x76, 0x02, 0x4c, 0x4c, 0xba, 0x3f, 0x3f, 0x47, 0x12, 0x0c,
                0xd1, 0xe9,
            ]),
        )],
    )?;
    picture_fields(doc, locator, physical, frames, edit_rate, handler, None)
}

pub(super) fn picture_fields(
    doc: &mut AvbDocument,
    locator: u32,
    physical: u32,
    frames: u32,
    edit_rate: f64,
    handler: u32,
    avc_transfer: Option<Uuid>,
) -> Result<u32> {
    let avc = avc_transfer.is_some();
    let (width, height) = RASTER;
    let mut fields = common_descriptor(1, locator, Value::Ref(physical), UME_PROVIDER);
    fields.extend([
        ("attributes", Value::Ref(handler)),
        ("edit_rate", Value::Float(edit_rate)),
        ("aspect_ratio", pair(16, 9)),
        ("line_map", pair(0, 0)),
        ("uniformness", Value::Bool(false)),
        ("next_did_desc", Value::Null),
        ("compress_method", Value::Bytes(b"RI00".to_vec())),
        ("frame_checked_with_mapper", Value::Bool(false)),
    ]);
    fields.push((
        "essence_compression",
        Value::Uuid(if avc {
            super::mp4::AVC_COMPRESSION
        } else {
            Uuid {
                data1: 0x04010202,
                data2: 0x0306,
                data3: 0x0400,
                data4: [6, 14, 43, 52, 4, 1, 1, 13],
            }
        }),
    ));
    if let Some(transfer) = avc_transfer {
        fields.extend([
            ("transfer_characteristic", Value::Uuid(transfer)),
            ("color_primaries", Value::Uuid(super::mp4::REC709_PRIMARIES)),
            ("coding_equations", Value::Uuid(super::mp4::REC709_MATRIX)),
        ]);
    }
    integers(
        &mut fields,
        [
            ("length", frames.into()),
            ("is_omfi", 0),
            ("data_offset", 0),
            (
                "stored_height",
                if avc { (height + 15) / 16 * 16 } else { height },
            ),
            ("stored_width", width),
            ("sampled_height", height),
            ("sampled_width", width),
            ("sampled_x_offset", 0),
            ("sampled_y_offset", 0),
            ("display_height", height),
            ("display_width", width),
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
            ("vertical_subsampling", if avc { 2 } else { 1 }),
            ("component_width", if avc { 8 } else { 10 }),
            ("color_sitting", 0),
            ("black_ref_level", if avc { 16 } else { 64 }),
            ("white_ref_level", if avc { 235 } else { 940 }),
            ("color_range", if avc { 220 } else { 877 }),
            ("frame_index_offset", 0),
            ("alpha_sampled_width", 0),
            ("ignore_bw", 0),
        ],
    );
    add_descriptor(doc, *b"CDCI", "CDCIDescriptor", fields)
}

fn audio_descriptor(
    doc: &mut AvbDocument,
    locator: u32,
    physical: u32,
    samples: i64,
    bits: i64,
) -> Result<u32> {
    let source_length = attrs(
        doc,
        [(
            "_AMD_SOURCE_LENGTH",
            Value::Bytes(samples.to_le_bytes().to_vec()),
        )],
    )?;
    let mut provider: Vec<_> =
        "[Avid Technology, Inc.][WaveAiff Plug-In][1.9.3.11080][WaveAiffReaderMovMediaContainer]"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
    provider.resize(512, 0);
    let mut fields = common_descriptor(1, locator, Value::Ref(physical), WAV_PROVIDER);
    fields.extend([
        ("wchar", Value::Bytes(provider)),
        ("attributes", Value::Ref(source_length)),
        ("edit_rate", Value::Float(SAMPLE_RATE as f64)),
        ("sample_rate", Value::Float(SAMPLE_RATE as f64)),
        ("locked", Value::Bool(false)),
        ("has_peak_envelope_data", Value::Bool(false)),
        ("peak_of_peaks_offset", Value::UInt(u64::MAX)),
    ]);
    let block = bits / 8 * 2;
    integers(
        &mut fields,
        [
            ("length", samples),
            ("is_omfi", 0),
            ("data_offset", 0),
            ("channels", 2),
            ("quantization_bits", bits),
            ("audio_ref_level", -20),
            ("electro_spatial_formulation", 2),
            ("dial_norm", 27),
            ("coding_format", 1),
            ("block_align", block),
            ("sequence_offset", 0),
            ("average_bps", SAMPLE_RATE * block),
            ("peak_envelope_version", 0),
            ("peak_envelope_format", 0),
            ("points_per_peak_value", 0),
            ("peak_envelope_block_size", 0),
            ("peak_channel_count", 0),
            ("peak_frame_count", 0),
            ("peak_envelope_timestamp", 0),
        ],
    );
    add_descriptor(doc, *b"PCMA", "PCMADescriptor", fields)
}

fn pair(a: i64, b: i64) -> Value {
    Value::List(vec![Value::Int(a), Value::Int(b)])
}

fn integers(
    fields: &mut Vec<(&'static str, Value)>,
    values: impl IntoIterator<Item = (&'static str, i64)>,
) {
    fields.extend(
        values
            .into_iter()
            .map(|(name, value)| (name, Value::Int(value))),
    );
}

pub(super) fn add_descriptor(
    doc: &mut AvbDocument,
    class: [u8; 4],
    type_name: &'static str,
    fields: Vec<(&'static str, Value)>,
) -> Result<u32> {
    let original = Node {
        class,
        type_name,
        fields,
    };
    let saved = canonical_descriptor(
        &original,
        doc.big_endian(),
        u32::try_from(doc.object_count())?,
        "missing encoded original descriptor",
        "original descriptor changed during encoding",
    )?;
    Ok(doc.add_object(saved)?)
}
