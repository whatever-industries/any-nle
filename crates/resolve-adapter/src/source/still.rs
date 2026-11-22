//! Saved single-file picture caches and their constant still holds.
use super::*;

pub(super) fn read(
    clip: &Node,
    video: &Node,
    timing: &encoding::Map,
    cache: &CacheRecord,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
) -> Result<Option<Master>> {
    if !cache.same_file_name || timing.int("NumFrames")? != 1
        || clip.optional("EmbeddedAudioVec")?.is_some_and(|audio| !audio.children.is_empty())
    {
        return Ok(None);
    }
    let Some(codec) = literal_codec(&cache.path) else { return Ok(None) };
    if cache.codec.is_some() && !(codec == "heic" && cache.codec.as_deref() == Some("HEVC")) {
        return Ok(None);
    }
    let (rate, drop) = frame_rate(timing.bytes("FrameRate")?)?;
    if drop {
        return Ok(None);
    }
    let id = clip.id()?;
    let geometry = blobs.qt(video.get("Geometry")?)?;
    let size = raster(geometry.bytes("Resolution")?)?;
    let pixel_aspect = geometry.pixel_aspect()?;
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
    add_media(graph, &mut item, SourceMedia {
        id: format!("{id}/video"),
        kind: StreamKind::Video,
        locations: vec![cache.path.clone()],
        codec: Some(codec.into()),
        frame_size: Some(size),
        rate: Some(rate),
        sample_rate: None,
        bits: None,
        duration: seconds(1, rate)?,
        start: None,
        clock: None,
        stream: Some(1),
        online: None,
        original: None,
    });
    metadata::capture_pixel_aspect(graph, &format!("{id}/video"), pixel_aspect);
    // The native cache start can be the image number. Keep it as a saved
    // witness and address the one literal file from its own first frame.
    receipt(graph, id, vec![
        ("kind", text("standalone_still")),
        ("cache_frames", Value::Int(1)),
        ("saved_cache_start_frame", Value::Int(timing.int("StartFrame")?)),
    ]);
    metadata::capture(clip, video, None, blobs, graph)?;
    graph.items.push(item);
    Ok(Some(Master {
        rate, length: 1, start: 0, audio: BTreeMap::new(), path: cache.path.clone(), size,
        prores_hq: false, avc: false, camera_pcm: false, native_mono_movie: false, saved_aac_clock: false,
        external_audio: false, wav_samples: None,
        standalone_audio: None, still: true, standalone_mono_atom: false, stereo_pool_route: false,
        native_stereo_pcm: false, split_atom_audio: false,
    }))
}

fn literal_codec(path: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next()?;
    if name.contains(['[', ']', '%', '#', '*', '?']) {
        return None;
    }
    match name.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("jpeg"),
        "png" => Some("png"),
        "tif" | "tiff" => Some("tiff"),
        "heic" => Some("heic"),
        _ => None,
    }
}

pub(super) fn edit(
    master: &Master,
    node: &Node,
    grid: Grid,
    track: i64,
    owner: &str,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<Option<Placement>> {
    let saved_in = match node.get("In")? {
        "" => Some(0),
        value => timing::FrameField::parse(value)?.integer(),
    };
    let start = timing::FrameField::parse(node.get("Start")?)?.integer();
    let length = timing::FrameField::parse(node.get("Duration")?)?.integer();
    let (Some(start), Some(length)) = (start, length) else {
        record_unqualified_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
        return Ok(None);
    };
    let rate = Rational::integer(24);
    let saved_map = node.get("MediaTimemapBA")?;
    let map_end_frames = 60_000 * 24;
    let same_clock = grid.rate == rate && master.rate == rate
        && encoding::rate(&hex_bytes(node.get("MediaFrameRate")?)?)? == rate
        && timing::clock_matches(node.get("MediaStartTime")?, Rational::ZERO, master.rate)?;
    let simple = same_clock && saved_in.is_some_and(|source| source >= 0
        && source.checked_add(length).is_some_and(|end| end <= map_end_frames))
        && observations::still_hold(saved_map, blobs)?;
    let Some(count) = usize::try_from(length).ok().filter(|n| simple && (1..=graph::MAX_LIST).contains(n))
    else {
        record_unqualified_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
        return Ok(None);
    };
    let edit = grid.edit_at(start, length, 0)?;
    leaf_range_matches(master, node, 0, 1, owner, graph)?;
    let id = node.id()?;
    let mut out = placement(id, node.get("MediaRef")?, grid, edit, StreamKind::Video, track, None)?;
    if count > 1 {
        blobs.charge(count * size_of::<i64>())?;
        out.frame_map = Some(vec![0; count]);
    }
    record_saved_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
    if node.maybe("EffectFiltersBA")?.is_some_and(|body| !body.is_empty()) {
        omitted(graph, owner, id, "effect", "Resolve still effect controls are unqualified");
    }
    capture_grade(node, id, graph, blobs)?;
    capture_clip_fusion(node, id, graph, blobs)?;
    Ok(Some(out))
}
