//! Resolve graph/2 angle tracks and independently selected mono sources.
use super::*;
use graph::{
    source::{Multicam, MulticamAlternative, MulticamUse},
    StreamKind,
};
use prproj::convert::{
    multicam_tracks::{Piece, PreparedTrackMulticam, Use},
    writer::{prepare_typed_project_for_premiere_27, TypedMedia},
};
use std::collections::BTreeMap;

const TRACKS_ADAPTER: &str = "resolve.multicam_tracks";

pub(super) fn legacy_profile(graph: &SourceGraph, sequence: &SourceSequence) -> bool {
    let candidates: HashSet<_> = graph
        .multicam_uses
        .iter()
        .filter(|edit| edit.sequence == sequence.id)
        .flat_map(|edit| &edit.candidates)
        .map(String::as_str)
        .collect();
    sequence.frame_size == Some((1920, 1080))
        && sequence.rate == crate::prepare::rate()
        && candidates.len() == 1
        && graph.multicams.iter().any(|group| {
            candidates.contains(group.id.as_str())
                && group
                    .lanes
                    .iter()
                    .filter(|lane| lane.kind == StreamKind::Audio)
                    .count()
                    == 2
                && group.lanes.iter().any(|lane| {
                    lane.kind == StreamKind::Video
                        && lane.alternatives.len() == 2
                        && lane.alternatives.iter().all(|alt| {
                            alt.pieces
                                .iter()
                                .filter(|piece| piece.item.is_some())
                                .count()
                                == 1
                        })
                })
        })
}

pub(super) fn write(
    mut graph: SourceGraph,
    sequence_index: usize,
    ffprobe: &Path,
    output: &Path,
    accept_losses: bool,
) -> Result<Vec<Loss>> {
    let sequence = &graph.sequences[sequence_index];
    let fail = |reason: &str| format!("{}: {reason}", sequence.id);
    let size = sequence
        .frame_size
        .ok_or_else(|| fail("unknown Resolve timeline raster"))?;
    if !matches!(
        (sequence.rate.num(), sequence.rate.den()),
        (24, 1) | (24_000, 1001)
    ) || !sequence
        .clock
        .as_ref()
        .is_some_and(|clock| !clock.drop && clock.provenance.route == "Resolve archive clock")
        || !sequence.transitions.is_empty()
        || !sequence.texts.is_empty()
        || !sequence.disabled_placements.is_empty()
        || [
            crate::generator::ADAPTER,
            "resolve.nested_uses",
            crate::dissolve::ADAPTER,
        ]
        .into_iter()
        .any(|adapter| graph.sidecar(adapter, &sequence.id).is_some())
    {
        return Err(
            fail("unsupported Resolve linked multicam timeline timing or structure").into(),
        );
    }
    crate::linked::check_origin(sequence).map_err(|error| format!("{}: {error}", sequence.id))?;
    let mut uses: BTreeMap<&str, Vec<&MulticamUse>> = BTreeMap::new();
    let mut omitted_uses = HashSet::new();
    let mut losses = Vec::new();
    for edit in graph
        .multicam_uses
        .iter()
        .filter(|edit| edit.sequence == sequence.id)
    {
        let [group] = edit.candidates.as_slice() else {
            return Err(format!("{}: ambiguous Resolve multicam identity", edit.id).into());
        };
        if let Some(reason) = &edit.reason {
            if !edit.binding.placements.is_empty() {
                return Err(format!(
                    "{}: unresolved multicam has an active leaf binding",
                    edit.id
                )
                .into());
            }
            omitted_uses.insert(edit.id.as_str());
            losses.push(Loss {
                object: edit.id.clone(),
                aspect: "multicam_binding".into(),
                reason: reason.clone(),
            });
        } else {
            uses.entry(group).or_default().push(edit);
        }
    }
    if uses.is_empty() {
        return Err(fail("no qualified Resolve multicam edits on the selected timeline").into());
    }

    let definitions: HashMap<_, _> = graph
        .multicams
        .iter()
        .map(|group| (group.id.as_str(), group))
        .collect();
    let original_items: HashMap<_, _> = graph
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let original_media: HashMap<_, _> = graph
        .media
        .iter()
        .map(|media| (media.id.as_str(), media))
        .collect();
    let placements: HashMap<_, _> = sequence
        .placements
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let mut claimed = HashSet::new();
    let mut groups = Vec::new();
    for (id, edits) in uses {
        let group = definitions[id];
        let invalid = |reason: &str| format!("{id}: {reason}");
        let source = graph
            .sidecar(TRACKS_ADAPTER, id)
            .ok_or_else(|| invalid("recapture the Resolve archive to retain the source raster"))?;
        if group.reason.is_some()
            || group.rate != Some(sequence.rate)
            || source.get("frame_size")
                != Some(&Value::Arr(vec![Value::Int(size.0), Value::Int(size.1)]))
        {
            return Err(invalid(
                "unqualified Resolve multicam clock, source canvas, or definition",
            )
            .into());
        }
        let duration = ticks(group.duration, id)?;
        let origin = match source.get("record_origin") {
            Some(Value::Str(value)) => ticks(Rational::parse(value), id)?,
            _ => return Err(invalid("unresolved Resolve multicam origin").into()),
        };
        let picture = group
            .lanes
            .iter()
            .find(|lane| lane.kind == StreamKind::Video && lane.index == 1);
        let audio = group
            .lanes
            .iter()
            .find(|lane| lane.kind == StreamKind::Audio && lane.index == 1);
        if group.lanes.len() != usize::from(picture.is_some()) + usize::from(audio.is_some()) {
            return Err(invalid("expected picture and at most one independent mono lane").into());
        }
        let mut out = PreparedTrackMulticam {
            id: id.into(),
            name: group.name.clone(),
            bin: 0,
            sequence: 0,
            origin,
            duration,
            picture: Vec::new(),
            mono: Vec::new(),
            uses: Vec::new(),
        };
        let fit = matches!(source.get(crate::input_scaling::PROJECT_SETTING), Some(Value::Str(mode)) if mode == crate::input_scaling::FIT);
        let mut selected_mono = None;
        for edit in edits {
            let (lane, angle) = match edit.kind {
                StreamKind::Video => (
                    picture,
                    Some(usize::try_from(
                        edit.selected.ok_or("missing selected picture angle")?,
                    )?),
                ),
                StreamKind::Audio => {
                    let selected = edit.selected.ok_or("missing selected mono source")?;
                    if selected_mono
                        .replace(selected)
                        .is_some_and(|last| last != selected)
                    {
                        return Err(format!(
                            "{}: switched mono source inside one multicam is unqualified",
                            edit.id
                        )
                        .into());
                    }
                    (audio, None)
                }
            };
            let alternative = lane
                .filter(|lane| Some(lane.index) == edit.lane)
                .and_then(|lane| lane.alternatives.get(edit.selected.unwrap_or(-1) as usize))
                .ok_or_else(|| format!("{}: absent Resolve multicam angle or lane", edit.id))?;
            require_binding(group, edit, alternative, &placements, &mut claimed)?;
            out.uses.push(Use {
                angle,
                track: edit.track,
                start: ticks(edit.record_start, &edit.id)?,
                source_in: ticks(edit.source_in, &edit.id)?,
                duration: ticks(edit.duration, &edit.id)?,
            });
        }
        if let Some(picture) = picture {
            for angle in &picture.alternatives {
                let mut pieces = prepare_pieces(angle, true, &original_items, &original_media)?;
                for piece in &mut pieces {
                    let item = original_items[piece.clip.as_str()];
                    let raster = item
                        .media
                        .iter()
                        .filter_map(|link| original_media.get(link.media.as_str()))
                        .find(|media| media.kind == StreamKind::Video)
                        .and_then(|media| media.frame_size);
                    if raster != Some(size) {
                        if !fit {
                            return Err(format!(
                                "{}: unknown Resolve multicam raster fitting",
                                item.id
                            )
                            .into());
                        }
                        piece.fit = true;
                    }
                }
                out.picture.push(pieces);
            }
            losses.push(Loss { object: id.into(), aspect: "multicam_picture_default".into(),
                reason: "Premiere supplies its default angle for fresh edits. Saved edit angles are retained.".into() });
        }
        if let Some(lane) = audio {
            let selected = selected_mono
                .or(lane.selected)
                .or_else(|| (lane.alternatives.len() == 1).then_some(0))
                .ok_or_else(|| {
                    invalid("multiple mono source alternatives without a saved selection")
                })?;
            if lane.alternatives.len() != 1 {
                return Err(invalid(
                    "independent alternate audio sources need native qualification",
                )
                .into());
            }
            out.mono = prepare_pieces(
                &lane.alternatives[selected as usize],
                false,
                &original_items,
                &original_media,
            )?;
        }
        groups.push(out);
    }

    let mut used: HashSet<String> = groups
        .iter()
        .flat_map(|group| group.picture.iter().flatten().chain(&group.mono))
        .map(|piece| piece.clip.clone())
        .collect();
    used.extend(
        sequence
            .placements
            .iter()
            .filter(|p| !claimed.contains(p.id.as_str()))
            .map(|p| p.item.clone()),
    );
    let group_ids: HashSet<_> = groups.iter().map(|group| group.id.as_str()).collect();
    let used_media: HashSet<String> = graph
        .items
        .iter()
        .filter(|item| used.contains(&item.id))
        .flat_map(|item| item.media.iter().map(|link| link.media.clone()))
        .collect();
    let owners: HashSet<_> = std::iter::once(sequence.id.as_str())
        .chain(group_ids.iter().copied())
        .chain(used.iter().map(String::as_str))
        .chain(used_media.iter().map(String::as_str))
        .chain(sequence.placements.iter().map(|p| p.id.as_str()))
        .collect();
    losses.extend(
        graph
            .losses
            .iter()
            .filter(|loss| {
                (owners.contains(loss.object.as_str())
                    || loss
                        .object
                        .split_once('/')
                        .is_some_and(|(owner, _)| owners.contains(owner)))
                    && !(omitted_uses.contains(loss.object.as_str())
                        && loss.aspect == "multicam_binding")
                    && !(loss.aspect == "multicam_binding"
                        && loss.reason == crate::source::MULTICAM_OBSERVED_GAP)
            })
            .cloned(),
    );
    if !accept_losses && !losses.is_empty() {
        let first = &losses[0];
        return Err(format!(
            "{}: {} ({} occurrence(s) require --accept-losses)",
            first.object,
            first.reason,
            losses.len()
        )
        .into());
    }
    let claimed: HashSet<String> = claimed.into_iter().map(str::to_owned).collect();
    graph.sequences = vec![graph.sequences.swap_remove(sequence_index)];
    graph.sequences[0]
        .placements
        .retain(|edit| !claimed.contains(&edit.id));
    graph.multicams.clear();
    graph.multicam_uses.clear();
    graph.losses.clear();
    graph.items.retain(|item| used.contains(&item.id));
    graph.media.retain(|media| used_media.contains(&media.id));
    graph
        .bins
        .iter_mut()
        .for_each(|bin| bin.items.retain(|id| used.contains(id)));
    let mut scoped = used;
    scoped.extend(used_media);
    scoped.insert(graph.sequences[0].id.clone());
    scoped.extend(graph.bins.iter().map(|bin| bin.id.clone()));
    scoped.extend(
        graph.sequences[0]
            .placements
            .iter()
            .map(|edit| edit.id.clone()),
    );
    graph.sidecars.retain(|row| scoped.contains(&row.object));
    let verified = verify_originals(&mut graph, ffprobe)?;
    let mut prepared = premiere_adapter::prepare_resolve_originals(&graph, &Options::default())?;
    let group_bin = prepared.input.bins.len();
    prepared.input.bins.push(prproj::convert::model::Bin {
        name: "Multicam Sources".into(),
        clips: Vec::new(),
    });
    for group in &mut groups {
        group.bin = group_bin;
        for piece in group.picture.iter_mut().flatten().chain(&mut group.mono) {
            if let Some(id) = piece.clip.strip_prefix("clip:") {
                piece.clip = id.to_owned();
            }
        }
    }
    prepared.input.multicam_tracks = groups;
    let staged = prepare_typed_project_for_premiere_27(
        &prepared.input,
        output,
        &mut RandomUuids::new(),
        TypedMedia::Bound {
            bindings: &[],
            native_av: false,
        },
        None,
    )?;
    for entry in verified.values() {
        entry.0.recheck()?;
    }
    staged.publish()?;
    losses.extend(prepared.losses);
    Ok(losses)
}

fn ticks(time: Option<Rational>, owner: &str) -> Result<i64> {
    time.and_then(|time| time.to_units(prproj::build::time::TICKS_PER_SECOND))
        .ok_or_else(|| format!("{owner}: missing or inexact multicam time").into())
}

fn prepare_pieces(
    alternative: &MulticamAlternative,
    picture: bool,
    items: &HashMap<&str, &graph::source::SourceItem>,
    media: &HashMap<&str, &graph::source::SourceMedia>,
) -> Result<Vec<Piece>> {
    if alternative.reason.is_some() {
        return Err(format!("{}: unqualified multicam angle", alternative.id).into());
    }
    let mut pieces = Vec::new();
    for piece in &alternative.pieces {
        let Some(item) = &piece.item else { continue };
        let kind = if picture {
            StreamKind::Video
        } else {
            StreamKind::Audio
        };
        let links = items[item.as_str()]
            .media
            .iter()
            .filter(|link| {
                let stream = media[link.media.as_str()];
                stream.kind == kind
                    && (stream.stream == piece.stream
                        || (picture && stream.stream.is_none() && piece.stream == Some(1)))
            })
            .collect::<Vec<_>>();
        if !matches!(links.as_slice(), [link] if link.source_offset == Some(Rational::ZERO) && link.gain_db == 0.0)
        {
            return Err(
                format!("{}: unresolved original offset or channel", alternative.id).into(),
            );
        }
        pieces.push(Piece {
            clip: item.clone(),
            fit: false,
            channel: if picture {
                0
            } else {
                piece.stream.ok_or("missing mono source label")?
            },
            start: ticks(Some(piece.start), &alternative.id)?,
            duration: ticks(Some(piece.duration), &alternative.id)?,
            source_in: ticks(piece.source_in, &alternative.id)?,
        });
    }
    Ok(pieces)
}

fn require_binding<'a>(
    group: &Multicam,
    edit: &'a MulticamUse,
    alternative: &MulticamAlternative,
    placements: &HashMap<&str, &graph::Placement>,
    claimed: &mut HashSet<&'a str>,
) -> Result<()> {
    let fail = || {
        format!(
            "{}: multicam leaf timing, effect or selector contradicts the selected angle",
            edit.id
        )
    };
    let from = ticks(edit.source_in, &edit.id)?;
    let end = from
        .checked_add(ticks(edit.duration, &edit.id)?)
        .ok_or_else(fail)?;
    if end > ticks(group.duration, &group.id)? || alternative.reason.is_some() {
        return Err(fail().into());
    }
    let record = ticks(edit.record_start, &edit.id)?;
    let mut leaves = edit.binding.placements.iter();
    let source_start = edit.source_in.ok_or_else(fail)?;
    let source_end = source_start
        .checked_add(edit.duration.ok_or_else(fail)?)
        .ok_or_else(fail)?;
    let first = alternative.pieces.partition_point(|piece| {
        piece
            .start
            .checked_add(piece.duration)
            .is_some_and(|end| end <= source_start)
    });
    for piece in alternative.pieces[first..]
        .iter()
        .take_while(|piece| piece.start < source_end)
    {
        let Some(item) = &piece.item else { continue };
        let start = ticks(Some(piece.start), &edit.id)?;
        let lo = start.max(from);
        let hi = start
            .checked_add(ticks(Some(piece.duration), &edit.id)?)
            .ok_or_else(fail)?
            .min(end);
        if hi <= lo {
            continue;
        }
        let id = leaves.next().ok_or_else(fail)?;
        let leaf = placements.get(id.as_str()).ok_or_else(fail)?;
        crate::prepare::check_plain(leaf)?;
        let lane = match edit.kind {
            StreamKind::Video => {
                leaf.video_track == Some(edit.track) && leaf.audio_tracks.is_empty()
            }
            StreamKind::Audio => {
                leaf.video_track.is_none()
                    && leaf.audio_tracks == [edit.track]
                    && piece
                        .stream
                        .is_some_and(|channel| leaf.audio_streams == [channel])
            }
        };
        if !claimed.insert(id)
            || !lane
            || &leaf.item != item
            || ticks(Some(leaf.record_start), id)?
                != record.checked_add(lo - from).ok_or_else(fail)?
            || ticks(Some(leaf.source_in), id)?
                != ticks(piece.source_in, &edit.id)?
                    .checked_add(lo - start)
                    .ok_or_else(fail)?
            || ticks(Some(leaf.duration), id)? != hi - lo
        {
            return Err(fail().into());
        }
    }
    if leaves.next().is_some() {
        return Err(fail().into());
    }
    Ok(())
}
