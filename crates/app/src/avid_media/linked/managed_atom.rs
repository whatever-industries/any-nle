//! Write native managed-media references for a verified split OP-Atom clip.
use super::{
    markers, native_bin as saved, original, EditRate, ManagedBinding, Plan, Result, Route,
    SourceSet,
};
use crate::{backend_bundle::BackendTools, rewrap};
use anynle_resolve_adapter::{verify_avid_managed_atom, ManagedAtom};
use avid_reader::{
    document::{AvbDocument, MobKind},
    document_extra::create_msm_locator,
    io::{MobId, Uuid},
    objects::{Node, Value},
};
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

struct Member {
    package: MobId,
    volume: String,
    stamp: rewrap::source::Stamp,
    indexed: PathBuf,
    indexed_stamp: rewrap::source::Stamp,
    descriptor: Essence,
}

enum Essence {
    Picture {
        offset: i64,
        bytes: i64,
        frame_index: i64,
        color: [Option<[u8; 16]>; 3],
    },
    Pcm {
        offset: i64,
        samples: i64,
    },
}

fn indexed_file(path: &Path) -> Result<(&str, PathBuf)> {
    let mut parts = path.strip_prefix("/Volumes")?.components();
    let volume = parts
        .next()
        .and_then(|p| p.as_os_str().to_str())
        .ok_or("managed MXFs need a mounted Avid media volume")?;
    let root = Path::new("/Volumes")
        .join(volume)
        .join("Avid MediaFiles/MXF");
    let filename = path.file_name().ok_or("managed MXF filename is missing")?;
    if let Ok(relative) = path.strip_prefix(&root) {
        let mut parts = relative.components();
        if parts.next().is_some_and(|p| numbered(p.as_os_str()))
            && parts.next().is_some()
            && parts.next().is_none()
        {
            return Ok((volume, path.into()));
        }
    }
    let mut candidate = None;
    for (i, folder) in fs::read_dir(root)?.enumerate() {
        if i >= 4096 {
            return Err("Avid media folder search exceeds its directory budget".into());
        }
        let folder = folder?;
        if !folder.file_type()?.is_dir() || !numbered(&folder.file_name()) {
            continue;
        }
        let copy = folder.path().join(filename);
        match fs::symlink_metadata(&copy) {
            Ok(metadata) if metadata.is_file() => {
                if candidate.replace(copy).is_some() {
                    return Err(
                        "managed MXF has multiple same-named copies in Avid MediaFiles".into(),
                    );
                }
            }
            Ok(_) => return Err("managed MXF candidate is not a regular file".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok((
        volume,
        candidate.ok_or("managed MXF needs a matching copy in Avid MediaFiles/MXF")?,
    ))
}

fn numbered(value: &std::ffi::OsStr) -> bool {
    value
        .to_str()
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|byte| byte.is_ascii_digit()))
}

fn equal_files(a: &Path, b: &Path) -> Result<bool> {
    let (mut a, mut b) = (
        BufReader::new(File::open(a)?),
        BufReader::new(File::open(b)?),
    );
    let len = a.get_ref().metadata()?.len();
    if len != b.get_ref().metadata()?.len() {
        return Ok(false);
    }
    let (mut left, mut right) = ([0; 32 << 10], [0; 32 << 10]);
    let mut remaining = len;
    while remaining != 0 {
        let count = remaining.min(left.len() as u64) as usize;
        a.read_exact(&mut left[..count])?;
        b.read_exact(&mut right[..count])?;
        if left[..count] != right[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
}

fn verify(route: &Route<'_>, index: usize, tools: &BackendTools) -> Result<Member> {
    let path = Path::new(route.path);
    let stamp = rewrap::source::stamp(path)?;
    let ManagedBinding {
        material,
        receipt,
        clock,
    } = route.managed.ok_or("missing managed OP-Atom binding")?;
    let facts = verify_avid_managed_atom(path, &tools.ffprobe)?;
    let ManagedAtom {
        count, index: slot, ..
    } = facts;
    if facts.material != material
        || &facts.receipt() != receipt
        || &facts.clock != clock
        || slot != index
        || count != route.stream.duration_ts
    {
        return Err("managed OP-Atom file contradicts its saved package, clock or selector".into());
    }
    let descriptor = if index == 0 {
        let (offset, bytes, index_key) = rewrap::managed_dnxhd_layout(path, count)?;
        Essence::Picture {
            offset: offset.try_into()?,
            bytes: bytes.try_into()?,
            frame_index: index_key
                .checked_sub(offset)
                .ok_or("picture index precedes the essence")?
                .try_into()?,
            color: [facts.transfer, facts.primaries, facts.matrix],
        }
    } else {
        let spans = rewrap::essence_spans(path)?;
        let [sound] = spans.as_slice() else {
            return Err("expected a single managed PCM essence payload".into());
        };
        if sound.kind != 0x16
            || i64::try_from(sound.bytes)? != count.checked_mul(3).ok_or("PCM extent overflows")?
        {
            return Err("managed PCM essence size contradicts its saved sample extent".into());
        }
        Essence::Pcm {
            offset: sound.offset.try_into()?,
            samples: count,
        }
    };
    let (volume, indexed) = indexed_file(path)?;
    let indexed_stamp = rewrap::source::stamp(&indexed)?;
    if indexed != path && !equal_files(path, &indexed)? {
        return Err("Avid MediaFiles copy differs from the Premiere original".into());
    }
    Ok(Member {
        package: MobId::from_urn(&facts.siblings[index])?,
        volume: volume.into(),
        stamp,
        indexed,
        indexed_stamp,
        descriptor,
    })
}

pub(super) struct Verified<'a> {
    members: Vec<Member>,
    pub(super) master_id: MobId,
    physical_id: MobId,
    name: &'a str,
}

pub(super) fn prepare<'a>(source: &SourceSet<'a>, tools: &BackendTools) -> Result<Verified<'a>> {
    let members = source
        .routes
        .iter()
        .enumerate()
        .map(|(i, route)| {
            verify(route, i, tools).map_err(|e| format!("{}: {e}", route.path).into())
        })
        .collect::<Result<Vec<_>>>()?;
    let ManagedBinding {
        material,
        receipt,
        clock,
    } = source.routes[0]
        .managed
        .ok_or("missing managed material package")?;
    let Some(graph::Value::Str(name)) = receipt.get("name") else {
        return Err("missing saved managed master name".into());
    };
    let master_id = MobId::from_urn(material)?;
    let physical_id = MobId::from_urn(&clock.provenance.source_package)?;
    if master_id == physical_id
        || members
            .iter()
            .any(|file| [master_id, physical_id].contains(&file.package))
    {
        return Err("managed OP-Atom package identifiers overlap".into());
    }
    Ok(Verified {
        members,
        master_id,
        physical_id,
        name,
    })
}

impl Verified<'_> {
    pub(super) fn source_chain(
        &self,
        doc: &mut AvbDocument,
        source: &SourceSet<'_>,
        edit_rate: EditRate,
        timestamp: u32,
    ) -> Result<[MobId; 3]> {
        let Self {
            members,
            master_id,
            physical_id,
            name,
        } = self;
        let (master_id, physical_id, name) = (*master_id, *physical_id, *name);
        let (rate, extent, count) = (
            edit_rate.saved(),
            source.routes[0].extent,
            source.routes[0].clock,
        );
        let make = |doc: &mut AvbDocument, name, kind, id| {
            original::composition(doc, name, kind, id, extent, rate, timestamp)
        };
        let physical = make(
            doc,
            source.routes[0].tape.unwrap_or(name),
            MobKind::Source,
            physical_id,
        )?;
        let desc =
            original::add_descriptor(doc, *b"MDES", "MediaDescriptor", base(5, Value::Null))?;
        doc.set_field(physical, "descriptor", Value::Ref(desc))?;
        let timecode = doc.create_timecode(rate, extent, 24, count)?;
        doc.append_track(physical, 1, timecode)?;
        doc.add_bin_source(physical)?;
        let master = make(doc, name, MobKind::Master, master_id)?;
        let master_clock = doc.create_timecode(rate, extent, 24, count)?;
        doc.append_track(master, 1, master_clock)?;
        let mark_list = markers::write(doc, &source.picture_markers)?;
        let attrs = saved::index(saved::node(doc, master)?, "attributes")?;
        markers::attach(doc, attrs, mark_list)?;
        for (file, (kind, lane)) in members.iter().zip([(1, 1), (2, 1), (2, 2)]) {
            let terminal =
                doc.create_source_clip(rate, kind, extent, original::TERMINAL_SOURCE, 0, 0)?;
            doc.append_track(physical, lane, terminal)?;
            let filename = file
                .indexed
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or("managed MXF needs a valid filename")?;
            let mob = make(doc, filename, MobKind::Source, file.package)?;
            let locator =
                create_msm_locator(doc, &file.volume, file.package, Some(4), Some(&file.volume))?;
            let desc = descriptor(doc, locator, &file.descriptor, rate, extent)?;
            doc.set_field(mob, "descriptor", Value::Ref(desc))?;
            let upstream = doc.create_source_clip(rate, kind, extent, physical_id, lane, 0)?;
            doc.append_track(mob, lane, upstream)?;
            doc.add_bin_source(mob)?;
            let clip = doc.create_source_clip(rate, kind, extent, file.package, lane, 0)?;
            let track = original::bracket(doc, rate, kind, clip)?;
            doc.append_track(master, lane, track)?;
        }
        doc.add_bin_item(master)?;
        Ok([master_id; 3])
    }

    pub(super) fn recheck(&self, source: &SourceSet<'_>) -> Result<()> {
        for (route, file) in source.routes.iter().zip(&self.members) {
            if rewrap::source::stamp(Path::new(route.path))? != file.stamp
                || rewrap::source::stamp(&file.indexed)? != file.indexed_stamp
            {
                return Err(
                    format!("{} changed during managed OP-Atom preparation", route.path).into(),
                );
            }
        }
        Ok(())
    }
}

pub(super) fn write(
    plan: &Plan<'_>,
    destination: &Path,
    tools: &BackendTools,
    recheck_source: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let source = &plan.sources[0];
    let verified = prepare(source, tools)?;
    let (mut doc, timestamp) = original::new_bin()?;
    let masters = verified.source_chain(&mut doc, source, plan.rate, timestamp)?;
    saved::write_timeline(&mut doc, plan, &[masters], timestamp)?;
    let decoded = AvbDocument::from_bytes(doc.to_bytes()?)?;
    check(&decoded, plan, &verified)?;
    recheck_source()?;
    verified.recheck(source)?;
    decoded.write_new(destination)?;
    Ok(())
}

fn base(kind: i64, locator: Value) -> Vec<(&'static str, Value)> {
    vec![
        ("mob_kind", Value::Int(kind)),
        ("locator", locator),
        ("intermediate", Value::Bool(false)),
        ("physical_media", Value::Null),
    ]
}

fn label(bytes: Option<[u8; 16]>) -> Uuid {
    let bytes = bytes.unwrap_or([0; 16]);
    Uuid {
        data1: u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
        data2: u16::from_be_bytes(bytes[12..14].try_into().unwrap()),
        data3: u16::from_be_bytes(bytes[14..].try_into().unwrap()),
        data4: bytes[..8].try_into().unwrap(),
    }
}

fn descriptor(
    doc: &mut AvbDocument,
    locator: u32,
    essence: &Essence,
    rate: f64,
    extent: u32,
) -> Result<u32> {
    let mut fields = base(1, Value::Ref(locator));
    fields.push(("is_omfi", Value::Int(0)));
    let (class, name) = match *essence {
        Essence::Picture {
            offset,
            bytes,
            frame_index,
            color,
        } => {
            fields.extend([
                ("edit_rate", Value::Float(rate)),
                (
                    "aspect_ratio",
                    Value::List(vec![Value::Int(16), Value::Int(9)]),
                ),
                ("line_map", Value::List(vec![Value::Int(0)])),
                ("compress_method", Value::Bytes(b"AVHD".to_vec())),
                ("uniformness", Value::Bool(false)),
                ("next_did_desc", Value::Null),
                ("frame_checked_with_mapper", Value::Bool(true)),
                ("transfer_characteristic", Value::Uuid(label(color[0]))),
                ("color_primaries", Value::Uuid(label(color[1]))),
                ("coding_equations", Value::Uuid(label(color[2]))),
            ]);
            fields.extend(
                [
                    ("length", extent.into()),
                    ("data_offset", offset),
                    ("stored_height", 1080),
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
                    ("did_image_size", bytes),
                    ("resolution_id", 1253),
                    ("image_alignment_factor", 4096),
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
                    ("frame_index_offset", frame_index),
                    ("alpha_sampled_width", 0),
                    ("ignore_bw", 0),
                ]
                .map(|(name, value)| (name, Value::Int(value))),
            );
            (*b"CDCI", "CDCIDescriptor")
        }
        Essence::Pcm { offset, samples } => {
            fields.extend([
                ("edit_rate", Value::Float(48000.0)),
                ("sample_rate", Value::Float(48000.0)),
                ("locked", Value::Bool(false)),
                ("has_peak_envelope_data", Value::Bool(false)),
                ("peak_of_peaks_offset", Value::UInt(u64::MAX)),
            ]);
            fields.extend(
                [
                    ("length", samples),
                    ("data_offset", offset),
                    ("channels", 1),
                    ("quantization_bits", 24),
                    ("audio_ref_level", -20),
                    ("electro_spatial_formulation", 2),
                    ("dial_norm", 27),
                    ("coding_format", 1),
                    ("block_align", 3),
                    ("sequence_offset", 0),
                    ("average_bps", 144000),
                    ("peak_envelope_version", 0),
                    ("peak_envelope_format", 0),
                    ("points_per_peak_value", 0),
                    ("peak_envelope_block_size", 0),
                    ("peak_channel_count", 0),
                    ("peak_frame_count", 0),
                    ("peak_envelope_timestamp", 0),
                ]
                .map(|(name, value)| (name, Value::Int(value))),
            );
            (*b"PCMA", "PCMADescriptor")
        }
    };
    original::add_descriptor(doc, class, name, fields)
}

fn check(doc: &AvbDocument, plan: &Plan<'_>, verified: &Verified<'_>) -> Result<()> {
    let mobs = (1..=u32::try_from(doc.object_count())?)
        .filter_map(|id| {
            doc.node(id)
                .filter(|node| node.class == *b"CMPO")
                .map(|node| (id, node))
        })
        .collect::<Vec<_>>();
    let by_id = |id| -> Result<(u32, &Node)> {
        let mut matches = mobs
            .iter()
            .filter(|(_, node)| saved::mob_id(node).ok() == Some(id));
        let found = matches
            .next()
            .copied()
            .ok_or("saved managed package was not found")?;
        if matches.next().is_some() {
            return Err("duplicate saved managed package".into());
        }
        Ok(found)
    };
    verified.check_sources(doc, &plan.sources[0], plan.rate, by_id)?;
    let sequences = mobs
        .iter()
        .filter(|(_, node)| saved::number(node, "mob_type_id").ok() == Some(1))
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    let [sequence] = sequences.as_slice() else {
        return Err("saved managed bin needs exactly one timeline".into());
    };
    saved::check_timeline(doc, plan, *sequence, &[[verified.master_id; 3]])
}

impl Verified<'_> {
    pub(super) fn check_sources<'a>(
        &self,
        doc: &'a AvbDocument,
        source: &SourceSet<'_>,
        edit_rate: EditRate,
        by_id: impl Fn(MobId) -> Result<(u32, &'a Node)>,
    ) -> Result<()> {
        let Self {
            members,
            master_id,
            physical_id,
            name,
        } = self;
        let (master_id, physical_id, name) = (*master_id, *physical_id, *name);
        let (master_index, master) = by_id(master_id)?;
        let (_, physical) = by_id(physical_id)?;
        if master.get("name").and_then(Value::as_str) != Some(name) {
            return Err("saved managed master name changed".into());
        }
        for node in [master, physical] {
            let tc = saved::track(doc, node, 3, 1)?;
            if tc.class != *b"TCCP"
                || saved::number(tc, "start")? != i64::from(source.routes[0].clock)
                || saved::number(tc, "fps")? != 24
                || saved::number(tc, "flags")? != 0
            {
                return Err("saved managed timecode changed".into());
            }
        }
        markers::check(doc, master_index, &source.picture_markers)?;
        let rate = Value::Float(edit_rate.saved());
        for (file, (kind, lane)) in members.iter().zip([(1, 1), (2, 1), (2, 2)]) {
            let (_, package) = by_id(file.package)?;
            for (owner, target) in [(master, file.package), (package, physical_id)] {
                let track = saved::track(doc, owner, kind, lane)?;
                let clip = saved::leaf(doc, track, *b"SCLP", &rate)?;
                if saved::mob_id(clip)? != target
                    || saved::number(clip, "track_id")? != lane
                    || saved::number(clip, "start_time")? != 0
                    || saved::number(clip, "length")? != i64::from(source.routes[0].extent)
                {
                    return Err("saved managed file or physical selector changed".into());
                }
            }
            let desc = saved::reference(doc, package, "descriptor")?;
            let locator = saved::reference(doc, desc, "locator")?;
            let layout_matches = match file.descriptor {
                Essence::Picture {
                    offset,
                    bytes,
                    frame_index,
                    ..
                } => {
                    desc.class == *b"CDCI"
                        && saved::number(desc, "data_offset")? == offset
                        && saved::number(desc, "did_image_size")? == bytes
                        && saved::number(desc, "frame_index_offset")? == frame_index
                }
                Essence::Pcm { offset, samples } => {
                    desc.class == *b"PCMA"
                        && saved::number(desc, "data_offset")? == offset
                        && saved::number(desc, "length")? == samples
                        && saved::number(desc, "channels")? == 1
                }
            };
            if !layout_matches
                || locator.class != *b"MSML"
                || saved::number(locator, "domain_type")? != 4
                || saved::mob_id(locator)? != file.package
                || locator.get("last_known_volume").and_then(Value::as_str)
                    != Some(file.volume.as_str())
            {
                return Err("saved managed descriptor or locator changed".into());
            }
        }
        Ok(())
    }
}
