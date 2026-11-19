//! Internal Resolve-to-Premiere paths for linked media.
mod multicam;
use crate::{
    audio::{self, Volume},
    dissolve,
    linked::{
        self,
        mixer::{prepare_linked_track_pans, DirectMixer, StripProcessing},
        MediaSpec, Profile,
    },
    media::{check_wav_extent, media_digest},
    mixed_rate::{Cadence, ResolveTrim},
    prepare::{frames_on, MAX_EDITS_PER_LANE},
    retime::PictureSpeed,
    source::{
        mixer::{
            fader_ticks, neutral_empty_stereo, parse_track_mutes, DIRECT_PROFILE, FAIRLIGHT_ADAPTER, FAIRLIGHT_LOSS,
            MAIN_BUS_LEVEL, MAIN_BUS_LOSS, MUTED_AUDIO_TRACKS, TRACK_MUTE_LOSS,
        },
        ADAPTER, EDIT_MARKER_ADAPTER, EDIT_MARKER_LOSS, POOL_MARKER_ADAPTER,
    },
    timeline_marks::{self, TimelineMarks},
    video::{
        centered_crop_profile, rotation_axis_profile, validate_static_geometry,
        motion::{LinearMotion, NATIVE_LINEAR_KIND},
        roll::{LinearRoll, RESOLVE_KIND as LINEAR_ROLL_KIND},
    },
    Result, StreamSelector,
};
use graph::{
    source::{OriginalStream, SourceGraph, SourceItem, SourceMarker, SourceMedia, SourceSequence, TextKind},
    Loss, Placement, Rational, StreamKind, Value,
};
use prproj::{
    convert::{
        colors::LABEL_NAMES,
        model::{Options, TICKS_PER_SECOND},
        prepared::{PictureEffect, PreparedAudioCrossfade, PreparedAudioEdgeFade, PreparedAudioMixer,
            PreparedClip, PreparedEditLabel, PreparedPictureEffect, PreparedTimelineMarks, PreparedTrackLane,
            PreparedTrackState, PreparedVideoDissolve, SampleRepresentation},
        writer::{prepare_typed_project_with_marker_provenance, MarkerDataKey, MarkerProvenance},
    },
    RandomUuids,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub fn write(
    graph: SourceGraph,
    sequence_id: &str,
    ffprobe: &Path,
    output: &Path,
) -> Result<Vec<graph::Loss>> {
    write_with_options(graph, sequence_id, ffprobe, output, false)
}

pub fn write_with_options(
    mut graph: SourceGraph,
    sequence_id: &str,
    ffprobe: &Path,
    output: &Path,
    accept_losses: bool,
) -> Result<Vec<graph::Loss>> {
    graph.validate()?;
    let index = graph
        .sequences
        .iter()
        .position(|seq| seq.id == sequence_id)
        .ok_or("selected Resolve sequence is absent")?;
    let sequence = &graph.sequences[index];
    if let Some(edit) = sequence.disabled_placements.first() {
        return Err(format!("{}: disabled source edit has no qualified Resolve export mapping", edit.id).into());
    }
    if graph.multicam_uses.iter().any(|usage| usage.sequence == sequence_id) {
        return multicam::write(&graph, sequence, ffprobe, output, accept_losses);
    }
    let track_state = crate::track_state::TrackState::prepare(&graph, sequence)?;
    let (mut mixer, mapped_direct_mixer) = validate_sequence(&graph, sequence, track_state.is_some())?;
    let empty_stereo = if track_state.is_none()
        && sequence.tracks.iter().map(|track| (track.kind, track.index, track.level_db))
            .eq([(StreamKind::Video, 1, None), (StreamKind::Audio, 1, None)])
        && sequence.placements.iter().all(|edit| edit.audio_tracks.is_empty() && edit.grouped_audio.is_empty())
        && graph.sidecar(FAIRLIGHT_ADAPTER, sequence_id).is_some_and(neutral_empty_stereo)
    {
        crate::source::default_empty_stereo(&graph, sequence_id).map(str::to_owned)
    } else {
        None
    };
    let (timeline_marks, mut metadata_losses) = prepare_timeline_marks(&graph, sequence)?;
    let accounted_marks = timeline_marks.is_some() || !metadata_losses.is_empty();
    let labels = prepare_edit_labels(&graph, sequence, &mut metadata_losses)?;
    let handled_labels: HashSet<_> = labels.iter().map(|label| label.owner.as_str())
        .chain(metadata_losses.iter().filter(|loss| loss.aspect == "clip_label")
            .map(|loss| loss.object.as_str())).collect();
    let mut transitions = Vec::new();
    let native_transitions = dissolve::prepare_all_native(&graph, sequence)?;
    dissolve::require_single_per_lane(&native_transitions)?;
    for transition in native_transitions {
        let d = transition.prepared;
        if d.duration != dissolve::LEGACY_DURATION || d.style == dissolve::Style::Power {
            return Err(format!("Resolve transition {}: outside the qualified Premiere transition profile", d.id).into());
        }
        if d.audio() && mixer.mono_track_pans.is_some() {
            return Err(format!("Resolve transition {}: audio crossfade with track pan is unqualified", d.id).into());
        }
        if !d.audio() && sequence.rate == Rational::integer(24) && d.before != dissolve::LEGACY_DURATION / 2 {
            return Err(format!(
                "Resolve transition {}: 24 fps Premiere video dissolve requires centered alignment",
                d.id
            ).into());
        }
        let frame = Rational::new(sequence.rate.den().into(), sequence.rate.num().into())
            .and_then(|period| period.to_units(TICKS_PER_SECOND))
            .ok_or_else(|| format!("Resolve transition {}: no exact Premiere frame clock", d.id))?;
        transitions.push((d, transition.outgoing.to_owned(), transition.incoming.to_owned(), frame));
    }
    let used: HashSet<_> = sequence.placements.iter().map(|p| p.item.clone()).collect();
    let mut owned = used.clone();
    owned.insert(sequence_id.into());
    owned.extend(sequence.placements.iter().map(|p| p.id.clone()));
    owned.extend(sequence.texts.iter().map(|text| text.id.clone()));
    graph.items.retain(|item| used.contains(&item.id));
    let used_media: HashSet<_> = graph
        .items
        .iter()
        .flat_map(|item| item.media.iter().map(|link| link.media.clone()))
        .collect();
    owned.extend(used_media.iter().cloned());
    graph.sequences = vec![graph.sequences.swap_remove(index)];
    let (fades, mut destination_losses) = prepare_clip_controls(&mut graph)?;
    let selected_edit_marker = |loss: &Loss| {
        owned.contains(&loss.object) && graph.sidecar(EDIT_MARKER_ADAPTER, &loss.object).is_some()
    };
    let selected_marker_color = |loss: &Loss| {
        loss.aspect == "marker_color"
            && (loss.object == sequence_id
                || (used.contains(&loss.object)
                    && graph.sidecar(POOL_MARKER_ADAPTER, &loss.object).is_some())
                || selected_edit_marker(loss))
    };
    if let Some(loss) = graph
        .losses
        .iter()
        .find(|loss| {
            if selected_marker_color(loss) || (loss.aspect == EDIT_MARKER_LOSS && selected_edit_marker(loss)) {
                return false;
            }
            if (accounted_marks && TimelineMarks::recover(loss, sequence_id))
                || (handled_labels.contains(loss.object.as_str())
                    && crate::clip_label::ClipLabel::recover(loss))
                || (track_state.is_some() && crate::track_state::TrackState::recover(loss, sequence_id))
            {
                return false;
            }
            if transitions.iter().any(|(d, ..)| d.recover(loss, sequence_id)) {
                return false;
            }
            let control = (loss.aspect.as_str(), loss.reason.as_str());
            if empty_stereo.as_deref().is_some_and(|track|
                crate::source::is_empty_stereo_loss(loss, sequence_id, track)
                    || (loss.object == sequence_id && control == FAIRLIGHT_LOSS))
            {
                return false;
            }
            let recovered = loss.object == sequence_id
                && ((mixer.mix_level_db.is_some() && control == MAIN_BUS_LOSS)
                    || (!mixer.muted_tracks.is_empty() && control == TRACK_MUTE_LOSS)
                    || ((mixer.mono_track_pans.is_some() || mapped_direct_mixer) && control == FAIRLIGHT_LOSS));
            (loss.object == "project" || owned.contains(&loss.object)) && !recovered
        })
    {
        return Err(format!("Resolve source loss on {}: {}", loss.object, loss.reason).into());
    }
    metadata_losses.extend(graph.losses.iter().filter(|loss| {
        loss.aspect == EDIT_MARKER_LOSS && selected_edit_marker(loss)
    }).cloned());
    if !accept_losses && !metadata_losses.is_empty() {
        let details = metadata_losses.iter()
            .map(|loss| format!("{}: {}", loss.object, loss.reason))
            .collect::<Vec<_>>().join("\n");
        return Err(format!("Resolve metadata requires --accept-losses before conversion:\n{details}").into());
    }
    destination_losses.extend(metadata_losses);
    if let Some(track) = empty_stereo {
        destination_losses.push(Loss {
            object: track,
            aspect: "empty_stereo_track".into(),
            reason: "Resolve's empty default stereo track is omitted from the Premiere timeline".into(),
        });
    }
    destination_losses.extend(graph.losses.iter().filter(|loss| selected_marker_color(loss)).cloned());
    prepare_track_pan(&mut graph.sequences[0], &mut mixer, &mut destination_losses);
    graph.media.retain(|media| used_media.contains(&media.id));
    graph
        .bins
        .iter_mut()
        .for_each(|bin| bin.items.retain(|item| used.contains(item)));
    graph.multicams.clear();
    graph.multicam_uses.clear();
    graph.losses.clear();
    graph
        .sidecars
        .retain(|receipt| owned.contains(&receipt.object));
    let verified = verify_originals(&mut graph, ffprobe)?;
    let mut trim_losses = Vec::new();
    let picture = prepare_discrete_tracks(&mut graph, &verified, &mut trim_losses)?;
    if !accept_losses && !trim_losses.is_empty() {
        let details = trim_losses.iter().map(|loss| format!("{}: {}", loss.object, loss.reason))
            .collect::<Vec<_>>().join("\n");
        return Err(format!("Resolve frame hold trim changes require --accept-losses before conversion:\n{details}").into());
    }
    destination_losses.extend(trim_losses);

    let mut prepared = premiere_adapter::prepare_resolve_originals(&graph, &Options::default())?;
    if let Some(marks) = timeline_marks {
        prepared.input.timeline_marks.insert(0, marks);
    }
    for (owner, held_ticks) in picture.holds {
        let &edit = prepared.ownership.get(&owner)
            .ok_or_else(|| format!("{owner}: Resolve frame hold lost its prepared edit"))?;
        prepared.input.frame_holds.insert(edit, held_ticks);
    }
    if let Some(state) = track_state {
        let states = state.0.into_iter().enumerate().filter_map(|(lane, track)| {
            (!track.name.is_empty() || track.locked || track.disabled).then(|| PreparedTrackState {
                lane: if lane == 0 { PreparedTrackLane::Picture(1) } else { PreparedTrackLane::Audio(lane as i64) },
                name: (!track.name.is_empty()).then_some(track.name),
                locked: track.locked,
                picture_disabled: track.disabled,
                create_empty: false,
            })
        }).collect();
        prepared.input.track_states.insert(0, states);
    }
    for label in labels {
        let &(sequence, event) = prepared.ownership.get(&label.owner)
            .ok_or_else(|| format!("{}: Resolve label lost its prepared edit", label.owner))?;
        prepared.input.edit_labels.push(PreparedEditLabel {
            sequence, event, audio_track: label.audio_track, index: label.index,
        });
        if label.source != label.destination {
            destination_losses.push(Loss {
                object: label.owner,
                aspect: "clip_label_palette".into(),
                reason: format!("Resolve {} mapped to Premiere {}", label.source, label.destination),
            });
        }
    }
    for (owner, effect) in picture.motions {
        let &(sequence, event) = prepared.ownership.get(&owner)
            .ok_or_else(|| format!("{owner}: Resolve Linear motion lost its prepared edit"))?;
        prepared.input.picture_effects.push(PreparedPictureEffect {
            sequence,
            event,
            effect,
            reflection_before_motion: [false; 2],
        });
    }
    for (d, outgoing, incoming, frame) in transitions {
        let refuse = |reason| format!("Resolve transition {}: {reason}", d.id);
        let (Some(&(sequence, out)), Some(&(other_sequence, inc))) =
            (prepared.ownership.get(&outgoing), prepared.ownership.get(&incoming))
        else {
            return Err(refuse("missing prepared edit identity").into());
        };
        if sequence != other_sequence {
            return Err(refuse("endpoints resolved to different sequences").into());
        }
        let before_cut = d.before * frame;
        let duration = dissolve::LEGACY_DURATION * frame;
        if d.audio() {
            if fades.iter().any(|fade| fade.track == 1) {
                return Err(refuse("crossfade cannot share a lane with clip edge fades").into());
            }
            prepared.input.audio_crossfades.push(PreparedAudioCrossfade {
                sequence, track: 1, outgoing: out, incoming: inc, before_cut, duration,
            });
        } else {
            prepared.input.video_dissolves.push(PreparedVideoDissolve {
                sequence, outgoing: out, incoming: inc, before_cut, duration,
            });
            destination_losses.push(Loss {
                object: d.id,
                aspect: "transition_approximation".into(),
                reason: "Resolve Cross Dissolve mapped to editable Premiere Cross Dissolve (Legacy); blend sampling can differ".into(),
            });
        }
    }
    if mixer.mix_level_db.is_some() || !mixer.muted_tracks.is_empty() || mixer.mono_track_pans.is_some() {
        prepared.input.audio_mixers.insert(0, mixer);
    }
    for fade in fades {
        let &(sequence, event) = prepared
            .ownership
            .get(&fade.owner)
            .ok_or_else(|| format!("{}: Resolve clip fade lost its prepared edit", fade.owner))?;
        prepared.input.audio_edge_fades.push(PreparedAudioEdgeFade {
            sequence,
            event,
            track: fade.track,
            head: fade.head,
            tail: fade.tail,
        });
    }
    let mut custom_data = HashMap::new();
    let mut custom_losses = Vec::new();
    let mut original_colors = HashMap::new();
    let mut color_losses = Vec::new();
    let sequence = &graph.sequences[0];
    let timeline_data = graph.sidecar(ADAPTER, &sequence.id).and_then(|data| data.get("markers"));
    let owners = std::iter::once((&sequence.id, timeline_data, None))
        .chain(sequence.placements.iter().map(|edit|
            (&edit.id, graph.sidecar(EDIT_MARKER_ADAPTER, &edit.id), None)))
        .chain(graph.items.iter().map(|item|
            (&item.id, graph.sidecar(POOL_MARKER_ADAPTER, &item.id), Some(item))));
    for (owner, receipt, item) in owners {
        let Some(Value::Arr(rows)) = receipt else { continue };
        for row in rows {
            let (Some(Value::Int(frame)), Some(Value::Str(custom))) =
                (row.get("frame"), row.get("custom_data")) else { continue };
            let original_color = match row.get("color_bit") {
                Some(Value::Int(bit)) => u64::try_from(*bit).ok()
                    .filter(|&bit| crate::marker_palette::resolve_only_color(bit).is_some()),
                _ => None,
            };
            if custom.is_empty() && original_color.is_none() {
                continue;
            }
            let match_with_marker = if let Some(item) = item {
                source_marker_key(item, *frame, &prepared.input.clips)
            } else if owner == &sequence.id {
                sequence_marker_key(sequence, *frame, &prepared.input.sequences[0].markers)
            } else {
                edit_marker_key(sequence, owner, *frame, &prepared.ownership)
            };
            let key = match_with_marker.as_ref().map(|(key, _)| key.clone());
            if !custom.is_empty() {
                if key.as_ref().is_some_and(|key| custom_data.insert(key.clone(), custom.clone()).is_some()) {
                    return Err(format!("{owner}: duplicate marker custom-data claim").into());
                }
                custom_losses.push((key.clone(), owner, *frame));
            }
            if let Some(bit) = original_color {
                let expected = crate::marker_palette::resolve_only_color(bit);
                let key = match_with_marker.filter(|(_, marker)| marker.color.as_ref().is_some_and(|color|
                    color.palette == "resolve" && Some(color.name.as_str()) == expected))
                    .map(|(key, _)| key);
                if key.as_ref().is_some_and(|key| original_colors.insert(key.clone(), bit).is_some()) {
                    return Err(format!("{owner}: duplicate marker original-color claim").into());
                }
                color_losses.push((key, owner, *frame));
            }
        }
    }
    let output = prepare_typed_project_with_marker_provenance(
        &prepared.input, output, &mut RandomUuids::new(),
        MarkerProvenance { custom_data: &custom_data, original_colors: &original_colors },
    )?;
    for (key, owner, frame) in custom_losses {
        if key.is_none_or(|key| !output.has_marker_custom_data(&key)) {
            destination_losses.push(marker_custom_data_loss(owner, frame));
        }
    }
    for (key, owner, frame) in color_losses {
        if key.is_none_or(|key| !output.has_marker_original_color(&key)) {
            destination_losses.push(Loss {
                object: owner.into(),
                aspect: "marker_color".into(),
                reason: format!("{owner}/marker/{frame}: original Resolve color retained in source graph"),
            });
        }
    }
    for media in verified.values() {
        media.0.recheck()?;
    }
    output.publish()?;
    destination_losses.extend(prepared.losses);
    Ok(destination_losses)
}

fn sequence_marker_key<'a>(
    sequence: &'a SourceSequence,
    frame: i64,
    prepared: &[prproj::convert::model::Marker],
) -> Option<(MarkerDataKey, &'a SourceMarker)> {
    let mut markers = sequence.markers.iter().filter(|marker| {
        marker.owner == graph::MarkerOwner::Sequence && marker.track.is_none()
            && !marker.inherited && marker.binding.reason.is_none()
            && marker.start.and_then(|start| frames_on(start, sequence.rate).ok()) == Some(frame)
    });
    let marker = markers.next()?;
    let ticks = marker.start?.to_units(TICKS_PER_SECOND)?;
    if markers.next().is_some() {
        return None;
    }
    let mut candidates = prepared.iter().enumerate().filter(|(_, marker)|
        marker.owner == "sequence" && marker.start == ticks);
    let index = candidates.next()?.0;
    candidates.next().is_none().then_some((MarkerDataKey::Sequence(0, index), marker))
}

fn edit_marker_key<'a>(
    sequence: &'a SourceSequence,
    owner: &str,
    frame: i64,
    ownership: &HashMap<String, (usize, usize)>,
) -> Option<(MarkerDataKey, &'a SourceMarker)> {
    let mut markers = sequence.markers.iter().filter(|marker| {
        marker.owner == graph::MarkerOwner::Edit && !marker.inherited
            && marker.binding.reason.is_none() && marker.binding.placements.as_slice() == [owner]
            && marker.source.and_then(|source| frames_on(source, sequence.rate).ok()) == Some(frame)
    });
    let marker = markers.next()?;
    if markers.next().is_some() {
        return None;
    }
    let &(sequence_index, event) = ownership.get(owner)?;
    let key = MarkerDataKey::Edit(sequence_index, event, marker.source?.to_units(TICKS_PER_SECOND)?);
    Some((key, marker))
}

fn marker_custom_data_loss(owner: &str, frame: i64) -> Loss {
    Loss {
        object: owner.into(),
        aspect: "marker_custom_data".into(),
        reason: format!("{owner}/marker/{frame}: Resolve custom data retained in source graph"),
    }
}

fn source_marker_key<'a>(
    item: &'a SourceItem, frame: i64, clips: &[PreparedClip],
) -> Option<(MarkerDataKey, &'a SourceMarker)> {
    let clip = clips.iter().find(|clip| clip.id() == item.id)?;
    let PreparedClip::Original(clip) = clip else { return None };
    match (&clip.video, &clip.audio) {
        (Some(video), None) if item.rate.is_some_and(|rate|
            video.codec == "apch"
                && (video.rate.num, video.rate.den) == (rate.num(), rate.den())
                && (video.width, video.height) == (1920, 1080)) => {}
        (None, Some(audio)) if item.rate.is_none() && clip.container == "wav"
            && audio.source_start == 0 && audio.sample_rate == 48000 && matches!(audio.channels, 1 | 2)
            && matches!(audio.representation, SampleRepresentation::Pcm16 | SampleRepresentation::Pcm24) => {}
        _ => return None,
    }
    let mut markers = item.markers.iter().filter(|marker| {
        marker.owner == graph::MarkerOwner::Sequence && marker.track.is_none()
            && !marker.inherited && marker.binding.reason.is_none()
            && marker.owner_local == marker.start
            && item.rate.or(marker.owner_rate).filter(|&rate| crate::source::supported_marker_rate(rate))
                .and_then(|rate| frames_on(marker.start?, rate).ok()) == Some(frame)
    });
    let marker = markers.next()?;
    let ticks = marker.start?.to_units(TICKS_PER_SECOND)?;
    if markers.next().is_some() {
        return None;
    }
    let mut candidates = clip.metadata.markers.iter().enumerate().filter(|(_, marker)| marker.start == ticks);
    let (index, _) = candidates.next()?;
    candidates.next().is_none().then_some((MarkerDataKey::Source(item.id.clone(), index), marker))
}

fn prepare_timeline_marks(
    graph: &SourceGraph,
    sequence: &SourceSequence,
) -> Result<(Option<PreparedTimelineMarks>, Vec<Loss>)> {
    let Some(receipt) = graph.sidecar(ADAPTER, &sequence.id) else {
        return Ok((None, Vec::new()));
    };
    let Some(marks) = receipt.get(timeline_marks::FIELD) else {
        return Ok((None, Vec::new()));
    };
    let Value::Obj(fields) = marks else {
        return Err(format!("{}: malformed Resolve timeline mark receipt", sequence.id).into());
    };
    if fields.iter().any(|(name, _)| !timeline_marks::TAGS.iter().any(|(_, field)| name == field)) {
        return Err(format!("{}: unknown Resolve timeline mark field", sequence.id).into());
    }
    if let Some(Value::Int(length)) = receipt.get("record_length") {
        if let Ok(Some(marks)) = TimelineMarks::prepare(graph, &sequence.id, *length) {
            let [start, end, ..] = marks.frames();
            let invalid = || format!("{}: Resolve timeline marks exceed the exact Premiere tick grid", sequence.id);
            let frame_ticks = Rational::new(sequence.rate.den().into(), sequence.rate.num().into())
                .and_then(|period| period.to_units(TICKS_PER_SECOND))
                .ok_or_else(invalid)?;
            let in_ticks = start.map(|frame| frame.checked_mul(frame_ticks).ok_or_else(invalid)).transpose()?;
            let out_ticks = end.map(|frame| {
                frame.checked_add(1).and_then(|exclusive| exclusive.checked_mul(frame_ticks)).ok_or_else(invalid)
            }).transpose()?;
            return Ok((Some(PreparedTimelineMarks { in_ticks, out_ticks }), Vec::new()));
        }
    }
    let mut losses = Vec::new();
    for (_, field) in timeline_marks::TAGS {
        let Some(value) = marks.get(field) else {
            continue;
        };
        if matches!(value, Value::Str(text) if text.is_empty()) {
            continue;
        }
        let description = match value {
            Value::Str(frame) if frame.bytes().all(|b| b.is_ascii_digit()) => {
                format!("{field} at Resolve frame {frame}")
            }
            _ => format!("{field} with an unqualified saved value"),
        };
        losses.push(Loss {
            object: sequence.id.clone(),
            aspect: timeline_marks::LOSS.0.into(),
            reason: format!("{description} omitted from Premiere"),
        });
    }
    Ok((None, losses))
}

struct EditLabel {
    owner: String,
    audio_track: Option<i64>,
    index: u8,
    source: &'static str,
    destination: &'static str,
}

fn prepare_edit_labels(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    omissions: &mut Vec<Loss>,
) -> Result<Vec<EditLabel>> {
    let saved: HashMap<_, _> = graph.sidecars.iter()
        .filter(|sidecar| sidecar.adapter == crate::clip_label::ADAPTER)
        .map(|sidecar| (sidecar.object.as_str(), &sidecar.data))
        .collect();
    let mut labels = Vec::new();
    for edit in &sequence.placements {
        let Some(label) = saved.get(edit.id.as_str()) else {
            continue;
        };
        if let Some(Value::Int(code)) = label.get("saved_code") {
            if let Some((source, destination)) = crate::clip_label::ClipLabel::premiere(*code) {
                let index = LABEL_NAMES.iter().position(|&name| name == destination)
                    .ok_or("Resolve label mapping names an unknown Premiere palette entry")?;
                labels.push(EditLabel {
                    owner: edit.id.clone(),
                    audio_track: edit.audio_tracks.first().copied(),
                    index: index as u8,
                    source,
                    destination,
                });
                continue;
            }
        }
        let description = match label.get("saved_code") {
            Some(Value::Int(code)) => format!("Resolve clip label code {code}"),
            _ => "Resolve clip label with an unqualified saved value".into(),
        };
        omissions.push(Loss {
            object: edit.id.clone(),
            aspect: "clip_label".into(),
            reason: format!("{description} omitted from Premiere"),
        });
    }
    Ok(labels)
}

fn validate_sequence(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    has_track_state: bool,
) -> Result<(PreparedAudioMixer, bool)> {
    let sequence_id = sequence.id.as_str();
    let has_audio = sequence
        .placements
        .iter()
        .any(|p| !p.audio_tracks.is_empty());
    let has_linear_motion = sequence.placements.iter().any(|p| {
        p.effects.iter().any(|effect| effect.kind == NATIVE_LINEAR_KIND)
    });
    let qualified_clock = sequence.clock.as_ref().is_some_and(|clock| {
        clock.provenance.route == "Resolve archive clock"
            && clock.provenance.source_package == sequence.id
            && clock.provenance.timecode_track == "Sm2Sequence.MediaExtents"
            && clock.provenance.source_offset == 0
            && clock.provenance.source_rate == sequence.rate
            && clock.provenance.origin_count == clock.effective_count
            && clock.rate == sequence.rate
            && clock.effective_count < crate::DAY as u64
            && clock.display_base == 24
            && !clock.drop
            && sequence.start.is_some()
            && clock.exact_start() == sequence.start
    });
    let sequence_receipt = graph.sidecar(ADAPTER, sequence_id);
    let fairlight = graph.sidecar(FAIRLIGHT_ADAPTER, sequence_id);
    let mut mono_track_pans = prepare_linked_track_pans(fairlight, sequence_id)?
        .map(|pans| pans.map(|pan| f64::from(pan) / 10.0));
    let mut mapped_direct_mixer = false;
    if has_track_state && has_audio && fairlight.is_some_and(|r| r.get(DIRECT_PROFILE).is_some()) {
        // Resolve saves a separate Fairlight mirror when a track is renamed or
        // locked. Reuse the direct-mixer validation; only its existing
        // single-bus, static-pan equivalent is writable in Premiere.
        if let Some(direct) = DirectMixer::prepare(graph, sequence)? {
            if direct.buses != 1 || direct.output != [0; 2]
                || direct.pan_bypassed != [false; 2] || direct.soloed != [false; 2]
                || direct.processing != [StripProcessing::DEFAULT; 2]
                || direct.bus_processing != [StripProcessing::DEFAULT; 2]
                || direct.bus_pre_gain != [0; 2]
            {
                return Err(format!("{sequence_id}: unsupported Resolve track state with Fairlight routing or processing").into());
            }
            let pans = direct.pan.map(|pan| f64::from(pan) / 10.0);
            if mono_track_pans.is_some_and(|linked| linked != pans) {
                return Err(format!("{sequence_id}: Resolve track pan receipts disagree").into());
            }
            mono_track_pans = Some(pans);
        } else if mono_track_pans.is_some() {
            return Err(format!("{sequence_id}: Resolve track pan receipts disagree").into());
        }
        mapped_direct_mixer = true;
    }
    if !(sequence.rate == Rational::integer(24)
        || (has_audio || has_linear_motion) && (sequence.rate.num(), sequence.rate.den()) == (24000, 1001))
        || sequence.frame_size != Some((1920, 1080))
        || !qualified_clock
        || !sequence
            .tracks
            .iter()
            .filter(|track| track.kind == StreamKind::Video)
            .map(|track| track.index)
            .eq([1])
        || has_audio
            && !sequence
                .tracks
                .iter()
                .filter(|t| t.kind == StreamKind::Audio)
                .map(|t| t.index)
                .eq([1, 2])
        || !sequence.transitions.is_empty()
        || sequence.placements.is_empty()
        || sequence
            .texts
            .iter()
            .any(|text| text.kind != TextKind::Caption)
        || graph
            .sidecar(crate::generator::ADAPTER, sequence_id)
            .is_some()
        || graph.sidecar("resolve.nested_uses", sequence_id).is_some()
        || has_track_state && !has_audio
        || fairlight.is_some_and(|r| r.get(DIRECT_PROFILE).is_some())
            && mono_track_pans.is_none() && !mapped_direct_mixer
        || mono_track_pans.is_some() && !has_audio
    {
        return Err(format!(
            "{sequence_id}: unsupported Resolve-to-Premiere sequence clock, topology or controls"
        )
        .into());
    }
    let mix_level = sequence_receipt
        .and_then(|receipt| receipt.get(MAIN_BUS_LEVEL))
        .map(|value| {
            let unsupported = || format!("{sequence_id}: unsupported Resolve main bus gain");
            let db = match value {
                Value::Float(db) => *db,
                Value::Int(db) => *db as f64,
                _ => return Err(unsupported()),
            };
            if !has_audio || fader_ticks(db).is_none() {
                return Err(unsupported());
            }
            Ok(db)
        })
        .transpose()?;
    let mut muted_tracks = Vec::new();
    if let Some(value) = sequence_receipt.and_then(|receipt| receipt.get(MUTED_AUDIO_TRACKS)) {
        if !has_audio {
            return Err(format!("{sequence_id}: Resolve track mute requires mono audio lanes").into());
        }
        let mutes = parse_track_mutes(value).map_err(|error| format!("{sequence_id}: {error}"))?;
        muted_tracks.extend((1..=2).zip(mutes).filter_map(|(track, muted)| muted.then_some(track)));
    }
    for track in &sequence.tracks {
        if track.level_db.is_some_and(|db| {
            !has_audio
                || track.kind != StreamKind::Audio
                || !matches!(track.index, 1 | 2)
                || fader_ticks(db).is_none()
        }) {
            return Err(format!(
                "{sequence_id}: unsupported Resolve track gain on {:?} track {}",
                track.kind, track.index
            )
            .into());
        }
    }
    let mut lanes = [Vec::new(), Vec::new(), Vec::new()];
    for edit in &sequence.placements {
        let unsupported = || format!("{}: unsupported Resolve edit topology or controls", edit.id);
        let lane = match (
            edit.video_track,
            edit.audio_tracks.as_slice(),
            edit.audio_streams.as_slice(),
        ) {
            (Some(1), [], []) => 0,
            (None, [lane @ (1 | 2)], [1 | 2]) if has_audio => *lane as usize,
            _ => return Err(unsupported().into()),
        };
        if let Some(transform) = &edit.transform {
            let qualified_geometry = match (sequence.rate.num(), sequence.rate.den()) {
                (24000, 1001) => transform.rotation == 0.0 && validate_static_geometry(transform).is_ok(),
                (24, 1) => centered_crop_profile(transform)
                    || rotation_axis_profile(transform)
                    || (transform.position == (0.0, 0.0)
                        && transform.scale == (100.0, 100.0)
                        && transform.crop == [0.0; 4]
                        && validate_static_geometry(transform).is_ok()),
                _ => false,
            };
            if lane != 0
                || !qualified_geometry
                || edit.speed.is_some()
                || !edit.effects.is_empty()
            {
                return Err(format!("{}: unsupported Resolve static picture geometry or effect stack for Premiere", edit.id).into());
            }
        }
        let qualified_effects = match edit.effects.as_slice() {
            [] => true,
            [effect] if lane == 0 => match effect.kind.as_str() {
                NATIVE_LINEAR_KIND => (sequence.rate.num(), sequence.rate.den()) == (24000, 1001),
                LINEAR_ROLL_KIND => sequence.rate == Rational::integer(24) && edit.speed.is_none(),
                _ => false,
            },
            _ => false,
        };
        if !edit.grouped_audio.is_empty()
            || !qualified_effects
            || edit.frame_map.is_some() && (lane != 0 || edit.speed.is_some()
                || edit.transform.is_some() || !edit.effects.is_empty())
            || (has_audio || has_linear_motion) && edit.speed.is_some()
            || edit.gain_db != 0.0
            || edit.track_gains_db.iter().any(Option::is_some)
            || lane == 0
                && (edit.volume_db.is_some()
                    || !edit.volume_keys.is_empty()
                    || !edit.track_pans.is_empty())
            || graph.sidecar(crate::grade::ADAPTER, &edit.id).is_some()
            || graph.sidecar(crate::grade::CDL_ADAPTER, &edit.id).is_some()
            || graph
                .sidecar(crate::audio::UNQUALIFIED_FADE_ADAPTER, &edit.id)
                .is_some()
            || graph.sidecar("resolve.clip_fairlight", &edit.id).is_some()
            || lanes[lane].len() == MAX_EDITS_PER_LANE
        {
            return Err(unsupported().into());
        }
        let end = edit
            .record_start
            .checked_add(edit.duration)
            .ok_or_else(unsupported)?;
        lanes[lane].push((edit.record_start, end, edit.id.as_str()));
    }
    for (lane, edits) in lanes
        .iter_mut()
        .enumerate()
        .take(if has_audio { 3 } else { 1 })
    {
        if edits.is_empty() {
            return Err(format!("{sequence_id}: missing edits on Resolve lane {lane}").into());
        }
        edits.sort_unstable();
        if let Some(pair) = edits.windows(2).find(|w| w[0].1 > w[1].0) {
            return Err(format!("{}: overlapping edits on Resolve lane {lane}", pair[1].2).into());
        }
    }
    Ok((PreparedAudioMixer {
        mix_level_db: mix_level,
        muted_tracks,
        mono_track_pans: mono_track_pans.map(|pans| pans.map(Some)),
    }, mapped_direct_mixer))
}

fn prepare_track_pan(sequence: &mut SourceSequence, mixer: &mut PreparedAudioMixer, losses: &mut Vec<Loss>) {
    let Some(mut pans) = mixer.mono_track_pans else {
        return;
    };
    let mut combined = HashSet::new();
    for (lane, track_pan) in (1..=2).zip(&mut pans) {
        let Some(track) = *track_pan else { continue };
        let lane_has_clip_pan = sequence.placements.iter().any(|edit| {
            edit.audio_tracks == [lane] && edit.track_pans.first().copied().flatten()
                .is_some_and(|pan| pan != 0.0)
        });
        if !lane_has_clip_pan {
            if track != 0.0 && track.abs() != 100.0 {
                losses.push(Loss {
                    object: sequence.id.clone(),
                    aspect: "track_pan_law".into(),
                    reason: format!("A{lane}: Premiere preserves the pan value and applies its own pan law"),
                });
            }
            continue;
        }

        // A stereo input keeps clip pan editable. Resolve's summed pan is
        // written on every clip on this lane, including clips with neutral pan.
        *track_pan = None;
        if track == 0.0 {
            continue;
        }
        for edit in sequence.placements.iter_mut().filter(|edit| edit.audio_tracks == [lane]) {
            let clip = edit.track_pans.first().copied().flatten().unwrap_or(0.0);
            let effective = (track + clip).clamp(-100.0, 100.0);
            edit.track_pans = vec![Some(effective)];
            combined.insert(edit.id.clone());
            losses.push(Loss {
                object: edit.id.clone(),
                aspect: "combined_pan".into(),
                reason: format!(
                    "A{lane}: Resolve clip pan {clip} and track pan {track} become one editable Premiere clip pan {effective}; Premiere applies its own pan law"
                ),
            });
        }
    }
    losses.retain(|loss| loss.aspect != "clip_pan_law" || !combined.contains(&loss.object));
    mixer.mono_track_pans = pans.iter().any(Option::is_some).then_some(pans);
}

struct ClipFade {
    owner: String,
    track: i64,
    head: i64,
    tail: i64,
}

fn prepare_clip_controls(graph: &mut SourceGraph) -> Result<(Vec<ClipFade>, Vec<Loss>)> {
    let receipts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|r| r.adapter == ADAPTER)
        .map(|r| (r.object.as_str(), &r.data))
        .collect();
    let sequence = &graph.sequences[0];
    let frame_ticks = Rational::new(sequence.rate.den().into(), sequence.rate.num().into())
        .and_then(|frame| frame.to_units(TICKS_PER_SECOND))
        .ok_or("Resolve clip fades need an exact Premiere frame clock")?;
    let mut recovered = HashSet::new();
    let mut losses = Vec::new();
    let mut fades = Vec::new();
    for edit in &sequence.placements {
        let [track] = edit.audio_tracks.as_slice() else {
            continue;
        };
        let unsupported = || format!("{}: unsupported Resolve clip pan or envelope", edit.id);
        match edit.track_pans.as_slice() {
            [] => {}
            [Some(pan)] if (-100.0..=100.0).contains(pan) => {
                recovered.insert((edit.id.as_str(), audio::CLIP_PAN_LOSS));
                if *pan != 0.0 {
                    losses.push(Loss {
                        object: edit.id.clone(),
                        aspect: "clip_pan_law".into(),
                        reason: "Premiere retains the clip pan value and applies its own pan law"
                            .into(),
                    });
                }
            }
            _ => return Err(unsupported().into()),
        }
        let receipt = receipts.get(edit.id.as_str()).copied().filter(|data| {
            [
                "fade_shape",
                "fade_clock",
                "fade_in_frames",
                "fade_out_frames",
                "fade_in_fractional_frames",
                "fade_out_fractional_frames",
                "volume_key_clock",
            ]
            .iter()
            .any(|key| data.get(key).is_some())
        });
        if edit.volume_db.is_some_and(|db| !audio::VOLUME_DB.contains(&db)) {
            return Err(unsupported().into());
        }
        let Some(receipt) = receipt else {
            if !edit.volume_keys.is_empty() {
                return Err(unsupported().into());
            }
            continue;
        };
        let rate = sequence.rate;
        let length = frames_on(edit.duration, rate).map_err(|_| unsupported())?;
        let source_in = if edit.volume_keys.is_empty() {
            0
        } else {
            frames_on(edit.source_in, rate).map_err(|_| unsupported())?
        };
        let envelope = audio::prepare(edit, source_in, length, rate, Some(receipt))?
            .ok_or_else(unsupported)?;
        if let Some(loss) = envelope.recovered_loss() {
            recovered.insert((edit.id.as_str(), loss));
        }
        if matches!(envelope.volume, Some(Volume::Keys(_))) {
            losses.push(Loss {
                object: edit.id.clone(),
                aspect: "volume_interpolation".into(),
                reason: "Premiere keeps the editable key times and dB values; its Linear curve can differ from Resolve's linear-dB curve"
                    .into(),
            });
        }
        let Some(fade) = envelope.fades else {
            continue;
        };
        if fade.head_point.is_some() || fade.tail_point.is_some() {
            return Err(unsupported().into());
        }
        let [head, tail] = fade.ticks(length, frame_ticks).ok_or_else(unsupported)?;
        fades.push(ClipFade {
            owner: edit.id.clone(),
            track: *track,
            head,
            tail,
        });
    }
    graph.losses.retain(|loss| {
        !recovered.contains(&(
            loss.object.as_str(),
            (loss.aspect.as_str(), loss.reason.as_str()),
        ))
    });
    Ok((fades, losses))
}

type VerifiedFiles = HashMap<String, (linked::Media, Rational, i64)>;

fn verify_originals(graph: &mut SourceGraph, ffprobe: &Path) -> Result<VerifiedFiles> {
    let receipts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|r| r.adapter == ADAPTER)
        .map(|r| (r.object.as_str(), &r.data))
        .collect();
    let mut verified = VerifiedFiles::new();
    let cameras: std::collections::HashSet<_> = graph.media.iter().filter(|source| {
        source.kind == StreamKind::Video && source.codec.as_deref() == Some("avc1")
            && source.rate == Some(Rational::integer(24)) && source.frame_size == Some((4096, 2160))
    }).flat_map(|source| source.locations.clone()).collect();
    for source in &mut graph.media {
        let [locator] = source.locations.as_slice() else {
            return Err(format!("{}: Resolve original needs one saved locator", source.id).into());
        };
        let path = Path::new(locator);
        let file_error =
            |error: Box<dyn std::error::Error>| format!("{} ({locator}): {error}", source.id);
        if !verified.contains_key(locator) {
            let camera = cameras.contains(locator);
            let spec = MediaSpec {
                item_id: String::new(),
                bin_id: String::new(),
                path: path.to_owned(),
                sha256: if camera { String::new() } else { hex::encode(media_digest(path).map_err(file_error)?) },
                streams: vec![],
            };
            let binding = linked::probe_resolve_original(&spec, ffprobe, camera).map_err(file_error)?;
            if let Profile::Wav {
                bits,
                channels,
                samples,
            } = binding.0.profile
            {
                check_wav_extent(path, bits, channels, samples).map_err(file_error)?;
            }
            verified.insert(locator.clone(), binding);
        }
        let (media, time_base, duration_ts) = &verified[locator];
        let camera = media.profile.timecoded_camera_pcm();
        let (container, channel, channels, representation) = match (&media.profile, source.kind) {
            (Profile::Mov { .. }, StreamKind::Video) if camera => ("mov", None, None, None),
            (Profile::Mov { .. }, StreamKind::Audio) if camera => {
                let channel = match receipts.get(source.id.as_str())
                    .map(|r| (r.get("saved_cache_track"), r.get("saved_channel"))) {
                    Some((Some(Value::Int(0)), Some(Value::Int(channel @ (0 | 1)))))
                        if source.stream == Some(channel + 1) => *channel,
                    _ => return Err(format!("{}: unresolved embedded PCM physical selector", source.id).into()),
                };
                ("mov", Some(channel), Some(2), Some("pcm16be".into()))
            }
            (
                Profile::Mov {
                    video: crate::linked::MovieVideo::ProResHq,
                    aac: false,
                    ..
                },
                StreamKind::Video,
            ) if source.codec.as_deref() == Some("apch") => ("mov", None, None, None),
            (Profile::Wav { bits, channels, .. }, StreamKind::Audio)
                if source.codec.as_deref() == Some("pcm") =>
            {
                let receipt = receipts.get(source.id.as_str());
                let channel =
                    match receipt.map(|r| (r.get("saved_cache_track"), r.get("saved_channel"))) {
                        Some((Some(Value::Int(0)), Some(Value::Int(channel))))
                            if (0..*channels).contains(channel) =>
                        {
                            *channel
                        }
                        _ => {
                            return Err(
                                format!("{}: unresolved WAV physical selector", source.id).into()
                            )
                        }
                    };
                let representation = if *bits == 16 { "pcm16" } else { "pcm24" };
                (
                    "wav",
                    Some(channel),
                    Some(*channels),
                    Some(representation.into()),
                )
            }
            _ => {
                return Err(format!(
                    "{} ({locator}): expected verified ProRes HQ MOV or PCM16/PCM24 WAV",
                    source.id
                )
                .into())
            }
        };
        let (index, time_base, duration_ts) = if camera && source.kind == StreamKind::Audio {
            let time_base = Rational::new(1, 48_000).unwrap();
            (1, time_base, source.duration.to_units(48_000).ok_or("invalid embedded PCM extent")?)
        } else {
            (0, *time_base, *duration_ts)
        };
        let selector = StreamSelector {
            media_id: source.id.clone(),
            index,
            channel: channel.map(|c| c as usize),
        };
        linked::check_stream(source, &selector, media, time_base, duration_ts)
            .map_err(file_error)?;
        if camera && source.kind == StreamKind::Video {
            let expected = match media.profile {
                Profile::Mov { quicktime_timecode: Some(start), .. } => u64::try_from(start)?,
                _ => return Err("missing verified original MOV clock".into()),
            };
            let clock = source.clock.as_mut().filter(|clock| {
                clock.effective_count == expected && !clock.drop && clock.rate == Rational::integer(24)
                    && clock.exact_start() == source.start
            }).ok_or("saved Resolve camera timecode contradicts the verified MOV clock")?;
            clock.provenance.route = graph::source::VERIFIED_LINKED_MOV_TIMECODE.into();
        }
        let original = OriginalStream {
            saved_locators: source.locations.clone(),
            container: container.into(),
            stream_index: index as i64,
            channel_index: channel,
            channels,
            time_base,
            start_pts: 0,
            duration_ts,
            sample_representation: representation,
        };
        if source
            .original
            .as_ref()
            .is_some_and(|saved| saved != &original)
        {
            return Err(format!("{}: contradictory original stream facts", source.id).into());
        }
        source.original = Some(original);
    }
    Ok(verified)
}

enum PremiereSampling {
    FrameSampling,
    SourceIn(Rational),
    TimeRemapping,
}

fn verify_premiere_sampling(
    edit: &Placement,
    map: &[i64],
    source: &SourceMedia,
    record_rate: Rational,
    verified: &VerifiedFiles,
    timing: Option<&Value>,
) -> Result<PremiereSampling> {
    let refuse = || {
        format!("{}: Premiere mixed-rate picture requires a verified Frame Sampling map and picture-only ProRes HQ original or qualified timecoded AVC camera original", edit.id)
    };
    let [locator] = source.locations.as_slice() else {
        return Err(refuse().into());
    };
    let (media, ..) = verified.get(locator).ok_or_else(refuse)?;
    let Profile::Mov { rate, frames, .. } = &media.profile else {
        return Err(refuse().into());
    };
    let prores = matches!(&media.profile, Profile::Mov {
        video: crate::linked::MovieVideo::ProResHq, aac: false, pcm: None,
        quicktime_timecode: None | Some(0), avid_source_timecode: None, raster: (1920, 1080), ..
    });
    if !prores && !media.profile.timecoded_camera_pcm() {
        return Err(refuse().into());
    }
    let cadence = Cadence::for_resolve_source(*rate, record_rate).ok_or_else(refuse)?;
    let length = frames_on(edit.duration, record_rate).map_err(|_| refuse())?;
    if source.rate != Some(*rate) {
        return Err(refuse().into());
    }
    if let Ok(source_in) = frames_on(edit.source_in, record_rate) {
        if cadence.agrees(map, source_in, length)
            && cadence.last_frame(source_in, length).is_some_and(|last| last < *frames)
        {
            return Ok(PremiereSampling::FrameSampling);
        }
    }
    if map.last().is_some_and(|last| *last < *frames) {
        if let Some(in_point) = cadence.premiere_default_in(edit.source_in, map, length) {
            return Ok(PremiereSampling::SourceIn(in_point));
        }
        if prores && matches!(cadence, Cadence::DropFrames) && (2..=16_384).contains(&length) {
            if let Some(trim) = native_conform_receipt(timing, &edit.id, cadence, record_rate) {
                if trim.source_in() == edit.source_in && trim.agrees(map, length) {
                    return Ok(PremiereSampling::TimeRemapping);
                }
            }
        }
    }
    Err(refuse().into())
}

fn native_conform_receipt(
    timing: Option<&Value>,
    edit: &str,
    cadence: Cadence,
    record_rate: Rational,
) -> Option<crate::mixed_rate::DefaultConform> {
    use crate::nests::string;
    let Value::Arr(rows) = timing? else { return None };
    let mut rows = rows.iter().filter(|row| {
        matches!(row.get("occurrence"), Some(Value::Str(id)) if id == edit)
    });
    let row = rows.next()?;
    if rows.next().is_some()
        || Rational::parse(string(row, "source_rate")?)? != cadence.source_rate()
        || Rational::parse(string(row, "record_rate")?)? != record_rate
    {
        return None;
    }
    match cadence.resolve_trim(string(row, "saved_in")?, true)? {
        ResolveTrim::DefaultConform(trim) => Some(trim),
        ResolveTrim::Adjusted(..) => None,
    }
}

fn prepare_frame_hold(edit: &Placement, map: &[i64], rate: Rational, has_audio: bool) -> Result<i64> {
    let refuse = || format!("{}: Premiere frame hold requires a constant map at source In on linked picture-only 1080p24 ProRes HQ", edit.id);
    if rate != Rational::integer(24) || has_audio || edit.video_track != Some(1) {
        return Err(refuse().into());
    }
    let source_in = frames_on(edit.source_in, rate).map_err(|_| refuse())?;
    let length = frames_on(edit.duration, rate).map_err(|_| refuse())?;
    if crate::retime::hold_count(length) != Some(map.len())
        || map.iter().any(|&frame| frame != source_in)
    {
        return Err(refuse().into());
    }
    edit.source_in.to_units(TICKS_PER_SECOND).ok_or_else(|| refuse().into())
}

#[derive(Default)]
struct PictureControls {
    holds: HashMap<String, i64>,
    motions: Vec<(String, PictureEffect)>,
}

fn prepare_discrete_tracks(
    graph: &mut SourceGraph,
    verified: &VerifiedFiles,
    trim_losses: &mut Vec<Loss>,
) -> Result<PictureControls> {
    let media: HashMap<_, _> = graph.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut routes = HashMap::new();
    for item in &mut graph.items {
        let invalid = || format!("{}: unsupported Resolve original master layout", item.id);
        let mut streams = item.media.iter().map(|link| media[link.media.as_str()]);
        let first = streams.next().ok_or_else(invalid)?;
        if item
            .media
            .iter()
            .any(|link| link.source_offset != Some(Rational::ZERO) || link.gain_db != 0.0)
        {
            return Err(format!(
                "{}: unresolved or nonzero item-to-file offset, or master gain",
                item.id
            )
            .into());
        }
        match first.kind {
            StreamKind::Video if item.media.len() == 1 && item.stream_groups.is_empty() => {}
            StreamKind::Video if verified.get(&first.locations[0]).is_some_and(|(media, ..)| {
                media.profile.timecoded_camera_pcm()
            }) => {
                if item.media.len() != 3 || !item.stream_groups.is_empty()
                    || !streams.enumerate().all(|(index, source)| {
                        source.kind == StreamKind::Audio && source.locations == first.locations
                            && source.stream == Some(index as i64 + 1)
                    })
                {
                    return Err(invalid().into());
                }
            }
            StreamKind::Audio => {
                let channels = first.original.as_ref().and_then(|o| o.channels);
                if channels != Some(item.media.len() as i64)
                    || streams
                        .any(|s| s.kind != StreamKind::Audio || s.locations != first.locations)
                    || !(item.stream_groups.is_empty() || item.stream_groups == [vec![1, 2]])
                {
                    return Err(invalid().into());
                }
                // The source pool may group L/R as stereo. Use separate mono
                // source views in Premiere for timeline edits and fresh insertion.
                item.stream_groups.clear();
            }
            _ => return Err(invalid().into()),
        }
        routes.insert(item.id.as_str(), item.media.as_slice());
    }
    let timing = graph.sidecars.iter()
        .find(|receipt| receipt.adapter == "resolve.timing" && receipt.object == graph.sequences[0].id)
        .map(|receipt| &receipt.data);
    let input_fit: std::collections::HashSet<_> = graph.sidecars.iter()
        .filter(|receipt| receipt.adapter == "resolve.input_scaling"
            && matches!(&receipt.data, Value::Str(mode) if mode == "scale_to_fit"))
        .map(|receipt| receipt.object.as_str())
        .collect();
    let sequence = &mut graph.sequences[0];
    let has_audio = sequence.placements.iter().any(|edit| !edit.audio_tracks.is_empty());
    let mut picture_controls = PictureControls::default();
    for edit in &mut sequence.placements {
        let invalid = || {
            format!(
                "{}: unsupported source clock, channel or playable range",
                edit.id
            )
        };
        let (kind, time_grid) = if edit.video_track.is_some() {
            (StreamKind::Video, sequence.rate)
        } else {
            (StreamKind::Audio, Rational::integer(48_000))
        };
        let source = routes[edit.item.as_str()]
            .iter()
            .map(|link| media[link.media.as_str()])
            .find(|m| {
                m.kind == kind
                    && (kind == StreamKind::Video
                        || m.stream == edit.audio_streams.first().copied())
            })
            .ok_or_else(invalid)?;
        let mixed = kind == StreamKind::Video && source.rate != Some(sequence.rate);
        let camera_picture = mixed && verified.get(&source.locations[0])
            .is_some_and(|(media, ..)| media.profile.timecoded_camera_pcm());
        let mut hold = false;
        let mut use_premiere_ticks = false;
        if let Some(map) = edit.frame_map.take() {
            if mixed {
                // Prefer ordinary Frame Sampling when its saved clocks can
                // reproduce the whole map.
                match verify_premiere_sampling(edit, &map, source, sequence.rate, verified, timing)? {
                    PremiereSampling::FrameSampling => {}
                    PremiereSampling::SourceIn(in_point) => {
                        edit.source_in = in_point;
                        use_premiere_ticks = true;
                    }
                    PremiereSampling::TimeRemapping => {
                        edit.frame_map = Some(map);
                        use_premiere_ticks = true;
                    }
                }
            } else {
                let ticks = prepare_frame_hold(edit, &map, sequence.rate, has_audio)?;
                picture_controls.holds.insert(edit.id.clone(), ticks);
                hold = true;
            }
        } else if mixed {
            return Err(invalid().into());
        }
        let source_duration = edit
            .speed
            .as_ref()
            .map_or(edit.duration, |speed| speed.source_duration);
        let playable_duration = if let Some(speed) = edit.speed.as_ref().filter(|speed| {
            kind == StreamKind::Video && speed.ratio == (2, 1)
        }) {
            let length = frames_on(edit.duration, time_grid)?;
            if time_grid != Rational::integer(24)
                || PictureSpeed::Half.graph(length).as_ref() != Some(speed)
            {
                return Err(invalid().into());
            }
            Rational::new(PictureSpeed::Half.sampled_source_frames(length)?.into(), 24)
                .ok_or_else(invalid)?
        } else {
            source_duration
        };
        let end = edit
            .source_in
            .checked_add(playable_duration)
            .ok_or_else(invalid)?;
        if use_premiere_ticks {
            if edit.source_in.to_units(TICKS_PER_SECOND).is_none()
                || end.to_units(TICKS_PER_SECOND).is_none()
            {
                return Err(invalid().into());
            }
        } else if frames_on(edit.source_in, time_grid).is_err()
            || frames_on(end, time_grid).is_err()
        {
            return Err(invalid().into());
        }
        if end > source.duration {
            if !hold {
                return Err(invalid().into());
            }
            // Premiere needs an underlying playable edit for the full hold.
            // Keep its window as close to the saved source In as possible.
            let last_in = source.duration.checked_sub(edit.duration)
                .filter(|value| !value.is_negative())
                .ok_or_else(|| format!("{}: Premiere frame hold is longer than its playable original", edit.id))?;
            let new_in = frames_on(last_in, time_grid).map_err(|_| invalid())?;
            let held = frames_on(edit.source_in, time_grid).map_err(|_| invalid())?;
            trim_losses.push(Loss {
                object: edit.id.clone(),
                aspect: "frame_hold_source_in".into(),
                reason: format!("Premiere holds source frame {held} and uses frame {new_in} as the underlying source In to keep the edit within the original."),
            });
            edit.source_in = last_in;
        }
        if camera_picture {
            if !input_fit.contains(edit.id.as_str()) || sequence.frame_size != Some((1920, 1080)) {
                return Err(format!("{}: unresolved camera input scaling", edit.id).into());
            }
            let (width, height) = source.frame_size.ok_or_else(invalid)?;
            let scale = (1920.0 / width as f64).min(1080.0 / height as f64) * 100.0;
            edit.transform = Some(graph::Transform {
                position: (0.0, 0.0), scale: (scale, scale), rotation: 0.0,
                anchor: (0.0, 0.0), crop: [0.0; 4],
            });
        }
        if let [effect] = edit.effects.as_slice() {
            let refuse = || format!("{}: unsupported Resolve Linear picture controls", edit.id);
            if kind != StreamKind::Video || edit.transform.is_some() || edit.speed.is_some() {
                return Err(refuse().into());
            }
            let source_in = frames_on(edit.source_in, sequence.rate).map_err(|_| refuse())?;
            let length = frames_on(edit.duration, sequence.rate).map_err(|_| refuse())?;
            let available = frames_on(source.duration, sequence.rate).map_err(|_| refuse())?;
            let picture = match effect.kind.as_str() {
                NATIVE_LINEAR_KIND if (sequence.rate.num(), sequence.rate.den()) == (24000, 1001) => {
                    let (base, picture) = LinearMotion::prepare_premiere(
                        effect, source_in, length, available, TICKS_PER_SECOND / 24000 * 1001,
                    ).map_err(|error| format!("{}: {error}", refuse()))?;
                    edit.transform = Some(base);
                    picture
                }
                LINEAR_ROLL_KIND if sequence.rate == Rational::integer(24) => {
                    LinearRoll::from_graph(effect, source_in, length, available)
                        .and_then(|roll| roll.prepare_premiere())
                        .map_err(|error| format!("{}: {error}", refuse()))?
                }
                _ => return Err(refuse().into()),
            };
            edit.effects.clear();
            picture_controls.motions.push((edit.id.clone(), picture));
        }
    }
    Ok(picture_controls)
}
