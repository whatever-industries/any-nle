//! Resolve the saved backing routes of qualified inactive Avid cuts.
use super::*;

pub(super) fn capture(
    source: &av::SourceSequence,
    copies: &HashMap<avid_reader::io::MobId, Vec<(usize, usize)>>,
    graph: &SourceGraph,
    budget: &mut Budget,
) -> Result<Vec<Placement>, String> {
    let rate = Rate::new(24, 1).unwrap();
    if source.rate != Some(rate) || !source.unresolved.iter().any(|issue| matches!(
        issue.issue_kind, av::SourceIssueKind::DisabledClip { plain_mono: true, .. }
            | av::SourceIssueKind::DisabledClip { plain_stereo: true, .. }
            | av::SourceIssueKind::DisabledClip { neutral_picture: Some(_), .. }
    )) {
        return Ok(Vec::new());
    }
    let mut owners = HashMap::new();
    for issue in &source.unresolved {
        *owners.entry(issue.owner.as_str()).or_insert(0usize) += 1;
    }
    let media: HashMap<_, _> = graph.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let packages: HashMap<_, _> = graph.sidecars.iter()
        .filter(|sidecar| sidecar.adapter == "avid.source")
        .filter_map(|sidecar| match sidecar.data.get("file_package") {
            Some(Value::Str(package)) => Some((sidecar.object.as_str(), package.as_str())),
            _ => None,
        })
        .collect();
    let has_format = |track, format| {
        source.info.audio_track_formats.iter()
            .filter_map(|&(lane, value)| (lane == track).then_some(value))
            .eq([format])
    };
    source.unresolved.iter().filter_map(|issue| {
        let av::SourceIssueKind::DisabledClip {
            length, backing_class, plain_mono, plain_stereo, neutral_picture,
        } = issue.issue_kind else { return None };
        let Observed::Known(start) = issue.record_start else { return None };
        if owners[issue.owner.as_str()] != 1 || issue.record_rate != Some(rate)
            || start < 0 || length <= 0
        {
            return None;
        }
        let [first, rest @ ..] = issue.route.as_slice() else { return None };
        let channel = first.track;
        let track = issue.track;
        let kind = kind(issue.kind)?;
        match (kind, track, &backing_class, rest) {
            (StreamKind::Video, 1, b"SCLP", []) if channel == 1 && neutral_picture.is_some() => {}
            (StreamKind::Audio, 1 | 2, b"SCLP", [])
                if plain_mono && has_format(track, 1) && matches!(channel, 1 | 2) => {}
            (StreamKind::Audio, 3, b"TKFX", [right])
                if plain_stereo && has_format(track, 2) && channel == 1 && right.track == 2 => {}
            _ => return None,
        }
        let [(_, item_index)] = copies.get(&first.mob)?.as_slice() else { return None };
        let item = &graph.items[*item_index];
        let Observed::Known(source_in) = first.source_offset else { return None };
        if source_in < 0 || !issue.route.iter().all(|route| {
            route.mob == first.mob && route.rate == Some(rate)
                && route.length == Observed::Known(length)
                && route.source_offset == first.source_offset
                && item.media.iter().any(|binding| {
                    let m = media[binding.media.as_str()];
                    m.kind == kind && (kind == StreamKind::Video || m.stream == Some(route.track))
                        && neutral_picture.is_none_or(|package| {
                            packages.get(m.id.as_str())
                                .is_some_and(|saved| saved.eq_ignore_ascii_case(&package.umid_hex()))
                        })
                })
        }) {
            return None;
        }
        Some((issue.owner.as_str(), &item.id, kind, track, channel, plain_stereo, start, source_in, length))
    }).map(|(id, item, kind, track, channel, stereo, start, source_in, length)| {
        budget.charge(size_of::<Placement>() + id.len() + item.len() + 256)?;
        let picture = kind == StreamKind::Video;
        Ok(Placement {
            id: id.into(),
            item: item.clone(),
            record_start: time(start, rate)?,
            source_in: time(source_in, rate)?,
            duration: time(length, rate)?,
            video_track: picture.then_some(track),
            audio_tracks: if picture || stereo { vec![] } else { vec![track] },
            audio_streams: if picture || stereo { vec![] } else { vec![channel] },
            grouped_audio: if stereo {
                vec![GroupedAudio { track, streams: vec![1, 2], gain_db: None, pans: vec![] }]
            } else { vec![] },
            gain_db: 0.0,
            volume_db: None,
            volume_keys: vec![],
            track_gains_db: vec![],
            track_pans: vec![],
            speed: None,
            frame_map: None,
            transform: None,
            effects: vec![],
        })
    }).collect()
}
