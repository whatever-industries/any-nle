//! Source evidence for the saved external mono route on a camera master.
use super::*;
use encoding::Map;

const POOL_ADAPTER: &str = "resolve.virtual_audio";
const EDIT_ADAPTER: &str = "resolve.virtual_audio_edits";
const EXTERNAL_MONO: &[u8] = &[0, 0, 0, 2, 0, 0, 0, 1, 0x20, 0, 0x40, 1];

pub(super) type CacheIndex<'a> = HashMap<&'a str, Option<(&'a Node, &'a Node)>>;

pub(super) fn index<'a>(folders: impl Iterator<Item = &'a Node>) -> Result<CacheIndex<'a>> {
    let mut caches = HashMap::new();
    for folder in folders {
        for entry in folder.one("MediaVec")?.elements()? {
            let clip = entry.entry()?;
            for cache in clip
                .children
                .iter()
                .filter(|node| node.name == "EmbeddedAudioVec")
                .flat_map(|vec| &vec.children)
                .filter(|entry| entry.name == "Element")
                .filter_map(|entry| entry.entry().ok())
                .filter(|node| node.name == "BtAudioInfo")
            {
                if let Some(id) = cache.id.as_deref() {
                    caches
                        .entry(id)
                        .and_modify(|found| *found = None)
                        .or_insert(Some((clip, cache)));
                }
            }
        }
    }
    Ok(caches)
}

pub(super) fn capture(
    clip: &Node,
    embedded: &Node,
    codec: &str,
    caches: &CacheIndex<'_>,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
) -> Result<bool> {
    if codec != "Linear PCM" {
        return Ok(false);
    }
    let Some(saved) = clip
        .maybe("VirtualAudioTracksBA")?
        .filter(|saved| saved.len() <= 512)
    else {
        return Ok(false);
    };
    let Ok(bytes) = hex_bytes(saved) else {
        return Ok(false);
    };
    let Ok(selectors) = qt(&bytes) else {
        return Ok(false);
    };
    if !selectors.keys().eq(["0"]) || !external_mono(&qt(selectors.bytes("0")?)?)? {
        return Ok(false);
    }
    blobs.charge(bytes.len())?;
    let routes = audio_routes(clip, blobs)?;
    if !routes.keys().eq(["0", "1"]) {
        return Err("expected Resolve embedded and external audio routes".into());
    }
    let camera = qt(routes.bytes("0")?)?;
    let external = qt(routes.bytes("1")?)?;
    if camera.int("Type")? != 0 || external.int("Origin")? != 1 {
        return Err("unexpected Resolve external audio origin".into());
    }
    let camera_channels = qt(camera.bytes("ChannelVecBA")?)?;
    let external_channels = qt(external.bytes("ChannelVecBA")?)?;
    if !camera_channels.keys().eq(["0", "1"]) || !external_channels.keys().eq(["0"]) {
        return Err("unexpected Resolve external audio channel layout".into());
    }
    let local_ref = embedded.id()?;
    validate_channel(&qt(camera_channels.bytes("0")?)?, local_ref, 0)?;
    validate_channel(&qt(camera_channels.bytes("1")?)?, local_ref, 1)?;
    let selected = qt(external_channels.bytes("0")?)?;
    let cache_ref = selected
        .text("MediaRef")?
        .ok_or("missing Resolve external audio cache reference")?;
    validate_channel(&selected, cache_ref, 0)?;
    let (source, cache_node) = caches
        .get(cache_ref)
        .copied()
        .flatten()
        .ok_or("missing or ambiguous Resolve external audio cache")?;
    if source.name != "Sm2MpAudioClip" || source.id()? == clip.id()? || cache_ref == local_ref {
        return Err("Resolve external audio route must refer to a standalone audio item".into());
    }
    let (saved_path, saved_codec) = cache(cache_node, blobs)?;
    same(&saved_codec, "Linear PCM", "external audio cache codec")?;
    let offset = selected.int("SampleOffset")?;
    let owner = clip.id()?;
    let embedded_route = route_facts(&camera, embedded, owner, &[1, 2], blobs)?;
    let external_route = route_facts(&external, cache_node, source.id()?, &[1], blobs)?;
    let mut facts = vec![
        ("embedded_stereo".into(), Value::Obj(embedded_route)),
        ("external_mono".into(), Value::Obj(external_route)),
        ("saved_sample_offset".into(), Value::Int(offset)),
        ("saved_route_index".into(), Value::Int(1)),
        ("saved_origin_code".into(), Value::Int(1)),
        ("saved_selector_index".into(), Value::Int(0)),
        ("saved_selectors".into(), text(saved)),
        ("external_saved_path".into(), text(&saved_path)),
    ];
    if external.keys().any(|key| key == "BitDepth") {
        facts.push((
            "external_saved_route_depth_code".into(),
            Value::Int(external.int("BitDepth")?),
        ));
    }
    graph.sidecars.push(Sidecar {
        adapter: POOL_ADAPTER.into(),
        object: owner.into(),
        data: Value::Obj(facts),
    });
    loss(
        graph,
        owner,
        "audio_routing",
        "Resolve external mono route is saved; portable playback timing is unqualified",
    );
    Ok(true)
}

fn validate_channel(channel: &Map, media_ref: &str, physical_channel: i64) -> Result<()> {
    if channel.text("MediaRef")? != Some(media_ref)
        || saved_index(channel, "ChannelIdx")? != physical_channel
        || saved_index(channel, "TrackIdx")? != 0
    {
        return Err("contradictory Resolve external audio channel".into());
    }
    Ok(())
}

fn saved_index(map: &Map, key: &str) -> Result<i64> {
    if map.keys().any(|field| field == key) {
        map.int(key)
    } else {
        Ok(0)
    }
}

fn route_facts(
    route: &Map,
    audio: &Node,
    pool_item: &str,
    channels: &[i64],
    blobs: &mut Blobs,
) -> Result<Vec<(String, Value)>> {
    let tracks = blobs.qt(audio.get("TracksBA")?)?;
    if !tracks.keys().eq(["0"]) {
        return Err("expected one cache track for Resolve external audio routing".into());
    }
    let track = qt(tracks.bytes("0")?)?;
    let sample_rate = track.int("SampleRate")?;
    let samples = track.int("Duration")?;
    if sample_rate != 48_000
        || samples <= 0
        || track.int("IdxTrack")? != 0
        || track.int("NumChannels")? != channels.len() as i64
        || track.text("CodecName")? != Some("Linear PCM")
        || route.text("CodecName")? != Some("Linear PCM")
    {
        return Err("contradictory Resolve external audio cache clock or format".into());
    }
    let start = encoding::sample_start(track.real("StartTime")?, sample_rate)?;
    let extent = route.bytes("MediaExtents")?;
    let ([origin, duration], []) = extent.as_chunks::<8>() else {
        return Err("invalid Resolve external audio extent".into());
    };
    if encoding::sample_start(f64::from_le_bytes(*origin), sample_rate)? != start
        || f64::from_le_bytes(*duration) != samples as f64 / sample_rate as f64
    {
        return Err("contradictory Resolve external audio cache clock or format".into());
    }
    Ok(vec![
        ("pool_item".into(), text(pool_item)),
        ("cache_id".into(), text(audio.id()?)),
        ("cache_track".into(), Value::Int(0)),
        (
            "physical_channels".into(),
            Value::Arr(channels.iter().copied().map(Value::Int).collect()),
        ),
        ("sample_rate".into(), Value::Int(sample_rate)),
        ("sample_count".into(), Value::Int(samples)),
        ("start".into(), text(&start.to_string())),
        (
            "saved_cache_depth_code".into(),
            Value::Int(track.int("BitDepth")?),
        ),
        ("saved_route_extent".into(), text(&hex::encode(extent))),
    ])
}

fn external_mono(selector: &Map) -> Result<bool> {
    Ok(selector.int("AudioType")? == 1 && selector.bytes("ChannelsBA")? == EXTERNAL_MONO)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn observe_edit(
    source_rate: Rational,
    node: &Node,
    track: &Node,
    index: i64,
    owner: &str,
    record_rate: Rational,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<()> {
    let id = node.id()?;
    let saved_index = node.maybe("MediaTrackIdx")?;
    let selector = node
        .maybe("VirtualAudioTrackBA")?
        .filter(|encoded| !encoded.is_empty() && encoded.len() <= 512);
    let uses_external = match (saved_index, selector) {
        (Some("0"), Some(encoded)) => external_mono(&blobs.qt(encoded)?)?,
        _ => false,
    };
    observations::append(
        graph,
        EDIT_ADAPTER,
        owner,
        Value::Obj(vec![
            ("occurrence".into(), text(id)),
            ("pool_item".into(), text(node.get("MediaRef")?)),
            ("track_id".into(), text(track.id()?)),
            ("track".into(), Value::Int(index)),
            (
                "saved_track_index".into(),
                saved_index.map_or(Value::Null, text),
            ),
            ("saved_selector".into(), selector.map_or(Value::Null, text)),
            (
                "saved_route_index".into(),
                if uses_external {
                    Value::Int(1)
                } else {
                    Value::Null
                },
            ),
        ]),
    );
    record_saved_timing(graph, node, owner, source_rate, record_rate, blobs)?;
    let reason = if uses_external {
        "Resolve external mono edit is saved; playback timing is unqualified"
    } else {
        "Resolve edit has an unqualified selector for a master with external audio"
    };
    omitted(graph, owner, id, "audio_routing", reason);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{qt as encode_qt, Qt};

    #[test]
    fn external_selector_is_distinct_from_embedded_left() {
        let mut embedded = EXTERNAL_MONO.to_vec();
        embedded[8] = 0;
        for (bytes, expected) in [(EXTERNAL_MONO, true), (&embedded, false)] {
            let selector =
                encode_qt(&[("AudioType", Qt::Int(1)), ("ChannelsBA", Qt::Bytes(bytes))]);
            assert_eq!(external_mono(&qt(&selector).unwrap()).unwrap(), expected);
        }
    }

    #[test]
    fn audio_cache_identity_must_resolve_uniquely() {
        let owner_id = "00000000-0000-0000-0000-000000000001";
        let cache_id = "00000000-0000-0000-0000-000000000002";
        let folder = xml::parse(
            &format!(
                "<Folder><MediaVec>
                <Element><Sm2MpAudioClip DbId=\"{owner_id}\"><EmbeddedAudioVec>
                    <Element><BtAudioInfo DbId=\"{cache_id}\"/></Element>
                </EmbeddedAudioVec></Sm2MpAudioClip></Element>
            </MediaVec></Folder>"
            ),
            &mut 0,
        )
        .unwrap();
        let single = index([&folder].into_iter()).unwrap();
        let (owner, cache) = single[cache_id].unwrap();
        assert_eq!(owner.id().unwrap(), owner_id);
        assert_eq!(cache.id().unwrap(), cache_id);
        assert!(index([&folder, &folder].into_iter()).unwrap()[cache_id].is_none());
    }
}
