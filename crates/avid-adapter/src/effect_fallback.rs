//! Verify file packages before using dry audio or a saved effect render.
use super::*;

#[derive(Default)]
pub(super) struct Verified {
    pub dry_audio: HashMap<(MobId, i64), (Rational, PathBuf)>,
    pub dry_subclip: HashMap<(MobId, i64), (Rational, PathBuf)>,
    pub timewarp: HashSet<(MobId, MobId, MobId)>,
    pub audio_renders: HashMap<MobId, ((MobId, MobId), PathBuf)>,
    pub omitted_caches: HashSet<MobId>,
    pub inputs: Vec<av::SourceInput>,
    link_dry_gain: bool,
    missing_equalizer: HashSet<(MobId, MobId)>,
    equalizer_dry: HashSet<(MobId, i64, MobId)>,
}

impl Verified {
    pub fn alternative_equalizer_input(&self, event: &av::SourceEvent) -> Option<(MobId, i64, i64)> {
        let equalizer = event.equalizer.as_ref()?;
        let render = equalizer.render?;
        if render.input_gain.is_some() || equalizer.pitch.is_some() {
            return None;
        }
        let range = event.range.as_ref()?;
        let rate = Rate::new(24, 1)?;
        let lane = (render.dry_master, render.dry_master_track);
        let (extent, _) = self.dry_audio.get(&lane)?;
        let end = time(render.dry_start.checked_add(range.length)?, rate).ok()?;
        (equalizer_render_matches(event, render)
            && self.missing_equalizer.contains(&(event.event.master, render.file))
            && self.equalizer_dry.contains(&(lane.0, lane.1, render.dry_file))
            && end <= *extent)
            .then_some((lane.0, lane.1, render.dry_start))
    }

    pub fn alternative_gain_input(&self, event: &av::SourceEvent) -> Option<(MobId, i64)> {
        let &(mob, start) = event.gain_dry_input.as_deref()?;
        let range = event.range.as_ref()?;
        let (extent, _) = self.dry_audio.get(&(mob, 1))?;
        let end = start.checked_add(range.length).and_then(|n| time(n, range.record_rate).ok())?;
        (self.link_dry_gain && event.event.combined.is_none() && end <= *extent)
            .then_some((mob, start))
    }
}

pub(super) fn equalizer_render_matches(event: &av::SourceEvent, render: av::SourceEqualizerRender) -> bool {
    audio_render_matches(event, render.start, render.frames)
}

pub(super) fn audio_render_matches(event: &av::SourceEvent, start: i64, frames: i64) -> bool {
    let Some(range) = event.range.as_ref() else { return false };
    let rate = Rate::new(24, 1).unwrap();
    event.event.kind == KIND_SOUND && event.event.combined.is_none()
        && event.event.master_track == 1 && range.source_in == start
        && range.record_rate == rate && range.source_rate == rate
        && range.length > 0 && range.length == range.source_length
        && range.source_in >= 0
        && range.source_in.checked_add(range.source_length).is_some_and(|end| end <= frames)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct AudioCache {
    file: MobId,
    origin: MobId,
    frames: i64,
    padding_samples: i64,
}

pub(super) fn verify(
    facts: &av::SourceProject, index: &index::MediaIndex, tool: &Path, link_dry_gain: bool,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Verified, String> {
    let mut out = Verified { link_dry_gain, ..Default::default() };
    let events = facts.sequences.iter().flat_map(|sequence| &sequence.events);
    let mut dry: HashSet<_> = events.clone()
        .filter(|event| event.dry_gain_bypass || event.equalizer.as_ref().is_some_and(|eq| eq.render.is_none()))
        .map(|event| (event.event.master, event.event.master_track)).collect();
    let subclip: HashSet<_> = events.clone().filter(|event| event.subclip_gain.is_some()
            && event.event.combined.is_none()
            && event.range.as_ref().is_some_and(|range| Rate::new(24, 1) == Some(range.record_rate)))
        .map(|event| (event.event.master, event.event.master_track)).collect();
    if link_dry_gain {
        dry.extend(events.clone().filter_map(|event| event.gain_dry_input.as_deref())
            .map(|(mob, _)| (*mob, 1)));
    }
    let mut caches: HashMap<_, HashSet<_>> = HashMap::new();
    let mut audio_caches: HashMap<_, HashSet<_>> = HashMap::new();
    let mut inner_pitch = HashSet::new();
    let mut equalizer_dry = HashMap::<_, HashSet<_>>::new();
    for event in events.clone() {
        if let Some(render) = event.audiosuite.as_deref() {
            audio_caches.entry(event.event.master).or_default().insert(AudioCache {
                file: render.file, origin: render.origin, frames: render.frames,
                padding_samples: render.padding_samples,
            });
        }
        if let Some((eq, render)) = event.equalizer.as_ref().and_then(|eq| Some((eq, eq.render?))) {
            let cache_master = event.event.master;
            audio_caches.entry(cache_master).or_default().insert(AudioCache {
                file: render.file, origin: render.origin, frames: render.frames, padding_samples: 0,
            });
            if let Some(pitch) = &eq.pitch {
                inner_pitch.insert(pitch.master);
            }
            let package = render.file.umid_hex();
            if link_dry_gain && render.input_gain.is_none() && eq.pitch.is_none() && !index.files.contains_key(&package)
                && index.ambiguous.binary_search(&package).is_err()
            {
                out.missing_equalizer.insert((cache_master, render.file));
                let lane = (render.dry_master, render.dry_master_track);
                equalizer_dry.entry(lane).or_default().insert(render.dry_file);
            }
        }
    }
    for issue in facts.sequences.iter().flat_map(|sequence| &sequence.unresolved) {
        if matches!(issue.issue_kind, av::SourceIssueKind::Timewarp { .. }) {
            if let [master, file, origin] = issue.route.as_slice() {
                caches.entry(master.mob).or_default().insert((file.mob, origin.mob));
            }
        }
    }
    if dry.is_empty() && subclip.is_empty() && caches.is_empty() && audio_caches.is_empty() {
        return Ok(out);
    }
    let other_users: HashSet<_> = facts.groups.iter().flat_map(|group| &group.lanes)
        .flat_map(|lane| &lane.alternatives).flat_map(|alternative| &alternative.pieces)
        .filter_map(|piece| match piece {
            av::SourceGroupPiece::Media { master, .. } => Some(*master),
            _ => None,
        })
        .chain(facts.subclips.iter().flat_map(|subclip| &subclip.tracks).filter_map(|track| track.target_mob))
        .collect();
    let needed: HashSet<_> = events
        .filter(|event| event.audiosuite.is_none() && event.equalizer.as_ref().and_then(|eq| eq.render).is_none())
        .map(|event| event.event.master).collect();
    for master in &facts.masters {
        let mob = master.master.mob_id;
        if inner_pitch.contains(&mob) && !needed.contains(&mob) && !other_users.contains(&mob)
            && !audio_caches.contains_key(&mob) && !caches.contains_key(&mob)
        {
            out.omitted_caches.insert(mob);
        }
        if let Some(bindings) = audio_caches.get(&mob) {
            if let Some(&cache) = bindings.iter().next().filter(|_| link_dry_gain && bindings.len() == 1) {
                if let Some((path, before)) = verify_audio_cache(master, cache, index, tool, check)? {
                    retain_input(&mut out, before, path, check)?;
                    out.audio_renders.insert(mob, ((cache.file, cache.origin), path.clone()));
                }
            }
            if !out.audio_renders.contains_key(&mob) && !needed.contains(&mob) && !other_users.contains(&mob) {
                out.omitted_caches.insert(mob);
            }
        }
        for (ordinal, track) in master.master.tracks.iter().enumerate() {
            let lane = (mob, track.index);
            let eq_dry = equalizer_dry.get(&lane).is_some_and(|files| files.contains(&track.file_mob));
            let dry_audio = dry.contains(&lane) || eq_dry;
            let subclip_gain = subclip.contains(&lane);
            if track.kind != KIND_SOUND || (!dry_audio && !subclip_gain)
                || master.offsets.get(ordinal) != Some(&Observed::Known(0))
            { continue; }
            let Some(Essence::Sound { sample_rate: 48_000.0, bits, channels: 1 }) = track.essence.as_ref()
                else { continue; };
            let codec = match bits { 16 => "pcm_s16le", 24 => "pcm_s24le", _ => continue };
            let package = track.file_mob.umid_hex();
            let Some(path) = index.files.get(&package) else { continue };
            let identity = index.identity(path)?;
            if identity.file_packages != [package] || identity.file_essence_tracks != 1
                || identity.file_tracks != [(48_000, 1, track.length)] || track.length <= 0
            { continue; }
            check()?;
            let before = revision::file_revision_checked(path, check).map_err(|e| e.to_string())?;
            let Ok(probe) = original::probe_offline_atom(path, tool) else { continue };
            let Some(Value::Arr(streams)) = probe.get("streams") else { continue };
            let plain_mono = streams.len() == 1;
            let mut essence = streams.iter()
                .filter(|stream| original::text(stream, "codec_type").as_deref() != Some("data"));
            let Some(pcm) = essence.next() else { continue };
            let dry_audio = dry_audio && plain_mono;
            let subclip_gain = subclip_gain && *bits == 24 && essence.next().is_none()
                && probe.get("format").and_then(|f| f.get("tags"))
                    .and_then(|tags| original::text(tags, "operational_pattern_ul")).as_deref()
                    .is_some_and(|ul| matches!(ul, "060e2b34.04010102.0d010201.10010000" | "060e2b34.04010102.0d010201.10030000"));
            if !dry_audio && !subclip_gain { continue; }
            if original::text(pcm, "codec_name").as_deref() == Some(codec)
                && original::text(pcm, "time_base").as_deref() == Some("1/48000")
                && original::number(pcm, "channels") == Some(1)
                && original::number(pcm, "start_pts") == Some(0)
                && original::number(pcm, "duration_ts") == Some(track.length)
                && original::complete_mono_pcm_packets(path, tool, track.length, bits / 8)
            {
                retain_input(&mut out, before, path, check)?;
                let extent = Rational::new(track.length.into(), 48_000)
                    .ok_or("dry audio extent overflow")?;
                if dry_audio {
                    out.dry_audio.insert(lane, (extent, path.clone()));
                    if eq_dry && master.rate == Rate::new(24, 1) {
                        out.equalizer_dry.insert((mob, track.index, track.file_mob));
                    }
                }
                if subclip_gain {
                    out.dry_subclip.insert(lane, (extent, path.clone()));
                }
            }
        }
        let Some(bindings) = caches.get(&mob) else { continue };
        if let [track] = master.master.tracks.as_slice() {
            if let Some(&(file, origin)) = bindings.iter().next().filter(|_| bindings.len() == 1) {
                if track.kind == KIND_PICTURE && track.index == 1 && track.file_mob == file
                    && master.offsets.as_slice() == [Observed::Known(0)]
                {
                    if let Some(path) = index.files.get(&file.umid_hex()) {
                        let identity = index.identity(path)?;
                        if identity.material_packages == [mob.umid_hex()]
                            && identity.file_packages == [file.umid_hex()]
                            && identity.source_packages.contains(&origin.umid_hex())
                            && identity.file_essence_tracks == 1
                        {
                            check()?;
                            let before = revision::file_revision_checked(path, check).map_err(|e| e.to_string())?;
                            if let Some(Some((rate, extent))) = original::square_1080p_mxf(path, tool) {
                                if master.rate.map(rational) == Some(rate)
                                    && identity.file_tracks == [(rate.num(), rate.den(), track.length)]
                                    && matches!(&track.essence, Some(Essence::Picture { compression, .. })
                                        if compression == "AVHD" || compression == "RI00"
                                            && matches!(identity.picture_essence_coding,
                                                Some(avid_reader::mxf::DNXHR_LB_CODING | avid_reader::mxf::DNXHR_LB_CODING_0A
                                                    | avid_reader::mxf::PRORES_422_LT_CODING)))
                                    && extent == time(track.length, master.rate.unwrap())?
                                    && original::complete_picture_packets(path, tool, track.length)
                                {
                                    retain_input(&mut out, before, path, check)?;
                                    out.timewarp.insert((mob, file, origin));
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Precomputes selected only as alternatives are not required bin media.
        if !needed.contains(&mob) && !other_users.contains(&mob) {
            out.omitted_caches.insert(mob);
        }
    }
    if link_dry_gain {
        let mut replaceable = HashSet::new();
        let mut required = HashSet::new();
        for event in facts.sequences.iter().flat_map(|sequence| &sequence.events) {
            if out.alternative_gain_input(event).is_some() {
                replaceable.insert(event.event.master);
            } else {
                required.insert(event.event.master);
            }
        }
        out.omitted_caches.extend(replaceable.difference(&required)
            .filter(|mob| !other_users.contains(mob)).copied());
    }
    Ok(out)
}

fn verify_audio_cache<'a>(
    master: &av::SourceMaster, cache: AudioCache,
    index: &'a index::MediaIndex, tool: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Option<(&'a PathBuf, String)>, String> {
    let AudioCache { file, origin, frames, padding_samples } = cache;
    let [track] = master.master.tracks.as_slice() else { return Ok(None) };
    let package = file.umid_hex();
    let Some(samples) = frames.checked_mul(2_000).filter(|&n| n > 0) else {
        return Ok(None);
    };
    if master.master.length != frames || track.file_mob != file || track.kind != KIND_SOUND || track.index != 1
        || !matches!(track.essence, Some(Essence::Sound { sample_rate: 48_000.0, bits: 24, channels: 1 }))
        || samples.checked_add(padding_samples) != Some(track.length) || master.rate != Rate::new(24, 1)
        || master.offsets.as_slice() != [Observed::Known(0)]
    {
        return Ok(None);
    }
    let Some(path) = index.files.get(&package) else { return Ok(None) };
    check()?;
    let before = revision::file_revision_checked(path, check).map_err(|e| e.to_string())?;
    let identity = index.identity(path)?;
    if identity.material_packages != [master.master.mob_id.umid_hex()]
        || identity.file_packages != [package]
        || !identity.source_packages.contains(&origin.umid_hex())
        || identity.file_essence_tracks != 1 || identity.file_tracks != [(48_000, 1, samples)]
        || !identity.timecodes.is_empty()
    {
        return Ok(None);
    }
    let Ok(probe) = original::probe_offline_atom(path, tool) else { return Ok(None) };
    let Some(Value::Arr(streams)) = probe.get("streams") else { return Ok(None) };
    let [pcm] = streams.as_slice() else { return Ok(None) };
    let op = probe.get("format").and_then(|f| f.get("tags"))
        .and_then(|tags| original::text(tags, "operational_pattern_ul"));
    let valid = op.as_deref() == Some("060e2b34.04010102.0d010201.10000000")
        && original::text(pcm, "codec_type").as_deref() == Some("audio")
        && original::text(pcm, "codec_name").as_deref() == Some("pcm_s24le")
        && original::text(pcm, "time_base").as_deref() == Some("1/48000")
        && original::number(pcm, "bits_per_sample") == Some(24)
        && original::number(pcm, "channels") == Some(1)
        && original::number(pcm, "start_pts") == Some(0)
        && original::number(pcm, "duration_ts") == Some(samples)
        && match padding_samples {
            0 => original::complete_mono_pcm_packets(path, tool, samples, 3),
            2 => original::complete_pcm24_with_zero_tail(path, tool, samples),
            _ => false,
        };
    Ok(valid.then_some((path, before)))
}

fn retain_input(
    verified: &mut Verified, before: String, path: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if revision::file_revision_checked(path, check).map_err(|e| e.to_string())? != before
        || verified.inputs.iter().any(|input| input.path == path && input.revision != before)
    {
        return Err("effect fallback source changed during media verification".into());
    }
    if !verified.inputs.iter().any(|input| input.path == path) {
        verified.inputs.push(av::SourceInput { path: path.into(), revision: before });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_timewarp(
    issue: &av::SourceIssue, length: i64, rate: Rate, bin: usize,
    copies: &HashMap<MobId, Vec<(usize, usize)>>, verified: &Verified,
    sequence: &mut SourceSequence, out: &mut ReadResult, budget: &mut Budget,
) -> Result<(), String> {
    let Observed::Known(frame) = issue.record_start else {
        return loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING, &issue.reason);
    };
    if issue.record_rate != Some(rate) || frame < 0 || length <= 0 || issue.track < 1 {
        return loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING, &issue.reason);
    }
    let start = time(frame, rate)?;
    let duration = time(length, rate)?;
    let end = start.checked_add(duration).ok_or("Timewarp record range overflow")?;
    let overlaps = |at: Rational, span: Rational| {
        at < end && at.checked_add(span).is_none_or(|last| last > start)
    };
    if sequence.placements.iter().any(|p| p.video_track == Some(issue.track) && overlaps(p.record_start, p.duration))
        || sequence.transitions.iter().any(|t| t.kind == StreamKind::Video && t.track == issue.track
            && t.cut.checked_sub(t.cut_offset).is_none_or(|at| overlaps(at, t.duration)))
    {
        return loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING, format!(
            "{}: V{} Timewarp at record frame {frame} overlaps another edit or transition", issue.owner, issue.track));
    }
    let location = format!("{} · V{} at record frame {frame} for {length} frames", sequence.name, issue.track);
    if let [render, file, origin] = issue.route.as_slice() {
        if let (Observed::Known(source_frame), Some(item)) = (&render.source_offset,
            source_master_copy(render.mob, bin, copies, &out.graph.items))
        {
            if verified.timewarp.contains(&(render.mob, file.mob, origin.mob)) {
                let placement = format!("{}/render", issue.owner);
                budget.charge(size_of::<Placement>() + placement.len() + item.id.len())?;
                sequence.placements.push(Placement {
                    id: placement.clone(), item: item.id.clone(), record_start: start,
                    source_in: time(*source_frame, rate)?, duration,
                    video_track: Some(issue.track), audio_tracks: vec![], audio_streams: vec![],
                    grouped_audio: vec![], gain_db: 0.0, volume_db: None, volume_keys: vec![],
                    track_gains_db: vec![], track_pans: vec![], speed: None, frame_map: None,
                    transform: None, effects: vec![],
                });
                let composite = issue.timewarp.as_ref().is_some_and(|controls| controls.resize_render_owner.is_some());
                let detail = if composite {
                    "linked the verified saved Resize render containing this Timewarp. Resize and retiming controls are baked in."
                } else {
                    "linked the verified saved Timewarp render. Retiming controls are unavailable."
                };
                return loss(out, budget, &placement, "timewarp_render", format!("{location}: {detail}"));
            }
        }
    }
    let mode = issue.timewarp.as_deref().and_then(av::SourceTimewarpControls::explicit_fluid_motion);
    let reason = match mode {
        Some(mode) => format!("Avid Timewarp uses {mode}. Editable conversion cannot reproduce its synthesized frames"),
        None => issue.reason.trim_end_matches('.').to_owned(),
    };
    let comment = format!("{location}: {reason}. No verified render was available. This span is a gap.");
    mark_gap(issue, sequence, start, duration, format!("Timewarp omitted · V{}", issue.track),
        comment, "timewarp_gap", out, budget)
}

pub(super) fn prepare_effect_gap(
    issue: &av::SourceIssue, effect: &str, length: i64, rate: Rate,
    native_transitions: &[av::SourceTransition], cut_fallbacks: &mut HashSet<String>,
    sequence: &mut SourceSequence, out: &mut ReadResult, budget: &mut Budget,
) -> Result<(), String> {
    let (Some(kind), Observed::Known(frame)) = (kind(issue.kind), &issue.record_start) else {
        return loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING, &issue.reason);
    };
    if issue.record_rate != Some(rate) || *frame < 0 || length <= 0 || issue.track < 1 {
        return loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING, &issue.reason);
    }
    let start = time(*frame, rate)?;
    let duration = time(length, rate)?;
    let end = start.checked_add(duration).ok_or("Effect gap end overflows its record clock")?;
    let lane = if kind == StreamKind::Video { "V" } else { "A" };
    let overlap_error = |out: &mut ReadResult, budget: &mut Budget, sequence: &SourceSequence| {
        loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING, format!(
            "{}: {} · {lane}{} at record frame {frame} for {length} frames: {effect} overlaps another edit or transition",
            issue.owner, sequence.name, issue.track))
    };
    // Avid clip ranges include the overlap. The marked gap starts and
    // ends at the saved cuts, and Premiere drops those exact dissolves.
    let mut gap_start = start;
    let mut gap_end = end;
    let mut edges = [None, None];
    for t in sequence.transitions.iter().filter(|t| t.kind == kind && t.track == issue.track) {
        let begin = t.cut.checked_sub(t.cut_offset).ok_or("Transition start overflow")?;
        let last = begin.checked_add(t.duration).ok_or("Transition end overflow")?;
        if begin >= end || last <= start {
            continue;
        }
        let native = native_transitions.iter().find(|native| native.id == t.id);
        let edge = native.filter(|native| native.rate == Some(rate)
            && kind == StreamKind::Audio && effect == "AudioSuite Gain"
            && t.style == "avid.BLEND_AUDIO_DISSOLVE").and_then(|native| {
            let is_gap = |n| matches!(&native.gain_gap_edges[n], Some(av::GainGapEdge::Gain(owner)) if owner == &issue.owner);
            let outgoing = is_gap(0) && last == end && t.outgoing.placements.is_empty();
            let incoming = is_gap(1) && begin == start && t.incoming.placements.is_empty();
            match (outgoing, incoming) {
                (true, false) => Some(0),
                (false, true) => Some(1),
                _ => None,
            }
        });
        let Some(edge) = edge else { return overlap_error(out, budget, sequence) };
        if edges[edge].replace(t).is_some() {
            return overlap_error(out, budget, sequence);
        }
        if edge == 0 {
            gap_end = t.cut;
        } else {
            gap_start = t.cut;
        }
    }
    if gap_start >= gap_end {
        return overlap_error(out, budget, sequence);
    }
    let occupied = sequence.placements.iter().chain(&sequence.disabled_placements).any(|p| {
        let same_lane = if kind == StreamKind::Video {
            p.video_track == Some(issue.track)
        } else {
            p.audio_tracks.contains(&issue.track)
                || p.grouped_audio.iter().any(|group| group.track == issue.track)
        };
        if !same_lane { return false; }
        let mut first = p.record_start;
        let Some(mut last) = first.checked_add(p.duration) else { return true };
        if let Some(t) = edges[0] {
            if t.incoming.reason.is_none() && t.incoming.placements == [p.id.as_str()]
                && first.checked_add(t.cut_offset) == Some(t.cut)
            {
                first = t.cut;
            }
        }
        if let Some(t) = edges[1] {
            if t.outgoing.reason.is_none() && t.outgoing.placements == [p.id.as_str()]
                && last.checked_sub(t.duration).and_then(|at| at.checked_add(t.cut_offset)) == Some(t.cut)
            {
                last = t.cut;
            }
        }
        first < gap_end && last > gap_start
    });
    if occupied {
        return overlap_error(out, budget, sequence);
    }
    for t in edges.into_iter().flatten() {
        budget.charge(t.id.len() + 64)?;
        cut_fallbacks.insert(t.id.clone());
    }
    let frame_at = |at: Rational| at.to_units(rate.num)
        .filter(|n| n % rate.den == 0).map(|n| n / rate.den)
        .ok_or("Effect gap is off the record frame grid");
    let gap_duration = gap_end.checked_sub(gap_start).ok_or("Effect gap duration overflow")?;
    let location = format!("{} · {lane}{} at record frame {} for {} frames",
        sequence.name, issue.track, frame_at(gap_start)?, frame_at(gap_duration)?);
    let reason = issue.reason.trim_end_matches('.');
    let comment = format!("{location}: {reason}. This span is a gap.");
    mark_gap(issue, sequence, gap_start, gap_duration, format!("{effect} omitted · {lane}{}", issue.track),
        comment, "effect_gap", out, budget)
}

#[allow(clippy::too_many_arguments)]
fn mark_gap(
    issue: &av::SourceIssue, sequence: &mut SourceSequence, start: Rational, duration: Rational,
    name: String, comment: String, aspect: &str, out: &mut ReadResult, budget: &mut Budget,
) -> Result<(), String> {
    budget.charge(size_of::<SourceMarker>() + issue.owner.len() + comment.len() + name.len() + 64)?;
    sequence.markers.push(SourceMarker {
        name,
        comment: comment.clone(), user: String::new(), color: None, start: Some(start), duration,
        track: None, owner: MarkerOwner::Sequence, owner_occurrence: issue.owner.clone(),
        owner_local: Some(start), owner_rate: Some(sequence.rate),
        binding: SourceBinding { placements: vec![], reason: None }, source: None, inherited: false,
    });
    // Name the occurrence in the report so two same-position losses stay distinct.
    loss(out, budget, &sequence.id, aspect, format!("{}: {comment}", issue.owner))
}
