//! Ordinary file-backed original masters. Physical streams and channels are explicit.
use super::model::{ticks_per_unit, Fraction};
use super::prepared::*;
use crate::build::media as m;
use crate::error::{Error, Result};
use crate::ids::UuidSource;
use crate::model::project::resolve_path;
use crate::model::{ItemId, Project};

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
            SampleRepresentation::Pcm16 => Some("3"),
            SampleRepresentation::Pcm24 => None,
        };
        let e = m::new_audio_stream(
            &mut project.doc.tree,
            ticks_per_unit(Fraction::new(a.sample_rate, 1)),
            &m::audio_channel_layout(a.channels as u32),
            a.duration,
            sample,
        );
        audio_stream = Some(project.register(e)?);
    }
    let uid = ids.uuid4().to_string();
    let e = if let Some(v) = &c.video {
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
            0,
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
    let media = project.register_uid(e, &uid)?;
    project.imported_media.push(media);
    let e = m::new_markers_collection(&mut project.doc.tree, &state);
    let markers = project.register(e)?;
    let format = if let Some(v) = &c.video {
        super::rewrap::display_format(v.rate, false)
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
        0,
        "",
    );
    let logging = project.register(e)?;
    let mut clips = Vec::new();
    let mut chains = Vec::new();
    let mut vectors = Vec::new();
    if let Some(v) = &c.video {
        let e = m::new_movie_media_source(&mut project.doc.tree, &uid, v.duration);
        let source = project.register(e)?;
        let e = m::new_movie_template_clip(
            &mut project.doc.tree,
            ids,
            &markers,
            &source,
            label.0,
            label.1,
        );
        clips.push(project.register(e)?);
    }
    if let Some(a) = &c.audio {
        let e = m::new_audio_media_source(&mut project.doc.tree, &uid, a.duration);
        let source = project.register(e)?;
        for (group, g) in c.groups.iter().enumerate() {
            let mut secondary = Vec::new();
            for n in 0..a.channels {
                let e = m::new_secondary_content(&mut project.doc.tree, &source, n);
                secondary.push(project.register(e)?);
            }
            let refs: Vec<_> = secondary.iter().map(String::as_str).collect();
            let e = m::new_audio_template_clip(
                &mut project.doc.tree,
                ids,
                &markers,
                &source,
                &refs,
                label.0,
                label.1,
                &m::audio_channel_layout(a.channels as u32),
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
                    channel.channel_index,
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
