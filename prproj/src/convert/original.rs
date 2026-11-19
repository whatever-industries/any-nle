//! Ordinary file-backed original masters. Physical streams and channels are explicit.
use super::model::{ticks_per_unit, Event, Fraction};
use super::prepared::*;
use crate::build::media as m;
use crate::error::{Error, Result};
use crate::ids::UuidSource;
use crate::model::project::resolve_path;
use crate::model::{ItemId, Project};
use crate::xml::mutations::{append_child, append_leaf};

pub(super) fn mono_source_channel(clip: &OriginalClip, event: &Event, slot: usize) -> Option<i64> {
    if event.audio_channels.is_empty() {
        clip.channels.get(slot).map(|channel| channel.avid_track)
    } else {
        event.audio_channels.get(slot).copied()
    }
}

pub fn add_item(
    project: &mut Project,
    c: &OriginalClip,
    name: &str,
    ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    validate_original(c).map_err(|e| Error::Value(e.to_string()))?;
    let dir = resolve_path(project.path())
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(audio) = c.audio.as_ref().filter(|a| a.channels == 8) {
        return add_eight_mono_wav(project, c, audio, name, &dir, ids);
    }
    let state = ids.uuid4().to_string();
    let duration = c
        .video
        .as_ref()
        .map(|v| v.duration)
        .or_else(|| c.audio.as_ref().map(|a| a.duration))
        .unwrap();
    let frame_rate = c
        .video
        .as_ref()
        .map(|v| ticks_per_unit(v.rate))
        .unwrap_or_else(|| ticks_per_unit(Fraction::new(c.audio.as_ref().unwrap().sample_rate, 1)));
    let source_start = c.video.as_ref().map_or(0, |v| v.source_start);
    let still = match c.container.as_str() {
        "jpeg" => m::still_profile(".jpg"),
        "png" => m::still_profile(".png"),
        "tiff" => m::still_profile(".tif"),
        _ => None,
    };
    let label = if c.video.is_some() {
        if c.audio.is_some() {
            (m::AV_LABEL_INDEX, m::AV_LABEL_COLOR)
        } else {
            (m::VIDEO_LABEL_INDEX, m::VIDEO_LABEL_COLOR)
        }
    } else {
        (m::AUDIO_LABEL_INDEX, m::AUDIO_LABEL_COLOR)
    };
    let mut audio_stream = None;
    if let Some(a) = &c.audio {
        let sample = match a.representation {
            SampleRepresentation::Aac => Some(m::AAC_SAMPLE_TYPE),
            SampleRepresentation::Mp3 => m::compressed_sample_type(".mp3"),
            SampleRepresentation::Pcm16 => Some("3"),
            SampleRepresentation::Pcm16Be => Some("11"),
            SampleRepresentation::Pcm24 => None,
        };
        let e = m::new_audio_stream(
            &mut project.doc.tree,
            ticks_per_unit(Fraction::new(a.sample_rate, 1)),
            &m::audio_channel_layout(if c.separate_mono_streams { 1 } else { a.channels as u32 }),
            a.duration,
            sample,
        );
        audio_stream = Some(project.register(e)?);
    }
    let uid = ids.uuid4().to_string();
    let e = if let (Some(v), Some(profile)) = (&c.video, still) {
        let e = m::new_video_stream(&mut project.doc.tree, v.width, v.height, frame_rate, profile);
        let stream = project.register(e)?;
        m::new_media(&mut project.doc.tree, ids, &uid, &stream, &c.path, &dir, &state)
    } else if let Some(v) = &c.video {
        let e = m::new_movie_video_stream(
            &mut project.doc.tree,
            v.width,
            v.height,
            ticks_per_unit(v.rate),
            v.duration,
            &c.container,
            &v.codec,
            0,
        )?;
        if c.container == "mxf" {
            // Premiere's native DNxHR OP1a import identifies these YCbCr
            // values as display-referred BT.709 with a valid range.
            let tree = &mut project.doc.tree;
            let color = tree.find1(e, "OriginalColorSpace")
                .ok_or_else(|| Error::Value("linked MXF color field is absent".into()))?;
            tree.set_text(color, Some(r#"{"baseColorProfile":{"colorProfileName":"BT.709,32f,Display-Referred"},"baseProfileType":1,"colorSpaceMetadata":{"dataRangeInfoValid":1}}"#));
        }
        let stream = project.register(e)?;
        m::new_movie_media(
            &mut project.doc.tree,
            ids,
            &uid,
            &stream,
            &c.path,
            &dir,
            &state,
            audio_stream.as_deref(),
            c.audio
                .as_ref()
                .map(|a| ticks_per_unit(Fraction::new(a.sample_rate, 1))),
            source_start,
        )
    } else {
        let a = c.audio.as_ref().unwrap();
        let key = ids.uuid4().to_string();
        let hash = ids.uuid4().to_string();
        m::new_audio_media(
            &mut project.doc.tree,
            &uid,
            audio_stream.as_deref().unwrap(),
            &c.path,
            &dir,
            &state,
            ticks_per_unit(Fraction::new(a.sample_rate, 1)),
            &key,
            &hash,
            0,
        )
    };
    let reel = c
        .metadata
        .tape_name
        .as_deref()
        .filter(|value| c.video.is_some() && !value.is_empty());
    if let Some(reel) = reel {
        append_leaf(&mut project.doc.tree, e, "AlternateTapeName", reel);
        append_leaf(&mut project.doc.tree, e, "TapeName", reel);
    }
    let media = project.register_uid(e, &uid)?;
    project.imported_media.push(media);
    let proxy = if let Some(path) = &c.proxy_path {
        let v = c.video.as_ref().expect("validated proxy video");
        let e = m::new_movie_video_stream(
            &mut project.doc.tree,
            v.width,
            v.height,
            ticks_per_unit(v.rate),
            v.duration,
            if c.audio.is_some() { "mov" } else { "mxf" },
            "AVdn",
            0,
        )?;
        append_leaf(&mut project.doc.tree, e, "IsFrameRectOverridden", "true");
        append_leaf(
            &mut project.doc.tree,
            e,
            "OverriddenFrameRect",
            &format!("0,0,{},{}", v.width, v.height),
        );
        let vid = project.register(e)?;
        let audio = c
            .audio
            .as_ref()
            .map(|a| {
                let e = m::new_audio_stream(
                    &mut project.doc.tree,
                    ticks_per_unit(Fraction::new(a.sample_rate, 1)),
                    &m::audio_channel_layout(a.channels as u32),
                    a.duration,
                    (a.representation == SampleRepresentation::Pcm16).then_some("3"),
                );
                project.register(e)
            })
            .transpose()?;
        let proxy_uid = ids.uuid4().to_string();
        let state = ids.uuid4().to_string();
        let e = m::new_movie_media(
            &mut project.doc.tree,
            ids,
            &proxy_uid,
            &vid,
            path,
            &dir,
            &state,
            audio.as_deref(),
            c.audio
                .as_ref()
                .map(|a| ticks_per_unit(Fraction::new(a.sample_rate, 1))),
            source_start,
        );
        append_leaf(&mut project.doc.tree, e, "IsProxy", "true");
        append_leaf(&mut project.doc.tree, e, "AlternateStart", "0");
        let node = project.register_uid(e, &proxy_uid)?;
        project.imported_media.push(node);
        Some(proxy_uid)
    } else {
        None
    };
    let e = m::new_markers_collection(&mut project.doc.tree, &state);
    let markers = project.register(e)?;
    let format = if let Some(v) = &c.video {
        let verified_drop = v.codec == "apch" && v.rate == Fraction::new(30_000, 1001)
            && c.metadata.time_display == Some(102);
        super::rewrap::display_format(v.rate, verified_drop)
            .ok_or_else(|| Error::NotImplemented("original picture time display".into()))?
            .to_string()
    } else {
        "200".into()
    };
    let e = m::new_media_logging_info(
        &mut project.doc.tree,
        name,
        frame_rate,
        duration,
        if c.video.is_none() {
            Some("1")
        } else if c.audio.is_none() {
            Some("2")
        } else {
            None
        },
        &format,
        source_start,
        "",
    );
    if let Some(reel) = reel {
        append_leaf(&mut project.doc.tree, e, "TapeName", reel);
    }
    let logging = project.register(e)?;
    let mut clips = Vec::new();
    let mut chains = Vec::new();
    let mut vectors = Vec::new();
    if let Some(v) = &c.video {
        let e = if still.is_some() {
            m::new_media_source(&mut project.doc.tree, &uid)
        } else {
            m::new_movie_media_source(&mut project.doc.tree, &uid, v.duration)
        };
        if let Some(uid) = &proxy {
            let content = project
                .doc
                .tree
                .find1(e, "MediaSource/Content")
                .expect("video content");
            let reference = project
                .doc
                .tree
                .new_element_with("ProxyMedia", &[("ObjectURef", uid)]);
            append_child(&mut project.doc.tree, content, reference, None);
        }
        let source = project.register(e)?;
        let e = if still.is_some() {
            m::new_template_clip(&mut project.doc.tree, ids, &markers, &source, label.0, label.1, v.duration)
        } else {
            m::new_movie_template_clip(&mut project.doc.tree, ids, &markers, &source, label.0, label.1)
        };
        clips.push(project.register(e)?);
    }
    if let Some(a) = &c.audio {
        let source_node = m::new_audio_media_source(&mut project.doc.tree, &uid, a.duration);
        let source = project.register(source_node)?;
        let second_source = if c.separate_mono_streams {
            let sample_rate = ticks_per_unit(Fraction::new(a.sample_rate, 1));
            let stream = m::new_audio_stream(&mut project.doc.tree, sample_rate,
                &m::audio_channel_layout(1), a.duration, None);
            let stream_id = project.register(stream)?;
            let sibling_uid = ids.uuid4().to_string();
            let tree = &mut project.doc.tree;
            let file_key = tree.find1(media, "FileKey")
                .and_then(|n| tree.text(n))
                .ok_or_else(|| Error::Value("linked original FileKey is absent".into()))?.to_owned();
            let hash = tree.find1(media, "ModificationState")
                .and_then(|n| tree[n].get("BinaryHash"))
                .ok_or_else(|| Error::Value("linked original modification hash is absent".into()))?.to_owned();
            let e = m::new_audio_media(tree, &sibling_uid, &stream_id, &c.path, &dir, &state,
                sample_rate, &file_key, &hash, 1);
            let sibling = project.register_uid(e, &sibling_uid)?;
            project.imported_media.push(sibling);
            let second = m::new_audio_media_source(&mut project.doc.tree, &sibling_uid, a.duration);
            Some(project.register(second)?)
        } else { None };
        let mut proxy_refs = Vec::new();
        if let Some(uid) = &proxy {
            for channel in 0..a.channels {
                let e = crate::build::top(
                    &mut project.doc.tree,
                    "AudioProxy",
                    "56e9cdb3-7244-44ff-98a3-f1932ebf202d",
                    "1",
                    None,
                );
                let tree = &mut project.doc.tree;
                let r = tree.new_element_with("ProxyMedia", &[("ObjectURef", uid)]);
                append_child(tree, e, r, None);
                append_leaf(tree, e, "ProxyStreamIndex", &channel.to_string());
                append_leaf(tree, e, "OriginalSecondaryIndex", &channel.to_string());
                proxy_refs.push(project.register(e)?);
            }
        }
        if !proxy_refs.is_empty() {
            let tree = &mut project.doc.tree;
            let content = tree
                .find1(source_node, "MediaSource/Content")
                .expect("audio content");
            let proxies = tree.new_element_with("AudioProxies", &[("Version", "1")]);
            append_child(tree, content, proxies, None);
            for (index, id) in proxy_refs.iter().enumerate() {
                let r = tree.new_element_with(
                    "AudioProxyItem",
                    &[("Index", &index.to_string()), ("ObjectRef", id)],
                );
                append_child(tree, proxies, r, None);
            }
        }
        for (group, g) in c.groups.iter().enumerate() {
            let group_source = if group == 1 { second_source.as_ref().unwrap_or(&source) } else { &source };
            let physical_channels = if c.separate_mono_streams { 1 } else { a.channels };
            let mut secondary = Vec::new();
            for n in 0..physical_channels {
                let e = m::new_secondary_content(&mut project.doc.tree, group_source, n);
                secondary.push(project.register(e)?);
            }
            let refs: Vec<_> = secondary.iter().map(String::as_str).collect();
            let e = m::new_audio_template_clip(
                &mut project.doc.tree,
                ids,
                &markers,
                group_source,
                &refs,
                label.0,
                label.1,
                &m::audio_channel_layout(physical_channels as u32),
            );
            let gain = c
                .channels
                .iter()
                .find(|c| c.avid_track == g[0])
                .unwrap()
                .gain_db;
            if gain != 0.0 {
                m::opatom::set_master_gain(&mut project.doc.tree, e, gain);
            }
            clips.push(project.register(e)?);
            let mut members = Vec::new();
            for lane in g {
                let channel = c.channels.iter().find(|c| c.avid_track == *lane).unwrap();
                let e = m::new_channel_serializer(
                    &mut project.doc.tree,
                    group as i64,
                    if c.separate_mono_streams { 0 } else { channel.channel_index },
                );
                members.push(project.register(e)?);
            }
            let refs: Vec<_> = members.iter().map(String::as_str).collect();
            let kind = if g.len() == 2 { 1 } else { 0 };
            let e = m::new_channel_vector(&mut project.doc.tree, &refs, kind);
            vectors.push(project.register(e)?);
            let e = m::new_master_audio_chain(&mut project.doc.tree, g.len() as u32, kind);
            chains.push(project.register(e)?);
        }
    }
    let refs: Vec<_> = vectors.iter().map(String::as_str).collect();
    let e = if refs.is_empty() {
        m::new_channel_groups(&mut project.doc.tree)
    } else {
        m::new_audio_channel_groups(&mut project.doc.tree, &refs)
    };
    let groups = project.register(e)?;
    let master_uid = ids.uuid4().to_string();
    let refs: Vec<_> = clips.iter().map(String::as_str).collect();
    let chain_refs: Vec<_> = chains.iter().map(String::as_str).collect();
    let e = if chains.is_empty() {
        m::new_master_clip(
            &mut project.doc.tree,
            &master_uid,
            &logging,
            &clips[0],
            &groups,
            name,
        )
    } else {
        m::new_audio_master_clip(
            &mut project.doc.tree,
            ids,
            &master_uid,
            &logging,
            &chain_refs,
            &refs,
            &groups,
            name,
        )
    };
    let master = project.register_uid(e, &master_uid)?;
    let item_uid = ids.uuid4().to_string();
    let e = m::new_clip_item(&mut project.doc.tree, &item_uid, &master_uid, name, label.0);
    let e = project.register_uid(e, &item_uid)?;
    let item = project.attach_imported_item(e, &item_uid, master, &refs, &c.path, duration)?;
    project.item_extra.entry(item).or_default().channel_groups = Some(
        c.groups
            .iter()
            .map(|g| {
                g.iter()
                    .map(|n| c.channels.iter().position(|c| c.avid_track == *n).unwrap())
                    .collect()
            })
            .collect(),
    );
    Ok(item)
}

/// Premiere saves one mono media view per WAV channel, with a common file key.
fn add_eight_mono_wav(
    project: &mut Project,
    clip: &OriginalClip,
    audio: &OriginalAudio,
    name: &str,
    dir: &str,
    ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    let state = ids.uuid4().to_string();
    let key = ids.uuid4().to_string();
    let hash = ids.uuid4().to_string();
    let sample_clock = ticks_per_unit(Fraction::new(audio.sample_rate, 1));
    let mono = m::audio_channel_layout(1);
    let mut templates = Vec::new();
    let mut vectors = Vec::new();
    let mut chains = Vec::new();
    for group in 0..8 {
        let ch = clip
            .channels
            .iter()
            .find(|ch| ch.avid_track == group + 1)
            .expect("validated one channel per lane");
        let stream = m::new_audio_stream(
            &mut project.doc.tree,
            sample_clock,
            &mono,
            audio.duration,
            Some("3"),
        );
        let stream = project.register(stream)?;
        let uid = ids.uuid4().to_string();
        let media = m::new_audio_media(
            &mut project.doc.tree,
            &uid,
            &stream,
            &clip.path,
            dir,
            &state,
            sample_clock,
            &key,
            &hash,
            ch.channel_index,
        );
        append_leaf(
            &mut project.doc.tree,
            media,
            "Start",
            &audio.source_start.to_string(),
        );
        append_leaf(&mut project.doc.tree, media, "TimeDisplay", "100");
        let media = project.register_uid(media, &uid)?;
        project.imported_media.push(media);
        let source = m::new_audio_media_source(&mut project.doc.tree, &uid, audio.duration);
        let source = project.register(source)?;
        let secondary = m::new_secondary_content(&mut project.doc.tree, &source, 0);
        let secondary = project.register(secondary)?;
        let markers = m::new_markers_collection(&mut project.doc.tree, &state);
        let markers = project.register(markers)?;
        let template = m::new_audio_template_clip(
            &mut project.doc.tree,
            ids,
            &markers,
            &source,
            &[&secondary],
            m::AUDIO_LABEL_INDEX,
            m::AUDIO_LABEL_COLOR,
            &mono,
        );
        if ch.gain_db != 0.0 {
            m::opatom::set_master_gain(&mut project.doc.tree, template, ch.gain_db);
        }
        templates.push(project.register(template)?);
        let member = m::new_channel_serializer(&mut project.doc.tree, group, 0);
        let member = project.register(member)?;
        let vector = m::new_channel_vector(&mut project.doc.tree, &[&member], 0);
        vectors.push(project.register(vector)?);
        let chain = m::new_master_audio_chain(&mut project.doc.tree, 1, 0);
        chains.push(project.register(chain)?);
    }

    let refs: Vec<_> = vectors.iter().map(String::as_str).collect();
    let groups = m::new_audio_channel_groups(&mut project.doc.tree, &refs);
    let groups = project.register(groups)?;
    let log = m::new_media_logging_info(
        &mut project.doc.tree,
        name,
        sample_clock,
        audio.duration,
        Some("1"),
        "200",
        audio.source_start,
        "",
    );
    let log = project.register(log)?;
    let refs: Vec<_> = templates.iter().map(String::as_str).collect();
    let chain_refs: Vec<_> = chains.iter().map(String::as_str).collect();
    let master_uid = ids.uuid4().to_string();
    let master = m::new_audio_master_clip(
        &mut project.doc.tree,
        ids,
        &master_uid,
        &log,
        &chain_refs,
        &refs,
        &groups,
        name,
    );
    m::opatom::set_time_display(&mut project.doc.tree, master, 100)?;
    let master = project.register_uid(master, &master_uid)?;
    let item_uid = ids.uuid4().to_string();
    let item = m::new_clip_item(
        &mut project.doc.tree,
        &item_uid,
        &master_uid,
        name,
        m::AUDIO_LABEL_INDEX,
    );
    let item = project.register_uid(item, &item_uid)?;
    let item =
        project.attach_imported_item(item, &item_uid, master, &refs, &clip.path, audio.duration)?;
    project.item_extra.entry(item).or_default().channel_groups =
        Some((0..8).map(|n| vec![n]).collect());
    Ok(item)
}
