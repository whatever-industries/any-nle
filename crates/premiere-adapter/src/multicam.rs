//! Source Group interpretation for the bounded typed Premiere writer.
use super::*;
use graph::source::{MulticamAlternative, MulticamPiece};
use prproj::convert::multicam::{
    PreparedCamera, PreparedGroupAudio, PreparedMulticam, PreparedMulticamUse, MAX_GROUPS, RATE,
};

fn refusal(id: &str, reason: &str) -> GraphError {
    error(format!("unsupported_editable_multicam {id}: {reason}"))
}

fn real_piece<'a>(alt: &'a MulticamAlternative, group: &str) -> R<&'a MulticamPiece> {
    let mut media = alt.pieces.iter().filter(|p| p.item.is_some());
    if alt.reason.is_some() {
        return Err(refusal(group, "camera or audio alternative is unresolved"));
    }
    let Some(p) = media.next() else {
        return Err(refusal(group, "alternative has no media piece"));
    };
    if media.next().is_some() {
        return Err(refusal(
            group,
            "discontinuous alternatives require separate qualification",
        ));
    }
    Ok(p)
}

pub(super) fn prepare(
    g: &SourceGraph,
    input: &mut PreparedProjectInput,
    ownership: &HashMap<String, (usize, usize)>,
    clock: ClockProfile,
    diagnostics: &mut Vec<String>,
    losses: &mut Vec<Loss>,
) -> R<()> {
    if g.multicams.is_empty() && g.multicam_uses.is_empty() {
        return Ok(());
    }
    if g.multicams.len() > MAX_GROUPS || g.multicam_uses.len() > model::MAX_ITEMS {
        return Err(refusal("project", "prepared group/use count exceeds limit"));
    }
    let direct = clock == ClockProfile::LegacyImport;
    if direct && input.options.bin_mode != "separate" {
        return Err(error("multicam_native_route_required: linked editable Groups require Separate files"));
    }
    let pcm24_direct: HashSet<_> = input.clips.iter().filter_map(|clip| match clip {
        PreparedClip::Legacy(c) if input.direct_dnxhr_origins.contains_key(&c.id)
            && c.audio.iter().all(|a| a.bits == 24) => Some(c.id.as_str()),
        _ => None,
    }).collect();
    let mut indices = HashMap::new();
    let mut bin_indices = HashMap::new();
    let known: HashSet<_> = g.multicams.iter().map(|m| m.id.as_str()).collect();
    for (i, b) in g.bins.iter().enumerate() {
        for id in &b.items {
            if known.contains(id.as_str()) && bin_indices.insert(id.as_str(), i).is_some() {
                return Err(refusal(id, "group belongs to more than one selected bin"));
            }
        }
    }
    let mut audio_lanes = Vec::new();
    let mut picture_order = Vec::new();
    for m in &g.multicams {
        if m.reason.is_some() || m.rate.map(fraction) != Some(RATE) {
            return Err(refusal(
                &m.id,
                "needs one resolved picture lane and resolved mono lanes at 23.976 fps",
            ));
        }
        let mut audio: Vec<_> = m
            .lanes
            .iter()
            .filter(|l| l.kind == StreamKind::Audio)
            .collect();
        if audio.len() > 2 && m.lanes.len() - audio.len() == 1 && (!direct || audio.len() > 3) {
            return Err(refusal(
                &m.id,
                &format!(
                    "saved Group {:?} has {} audio lanes. Separate files supports up to three independent mono lanes in the scoped 23.976 profile. Select bins without this Group and its dependent edits",
                    m.name,
                    audio.len(),
                ),
            ));
        }
        if m.lanes.len() != audio.len() + 1 || !matches!(audio.len(), 1..=3) {
            return Err(refusal(
                &m.id,
                "needs one resolved picture lane and one to three resolved mono lanes at 23.976 fps",
            ));
        }
        audio.sort_by_key(|l| l.index);
        let video = m
            .lanes
            .iter()
            .find(|l| l.kind == StreamKind::Video)
            .ok_or_else(|| refusal(&m.id, "missing picture lane"))?;
        let angles = if audio.len() == 1 && video.selected == Some(1) { [1, 0] } else { [0, 1] };
        if video.selected != Some(angles[0] as i64)
            || std::iter::once(video).chain(audio.iter().copied())
                .any(|l| l.reason.is_some() || l.alternatives.len() != 2)
        {
            return Err(refusal(
                &m.id,
                "two resolved cameras and the first default picture angle are required",
            ));
        }
        let mut cameras = Vec::with_capacity(2);
        let mut audio_alternatives = vec![[0; 2]; audio.len()];
        let mut audio_cameras = vec![None; audio.len()];
        for (i, &angle) in angles.iter().enumerate() {
            let alt = &video.alternatives[angle];
            let picture = real_piece(alt, &m.id)?;
            if picture.stream != Some(1) {
                return Err(refusal(
                    &m.id,
                    "camera picture does not resolve to the leaf's single video stream",
                ));
            }
            let mut labels = Vec::with_capacity(audio.len());
            for (j, lane) in audio.iter().enumerate() {
                let mut matching = None;
                for (index, candidate) in lane.alternatives.iter().enumerate() {
                    let p = real_piece(candidate, &m.id)?;
                    // An audio selection is not a camera index. Prove the identity and
                    // the complete extent against its picture alternative.
                    if p.item == picture.item
                        && p.start == picture.start
                        && p.source_in == picture.source_in
                        && p.duration == picture.duration
                        && candidate.pieces.len() == alt.pieces.len()
                        && candidate.pieces.iter().zip(&alt.pieces).all(|(x, y)| {
                            x.start == y.start && x.duration == y.duration && x.item == y.item
                        })
                    {
                        if matching.replace((index as i64, p.stream)).is_some() {
                            return Err(refusal(&m.id, "audio alternative identity is ambiguous"));
                        }
                    }
                }
                let Some((index, Some(label))) = matching else {
                    return Err(refusal(
                        &m.id,
                        "audio alternatives do not cover the corresponding picture extent",
                    ));
                };
                if lane.selected == Some(index) {
                    audio_cameras[j] = Some(i);
                }
                labels.push(label);
                audio_alternatives[j][i] = index;
            }
            let clip = clip_id(picture.item.as_deref().unwrap());
            if direct && !pcm24_direct.contains(clip.as_str()) {
                let width = if audio.len() == 3 { "three" } else { "two" };
                return Err(refusal(&m.id, &format!(
                    "linked camera {} requires qualified DNx LB and {width} original mono PCM24 siblings",
                    i + 1
                )));
            }
            cameras.push(PreparedCamera {
                clip,
                start: ticks(picture.start, "camera start")?,
                source_in: ticks(
                    picture
                        .source_in
                        .ok_or_else(|| refusal(&m.id, "camera origin unresolved"))?,
                    "camera origin",
                )?,
                duration: ticks(picture.duration, "camera duration")?,
                audio_labels: labels,
            });
        }
        let audio_cameras = audio_cameras.into_iter().collect::<Option<Vec<_>>>()
            .ok_or_else(|| refusal(&m.id, "default audio lanes need exact saved camera selections"))?;
        if !direct && audio_cameras.iter().any(|&camera| camera != 0) && audio.len() != 1 {
            return Err(refusal(&m.id, "both default audio lanes must resolve to the first camera"));
        }
        let bin = *bin_indices
            .get(m.id.as_str())
            .ok_or_else(|| refusal(&m.id, "group has no owning bin"))?;
        let duration = ticks(
            m.duration
                .ok_or_else(|| refusal(&m.id, "group extent unresolved"))?,
            "group duration",
        )?;
        indices.insert(m.id.as_str(), input.multicams.len());
        audio_lanes.push(audio.iter().map(|lane| lane.index).zip(audio_alternatives).collect::<Vec<_>>());
        picture_order.push(angles);
        if angles == [1, 0] {
            loss(losses, &m.id, "group_angle_order",
                "The saved default picture is first in Premiere's angle order. Existing selections follow the original camera identity.");
        }
        input.multicams.push(PreparedMulticam {
            id: m.id.clone(),
            name: m.name.clone(),
            bin,
            duration,
            audio_cameras,
            cameras: cameras
                .try_into()
                .map_err(|_| refusal(&m.id, "wrong camera count"))?,
        });
    }
    let sequences: HashMap<_, _> = g
        .sequences
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let mut by_event = HashMap::new();
    let mut reported_clocks = HashSet::new();
    let warp_uses: HashSet<_> = g.sidecars.iter().filter(|s| s.adapter == "avid.group_use"
        && key(&s.data, "warp_picture_input").and_then(boolean) == Some(true))
        .map(|s| s.object.as_str()).collect();
    for u in &g.multicam_uses {
        let [id] = u.candidates.as_slice() else {
            return Err(refusal(
                &u.id,
                "group use identity is unresolved or ambiguous",
            ));
        };
        let [p] = u.binding.placements.as_slice() else {
            return Err(refusal(
                &u.id,
                "group use needs one exact bound leaf placement",
            ));
        };
        if u.reason.is_some() || u.binding.reason.is_some() {
            return Err(refusal(
                &u.id,
                "group use or leaf binding has an unresolved witness",
            ));
        }
        let &(sequence, event) = ownership
            .get(p.as_str())
            .ok_or_else(|| refusal(&u.id, "bound placement did not survive preparation"))?;
        if sequences.get(u.sequence.as_str()) != Some(&sequence) {
            return Err(refusal(&u.id, "bound event belongs to another sequence"));
        }
        if g.sequences[sequence].start.is_none() && reported_clocks.insert(sequence) {
            loss(losses, &u.sequence, "sequence_clock",
                "qualified source record start is unavailable; existing compatibility display origin retained, with exact relative edit spans");
        }
        let group = *indices
            .get(id.as_str())
            .ok_or_else(|| refusal(&u.id, "group definition missing"))?;
        let e = &input.sequences[sequence].events[event];
        let exact = |v: Option<Rational>, label: &str| -> R<i64> {
            ticks(v.ok_or_else(|| refusal(&u.id, label))?, label)
        };
        let source_in = exact(u.source_in, "group-local origin unresolved")?;
        if exact(u.record_start, "group record start unresolved")? != e.record_start
            || exact(u.duration, "group edit duration unresolved")? != e.duration
        {
            return Err(refusal(
                &u.id,
                "prepared event changes the Group selector's record span",
            ));
        }
        let i = *by_event.entry((sequence, event)).or_insert_with(|| {
            let i = input.multicam_uses.len();
            input.multicam_uses.push(PreparedMulticamUse {
                sequence,
                event,
                group,
                source_in,
                video: None,
                warp_picture_input: false,
                audio: vec![PreparedGroupAudio::Group(usize::MAX); e.audio_tracks.len()],
            });
            i
        });
        let use_ = &mut input.multicam_uses[i];
        if use_.group != group || use_.source_in != source_in {
            return Err(refusal(
                &u.id,
                "prepared event combines different group identities or origins",
            ));
        }
        let selected = u
            .selected
            .ok_or_else(|| refusal(&u.id, "selection unresolved"))?;
        match u.kind {
            StreamKind::Video => {
                let target = picture_order[group].iter().position(|&angle| angle as i64 == selected)
                    .ok_or_else(|| refusal(&u.id, "picture choice does not name a saved angle"))?;
                if u.lane.is_none()
                    || e.video_track != Some(u.track)
                    || use_.video.replace(target).is_some()
                {
                    return Err(refusal(
                        &u.id,
                        "picture selection is not a unique prepared event lane",
                    ));
                }
                use_.warp_picture_input = warp_uses.contains(u.id.as_str());
            }
            StreamKind::Audio => {
                let (channel, (_, alternatives)) = audio_lanes[group].iter().enumerate()
                    .find(|(_, (lane, _))| u.lane == Some(*lane))
                    .ok_or_else(|| refusal(&u.id, "audio selection has no exact Group lane"))?;
                let default = input.multicams[group].audio_cameras[channel];
                let choice = if selected == alternatives[default] {
                    PreparedGroupAudio::Group(channel)
                } else if direct && selected == alternatives[1 - default] {
                    loss(losses, &u.id, "group_audio_editability",
                        "alternate Group audio is a direct mono clip in Premiere; the picture angle remains editable");
                    PreparedGroupAudio::LinkedAlternate(channel)
                } else {
                    return Err(refusal(&u.id, "changing or independently selected group audio needs the qualified direct link route"));
                };
                let slot = e
                    .audio_tracks
                    .iter()
                    .position(|&t| t == u.track)
                    .ok_or_else(|| refusal(&u.id, "audio selection has no exact prepared track"))?;
                if std::mem::replace(&mut use_.audio[slot], choice) != PreparedGroupAudio::Group(usize::MAX) {
                    return Err(refusal(&u.id, "duplicate group audio binding"));
                }
            }
        }
    }
    for (si, (sequence, source)) in input.sequences.iter_mut().zip(&g.sequences).enumerate() {
        for marker in &mut sequence.markers {
            let Some(event) = marker.event.and_then(|index| usize::try_from(index).ok()) else {
                continue;
            };
            let Some(&use_index) = by_event.get(&(si, event)) else {
                continue;
            };
            if marker.owner != "edit" {
                continue;
            }
            let use_ = &input.multicam_uses[use_index];
            let direct_audio = marker.track.as_deref().and_then(|track| track.strip_prefix('A'))
                .and_then(|track| track.parse::<i64>().ok())
                .is_some_and(|track| sequence.events[event].audio_tracks.iter().zip(&use_.audio)
                    .any(|(&lane, choice)| lane == track && matches!(choice, PreparedGroupAudio::LinkedAlternate(_))));
            if marker.inherited && direct_audio {
                // The independently linked mono clip already carries its source markers.
                continue;
            }
            // Each use must keep its shared multicam source. Leaf markers inside that
            // source cannot provide a marker on the enclosing timeline edit.
            loss(losses, &source.id, "marker_ownership", format!(
                "marker {:?} on {} at {} ticks moved from the Group edit to the sequence",
                marker.name, marker.track.as_deref().unwrap_or("unknown track"), marker.start,
            ));
            marker.owner = "sequence".into();
            marker.event = None;
            marker.source = None;
            marker.inherited = false;
        }
    }
    for (b, source) in input.bins.iter_mut().zip(&g.bins) {
        b.clips = source
            .items
            .iter()
            .filter(|id| !known.contains(id.as_str()))
            .map(|id| clip_id(id))
            .collect();
    }
    for s in g
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.group" && known.contains(s.object.as_str()))
    {
        if key(&s.data, "markers").is_some_and(|v| !matches!(v, Value::Arr(xs) if xs.is_empty()))
            || key(&s.data, "marks").is_some_and(|v| match v {
                Value::Null => false,
                Value::Obj(fields) => fields.iter().any(|(_, v)| !matches!(v, Value::Null)),
                _ => true,
            })
        {
            loss(losses, &s.object, "group_metadata", "native Group marks/markers remain in source facts; editable Premiere output does not reproduce them");
        }
    }
    diagnostics.push(format!("{} editable two-camera Premiere source item(s), {} bound event(s); picture angle remains editable", input.multicams.len(), input.multicam_uses.len()));
    Ok(())
}
