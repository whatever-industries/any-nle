//! Resolve reference-audio multicam source interpretation.
pub(super) mod selection;
use super::*;
use graph::source::{
    Multicam, MulticamAlternative, MulticamLane, MulticamPiece, MulticamUse, SourceBinding,
};

#[derive(Clone, PartialEq, Eq)]
struct Member<'a> {
    item: &'a str,
    start: i64,
    length: i64,
    source_in: i64,
}

pub(super) struct Group<'a> {
    grid: Grid,
    picture: Vec<(&'a str, String, Member<'a>)>,
    audio: Option<(&'a str, String, Member<'a>)>,
}

pub(super) fn definition<'a>(context: &mut Context<'a>, clip: &'a Node) -> Result<Group<'a>> {
    let id = clip.id()?;
    let node = clip.one("Sequence")?.entry()?;
    expect(node, "Sm2Sequence")?;
    same(node.get("Parent")?, id, "Group sequence owner")?;
    let grid = Grid::read(node)?;
    let container = context.container(node.id()?, grid.length)?;
    let mut picture = Vec::new();
    let mut audio_tracks = BTreeMap::new();
    let mut picture_complete = true;
    let mut audio_complete = true;
    if let Some(container) = container {
        for (kind, track) in tracks(container)? {
            let track_id = track.id()?;
            claim(&mut context.ids, track_id)?;
            let fields = context.blobs.qt(track.get("FieldsBlob")?)?;
            let angle = fields
                .text("AngleId")?
                .ok_or("missing Resolve Group angle identity")?;
            let entries = track
                .one("Items")?
                .elements()?
                .map(Node::entry)
                .collect::<Result<Vec<_>>>()?;
            let mut member = None;
            for item in &entries {
                let item_id = item.id()?;
                claim(&mut context.ids, item_id)?;
                if !ordinary_item(item, kind, id, &mut context.graph)? {
                    continue;
                }
                effects::report_group_controls(item, id, &mut context.graph)?;
                let edit = grid.edit(item, false)?;
                let target = item.get("MediaRef")?;
                let Some(master) = context.masters.get(target) else {
                    if !context.pool_items.contains(target) {
                        return Err("unresolved Resolve Group member reference".into());
                    }
                    continue;
                };
                if !leaf_matches(master, item, grid, edit, id, &mut context.graph, &mut context.blobs)? {
                    continue;
                }
                if entries.len() == 1
                    && track.get("Flags")? == "0"
                    && (kind == StreamKind::Video
                        || group_audio_member(item, master, track, &mut context.blobs)?)
                {
                    member = Some(Member {
                        item: target,
                        start: edit.start,
                        length: edit.length,
                        source_in: edit.source_in,
                    });
                }
            }
            let Some(member) = member else {
                match kind {
                    StreamKind::Video => picture_complete = false,
                    StreamKind::Audio => audio_complete = false,
                }
                continue;
            };
            match kind {
                StreamKind::Video => {
                    if picture.iter().any(|(_, name, _)| name == angle) {
                        return Err("duplicate Resolve Group video angle identity".into());
                    }
                    picture.push((track_id, angle.to_owned(), member));
                }
                StreamKind::Audio => {
                    if audio_tracks.insert(angle.to_owned(), member).is_some() {
                        return Err("duplicate Resolve Group audio angle identity".into());
                    }
                }
            }
        }
    }
    if !picture_complete || picture.len() != 2 {
        picture.clear();
    }
    let mut audio = None;
    if audio_complete
        && picture.len() == 2
        && audio_tracks.len() == 2
        && picture
            .iter()
            .all(|(_, angle, _)| audio_tracks.contains_key(angle))
        && clip.get("AudioSource")? == "AUDIO_SOURCE_EMBEDDED"
        && group_audio_routes(clip, &mut context.blobs)?
    {
        let mut tracks = audio_tracks.values();
        let first = tracks.next().ok_or("missing Resolve reference audio")?;
        if tracks.all(|member| member == first) {
            let matches: Vec<_> = picture
                .iter()
                .filter(|(_, _, member)| member.item == first.item)
                .collect();
            if let [only] = matches.as_slice() {
                audio = Some((only.0, only.1.clone(), first.clone()));
            }
        }
    }

    let mut lanes = Vec::new();
    if !picture.is_empty() {
        lanes.push(MulticamLane {
            kind: StreamKind::Video,
            index: 1,
            selected: None,
            alternatives: picture
                .iter()
                .map(|(track, _, member)| {
                    alternative(id, track, StreamKind::Video, 1, member, grid)
                })
                .collect::<Result<_>>()?,
            reason: Some(
                "Resolve Group-wide picture default is unqualified; edit selectors are separate"
                    .into(),
            ),
        });
    } else {
        loss(
            &mut context.graph,
            id,
            "multicam_picture",
            "unqualified Resolve Group picture topology",
        );
    }
    if let Some((track, _, member)) = &audio {
        for lane in [1, 2] {
            lanes.push(MulticamLane {
                kind: StreamKind::Audio,
                index: lane,
                selected: Some(0),
                alternatives: vec![alternative(
                    id,
                    track,
                    StreamKind::Audio,
                    lane,
                    member,
                    grid,
                )?],
                reason: None,
            });
        }
    } else {
        loss(
            &mut context.graph,
            id,
            "multicam_audio",
            "unqualified Resolve Group reference-audio topology",
        );
    }
    context.graph.multicams.push(Multicam {
        id: id.into(),
        name: clip.get("Name")?.into(),
        rate: Some(grid.rate),
        duration: (grid.length > 0)
            .then(|| seconds(grid.length, grid.rate))
            .transpose()?,
        reason: lanes
            .is_empty()
            .then(|| "unqualified Resolve Group topology".into()),
        lanes,
    });
    receipt(
        &mut context.graph,
        id,
        vec![
            ("kind", text("multicam")),
            (
                "video_angles",
                Value::Arr(picture.iter().map(|(_, angle, _)| text(angle)).collect()),
            ),
            (
                "reference_audio_angle",
                audio
                    .as_ref()
                    .map_or(Value::Null, |(_, angle, _)| text(angle)),
            ),
        ],
    );
    Ok(Group {
        grid,
        picture,
        audio,
    })
}

fn group_audio_member(
    item: &Node,
    master: &Master,
    track: &Node,
    blobs: &mut Blobs,
) -> Result<bool> {
    let fields = blobs.qt(item.get("VirtualAudioTrackBA")?)?;
    Ok(track.get("SubType")? == "258"
        && item.get("MediaTrackIdx")? == "0"
        && fields.int("AudioType")? == 0
        && fields.bytes("ChannelsBA")? == [0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0x40, 1, 0, 0, 0x80, 1]
        && master.audio.get(&0) == Some(&0)
        && master.audio.get(&1) == Some(&i64::from(!master.split_atom_audio)))
}

fn group_audio_routes(clip: &Node, blobs: &mut Blobs) -> Result<bool> {
    let outer = blobs.qt(clip.get("VirtualAudioTracksBA")?)?;
    if !outer.keys().eq(["0", "1"]) {
        return Ok(false);
    }
    for (key, channel) in [("0", 1), ("1", 2)] {
        let route = qt(outer.bytes(key)?)?;
        if route.int("AudioType")? != 1
            || route.bytes("ChannelsBA")? != [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, channel]
        {
            return Ok(false);
        }
    }
    let data = blobs.envelope(clip.get("FieldsBlob")?)?;
    let nested = pb_bytes(&data, 1)?.ok_or("missing Resolve Group metadata")?;
    let nested = pb_bytes(nested, 1)?.ok_or("missing Resolve Group routing")?;
    let tracks = qt(pb_bytes(nested, 5)?.ok_or("missing Resolve Group virtual routes")?)?;
    if !tracks.keys().eq(["0"]) {
        return Ok(false);
    }
    let track = qt(tracks.bytes("0")?)?;
    if track.int("Type")? != 258 {
        return Ok(false);
    }
    let channels = qt(track.bytes("ChannelVecBA")?)?;
    if !channels.keys().eq(["0", "1"]) {
        return Ok(false);
    }
    for key in ["0", "1"] {
        let route = qt(channels.bytes(key)?)?;
        same(
            route
                .text("MediaRef")?
                .ok_or("missing Resolve Group transport reference")?,
            clip.id()?,
            "Group transport reference",
        )?;
        let index = if route.keys().any(|key| key == "ChannelIdx") {
            route.int("ChannelIdx")?
        } else {
            0
        };
        if index.to_string() != key {
            return Ok(false);
        }
    }
    Ok(true)
}

fn alternative(
    id: &str,
    track: &str,
    kind: StreamKind,
    lane: i64,
    member: &Member,
    grid: Grid,
) -> Result<MulticamAlternative> {
    let mut pieces = Vec::new();
    let mut append = |start, length, source: Option<&Member>| -> Result<()> {
        if length > 0 {
            pieces.push(MulticamPiece {
                start: seconds(start, grid.rate)?,
                duration: seconds(length, grid.rate)?,
                item: source.map(|member| member.item.into()),
                stream: source.map(|_| lane),
                source_in: source
                    .map(|member| seconds(member.source_in, grid.rate))
                    .transpose()?,
            });
        }
        Ok(())
    };
    append(0, member.start, None)?;
    append(member.start, member.length, Some(member))?;
    let end = member.start + member.length;
    append(end, grid.length - end, None)?;
    // Track identifiers are native. The lane suffix distinguishes the two audio routes.
    Ok(MulticamAlternative {
        id: format!("{id}/{track}/{}/{lane}", kind.as_str()),
        pieces,
        reason: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn placement(
    group: &Group,
    node: &Node,
    grid: Grid,
    edit: Edit,
    kind: StreamKind,
    track: i64,
    out: &mut SourceSequence,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<()> {
    let group_id = node.get("MediaRef")?;
    let id = node.id()?;
    effects::report_group_controls(node, &out.id, graph)?;
    let data = blobs.envelope(node.get("FieldsBlob")?)?;
    let meta = pb_bytes(&data, 1)?.ok_or("missing Resolve Group edit metadata")?;
    let angle = pb_bytes(meta, 3)?.map(std::str::from_utf8).transpose()?;
    let channel = if kind == StreamKind::Audio {
        mono(node, blobs, true)?
    } else {
        None
    };
    let compatible_grid = group.grid.rate == grid.rate
        && rate(&hex_bytes(node.get("MediaFrameRate")?)?)? == grid.rate
        && identity_timemap(node, group.grid.length, group.grid.rate)?;
    let selected = match kind {
        StreamKind::Video => group
            .picture
            .iter()
            .enumerate()
            .find(|(_, (_, name, _))| angle == Some(name.as_str()))
            .map(|(index, (_, _, member))| (1, index as i64, member)),
        StreamKind::Audio => group
            .audio
            .as_ref()
            .filter(|(_, name, _)| angle == Some(name.as_str()))
            .zip(channel)
            .map(|((_, _, member), channel)| (channel + 1, 0, member)),
    };
    let mut binding = SourceBinding {
        placements: vec![],
        reason: None,
    };
    let mut qualified = None;
    if compatible_grid {
        if edit.source_in + edit.length > group.grid.length
            || frames(node.get("MediaStartTime")?.parse()?, grid.rate)? != group.grid.origin
        {
            return Err("Resolve Group edit contradicts owner clock or extent".into());
        }
        if let Some((lane, index, member)) = selected {
            qualified = Some((lane, index));
            let start = edit.source_in.max(member.start);
            let end = (edit.source_in + edit.length).min(member.start + member.length);
            if end > start {
                let leaf = Edit {
                    start: edit.start + start - edit.source_in,
                    length: end - start,
                    source_in: member.source_in + start - member.start,
                };
                out.placements.push(super::placement(
                    id,
                    member.item,
                    grid,
                    leaf,
                    kind,
                    track,
                    (kind == StreamKind::Audio).then_some(lane),
                )?);
                binding.placements.push(id.into());
            }
            if start != edit.source_in || end != edit.source_in + edit.length {
                binding.reason = Some(selection::OBSERVED_GAP.into());
            }
        }
    }
    let reason = qualified
        .is_none()
        .then(|| "unqualified Resolve Group edit clock, angle or audio selector".to_owned());
    if let Some(reason) = &reason {
        binding.reason = Some(reason.clone());
    }
    let use_id = format!("{id}/use");
    if let Some(reason) = &binding.reason {
        loss(graph, &use_id, "multicam_binding", reason);
    }
    receipt(
        graph,
        &use_id,
        vec![
            ("angle_id", angle.map_or(Value::Null, text)),
            (
                "saved_audio_selector",
                channel.map_or(Value::Null, Value::Int),
            ),
        ],
    );
    graph.multicam_uses.push(MulticamUse {
        id: use_id,
        sequence: out.id.clone(),
        candidates: vec![group_id.into()],
        kind,
        track,
        lane: qualified.map(|(lane, _)| lane),
        selected: qualified.map(|(_, index)| index),
        record_start: Some(seconds(edit.start, grid.rate)?),
        duration: Some(seconds(edit.length, grid.rate)?),
        source_in: Some(seconds(edit.source_in, grid.rate)?),
        binding,
        reason,
    });
    Ok(())
}
