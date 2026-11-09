//! Explicit experimental, file-backed OP1a AV masters. No hidden source sequences.
//! Full-source templates retain physical channel layouts; logical vectors select
//! channel assignments independently of container stream indices.
use super::model::{event_audio_files, ticks_per_unit, Clip, Model};
use super::opatom::{ensure_audio_media, ensure_video_media, MediaTable};
use super::rewrap::MediaRelink;
use super::writer::{Result, WriteError};
use crate::build::media::{self as m, opatom as op};
use crate::build::time::Time;
use crate::ids::UuidSource;
use crate::model::{ItemId, Project, TrackId, TrackItemId};
use std::collections::{HashMap, HashSet};

fn invalid(message: impl Into<String>) -> WriteError {
    WriteError::Unverified(super::opatom::UnverifiedShape(format!(
        "native OP1a master: {}",
        message.into()
    )))
}

/// Model indices in each logical channel group. Order follows the explicit Avid labels;
/// each stereo tuple retains its saved L/R order, even when reversed/non-adjacent.
pub fn groups(clip: &Clip) -> Result<Vec<Vec<usize>>> {
    if !clip.multichannel_groups.is_empty() {
        let mut declared: Vec<Vec<i64>> = clip
            .stereo_pairs
            .iter()
            .map(|(l, r)| vec![*l, *r])
            .collect();
        declared.extend(clip.multichannel_groups.iter().map(|g| g.channels.clone()));
        let mut order: Vec<_> = (0..clip.audio.len()).collect();
        order.sort_by_key(|i| clip.audio[*i].avid_track);
        let mut answer = Vec::new();
        for i in order {
            let label = clip.audio[i]
                .avid_track
                .ok_or_else(|| invalid("missing Avid audio label"))?;
            if let Some(g) = declared.iter().find(|g| g.first() == Some(&label)) {
                answer.push(
                    g.iter()
                        .map(|n| {
                            clip.audio
                                .iter()
                                .position(|a| a.avid_track == Some(*n))
                                .ok_or_else(|| invalid("missing declared channel"))
                        })
                        .collect::<Result<Vec<_>>>()?,
                );
            } else if !declared.iter().any(|g| g.contains(&label)) {
                answer.push(vec![i]);
            }
        }
        return Ok(answer);
    }
    super::combined::channel_groups(clip).map(|groups| {
        let mut order: Vec<_> = (0..clip.audio.len()).collect();
        order.sort_by_key(|&i| clip.audio[i].avid_track);
        groups
            .into_iter()
            .map(|g| g.into_iter().map(|i| order[i]).collect())
            .collect()
    })
}

fn group_type(clip: &Clip, group: &[usize]) -> Result<i64> {
    if group.len() == 1 {
        return Ok(0);
    }
    if group.len() == 2 {
        return Ok(1);
    }
    let labels: Vec<_> = group
        .iter()
        .filter_map(|i| clip.audio[*i].avid_track)
        .collect();
    let g = clip
        .multichannel_groups
        .iter()
        .find(|g| g.channels == labels)
        .ok_or_else(|| invalid("missing explicit multichannel declaration"))?;
    Ok(if g.format == "5.1" { 2 } else { 3 })
}
fn adaptive_layout() -> String {
    format!("[{}]", vec!["{\"channellabel\":0}"; 32].join(","))
}
fn premiere_indices(clip: &Clip, group: &[usize]) -> Result<Vec<usize>> {
    if group.len() <= 2 {
        return Ok(group.to_vec());
    }
    let labels: Vec<_> = group
        .iter()
        .filter_map(|i| clip.audio[*i].avid_track)
        .collect();
    let declaration = clip
        .multichannel_groups
        .iter()
        .find(|g| g.channels == labels)
        .ok_or_else(|| invalid("missing explicit multichannel declaration"))?;
    declaration
        .premiere_channels()
        .iter()
        .map(|label| {
            clip.audio
                .iter()
                .position(|a| a.avid_track == Some(*label))
                .ok_or_else(|| invalid("missing declared channel"))
        })
        .collect()
}

/// Cheap refusal pass, called before backend execution as well as by the writer.
pub fn sequence_channels(model: &Model, seq: &super::model::Sequence) -> Result<usize> {
    let mut widths = HashSet::new();
    for e in &seq.events {
        for st in &e.stereo {
            if st.channels.len() > 2 {
                widths.insert(st.channels.len());
            }
        }
    }
    if widths.is_empty() {
        return Ok(0);
    }
    if widths.len() != 1 {
        return Err(invalid(
            "mixed 5.1/7.1 timeline output needs explicit destination routing",
        ));
    }
    let width = *widths.iter().next().unwrap();
    for e in &seq.events {
        if !event_audio_files(&model.clips[&e.clip], e)?.is_empty()
            || e.stereo.iter().any(|st| st.channels.len() != width)
        {
            return Err(invalid(
                "mixed mono/stereo/surround timeline output needs explicit destination routing",
            ));
        }
    }
    Ok(width)
}

pub fn preflight(model: &Model) -> Result<()> {
    super::model::validate_model(model)?;
    let used: HashSet<&str> = model
        .bins
        .iter()
        .flat_map(|b| b.clips.iter().map(String::as_str))
        .chain(
            model
                .sequences
                .iter()
                .flat_map(|s| s.events.iter().map(|e| e.clip.as_str())),
        )
        .collect();
    for clip in model
        .clips
        .values()
        .filter(|c| used.contains(c.id.as_str()))
    {
        let v = clip
            .video
            .as_ref()
            .ok_or_else(|| invalid("requires picture-backed masters"))?;
        if !matches!(v.codec.as_str(), "AVdn" | "apcs") || !super::rewrap::supported_rate(v.rate) {
            return Err(invalid(
                "requires supported DNx/ProRes picture and an exact rewrap rate",
            ));
        }
        if clip.audio.len() > 32 {
            return Err(invalid("more than 32 mono sources"));
        }
        let mut labels = HashSet::new();
        for a in &clip.audio {
            if a.bits != clip.audio[0].bits {
                return Err(invalid(
                    "mixed PCM bit depths in one OP1a file are not supported; no sample conversion",
                ));
            }
            if !a.avid_track.is_some_and(|t| t > 0 && labels.insert(t)) {
                return Err(invalid("missing or duplicate Avid audio labels"));
            }
            if a.sample_rate != 48000
                || !matches!(a.bits, 16 | 24)
                || a.duration != v.duration
                || a.start != v.start
            {
                return Err(invalid(
                    "requires matching full-span mono 48 kHz PCM16/24 sources and clocks",
                ));
            }
        }
        let gs = groups(clip)?;
        for g in &gs {
            if g.len() > 1
                && g.iter()
                    .any(|i| clip.audio[*i].gain_db != clip.audio[g[0]].gain_db)
            {
                return Err(invalid(
                    "unequal grouped master gains require per-channel gain support",
                ));
            }
        }
    }
    for seq in &model.sequences {
        sequence_channels(model, seq)?;
        for e in &seq.events {
            let c = &model.clips[&e.clip];
            let gs = groups(c)?;
            if !e.stereo.is_empty()
                && (e.audio.volume_db.is_some() || !e.audio.volume_keys.is_empty())
            {
                return Err(invalid(
                    "stereo clip volume/automation needs a stereo volume component",
                ));
            }
            for a in event_audio_files(c, e)? {
                let i = c.audio.iter().position(|f| std::ptr::eq(f, a)).unwrap();
                if !gs.contains(&vec![i]) {
                    return Err(invalid(
                        "mono edit of a grouped stereo member requires a derived source mapping",
                    ));
                }
            }
            for st in &e.stereo {
                let wanted = st
                    .channels
                    .iter()
                    .map(|t| c.audio.iter().position(|a| a.avid_track == Some(*t)))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| invalid("unknown stereo source label"))?;
                if !matches!(wanted.len(), 2 | 6 | 8) || !gs.contains(&wanted) {
                    return Err(invalid("edit channel order must match a saved bin group"));
                }
                if st.pans.iter().any(Option::is_some)
                    && (wanted.len() != 2
                        || !(st.pans[0].is_none_or(|p| p == -100.0)
                            && st.pans[1].is_none_or(|p| p == 100.0)))
                {
                    return Err(invalid(
                        "non-default per-channel stereo pan requires additional support",
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn validate_bindings(model: &Model, mappings: &[MediaRelink]) -> Result<()> {
    preflight(model)?;
    let mut paths = HashMap::new();
    for m in mappings {
        if !super::model::is_absolute_path(&m.target_path)
            || m.source_path == m.target_path
            || m.target_path
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{fffe}' | '\u{ffff}'))
        {
            return Err(invalid(
                "binding requires a distinct XML-safe absolute target path",
            ));
        }
        if let Some(old) = paths.insert(m.source_path.as_str(), m) {
            if old != m {
                return Err(invalid("conflicting media bindings"));
            }
        }
    }
    let used: HashSet<&str> = model
        .bins
        .iter()
        .flat_map(|b| b.clips.iter().map(String::as_str))
        .chain(
            model
                .sequences
                .iter()
                .flat_map(|s| s.events.iter().map(|e| e.clip.as_str())),
        )
        .collect();
    let wanted: HashSet<&str> = model
        .clips
        .values()
        .filter(|c| used.contains(c.id.as_str()))
        .flat_map(|c| {
            std::iter::once(c.video.as_ref().unwrap().path.as_str())
                .chain(c.audio.iter().map(|a| a.path.as_str()))
        })
        .collect();
    if paths.keys().any(|p| !wanted.contains(p)) {
        return Err(invalid("binding names an unreferenced source"));
    }
    for c in model
        .clips
        .values()
        .filter(|c| used.contains(c.id.as_str()))
    {
        let v = c.video.as_ref().unwrap();
        let vm = paths
            .get(v.path.as_str())
            .ok_or_else(|| invalid("missing picture binding"))?;
        if vm.audio_stream.is_some() {
            return Err(invalid("picture binding is an audio selector"));
        }
        let mut ordered: Vec<_> = c.audio.iter().collect();
        ordered.sort_by_key(|a| a.avid_track);
        for (i, a) in ordered.iter().enumerate() {
            let am = paths
                .get(a.path.as_str())
                .ok_or_else(|| invalid("missing audio binding"))?;
            if am.target_path != vm.target_path || am.audio_stream != Some(i as u32) {
                return Err(invalid(
                    "audio binding disagrees with Avid-label-sorted output ordinals",
                ));
            }
        }
    }
    Ok(())
}

/// Compose an ordinary MasterClip with video and one AudioClip/chain per logical group.
/// SecondaryContent points at each selected mono source in saved L/R order. Multiple
/// logical masters may reuse the same physical Media views without sharing grouping.
pub fn add_item(
    project: &mut Project,
    media: &mut MediaTable,
    clip: &Clip,
    name: &str,
    ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    let v = clip
        .video
        .as_ref()
        .ok_or_else(|| invalid("missing picture"))?;
    let (video_uid, state) = ensure_video_media(project, media, v, clip, ids)?;
    let e = m::new_movie_media_source(&mut project.doc.tree, &video_uid, v.duration);
    let source = project.register(e)?;
    let e = m::new_markers_collection(&mut project.doc.tree, &state);
    let markers = project.register(e)?;
    let info = op::ClipInfo {
        name: &clip.name,
        tape_name: clip.tape_name.as_deref(),
        material_umid: clip.material_umid.as_deref(),
    };
    let format = super::rewrap::video_format(clip, v.rate)?;
    let e = op::logging(
        &mut project.doc.tree,
        &info,
        ticks_per_unit(v.rate),
        v.duration,
        "2",
        &format,
        v.start,
    );
    // Ordinary AV import omits CaptureMode (video-only retains 2).
    if !clip.audio.is_empty() {
        if let Some(n) = project.doc.tree.child_by_tag(e, "CaptureMode") {
            project.doc.tree.remove(e, n);
        }
    }
    let logging = project.register(e)?;
    let e = m::new_movie_template_clip(
        &mut project.doc.tree,
        ids,
        &markers,
        &source,
        op::VIDEO_LABEL.0,
        op::VIDEO_LABEL.1,
    );
    let mut clips = vec![project.register(e)?];
    let mut sources = Vec::new();
    for a in &clip.audio {
        let (uid, _) = ensure_audio_media(project, media, a, clip, ids)?;
        let e = m::new_audio_media_source(&mut project.doc.tree, &uid, a.duration);
        sources.push(project.register(e)?);
    }
    let gs = groups(clip)?;
    let ordinal = |i: usize| {
        clip.audio
            .iter()
            .filter(|a| a.avid_track < clip.audio[i].avid_track)
            .count()
    };
    let mono_views = media.mono_views.contains(&v.path);
    let mut chains = Vec::new();
    let mut vectors = Vec::new();
    if mono_views {
        for (i, a) in clip.audio.iter().enumerate() {
            let e = m::new_secondary_content(&mut project.doc.tree, &sources[i], 0);
            let secondary = project.register(e)?;
            let e = m::new_audio_template_clip(
                &mut project.doc.tree,
                ids,
                &markers,
                &sources[i],
                &[secondary.as_str()],
                op::VIDEO_LABEL.0,
                op::VIDEO_LABEL.1,
                &m::audio_channel_layout(1),
            );
            if a.gain_db != 0.0 {
                op::set_master_gain(&mut project.doc.tree, e, a.gain_db);
            }
            clips.push(project.register(e)?);
            let e = m::new_master_audio_chain(&mut project.doc.tree, 1, 0);
            chains.push(project.register(e)?);
        }
        for g in &gs {
            let mut channels = Vec::new();
            for i in premiere_indices(clip, g)? {
                let e = m::new_channel_serializer(&mut project.doc.tree, i as i64, 0);
                channels.push(project.register(e)?);
            }
            let refs: Vec<_> = channels.iter().map(String::as_str).collect();
            let e = m::new_channel_vector(&mut project.doc.tree, &refs, group_type(clip, g)?);
            vectors.push(project.register(e)?);
        }
    } else {
        for (group, g) in gs.iter().enumerate() {
            let channel_type = group_type(clip, g)?;
            let mut secondary = Vec::new();
            let mut channels = Vec::new();
            // Each gain-bearing source template describes the whole physical source.
            // Its channel vector selects only the logical group's physical channels.
            for channel in 0..clip.audio.len() {
                let e =
                    m::new_secondary_content(&mut project.doc.tree, &sources[g[0]], channel as i64);
                secondary.push(project.register(e)?);
            }
            for i in premiere_indices(clip, g)? {
                let e = m::new_channel_serializer(
                    &mut project.doc.tree,
                    group as i64,
                    ordinal(i) as i64,
                );
                channels.push(project.register(e)?);
            }
            let refs: Vec<_> = secondary.iter().map(String::as_str).collect();
            let e = m::new_audio_template_clip(
                &mut project.doc.tree,
                ids,
                &markers,
                &sources[g[0]],
                &refs,
                op::VIDEO_LABEL.0,
                op::VIDEO_LABEL.1,
                &format!(
                    "[{}]",
                    vec!["{\"channellabel\":0}"; clip.audio.len()].join(",")
                ),
            );
            if clip.audio[g[0]].gain_db != 0.0 {
                op::set_master_gain(&mut project.doc.tree, e, clip.audio[g[0]].gain_db);
            }
            clips.push(project.register(e)?);
            let refs: Vec<_> = channels.iter().map(String::as_str).collect();
            let e = m::new_channel_vector(&mut project.doc.tree, &refs, channel_type);
            vectors.push(project.register(e)?);
            let e = m::new_master_audio_chain(&mut project.doc.tree, g.len() as u32, channel_type);
            if channel_type == 3 {
                let n = project
                    .doc
                    .tree
                    .child_by_tag(e, "AudioChannelLayout")
                    .unwrap();
                project.doc.tree.set_text(n, Some(adaptive_layout()));
            }
            chains.push(project.register(e)?);
        }
    }
    let refs: Vec<_> = vectors.iter().map(String::as_str).collect();
    let e = if vectors.is_empty() {
        m::new_channel_groups(&mut project.doc.tree)
    } else {
        m::new_audio_channel_groups(&mut project.doc.tree, &refs)
    };
    let groups_id = project.register(e)?;
    let master_uid = ids.uuid4().to_string();
    let clip_refs: Vec<_> = clips.iter().map(String::as_str).collect();
    let chain_refs: Vec<_> = chains.iter().map(String::as_str).collect();
    let e = if chains.is_empty() {
        m::new_master_clip(
            &mut project.doc.tree,
            &master_uid,
            &logging,
            &clips[0],
            &groups_id,
            name,
        )
    } else {
        m::new_audio_master_clip(
            &mut project.doc.tree,
            ids,
            &master_uid,
            &logging,
            &chain_refs,
            &clip_refs,
            &groups_id,
            name,
        )
    };
    let master = project.register_uid(e, &master_uid)?;
    let item_uid = ids.uuid4().to_string();
    let e = m::new_clip_item(
        &mut project.doc.tree,
        &item_uid,
        &master_uid,
        name,
        op::VIDEO_LABEL.0,
    );
    let e = project.register_uid(e, &item_uid)?;
    let item =
        project.attach_imported_item(e, &item_uid, master, &clip_refs, &v.path, v.duration)?;
    project.item_extra.entry(item).or_default().channel_groups = Some(gs);
    Ok(item)
}

/// Place the requested audio template rather than Project::add_clip's first AudioClip.
/// Restore the master reference even on failure; the placed copy owns its secondary objects.
pub fn place_audio(
    project: &mut Project,
    track: TrackId,
    item: ItemId,
    group: usize,
    start: Time,
    ids: &mut dyn UuidSource,
) -> Result<TrackItemId> {
    let master = project
        .item(item)
        .master
        .ok_or_else(|| invalid("master missing"))?;
    let refs = project.doc.tree.findall(master, "Clips/Clip")?;
    let mut audio = Vec::new();
    for r in refs {
        if project.doc.tree.tag(project.doc.resolve(r)?) == "AudioClip" {
            audio.push(r);
        }
    }
    let first = *audio.first().ok_or_else(|| invalid("no audio template"))?;
    let wanted = *audio
        .get(group)
        .ok_or_else(|| invalid("group outside master"))?;
    let original = project
        .doc
        .tree
        .get(first, "ObjectRef")
        .unwrap()
        .to_string();
    let selected = project
        .doc
        .tree
        .get(wanted, "ObjectRef")
        .unwrap()
        .to_string();
    project.doc.tree.set(first, "ObjectRef", selected);
    let placed = project.add_clip(track, item, start, ids);
    project.doc.tree.set(first, "ObjectRef", original);
    let placed = placed?;
    let data = project.track_item(placed).clone();
    let groups = project
        .doc
        .tree
        .child_by_tag(master, "AudioClipChannelGroups")
        .ok_or_else(|| invalid("master lacks channel groups"))?;
    let groups = project.doc.resolve(groups)?;
    let vectors = project
        .doc
        .tree
        .findall(groups, "ClipChannelVectors/ClipChannelVectorItem")?;
    let vector = project.doc.resolve(vectors[group])?;
    let members = project
        .doc
        .tree
        .findall(vector, "ClipChannels/ClipChannelItem")?;
    let mut indices = Vec::new();
    let mut source_clips = Vec::new();
    for member in members {
        let member = project.doc.resolve(member)?;
        let index = project
            .doc
            .tree
            .child_by_tag(member, "mSourceChannelIndex")
            .and_then(|n| project.doc.tree.text(n))
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| invalid("missing physical source channel"))?;
        source_clips.push(
            project
                .doc
                .tree
                .findtext(member, "SourceClipIndex")?
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| invalid("missing source template selector"))?,
        );
        indices.push(index);
    }
    let first_template = project.doc.resolve(first)?;
    let mono_views = audio.len() > 1
        && project
            .doc
            .tree
            .findtext(first_template, "AudioChannelLayout")?
            .as_deref()
            == Some("[{\"channellabel\":0}]");
    let contents = project
        .doc
        .tree
        .child_by_tag(data.clip, "SecondaryContents")
        .ok_or_else(|| invalid("placed audio lacks source channels"))?;
    let refs = project.doc.tree.children(contents).to_vec();
    for &reference in &refs {
        project.doc.tree.remove(contents, reference);
    }
    for (channel, &index) in indices.iter().enumerate() {
        let reference = if mono_views {
            let template = project.doc.resolve(
                *audio
                    .get(source_clips[channel])
                    .ok_or_else(|| invalid("source template selector outside master"))?,
            )?;
            let source = project
                .doc
                .tree
                .find1(template, "Clip/Source")
                .ok_or_else(|| invalid("mono template source missing"))?;
            let source = project
                .doc
                .tree
                .get(source, "ObjectRef")
                .ok_or_else(|| invalid("mono template source identity missing"))?
                .to_string();
            if channel == 0 {
                let dst = project
                    .doc
                    .tree
                    .find1(data.clip, "Clip/Source")
                    .ok_or_else(|| invalid("placed source missing"))?;
                project.doc.tree.set(dst, "ObjectRef", source.clone());
            }
            let e = m::new_secondary_content(&mut project.doc.tree, &source, index as i64);
            let id = project.register(e)?;
            project
                .doc
                .tree
                .new_element_with("SecondaryContentItem", &[("ObjectRef", &id)])
        } else {
            *refs
                .get(index)
                .ok_or_else(|| invalid("physical channel outside source template"))?
        };
        project
            .doc
            .tree
            .set(reference, "Index", channel.to_string());
        project.doc.tree.append(contents, reference);
    }
    let layout = project
        .doc
        .tree
        .child_by_tag(data.clip, "AudioChannelLayout")
        .ok_or_else(|| invalid("placed audio lacks layout"))?;
    project.doc.tree.set_text(
        layout,
        Some(if indices.len() == 8 {
            adaptive_layout()
        } else {
            m::audio_channel_layout(indices.len() as u32)
        }),
    );
    if indices.len() == 8 {
        for channel in 8..32 {
            let node = project.doc.tree.new_element_with(
                "SecondaryContent",
                &[
                    ("ClassID", "f9d004b5-cb04-4e2f-af6f-64fadc2c4be9"),
                    ("Version", "1"),
                ],
            );
            crate::xml::mutations::build_leaf(
                &mut project.doc.tree,
                node,
                "ChannelIndex",
                "18446744073709551615",
            );
            let id = project.register(node)?;
            project.doc.tree.sub_element(
                contents,
                "SecondaryContentItem",
                &[("Index", &channel.to_string()), ("ObjectRef", &id)],
            );
        }
    }
    if group > 0 {
        if let Some(n) = project.doc.tree.child_by_tag(data.subclip, "OrigChGrp") {
            project.doc.tree.set_text(n, Some(group.to_string()));
        }
    }
    let channels = project.item_extra[&item].channel_groups.as_ref().unwrap()[group].len();
    if channels > 1 {
        super::combined::declare_stereo_chain(project, placed)?;
        if channels > 2 {
            let chain_ref = project
                .doc
                .tree
                .find1(data.element, "ClipTrackItem/ComponentOwner/Components")
                .ok_or_else(|| invalid("placed group lacks component chain"))?;
            let chain = project.doc.resolve(chain_ref)?;
            let typ = if channels == 6 { "2" } else { "3" };
            for (tag, value) in [
                ("ChannelType", typ.to_string()),
                (
                    "AudioChannelLayout",
                    if channels == 8 {
                        adaptive_layout()
                    } else {
                        m::audio_channel_layout(channels as u32)
                    },
                ),
            ] {
                let n = project
                    .doc
                    .tree
                    .child_by_tag(chain, tag)
                    .ok_or_else(|| invalid("placed group lacks channel declaration"))?;
                project.doc.tree.set_text(n, Some(value));
            }
        }
    }
    // A later edit may replace a nonzero master gain with zero; remove the inherited
    // value so the writer always applies the exact effective gain once.
    if let Some(n) = project.doc.tree.child_by_tag(data.clip, "Gain") {
        project.doc.tree.remove(data.clip, n);
    }
    Ok(placed)
}
