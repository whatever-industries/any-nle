//! Recover verified disabled edits with independently verified managed media.
use super::subclips::{field_int, mob_id, urn};
use super::*;
use std::collections::BTreeMap;

const FRAME: i64 = TICKS_PER_SECOND / 24;

struct InactiveEdit<'a> {
    owner: &'a str,
    item: &'a SourceItem,
    track: i64, // V1 is zero.
    start: i64,
    duration: i64,
    source_in: i64,
    channel: i64,
    stereo: bool,
    cached_picture_package: Option<&'a str>,
}

fn from_graph<'a>(placement: &'a Placement, item: &'a SourceItem) -> Option<InactiveEdit<'a>> {
    if placement.gain_db != 0.0 || placement.volume_db.is_some()
        || !placement.volume_keys.is_empty() || !placement.track_gains_db.is_empty()
        || !placement.track_pans.is_empty() || placement.speed.is_some()
        || placement.frame_map.is_some() || placement.transform.is_some()
        || !placement.effects.is_empty()
    {
        return None;
    }
    let (track, channel, stereo) = match (
        placement.video_track, placement.audio_tracks.as_slice(),
        placement.audio_streams.as_slice(), placement.grouped_audio.as_slice(),
    ) {
        (Some(1), [], [], []) => (0, 1, false),
        (None, [track @ (1 | 2)], [channel @ (1 | 2)], []) => (*track, *channel, false),
        (None, [], [], [group])
            if group.track == 3 && group.streams == [1, 2]
                && group.gain_db.is_none() && group.pans.is_empty() => (3, 1, true),
        _ => return None,
    };
    let on_grid = |time: Rational| time.to_units(TICKS_PER_SECOND).filter(|ticks| ticks % FRAME == 0);
    Some(InactiveEdit {
        owner: placement.id.as_str(),
        item,
        track,
        start: on_grid(placement.record_start)?,
        duration: on_grid(placement.duration)?,
        source_in: on_grid(placement.source_in)?,
        channel,
        stereo,
        cached_picture_package: None,
    })
}

fn from_legacy<'a>(
    issue: &'a Value,
    owner: &'a str,
    masters: &HashMap<String, Option<&'a SourceItem>>,
    has_format: impl Fn(i64, i64) -> bool,
) -> Option<InactiveEdit<'a>> {
    if key(issue, "disabled_clip").and_then(boolean) != Some(true)
        || key(issue, "record_rate").and_then(str_value) != Some("24")
    {
        return None;
    }
    let track = field_int(issue, "track")?;
    let kind = field_int(issue, "kind")?;
    let class = key(issue, "backing_class").and_then(str_value)?;
    let stereo = key(issue, "plain_stereo_backing").and_then(boolean) == Some(true);
    let picture_file = key(issue, "neutral_picture_file").and_then(str_value)
        .filter(|id| mob_id(id));
    let track = match (kind, track, class, stereo) {
        (1, 1, "SCLP", false) if picture_file.is_some() => 0,
        (2, 3, "TKFX", true) if has_format(track, 2) => track,
        (2, 1 | 2, "SCLP", false)
            if has_format(track, 1)
                && key(issue, "plain_mono_backing").and_then(boolean) == Some(true) => track,
        _ => return None,
    };
    let start = field_int(key(issue, "record_start")?, "value")?.checked_mul(FRAME)?;
    let len = field_int(issue, "record_length").filter(|&n| n > 0)?;
    let duration = len.checked_mul(FRAME)?;
    let Value::Arr(route) = key(issue, "route")? else { return None };
    let [leaf, rest @ ..] = route.as_slice() else { return None };
    let channel = field_int(leaf, "track").filter(|&n| n == 1 || (track != 0 && n == 2))?;
    let mob = key(leaf, "mob").and_then(str_value)?;
    let item = masters.get(mob).copied().flatten()?;
    let source_in = field_int(key(leaf, "source_offset")?, "value")?.checked_mul(FRAME)?;
    let matches_lane = |leaf: &Value, expected: i64| {
        key(leaf, "rate").and_then(str_value) == Some("24")
            && key(leaf, "mob").and_then(str_value) == Some(mob)
            && field_int(leaf, "track") == Some(expected)
            && key(leaf, "source_offset").and_then(|v| field_int(v, "value"))
                .and_then(|n| n.checked_mul(FRAME)) == Some(source_in)
            && field_int(key(leaf, "length").unwrap_or(&Value::Null), "value") == Some(len)
    };
    if !matches_lane(leaf, channel)
        || match rest {
            [right] if stereo && channel == 1 => !matches_lane(right, 2),
            [] if !stereo => false,
            _ => true,
        }
    {
        return None;
    }
    Some(InactiveEdit {
        owner, item, track, start, duration, source_in, channel, stereo,
        cached_picture_package: picture_file.filter(|_| track == 0),
    })
}

pub(super) fn prepare<'a>(
    graph: &'a SourceGraph,
    media: &HashMap<&str, &SourceMedia>,
    sidecars: &Sidecars<'a>,
    unsupported_stereo: &HashSet<&str>,
    output: &mut PreparedProjectInput,
    diagnostics: &mut Vec<String>,
) -> HashSet<(&'a str, &'a str)> {
    let items: HashMap<_, _> = graph.items.iter().map(|item| (item.id.as_str(), item)).collect();
    let mut masters = HashMap::new();
    for item in &graph.items {
        if let Some(id) = item.umid.as_deref().filter(|id| mob_id(id)) {
            masters
                .entry(urn(id))
                .and_modify(|slot| *slot = None)
                .or_insert(Some(item));
        }
    }
    let clip_indexes: HashMap<_, _> = output
        .clips
        .iter()
        .enumerate()
        .map(|(index, clip)| (clip.id().to_owned(), index))
        .collect();
    let mut restored = HashSet::new();
    for (index, source) in graph.sequences.iter().enumerate() {
        let receipt = side(sidecars, "avid.source", &source.id);
        let issues = match receipt.and_then(|data| key(data, "unresolved")) {
            Some(Value::Arr(issues)) => issues.as_slice(),
            _ => &[],
        };
        if source.rate != Rational::integer(24)
            || !source.transitions.is_empty()
            || receipt.and_then(|data| key(data, "native_transitions"))
                .is_some_and(|v| !matches!(v, Value::Arr(a) if a.is_empty()))
            || !source.markers.is_empty()
            || (source.disabled_placements.is_empty() && issues.is_empty())
        {
            continue;
        }
        let sequence = &mut output.sequences[index];
        if (sequence.width, sequence.height) != (1920, 1080) {
            continue;
        }
        let extent = sequence
            .events
            .iter()
            .filter_map(|e| e.record_start.checked_add(e.duration))
            .max()
            .unwrap_or(0);
        let formats = match receipt.and_then(|data| key(data, "audio_track_formats")) {
            Some(Value::Arr(formats)) => Some(formats.as_slice()),
            _ => None,
        };
        let has_format = |track, format| {
            let mut entries = formats.into_iter().flatten().filter_map(|entry| match entry {
                Value::Arr(values) if values.first().and_then(integer) == Some(track) => {
                    Some(values)
                }
                _ => None,
            });
            matches!(entries.next().map(Vec::as_slice), Some([_, Value::Int(value)]) if *value == format)
                && entries.next().is_none()
        };
        let mut candidates = Vec::new();
        let mut owners = HashMap::new();
        let mut rows: [BTreeMap<i64, i64>; 4] = Default::default();
        for edit in &sequence.events {
            for track in edit
                .audio_tracks
                .iter()
                .copied()
                .chain(edit.stereo.iter().map(|st| st.audio_track))
                .chain((edit.video_track == Some(1)).then_some(0))
            {
                if let 0..=3 = track {
                    if let Some(end) = edit.record_start.checked_add(edit.duration) {
                        let row = &mut rows[track as usize];
                        let end = end.max(row.get(&edit.record_start).copied().unwrap_or(0));
                        row.insert(edit.record_start, end);
                    }
                }
            }
        }
        let mut verified = HashMap::new();
        let mut verified_stereo = HashMap::new();
        let portable_ids: HashSet<_> = source.disabled_placements.iter().map(|p| p.id.as_str()).collect();
        let legacy = issues.iter().filter_map(|issue| {
            let owner = key(issue, "owner").and_then(str_value).filter(|s| !s.is_empty())?;
            if portable_ids.contains(owner) {
                return None;
            }
            *owners.entry(owner).or_insert(0usize) += 1;
            formats?;
            from_legacy(issue, owner, &masters, has_format)
        });
        let portable = source.disabled_placements.iter().filter_map(|placement| {
            from_graph(placement, items[placement.item.as_str()])
        });
        for edit in legacy.chain(portable) {
            let InactiveEdit {
                owner, item, track, start, duration, source_in, channel, stereo,
                cached_picture_package,
            } = edit;
            let Some(end) = start.checked_add(duration) else { continue };
            if start < 0 || duration <= 0 || end > extent || source_in < 0 {
                continue;
            }
            let Some(source_end) = source_in.checked_add(duration) else { continue };
            let picture = track == 0;
            let matches_lane = |expected| item.media.iter().find(|reference| {
                let m = media[reference.media.as_str()];
                if picture { m.kind == StreamKind::Video }
                else { m.kind == StreamKind::Audio && m.stream == Some(expected) }
            }).is_some_and(|reference| {
                let m = media[reference.media.as_str()];
                reference.gain_db == 0.0
                    && (!(stereo || picture) || reference.source_offset == Some(Rational::ZERO))
                    && m.duration.to_units(TICKS_PER_SECOND).is_some_and(|extent| source_end <= extent)
                    && cached_picture_package.is_none_or(|package| {
                        side(sidecars, "avid.source", &m.id).and_then(|v| key(v, "file_package"))
                            .and_then(str_value).is_some_and(|saved| saved.eq_ignore_ascii_case(package))
                    })
            });
            if !matches_lane(channel) || (stereo && !matches_lane(2)) {
                continue;
            }
            let Some(&clip) = clip_indexes.get(&clip_id(&item.id)) else { continue };
            if stereo {
                if unsupported_stereo.contains(item.id.as_str())
                    || !matches!(output.clips[clip], PreparedClip::Legacy(_))
                    || !*verified_stereo.entry(item.id.as_str()).or_insert_with(|| {
                        item.markers.is_empty() && verified_stereo_siblings(item, media, sidecars)
                    })
                {
                    continue;
                }
            } else if (picture && !matches!(&output.clips[clip], PreparedClip::Legacy(clip) if clip.video.is_some()))
                || !*verified.entry(item.id.as_str()).or_insert_with(|| {
                    item.markers.is_empty() && matches!(
                        direct_opatom_clocks(item, media, sidecars, true, true, false),
                        Some((0, 0, _, _))
                    )
                })
            {
                continue;
            }
            candidates.push((track, start, end, channel, source_in, clip, stereo, owner));
        }
        candidates.sort_by_key(|&(track, start, end, ..)| (track, start, end));
        let mut previous = [None::<i64>; 4];
        // Replace overlapping intervals by prefix maxima so a longer, earlier clip
        // cannot be hidden by a nearer, shorter one on the same lane.
        for row in &mut rows {
            let mut end = 0;
            for current in row.values_mut() {
                end = end.max(*current);
                *current = end;
            }
        }
        for (i, &(track, start, end, channel, source_in, clip, stereo, owner)) in
            candidates.iter().enumerate()
        {
            let lane = track as usize;
            let earlier = previous[lane].is_some_and(|e| e > start);
            previous[lane] = Some(previous[lane].unwrap_or(0).max(end));
            let later = candidates
                .get(i + 1)
                .is_some_and(|next| next.0 == track && next.1 < end);
            let active = rows[lane]
                .range(..end)
                .next_back()
                .is_some_and(|(_, &e)| e > start);
            if earlier || later || active || owners.get(owner).is_some_and(|&count| count != 1) {
                continue;
            }
            let (audio_tracks, audio_channels, stereo_edits) = if stereo {
                let PreparedClip::Legacy(clip) = &mut output.clips[clip] else {
                    unreachable!()
                };
                if !clip.stereo_pairs.contains(&(1, 2)) {
                    clip.stereo_pairs.push((1, 2));
                }
                (
                    vec![],
                    vec![],
                    vec![StereoEdit {
                        audio_track: track,
                        channels: vec![1, 2],
                        gain_db: None,
                        pans: vec![None, None],
                    }],
                )
            } else if track == 0 {
                (vec![], vec![], vec![])
            } else {
                (vec![track], vec![channel], vec![])
            };
            let event = sequence.events.len();
            sequence.events.push(Event {
                clip: output.clips[clip].id().to_owned(),
                record_start: start,
                duration: end - start,
                source_in,
                video_track: (track == 0).then_some(1),
                audio_tracks,
                audio_channels,
                audio: EventAudio::default(),
                stereo: stereo_edits,
                audio_gains: vec![],
                audio_pans: vec![],
                speed_ratio: None,
                source_duration: None,
                color_correction: None,
                transform: None,
                matte: None,
                time_remap: None,
            });
            output.disabled_edits.insert((index, event));
            restored.insert((source.id.as_str(), owner));
            let location = match (track, stereo) {
                (0, _) => "picture clip on V1".into(),
                (_, true) => format!("stereo clip on A{track}"),
                (_, false) => format!("mono clip on A{track}"),
            };
            diagnostics.push(format!(
                "{owner}: preserved disabled {location} at record frame {}",
                start / FRAME
            ));
        }
    }
    restored
}
