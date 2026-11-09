//! Source Group interpretation for the bounded typed Premiere writer.
use super::*;
use graph::source::{MulticamAlternative, MulticamPiece};
use prproj::convert::multicam::{
    PreparedCamera, PreparedMulticam, PreparedMulticamUse, MAX_GROUPS, RATE,
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
    diagnostics: &mut Vec<String>,
    losses: &mut Vec<Loss>,
) -> R<()> {
    if g.multicams.is_empty() && g.multicam_uses.is_empty() {
        return Ok(());
    }
    if g.multicams.len() > MAX_GROUPS || g.multicam_uses.len() > model::MAX_ITEMS {
        return Err(refusal("project", "prepared group/use count exceeds limit"));
    }
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
    for m in &g.multicams {
        if m.reason.is_some() || m.rate.map(fraction) != Some(RATE) || m.lanes.len() != 3 {
            return Err(refusal(
                &m.id,
                "needs one resolved picture and two resolved mono lanes at 23.976 fps",
            ));
        }
        let mut audio: Vec<_> = m
            .lanes
            .iter()
            .filter(|l| l.kind == StreamKind::Audio)
            .collect();
        audio.sort_by_key(|l| l.index);
        let [a1, a2] = audio.as_slice() else {
            return Err(refusal(&m.id, "needs exactly two audio lanes"));
        };
        let video = m
            .lanes
            .iter()
            .find(|l| l.kind == StreamKind::Video)
            .ok_or_else(|| refusal(&m.id, "missing picture lane"))?;
        if video.selected != Some(0)
            || [video, *a1, *a2]
                .iter()
                .any(|l| l.reason.is_some() || l.alternatives.len() != 2)
        {
            return Err(refusal(
                &m.id,
                "two resolved cameras and the first default picture angle are required",
            ));
        }
        let mut cameras = Vec::with_capacity(2);
        for (i, alt) in video.alternatives.iter().enumerate() {
            let picture = real_piece(alt, &m.id)?;
            if picture.stream != Some(1) {
                return Err(refusal(
                    &m.id,
                    "camera picture does not resolve to the leaf's single video stream",
                ));
            }
            let mut labels = [0; 2];
            for (j, lane) in [*a1, *a2].into_iter().enumerate() {
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
                if (lane.selected == Some(index)) != (i == 0) {
                    return Err(refusal(
                        &m.id,
                        "both default audio lanes must resolve to the first camera",
                    ));
                }
                labels[j] = label;
            }
            cameras.push(PreparedCamera {
                clip: clip_id(picture.item.as_deref().unwrap()),
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
        let bin = *bin_indices
            .get(m.id.as_str())
            .ok_or_else(|| refusal(&m.id, "group has no owning bin"))?;
        let duration = ticks(
            m.duration
                .ok_or_else(|| refusal(&m.id, "group extent unresolved"))?,
            "group duration",
        )?;
        indices.insert(m.id.as_str(), input.multicams.len());
        audio_lanes.push([(a1.index, a1.selected), (a2.index, a2.selected)]);
        input.multicams.push(PreparedMulticam {
            id: m.id.clone(),
            name: m.name.clone(),
            bin,
            duration,
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
                audio: vec![usize::MAX; e.audio_tracks.len()],
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
                if u.lane.is_none()
                    || e.video_track != Some(u.track)
                    || !(0..2).contains(&selected)
                    || use_.video.replace(selected as usize).is_some()
                {
                    return Err(refusal(
                        &u.id,
                        "picture selection is not a unique prepared event lane",
                    ));
                }
            }
            StreamKind::Audio => {
                let channel = audio_lanes[group].iter().position(|&(lane, selection)| {
                    u.lane == Some(lane) && selection == Some(selected)
                }).ok_or_else(|| refusal(&u.id, "changing or independently selected group audio needs separate qualification"))?;
                let slot = e
                    .audio_tracks
                    .iter()
                    .position(|&t| t == u.track)
                    .ok_or_else(|| refusal(&u.id, "audio selection has no exact prepared track"))?;
                if std::mem::replace(&mut use_.audio[slot], channel) != usize::MAX {
                    return Err(refusal(&u.id, "duplicate group audio binding"));
                }
            }
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
    diagnostics.push(format!("{} editable two-camera Premiere source item(s), {} bound event(s); fixed first-camera mono audio, picture angle remains editable", input.multicams.len(), input.multicam_uses.len()));
    Ok(())
}
