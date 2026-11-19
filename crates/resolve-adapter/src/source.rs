//! Standalone source interpretation of the qualified Resolve archive profile.
mod atom;
pub mod compare;
pub mod conform;
pub mod drt;
mod effects;
mod encoding;
mod fairlight;
pub mod inventory;
mod markers;
pub(crate) use markers::{
    supported_marker_rate, EDIT_ADAPTER as EDIT_MARKER_ADAPTER, EDIT_MARKER_LOSS,
    POOL_ADAPTER as POOL_MARKER_ADAPTER,
};
mod metadata;
pub(crate) mod mixer;
mod multicam;
pub(crate) use multicam::{native_group_default_unavailable, REFERENCE_AUDIO_SWITCH};
mod observations;
mod title;
pub(crate) mod relink;
mod report;
mod wav;
mod xml;

use crate::{
    audio::Volume,
    Result,
};
use encoding::{extent, frame_rate, frames, hex_bytes, pb_bytes, qt, raster, rate, timecode, Blobs};
use graph::{
    source::{
        SourceClock, SourceClockProvenance, SourceGraph, SourceItem, SourceItemMedia, SourceMedia,
        SourceSequence, SourceTrack,
    },
    Bin, Loss, Placement, Project, Rational, Sidecar, StreamKind, Value,
};
use mixer::Mixer;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};
use xml::Node;

const ROOT_FOLDER: &str = "MediaPool/Master/MpFolder.xml";
pub(crate) const ADAPTER: &str = "resolve.source";
pub(crate) const BIN_ORDER_ADAPTER: &str = "resolve.bin_order";
pub(crate) const ACTIVE_TIMELINE_ADAPTER: &str = "resolve.active_timeline";
const OMITTED_TRACKS_ADAPTER: &str = "resolve.omitted_tracks";
const EMPTY_STEREO: &str = "unqualified Resolve track state or layout";
use crate::nests::{self, ADAPTER as NESTS_ADAPTER};
pub(crate) const CLOCK_ROUTE: &str = "Resolve archive clock";

pub(crate) fn default_empty_stereo<'a>(graph: &'a SourceGraph, owner: &str) -> Option<&'a str> {
    let Value::Arr(rows) = graph.sidecar(OMITTED_TRACKS_ADAPTER, owner)? else { return None };
    let [track] = rows.as_slice() else { return None };
    if !matches!(track.get("kind"), Some(Value::Str(kind)) if kind == "audio")
        || track.get("track") != Some(&Value::Int(1))
        || track.get("empty_default_stereo") != Some(&Value::Bool(true))
    {
        return None;
    }
    match track.get("id")? {
        Value::Str(id) => Some(id),
        _ => None,
    }
}

pub(crate) fn is_empty_stereo_loss(loss: &Loss, owner: &str, track: &str) -> bool {
    loss.object == owner && loss.aspect == "track"
        && loss.reason.split_once(": ") == Some((track, EMPTY_STEREO))
}

/// Read an exported .drp without opening media or making application calls.
pub fn read(path: &Path) -> Result<SourceGraph> {
    let members = xml::read(path)?;
    read_members(&members, false)
}

fn read_drp_bytes(bytes: &[u8]) -> Result<SourceGraph> {
    let members = xml::read_zip(std::io::Cursor::new(bytes))?;
    read_members(&members, false)
}

fn archive_is_drt(path: &Path) -> Result<bool> {
    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("drt") => Ok(true),
        Some(ext) if ext.eq_ignore_ascii_case("drp") => Ok(false),
        _ => Err("expected a Resolve .drp or .drt archive".into()),
    }
}

fn read_members(members: &BTreeMap<String, Node>, inspect_drt: bool) -> Result<SourceGraph> {
    let mut blobs = Blobs::default();
    let project = members
        .get("project.xml")
        .ok_or("missing Resolve project.xml")?;
    expect(project, "SM_Project")?;
    if project.get("ProjectVersion")? != "17" {
        return Err("unsupported Resolve project version".into());
    }
    let folders = folders(members, project, &mut blobs)?;
    let containers = containers(members)?;
    let mut graph = SourceGraph {
        project: Project {
            name: project.get("ProjectName")?.into(),
        },
        media: vec![],
        items: vec![],
        bins: vec![],
        sequences: vec![],
        losses: vec![],
        sidecars: vec![],
        multicams: vec![],
        multicam_uses: vec![],
    };
    let mut masters = HashMap::new();
    let mut timelines = HashMap::new();
    let mut groups = HashMap::new();
    let mut nested = HashMap::new();
    let mut compounds = Vec::new();
    let mut pool_items = HashSet::new();
    let mut ids = HashSet::new();
    claim(&mut ids, project.id()?)?;
    claim(&mut ids, project.one("MediaPool")?.entry()?.id()?)?;
    for (folder, order) in folders {
        let bin_id = folder.id()?;
        claim(&mut ids, bin_id)?;
        if let Some(order) = order {
            graph.sidecars.push(Sidecar {
                adapter: BIN_ORDER_ADAPTER.into(),
                object: bin_id.into(),
                data: order.receipt(),
            });
        }
        let mut bin = Bin {
            id: bin_id.into(),
            name: folder.get("Name")?.into(),
            parent: folder
                .maybe("MpFolder")?
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            items: vec![],
        };
        for entry in folder.one("MediaVec")?.elements()? {
            let clip = entry.entry()?;
            let clip_id = clip.id()?;
            claim(&mut ids, clip_id)?;
            pool_items.insert(clip_id);
            same(clip.get("MpFolder")?, bin_id, "pool item folder")?;
            match clip.name.as_str() {
                "Sm2MpVideoClip" => {
                    if clip.optional("Video")?.is_some() {
                        let master = media(clip, &mut blobs, &mut graph)?;
                        masters.insert(clip_id, master);
                        bin.items.push(clip_id.into());
                    } else {
                        omitted(
                            &mut graph,
                            bin_id,
                            clip_id,
                            "media",
                            "audio-only Resolve pool item is outside the qualified profile",
                        );
                    }
                }
                "Sm2MpAudioClip" => {
                    if let Some(master) = wav::read(clip, &mut blobs, &mut graph)? {
                        masters.insert(clip_id, master);
                        bin.items.push(clip_id.into());
                    } else {
                        omitted(
                            &mut graph,
                            bin_id,
                            clip_id,
                            "media",
                            "unqualified Resolve standalone audio cache",
                        );
                    }
                }
                "Sm2MpTimelineClip" => {
                    let handle = clip.one("TimelineSharedHandle")?.entry()?;
                    expect(handle, "Sm2Timeline")?;
                    let handle_id = handle.id()?;
                    claim(&mut ids, handle_id)?;
                    same(
                        handle.get("MpTimelineClip")?,
                        clip_id,
                        "timeline pool back-reference",
                    )?;
                    timelines.insert(handle_id, (handle, clip, bin_id));
                    let child = handle.one("Sequence")?.entry()?;
                    nested.insert(clip_id, (child, "timeline"));
                }
                "Sm2MpCompoundClip" => {
                    let child = clip.one("Sequence")?.entry()?;
                    expect(child, "Sm2Sequence")?;
                    same(child.get("Parent")?, clip_id, "compound sequence owner")?;
                    nested.insert(clip_id, (child, "compound"));
                    compounds.push((clip, child, bin_id));
                }
                "Sm2MpMulticamClip" => {
                    groups.insert(clip_id, clip);
                }
                _ => omitted(
                    &mut graph,
                    bin_id,
                    clip_id,
                    "pool_item",
                    "unsupported Resolve pool object class",
                ),
            }
        }
        graph.bins.push(bin);
    }

    let registered = project.one("TimelineHandleVec")?.elements()?;
    let cadence_profile = masters.values().any(|master| {
        master.camera_pcm || master.prores_hq
            && (supported_marker_rate(master.rate)
                || crate::mixed_rate::Cadence::for_resolve_source(master.rate, crate::prepare::rate()).is_some())
    });
    let (nearest_default, input_scaling_default) = if cadence_profile
        || masters.values().any(|master| master.size.0 > 0 && master.size != (1920, 1080))
    {
        encoding::default_picture_settings(project, &mut blobs).unwrap_or_default()
    } else {
        (false, None)
    };
    let mut context = Context {
        masters,
        groups: HashMap::new(),
        containers,
        ids,
        pool_items,
        nested,
        blobs,
        graph,
        used_containers: HashSet::new(),
        inspect_drt,
        nearest_default: cadence_profile && nearest_default,
        input_scaling_default,
    };
    // Definitions are independent of the order in which outer timelines use them.
    let groups: BTreeMap<_, _> = groups.into_iter().collect();
    for (id, node) in groups {
        let definition = multicam::definition(&mut context, node)?;
        context.groups.insert(id, definition);
    }
    for entry in registered {
        let handle_id = entry.scalar()?.trim();
        let (handle, clip, bin_id) = timelines
            .remove(handle_id)
            .ok_or("missing or duplicate registered Resolve timeline")?;
        let sequence = sequence(&mut context, handle, clip, bin_id)?;
        context.graph.sequences.push(sequence);
    }
    if !timelines.is_empty() {
        return Err("Resolve pool contains an unregistered timeline".into());
    }
    if !inspect_drt {
        let root = members.get(ROOT_FOLDER).ok_or("missing Resolve root bin")?;
        context.graph.sidecars.push(Sidecar {
            adapter: ACTIVE_TIMELINE_ADAPTER.into(),
            object: root.id()?.into(),
            data: active_timeline_receipt(project, &context.graph.sequences),
        });
    }
    for (clip, child, bin_id) in compounds {
        let id = child.id()?;
        let grid = Grid::read(child)?;
        let clock = grid.clock(id)?;
        let mut sequence = SourceSequence {
            disabled_placements: Vec::new(),
            id: id.into(),
            name: clip.get("Name")?.into(),
            rate: grid.rate,
            frame_size: Some(grid.size),
            start: clock.exact_start(),
            clock: Some(clock),
            tracks: vec![],
            placements: vec![],
            transitions: vec![],
            markers: vec![],
            texts: vec![],
        };
        let mut container_ref_agrees = false;
        if let Some(container) = context.container(id, grid.length)? {
            if let Some(encoded) = child.maybe("FieldsBlob")? {
                let bytes = hex_bytes(encoded)?;
                context.blobs.charge(bytes.len())?;
                let container_id = container.id()?;
                container_ref_agrees = qt(&bytes).ok()
                    .is_some_and(|map| map.text("SeqRef").ok().flatten() == Some(container_id));
            }
            read_tracks(&mut context, container, grid, &mut sequence, false, false)?;
        }
        receipt(&mut context.graph, id, vec![
            ("kind", text("compound")),
            ("container_ref_agrees", Value::Bool(container_ref_agrees)),
            ("media_pool_clip", text(clip.id()?)),
            ("bin_id", text(bin_id)),
            ("record_origin", Value::Int(grid.origin)),
            ("record_length", Value::Int(grid.length)),
        ]);
        context.graph.sequences.push(sequence);
    }
    if context.used_containers.len() != context.containers.len() {
        return Err("Resolve archive contains an unregistered sequence container".into());
    }
    let parents: HashSet<_> = context.graph.sidecars.iter()
        .filter(|row| row.adapter == NESTS_ADAPTER).map(|row| row.object.as_str()).collect();
    for row in context.graph.sidecars.iter().filter(|row| row.adapter == NESTS_ADAPTER) {
        let Value::Arr(uses) = &row.data else { return Err("invalid Resolve nesting facts".into()) };
        for usage in uses {
            let Some(Value::Str(child)) = usage.get("child_sequence") else {
                return Err("missing Resolve nested sequence identity".into());
            };
            if parents.contains(child.as_str()) {
                return Err("Resolve nested sequence chain exceeds one level or is cyclic".into());
            }
        }
    }
    markers::read(project, &mut context.graph, &context.masters, &mut context.blobs)?;
    context.graph.validate()?;
    Ok(context.graph)
}

fn active_timeline_receipt(project: &Node, sequences: &[SourceSequence]) -> Value {
    let index = match project.optional("CurrentTimelineIndex") {
        Err(_) => Err("ambiguous"),
        Ok(None) => Err("missing"),
        Ok(Some(node)) => node
            .scalar()
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .ok_or("malformed"),
    };
    // Sequences have already been resolved in TimelineHandleVec order.
    let (saved_index, sequence_id, reason) = match index {
        Ok(index) => match usize::try_from(index).ok().and_then(|i| sequences.get(i)) {
            Some(sequence) => (Value::Int(index), text(&sequence.id), Value::Null),
            None => (Value::Int(index), Value::Null, text("out_of_range")),
        },
        Err(reason) => (Value::Null, Value::Null, text(reason)),
    };
    Value::Obj(vec![
        ("saved_index".into(), saved_index),
        ("sequence_id".into(), sequence_id),
        ("unavailable_reason".into(), reason),
    ])
}

struct Context<'a> {
    masters: HashMap<&'a str, Master>,
    groups: HashMap<&'a str, multicam::Group<'a>>,
    containers: HashMap<&'a str, &'a Node>,
    ids: HashSet<&'a str>,
    pool_items: HashSet<&'a str>,
    nested: HashMap<&'a str, (&'a Node, &'static str)>,
    used_containers: HashSet<&'a str>,
    inspect_drt: bool,
    nearest_default: bool,
    input_scaling_default: Option<&'static str>,
    blobs: Blobs,
    graph: SourceGraph,
}

impl<'a> Context<'a> {
    fn container(&mut self, sequence_id: &'a str, length: i64) -> Result<Option<&'a Node>> {
        claim(&mut self.ids, sequence_id)?;
        match self.containers.get(sequence_id).copied() {
            Some(node) => {
                claim(&mut self.ids, node.id()?)?;
                self.used_containers.insert(sequence_id);
                Ok(Some(node))
            }
            None if length == 0 => Ok(None),
            None => Err("missing Resolve sequence container".into()),
        }
    }
}

fn expect(node: &Node, name: &str) -> Result<()> {
    same(&node.name, name, "object type")
}

fn same(found: &str, expected: &str, field: &str) -> Result<()> {
    if found.trim() != expected {
        return Err(format!("contradictory Resolve {field}").into());
    }
    Ok(())
}

fn claim<'a>(ids: &mut HashSet<&'a str>, id: &'a str) -> Result<()> {
    if !ids.insert(id) {
        return Err("duplicate Resolve object identity".into());
    }
    Ok(())
}

fn receipt(graph: &mut SourceGraph, id: &str, fields: Vec<(&str, Value)>) {
    graph.sidecars.push(Sidecar {
        adapter: ADAPTER.into(),
        object: id.into(),
        data: Value::Obj(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        ),
    });
}

fn loss(graph: &mut SourceGraph, id: &str, aspect: &str, reason: &str) {
    graph.losses.push(Loss {
        object: id.into(),
        aspect: aspect.into(),
        reason: reason.into(),
    });
}

fn omitted(graph: &mut SourceGraph, owner: &str, occurrence: &str, aspect: &str, reason: &str) {
    loss(graph, owner, aspect, &format!("{occurrence}: {reason}"));
}

fn text(s: &str) -> Value {
    Value::Str(s.into())
}

enum BinOrder {
    Saved { archive_matches: Option<bool> },
    ArchivePath,
}

impl BinOrder {
    fn receipt(self) -> Value {
        let (source, archive_matches) = match self {
            Self::Saved { archive_matches } => ("fields_blob", archive_matches),
            Self::ArchivePath => ("archive_path", None),
        };
        Value::Obj(vec![
            ("source".into(), text(source)),
            (
                "archive_order_matches".into(),
                archive_matches.map_or(Value::Null, Value::Bool),
            ),
        ])
    }
}

fn folders<'a>(
    members: &'a BTreeMap<String, Node>,
    project: &Node,
    blobs: &mut Blobs,
) -> Result<Vec<(&'a Node, Option<BinOrder>)>> {
    let root = members.get(ROOT_FOLDER).ok_or("missing Resolve root bin")?;
    let pool = project.one("MediaPool")?.entry()?;
    expect(pool, "Sm2MediaPool")?;
    let pool_id = pool.id()?;
    let fields = blobs.qt(pool.get("FieldsBlob")?)?;
    same(
        fields
            .text("RootFolderRef")?
            .ok_or("missing root bin reference")?,
        root.id()?,
        "root bin identity",
    )?;
    let mut seen = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    // BTreeMap ordering can place children before their parents.
    let mut entries: Vec<_> = members
        .iter()
        .filter(|(name, _)| name.starts_with("MediaPool/"))
        .collect();
    entries.sort_by_key(|(name, _)| name.matches('/').count());
    for &(name, folder) in &entries {
        expect(folder, "Sm2MpFolder")?;
        same(folder.get("MediaPool")?, pool_id, "bin media pool")?;
        let id = folder.id()?;
        let parent = folder.maybe("MpFolder")?.filter(|s| !s.is_empty());
        let directory = name
            .strip_suffix("/MpFolder.xml")
            .ok_or("invalid bin path")?;
        let expected_parent = directory.rsplit_once('/').map(|(parent, _)| parent);
        match parent {
            None if name == ROOT_FOLDER => {}
            Some(parent)
                if seen
                    .get(parent)
                    .is_some_and(|&(path, _)| Some(path) == expected_parent) =>
            {
                children.entry(parent).or_default().push(id);
            }
            _ => return Err("missing or contradictory Resolve bin parent".into()),
        }
        if seen.insert(id, (directory, folder)).is_some() {
            return Err("duplicate Resolve bin identity".into());
        }
    }
    let mut orders = HashMap::new();
    for (_, folder) in entries {
        let id = folder.id()?;
        let siblings = children.get_mut(id);
        let Some(fields) = folder
            .maybe("FieldsBlob")?
            .filter(|value| !value.is_empty())
        else {
            if siblings.is_some() {
                orders.insert(id, BinOrder::ArchivePath);
            }
            continue;
        };
        let fields = blobs.envelope(fields)?;
        if let Some(encoded) = pb_bytes(&fields, 2)? {
            let map = qt(encoded)?;
            let count = map.keys().count();
            let mut listed = HashSet::new();
            let mut saved = vec![""; count];
            for key in map.keys() {
                let index: usize = key.parse()?;
                let child = map.text(key)?.ok_or("missing Resolve child bin identity")?;
                if index >= count || index.to_string() != key || !listed.insert(child) {
                    return Err("ambiguous Resolve child bin registration".into());
                }
                saved[index] = child;
            }
            let expected = siblings.as_deref().map(Vec::as_slice).unwrap_or_default();
            if expected.len() != listed.len() || expected.iter().any(|child| !listed.contains(child))
            {
                return Err("Resolve child bin registration contradicts the archive paths".into());
            }
            if let Some(siblings) = siblings {
                // Use the archive-owned UUID slices once membership is proven.
                let mut numbered: Vec<_> = siblings
                    .iter()
                    .filter_map(|&child| {
                        let (path, _) = seen[child];
                        Some((folder_path_ordinal(path)?, child))
                    })
                    .collect();
                numbered.sort_unstable();
                let archive_matches = (numbered.len() == saved.len()
                    && !numbered.windows(2).any(|pair| pair[0].0 == pair[1].0))
                    .then(|| numbered.iter().map(|&(_, child)| child).eq(saved.iter().copied()));
                *siblings = saved
                    .iter()
                    .map(|&child| *seen.get_key_value(child).expect("verified child identity").0)
                    .collect();
                orders.insert(id, BinOrder::Saved { archive_matches });
            }
        } else if siblings.is_some() {
            return Err("missing Resolve child bin registration".into());
        }
    }

    // A breadth-first walk keeps every parent earlier than its children.
    let mut queue = vec![root.id()?];
    let mut ordered = Vec::with_capacity(seen.len());
    let mut cursor = 0;
    while let Some(&id) = queue.get(cursor) {
        ordered.push((seen[id].1, orders.remove(id)));
        if let Some(siblings) = children.remove(id) {
            queue.extend(siblings);
        }
        cursor += 1;
    }
    Ok(ordered)
}

fn folder_path_ordinal(path: &str) -> Option<usize> {
    let (ordinal, _) = path.rsplit('/').next()?.split_once('_')?;
    if ordinal.is_empty() || !ordinal.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    ordinal.parse().ok()
}

fn containers(members: &BTreeMap<String, Node>) -> Result<HashMap<&str, &Node>> {
    let mut result = HashMap::new();
    for (path, node) in members
        .iter()
        .filter(|(name, _)| name.starts_with("SeqContainer/"))
    {
        expect(node, "Sm2SequenceContainer")?;
        if *path != format!("SeqContainer/{}.xml", node.id()?) {
            return Err("Resolve sequence container path differs from its identity".into());
        }
        let mut ids = HashSet::new();
        for (_, track) in tracks(node)? {
            ids.insert(track.get("Sequence")?.trim());
        }
        // Empty containers can be attached only when a sequence witness exists.
        if ids.len() != 1 || result.insert(*ids.iter().next().unwrap(), node).is_some() {
            return Err("ambiguous Resolve sequence container registration".into());
        }
    }
    Ok(result)
}

fn tracks(node: &Node) -> Result<Vec<(StreamKind, &Node)>> {
    let mut result = Vec::new();
    for (kind, tag, ty) in [
        (StreamKind::Video, "VideoTrackVec", "0"),
        (StreamKind::Audio, "AudioTrackVec", "1"),
    ] {
        for entry in node.one(tag)?.elements()? {
            let track = entry.entry()?;
            expect(track, "Sm2TiTrack")?;
            same(track.get("Type")?, ty, "track kind")?;
            result.push((kind, track));
        }
    }
    Ok(result)
}

#[derive(Debug)]
struct Master {
    rate: Rational,
    length: i64,
    start: i64,
    audio: BTreeMap<i64, i64>,
    path: String,
    size: (i64, i64),
    prores_hq: bool,
    camera_pcm: bool,
    native_mono_movie: bool,
    wav_samples: Option<i64>,
    standalone_mono_atom: bool,
    stereo_pool_route: bool,
    native_stereo_pcm: bool,
    split_atom_audio: bool,
}

fn media(clip: &Node, blobs: &mut Blobs, graph: &mut SourceGraph) -> Result<Master> {
    let id = clip.id()?;
    let video = clip.one("Video")?.entry()?;
    expect(video, "BtVideoInfo")?;
    same(video.get("Type")?, "STANDARD_CLIP", "video media type")?;
    let timing = blobs.qt(video.get("Time")?)?;
    let (rate, drop) = frame_rate(timing.bytes("FrameRate")?)?;
    if timing.int("StartFrame")? != 0 {
        return Err("unsupported nonzero Resolve media cache origin".into());
    }
    let length = timing.int("NumFrames")?;
    if length <= 0 {
        return Err("invalid Resolve media duration".into());
    }
    let saved_timecode = timing.text("Timecode")?.filter(|value| !value.is_empty());
    let base = crate::linked::display_base(rate);
    let start = if drop {
        let label = saved_timecode.ok_or("drop-frame Resolve media cache has no timecode")?;
        timecode(label, i64::from(base))?;
        let label = format!("{};{}", &label[..8], &label[9..]);
        i64::try_from(crate::media::parse_timecode(&label, base)?.0)?
    } else {
        timecode(saved_timecode.unwrap_or("00:00:00:00"), i64::from(base))?
    };
    let geometry = blobs.qt(video.get("Geometry")?)?;
    let size = raster(geometry.bytes("Resolution")?)?;
    let (path, codec) = cache(video, blobs)?;
    let mut item = SourceItem {
        id: id.into(),
        name: clip.get("Name")?.into(),
        media: vec![],
        umid: None,
        tape: None,
        rate: Some(rate),
        color_rgb16: None,
        stream_groups: vec![],
        markers: vec![],
    };
    let mut clock = clock(id, "BtVideoTime.Timecode", start, rate)?;
    clock.drop = drop;
    let file_start = clock.exact_start();
    let clock =
        (saved_timecode.is_some() || !matches!(codec.as_str(), "apch" | "avc1" | "hvc1")).then_some(clock);
    add_media(
        graph,
        &mut item,
        SourceMedia {
            id: format!("{id}/video"),
            kind: StreamKind::Video,
            locations: vec![path.clone()],
            codec: Some(codec.clone()),
            frame_size: Some(size),
            rate: Some(rate),
            sample_rate: None,
            bits: None,
            duration: seconds(length, rate)?,
            start: file_start,
            clock,
            stream: Some(1),
            online: None,
            original: None,
        },
    );
    let proxy = proxy_observation(clip, blobs).unwrap_or_default();
    let pcm = proxy.as_ref().and_then(|v| match v.get("audio_bits") {
        Some(Value::Int(bits)) => Some(*bits), _ => None,
    });
    let mut audio = BTreeMap::new();
    let mut split_atom_audio = false;
    let mut stereo_pool_route = false;
    let mut native_stereo_pcm = false;
    let mut camera_pcm = false;
    let mut native_mono_movie = false;
    if let Some(embedded) = clip.optional("EmbeddedAudioVec")? {
        let mut entries = embedded.elements()?;
        match (entries.next(), entries.next()) {
            (Some(entry), None) => {
                let node = entry.entry()?;
                expect(node, "BtAudioInfo")?;
                let (audio_path, audio_codec) = cache(node, blobs)?;
                same(&audio_path, &path, "audio cache locator")?;
                let avc_audio = codec == "avc1"
                    && (audio_codec == "AAC"
                        || audio_codec == "Linear PCM" && crate::linked::embedded_movie_pcm_rate(rate))
                    && (size == (1920, 1080)
                        || size == (3840, 2160) && crate::linked::uhd_rate(rate))
                    && crate::linked::avc_rate(rate);
                let hevc_audio = codec == "hvc1" && audio_codec == "AAC"
                    && rate == Rational::integer(24) && size == (1920, 1080)
                    && saved_timecode.is_none()
                    && std::path::Path::new(&path).extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"));
                let timecoded_camera = codec == "avc1" && audio_codec == "Linear PCM"
                    && rate == Rational::integer(24) && size == (4096, 2160)
                    && saved_timecode.is_some() && Path::new(&path).extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mov"));
                let native_movie = crate::native_import::supported_rate(rate)
                    && matches!(audio_codec.as_str(), "Linear PCM" | "AAC")
                    && pcm.is_none()
                    && Path::new(&path).extension().is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("mov") || ext.eq_ignore_ascii_case("mp4")
                    });
                native_mono_movie = native_movie && native_mono_selectors(clip, blobs)?.is_some();
                let default_stereo_movie = native_movie
                    && clip.maybe("VirtualAudioTracksBA")?.is_none_or(str::is_empty)
                    && movie_stereo_route(clip, blobs)?;
                if timecoded_camera || native_mono_movie || default_stereo_movie
                    || start == 0 && (avc_audio || hevc_audio)
                {
                    read_embedded_stereo(
                        clip, node, &path, length, rate, start, &audio_codec, None, blobs, graph, &mut item,
                    )?;
                    audio = BTreeMap::from([(0, 0), (1, 1)]);
                    stereo_pool_route = true;
                    camera_pcm = timecoded_camera;
                    native_stereo_pcm = !timecoded_camera && audio_codec == "Linear PCM"
                        && crate::stereo::mov(std::path::Path::new(&path))
                        && clip.maybe("VirtualAudioTracksBA")?.is_none_or(str::is_empty);
                } else if codec == "apch" && audio_codec == "Linear PCM" && size == (1920,1080)
                    && rate == Rational::new(24000,1001).unwrap()
                    && saved_timecode.is_some()
                    && pcm.is_some()
                {
                    read_embedded_stereo(
                        clip, node, &path, length, rate, start, &audio_codec, pcm, blobs, graph, &mut item,
                    )?;
                    audio = BTreeMap::from([(0,0),(1,1)]);
                    stereo_pool_route = true;
                } else {
                    audio = audio_media(
                        clip,
                        node,
                        &audio_codec,
                        &path,
                        start,
                        rate,
                        blobs,
                        graph,
                        &mut item,
                    )?;
                }
            }
            (Some(first), Some(second)) if entries.next().is_none() => {
                if codec != "dnxhd_lb"
                    || rate != crate::prepare::rate()
                    || size != (1920, 1080)
                    || saved_timecode.is_none()
                {
                    return Err("unsupported Resolve split MXF picture profile".into());
                }
                audio = atom::read_audio(
                    clip,
                    video,
                    &path,
                    [first.entry()?, second.entry()?],
                    start,
                    length,
                    blobs,
                    graph,
                    &mut item,
                )?;
                split_atom_audio = true;
            }
            (None, None) => {}
            _ => return Err("unsupported Resolve audio cache count".into()),
        }
    }
    receipt(
        graph,
        id,
        vec![
            ("kind", text("media_pool_clip")),
            ("cache_frames", Value::Int(length)),
            ("video_cache_count", Value::Int(start)),
        ],
    );
    metadata::capture(clip, video, saved_timecode, blobs, graph)?;
    if let Some(proxy) = proxy {
        graph.sidecars.push(graph::Sidecar { adapter:"resolve.proxy".into(),object:id.into(),data:proxy });
    }
    graph.items.push(item);
    Ok(Master {
        prores_hq: codec == "apch",
        camera_pcm,
        native_mono_movie,
        rate,
        length,
        start,
        audio,
        path,
        size,
        wav_samples: None,
        standalone_mono_atom: false,
        stereo_pool_route,
        native_stereo_pcm,
        split_atom_audio,
    })
}

fn native_mono_selectors(clip: &Node, blobs: &mut Blobs) -> Result<Option<[String; 2]>> {
    let Some(saved) = clip.maybe("VirtualAudioTracksBA")?.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let routes = blobs.qt(saved)?;
    if !routes.keys().eq(["0", "1"]) {
        return Ok(None);
    }
    let mut selectors = [String::new(), String::new()];
    for (index, key) in ["0", "1"].into_iter().enumerate() {
        let bytes = routes.bytes(key)?;
        let route = qt(bytes)?;
        if route.int("AudioType").ok() != Some(1)
            || route.bytes("ChannelsBA").ok() != Some(&[
                0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, index as u8 + 1,
            ][..])
        {
            return Ok(None);
        }
        selectors[index] = hex::encode(bytes);
    }
    Ok(Some(selectors))
}

pub(crate) fn imported_selectors(path: &Path, id: &str) -> Result<Option<[String; 2]>> {
    let members = xml::read(path)?;
    let root = members.get(ROOT_FOLDER).ok_or("missing native import Media Pool")?;
    for entry in root.one("MediaVec")?.elements()? {
        let item = entry.entry()?;
        if item.id()? == id {
            return native_mono_selectors(item, &mut Blobs::default());
        }
    }
    Err("native import Media Pool item is missing".into())
}

fn cache(node: &Node, blobs: &mut Blobs) -> Result<(String, String)> {
    let data = blobs.envelope(node.get("Clip")?)?;
    let get = |field| -> Result<&str> {
        Ok(std::str::from_utf8(
            pb_bytes(&data, field)?.ok_or("missing Resolve media cache field")?,
        )?)
    };
    let folder = get(1)?;
    let name = get(2)?;
    if folder.is_empty()
        || name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || matches!(name, "." | "..")
        || folder.contains('\0')
        || folder
            .len()
            .checked_add(name.len())
            .is_none_or(|n| n >= 32768)
    {
        return Err("invalid Resolve saved media locator".into());
    }
    let separator = if folder.ends_with(['/', '\\']) {
        ""
    } else if folder.contains('\\') && !folder.contains('/') {
        "\\"
    } else {
        "/"
    };
    Ok((format!("{folder}{separator}{name}"), get(5)?.into()))
}

fn add_media(graph: &mut SourceGraph, item: &mut SourceItem, media: SourceMedia) {
    item.media.push(SourceItemMedia {
        media: media.id.clone(),
        gain_db: 0.0,
        source_offset: Some(Rational::ZERO),
    });
    graph.media.push(media);
}

fn audio_routes(clip: &Node, blobs: &mut Blobs) -> Result<encoding::Map> {
    let fields = blobs.envelope(clip.get("FieldsBlob")?)?;
    let nested = pb_bytes(&fields, 1)?.ok_or("missing Resolve pool metadata")?;
    let nested = pb_bytes(nested, 1)?.ok_or("missing Resolve pool routing")?;
    qt(pb_bytes(nested, 5)?.ok_or("missing Resolve virtual audio routes")?)
}

fn movie_stereo_route(clip: &Node, blobs: &mut Blobs) -> Result<bool> {
    let routes = audio_routes(clip, blobs)?;
    if !routes.keys().eq(["0"]) {
        return Ok(false);
    }
    let route = qt(routes.bytes("0")?)?;
    Ok(route.int("Type").ok() == Some(0)
        && qt(route.bytes("ChannelVecBA")?)?.keys().eq(["0", "1"]))
}

fn proxy_observation(clip: &Node, blobs: &mut Blobs) -> Result<Option<Value>> {
    let Some(encoded) = clip.maybe("FieldsBlob")?.filter(|v|!v.is_empty()) else { return Ok(None); };
    let fields = blobs.envelope(encoded)?;
    let Some(bytes) = pb_bytes(&fields,2)? else { return Ok(None); };
    // Preserve older unrelated fields: only the observed attached-proxy cache is interpreted.
    let Ok(top) = qt(bytes) else { return Ok(None); };
    let Ok(video) = top.bytes("VIDEO_INFO").and_then(qt) else { return Ok(None); };
    let packed = blobs.envelope(&hex::encode(video.bytes("Clip")?))?;
    let get = |key| -> Result<&str> {
        Ok(std::str::from_utf8(pb_bytes(&packed,key)?.ok_or("missing proxy locator field")?)?)
    };
    let folder = get(1)?;
    let file = get(2)?;
    if folder.is_empty() || file.is_empty() || file.contains(['/', '\\','\0']) || matches!(file,"."|"..") {
        return Err("invalid Resolve proxy locator".into());
    }
    let path = Path::new(folder).join(file);
    let timing = qt(video.bytes("Time")?)?;
    let audio_bits = match top.byte_list("AUDIO_INFO_VEC")? {
        Some([row]) => {
            let audio = qt(row)?;
            let tracks = qt(audio.bytes("TracksBA")?)?;
            let track = qt(tracks.bytes("0")?)?;
            let packed_audio = blobs.envelope(&hex::encode(audio.bytes("Clip")?))?;
            if track.text("CodecName")? == Some("Linear PCM") && track.int("NumChannels")? == 2
                && pb_bytes(&packed_audio,1)? == Some(folder.as_bytes()) && pb_bytes(&packed_audio,2)? == Some(file.as_bytes())
            {
                match track.int("BitDepth")? {1=>Value::Int(16),2=>Value::Int(24),_=>Value::Null}
            } else { Value::Null }
        }
        _ => Value::Null,
    };
    Ok(Some(Value::Obj(vec![
        ("path",text(path.to_str().ok_or("invalid proxy path")?)),
        ("frames",Value::Int(timing.int("NumFrames")?)),
        ("start", Value::Int(timecode(timing.text("Timecode")?.ok_or("missing proxy timecode")?,24)?)),
        ("rate", text(&rate(timing.bytes("FrameRate")?)?.to_string())),
        ("audio_bits",audio_bits),
    ].into_iter().map(|(k,v)|(k.into(),v)).collect())))
}

fn read_embedded_stereo(
    clip: &Node,
    node: &Node,
    path: &str,
    length: i64,
    rate: Rational,
    start: i64,
    codec: &str,
    proxy_depth: Option<i64>,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
    item: &mut SourceItem,
) -> Result<()> {
    let tracks = blobs.qt(node.get("TracksBA")?)?;
    let routes = audio_routes(clip, blobs)?;
    if !tracks.keys().eq(["0"]) || !routes.keys().eq(["0"]) {
        return Err("expected one embedded movie audio track and stereo route".into());
    }
    let track = qt(tracks.bytes("0")?)?;
    let route = qt(routes.bytes("0")?)?;
    let depth = track.int("BitDepth")?;
    let pcm = match (codec, depth) {
        ("Linear PCM", 1) => Some(16),
        ("Linear PCM", 2) => Some(24),
        ("AAC", 3) => None,
        _ => return Err("unqualified embedded movie audio format or depth".into()),
    };
    if proxy_depth.is_some() && proxy_depth != pcm {
        return Err("embedded movie PCM depth differs from its proxy".into());
    }
    let samples = track.int("Duration")?;
    if samples <= 0 || proxy_depth.is_some()
        && samples != length.checked_mul(crate::linked::samples_per_frame(rate)?)
            .ok_or("embedded stereo sample extent overflow")?
    {
        return Err("invalid embedded stereo sample extent".into());
    }
    let duration = Rational::new(samples.into(), 48_000).ok_or("invalid embedded stereo extent")?;
    let route_extent = route.bytes("MediaExtents")?;
    let route_agrees = {
        let [origin, duration] = route_extent.as_chunks::<8>().0 else {
            return Err("invalid embedded stereo sample extent".into());
        };
        route_extent.len() == 16
            && frames(f64::from_le_bytes(*origin), rate)? == start
            && (f64::from_le_bytes(*duration) == samples as f64 / 48_000.0
                || extent(route_extent, rate).ok() == Some((start, length)))
    };
    if track.int("NumChannels")? != 2
        || track.int("IdxTrack")? != 0
        || track.int("SampleRate")? != 48000
        || (pcm.is_none() && track.int("ChannelLayout")? != 2)
        || track.text("CodecName")? != Some(codec)
        || frames(track.real("StartTime")?,rate)? != start
        || track.int("Duration")? != samples
        || route.int("Type")? != 0
        || route.text("CodecName")? != Some(codec)
        || (pcm.is_some() && route.int("BitDepth")? != depth)
        || !route_agrees
    {
        return Err("unqualified embedded stereo sample clock or format".into());
    }
    let channels = qt(route.bytes("ChannelVecBA")?)?;
    if !channels.keys().eq(["0", "1"]) {
        return Err("incomplete embedded stereo channel routing".into());
    }
    for (channel, key) in ["0", "1"].into_iter().enumerate() {
        let row = qt(channels.bytes(key)?)?;
        let saved = if row.keys().any(|k| k == "ChannelIdx") {
            row.int("ChannelIdx")?
        } else {
            0
        };
        if row.text("MediaRef")? != Some(node.id()?) || saved != channel as i64 {
            return Err("embedded stereo selector contradicts its cache".into());
        }
        let label = channel as i64 + 1;
        let stream_id = format!("{}/audio/{label}", clip.id()?);
        if pcm.is_some() && proxy_depth.is_none() {
            receipt(graph, &stream_id, vec![
                ("saved_cache_track", Value::Int(0)),
                ("saved_channel", Value::Int(saved)),
            ]);
        }
        add_media(
            graph,
            item,
            SourceMedia {
                id: stream_id,
                kind: StreamKind::Audio,
                locations: vec![path.into()],
                codec: Some(match pcm {
                    Some(bits) if proxy_depth.is_some() => format!("pcm_s{bits}le"),
                    Some(_) => "pcm".into(),
                    None => "aac".into(),
                }),
                frame_size: None,
                rate: None,
                sample_rate: Some(48000),
                bits: pcm,
                duration,
                start: Some(Rational::ZERO),
                clock: None,
                stream: Some(label),
                online: None,
                original: None,
            },
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn audio_media(
    clip: &Node,
    node: &Node,
    codec: &str,
    path: &str,
    start: i64,
    picture_rate: Rational,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
    item: &mut SourceItem,
) -> Result<BTreeMap<i64, i64>> {
    let track_map = blobs.qt(node.get("TracksBA")?)?;
    let routes = audio_routes(clip, blobs)?;
    let mut labels = BTreeMap::new();
    for selector in routes.keys() {
        let index: i64 = selector.parse()?;
        if !(0..32).contains(&index) {
            return Err("unsupported Resolve mono selector index".into());
        }
        let label = index + 1;
        let route = qt(routes.bytes(selector)?)?;
        let channels = qt(route.bytes("ChannelVecBA")?)?;
        if !channels.keys().eq(["0"]) {
            loss(
                graph,
                clip.id()?,
                "audio_routing",
                "unsupported non-mono Resolve pool route",
            );
            continue;
        }
        let channel = qt(channels.bytes("0")?)?;
        same(
            channel
                .text("MediaRef")?
                .ok_or("missing Resolve audio media reference")?,
            node.id()?,
            "virtual audio media",
        )?;
        let saved_track = if channel.keys().any(|k| k == "TrackIdx") {
            channel.int("TrackIdx")?
        } else {
            0
        };
        let track = qt(track_map.bytes(&saved_track.to_string())?)?;
        if track.int("IdxTrack")? != saved_track {
            return Err("Resolve audio track index contradicts cache key".into());
        }
        if track.int("NumChannels")? != 1 || codec != "Linear PCM" {
            loss(
                graph,
                clip.id()?,
                "audio_routing",
                "unsupported Resolve audio cache profile",
            );
            continue;
        }
        let hz = track.int("SampleRate")?;
        let samples = track.int("Duration")?;
        let bits = match track.int("BitDepth")? {
            2 => 24,
            _ => {
                loss(
                    graph,
                    clip.id()?,
                    "audio_routing",
                    "unqualified Resolve audio depth code",
                );
                continue;
            }
        };
        if hz <= 0 || samples <= 0 || frames(track.real("StartTime")?, picture_rate)? != start {
            return Err("invalid or contradictory Resolve audio extent".into());
        }
        let duration =
            Rational::new(samples.into(), hz.into()).ok_or("invalid Resolve audio duration")?;
        let route_extent = extent(route.bytes("MediaExtents")?, picture_rate)?;
        if route_extent.0 != start || seconds(route_extent.1, picture_rate)? != duration {
            return Err("contradictory Resolve mono route extent".into());
        }
        if labels.insert(index, saved_track).is_some() {
            return Err("duplicate Resolve selector index".into());
        }
        let id = format!("{}/audio/{label}", clip.id()?);
        receipt(
            graph,
            &id,
            vec![
                ("saved_selector", Value::Int(index)),
                ("saved_cache_track", Value::Int(saved_track)),
            ],
        );
        // The audio cache's start is expressed on the witnessed picture grid.
        // It is not a separately qualified file timecode track.
        add_media(
            graph,
            item,
            SourceMedia {
                id,
                kind: StreamKind::Audio,
                locations: vec![path.into()],
                codec: Some("pcm".into()),
                frame_size: None,
                rate: None,
                sample_rate: Some(hz),
                bits: Some(bits),
                duration,
                start: Some(seconds(start, picture_rate)?),
                clock: None,
                stream: Some(label),
                online: None,
                original: None,
            },
        );
    }
    Ok(labels)
}

fn seconds(count: i64, rate: Rational) -> Result<Rational> {
    Rational::new(count as i128 * rate.den() as i128, rate.num() as i128)
        .ok_or_else(|| "Resolve time exceeds graph limits".into())
}

fn clock(id: &str, field: &str, count: i64, rate: Rational) -> Result<SourceClock> {
    let count = u64::try_from(count)?;
    Ok(SourceClock {
        effective_count: count,
        rate,
        display_base: crate::linked::display_base(rate),
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: count,
            source_offset: 0,
            source_rate: rate,
            source_package: id.into(),
            timecode_track: field.into(),
            route: CLOCK_ROUTE.into(),
        },
    })
}

#[derive(Clone, Copy)]
struct Grid {
    rate: Rational,
    drop: bool,
    origin: i64,
    length: i64,
    size: (i64, i64),
}

impl Grid {
    fn read(node: &Node) -> Result<Self> {
        let (rate, drop) = frame_rate(&hex_bytes(node.get("FrameRate")?)?)?;
        let (origin, length) = extent(&hex_bytes(node.get("MediaExtents")?)?, rate)?;
        Ok(Self {
            rate,
            drop,
            origin,
            length,
            size: raster(&hex_bytes(node.get("Resolution")?)?)?,
        })
    }

    fn clock(self, id: &str) -> Result<SourceClock> {
        let mut clock = clock(id, "Sm2Sequence.MediaExtents", self.origin, self.rate)?;
        clock.drop = self.drop;
        Ok(clock)
    }

    fn edit(self, node: &Node, wav: bool) -> Result<Edit> {
        self.audio_edit(node, wav, false)
    }

    fn audio_edit(self, node: &Node, wav: bool, standalone_mono_atom: bool) -> Result<Edit> {
        let duration = node.get("Duration")?;
        let length = if standalone_mono_atom {
            wav::integer_atom_frame(duration)?
        } else if wav {
            wav::integer_wav_frame(duration)?
        } else {
            duration.parse()?
        };
        let source_in = match node.get("In")? {
            "" => 0, // Native serialization of default zero.
            text if standalone_mono_atom => wav::integer_atom_frame(text)?,
            text if wav => wav::integer_wav_frame(text)?,
            text => text.parse::<i64>()?,
        };
        self.edit_at(node, length, source_in)
    }

    fn edit_at(self, node: &Node, length: i64, source_in: i64) -> Result<Edit> {
        let start: i64 = node.get("Start")?.parse()?;
        let start = start
            .checked_sub(self.origin)
            .ok_or("Resolve record start overflow")?;
        if start < 0
            || length <= 0
            || source_in < 0
            || start
                .checked_add(length)
                .is_none_or(|end| end > self.length)
            || source_in.checked_add(length).is_none()
        {
            return Err("Resolve edit lies outside its declared sequence extent".into());
        }
        Ok(Edit {
            start,
            length,
            source_in,
        })
    }
}

#[derive(Clone, Copy)]
struct Edit {
    start: i64,
    length: i64,
    source_in: i64,
}

fn sequence<'a>(
    context: &mut Context<'a>,
    handle: &'a Node,
    pool_clip: &Node,
    bin_id: &str,
) -> Result<SourceSequence> {
    let node = handle.one("Sequence")?.entry()?;
    expect(node, "Sm2Sequence")?;
    same(node.get("Parent")?, handle.id()?, "timeline sequence owner")?;
    let id = node.id()?;
    let grid = Grid::read(node)?;
    let clock = grid.clock(id)?;
    let mut out = SourceSequence {
        disabled_placements: Vec::new(),
        id: id.into(),
        name: handle.get("Name")?.into(),
        rate: grid.rate,
        frame_size: Some(grid.size),
        start: clock.exact_start(),
        clock: Some(clock),
        tracks: vec![],
        placements: vec![],
        transitions: vec![],
        markers: vec![],
        texts: vec![],
    };
    let mut mixer = MixerControls::default();
    let mut inherited_input_scaling = None;
    if let Some(container) = context.container(id, grid.length)? {
        let can_inherit_nearest = context.nearest_default && supported_marker_rate(grid.rate);
        let can_inherit_sizing = context.input_scaling_default.is_some()
            && !context.inspect_drt && grid.size == (1920, 1080);
        let inherits_project = (can_inherit_nearest || can_inherit_sizing)
            && node.maybe("FieldsBlob")?
                .and_then(|saved| context.blobs.qt(saved).ok())
                .is_some_and(|map| !map.keys().any(|key| key == "SequenceSetup"));
        let default_nearest = can_inherit_nearest && inherits_project;
        let default_input_fit =
            context.input_scaling_default == Some(crate::input_scaling::FIT) && default_nearest;
        let track_flags = read_tracks(context, container, grid, &mut out, default_nearest, default_input_fit)?;
        if inherits_project && can_inherit_sizing
            && out.placements.iter().any(|edit| {
                edit.video_track.is_some()
                    && context.masters.get(edit.item.as_str())
                        .is_some_and(|master| master.size != grid.size && master.rate == grid.rate)
            })
        {
            inherited_input_scaling = context.input_scaling_default;
        }
        mixer = read_mixer_controls(context, node, &mut out, track_flags)?;
    }
    let mut fields = vec![
        ("kind", text("registered_timeline")),
        ("timeline_handle", text(handle.id()?)),
        ("media_pool_clip", text(handle.get("MpTimelineClip")?)),
        ("bin_id", text(bin_id)),
        ("record_origin", Value::Int(grid.origin)),
        ("record_length", Value::Int(grid.length)),
    ];
    if let Some(mode) = inherited_input_scaling {
        fields.push((crate::input_scaling::PROJECT_SETTING, text(mode)));
    }
    let mut marks = Vec::new();
    for (tag, key) in crate::timeline_marks::TAGS {
        if let Some(value) = pool_clip.maybe(tag)? {
            marks.push((key.into(), text(value)));
        }
    }
    if marks.iter().any(|(_, value)| value != &text("")) {
        fields.push((crate::timeline_marks::FIELD, Value::Obj(marks)));
        loss(
            &mut context.graph, id, crate::timeline_marks::LOSS.0, crate::timeline_marks::LOSS.1,
        );
    }
    if let Some(ticks) = mixer.main_bus {
        fields.push((mixer::MAIN_BUS_LEVEL, Value::Float(f64::from(ticks) / 10.0)));
        loss(
            &mut context.graph,
            id,
            mixer::MAIN_BUS_LOSS.0,
            mixer::MAIN_BUS_LOSS.1,
        );
    }
    if mixer.mutes.iter().any(|&muted| muted) {
        let tracks = (1..=2)
            .zip(mixer.mutes)
            .filter_map(|(index, muted)| muted.then_some(Value::Int(index)))
            .collect();
        fields.push((mixer::MUTED_AUDIO_TRACKS, Value::Arr(tracks)));
        loss(
            &mut context.graph,
            id,
            mixer::TRACK_MUTE_LOSS.0,
            mixer::TRACK_MUTE_LOSS.1,
        );
    }
    receipt(&mut context.graph, id, fields);
    Ok(out)
}

#[derive(Default)]
struct MixerControls {
    main_bus: Option<i32>,
    mutes: [bool; 2],
}

fn read_mixer_controls(
    context: &mut Context<'_>,
    node: &Node,
    out: &mut SourceSequence,
    track_flags: Option<[bool; 2]>,
) -> Result<MixerControls> {
    let typed_mixer_profile = out.tracks.iter().map(|t| (t.kind, t.index)).eq([
        (StreamKind::Video, 1),
        (StreamKind::Audio, 1),
        (StreamKind::Audio, 2),
    ]);
    let tracks: Vec<_> = out
        .tracks
        .iter()
        .filter_map(|track| (track.kind == StreamKind::Audio).then_some(track.index))
        .collect();
    if tracks.is_empty() {
        return Ok(MixerControls::default());
    }
    let mixer = node
        .maybe("FieldsBlob")?
        .map(Mixer::decode)
        .transpose()?
        .flatten();
    let stereo = out.placements.iter().any(crate::stereo::placement);
    let flags_muted = track_flags.is_some_and(|flags| flags != [false; 2]);
    let track_state = crate::track_state::TrackState::prepare(&context.graph, out).ok().flatten();
    let Some(mixer) = mixer else {
        if stereo {
            loss(&mut context.graph, &out.id, mixer::FAIRLIGHT_LOSS.0, mixer::FAIRLIGHT_LOSS.1);
        }
        if flags_muted {
            unqualified_mute(&mut context.graph, &out.id);
        }
        if track_state.as_ref().is_some_and(|state| {
            state.0[1..].iter().any(|track| !track.name.is_empty() || track.locked)
        }) {
            loss(&mut context.graph, &out.id, "track_state",
                "Resolve timeline audio track state has no saved mixer mirror");
        }
        return Ok(MixerControls::default());
    };
    context.blobs.charge(mixer.decoded_bytes())?;
    if stereo && mixer.neutral_stereo(&tracks) {
        return Ok(MixerControls::default());
    }
    let metadata = if typed_mixer_profile && (track_state.is_some() || mixer.has_track_state()) {
        let mirrors = mixer.track_state();
        let matches = mirrors.as_ref().zip(track_state.as_ref()).is_some_and(|((audio, video, _), state)| {
            audio == &state.0[1..] && video.as_ref().is_none_or(|v| {
                v[0] == state.0[0] && v[1] == crate::track_state::Track::default()
            })
        });
        if !matches {
            loss(&mut context.graph, &out.id, "track_state",
                "Resolve timeline and mixer track state disagree");
        }
        mirrors.filter(|_| matches).map(|(_, _, normalized)| normalized)
    } else {
        None
    };
    let qualified_mixer = metadata.as_ref().unwrap_or(&mixer);
    let linked_pans = mixer.linked_track_pans(track_flags.filter(|_| track_state.is_none()));
    let typed_gain_mute = typed_mixer_profile
        && (linked_pans.is_some() || qualified_mixer.qualified(track_flags));
    let mut requires_mapping = stereo;
    if let Some((mut data, active_controls)) = mixer.source_facts(&tracks, typed_gain_mute, metadata.as_ref()) {
        requires_mapping |= active_controls;
        if let (Some(pans), Value::Obj(fields)) = (linked_pans, &mut data) {
            fields.push((mixer::LINKED_TRACK_PANS.into(), Value::Arr(
                pans.into_iter().map(|pan| Value::Int(i64::from(pan))).collect(),
            )));
        }
        context.graph.sidecars.push(Sidecar {
            adapter: mixer::FAIRLIGHT_ADAPTER.into(),
            object: out.id.clone(),
            data,
        });
    }
    if requires_mapping {
        loss(&mut context.graph, &out.id, mixer::FAIRLIGHT_LOSS.0, mixer::FAIRLIGHT_LOSS.1);
    }
    if !typed_mixer_profile {
        return Ok(MixerControls::default());
    }
    let faders = qualified_mixer.track_faders().unwrap_or_default();
    let main_bus = qualified_mixer.main_bus_fader().unwrap_or_default();
    let has_mute_state = flags_muted || qualified_mixer.has_mute_state();
    if faders == [0, 0] && main_bus == 0 && !has_mute_state {
        return Ok(MixerControls::default());
    }
    let mutes = qualified_mixer.track_mutes();
    if !typed_gain_mute
        || (has_mute_state && (mutes.is_none() || mutes != track_flags))
        || !faders.into_iter().all(mixer::valid_ticks)
        || !mixer::valid_ticks(main_bus)
        || context
            .graph
            .multicam_uses
            .iter()
            .any(|use_| use_.sequence == out.id)
    {
        if faders != [0, 0] {
            loss(
                &mut context.graph,
                &out.id,
                "track_volume",
                "unqualified Resolve track faders or mixer state",
            );
        }
        if main_bus != 0 {
            loss(
                &mut context.graph,
                &out.id,
                "bus_volume",
                "unqualified Resolve main bus fader or mixer state",
            );
        }
        if has_mute_state {
            unqualified_mute(&mut context.graph, &out.id);
        }
        return Ok(MixerControls::default());
    }
    for (track, ticks) in out.tracks[1..].iter_mut().zip(faders) {
        if ticks != 0 {
            track.level_db = Some(f64::from(ticks) / 10.0);
        }
    }
    Ok(MixerControls {
        main_bus: (main_bus != 0).then_some(main_bus),
        mutes: mutes.unwrap_or_default(),
    })
}

fn unqualified_mute(graph: &mut SourceGraph, id: &str) {
    loss(
        graph,
        id,
        mixer::TRACK_MUTE_LOSS.0,
        "unqualified Resolve track mute or mixer state",
    );
}

fn read_tracks<'a>(
    context: &mut Context<'a>,
    container: &'a Node,
    grid: Grid,
    out: &mut SourceSequence,
    default_nearest: bool,
    default_input_fit: bool,
) -> Result<Option<[bool; 2]>> {
    let tracks = tracks(container)?;
    let stereo_layout = crate::stereo::clock(grid.rate, grid.size)
        && match tracks.as_slice() {
            [(StreamKind::Video, _), (StreamKind::Audio, _)] => true,
            [(StreamKind::Video, _), (StreamKind::Audio, _), (StreamKind::Audio, a2)] =>
                a2.get("SubType")? == "1" && a2.get("Flags")? == "0"
                    && a2.maybe("UserDefinedName")?.unwrap_or("").is_empty(),
            _ => false,
        };
    let track_flags = match tracks.as_slice() {
        [(StreamKind::Video, v), (StreamKind::Audio, a1), (StreamKind::Audio, a2)]
            if matches!(v.get("Flags")?, "0" | "1" | "2" | "3") && a1.get("SubType")? == "1" && a2.get("SubType")? == "1" =>
        {
            match (a1.get("Flags")?, a2.get("Flags")?) {
                (a @ ("0" | "1" | "2" | "3"), b @ ("0" | "1" | "2" | "3")) =>
                    Some([matches!(a, "2" | "3"), matches!(b, "2" | "3")]),
                _ => None,
            }
        }
        _ => None,
    };
    let mut video = 0;
    let mut audio = 0;
    let mut has_track_state = false;
    for (kind, track) in tracks {
        let counter = if kind == StreamKind::Video {
            &mut video
        } else {
            &mut audio
        };
        *counter += 1;
        let index = *counter;
        let track_id = track.id()?;
        claim(&mut context.ids, track_id)?;
        out.tracks.push(SourceTrack::new(kind, index, None));
        let flags = track.get("Flags")?;
        let name = track.maybe("UserDefinedName")?.unwrap_or("");
        let locked = matches!(flags, "1" | "3");
        let disabled = kind == StreamKind::Video && matches!(flags, "2" | "3");
        let stereo = stereo_layout && kind == StreamKind::Audio && index == 1 && flags == "0" && name.is_empty()
            && track.get("SubType")? == "0"
            && track.maybe("FieldsBlob")? == Some(crate::captions::TRACK_FIELDS)
            && !track.one("Items")?.children.is_empty();
        if !name.is_empty() || locked || disabled {
            has_track_state = true;
            observations::append(&mut context.graph, crate::track_state::ADAPTER, &out.id,
                Value::Obj(vec![
                    ("id".into(), text(track_id)),
                    ("kind".into(), text(kind.as_str())),
                    ("track".into(), Value::Int(index)),
                    ("name".into(), text(name)),
                    ("locked".into(), Value::Bool(locked)),
                    ("disabled".into(), Value::Bool(disabled)),
                ]));
        }
        let supported = (matches!(flags, "0" | "1") || disabled
            || (matches!(flags, "2" | "3") && kind == StreamKind::Audio && track_flags.is_some()))
            && (kind == StreamKind::Video || stereo || track.get("SubType")? == "1");
        if !supported {
            let mut track_facts = vec![
                ("id".into(), text(track_id)),
                ("kind".into(), text(kind.as_str())),
                ("track".into(), Value::Int(index)),
            ];
            if kind == StreamKind::Audio && flags == "0" && name.is_empty() && track.get("SubType")? == "0"
                && track.maybe("FieldsBlob")? == Some(crate::captions::TRACK_FIELDS)
                && track.one("Items")?.children.is_empty()
            {
                track_facts.push(("empty_default_stereo".into(), Value::Bool(true)));
            }
            observations::append(&mut context.graph, OMITTED_TRACKS_ADAPTER, &out.id,
                Value::Obj(track_facts));
            omitted(
                &mut context.graph,
                &out.id,
                track_id,
                "track",
                EMPTY_STEREO,
            );
        }
        let mut entries = track.one("Items")?.elements()?.peekable();
        let mut previous = None;
        while let Some(entry) = entries.next() {
            let node = entry.entry()?;
            let before = previous.replace(node);
            let id = node.id()?;
            claim(&mut context.ids, id)?;
            if node.name == "Sm2TiTransition" {
                observations::transition(
                    &mut context.graph,
                    node,
                    &out.id,
                    track_id,
                    kind,
                    index,
                    grid,
                    [
                        before,
                        entries.peek().map(|entry| entry.entry()).transpose()?,
                    ],
                    context.inspect_drt,
                )?;
                continue;
            }
            if supported && kind == StreamKind::Video && node.name == "Sm2TiGenerator"
                && node.maybe("PrettyType")? == Some("Solid Color") && node.maybe("Flags")? == Some("0")
                && node.maybe("FieldsBlob")?.is_none_or(str::is_empty)
                && node.maybe("MarkersBA")?.is_none_or(str::is_empty)
            {
                if let Some(rgb) = crate::generator::rgb(node.maybe("EffectFiltersBA")?.unwrap_or_default()) {
                    let edit = grid.edit(node, false)?;
                    let solid = crate::generator::Solid { start: edit.start, duration: edit.length, rgb };
                    observations::append(&mut context.graph, crate::generator::ADAPTER, &out.id, solid.receipt(id, index));
                    continue;
                }
            }
            if supported && kind == StreamKind::Video && node.name == "Sm2TiVideoClip"
                && node.maybe("PrettyType")? == Some("Fusion Title")
                && node.maybe("Flags")? == Some("0")
                && node.optional("CompositionTable")?.is_some()
            {
                title::capture(&mut context.graph, out, node, grid, index, &mut context.blobs)?;
                continue;
            }
            if !supported || !ordinary_item(node, kind, &out.id, &mut context.graph)? {
                continue;
            }
            if context.inspect_drt {
                let mut missing = Vec::new();
                for field in [
                    "Start",
                    "Duration",
                    "In",
                    "MediaRef",
                    "MediaFrameRate",
                    "MediaStartTime",
                    "MediaTimemapBA",
                ] {
                    if node.maybe(field)?.is_none_or(|value| field != "In" && value.is_empty()) {
                        missing.push(field);
                    }
                }
                if !missing.is_empty() {
                    omitted(
                        &mut context.graph,
                        &out.id,
                        id,
                        "placement",
                        &format!("missing saved Resolve fields: {}", missing.join(", ")),
                    );
                    continue;
                }
            }
            let target = node.get("MediaRef")?;
            if let Some(group) = context.groups.get(target) {
                if stereo {
                    omitted(&mut context.graph, &out.id, id, "audio_routing",
                        "Resolve stereo multicam track is outside the linked PCM profile");
                    continue;
                }
                let edit = grid.edit(node, false)?;
                multicam::placement(
                    group,
                    node,
                    grid,
                    edit,
                    kind,
                    index,
                    out,
                    &mut context.graph,
                    &mut context.blobs,
                )?;
            } else if let Some(master) = context.masters.get(target) {
                if master.rate != grid.rate
                    && !((kind == StreamKind::Video && index == 1
                        || kind == StreamKind::Audio && master.camera_pcm)
                        && crate::mixed_rate::Cadence::for_resolve_source(master.rate, grid.rate).is_some())
                {
                    record_unqualified_timing(
                        &mut context.graph,
                        node,
                        &out.id,
                        master.rate,
                        grid.rate,
                        &mut context.blobs,
                    )?;
                    continue;
                }
                if context.inspect_drt
                    && kind == StreamKind::Audio
                    && (node.maybe("VirtualAudioTrackBA")?.is_none_or(str::is_empty)
                        || node.maybe("MediaTrackIdx")?.is_none_or(str::is_empty))
                {
                    omitted(
                        &mut context.graph,
                        &out.id,
                        id,
                        "audio_routing",
                        "missing saved Resolve audio selector",
                    );
                    continue;
                }
                if let Some(p) = direct(
                    master,
                    node,
                    grid,
                    kind,
                    index,
                    stereo,
                    default_nearest,
                    default_input_fit,
                    &out.id,
                    &mut context.graph,
                    &mut context.blobs,
                )? {
                    out.placements.push(p);
                }
            } else if let Some(&(child, class)) = context.nested.get(target) {
                let edit = grid.edit(node, false)?;
                let child_grid = Grid::read(child)?;
                let plain = kind == StreamKind::Video && index == 1
                    && grid.rate == Rational::integer(24) && child_grid.rate == grid.rate
                    && child_grid.length > 0
                    && child_grid.size == grid.size
                    && node.maybe("FieldsBlob")? == Some(nests::ITEM_FIELDS)
                    && node.maybe("MediaStartTime")?.and_then(|v| v.parse::<f64>().ok()) == Some(child_grid.origin as f64 / 24.0)
                    && node.maybe("MediaFrameRate")? == Some("00000000000038400000000000000000")
                    && node.maybe("MediaTimemapBA")?.is_some_and(|v| nests::identity_map(v, child_grid.length))
                    && node.maybe("MixedFrameRateAlignment")? == Some("0")
                    && node.maybe("CurrentSelectorIdx")? == Some("0")
                    && node.maybe("WasDisbanded")? == Some("false")
                    && node.maybe("IsForceConformed")? == Some("true")
                    && node.maybe("IsPreConformed")? == Some("false")
                    && !node.children.iter().any(|f| matches!(f.name.as_str(),
                        "pLmVerTable" | "CompositionTable" | "VirtualAudioTrackBA"))
                    && ["EffectFiltersBA", "LinkedItemSync", "MarkersBA", "PrettyType",
                        "ImportExportMetadataBA", "MediaFilePath", "MediaReelNumber", "MediaMetadata"]
                        .into_iter().try_fold(true, |clean, field| {
                            Ok::<_, Box<dyn std::error::Error>>(clean && node.maybe(field)?.is_none_or(str::is_empty))
                        })?;
                observations::append(&mut context.graph, NESTS_ADAPTER, &out.id, Value::Obj(vec![
                    ("profile".into(), if plain { text(nests::PROFILE) } else { Value::Null }),
                    ("id".into(), text(id)),
                    ("kind".into(), text(class)),
                    ("track_kind".into(), text(kind.as_str())),
                    ("track".into(), Value::Int(index)),
                    ("pool_id".into(), text(target)),
                    ("child_sequence".into(), text(child.id()?)),
                    ("record_start_frames".into(), Value::Int(edit.start)),
                    ("record_duration_frames".into(), Value::Int(edit.length)),
                    ("saved_source_in_frames".into(), Value::Int(edit.source_in)),
                    ("child_origin_frames".into(), Value::Int(child_grid.origin)),
                    ("child_rate".into(), text(&child_grid.rate.to_string())),
                    ("record_rate".into(), text(&grid.rate.to_string())),
                ]));
                omitted(&mut context.graph, &out.id, id, "placement",
                    nests::LOSS);
            } else if context.pool_items.contains(target) {
                omitted(
                    &mut context.graph,
                    &out.id,
                    id,
                    "media",
                    "edit refers to an unsupported Resolve pool item",
                );
            } else if context.inspect_drt {
                omitted(
                    &mut context.graph,
                    &out.id,
                    id,
                    "media",
                    "missing Resolve pool record",
                );
            } else {
                return Err("unresolved Resolve placement media reference".into());
            }
        }
    }
    if has_track_state {
        let (aspect, reason) = crate::track_state::LOSS;
        loss(&mut context.graph, &out.id, aspect, reason);
    }
    read_captions(context, container.one("SubtitleTrackVec")?, grid, out)?;
    if !container.one("GeometryTrackVec")?.children.is_empty() {
        loss(&mut context.graph, &out.id, "tracks", "geometry tracks are outside the Resolve source profile");
    }
    Ok(track_flags)
}

fn read_captions<'a>(
    context: &mut Context<'a>,
    vector: &'a Node,
    grid: Grid,
    out: &mut SourceSequence,
) -> Result<()> {
    if vector.children.iter().any(|child| child.name != "Element") {
        loss(&mut context.graph, &out.id, "tracks", "unsupported Resolve subtitle track");
        return Ok(());
    }
    let mut entries = vector.elements()?;
    let Some(entry) = entries.next() else { return Ok(()) };
    let track = entry.entry()?;
    if entries.next().is_some()
        || track.name != "Sm2TiTrack"
        || track.maybe("Type")? != Some("2")
        || track.maybe("SubType")? != Some("0")
        || track.maybe("Flags")? != Some("0")
        || track.maybe("Sequence")? != Some(out.id.as_str())
        || track.maybe("FieldsBlob")? != Some(crate::captions::TRACK_FIELDS)
    {
        loss(&mut context.graph, &out.id, "tracks", "unsupported Resolve subtitle track");
        return Ok(());
    }
    claim(&mut context.ids, track.id()?)?;
    let mut end = 0;
    for entry in track.one("Items")?.elements()? {
        let cue = entry.entry()?;
        let id = cue.id()?;
        claim(&mut context.ids, id)?;
        if cue.name != "Sm2TiGenerator" || cue.maybe("PrettyType")? != Some("Subtitle")
            || cue.maybe("Flags")? != Some("0")
            || cue.maybe("RenderTextEnabled")? != Some("true")
            || cue.maybe("FieldsBlob")?.is_some_and(|value| !value.is_empty())
            || cue.maybe("EffectFiltersBA")?.is_some_and(|value| !value.is_empty())
        {
            omitted(&mut context.graph, &out.id, id, "text", "unsupported Resolve subtitle cue");
            continue;
        }
        let text = cue.get("Name")?;
        let start = cue.get("Start")?.parse::<i64>()?.checked_sub(grid.origin)
            .ok_or("Resolve subtitle start overflow")?;
        let duration: i64 = cue.get("Duration")?.parse()?;
        if start < end || duration <= 0 || !crate::captions::plain_text(text) {
            return Err(format!("invalid or overlapping Resolve subtitle {id}").into());
        }
        end = start.checked_add(duration).filter(|end| *end <= grid.length)
            .ok_or("Resolve subtitle exceeds timeline extent")?;
        out.texts.push(graph::source::SourceText {
            id: id.into(),
            kind: graph::source::TextKind::Caption,
            track: 1,
            content: Some(text.into()),
            record_start: Some(seconds(start, grid.rate)?),
            duration: Some(seconds(duration, grid.rate)?),
            reason: None,
        });
    }
    Ok(())
}

fn ordinary_item(
    node: &Node,
    kind: StreamKind,
    owner: &str,
    graph: &mut SourceGraph,
) -> Result<bool> {
    let expected = if kind == StreamKind::Video {
        "Sm2TiVideoClip"
    } else {
        "Sm2TiAudioClip"
    };
    if node.name != expected || node.get("Flags")? != "0" {
        omitted(
            graph,
            owner,
            node.id()?,
            "placement",
            "unsupported Resolve edit type or clip state",
        );
        return Ok(false);
    }
    if node
        .maybe("MarkersBA")?
        .is_some_and(|value| !value.is_empty())
    {
        omitted(
            graph,
            owner,
            node.id()?,
            "marker",
            "Resolve clip marker ownership or clock is unqualified",
        );
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn direct(
    master: &Master,
    node: &Node,
    grid: Grid,
    kind: StreamKind,
    track: i64,
    stereo: bool,
    default_nearest: bool,
    default_input_fit: bool,
    owner: &str,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<Option<Placement>> {
    use observations::PictureTiming;
    let timing = if kind == StreamKind::Video
        && track == 1
        && grid.rate == Rational::integer(24)
        && master.rate == grid.rate
        && rate(&hex_bytes(node.get("MediaFrameRate")?)?)? == master.rate
        && node.maybe("EffectFiltersBA")? == Some(crate::retime::NEAREST)
    {
        observations::constant_picture_timing(node.get("MediaTimemapBA")?, master.length, blobs)?
    } else {
        None
    };
    let mixed = match crate::mixed_rate::Cadence::for_resolve_source(master.rate, grid.rate) {
        Some(cadence) if (kind == StreamKind::Video && track == 1
            && (master.camera_pcm && default_input_fit || master.prores_hq && master.start == 0 && master.audio.is_empty()
                && master.size == (1920, 1080)) || kind == StreamKind::Audio && master.camera_pcm && !stereo)
            && grid.size == (1920, 1080)
            && Path::new(&master.path).extension().and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mov"))
            && node.maybe("EffectFiltersBA")?.is_none_or(str::is_empty)
            && node.maybe("MixedFrameRateAlignment")? == Some("0")
            && rate(&hex_bytes(node.get("MediaFrameRate")?)?)? == master.rate
            && (hex_bytes(node.get("MediaTimemapBA")?)? == cadence.time_map(master.length)
                || kind == StreamKind::Audio && identity_timemap(node, master.length, master.rate)?) =>
        {
            match cadence.resolve_trim(node.get("In")?, default_nearest) {
                Some(crate::mixed_rate::ResolveTrim::Adjusted(..)) if master.camera_pcm => None,
                other => other,
            }
        }
        _ => None,
    };
    let mut audio_in = None;
    let (edit, speed, sampled) = match (mixed, timing) {
        (Some(crate::mixed_rate::ResolveTrim::Adjusted(cadence, source_in)), _) => {
            let length = node.get("Duration")?.parse()?;
            let edit = grid.edit_at(node, length, source_in)?;
            let last = cadence.last_frame(source_in, length)
                .ok_or("invalid Resolve mixed-rate trim")?;
            leaf_range_matches(master, node, last, 1, owner, graph)?;
            record_saved_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
            let mut total = 0;
            let map = cadence.capture(source_in, length, &mut total)?;
            blobs.charge(total * size_of::<i64>())?;
            (edit, None, Some((seconds(source_in, grid.rate)?, map)))
        }
        (Some(crate::mixed_rate::ResolveTrim::DefaultConform(trim)), _) => {
            let length = node.get("Duration")?.parse()?;
            // The timeline shape is on the record clock. The placement's
            // exact trim is set separately on its native file clock.
            let edit = grid.edit_at(node, length, 0)?;
            let last = trim.last_frame(length).ok_or("invalid Resolve default conform duration")?;
            leaf_range_matches(master, node, last, 1, owner, graph)?;
            record_saved_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
            if kind == StreamKind::Audio {
                let source_in = trim.source_in();
                let extent = seconds(master.length, master.rate)?;
                if source_in.checked_add(seconds(length, grid.rate)?)
                    .is_none_or(|end| end > extent)
                {
                    return Err("Resolve embedded audio edit exceeds its PCM sample extent".into());
                }
                audio_in = Some(source_in);
                (edit, None, None)
            } else {
                blobs.charge(usize::try_from(length)? * size_of::<i64>())?;
                (edit, None, Some((trim.source_in(), trim.capture(length))))
            }
        }
        (None, Some(PictureTiming::Speed(speed))) => {
            let length = node.get("Duration")?.parse()?;
            let source_in = speed.file_source_in(node.get("In")?, length, master.length)
                .ok_or("invalid Resolve retime in-point")?;
            let edit = grid.edit_at(node, length, source_in)?;
            record_saved_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
            leaf_range_matches(
                master, node, edit.source_in, speed.sampled_source_frames(edit.length)?,
                owner, graph,
            )?;
            (edit, Some(speed), None)
        }
        (None, Some(PictureTiming::Hold(frame))) if master.prores_hq && master.start == 0
            && master.audio.is_empty() && master.size == (1920, 1080) && grid.size == master.size
            && Path::new(&master.path).extension().and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mov"))
            && node.get("In")? == crate::retime::HOLD_IN =>
        {
            let length = node.get("Duration")?.parse()?;
            record_saved_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
            let Some(count) = crate::retime::hold_count(length) else {
                omitted(graph, owner, node.id()?, "timing",
                    "Resolve freeze has no supported whole-frame record length in graph/2");
                return Ok(None);
            };
            let edit = grid.edit_at(node, length, frame)?;
            leaf_range_matches(master, node, frame, 1, owner, graph)?;
            blobs.charge(count * size_of::<i64>())?;
            (edit, None, Some((seconds(frame, grid.rate)?, vec![frame; count])))
        }
        _ => {
            if master.rate != grid.rate {
                record_unqualified_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
                return Ok(None);
            }
            let edit = grid.audio_edit(
                node,
                kind == StreamKind::Audio && master.wav_samples.is_some(),
                kind == StreamKind::Audio && master.standalone_mono_atom,
            )?;
            if !leaf_matches(master, node, grid, edit, owner, graph, blobs)? {
                return Ok(None);
            }
            (edit, None, None)
        }
    };
    let audio = if stereo {
        if !master.native_stereo_pcm || (master.wav_samples.is_none() && master.size != grid.size)
            || node.maybe("FieldsBlob")? != Some(crate::stereo::CLIP_FIELDS)
            || node.get("VirtualAudioTrackBA")? != crate::stereo::SELECTOR
            || node.get("MediaTrackIdx")? != "0"
        {
            omitted(graph, owner, node.id()?, "audio_routing",
                "unqualified Resolve stereo PCM source or channel routing");
            return Ok(None);
        }
        None
    } else if kind == StreamKind::Audio {
        let selector = if master.camera_pcm || master.native_mono_movie {
            mono(node, blobs, true, master.native_mono_movie)?
        } else if master.stereo_pool_route {
            wav::stereo_channel(node, blobs)?
        } else {
            mono(node, blobs, false, false)?
        };
        let Some(selector) = selector else {
            omitted(
                graph,
                owner,
                node.id()?,
                "audio_routing",
                "unqualified Resolve clip audio selector",
            );
            return Ok(None);
        };
        let physical = if master.stereo_pool_route {
            Some(selector)
        } else {
            master.audio.get(&selector).copied()
        };
        let Some(physical) = physical else {
            omitted(
                graph,
                owner,
                node.id()?,
                "audio_routing",
                "Resolve selector lacks a qualified pool route",
            );
            return Ok(None);
        };
        Some(if master.wav_samples.is_some() {
            physical + 1
        } else {
            selector + 1
        })
    } else {
        None
    };
    let mut placement = placement(
        node.id()?,
        node.get("MediaRef")?,
        grid,
        edit,
        kind,
        track,
        audio,
    )?;
    if stereo {
        placement.audio_tracks.clear();
        placement.grouped_audio.push(crate::stereo::group());
    }
    if let Some(source_in) = audio_in {
        placement.source_in = source_in;
    }
    if kind == StreamKind::Video {
        capture_grade(node, &placement.id, graph, blobs)?;
    }
    if let Some(speed) = speed {
        placement.speed = speed.graph(edit.length);
        return Ok(Some(placement));
    }
    if let Some((source_in, sampled)) = sampled {
        placement.source_in = source_in;
        placement.frame_map = Some(sampled);
        if master.camera_pcm {
            graph.sidecars.push(Sidecar {
                adapter: "resolve.input_scaling".into(),
                object: placement.id.clone(),
                data: text("scale_to_fit"),
            });
        }
        return Ok(Some(placement));
    }
    if kind == StreamKind::Audio {
        if let Some(saved) = node.maybe("FieldsBlob")?.filter(|saved| !saved.is_empty()) {
            if let Some(data) = fairlight::clip_plugins(saved, blobs)? {
                graph.sidecars.push(Sidecar {
                    adapter: fairlight::CLIP_ADAPTER.into(),
                    object: placement.id.clone(),
                    data,
                });
                loss(graph, &placement.id, "clip_fairlight", fairlight::CLIP_LOSS);
            }
        }
    }
    if let Some(filters) = node
        .maybe("EffectFiltersBA")?
        .filter(|value| !value.is_empty())
    {
        let interpreted = match kind {
            // This edit already passed the same-rate identity map and range checks.
            StreamKind::Video if filters == crate::retime::NEAREST => true,
            StreamKind::Video if master.size != grid.size
                && matches!(filters, crate::input_scaling::CROP_FILTERS | crate::input_scaling::PROJECT_FILTERS) =>
            {
                let fields = blobs.envelope(node.get("FieldsBlob")?)?;
                let state = pb_bytes(&fields, 1)?.ok_or("missing Resolve edit state")?;
                let property = pb_bytes(state, 1)?.ok_or("missing Resolve clip sizing control")?;
                if !encoding::valid_clip_sizing_state(property)? {
                    return Err("Resolve sizing filters contradict the saved sizing control".into());
                }
                if filters == crate::input_scaling::CROP_FILTERS {
                    graph.sidecars.push(crate::input_scaling::receipt(
                        crate::input_scaling::RESOLVE, &placement.id, crate::input_scaling::CENTER_CROP,
                    ));
                }
                true
            }

            StreamKind::Video if grid.size == (1920, 1080) && master.size == grid.size => {
                match effects::picture_control(
                    filters, blobs, edit.source_in, edit.length, master.length,
                )? {
                    Some(crate::video::PictureControl::Transform(transform)) => {
                        placement.transform = Some(transform);
                        true
                    }
                    Some(crate::video::PictureControl::Reflection(reflection)) => {
                        placement.effects.push(reflection.readback());
                        true
                    }
                    Some(crate::video::PictureControl::ReflectedTransform(transform, reflection)) => {
                        placement.transform = Some(transform);
                        placement.effects.push(reflection.readback());
                        true
                    }
                    Some(crate::video::PictureControl::LinearMotion(motion)) => {
                        placement.effects.push(motion.readback());
                        true
                    }
                    Some(crate::video::PictureControl::LinearRoll(roll))
                        if grid.rate == Rational::integer(24) =>
                    {
                        placement.effects.push(roll.readback());
                        true
                    }
                    Some(crate::video::PictureControl::LinearRoll(_)) => false,
                    None => false,
                }
            }
            StreamKind::Audio => {
                if let Some(audio) = effects::clip_audio(filters, blobs, edit)? {
                    if let Some(pan) = audio.pan {
                        placement.track_pans = vec![Some(pan)];
                        let (aspect, reason) = crate::audio::CLIP_PAN_LOSS;
                        loss(graph, &placement.id, aspect, reason);
                    }
                    if let Some(envelope) = audio.envelope {
                        match envelope.volume {
                            Some(Volume::Static(db)) => placement.volume_db = Some(db),
                            Some(Volume::Keys(keys)) => {
                                for (frame, db) in keys {
                                    placement.volume_keys.push(graph::VolumeKey {
                                        at: seconds(i64::from(frame), grid.rate)?,
                                        db,
                                    });
                                }
                            }
                            None => {}
                        }
                        if let Some((aspect, reason)) = envelope.recovered_loss() {
                            receipt(graph, &placement.id, envelope.receipt());
                            loss(graph, &placement.id, aspect, reason);
                        }
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !interpreted {
            if kind == StreamKind::Audio {
                if let Some(data) = effects::custom_fades(filters, blobs, edit)? {
                    graph.sidecars.push(Sidecar {
                        adapter: crate::audio::UNQUALIFIED_FADE_ADAPTER.into(),
                        object: placement.id.clone(),
                        data,
                    });
                }
            }
            omitted(
                graph,
                owner,
                node.id()?,
                "effect",
                "Resolve clip effect controls are unqualified",
            );
        }
    }
    Ok(Some(placement))
}

fn capture_grade(node: &Node, id: &str, graph: &mut SourceGraph, blobs: &mut Blobs) -> Result<()> {
    let Some(wrapper) = node.optional("pLmVerTable")? else { return Ok(()) };
    if wrapper.children.is_empty() {
        return Ok(());
    }
    let table = wrapper.entry()?;
    expect(table, "ListMgt::LmVersionTable")?;
    let active = table.maybe("pActive")?.filter(|id| !id.is_empty());
    let Some(locals) = table.optional("Locals")? else {
        if active.is_some() {
            loss(graph, id, "grade", "Resolve grade has no supported local version");
        }
        return Ok(());
    };
    let versions = locals.elements()?.map(Node::entry).collect::<Result<Vec<_>>>()?;
    let saved = active.and_then(|id| versions.iter().find(|v| v.id().ok() == Some(id)));
    if let Some(version) = saved {
        if version.maybe("HasCorrection")? != Some("true") {
            return Ok(());
        }
        let body = version.get("Body")?;
        let bytes = hex_bytes(body)?;
        blobs.charge(bytes.len())?;
        if versions.len() == 1 && table.maybe("VerType")? == Some("0") {
            if let Some((gain, _)) = crate::grade::decode(&bytes) {
                graph.sidecars.push(Sidecar {
                    adapter: crate::grade::ADAPTER.into(),
                    object: id.into(),
                    data: Value::Obj(vec![
                        ("node_id".into(), Value::Int(1)),
                        ("version_id".into(), text(version.id()?)),
                        ("gain".into(), Value::Float(f64::from(gain))),
                        ("value_space".into(), text("resolve.drx")),
                        ("body".into(), text(body)),
                    ]),
                });
                return Ok(());
            }
            if let Some(slope) = crate::grade::decode_exposure_cdl(&bytes) {
                graph.sidecars.push(Sidecar {
                    adapter: crate::grade::CDL_ADAPTER.into(),
                    object: id.into(),
                    data: Value::Obj(vec![
                        ("node_id".into(), Value::Int(1)),
                        ("version_id".into(), text(version.id()?)),
                        ("slope".into(), Value::Float(f64::from(slope))),
                        ("value_space".into(), text("resolve.cdl")),
                    ]),
                });
                return Ok(());
            }
        }
        use sha2::{Digest, Sha256};
        receipt_grade_hash(graph, id, version.id()?, &hex::encode(Sha256::digest(&bytes)));
    } else if active.is_none() && versions.is_empty() {
        return Ok(());
    }
    loss(graph, id, "grade", "Resolve clip grade requires a destination mapping");
    Ok(())
}

fn receipt_grade_hash(graph: &mut SourceGraph, id: &str, version: &str, hash: &str) {
    graph.sidecars.push(Sidecar {
        adapter: crate::grade::OPAQUE_ADAPTER.into(),
        object: id.into(),
        data: Value::Obj(vec![
            ("version_id".into(), text(version)),
            ("body_sha256".into(), text(hash)),
        ]),
    });
}

fn record_unqualified_timing(
    graph: &mut SourceGraph,
    node: &Node,
    owner: &str,
    source_rate: Rational,
    record_rate: Rational,
    blobs: &mut Blobs,
) -> Result<()> {
    record_saved_timing(graph, node, owner, source_rate, record_rate, blobs)?;
    omitted(
        graph,
        owner,
        node.id()?,
        "timing",
        "unqualified Resolve rate or retime map",
    );
    Ok(())
}

fn record_saved_timing(
    graph: &mut SourceGraph,
    node: &Node,
    owner: &str,
    source_rate: Rational,
    record_rate: Rational,
    blobs: &mut Blobs,
) -> Result<()> {
    let id = node.id()?;
    let mut timing = vec![
        ("occurrence".into(), text(id)),
        ("source_rate".into(), text(&source_rate.to_string())),
        ("record_rate".into(), text(&record_rate.to_string())),
        ("saved_start".into(), text(node.get("Start")?)),
        ("saved_duration".into(), text(node.get("Duration")?)),
        ("saved_in".into(), text(node.get("In")?)),
    ];
    let map = node.get("MediaTimemapBA")?;
    if map.len() <= 8192 {
        timing.push(("saved_time_map".into(), text(map)));
    }
    timing.push(("map".into(), observations::time_map(map, blobs)?));
    observations::append(graph, "resolve.timing", owner, Value::Obj(timing));
    Ok(())
}

fn leaf_matches(
    master: &Master,
    node: &Node,
    grid: Grid,
    edit: Edit,
    owner: &str,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<bool> {
    let map_matches = match master.wav_samples {
        Some(samples) => wav::identity_timemap(node, samples, master.rate)?,
        None => identity_timemap(node, master.length, master.rate)?,
    };
    if master.rate != grid.rate
        || rate(&hex_bytes(node.get("MediaFrameRate")?)?)? != master.rate
        || !map_matches
    {
        record_unqualified_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
        return Ok(false);
    }
    leaf_range_matches(master, node, edit.source_in, edit.length, owner, graph)?;
    Ok(true)
}

fn leaf_range_matches(
    master: &Master,
    node: &Node,
    source_in: i64,
    source_length: i64,
    owner: &str,
    graph: &mut SourceGraph,
) -> Result<()> {
    if source_in < 0 || source_in.checked_add(source_length).is_none_or(|end| end > master.length)
        || frames(node.get("MediaStartTime")?.parse()?, master.rate)? != master.start
    {
        return Err("Resolve edit contradicts media cache clock or extent".into());
    }
    if let Some(saved) = node.maybe("MediaFilePath")?.filter(|x| !x.is_empty()) {
        if saved != master.path {
            omitted(
                graph,
                owner,
                node.id()?,
                "locator",
                "timeline locator differs from the Resolve Media Pool cache",
            );
        }
    }
    Ok(())
}

fn identity_timemap(node: &Node, length: i64, rate: Rational) -> Result<bool> {
    let bytes = hex_bytes(node.get("MediaTimemapBA")?)?;
    let [2, value @ ..] = bytes.as_slice() else {
        return Ok(false);
    };
    if value.len() != 8 {
        return Ok(false);
    }
    Ok(frames(f64::from_be_bytes(value.try_into()?), rate)? == length - 1)
}

fn mono(node: &Node, blobs: &mut Blobs, group: bool, embedded_movie: bool) -> Result<Option<i64>> {
    let virtual_track = blobs.qt(node.get("VirtualAudioTrackBA")?)?;
    if virtual_track.int("AudioType")? != 1 {
        return Ok(None);
    }
    let channel = virtual_track.bytes("ChannelsBA")?;
    if channel.len() != 12 || channel[..8] != [0, 0, 0, 2, 0, 0, 0, 1] {
        return Ok(None);
    }
    let bits = u32::from_be_bytes(channel[8..].try_into()?);
    let selector: i64 = node.get("MediaTrackIdx")?.parse()?;
    let qualified = match (group, bits) {
        (_, 0x4001) => 0,
        (false, 0x8001) | (true, 0x4002) => 1,
        _ => return Ok(None),
    };
    // A native embedded-movie edit can use physical stream zero and override
    // its channel, or select the corresponding split Media Pool track.
    if qualified != selector && !(embedded_movie && selector == 0) {
        return Err("Resolve mono selector contradicts virtual channel".into());
    }
    Ok(Some(qualified))
}

fn placement(
    id: &str,
    item: &str,
    grid: Grid,
    edit: Edit,
    kind: StreamKind,
    track: i64,
    audio: Option<i64>,
) -> Result<Placement> {
    Ok(Placement {
        id: id.into(),
        item: item.into(),
        record_start: seconds(edit.start, grid.rate)?,
        source_in: seconds(edit.source_in, grid.rate)?,
        duration: seconds(edit.length, grid.rate)?,
        video_track: (kind == StreamKind::Video).then_some(track),
        audio_tracks: if kind == StreamKind::Audio {
            vec![track]
        } else {
            vec![]
        },
        audio_streams: audio.into_iter().collect(),
        grouped_audio: vec![],
        gain_db: 0.0,
        volume_db: None,
        volume_keys: vec![],
        track_gains_db: vec![],
        track_pans: vec![],
        speed: None,
        frame_map: None,
        transform: None,
        effects: vec![],
    })
}

#[cfg(test)]
mod tests;
