//! Standalone source interpretation of the qualified Resolve archive profile.
mod atom;
mod effects;
mod encoding;
mod markers;
pub(crate) mod mixer;
mod multicam;
mod observations;
mod wav;
mod xml;

use crate::{
    audio::{FADE_LOSS, KEY_LOSS},
    Result,
};
use encoding::{extent, frames, hex_bytes, pb_bytes, qt, raster, rate, timecode, Blobs};
use graph::{
    source::{
        SourceClock, SourceClockProvenance, SourceGraph, SourceItem, SourceItemMedia, SourceMedia,
        SourceSequence,
    },
    Bin, Loss, Placement, Project, Rational, Sidecar, StreamKind, Track, Value,
};
use mixer::Mixer;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};
use xml::Node;

const ROOT_FOLDER: &str = "MediaPool/Master/MpFolder.xml";
pub(crate) const ADAPTER: &str = "resolve.source";
pub(crate) const CLOCK_ROUTE: &str = "Resolve archive clock";

/// Read an exported .drp without opening media or making application calls.
pub fn read(path: &Path) -> Result<SourceGraph> {
    let members = xml::read(path)?;
    let mut blobs = Blobs::default();
    let project = members
        .get("project.xml")
        .ok_or("missing Resolve project.xml")?;
    expect(project, "SM_Project")?;
    if project.get("ProjectVersion")? != "17" {
        return Err("unsupported Resolve project version".into());
    }
    let folders = folders(&members, project, &mut blobs)?;
    let containers = containers(&members)?;
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
    let mut pool_items = HashSet::new();
    let mut ids = HashSet::new();
    claim(&mut ids, project.id()?)?;
    claim(&mut ids, project.one("MediaPool")?.entry()?.id()?)?;
    for folder in folders {
        let bin_id = folder.id()?;
        claim(&mut ids, bin_id)?;
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
    let mut context = Context {
        masters,
        groups: HashMap::new(),
        containers,
        ids,
        pool_items,
        blobs,
        graph,
        used_containers: HashSet::new(),
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
    if context.used_containers.len() != context.containers.len() {
        return Err("Resolve archive contains an unregistered sequence container".into());
    }
    markers::read(project, &mut context.graph, &mut context.blobs)?;
    context.graph.validate()?;
    Ok(context.graph)
}

struct Context<'a> {
    masters: HashMap<&'a str, Master>,
    groups: HashMap<&'a str, multicam::Group<'a>>,
    containers: HashMap<&'a str, &'a Node>,
    ids: HashSet<&'a str>,
    pool_items: HashSet<&'a str>,
    used_containers: HashSet<&'a str>,
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

fn folders<'a>(
    members: &'a BTreeMap<String, Node>,
    project: &Node,
    blobs: &mut Blobs,
) -> Result<Vec<&'a Node>> {
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
    let mut ordered = Vec::new();
    let mut seen = HashMap::new();
    // BTreeMap ordering can place children before their parents.
    let mut entries: Vec<_> = members
        .iter()
        .filter(|(name, _)| name.starts_with("MediaPool/"))
        .collect();
    entries.sort_by_key(|(name, _)| name.matches('/').count());
    for (name, folder) in entries {
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
            Some(parent) if seen.get(parent).copied() == expected_parent => {}
            _ => return Err("missing or contradictory Resolve bin parent".into()),
        }
        if seen.insert(id, directory).is_some() {
            return Err("duplicate Resolve bin identity".into());
        }
        ordered.push(folder);
    }
    let mut children: HashMap<&str, HashSet<&str>> = HashMap::new();
    for folder in &ordered {
        if let Some(parent) = folder.maybe("MpFolder")?.filter(|s| !s.is_empty()) {
            children.entry(parent).or_default().insert(folder.id()?);
        }
    }
    for folder in &ordered {
        let Some(fields) = folder
            .maybe("FieldsBlob")?
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let fields = blobs.envelope(fields)?;
        let mut listed = HashSet::new();
        if let Some(encoded) = pb_bytes(&fields, 2)? {
            let map = qt(encoded)?;
            let count = map.keys().count();
            for key in map.keys() {
                let index: usize = key.parse()?;
                if index >= count
                    || index.to_string() != key
                    || !listed.insert(map.text(key)?.ok_or("missing Resolve child bin identity")?)
                {
                    return Err("ambiguous Resolve child bin registration".into());
                }
            }
            if children
                .get(folder.id()?)
                .map_or(!listed.is_empty(), |expected| expected != &listed)
            {
                return Err("Resolve child bin registration contradicts the archive paths".into());
            }
        } else if children.contains_key(folder.id()?) {
            return Err("missing Resolve child bin registration".into());
        }
    }
    Ok(ordered)
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
    wav_samples: Option<i64>,
    stereo_pool_route: bool,
    split_atom_audio: bool,
}

fn media(clip: &Node, blobs: &mut Blobs, graph: &mut SourceGraph) -> Result<Master> {
    let id = clip.id()?;
    let video = clip.one("Video")?.entry()?;
    expect(video, "BtVideoInfo")?;
    same(video.get("Type")?, "STANDARD_CLIP", "video media type")?;
    let timing = blobs.qt(video.get("Time")?)?;
    let rate = rate(timing.bytes("FrameRate")?)?;
    if timing.int("StartFrame")? != 0 {
        return Err("unsupported nonzero Resolve media cache origin".into());
    }
    let length = timing.int("NumFrames")?;
    if length <= 0 {
        return Err("invalid Resolve media duration".into());
    }
    let saved_timecode = timing.text("Timecode")?;
    let start = timecode(saved_timecode.unwrap_or("00:00:00:00"), 24)?;
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
    let clock = clock(id, "BtVideoTime.Timecode", start, rate)?;
    let file_start = clock.exact_start();
    let clock =
        (saved_timecode.is_some() || !matches!(codec.as_str(), "apch" | "avc1")).then_some(clock);
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
    let mut audio = BTreeMap::new();
    let mut split_atom_audio = false;
    let mut stereo_pool_route = false;
    if let Some(embedded) = clip.optional("EmbeddedAudioVec")? {
        let mut entries = embedded.elements()?;
        match (entries.next(), entries.next()) {
            (Some(entry), None) => {
                let node = entry.entry()?;
                expect(node, "BtAudioInfo")?;
                let (audio_path, audio_codec) = cache(node, blobs)?;
                same(&audio_path, &path, "audio cache locator")?;
                if codec == "avc1"
                    && audio_codec == "AAC"
                    && size == (1920, 1080)
                    && rate == Rational::integer(24)
                    && start == 0
                {
                    read_avc_stereo(clip, node, &path, length, blobs, graph, &mut item)?;
                    audio = BTreeMap::from([(0, 0), (1, 1)]);
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
    graph.items.push(item);
    Ok(Master {
        rate,
        length,
        start,
        audio,
        path,
        size,
        wav_samples: None,
        stereo_pool_route,
        split_atom_audio,
    })
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

fn read_avc_stereo(
    clip: &Node,
    node: &Node,
    path: &str,
    length: i64,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
    item: &mut SourceItem,
) -> Result<()> {
    let tracks = blobs.qt(node.get("TracksBA")?)?;
    let routes = audio_routes(clip, blobs)?;
    if !tracks.keys().eq(["0"]) || !routes.keys().eq(["0"]) {
        return Err("expected one embedded AVC audio track and stereo route".into());
    }
    let track = qt(tracks.bytes("0")?)?;
    let route = qt(routes.bytes("0")?)?;
    let duration = seconds(length, Rational::integer(24))?;
    if track.int("NumChannels")? != 2
        || track.int("IdxTrack")? != 0
        || track.int("SampleRate")? != 48000
        || track.int("ChannelLayout")? != 2
        || track.int("BitDepth")? != 3
        || track.text("CodecName")? != Some("AAC")
        || track.real("StartTime")? != 0.0
        || Some(track.int("Duration")?) != length.checked_mul(2000)
        || route.int("Type")? != 0
        || route.text("CodecName")? != Some("AAC")
        || extent(route.bytes("MediaExtents")?, Rational::integer(24))? != (0, length)
    {
        return Err("unqualified embedded AVC stereo sample clock or format".into());
    }
    let channels = qt(route.bytes("ChannelVecBA")?)?;
    if !channels.keys().eq(["0", "1"]) {
        return Err("incomplete AVC stereo channel routing".into());
    }
    for (channel, key) in ["0", "1"].into_iter().enumerate() {
        let row = qt(channels.bytes(key)?)?;
        let saved = if row.keys().any(|k| k == "ChannelIdx") {
            row.int("ChannelIdx")?
        } else {
            0
        };
        if row.text("MediaRef")? != Some(node.id()?) || saved != channel as i64 {
            return Err("AVC stereo selector contradicts its cache".into());
        }
        let label = channel as i64 + 1;
        add_media(
            graph,
            item,
            SourceMedia {
                id: format!("{}/audio/{label}", clip.id()?),
                kind: StreamKind::Audio,
                locations: vec![path.into()],
                codec: Some("aac".into()),
                frame_size: None,
                rate: None,
                sample_rate: Some(48000),
                bits: None,
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
        display_base: 24,
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
    origin: i64,
    length: i64,
    size: (i64, i64),
}

impl Grid {
    fn read(node: &Node) -> Result<Self> {
        let rate = rate(&hex_bytes(node.get("FrameRate")?)?)?;
        let (origin, length) = extent(&hex_bytes(node.get("MediaExtents")?)?, rate)?;
        Ok(Self {
            rate,
            origin,
            length,
            size: raster(&hex_bytes(node.get("Resolution")?)?)?,
        })
    }

    fn edit(self, node: &Node, wav: bool) -> Result<Edit> {
        let start: i64 = node.get("Start")?.parse()?;
        let start = start
            .checked_sub(self.origin)
            .ok_or("Resolve record start overflow")?;
        let duration = node.get("Duration")?;
        let length = if wav {
            wav::integer_duration(duration)?
        } else {
            duration.parse()?
        };
        let source_in = match node.get("In")? {
            "" => 0, // Native serialization of default zero.
            text => text.parse::<i64>()?,
        };
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
    let clock = clock(id, "Sm2Sequence.MediaExtents", grid.origin, grid.rate)?;
    let mut out = SourceSequence {
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
    if let Some(container) = context.container(id, grid.length)? {
        let track_flags = read_tracks(context, container, grid, &mut out)?;
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
    let mut marks = Vec::new();
    for (tag, key) in [
        ("MarkInVideo", "video_in"),
        ("MarkOutVideo", "video_out"),
        ("MarkInAudio", "audio_in"),
        ("MarkOutAudio", "audio_out"),
    ] {
        if let Some(value) = pool_clip.maybe(tag)? {
            marks.push((key.into(), text(value)));
        }
    }
    if marks.iter().any(|(_, value)| value != &text("")) {
        fields.push(("saved_timeline_marks", Value::Obj(marks)));
        loss(
            &mut context.graph, id, "timeline_marks",
            "Resolve timeline marks retained in graph; destination mapping is unqualified",
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
    if out.tracks.iter().map(|t| (t.kind, t.index)).ne([
        (StreamKind::Video, 1),
        (StreamKind::Audio, 1),
        (StreamKind::Audio, 2),
    ]) {
        return Ok(MixerControls::default());
    }
    let mixer = node
        .maybe("FieldsBlob")?
        .map(Mixer::decode)
        .transpose()?
        .flatten();
    let flags_muted = track_flags.is_some_and(|flags| flags != [false; 2]);
    let Some(mixer) = mixer else {
        if flags_muted {
            unqualified_mute(&mut context.graph, &out.id);
        }
        return Ok(MixerControls::default());
    };
    context.blobs.charge(mixer.decoded_bytes())?;
    if let Some((data, requires_mapping)) = mixer.source_facts() {
        context.graph.sidecars.push(Sidecar {
            adapter: "resolve.fairlight".into(),
            object: out.id.clone(),
            data,
        });
        if requires_mapping {
            loss(
                &mut context.graph,
                &out.id,
                "fairlight_controls",
                "Resolve Fairlight state retained in graph; destination mapping is unqualified",
            );
        }
    }
    let faders = mixer.track_faders().unwrap_or_default();
    let main_bus = mixer.main_bus_fader().unwrap_or_default();
    let has_mute_state = flags_muted || mixer.has_mute_state();
    if faders == [0, 0] && main_bus == 0 && !has_mute_state {
        return Ok(MixerControls::default());
    }
    let mutes = mixer.track_mutes();
    if !mixer.qualified()
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
) -> Result<Option<[bool; 2]>> {
    let tracks = tracks(container)?;
    let track_flags = match tracks.as_slice() {
        [(StreamKind::Video, v), (StreamKind::Audio, a1), (StreamKind::Audio, a2)]
            if v.get("Flags")? == "0" && a1.get("SubType")? == "1" && a2.get("SubType")? == "1" =>
        {
            match (a1.get("Flags")?, a2.get("Flags")?) {
                (a @ ("0" | "2"), b @ ("0" | "2")) => Some([a == "2", b == "2"]),
                _ => None,
            }
        }
        _ => None,
    };
    let mut video = 0;
    let mut audio = 0;
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
        out.tracks.push(Track {
            kind,
            index,
            level_db: None,
        });
        let flags = track.get("Flags")?;
        let supported = (flags == "0"
            || (flags == "2" && kind == StreamKind::Audio && track_flags.is_some()))
            && (kind == StreamKind::Video || track.get("SubType")? == "1");
        if !supported {
            omitted(
                &mut context.graph,
                &out.id,
                track_id,
                "track",
                "unqualified Resolve track state or layout",
            );
        }
        for entry in track.one("Items")?.elements()? {
            let node = entry.entry()?;
            let id = node.id()?;
            claim(&mut context.ids, id)?;
            if node.name == "Sm2TiTransition" {
                observations::transition(
                    &mut context.graph, node, &out.id, track_id, kind, index, grid,
                )?;
                continue;
            }
            if !supported || !ordinary_item(node, kind, &out.id, &mut context.graph)? {
                continue;
            }
            let target = node.get("MediaRef")?;
            if let Some(group) = context.groups.get(target) {
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
                if master.rate != grid.rate {
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
                let edit = grid.edit(
                    node,
                    kind == StreamKind::Audio && master.wav_samples.is_some(),
                )?;
                if let Some(p) = direct(
                    master,
                    node,
                    grid,
                    edit,
                    kind,
                    index,
                    &out.id,
                    &mut context.graph,
                    &mut context.blobs,
                )? {
                    out.placements.push(p);
                }
            } else if context.pool_items.contains(target) {
                omitted(
                    &mut context.graph,
                    &out.id,
                    id,
                    "media",
                    "edit refers to an unsupported Resolve pool item",
                );
            } else {
                return Err("unresolved Resolve placement media reference".into());
            }
        }
    }
    for tag in ["SubtitleTrackVec", "GeometryTrackVec"] {
        if !container.one(tag)?.children.is_empty() {
            loss(
                &mut context.graph,
                &out.id,
                "tracks",
                "subtitle or geometry tracks are outside the Resolve source profile",
            );
        }
    }
    Ok(track_flags)
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
    edit: Edit,
    kind: StreamKind,
    track: i64,
    owner: &str,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<Option<Placement>> {
    if !leaf_matches(master, node, grid, edit, owner, graph, blobs)? {
        return Ok(None);
    }
    let audio = if kind == StreamKind::Audio {
        let selector = if master.stereo_pool_route {
            wav::stereo_channel(node, blobs)?
        } else {
            mono(node, blobs, false)?
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
    if let Some(filters) = node
        .maybe("EffectFiltersBA")?
        .filter(|value| !value.is_empty())
    {
        let interpreted = match kind {
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
                    None => false,
                }
            }
            StreamKind::Audio => match effects::clip_audio(filters, blobs, edit)? {
                Some(effects::ClipAudio::StaticVolume(db)) => {
                    placement.volume_db = Some(db);
                    true
                }
                Some(effects::ClipAudio::VolumeKeys(keys)) => {
                    for (frame, db) in keys {
                        placement.volume_keys.push(graph::VolumeKey {
                            at: seconds(frame, grid.rate)?,
                            db,
                        });
                    }
                    receipt(
                        graph,
                        &placement.id,
                        vec![
                            ("volume_key_clock", text("item_source")),
                            ("volume_interpolation", text("linear_db")),
                            ("volume_extrapolation", text("hold")),
                        ],
                    );
                    loss(graph, &placement.id, KEY_LOSS.0, KEY_LOSS.1);
                    true
                }
                Some(effects::ClipAudio::Fades { head, tail }) => {
                    receipt(
                        graph,
                        &placement.id,
                        vec![
                            ("fade_in_frames", Value::Int(head)),
                            ("fade_out_frames", Value::Int(tail)),
                            ("fade_clock", text("clip_edges")),
                            ("fade_shape", text("linear_amplitude")),
                        ],
                    );
                    loss(graph, &placement.id, FADE_LOSS.0, FADE_LOSS.1);
                    true
                }
                None => false,
            },
            _ => false,
        };
        if !interpreted {
            if kind == StreamKind::Audio {
                if let Some(data) = effects::custom_fades(filters, blobs, edit)? {
                    graph.sidecars.push(Sidecar {
                        adapter: "resolve.clip_fades".into(),
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

fn record_unqualified_timing(
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
    omitted(
        graph,
        owner,
        id,
        "timing",
        "unqualified Resolve rate or retime map",
    );
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
    if edit.source_in + edit.length > master.length
        || frames(node.get("MediaStartTime")?.parse()?, grid.rate)? != master.start
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
    Ok(true)
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

fn mono(node: &Node, blobs: &mut Blobs, group: bool) -> Result<Option<i64>> {
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
    if qualified != selector {
        return Err("Resolve mono selector contradicts virtual channel".into());
    }
    Ok(Some(selector))
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
