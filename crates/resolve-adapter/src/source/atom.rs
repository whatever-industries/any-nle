//! Read the saved separate-file mono routes of the qualified Resolve MXF cohort.
use super::{add_media, audio_routes, cache, expect, receipt, seconds, text, Blobs, Node};
use crate::{
    prepare::rate,
    source::encoding::{extent, frames, qt, Map},
    Result,
};
use graph::{
    source::{SourceGraph, SourceItem, SourceMedia},
    StreamKind, Value,
};
use std::collections::{BTreeMap, HashSet};

struct Packages {
    file: [u8; 32],
    master: [u8; 32],
}

fn packages(owner: &Map) -> Result<Packages> {
    let metadata = qt(owner.bytes("MXFMetadata")?)?;
    let file = metadata.bytes("sourceMobId")?.try_into()?;
    let master = metadata.bytes("masterMobId")?.try_into()?;
    if file == [0; 32] || master == [0; 32] || file == master {
        return Err("invalid Resolve split MXF package identity".into());
    }
    Ok(Packages { file, master })
}

fn mxf_path(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("mxf"))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_audio(
    clip: &Node,
    video: &Node,
    picture_path: &str,
    audio: [&Node; 2],
    start: i64,
    length: i64,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
    item: &mut SourceItem,
) -> Result<BTreeMap<i64, i64>> {
    let picture_id = video.id()?;
    let ids = [audio[0].id()?, audio[1].id()?];
    if ids[0] == ids[1] || ids.contains(&picture_id) || ids.contains(&clip.id()?) {
        return Err("duplicate Resolve split MXF cache identity".into());
    }
    let routes = audio_routes(clip, blobs)?;
    if !routes.keys().eq(["0", "1"]) {
        return Err("expected two Resolve split MXF mono selectors".into());
    }
    let picture = packages(&blobs.qt(video.get("VideoMetadata")?)?)?;
    if !mxf_path(picture_path) {
        return Err("expected a saved Resolve picture MXF locator".into());
    }
    let expected_samples = length
        .checked_mul(2002)
        .ok_or("split MXF sample overflow")?;
    let rate = rate();
    let duration = seconds(length, rate)?;
    let mut paths = HashSet::from([picture_path.to_owned()]);
    let mut files = HashSet::from([picture.file]);
    let mut seen = HashSet::new();

    for (index, key) in ["0", "1"].into_iter().enumerate() {
        let route = qt(routes.bytes(key)?)?;
        let channels = qt(route.bytes("ChannelVecBA")?)?;
        if !channels.keys().eq(["0"]) {
            return Err("unsupported Resolve split MXF channel count".into());
        }
        let channel = qt(channels.bytes("0")?)?;
        let saved_track = if channel.keys().any(|name| name == "TrackIdx") {
            channel.int("TrackIdx")?
        } else {
            0
        };
        let reference = channel
            .text("MediaRef")?
            .ok_or("missing split MXF cache reference")?;
        let slot = ids
            .iter()
            .position(|&id| id == reference)
            .ok_or("unknown Resolve split MXF cache reference")?;
        if saved_track != 0 || !seen.insert(slot) {
            return Err("split MXF selectors must address track zero in different caches".into());
        }
        let node = audio[slot];
        expect(node, "BtAudioInfo")?;
        let (path, codec) = cache(node, blobs)?;
        if !mxf_path(&path) || !paths.insert(path.clone()) || codec != "Linear PCM" {
            return Err("invalid Resolve split MXF audio locator or codec".into());
        }
        let tracks = blobs.qt(node.get("TracksBA")?)?;
        if !tracks.keys().eq(["0"]) {
            return Err("expected one physical track in the split MXF cache".into());
        }
        let track = qt(tracks.bytes("0")?)?;
        let identity = packages(&track)?;
        if identity.master != picture.master || !files.insert(identity.file) {
            return Err("Resolve split MXF package identities disagree".into());
        }
        if route.text("CodecName")? != Some("Linear PCM")
            || route.int("BitDepth")? != 2
            || track.int("IdxTrack")? != 0
            || track.int("NumChannels")? != 1
            || track.int("SampleRate")? != 48000
            || track.int("BitDepth")? != 2
            || track.int("Duration")? != expected_samples
            || frames(track.real("StartTime")?, rate)? != start
            || extent(route.bytes("MediaExtents")?, rate)? != (start, length)
        {
            return Err("invalid Resolve split MXF audio extent or PCM profile".into());
        }
        let selector = index as i64;
        let label = selector + 1;
        let id = format!("{}/audio/{label}", item.id);
        receipt(
            graph,
            &id,
            vec![
                ("saved_selector", Value::Int(selector)),
                ("saved_cache_track", Value::Int(0)),
                ("saved_cache_id", text(reference)),
                (
                    "saved_file_package",
                    Value::Str(hex::encode_upper(identity.file)),
                ),
            ],
        );
        add_media(
            graph,
            item,
            SourceMedia {
                id,
                kind: StreamKind::Audio,
                locations: vec![path],
                codec: Some("pcm".into()),
                frame_size: None,
                rate: None,
                sample_rate: Some(48000),
                bits: Some(24),
                duration,
                start: Some(seconds(start, rate)?),
                clock: None,
                stream: Some(label),
                online: None,
                original: None,
            },
        );
    }
    item.umid = Some(hex::encode_upper(picture.master));
    receipt(
        graph,
        &format!("{}/video", item.id),
        vec![(
            "saved_file_package",
            Value::Str(hex::encode_upper(picture.file)),
        )],
    );
    Ok(BTreeMap::from([(0, 0), (1, 0)]))
}
