//! Resolve Premiere's saved clip, Media, and channel-group references.
use super::{
    leaf,
    physical::{Physical, Stream},
    ticks,
};
use crate::Result;
use graph::StreamKind;
use prproj::{build::time::TICKS_PER_SECOND, model::Project, xml::NodeId};
use std::path::Path;

pub(super) fn reference(project: &Project, parent: NodeId, path: &str) -> Result<NodeId> {
    let refs = project.doc.tree.findall(parent, path)?;
    let [node] = refs.as_slice() else {
        return Err(format!("missing or ambiguous Premiere {path} reference").into());
    };
    Ok(project.doc.resolve(*node)?)
}

fn references(project: &Project, parent: NodeId, path: &str, max: usize) -> Result<Vec<NodeId>> {
    let tree = &project.doc.tree;
    let refs = tree.findall(parent, path)?;
    if refs.len() > max {
        return Err(format!("too many saved Premiere {path} references").into());
    }
    // The saved vectors use explicit indices. Physical audio and group selectors
    // address those indices rather than the display names of the clips or tracks.
    let mut ordered = vec![None; refs.len()];
    for node in refs {
        let index = tree
            .get(node, "Index")
            .ok_or("missing Premiere vector index")?
            .parse::<usize>()?;
        let slot = ordered
            .get_mut(index)
            .ok_or("Premiere vector index is out of range")?;
        if slot.replace(project.doc.resolve(node)?).is_some() {
            return Err("duplicate Premiere vector index".into());
        }
    }
    Ok(ordered.into_iter().flatten().collect())
}

pub(super) fn clip_source(project: &Project, clip: NodeId, kind: StreamKind) -> Result<NodeId> {
    let source = reference(project, clip, "Clip/Source")?;
    let expected = if kind == StreamKind::Video {
        "VideoMediaSource"
    } else {
        "AudioMediaSource"
    };
    if project.doc.tree.tag(source) != expected {
        return Err("Premiere edit is not bound to a physical clip".into());
    }
    Ok(source)
}

pub(super) fn check_media(
    project: &Project,
    media: NodeId,
    primary: NodeId,
    path: &Path,
) -> Result<()> {
    let tree = &project.doc.tree;
    if tree.tag(media) != "Media" {
        return Err("Premiere physical source does not reference a Media object".into());
    }
    super::super::metadata::check_locators(tree, media, path)?;
    if media != primary {
        let key = leaf(tree, primary, "FileKey")?;
        if key.is_none() || leaf(tree, media, "FileKey")? != key {
            return Err("Premiere stream and primary FileKey disagree".into());
        }
    }
    Ok(())
}

fn stream_cache(
    project: &Project,
    media: NodeId,
    physical: &Physical,
    stream: &Stream,
) -> Result<NodeId> {
    let tree = &project.doc.tree;
    let picture = stream.kind() == StreamKind::Video;
    let node = reference(
        project,
        media,
        if picture {
            "VideoStream"
        } else {
            "AudioStream"
        },
    )?;
    if tree.tag(node)
        != if picture {
            "VideoStream"
        } else {
            "AudioStream"
        }
    {
        return Err("unexpected Premiere stream cache object".into());
    }
    let cached_clock: i64 = leaf(tree, node, "FrameRate")?
        .ok_or("missing saved stream clock")?
        .parse()?;
    let expected_ticks = if let Some(rate) = stream.rate {
        TICKS_PER_SECOND * rate.den() / rate.num()
    } else {
        TICKS_PER_SECOND / 48000
    };
    let cached_extent: i64 = leaf(tree, node, "Duration")?
        .ok_or("missing saved stream extent")?
        .parse()?;
    let cached = ticks(cached_extent)?;
    let video_rounded = !picture
        && physical
            .video
            .as_ref()
            .is_some_and(|v| v.duration == cached)
        && extent_within_sample(cached_extent, stream.count);
    if cached_clock != expected_ticks || cached != stream.duration && !video_rounded {
        return Err("Premiere stream cache clock or extent contradicts the original".into());
    }
    if let Some((width, height)) = stream.size {
        let rect = leaf(tree, node, "FrameRect")?.ok_or("missing saved picture raster")?;
        if rect != format!("0,0,{width},{height}") {
            return Err("Premiere picture cache raster contradicts the original".into());
        }
    }
    Ok(node)
}

pub(super) fn extent_within_sample(saved_ticks: i64, physical_samples: i64) -> bool {
    let step = i128::from(TICKS_PER_SECOND / 48000);
    (i128::from(saved_ticks) - i128::from(physical_samples) * step).abs() < step
}

pub(super) struct ItemBindings {
    video_source: Option<NodeId>,
    audio: Vec<AudioTemplate>,
    pub groups: Vec<Vec<i64>>,
}

struct AudioTemplate {
    source: NodeId,
    selectors: Vec<(usize, (usize, i64))>,
}

/// Premiere uses a separate Media stream number for multiple QuickTime
/// audio streams. Its OP1a importer can instead flatten two mono channels
/// into one cache, or save a per-Media SingleAudioChannel selector.
fn audio_template(
    project: &Project,
    clip: NodeId,
    primary: NodeId,
    path: &Path,
    physical: &Physical,
) -> Result<AudioTemplate> {
    let tree = &project.doc.tree;
    let source = clip_source(project, clip, StreamKind::Audio)?;
    let media = reference(project, source, "MediaSource/Media")?;
    check_media(project, media, primary, path)?;
    let ordinal = leaf(tree, media, "StreamNumber")?
        .map(str::parse::<usize>)
        .transpose()?
        .unwrap_or(0);
    let stream = physical
        .audio
        .get(ordinal)
        .ok_or("Premiere selects an absent original audio stream")?;
    let cache = stream_cache(project, media, physical, stream)?;
    let layout: Vec<serde_json::Value> = serde_json::from_str(
        leaf(tree, cache, "AudioChannelLayout")?.ok_or("missing saved audio layout")?,
    )?;
    let mxf_unlabelled_pair = physical.container == "mxf"
        && matches!(layout.as_slice(), [a, b] if a["channellabel"] == 0 && b["channellabel"] == 0);
    let flattened_op1a = mxf_unlabelled_pair
        && physical.audio.len() == 2
        && ordinal == 0
        && leaf(tree, media, "StreamNumber")?.is_none();
    let single = leaf(tree, media, "SingleAudioChannel")?
        .map(str::parse::<usize>)
        .transpose()?;
    let cache_single = leaf(tree, cache, "SingleChannel")?
        .map(str::parse::<usize>)
        .transpose()?;
    if single != cache_single
        || single.is_some_and(|i| !flattened_op1a || i >= 2)
        || !flattened_op1a && Some(layout.len() as i64) != stream.channels
        || !mxf_unlabelled_pair
            && stream.channels == Some(2)
            && !matches!(layout.as_slice(), [a, b] if a["channellabel"] == 100 && b["channellabel"] == 101)
        || !flattened_op1a
            && stream.channels == Some(1)
            && !matches!(layout.as_slice(), [a] if a["channellabel"] == 0)
    {
        return Err("Premiere saved audio layout contradicts the physical streams".into());
    }
    let refs = references(project, clip, "SecondaryContents/SecondaryContentItem", 2)?;
    let mut selectors = Vec::with_capacity(refs.len());
    for selector in refs {
        if tree.tag(selector) != "SecondaryContent"
            || reference(project, selector, "Content")? != source
        {
            return Err("Premiere audio selector has a contradictory content owner".into());
        }
        let channel: usize = leaf(tree, selector, "ChannelIndex")?
            .ok_or("missing saved audio channel")?
            .parse()?;
        let selection = match (flattened_op1a, single, channel) {
            (true, Some(stream), 0) => (stream, 0),
            (true, None, 0 | 1) => (channel, 0),
            (false, None, n) if n < stream.channels.unwrap_or_default() as usize => {
                (ordinal, n as i64)
            }
            _ => return Err("Premiere selects an absent physical audio channel".into()),
        };
        if selectors.iter().any(|&(_, prior)| prior == selection) {
            return Err("duplicate physical channel in Premiere clip".into());
        }
        selectors.push((channel, selection));
    }
    if selectors.is_empty() {
        return Err("missing Premiere saved audio channel selectors".into());
    }
    Ok(AudioTemplate { source, selectors })
}

fn label(physical: &Physical, (ordinal, channel): (usize, i64)) -> i64 {
    let prior_channels: i64 = physical.audio[..ordinal]
        .iter()
        .filter_map(|s| s.channels)
        .sum();
    prior_channels + channel + 1
}

impl ItemBindings {
    pub(super) fn read(
        project: &Project,
        master: NodeId,
        primary: NodeId,
        path: &Path,
        physical: &Physical,
    ) -> Result<Self> {
        let tree = &project.doc.tree;
        let mut video_source = None;
        let mut audio = Vec::new();
        for clip in references(project, master, "Clips/Clip", 3)? {
            match tree.tag(clip) {
                "VideoClip" if video_source.is_none() => {
                    let source = clip_source(project, clip, StreamKind::Video)?;
                    if reference(project, source, "MediaSource/Media")? != primary {
                        return Err("Premiere video template and primary Media disagree".into());
                    }
                    let stream = physical
                        .video
                        .as_ref()
                        .ok_or("missing physical picture for Premiere video template")?;
                    stream_cache(project, primary, physical, stream)?;
                    video_source = Some(source);
                }
                "AudioClip" => audio.push(audio_template(project, clip, primary, path, physical)?),
                _ => return Err("unqualified or duplicate Premiere master clip template".into()),
            }
        }
        if video_source.is_none() && audio.is_empty() {
            return Err("Premiere master has no physical clip templates".into());
        }
        let mut groups = Vec::new();
        if !audio.is_empty() {
            let vector = reference(project, master, "AudioClipChannelGroups")?;
            for group in references(
                project,
                vector,
                "ClipChannelVectors/ClipChannelVectorItem",
                2,
            )? {
                let members = references(project, group, "ClipChannels/ClipChannelItem", 2)?;
                let kind = leaf(tree, group, "ChannelType")?;
                if !matches!((kind, members.len()), (Some("0"), 1) | (Some("1"), 2)) {
                    return Err("unsupported Premiere saved audio group".into());
                }
                let mut labels = Vec::with_capacity(members.len());
                for member in members {
                    let index: usize = leaf(tree, member, "SourceClipIndex")?
                        .ok_or("missing Premiere audio template index")?
                        .parse()?;
                    let channel: usize = leaf(tree, member, "mSourceChannelIndex")?
                        .ok_or("missing Premiere group channel")?
                        .parse()?;
                    let template = audio
                        .get(index)
                        .ok_or("Premiere audio group names a missing template")?;
                    let (_, selection) = template
                        .selectors
                        .iter()
                        .find(|(saved, _)| *saved == channel)
                        .ok_or("Premiere audio group names a missing channel")?;
                    let value = label(physical, *selection);
                    if labels.contains(&value) {
                        return Err("duplicate physical channel in Premiere audio group".into());
                    }
                    labels.push(value);
                }
                groups.push(labels);
            }
            if groups.is_empty() {
                return Err("Premiere audio master has no saved channel groups".into());
            }
        }
        Ok(Self {
            video_source,
            audio,
            groups,
        })
    }

    pub(super) fn has_picture(&self) -> bool {
        self.video_source.is_some()
    }

    pub(super) fn video(&self, project: &Project, clip: NodeId) -> Result<()> {
        if Some(clip_source(project, clip, StreamKind::Video)?) != self.video_source {
            return Err("Premiere picture edit and its master bind different sources".into());
        }
        Ok(())
    }

    pub(super) fn audio(
        &self,
        project: &Project,
        clip: NodeId,
        subclip: NodeId,
        primary: NodeId,
        path: &Path,
        physical: &Physical,
    ) -> Result<Vec<i64>> {
        let tree = &project.doc.tree;
        let edit = audio_template(project, clip, primary, path, physical)?;
        if !self.audio.iter().any(|t| {
            t.source == edit.source && edit.selectors.iter().all(|pair| t.selectors.contains(pair))
        }) {
            return Err("Premiere audio edit and its master bind different sources".into());
        }
        let group: usize = leaf(tree, subclip, "OrigChGrp")?
            .map(str::parse)
            .transpose()?
            .unwrap_or(0);
        let secondary: Option<usize> = leaf(tree, clip, "SecondaryIndex")?
            .map(str::parse)
            .transpose()?;
        if secondary.is_some_and(|n| n != group) {
            return Err("Premiere audio edit has contradictory group indices".into());
        }
        let labels: Vec<_> = edit
            .selectors
            .iter()
            .map(|&(_, selection)| label(physical, selection))
            .collect();
        // The retained split-WAV save indexes the channels of its single
        // stereo master group. Verify both saved indices and the edit selector.
        let split_wav = physical.container == "wav"
            && labels.len() == 1
            && matches!(self.groups.as_slice(), [stereo] if stereo.len() == 2 && stereo.get(group) == labels.first())
            && (group == 0 || secondary == Some(group));
        let layout =
            leaf(tree, clip, "AudioChannelLayout")?.ok_or("missing Premiere edit audio layout")?;
        let layout: Vec<serde_json::Value> = serde_json::from_str(layout)?;
        let valid_layout = matches!(layout.as_slice(), [mono] if labels.len() == 1 && mono["channellabel"] == 0)
            || matches!(layout.as_slice(), [left, right] if labels.len() == 2
                && left["channellabel"] == 100 && right["channellabel"] == 101);
        if !valid_layout || self.groups.get(group) != Some(&labels) && !split_wav {
            return Err(
                "Premiere audio edit selectors contradict its saved channel group or layout".into(),
            );
        }
        Ok(labels)
    }
}
