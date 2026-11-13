//! Internal Resolve-to-Premiere path for linked ProRes and discrete mono PCM.
use crate::{
    audio::{self, Volume},
    dissolve,
    linked::{
        self,
        mixer::{neutral_when_present, StripProcessing, PROCESSING_RECEIPT},
        MediaSpec, Profile,
    },
    media::{check_wav_extent, media_digest},
    prepare::{frames_on, MAX_EDITS_PER_LANE},
    source::{
        mixer::{
            fader_ticks, parse_track_mutes, DIRECT_PROFILE, FAIRLIGHT_ADAPTER, FAIRLIGHT_LOSS,
            LINKED_TRACK_PANS, MAIN_BUS_LEVEL, MAIN_BUS_LOSS, MUTED_AUDIO_TRACKS, TRACK_MUTE_LOSS,
        },
        ADAPTER,
    },
    Result, StreamSelector,
};
use graph::{
    source::{OriginalStream, SourceGraph, SourceSequence, TextKind},
    Loss, Rational, StreamKind, Value,
};
use prproj::{
    convert::{
        model::{Options, TICKS_PER_SECOND},
        prepared::{PictureEffect, PreparedAudioCrossfade, PreparedAudioEdgeFade, PreparedAudioMixer,
            PreparedPictureEffect, PreparedVideoDissolve},
        writer::prepare_typed_project,
    },
    RandomUuids,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub fn write(
    mut graph: SourceGraph,
    sequence_id: &str,
    ffprobe: &Path,
    output: &Path,
) -> Result<Vec<graph::Loss>> {
    graph.validate()?;
    let index = graph
        .sequences
        .iter()
        .position(|seq| seq.id == sequence_id)
        .ok_or("selected Resolve sequence is absent")?;
    let sequence = &graph.sequences[index];
    let mut mixer = validate_sequence(&graph, sequence)?;
    let transition = dissolve::prepare_native(&graph, sequence)?
        .map(|fade| {
            let d = fade.prepared;
            if d.audio && mixer.mono_track_pans.is_some() {
                return Err(format!("Resolve transition {}: audio crossfade with track pan is unqualified", d.id));
            }
            if !d.audio && (sequence.rate.num(), sequence.rate.den()) != (24000, 1001) {
                return Err(format!("Resolve transition {}: Premiere video dissolve requires the qualified 23.976 profile", d.id));
            }
            let frame = Rational::new(sequence.rate.den().into(), sequence.rate.num().into())
                .and_then(|period| period.to_units(TICKS_PER_SECOND))
                .ok_or_else(|| format!("Resolve transition {}: no exact Premiere frame clock", d.id))?;
            Ok::<_, String>((d, fade.outgoing.to_owned(), fade.incoming.to_owned(), frame))
        }).transpose()?;
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
    if let Some(loss) = graph
        .losses
        .iter()
        .find(|loss| {
            if transition.as_ref().is_some_and(|(d, ..)| d.recover(loss, sequence_id)) {
                return false;
            }
            let control = (loss.aspect.as_str(), loss.reason.as_str());
            let recovered = loss.object == sequence_id
                && ((mixer.mix_level_db.is_some() && control == MAIN_BUS_LOSS)
                    || (!mixer.muted_tracks.is_empty() && control == TRACK_MUTE_LOSS)
                    || (mixer.mono_track_pans.is_some() && control == FAIRLIGHT_LOSS));
            (loss.object == "project" || owned.contains(&loss.object)) && !recovered
        })
    {
        return Err(format!("Resolve source loss on {}: {}", loss.object, loss.reason).into());
    }
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
    let motions = prepare_discrete_tracks(&mut graph)?;

    let mut prepared = premiere_adapter::prepare_resolve_originals(&graph, &Options::default())?;
    for (owner, effect) in motions {
        let &(sequence, event) = prepared.ownership.get(&owner)
            .ok_or_else(|| format!("{owner}: Resolve Linear motion lost its prepared edit"))?;
        prepared.input.picture_effects.push(PreparedPictureEffect {
            sequence,
            event,
            effect,
            reflection_before_motion: [false; 2],
        });
    }
    if let Some((d, outgoing, incoming, frame)) = transition {
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
        let duration = dissolve::DURATION * frame;
        if d.audio {
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
    let output =
        prepare_typed_project(&prepared.input, output, &mut RandomUuids::new(), &[], false)?;
    for media in verified.values() {
        media.0.recheck()?;
    }
    output.publish()?;
    destination_losses.extend(prepared.losses);
    Ok(destination_losses)
}

fn validate_sequence(graph: &SourceGraph, sequence: &SourceSequence) -> Result<PreparedAudioMixer> {
    let sequence_id = sequence.id.as_str();
    let has_audio = sequence
        .placements
        .iter()
        .any(|p| !p.audio_tracks.is_empty());
    let has_linear_motion = sequence.placements.iter().any(|p| {
        p.effects.iter().any(|effect| effect.kind == crate::video::motion::NATIVE_LINEAR_KIND)
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
    let mono_track_pans = prepare_mono_track_pans(fairlight, sequence_id)?;
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
        || graph
            .sidecar(crate::track_state::ADAPTER, sequence_id)
            .is_some()
        || fairlight.is_some_and(|r| r.get(DIRECT_PROFILE).is_some()) && mono_track_pans.is_none()
        || mono_track_pans.is_some() && !has_audio
        || graph
            .multicam_uses
            .iter()
            .any(|usage| usage.sequence == sequence_id)
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
            if lane != 0
                || (sequence.rate.num(), sequence.rate.den()) != (24000, 1001)
                || edit.speed.is_some()
                || crate::video::validate_static_geometry(transform).is_err()
            {
                return Err(format!("{}: unsupported Resolve static position, scale or crop", edit.id).into());
            }
        }
        let qualified_effects = match edit.effects.as_slice() {
            [] => true,
            [effect] => lane == 0 && effect.kind == crate::video::motion::NATIVE_LINEAR_KIND,
            _ => false,
        };
        if !edit.grouped_audio.is_empty()
            || !qualified_effects
            || edit.frame_map.is_some()
            || (has_audio || has_linear_motion) && edit.speed.is_some()
            || edit.gain_db != 0.0
            || edit.track_gains_db.iter().any(Option::is_some)
            || lane == 0
                && (edit.volume_db.is_some()
                    || !edit.volume_keys.is_empty()
                    || !edit.track_pans.is_empty())
            || graph.sidecar(crate::grade::ADAPTER, &edit.id).is_some()
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
    Ok(PreparedAudioMixer {
        mix_level_db: mix_level,
        muted_tracks,
        mono_track_pans: mono_track_pans.map(|pans| pans.map(Some)),
    })
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

fn prepare_mono_track_pans(fairlight: Option<&Value>, sequence_id: &str) -> Result<Option<[f64; 2]>> {
    let Some(data) = fairlight else {
        return Ok(None);
    };
    let Some(saved_pans) = data.get(LINKED_TRACK_PANS) else {
        return Ok(None);
    };
    let unsupported = || format!("{sequence_id}: unsupported Resolve track pan or routing");
    let (Value::Arr(pans), Some(Value::Arr(tracks)), Some(Value::Arr(buses))) =
        (saved_pans, data.get("tracks"), data.get("buses"))
    else {
        return Err(unsupported().into());
    };
    let ([Value::Int(left), Value::Int(right)], [first, second], [bus]) =
        (pans.as_slice(), tracks.as_slice(), buses.as_slice())
    else {
        return Err(unsupported().into());
    };
    let row_value = |row: &Value, key, value| row.get(key) == Some(&Value::Int(value));
    if !row_value(bus, "index", 0)
        || !row_value(bus, "strip_id", 3)
        || !row_value(bus, "channels", 2)
        || !row_value(bus, "mute_code", 0)
        || !row_value(bus, "output_connected_code", 0)
        || !row_value(bus, "output_bus_index", -1)
        || !neutral_when_present(bus, &["raw_pan", "raw_pan_bypass", "raw_solo",
            "pre_dynamics_gain_tenths_db", "effects_in_code"])
        || StripProcessing::from_bus_receipt(bus).ok() != Some(StripProcessing::DEFAULT)
        || data.get("main_bus") != Some(bus)
        || data.get(PROCESSING_RECEIPT).is_some()
        || data.get(DIRECT_PROFILE).is_some_and(|profile| profile != &Value::Int(1))
        || [(*left, first), (*right, second)].into_iter().enumerate().any(|(index, (pan, row))| {
            !(-1000..=1000).contains(&pan)
                || !row_value(row, "track", index as i64 + 1)
                || !row_value(row, "strip_id", index as i64)
                || !row_value(row, "channel_index", 0)
                || !row_value(row, "pan_tenths_percent", pan)
                || !row_value(row, "pan_bypass_code", 0)
                || !row_value(row, "output_connected_code", 1)
                || !row_value(row, "output_bus_index", 0)
                || !neutral_when_present(row, &["solo_code", "pre_dynamics_gain_tenths_db",
                    "effects_in_code"])
                || row.get("unresolved_channel_fields").is_some_and(|fields| {
                    !neutral_when_present(fields, &["0x011_second"])
                })
                || !StripProcessing::saved_rows_are_default(row)
        })
    {
        return Err(unsupported().into());
    }
    Ok(Some([*left as f64 / 10.0, *right as f64 / 10.0]))
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
        let (head, tail) = if receipt.get("fade_in_fractional_frames").is_some()
            || receipt.get("fade_out_fractional_frames").is_some()
        {
            if !edit.volume_keys.is_empty() {
                return Err(unsupported().into());
            }
            let [head, tail] =
                fractional_fade_ticks(receipt, length, frame_ticks).ok_or_else(unsupported)?;
            recovered.insert((edit.id.as_str(), audio::FRACTIONAL_FADE_LOSS));
            (head, tail)
        } else {
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
            (
                fade.head.checked_mul(frame_ticks).ok_or_else(unsupported)?,
                fade.tail.checked_mul(frame_ticks).ok_or_else(unsupported)?,
            )
        };
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

fn fractional_fade_ticks(data: &Value, length: i64, frame_ticks: i64) -> Option<[i64; 2]> {
    let Value::Obj(fields) = data else {
        return None;
    };
    let (
        Some(Value::Str(clock)),
        Some(Value::Str(shape)),
        Some(Value::Float(head)),
        Some(Value::Float(tail)),
    ) = (
        data.get("fade_clock"),
        data.get("fade_shape"),
        data.get("fade_in_fractional_frames"),
        data.get("fade_out_fractional_frames"),
    )
    else {
        return None;
    };
    if fields.len() != 4
        || clock != "clip_edges"
        || shape != "linear_amplitude"
        || !head.is_finite()
        || !tail.is_finite()
        || *head < 0.0
        || *tail < 0.0
        || head + tail > length as f64
        || (head.fract() == 0.0 && tail.fract() == 0.0)
    {
        return None;
    }
    let ticks = |frames: f64| {
        let ticks = (frames * frame_ticks as f64).round();
        // Bound the float-to-tick quantization to exact integer f64 values.
        (ticks <= (1u64 << 53) as f64 && (frames == 0.0 || ticks >= 1.0)).then_some(ticks as i64)
    };
    let result = [ticks(*head)?, ticks(*tail)?];
    (result[0].checked_add(result[1])? <= length.checked_mul(frame_ticks)?).then_some(result)
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
    for source in &mut graph.media {
        let [locator] = source.locations.as_slice() else {
            return Err(format!("{}: Resolve original needs one saved locator", source.id).into());
        };
        let path = Path::new(locator);
        let file_error =
            |error: Box<dyn std::error::Error>| format!("{} ({locator}): {error}", source.id);
        if !verified.contains_key(locator) {
            let spec = MediaSpec {
                item_id: String::new(),
                bin_id: String::new(),
                path: path.to_owned(),
                sha256: hex::encode(media_digest(path).map_err(file_error)?),
                streams: vec![],
            };
            let binding = linked::probe(&spec, ffprobe, String::new(), 0).map_err(file_error)?;
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
        let (container, channel, channels, representation) = match (&media.profile, source.kind) {
            (
                Profile::Mov {
                    h264: false,
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
        let selector = StreamSelector {
            media_id: source.id.clone(),
            index: 0,
            channel: channel.map(|c| c as usize),
        };
        linked::check_stream(source, &selector, media, *time_base, *duration_ts)
            .map_err(file_error)?;
        let original = OriginalStream {
            saved_locators: source.locations.clone(),
            container: container.into(),
            stream_index: 0,
            channel_index: channel,
            channels,
            time_base: *time_base,
            start_pts: 0,
            duration_ts: *duration_ts,
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

fn prepare_discrete_tracks(graph: &mut SourceGraph) -> Result<Vec<(String, PictureEffect)>> {
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
    let sequence = &mut graph.sequences[0];
    let mut motions = Vec::new();
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
        let source_duration = edit
            .speed
            .as_ref()
            .map_or(edit.duration, |speed| speed.source_duration);
        let end = edit
            .source_in
            .checked_add(source_duration)
            .ok_or_else(invalid)?;
        if kind == StreamKind::Video && source.rate != Some(sequence.rate)
            || frames_on(edit.source_in, time_grid).is_err()
            || frames_on(end, time_grid).is_err()
            || end > source.duration
        {
            return Err(invalid().into());
        }
        if let [effect] = edit.effects.as_slice() {
            let refuse = || format!("{}: unsupported Resolve Linear position or scale", edit.id);
            if kind != StreamKind::Video
                || (sequence.rate.num(), sequence.rate.den()) != (24000, 1001)
                || edit.transform.is_some()
                || edit.speed.is_some()
            {
                return Err(refuse().into());
            }
            let (base, picture) = crate::video::motion::LinearMotion::prepare_premiere(
                effect,
                frames_on(edit.source_in, sequence.rate).map_err(|_| refuse())?,
                frames_on(edit.duration, sequence.rate).map_err(|_| refuse())?,
                frames_on(source.duration, sequence.rate).map_err(|_| refuse())?,
                TICKS_PER_SECOND / 24000 * 1001,
            )
            .map_err(|error| format!("{}: {error}", refuse()))?;
            edit.transform = Some(base);
            edit.effects.clear();
            motions.push((edit.id.clone(), picture));
        }
    }
    Ok(motions)
}
