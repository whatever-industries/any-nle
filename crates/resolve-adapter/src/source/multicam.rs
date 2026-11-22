//! Resolve reference-audio multicam source interpretation.
mod saved;
pub(super) mod selection;
use super::*;
use graph::source::{
    Multicam, MulticamAlternative, MulticamLane, MulticamPiece, MulticamUse, SourceBinding,
};

const UNKNOWN_PICTURE_DEFAULT: &str =
    "Resolve Group-wide picture default is unqualified; edit selectors are separate";
pub(crate) const PARTIAL_PICTURE: &str =
    "one or more Resolve Group picture angles remain unqualified";
pub(crate) const REFERENCE_AUDIO_SWITCH: (&str, &str) = (
    "multicam_reference_audio_envelope",
    "Resolve's saved angle differs from the reference audio source; the native cut-edge envelope has no qualified destination mapping",
);

pub(crate) fn native_group_default_unavailable(lane: &MulticamLane) -> bool {
    lane.selected.is_none() && lane.reason.as_deref() == Some(UNKNOWN_PICTURE_DEFAULT)
}

#[derive(Clone, PartialEq, Eq)]
struct Member {
    item: String,
    start: i64,
    length: i64,
    source_in: i64,
}

struct AngleTrack {
    id: String,
    angle: String,
    members: Vec<Member>,
}

pub(super) struct Group {
    pub(super) grid: Grid,
    single_audio_slot: bool,
    picture: Vec<(String, String, Member)>,
    extended_picture: Vec<AngleTrack>,
    mono_angles: Vec<AngleTrack>,
    audio: Option<(String, String, Member)>,
    saved_tracks: saved::Tracks,
}

pub(super) fn definition<'a>(context: &mut Context<'a>, clip: &'a Node) -> Result<Group> {
    let id = clip.id()?;
    let node = clip.one("Sequence")?.entry()?;
    expect(node, "Sm2Sequence")?;
    same(node.get("Parent")?, id, "Group sequence owner")?;
    let (grid, fractional_duration) = Grid::read_with_fractional_duration(node)?;
    let fractional_extent = fractional_duration.is_some();
    let container = context.container(node.id()?, grid.length.max(i64::from(fractional_extent)))?;
    let mut picture = Vec::new();
    let mut audio_tracks = BTreeMap::new();
    let mut picture_complete = true;
    let mut audio_complete = true;
    let mut saved_tracks = saved::Tracks::default();
    let mut extended_picture = Vec::new();
    let mut complete_extended_picture = true;
    let mut mono_angles = Vec::new();
    let mut complete_mono = true;
    if let Some(container) = container {
        for (kind, track) in tracks(&container)? {
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
            saved_tracks.capture(kind, angle, track, &entries, context)?;
            let mut member = None;
            let mut members = Vec::new();
            let neutral_track = track.get("Flags")? == "0";
            for item in &entries {
                let item_id = item.id()?;
                claim(&mut context.ids, item_id)?;
                if matches!(item.name.as_str(), "Sm2TiVideoClip" | "Sm2TiAudioClip" | "Sm2TiTransition") {
                    timing::observe_edit(&mut context.graph, id, item, grid.rate)?;
                }
                if !ordinary_item(item, kind, id, &mut context.graph)? {
                    continue;
                }
                effects::report_group_controls(item, id, &mut context.graph)?;
                let target = item.get("MediaRef")?;
                let Some(master) = context.masters.get(target) else {
                    if !context.pool_items.contains(target) {
                        return Err("unresolved Resolve Group member reference".into());
                    }
                    continue;
                };
                let Some(edit) = grid.edit(item)? else {
                    omitted(&mut context.graph, id, item_id, "timing", timing::SUBFRAME_LOSS);
                    continue;
                };
                if !leaf_matches(master, item, grid, edit, id, &mut context.graph, &mut context.blobs)? {
                    continue;
                }
                let direct_mono = kind == StreamKind::Audio && neutral_track
                    && master.wav_samples.is_some() && !master.stereo_pool_route
                    && master.audio.len() == 1 && master.audio.get(&0) == Some(&0)
                    && track.get("SubType")? == "1"
                    && item.maybe("MediaTrackIdx")? == Some("0")
                    && item.maybe("VirtualAudioTrackBA")?.is_some_and(|saved| !saved.is_empty())
                    && mono(item, &mut context.blobs, false, false)? == Some(0);
                if neutral_track && (kind == StreamKind::Video || direct_mono) {
                    members.push(Member {
                        item: target.into(), start: edit.start, length: edit.length, source_in: edit.source_in,
                    });
                }
                if entries.len() == 1
                    && neutral_track
                    && (kind == StreamKind::Video
                        || group_audio_member(item, master, track, &mut context.blobs)?)
                {
                    member = Some(Member {
                        item: target.into(),
                        start: edit.start,
                        length: edit.length,
                        source_in: edit.source_in,
                    });
                }
            }
            let (angles, complete) = match kind {
                StreamKind::Video => (&mut extended_picture, &mut complete_extended_picture),
                StreamKind::Audio => (&mut mono_angles, &mut complete_mono),
            };
            members.sort_by_key(|member| member.start);
            if !angle.is_empty()
                && !members.is_empty()
                && members.len() == entries.len()
                && members.windows(2).all(|pair| pair[0].start + pair[0].length <= pair[1].start)
                && !angles.iter().any(|track: &AngleTrack| track.angle == angle)
            {
                angles.push(AngleTrack { id: track_id.into(), angle: angle.into(), members });
            } else {
                *complete = false;
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
                    picture.push((track_id.to_owned(), angle.to_owned(), member));
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
    if fractional_extent && !complete_extended_picture {
        extended_picture.retain(|track| saved_tracks.unique_picture(&track.angle));
    }
    let partial_picture = fractional_extent && !complete_extended_picture && !extended_picture.is_empty();
    if !picture.is_empty() || !fractional_extent && !complete_extended_picture {
        extended_picture.clear();
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
                audio = Some((only.0.clone(), only.1.clone(), first.clone()));
            }
        }
    }

    let single_audio_slot = if audio.is_none()
        && clip.maybe("AudioSource")? == Some("AUDIO_SOURCE_EMBEDDED")
    {
        read_single_audio_slot(clip, &mut context.blobs)?
    } else {
        None
    };
    let input_scaling = match (context.input_scaling_default, node.maybe("FieldsBlob")?) {
        (Some(mode), Some(saved)) if !context.inspect_drt
            && context.blobs.qt(saved).ok().is_some_and(|map| !map.keys().any(|key| key == "SequenceSetup")) => Some(mode),
        _ => None,
    };
    let mono_transport = saved_tracks.publish(id, clip, grid, input_scaling, &mut context.graph, &mut context.blobs)?;
    if audio.is_some() || !complete_mono || !mono_transport {
        mono_angles.clear();
    }
    let mut lanes = Vec::new();
    if !picture.is_empty() || !extended_picture.is_empty() {
        let alternatives = if picture.is_empty() {
            extended_picture.iter().map(|track| track_alternative(id, track, StreamKind::Video, grid, fractional_duration)).collect::<Result<_>>()?
        } else {
            picture.iter().map(|(track, _, member)| {
                alternative(id, track, StreamKind::Video, 1, member, grid, fractional_duration)
            }).collect::<Result<_>>()?
        };
        lanes.push(MulticamLane {
            kind: StreamKind::Video,
            index: 1,
            selected: None,
            alternatives,
            reason: Some(UNKNOWN_PICTURE_DEFAULT.into()),
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
                    fractional_duration,
                )?],
                reason: None,
            });
        }
    } else if !mono_angles.is_empty() {
        lanes.push(MulticamLane {
            kind: StreamKind::Audio,
            index: 1,
            selected: None,
            alternatives: mono_angles.iter()
                .map(|track| track_alternative(id, track, StreamKind::Audio, grid, fractional_duration))
                .collect::<Result<_>>()?,
            reason: Some("Resolve Group-wide audio default is unqualified; edit selectors are separate".into()),
        });
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
        duration: match fractional_duration {
            Some(duration) => Some(duration),
            None if grid.length > 0 => Some(seconds(grid.length, grid.rate)?),
            None => None,
        },
        reason: if lanes.is_empty() {
            Some("unqualified Resolve Group topology".into())
        } else if partial_picture {
            Some(PARTIAL_PICTURE.into())
        } else {
            None
        },
        lanes,
    });
    let mut facts = vec![
            ("kind", text("multicam")),
            (
                "video_angles",
            Value::Arr(if picture.is_empty() {
                extended_picture.iter().map(|track| text(&track.angle)).collect()
            } else {
                picture.iter().map(|(_, angle, _)| text(angle)).collect()
            }),
            ),
            (
                "reference_audio_angle",
                audio
                    .as_ref()
                    .map_or(Value::Null, |(_, angle, _)| text(angle)),
            ),
        ];
    if fractional_extent {
        facts.push(("saved_media_extents".into(), text(node.get("MediaExtents")?)));
    }
    if let Some(saved) = single_audio_slot {
        facts.push(("single_audio_slot".into(), Value::Obj(vec![
            ("track_index".into(), Value::Int(0)),
            ("default_virtual_channel".into(), Value::Int(1)),
            ("transport_channels".into(), Value::Arr(vec![Value::Int(1), Value::Int(2)])),
            ("saved_virtual_audio_tracks".into(), text(saved)),
        ])));
    }
    receipt(&mut context.graph, id, facts);
    Ok(Group {
        grid,
        single_audio_slot: single_audio_slot.is_some(),
        picture,
        extended_picture,
        mono_angles,
        audio,
        saved_tracks,
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
    group_stereo_transport(clip, blobs)
}

fn read_single_audio_slot<'a>(clip: &'a Node, blobs: &mut Blobs) -> Result<Option<&'a str>> {
    let Some(saved) = clip.maybe("VirtualAudioTracksBA")?
        .filter(|saved| !saved.is_empty() && saved.len() <= 512)
    else {
        return Ok(None);
    };
    let Ok(bytes) = hex_bytes(saved) else { return Ok(None) };
    let Ok(outer) = qt(&bytes) else { return Ok(None) };
    if !outer.keys().eq(["0"]) {
        return Ok(None);
    }
    let route = qt(outer.bytes("0")?)?;
    if route.int("AudioType")? != 1
        || route.bytes("ChannelsBA")? != [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 1]
    {
        return Ok(None);
    }
    blobs.charge(bytes.len())?;
    Ok(group_stereo_transport(clip, blobs)?.then_some(saved))
}

fn single_slot_channel(node: &Node, blobs: &mut Blobs) -> Result<Option<i64>> {
    if node.get("MediaTrackIdx")?.parse::<i64>()? != 0 {
        return Err("Resolve Group edit selects an absent audio slot".into());
    }
    Ok(mono(node, blobs, true, true)?.map(|channel| channel + 1))
}

fn group_stereo_transport(clip: &Node, blobs: &mut Blobs) -> Result<bool> {
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
    duration: Option<Rational>,
) -> Result<MulticamAlternative> {
    alternative_pieces(id, track, kind, lane, std::slice::from_ref(member), grid, duration)
}

fn track_alternative(id: &str, track: &AngleTrack, kind: StreamKind, grid: Grid, duration: Option<Rational>) -> Result<MulticamAlternative> {
    alternative_pieces(id, &track.id, kind, 1, &track.members, grid, duration)
}

fn alternative_pieces(
    id: &str,
    track: &str,
    kind: StreamKind,
    lane: i64,
    members: &[Member],
    grid: Grid,
    duration: Option<Rational>,
) -> Result<MulticamAlternative> {
    let mut pieces = Vec::new();
    let mut append = |start, length, source: Option<&Member>| -> Result<()> {
        if length > 0 {
            pieces.push(MulticamPiece {
                start: seconds(start, grid.rate)?,
                duration: seconds(length, grid.rate)?,
                item: source.map(|member| member.item.clone()),
                stream: source.map(|_| lane),
                source_in: source
                    .map(|member| seconds(member.source_in, grid.rate))
                    .transpose()?,
            });
        }
        Ok(())
    };
    let mut end = 0;
    for member in members {
        append(end, member.start - end, None)?;
        append(member.start, member.length, Some(member))?;
        end = member.start + member.length;
    }
    append(end, grid.length - end, None)?;
    if let Some(duration) = duration {
        let start = seconds(grid.length, grid.rate)?;
        let duration = duration.checked_sub(start).ok_or("Resolve Group fractional tail overflows")?;
        pieces.push(MulticamPiece { start, duration, item: None, stream: None, source_in: None });
    }
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
    let single_slot = kind == StreamKind::Audio && group.single_audio_slot;
    let virtual_channel = if single_slot {
        single_slot_channel(node, blobs)?
    } else {
        None
    };
    // The virtual channel identifies the Group transport. Binding a physical
    // reference-audio lane still requires the qualified split profile.
    let channel = if kind == StreamKind::Audio && !single_slot {
        mono(node, blobs, true, false)?
    } else {
        None
    };
    let compatible_grid = group.grid.length > 0 && group.grid.rate == grid.rate
        && rate(&hex_bytes(node.get("MediaFrameRate")?)?)? == grid.rate
        && identity_timemap(node, group.grid.length, group.grid.rate)?;
    let selected = match kind {
        StreamKind::Video => group
            .picture
            .iter()
            .enumerate()
            .find(|(_, (_, name, _))| angle == Some(name.as_str()))
            .map(|(index, (_, _, member))| (1, index as i64, std::slice::from_ref(member)))
            .or_else(|| group.extended_picture.iter().enumerate()
                .find(|(_, track)| angle == Some(track.angle.as_str()))
                .map(|(index, track)| (1, index as i64, track.members.as_slice()))),
        StreamKind::Audio => group
            .audio
            .as_ref()
            // Definition only admits reference audio once both saved angle
            // tracks resolve to exactly the same member and range.
            .filter(|_| group.picture.iter().any(|(_, name, _)| angle == Some(name.as_str())))
            .zip(channel)
            .map(|((_, _, member), channel)| (channel + 1, 0, std::slice::from_ref(member)))
            .or_else(|| {
                if channel != Some(0) {
                    return None;
                }
                group.mono_angles.iter().enumerate()
                    .find(|(_, track)| angle == Some(track.angle.as_str()))
                    .map(|(index, track)| (1, index as i64, track.members.as_slice()))
            }),
    };
    let mut binding = SourceBinding {
        placements: vec![],
        reason: None,
    };
    let mut qualified = None;
    if compatible_grid {
        if edit.source_in + edit.length > group.grid.length
            || !timing::clock_matches(
                node.get("MediaStartTime")?, seconds(group.grid.origin, group.grid.rate)?, group.grid.rate,
            )?
        {
            return Err("Resolve Group edit contradicts owner clock or extent".into());
        }
        if let Some((lane, index, members)) = selected {
            qualified = Some((lane, index));
            let mut covered = 0;
            let source_end = edit.source_in + edit.length;
            let first = members.partition_point(|member| member.start + member.length <= edit.source_in);
            for member in members[first..].iter().take_while(|member| member.start < source_end) {
                let start = edit.source_in.max(member.start);
                let end = source_end.min(member.start + member.length);
                let leaf = Edit {
                    start: edit.start + start - edit.source_in,
                    length: end - start,
                    source_in: member.source_in + start - member.start,
                };
                let leaf_id = if binding.placements.is_empty() { id.into() }
                    else { format!("{id}/leaf/{}", binding.placements.len() + 1) };
                out.placements.push(super::placement(
                    &leaf_id,
                    &member.item,
                    grid,
                    leaf,
                    kind,
                    track,
                    (kind == StreamKind::Audio).then_some(lane),
                )?);
                binding.placements.push(leaf_id);
                covered += leaf.length;
            }
            if covered != edit.length {
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
    if kind == StreamKind::Audio
        && qualified.is_some()
        && group.audio.as_ref().is_some_and(|(_, reference, _)| angle != Some(reference.as_str()))
    {
        loss(graph, &use_id, REFERENCE_AUDIO_SWITCH.0, REFERENCE_AUDIO_SWITCH.1);
    }
    let mut facts = vec![
            ("angle_id", angle.map_or(Value::Null, text)),
            (
                "saved_audio_selector",
                channel.map_or(Value::Null, Value::Int),
            ),
        ];
    if single_slot {
        facts.push(("saved_media_track_index", Value::Int(0)));
        facts.push(("saved_virtual_channel", virtual_channel.map_or(Value::Null, Value::Int)));
        let encoded = node.get("VirtualAudioTrackBA")?;
        if encoded.len() <= 512 {
            facts.push(("saved_virtual_audio_track", text(encoded)));
        }
    }
    group.saved_tracks.bind_use(kind, angle, node, &mut facts, blobs)?;
    receipt(graph, &use_id, facts);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{qt as encode_qt, Qt};

    #[test]
    fn multipart_picture_angle_keeps_both_trims_and_the_gap() {
        let grid = Grid {
            rate: Rational::integer(24),
            drop: false,
            origin: 86_400,
            length: 30,
            size: (3840, 1920),
        };
        let track = AngleTrack {
            id: "picture-track".into(),
            angle: "Angle".into(),
            members: vec![
                Member { item: "first".into(), start: 0, length: 12, source_in: 5 },
                Member { item: "second".into(), start: 20, length: 10, source_in: 8 },
            ],
        };
        let picture = track_alternative("group", &track, StreamKind::Video, grid, None).unwrap();
        let [first, gap, second] = picture.pieces.as_slice() else {
            panic!("expected two source pieces and the intervening gap");
        };
        assert_eq!(first.item.as_deref(), Some("first"));
        assert_eq!(first.source_in, Some(Rational::new(5, 24).unwrap()));
        assert_eq!(gap.item, None);
        assert_eq!(gap.start, Rational::new(12, 24).unwrap());
        assert_eq!(gap.duration, Rational::new(8, 24).unwrap());
        assert_eq!(second.item.as_deref(), Some("second"));
        assert_eq!(second.start, Rational::new(20, 24).unwrap());
        assert_eq!(second.source_in, Some(Rational::new(8, 24).unwrap()));
    }

    #[test]
    fn single_group_audio_slot_preserves_each_virtual_channel() {
        let edit = |slot, channel| {
            let route = encode_qt(&[
                ("AudioType", Qt::Int(1)),
                ("ChannelsBA", Qt::Bytes(&[0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, channel])),
            ]);
            xml::parse(&format!(
                "<Edit><MediaTrackIdx>{slot}</MediaTrackIdx><VirtualAudioTrackBA>{}</VirtualAudioTrackBA></Edit>",
                hex::encode(route),
            ), &mut 0).unwrap()
        };
        let mut blobs = Blobs::default();
        assert_eq!(single_slot_channel(&edit(0, 1), &mut blobs).unwrap(), Some(1));
        assert_eq!(single_slot_channel(&edit(0, 2), &mut blobs).unwrap(), Some(2));
        assert_eq!(single_slot_channel(&edit(0, 3), &mut blobs).unwrap(), None);
        assert!(single_slot_channel(&edit(1, 2), &mut blobs).is_err());
        assert!(mono(&edit(0, 2), &mut blobs, true, false).is_err());
    }
}
