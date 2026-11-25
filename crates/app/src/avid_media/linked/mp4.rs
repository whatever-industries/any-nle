//! Native UME source records for a timecoded 1080p AVC/AAC MP4.
use super::{
    markers, native_bin as saved, original, EditRate, Plan, Result, Route, SourceSet, TIMECODE_BASE,
};
use avid_reader::{
    document::{AvbDocument, MobKind},
    document_extra::{create_file_locator, create_msm_locator, FileLocatorKind, FileLocatorPaths},
    io::{MobId, Uuid},
    objects::{Node, Value},
    primitives_extra::new_unique_mob_id,
};
use std::{collections::HashMap, fs, path::Path};

const SAMPLE_RATE: i64 = 48_000;
const AAC_PRIMING: i64 = 1024;
const CONTAINER_GUID: [u8; 16] = [
    0x73, 0xdc, 0x06, 0x3c, 0x76, 0x02, 0x4c, 0x4c, 0xba, 0x3f, 0x3f, 0x47, 0x12, 0x0c, 0xd1, 0xe9,
];
const AAC_CODING: [u8; 16] = [6, 14, 43, 52, 4, 1, 1, 13, 4, 2, 2, 2, 4, 4, 1, 0];
pub(super) const AVC_COMPRESSION: Uuid = ul(0x04010202, 0x0131, 0x4001, 13);
const REC709_TRANSFER: Uuid = ul(0x04010101, 0x0102, 0, 1);
pub(super) const REC709_PRIMARIES: Uuid = ul(0x04010101, 0x0303, 0, 6);
pub(super) const REC709_MATRIX: Uuid = ul(0x04010101, 0x0202, 0, 1);

pub(super) struct Encoding {
    pub level: i64,
    pub audio_samples: i64,
}

const fn ul(data1: u32, data2: u16, data3: u16, version: u8) -> Uuid {
    Uuid {
        data1,
        data2,
        data3,
        data4: [6, 14, 43, 52, 4, 1, 1, version],
    }
}

fn id() -> Result<MobId> {
    let mut id = new_unique_mob_id()?;
    id.label = original::TERMINAL_SOURCE.label;
    id.label[7] = 0;
    Ok(id)
}

fn audio_metadata(sample_rate: i64, samples: i64) -> [(&'static str, Value); 3] {
    [
        ("_SAVED_AAF_AUDIO_RATE_NUM", Value::Int(sample_rate)),
        ("_SAVED_AAF_AUDIO_RATE_DEN", Value::Int(1)),
        ("_SAVED_AAF_AUDIO_LENGTH", Value::Int(samples)),
    ]
}

fn selector_attrs(slot: i64, physical: i64) -> [(&'static str, Value); 2] {
    [
        ("_SAVED_AAF_SLOTID", Value::Int(slot)),
        ("_SAVED_AAF_PHYSICAL_TRACK_NUMBER", Value::Int(physical)),
    ]
}

pub(super) fn source_chain(
    doc: &mut AvbDocument,
    source: &SourceSet<'_>,
    edit_rate: EditRate,
    avc: &Encoding,
    timestamp: u32,
) -> Result<[MobId; 3]> {
    let [picture, audio, _] = &source.routes;
    let path = Path::new(picture.path);
    let volume = path
        .strip_prefix("/Volumes")
        .ok()
        .and_then(|p| p.components().next())
        .and_then(|part| part.as_os_str().to_str())
        .ok_or("original link generation uses mounted macOS media volumes")?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid MP4 filename")?;
    let modified = original::unix_seconds(fs::metadata(path)?.modified()?)?;
    let rate = edit_rate.saved();
    let extent = picture.extent;
    let physical_extent = extent
        .checked_add(1)
        .ok_or("MP4 physical extent overflow")?;
    let physical_origin = picture
        .clock
        .checked_sub(1)
        .ok_or("MP4 timecode has no preroll")?;
    let packet_samples = avc
        .audio_samples
        .checked_add(AAC_PRIMING)
        .ok_or("AAC sample overflow")?;
    let provider = original::UME_PROVIDER;
    let original_locator = create_file_locator(
        doc,
        FileLocatorKind::Mac,
        FileLocatorPaths {
            path: "",
            path_posix: Some(picture.path),
            path_utf8: Some(picture.path),
            path2_utf8: None,
        },
    )?;

    // Native UME references one common physical package from both file packages.
    let physical_id = id()?;
    let physical = original::composition(
        doc,
        filename,
        MobKind::Source,
        physical_id,
        physical_extent,
        rate,
        timestamp,
    )?;
    let desc = original::physical_descriptor(doc, original_locator, provider, 5)?;
    doc.set_field(physical, "descriptor", Value::Ref(desc))?;
    original::set_attrs(
        doc,
        physical,
        audio_metadata(SAMPLE_RATE, 0)
            .into_iter()
            .chain([("_SAVED_AAF_PHYSICAL_TRACK_NUMBER", Value::Int(2))]),
    )?;
    let clock =
        doc.create_timecode(rate, physical_extent, TIMECODE_BASE as u16, physical_origin)?;
    let clock = original::bracket(doc, rate, 3, clock)?;
    original::set_attrs(doc, clock, [("_SAVED_AAF_SLOTID", Value::Int(1))])?;
    doc.append_track(physical, 1, clock)?;
    for (kind, lane, slot) in [(1, 1, 2), (2, 1, 3), (2, 2, 4)] {
        let length = if kind == 1 { physical_extent } else { extent };
        let clip = doc.create_source_clip(rate, kind, length, original::TERMINAL_SOURCE, 0, 0)?;
        let component = if kind == 1 {
            original::bracket(doc, rate, kind, clip)?
        } else {
            let head = doc.create_filler(rate, kind, 0)?;
            let tail = doc.create_filler(rate, kind, 1)?;
            doc.create_sequence(rate, kind, &[head, clip, tail])?
        };
        original::set_attrs(doc, component, selector_attrs(slot, i64::from(lane)))?;
        doc.append_track(physical, lane, component)?;
    }
    doc.add_bin_source(physical)?;

    let video_id = id()?;
    let audio_id = id()?;
    for (kind, file_id) in [(1, video_id), (2, audio_id)] {
        let picture = kind == 1;
        let file = original::composition(
            doc,
            filename,
            MobKind::Source,
            file_id,
            extent,
            rate,
            timestamp,
        )?;
        let locator = create_msm_locator(doc, volume, file_id, Some(0), Some(volume))?;
        let upstream = original::physical_descriptor(doc, original_locator, provider, 1)?;
        let desc = if picture {
            let avc_sub = original::attrs(
                doc,
                [
                    ("ATN_AVCProfileConstraint", Value::Int(0)),
                    ("ATN_AVCDecodingDelay", Value::Int(255)),
                    ("ATN_AVCLevel", Value::Int(avc.level)),
                    ("ATN_AVCProfile", Value::Int(100)),
                ],
            )?;
            let attrs = original::attrs(
                doc,
                [
                    (
                        "_CONTAINER_HANDLER_GUID",
                        Value::Bytes(CONTAINER_GUID.to_vec()),
                    ),
                    ("_SD_AVC_SUB_DESCRIPTOR", Value::Ref(avc_sub)),
                ],
            )?;
            // The admitted Premiere sRGB override assigns this same Rec.709
            // interpretation to the source. The physical file remains unchanged.
            original::picture_fields(
                doc,
                locator,
                upstream,
                extent,
                rate,
                attrs,
                Some(REC709_TRANSFER),
            )?
        } else {
            audio_descriptor(doc, locator, upstream, packet_samples)?
        };
        doc.set_field(file, "descriptor", Value::Ref(desc))?;
        let mut attrs = Vec::from(if picture {
            audio_metadata(0, 0)
        } else {
            audio_metadata(SAMPLE_RATE, packet_samples)
        });
        attrs.push(("_AMA_FILE_DATE_TIME", Value::Int(i64::from(modified))));
        if !picture {
            attrs.push(("_SAVED_AAF_PHYSICAL_TRACK_NUMBER", Value::Int(2)));
        }
        original::set_attrs(doc, file, attrs)?;
        for lane in 1..=if picture { 1 } else { 2 } {
            // Native picture starts one frame into the physical package, while
            // audio starts at zero and UME resolves the AAC priming packet.
            let clip =
                doc.create_source_clip(rate, kind, extent, physical_id, lane, i32::from(picture))?;
            let component = original::bracket(doc, rate, kind, clip)?;
            let mut attrs = Vec::from(selector_attrs(
                if picture { 1 } else { 2 },
                if picture { 1 } else { 2 },
            ));
            if !picture {
                attrs.push(("_SAVED_AAF_MONOSLOTID", Value::Int(i64::from(lane) + 2)));
            }
            original::set_attrs(doc, component, attrs)?;
            doc.append_track(file, lane, component)?;
        }
        doc.add_bin_source(file)?;
    }

    let files = [video_id, audio_id];
    let picture_id = write_master(
        doc,
        picture,
        rate,
        files,
        avc.audio_samples,
        timestamp,
        &source.picture_markers,
    )?;
    let audio_id = if source.split_avc_items() {
        write_master(doc, audio, rate, files, avc.audio_samples, timestamp, &[])?
    } else {
        picture_id
    };
    Ok([picture_id, audio_id, audio_id])
}

fn write_master(
    doc: &mut AvbDocument,
    route: &Route<'_>,
    rate: f64,
    [video_id, audio_id]: [MobId; 2],
    samples: i64,
    timestamp: u32,
    marker_plan: &[markers::Marker<'_>],
) -> Result<MobId> {
    let extent = route.extent;
    let marker_list = markers::write(doc, marker_plan)?;
    let master_id = id()?;
    let master = original::composition(
        doc,
        route.name,
        MobKind::Master,
        master_id,
        extent,
        rate,
        timestamp,
    )?;
    let mut user_attrs = vec![("Video", Value::Str("QTFF".into()))];
    if let Some(tape) = route.tape {
        user_attrs.push(("TapeID", Value::Str(tape.into())));
    }
    let user = original::attrs(doc, user_attrs)?;
    let import = original::attrs(doc, [])?;
    original::set_attrs(
        doc,
        master,
        audio_metadata(SAMPLE_RATE, samples).into_iter().chain([
            ("_VERSION", Value::Int(2)),
            ("_USER", Value::Ref(user)),
            ("_IMPORTSETTING", Value::Ref(import)),
            ("_IGNOREALPHA", Value::Int(1)),
            ("_INVERTALPHA", Value::Int(0)),
            ("_IMPORTED_AMA", Value::Int(1)),
            ("_SPANNEDUSINGFILLER", Value::Int(1)),
            ("_BOUNDS_REFORMATTING_OPTION", Value::Int(1)),
        ]),
    )?;
    markers::attach(
        doc,
        saved::index(saved::node(doc, master)?, "attributes")?,
        marker_list,
    )?;
    for (kind, lane, source_id) in [(1, 1, video_id), (2, 1, audio_id), (2, 2, audio_id)] {
        let clip = doc.create_source_clip(rate, kind, extent, source_id, lane, 0)?;
        let component = original::bracket(doc, rate, kind, clip)?;
        if kind == 1 {
            original::set_attrs(doc, component, [("_SAVED_AAF_SLOTID", Value::Int(1))])?;
        } else {
            let lane_value = i64::from(lane);
            original::set_attrs(
                doc,
                component,
                [
                    ("_SAVED_AAF_SOURCESLOTID", Value::Int(2)),
                    ("_SAVED_AAF_CHANNELID", Value::Int(lane_value)),
                    ("_SAVED_AAF_SLOTID", Value::Int(lane_value + 1)),
                ],
            )?;
        }
        doc.append_track(master, lane, component)?;
    }
    doc.add_bin_item(master)?;
    Ok(master_id)
}

fn audio_descriptor(
    doc: &mut AvbDocument,
    locator: u32,
    physical: u32,
    samples: i64,
) -> Result<u32> {
    let attrs = original::attrs(
        doc,
        [
            (
                "_CONTAINER_HANDLER_GUID",
                Value::Bytes(CONTAINER_GUID.to_vec()),
            ),
            ("SoundEssenceCoding", Value::Bytes(AAC_CODING.to_vec())),
        ],
    )?;
    let mut fields =
        original::common_descriptor(1, locator, Value::Ref(physical), original::UME_PROVIDER);
    fields.extend([
        ("attributes", Value::Ref(attrs)),
        (
            "edit_rate",
            Value::Float(SAMPLE_RATE as f64 / AAC_PRIMING as f64),
        ),
        ("length", Value::Int(samples)),
        ("is_omfi", Value::Int(0)),
        ("data_offset", Value::Int(0)),
        ("channels", Value::Int(2)),
        ("quantization_bits", Value::Int(16)),
        ("sample_rate", Value::Float(SAMPLE_RATE as f64)),
        ("locked", Value::Bool(false)),
        ("audio_ref_level", Value::Int(-20)),
        ("electro_spatial_formulation", Value::Int(2)),
        ("dial_norm", Value::Int(27)),
        ("coding_format", Value::Int(0)),
        ("bit_rate", Value::Int(0)),
        ("sub_frame_alignment", Value::UInt(0)),
        ("origin", Value::UInt(0)),
    ]);
    original::add_descriptor(doc, *b"MPGA", "MPGADescriptor", fields)
}

pub(super) fn check(doc: &AvbDocument, plan: &Plan<'_>, avc: &Encoding) -> Result<()> {
    let mut mobs = HashMap::new();
    let mut masters = Vec::new();
    let mut composition = None;
    let root = doc.root().ok_or("missing linked MP4 bin root")?;
    for item in saved::list(root, "items")? {
        let item = saved::embedded(item)?;
        let id = saved::index(item, "mob")?;
        let mob = saved::node(doc, id)?;
        let identity = saved::mob_id(mob)?;
        if mobs.insert(identity, mob).is_some() {
            return Err("duplicate generated MP4 MobID".into());
        }
        match (saved::number(mob, "mob_type_id")?, item.get("user_placed")) {
            (1, Some(Value::Bool(true))) if composition.is_none() => composition = Some(id),
            (2, Some(Value::Bool(true))) => masters.push((id, mob)),
            (3, Some(Value::Bool(false))) => {}
            _ => return Err("unexpected linked MP4 bin item".into()),
        }
    }
    let expected_masters = 1 + usize::from(plan.sources[0].split_avc_items());
    if masters.len() != expected_masters || mobs.len() != 4 + expected_masters {
        return Err("unexpected linked MP4 master count".into());
    }
    check_sources(doc, &plan.sources[0], plan.rate, avc, &masters, |id| {
        mobs.get(&id)
            .map(|node| (0, *node))
            .ok_or_else(|| "missing generated MP4 source".into())
    })?;
    let (_, picture_master) = masters[0];
    let audio_master = masters[expected_masters - 1].1;
    saved::check_timeline(
        doc,
        plan,
        composition.ok_or("missing linked MP4 sequence")?,
        &[[
            saved::mob_id(picture_master)?,
            saved::mob_id(audio_master)?,
            saved::mob_id(audio_master)?,
        ]],
    )
}

pub(super) fn check_sources<'a>(
    doc: &'a AvbDocument,
    source_set: &SourceSet<'_>,
    rate: EditRate,
    avc: &Encoding,
    masters: &[(u32, &'a Node)],
    by_id: impl Fn(MobId) -> Result<(u32, &'a Node)>,
) -> Result<()> {
    let [video, audio, _] = &source_set.routes;
    let edit_rate = Value::Float(rate.saved());
    for (&(_, master), route) in masters.iter().zip([video, audio]) {
        let tape = saved::attribute(doc, master, "_USER")
            .and_then(Value::as_index)
            .and_then(|id| doc.node(id))
            .and_then(|attrs| attrs.get("entries"));
        let tape_matches = matches!(tape, Some(Value::Attrs(attrs)) if
            attrs.iter().filter(|(name, _)| name == "TapeID")
                .filter_map(|(_, value)| value.as_str()).eq(route.tape.into_iter()));
        if master.get("name").and_then(Value::as_str) != Some(route.name)
            || master.get("edit_rate") != Some(&edit_rate)
            || saved::number(master, "length")? != i64::from(route.extent)
            || saved::list(master, "tracks")?.len() != 3
            || saved::attribute(doc, master, "_SAVED_AAF_AUDIO_LENGTH")
                != Some(&Value::Int(avc.audio_samples))
            || saved::attribute(doc, master, "_IMPORTED_AMA") != Some(&Value::Int(1))
            || !tape_matches
        {
            return Err("generated linked MP4 master disagrees".into());
        }
    }
    let source = |owner: &Node, kind, lane, start, extent| -> Result<(&Node, MobId)> {
        let track = saved::track(doc, owner, kind, lane)?;
        let clip = saved::leaf(doc, track, *b"SCLP", &edit_rate)?;
        if saved::number(clip, "track_id")? != lane
            || saved::number(clip, "start_time")? != start
            || saved::number(clip, "length")? != extent
        {
            return Err("generated MP4 source trim or channel disagrees".into());
        }
        let identity = saved::mob_id(clip)?;
        let (_, mob) = by_id(identity)?;
        if saved::number(mob, "mob_type_id")? != 3 || mob.get("edit_rate") != Some(&edit_rate) {
            return Err("invalid MP4 source clock or kind".into());
        }
        Ok((mob, identity))
    };
    let mut source_ids = Vec::new();
    let mut file_ids = Vec::new();
    for &(_, master) in masters {
        for (kind, lane) in [(1, 1), (2, 1), (2, 2)] {
            let extent = i64::from(video.extent);
            let (file, file_id) = source(master, kind, lane, 0, extent)?;
            let (physical, physical_id) = source(file, kind, lane, i64::from(kind == 1), extent)?;
            file_ids.push(file_id);
            source_ids.push(physical_id);
            let desc = saved::reference(doc, file, "descriptor")?;
            let file_locator = saved::reference(doc, desc, "locator")?;
            let embedded = saved::reference(doc, desc, "physical_media")?;
            let upstream = saved::reference(doc, physical, "descriptor")?;
            if file_locator.class != *b"MSML"
                || saved::mob_id(file_locator)? != file_id
                || [desc, embedded, upstream]
                    .iter()
                    .any(|node| node.get("uuid") != Some(&Value::Uuid(original::UME_PROVIDER)))
            {
                return Err("generated MP4 provider or package identity disagrees".into());
            }
            saved::file_locator(doc, embedded, video.path)?;
            saved::file_locator(doc, upstream, video.path)?;
            let clock = saved::leaf(
                doc,
                saved::track(doc, physical, 3, 1)?,
                *b"TCCP",
                &edit_rate,
            )?;
            if saved::number(clock, "start")? + 1 != i64::from(video.clock)
                || saved::number(clock, "fps")? != TIMECODE_BASE
                || saved::number(clock, "flags")? != 0
                || saved::number(clock, "length")? < extent + 1
            {
                return Err("generated MP4 source timecode disagrees".into());
            }
            let valid = if kind == 1 {
                desc.class == *b"CDCI"
                    && saved::number(desc, "length")? == extent
                    && desc.get("essence_compression") == Some(&Value::Uuid(AVC_COMPRESSION))
                    && desc.get("transfer_characteristic") == Some(&Value::Uuid(REC709_TRANSFER))
                    && saved::number(desc, "display_width")? == 1920
                    && saved::number(desc, "display_height")? == 1080
            } else {
                desc.class == *b"MPGA"
                    && saved::number(desc, "channels")? == 2
                    && desc.get("sample_rate") == Some(&Value::Float(SAMPLE_RATE as f64))
                    && saved::number(desc, "length")? == avc.audio_samples + AAC_PRIMING
                    && saved::attribute(
                        doc,
                        saved::track(doc, master, kind, lane)?,
                        "_SAVED_AAF_CHANNELID",
                    ) == Some(&Value::Int(lane))
            };
            if !valid {
                return Err("generated MP4 descriptor or channel disagrees".into());
            }
        }
    }
    if source_ids.windows(2).any(|pair| pair[0] != pair[1])
        || file_ids[0] == file_ids[1]
        || file_ids[1] != file_ids[2]
        || file_ids.contains(&source_ids[0])
        || file_ids
            .chunks_exact(3)
            .any(|files| files != &file_ids[..3])
    {
        return Err("generated MP4 picture and audio package routing disagrees".into());
    }
    let (master_id, _) = masters[0];
    markers::check(doc, master_id, &source_set.picture_markers)?;
    Ok(())
}
