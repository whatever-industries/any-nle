//! Resolve graph/2 angle tracks and independently selected mono sources.
mod omissions;
mod picture_subset;
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
                && group.reason.as_deref() != Some(crate::source::MULTICAM_PARTIAL_PICTURE)
                && group.duration.is_none_or(|duration| frames_on(duration, sequence.rate).is_ok())
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

pub(super) fn prepare(
    mut graph: SourceGraph,
    sequence_index: usize,
    originals: &mut Originals<'_>,
    output: &Path,
    accept_losses: bool,
) -> Result<(PreparedProject, Vec<Loss>)> {
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
        || !sequence.disabled_placements.is_empty()
        || graph.sidecar(crate::generator::ADAPTER, &sequence.id).is_some()
    {
        return Err(
            fail("unsupported Resolve linked multicam timeline timing or structure").into(),
        );
    }
    let origin = crate::linked::check_origin(sequence)
        .map_err(|error| format!("{}: {error}", sequence.id))?;
    let mut uses: BTreeMap<&str, Vec<&MulticamUse>> = BTreeMap::new();
    let mut unresolved_picture = Vec::new();
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
            if edit.kind == StreamKind::Video {
                unresolved_picture.push((group.as_str(), edit.id.as_str()));
            }
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
    for (group, edit) in unresolved_picture {
        if definitions[group].reason.as_deref() == Some(crate::source::MULTICAM_PARTIAL_PICTURE) {
            return Err(format!("{edit}: unresolved selected picture in a partially bound Resolve Group").into());
        }
    }
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
        let partial = group.reason.as_deref() == Some(crate::source::MULTICAM_PARTIAL_PICTURE);
        if let Some(reason) = group.reason.as_deref().filter(|_| !partial) {
            return Err(format!("{id}: unqualified Resolve multicam definition: {reason}").into());
        }
        if group.rate != Some(sequence.rate) {
            return Err(invalid("Resolve multicam rate disagrees with its timeline").into());
        }
        if source.get("frame_size") != Some(&Value::Arr(vec![Value::Int(size.0), Value::Int(size.1)])) {
            return Err(invalid("Resolve multicam source canvas disagrees with its timeline").into());
        }
        let (duration, omitted_tail) = group_duration(group, sequence.rate)?;
        if omitted_tail {
            losses.push(Loss {
                object: id.into(),
                aspect: "multicam_fractional_tail".into(),
                reason: "Premiere uses the complete-frame Group extent. The saved fractional gap remains in graph/2.".into(),
            });
        }
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
        if partial {
            losses.extend(picture_subset::omitted_angles(&graph, group, source, picture, &edits)?);
        }
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
            require_binding(duration, edit, alternative, &placements, &mut claimed)?;
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

    let omissions = omissions::collect(&graph, sequence, &groups, &claimed, origin)?;
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
    let still_grades = still::grade_omissions(&graph, sequence, &claimed);
    let still_grade_owners: HashSet<_> = still_grades.iter().map(|loss| loss.object.as_str()).collect();
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
                    && !(loss.aspect == "grade" && still_grade_owners.contains(loss.object.as_str()))
                    && !omissions.replaces(loss, &sequence.id)
            })
            .cloned(),
    );
    losses.extend(still_grades);
    if !omissions.is_empty() && !accept_losses {
        return Err(omissions.approval_error(&sequence.id));
    }
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
    graph.sequences[0].texts.clear();
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
    let fit = fit_custom_canvas(&graph, size)?;
    let fitted_stills = still::prepare(&mut graph)?;
    let verified = bind_originals(&mut graph, originals, true)?;
    let mut prepared = premiere_adapter::prepare_resolve_originals(&graph, &Options::default())?;
    still::apply_fit(&mut prepared, fitted_stills)?;
    for (owner, source_size) in fit {
        let &(sequence, event) = prepared.ownership.get(&owner)
            .ok_or_else(|| format!("{owner}: Resolve input fitting lost its prepared edit"))?;
        if let Some(motion) = &mut prepared.input.sequences[sequence].events[event].transform {
            // Resolve applies the saved position to the fitted image. Premiere
            // Motion uses the original raster even with Scale to Frame Size.
            let fit = (size.0 as f64 / source_size.0 as f64)
                .min(size.1 as f64 / source_size.1 as f64);
            motion.position.0 *= fit;
            motion.position.1 *= fit;
        }
        prepared.input.fit_picture.insert((sequence, event));
    }
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
    omissions.check_prepared_end(&prepared.input)?;
    losses.extend(omissions.into_losses());
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
    verified.recheck()?;
    losses.extend(prepared.losses);
    Ok((staged, losses))
}

fn fit_custom_canvas(graph: &SourceGraph, canvas: (i64, i64)) -> Result<Vec<(String, (i64, i64))>> {
    use crate::input_scaling::{self, Sizing};
    use prproj::convert::{model::Fraction, writer::{preset, Report}};

    let sequence = &graph.sequences[0];
    let rate = Fraction::new(sequence.rate.num(), sequence.rate.den());
    if preset(&sequence.name, rate, canvas.0, canvas.1, &mut Report::default()).is_ok() {
        return Ok(Vec::new());
    }
    let items: HashMap<_, _> = graph.items.iter().map(|item| (item.id.as_str(), item)).collect();
    let media: HashMap<_, _> = graph.media.iter().map(|source| (source.id.as_str(), source)).collect();
    let sidecars = linked::SidecarLookup::Linear(graph);
    let mut fit = Vec::new();
    for edit in sequence.placements.iter().filter(|edit| edit.video_track.is_some()) {
        let source = items[edit.item.as_str()].media.iter()
            .map(|link| media[link.media.as_str()])
            .find(|source| source.kind == StreamKind::Video)
            .ok_or_else(|| format!("{}: missing picture for custom canvas", edit.id))?;
        let source_size = source.frame_size.filter(|&(w, h)| w > 0 && h > 0)
            .ok_or_else(|| format!("{}: missing linked picture raster for custom canvas", edit.id))?;
        if source_size == canvas || still::Format::from_codec(source.codec.as_deref()).is_some() {
            continue;
        }
        let sizing = input_scaling::prepare(sidecars, &sequence.id, &edit.id, true)
            .map_err(|error| format!("{}: {error}", edit.id))?;
        if sizing != Some(Sizing::Fit)
            || source.rate != Some(sequence.rate)
            || edit.transform.as_ref().is_some_and(|motion| !crate::video::fitted_motion(motion))
            || edit.speed.is_some() || edit.frame_map.is_some() || !edit.effects.is_empty()
        {
            return Err(format!("{}: custom canvas needs verified fitting and supported static picture controls", edit.id).into());
        }
        fit.push((edit.id.clone(), source_size));
    }
    Ok(fit)
}

fn ticks(time: Option<Rational>, owner: &str) -> Result<i64> {
    time.and_then(|time| time.to_units(prproj::build::time::TICKS_PER_SECOND))
        .ok_or_else(|| format!("{owner}: missing or inexact multicam time").into())
}

fn group_duration(group: &Multicam, rate: Rational) -> Result<(i64, bool)> {
    let fail = || format!("{}: unverified fractional multicam duration", group.id);
    let duration = group.duration.ok_or_else(fail)?;
    let count = i128::from(duration.num()) * i128::from(rate.num())
        / (i128::from(duration.den()) * i128::from(rate.den()));
    let whole = Rational::new(count * i128::from(rate.den()), i128::from(rate.num()))
        .filter(|time| *time > Rational::ZERO)
        .ok_or_else(fail)?;
    if whole == duration {
        return Ok((ticks(Some(duration), &group.id)?, false));
    }
    // Every qualified alternative must prove that the fractional tail is empty.
    if group.lanes.is_empty() || group.lanes.iter().any(|lane| {
        lane.alternatives.is_empty() || lane.alternatives.iter().any(|alternative| {
            alternative.reason.is_some() || !alternative.pieces.last().is_some_and(|piece| {
                piece.item.is_none() && piece.start <= whole
                    && piece.start.checked_add(piece.duration) == Some(duration)
            })
        })
    }) {
        return Err(fail().into());
    }
    Ok((ticks(Some(whole), &group.id)?, true))
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
        if picture && still::Format::from_codec(media[links[0].media.as_str()].codec.as_deref()).is_some() {
            return Err(format!("{}: linked stills inside multicam angles are unqualified", alternative.id).into());
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
    group_duration: i64,
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
    if end > group_duration || alternative.reason.is_some() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use graph::source::{MulticamLane, MulticamPiece};

    #[test]
    fn fractional_group_projection_requires_an_empty_tail_on_each_angle() {
        let source = MulticamPiece {
            start: Rational::ZERO,
            duration: Rational::integer(1),
            item: Some("source".into()),
            stream: Some(1),
            source_in: Some(Rational::ZERO),
        };
        let gap = MulticamPiece {
            start: Rational::integer(1),
            duration: Rational::new(1, 32).unwrap(),
            item: None,
            stream: None,
            source_in: None,
        };
        let mut group = Multicam {
            id: "group".into(),
            name: "Group".into(),
            rate: Some(Rational::integer(24)),
            duration: Some(Rational::new(33, 32).unwrap()),
            lanes: vec![MulticamLane {
                kind: StreamKind::Video,
                index: 1,
                selected: Some(0),
                alternatives: vec![MulticamAlternative {
                    id: "angle".into(),
                    pieces: vec![source, gap],
                    reason: None,
                }],
                reason: None,
            }],
            reason: None,
        };
        let rate = group.rate.unwrap();
        assert_eq!(group_duration(&group, rate).unwrap(), (TICKS_PER_SECOND, true));

        let tail = &mut group.lanes[0].alternatives[0].pieces[1];
        tail.item = Some("source".into());
        tail.stream = Some(1);
        tail.source_in = Some(Rational::integer(1));
        assert!(group_duration(&group, rate).is_err());
    }
}
