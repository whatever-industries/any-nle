//! Validate the graph and resolve each edit to its saved physical stream.
use super::{markers, Plan, Result, Route, Span, RASTER, RATE};
use graph::{source::SourceGraph, Rational, StreamKind};
use std::path::Path;

fn captured_receipt(row: &graph::Sidecar) -> bool {
    use graph::Value;
    match row.adapter.as_str() {
        // The native entry points validate the saved mixers before handing
        // the graph to this destination. Keep the qualifying receipts intact.
        "premiere.source"
        | "premiere.media_capture"
        | "premiere.media_identity"
        | "premiere.audio_groups"
        | "resolve.fairlight"
        | "resolve.active_timeline"
        | "resolve.bin_order" => true,
        "resolve.media_metadata" => matches!(&row.data, Value::Obj(fields)
            if fields.iter().all(|(field, _)| field == "UniqueMediaPoolItemId" || field == "Reel")),
        "resolve.source" => matches!(&row.data, Value::Obj(fields)
            if fields.iter().all(|(field, _)| matches!(field.as_str(),
                "kind" | "cache_frames" | "video_cache_count" | "saved_selector"
                | "saved_cache_track" | "saved_channel" | "cache_samples" | "audio_import_rate"
                | "timeline_handle" | "media_pool_clip" | "bin_id" | "record_origin" | "record_length"))),
        _ => false,
    }
}

fn frames(time: Rational) -> Option<u32> {
    time.to_units(RATE)?.try_into().ok()
}

fn timeline_clock(time: Option<Rational>) -> Option<u32> {
    frames(time?).filter(|count| *count < 86_400 * RATE as u32)
}

fn matches_clock(clock: Option<&graph::source::SourceClock>, start: u32) -> bool {
    clock.is_none_or(|clock| {
        clock.effective_count == u64::from(start)
            && clock.rate == Rational::integer(RATE)
            && clock.display_base == 24
            && !clock.drop
    })
}

pub(super) fn plan<'a>(
    graph: &'a SourceGraph,
    sequence_id: &str,
    captured: bool,
) -> Result<Plan<'a>> {
    let [sequence] = graph.sequences.as_slice() else {
        return Err("linked bin pilot requires one sequence".into());
    };
    if sequence.id != sequence_id {
        return Err("linked bin sequence ID was not found".into());
    }
    let active_bins: Vec<_> = graph.bins.iter().filter(|b| !b.items.is_empty()).collect();
    if !matches!(active_bins.as_slice(), [bin] if (captured || graph.bins.len() == 1 && bin.parent.is_none())
        && graph.items.iter().all(|item| bin.items.contains(&item.id)))
    {
        return Err("linked bin pilot requires both items in the same source bin".into());
    }
    for row in &graph.sidecars {
        if !captured || !captured_receipt(row) {
            return Err(format!(
                "{}: unsupported Avid source receipt {}",
                row.object, row.adapter
            )
            .into());
        }
    }
    let start =
        timeline_clock(sequence.start).ok_or("sequence needs a known full-frame 24 fps start")?;
    if sequence.rate != Rational::integer(RATE)
        || sequence.frame_size != Some(RASTER)
        || sequence.name.trim().is_empty()
        || !matches_clock(sequence.clock.as_ref(), start)
        || !sequence.transitions.is_empty()
        || !sequence.texts.is_empty()
        || !sequence.disabled_placements.is_empty()
        || !graph.losses.is_empty()
        || !graph.multicams.is_empty()
        || !graph.multicam_uses.is_empty()
        || graph.items.len() != 2
        || graph.media.len() != 3
        || sequence
            .tracks
            .iter()
            .filter(|t| !captured || !t.empty)
            .count()
            != 3
        || sequence.placements.len() != 3
    {
        return Err(
            "linked bin pilot requires a neutral 1080p24 V1/A1/A2 graph with two originals".into(),
        );
    }
    let mut lanes = Vec::with_capacity(3);
    let mut item_ids = Vec::with_capacity(3);
    let mut picture_markers = Vec::new();
    for (kind, lane, label) in [
        (StreamKind::Video, 1, "V1"),
        (StreamKind::Audio, 1, "A1"),
        (StreamKind::Audio, 2, "A2"),
    ] {
        if !sequence.tracks.iter().any(|track| {
            track.kind == kind
                && track.index == lane
                && !track.empty
                && matches!(track.level_db, None | Some(0.0))
                && track.name.as_deref().is_none_or(|name| name == label)
        }) {
            return Err(format!("{label}: missing neutral track").into());
        }
        let picture = kind == StreamKind::Video;
        let select = |edit: &&graph::Placement| {
            if picture {
                edit.video_track == Some(lane)
            } else {
                edit.audio_tracks == [lane]
            }
        };
        let mut edits = sequence.placements.iter().filter(select);
        let edit = edits
            .next()
            .ok_or_else(|| format!("{label}: missing edit"))?;
        if edits.next().is_some()
            || (picture && (!edit.audio_tracks.is_empty() || !edit.audio_streams.is_empty()))
            || (!picture && edit.video_track.is_some())
            || !edit.grouped_audio.is_empty()
            || edit.gain_db != 0.0
            || !matches!(edit.volume_db, None | Some(0.0))
            || !edit.volume_keys.is_empty()
            || edit
                .track_gains_db
                .iter()
                .any(|gain| !matches!(gain, None | Some(0.0)))
            || edit.track_pans.iter().any(Option::is_some)
            || edit.speed.is_some()
            || edit.frame_map.is_some()
            || edit.transform.is_some()
            || !edit.effects.is_empty()
        {
            return Err(format!("{}: expected one neutral edit on {label}", edit.id).into());
        }
        let item = graph
            .items
            .iter()
            .find(|item| item.id == edit.item)
            .ok_or("missing edit item")?;
        let members = item
            .media
            .iter()
            .filter_map(|binding| {
                let media = graph.media.iter().find(|media| media.id == binding.media)?;
                (media.kind == kind).then_some((binding, media))
            })
            .collect::<Vec<_>>();
        let selected = if picture {
            matches!(members.as_slice(), [_]).then(|| members[0])
        } else if let [selector] = edit.audio_streams.as_slice() {
            let mut matches = members
                .iter()
                .filter(|(_, media)| media.stream == Some(*selector));
            let first = matches.next();
            first.filter(|_| matches.next().is_none()).copied()
        } else {
            None
        };
        let (binding, media) =
            selected.ok_or_else(|| format!("{}: unresolved stream selector", edit.id))?;
        if (!picture && !item.markers.is_empty())
            || item.color_rgb16.is_some()
            || !(item.stream_groups.is_empty()
                || captured && !picture && item.stream_groups == [vec![1, 2]])
            || item
                .rate
                .is_some_and(|rate| rate != Rational::integer(RATE))
            || binding.gain_db != 0.0
            || item.media.len() != if picture { 1 } else { 2 }
        {
            return Err(format!("{}: unsupported item controls or layout", edit.id).into());
        }
        let original = media
            .original
            .as_ref()
            .ok_or("missing physical stream facts")?;
        let ([path], [saved_path]) = (
            media.locations.as_slice(),
            original.saved_locators.as_slice(),
        ) else {
            return Err("linked bin pilot needs one exact original locator per stream".into());
        };
        let extension = if picture { "mov" } else { "wav" };
        if path != saved_path
            || !Path::new(path).is_absolute()
            || !Path::new(path)
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case(extension))
            || media.online == Some(false)
            || original.container != extension
            || original.stream_index != 0
            || original.start_pts != 0
            || original.duration_ts <= 0
            || original.time_base.num() <= 0
        {
            return Err(format!(
                "{}: original locator or physical stream is unqualified",
                edit.id
            )
            .into());
        }
        let source_lane = if picture {
            if media.frame_size != Some(RASTER)
                || media.rate != Some(Rational::integer(RATE))
                || original.channel_index.is_some()
            {
                return Err("picture edit needs 1080p24 without an audio selector".into());
            }
            1
        } else {
            if media.sample_rate != Some(48_000)
                || !matches!(media.bits, Some(16 | 24))
                || original.channels != Some(2)
            {
                return Err("audio edit needs two-channel 48 kHz PCM16 or PCM24".into());
            }
            original
                .channel_index
                .filter(|n| matches!(n, 0 | 1))
                .ok_or("missing stereo WAV channel")?
                + 1
        };
        let duration = Rational::new(
            i128::from(original.duration_ts) * i128::from(original.time_base.num()),
            i128::from(original.time_base.den()),
        )
        .ok_or("invalid physical duration")?;
        let extent = frames(duration)
            .filter(|n| *n > 0)
            .ok_or("original extent needs whole edit frames")?;
        if picture {
            picture_markers = markers::prepare(&item.markers, binding.source_offset, extent)?;
        }
        // Premiere's WAV capture leaves display timecode absent. The
        // separate physical check still requires a zero embedded origin.
        let clock = if captured && !picture && media.start.is_none() {
            0
        } else {
            timeline_clock(media.start).ok_or("missing original timecode")?
        };
        if duration != media.duration || !matches_clock(media.clock.as_ref(), clock) {
            return Err(format!("{}: conflicting source extent or timecode", edit.id).into());
        }
        if !picture && clock != 0 {
            return Err(
                format!("{}: this WAV profile requires a zero source start", edit.id).into(),
            );
        }
        let source = binding
            .source_offset
            .and_then(|offset| edit.source_in.checked_add(offset));
        let span = Span {
            record: frames(edit.record_start).ok_or("record start is not on the frame grid")?,
            source: source
                .and_then(frames)
                .ok_or("source trim has an unknown offset or falls off the frame grid")?,
            length: frames(edit.duration)
                .filter(|n| *n > 0)
                .ok_or("edit needs a positive whole-frame duration")?,
        };
        if span
            .source
            .checked_add(span.length)
            .is_none_or(|end| end > extent)
            || span.source > i32::MAX as u32
        {
            return Err(format!("{}: source range exceeds the original extent", edit.id).into());
        }
        item_ids.push(item.id.as_str());
        lanes.push((
            Route {
                path,
                name: &item.name,
                tape: item.tape.as_deref(),
                clock,
                extent,
                source_lane,
                bits: media.bits,
                stream: original,
            },
            span,
        ));
    }
    let [(video, v), (left, a), (right, b)] =
        lanes.try_into().map_err(|_| "missing linked lane")?;
    if item_ids[0] == item_ids[1]
        || item_ids[1] != item_ids[2]
        || video.path == left.path
        || left.path != right.path
        || left.clock != right.clock
        || left.extent != right.extent
        || left.source_lane == right.source_lane
        || left.bits != right.bits
        || left.stream.time_base != right.stream.time_base
        || left.stream.duration_ts != right.stream.duration_ts
    {
        return Err("A1 and A2 must select distinct channels of the same verified WAV item".into());
    }
    let length = [v, a, b]
        .iter()
        .map(|span| span.record.checked_add(span.length))
        .try_fold(0, |length, end| Some(length.max(end?)))
        .ok_or("sequence end overflow")?;
    let sequence_markers = markers::prepare(&sequence.markers, Some(Rational::ZERO), length)?;
    Ok(Plan {
        name: &sequence.name,
        start,
        length,
        routes: [video, left, right],
        spans: [v, a, b],
        picture_markers,
        sequence_markers,
    })
}
