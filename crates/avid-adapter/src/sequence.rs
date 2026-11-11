use super::*;
use avid_reader::source as av;

fn routes(routes: &[av::RouteWitness]) -> Value {
    Value::Arr(
        routes
            .iter()
            .map(|r| {
                obj(vec![
                    ("mob", s(r.mob.urn())),
                    ("track", Value::Int(r.track)),
                    (
                        "rate",
                        r.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                    ),
                    ("source_offset", observed(&r.source_offset)),
                    ("length", observed(&r.length)),
                ])
            })
            .collect(),
    )
}
fn binding_receipt(b: &av::Binding) -> Value {
    obj(vec![
        (
            "placements",
            Value::Arr(b.placements.iter().map(s).collect()),
        ),
        ("reason", opt_s(b.reason.as_deref())),
    ])
}
fn binding(b: &av::Binding, map: &HashMap<String, String>) -> SourceBinding {
    let mut placements = vec![];
    let mut seen = HashSet::new();
    let mut missing = false;
    for id in &b.placements {
        if let Some(id) = map.get(id) {
            if seen.insert(id.as_str()) {
                placements.push(id.clone());
            }
        } else {
            missing = true;
        }
    }
    SourceBinding {
        placements,
        reason: b
            .reason
            .clone()
            .or_else(|| missing.then(|| "one or more native endpoints were not portable".into())),
    }
}
pub(super) struct ItemRoutes {
    pub audio: HashSet<i64>,
    pub raster: Option<(i64, i64)>,
    pub square_pixels: bool,
    pub motion_picture: Option<Rational>,
    pub motion_audio: HashMap<i64, (i64, Rational)>,
    pub timewarp_picture: Option<Rational>,
}

fn adapter_picture_frame(source_in: i64, phase: i64, record: i128, span_ratio: (i64, i64)) -> i128 {
    let p = i128::from(phase);
    let (span, source) = (i128::from(span_ratio.0), i128::from(span_ratio.1));
    i128::from(source_in) + (p + record) * source / span - p * source / span
}

fn adapter_audio_in_samples(
    source_in: i64, phase: i64, track_start: i64, span_ratio: (i64, i64), source_unit: i128,
) -> i128 {
    let (span, source) = (i128::from(span_ratio.0), i128::from(span_ratio.1));
    (i128::from(source_in) + i128::from(track_start)) * source_unit
        + (i128::from(phase) * source % span) * source_unit / span
}

fn motion_timing(
    event: &av::SourceEvent, range: &av::QualifiedRange, phase: i64, item: &ItemRoutes,
    covering_picture: bool, p: &mut Placement, budget: &mut Budget,
) -> Result<i64, String> {
    let picture = item.motion_picture.ok_or("requires qualified linked picture")?;
    let span_ratio = av::Rate::automatic_adapter_span(range.source_rate, range.record_rate)
        .ok_or("requires a qualified automatic adapter rate pair")?;
    if item.motion_audio.len() != 2 { return Err("requires two proven linked mono lanes".into()); }
    if event.event.combined.is_some()
        || !event.event.via.is_empty() || event.volume_automation.is_some()
        || event.picture_extension.is_some() || event.event.picture.is_some()
        || !event.picture_chain.is_empty()
        || event.event.animatte.is_some() || event.event.color.is_some()
    { return Err("requires an unstacked automatic adapter".into()); }
    if time(range.source_in.checked_add(range.source_length).ok_or("source extent overflow")?, range.source_rate)? > picture {
        return Err("saved source interval exceeds linked picture".into());
    }
    if event.event.kind == KIND_PICTURE {
        let count = usize::try_from(range.length).map_err(|_|"negative adapter length")?;
        if !(2..=16_384).contains(&count) { return Err("adapter map length outside bounds".into()); }
        budget.charge(count.checked_mul(size_of::<i64>()).ok_or("adapter map allocation overflow")?)?;
        let mut map = Vec::new();
        map.try_reserve_exact(count).map_err(|_|"adapter map allocation failed")?;
        for i in 0..count {
            let f = adapter_picture_frame(range.source_in, phase, i as i128, span_ratio);
            map.push(i64::try_from(f).map_err(|_|"adapter picture position overflow")?);
        }
        p.frame_map = Some(map);
        return Ok(0);
    }
    let (origin, duration) = item.motion_audio.get(&event.event.master_track)
        .copied().ok_or("linked AAC file-track origin unresolved")?;
    let source_unit = 48_000 * i128::from(range.source_rate.den) / i128::from(range.source_rate.num);
    let record_unit = 48_000 * i128::from(range.record_rate.den) / i128::from(range.record_rate.num);
    let samples = adapter_audio_in_samples(range.source_in, phase, origin, span_ratio, source_unit);
    let start = Rational::new(samples, 48_000).ok_or("audio origin overflow")?;
    let media_end = duration.to_units(48_000).ok_or("audio media end is not on the sample grid")?;
    let target = i128::from(range.length) * record_unit;
    let playable = (i128::from(media_end) - samples).min(target);
    let tail = target - playable;
    if playable <= 0 || tail >= record_unit || (tail > 0 && !covering_picture) {
        return Err("audio tail needs a qualified source end and covering picture".into());
    }
    p.source_in = start;
    p.duration = Rational::new(playable, 48_000).ok_or("adapter audio extent overflow")?;
    Ok(tail as i64)
}

fn apply_saved_timewarp(
    frames: &[i64], range: &av::QualifiedRange, item: &ItemRoutes,
    p: &mut Placement, budget: &mut Budget,
) -> Result<bool, String> {
    let rate = range.record_rate;
    let profile = match (rate.num, rate.den, range.length, range.source_length) {
        (24, 1, 48, 59) => range.source_in >= 0,
        (24_000, 1001, 1666, 1666) => range.source_in == 0,
        _ => false,
    };
    let end = range.source_in.checked_add(range.source_length).and_then(|n| time(n, rate).ok());
    if !profile || range.source_rate != rate || i64::try_from(frames.len()).ok() != Some(range.length)
        || frames.iter().any(|&n| !(0..range.source_length).contains(&n))
        || end.zip(item.timewarp_picture).is_none_or(|(end, duration)| end > duration)
    {
        return Ok(false);
    }
    budget.charge(frames.len() * size_of::<i64>())?;
    let mut map = Vec::new();
    map.try_reserve_exact(frames.len()).map_err(|_| "Timewarp map allocation failed")?;
    budget.charge((map.capacity() - frames.len()) * size_of::<i64>())?;
    // Source range origin is in the same clock, and the verified file offset is zero.
    map.extend(frames.iter().map(|frame| range.source_in + frame));
    p.speed = None;
    p.frame_map = Some(map);
    Ok(true)
}

#[cfg(test)]
#[test]
fn automatic_adapter_picture_and_sound_use_separate_native_clocks() {
    let forward = (1000, 1001);
    let reverse = (1001, 1000);
    assert_eq!([0, 951, 952, 1101].map(|n| adapter_picture_frame(48, 48, n, forward)),
        [48, 999, 1001, 1150]);
    assert_eq!(adapter_picture_frame(48, 0, 952, forward), 1000);
    assert_eq!(adapter_audio_in_samples(48, 48, 1, forward, 2000), 98_096);
    assert_eq!(adapter_audio_in_samples(48, 48, 0, forward, 2000), 96_096);
    assert_eq!([0, 952, 953, 954, 1103].map(|n| adapter_picture_frame(48, 49, n, reverse)),
        [48, 1000, 1000, 1001, 1150]);
    let start = adapter_audio_in_samples(48, 49, 1, reverse, 2002);
    assert_eq!(start, 100_002);
    assert_eq!(1104 * 2000 - (2_306_304 - start), 1698);
}
pub(super) fn append(
    source: &av::SourceSequence,
    copies: &HashMap<avid_reader::io::MobId, Vec<(usize, usize)>>,
    item_routes: &[ItemRoutes],
    multicams: &super::multicam::Index<'_>,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<(), String> {
    budget.charge(size_of::<SourceSequence>() + source.id.len() + source.info.name.len())?;
    for marker in &source.markers {
        reserve_marker(&marker.marker, budget)?;
        budget.charge(marker.id.len() + marker.owner.len())?;
    }
    for event in &source.events {
        budget.charge(
            event
                .id
                .len()
                .checked_add(event.owner.len())
                .and_then(|n| n.checked_mul(3))
                .ok_or("event identity budget overflow")?,
        )?;
        for text in &event.event.via {
            budget.charge(text.len())?;
        }
        if let Some(range) = &event.range {
            budget.charge(
                range
                    .route
                    .len()
                    .checked_mul(1024)
                    .ok_or("route slots budget overflow")?,
            )?;
            for route in &range.route {
                for witness in [&route.source_offset, &route.length] {
                    if let Observed::Unresolved(reason) = witness {
                        budget.charge(reason.len())?;
                    }
                }
            }
        }
    }
    for issue in &source.unresolved {
        budget.charge(issue.reason.len() + issue.owner.len() + issue.route.len() * 1024)?;
    }
    for text in &source.info.problems {
        budget.charge(text.len())?;
    }
    // Reserve original compact controls before hex/point/metadata expansion.
    reserve_controls(source, budget)?;
    let Some(rate) = source.rate else {
        let bin = out
            .graph
            .bins
            .get(source.info.bin)
            .ok_or("unknown sequence bin")?
            .id
            .clone();
        append_bin_receipt(
            out,
            budget,
            source.info.bin,
            "unresolved_sequences",
            raw_sequence(source),
        )?;
        loss(
            out,
            budget,
            &bin,
            "source_sequence",
            format!(
                "{}: native record rate unresolved; compact source witnesses retained on bin",
                source.id
            ),
        )?;
        out.diagnostics.push(format!(
            "{:?}: sequence record rate unresolved; native facts unavailable for portable sequence",
            source.info.name
        ));
        return Ok(());
    };
    let id = &source.id;
    let clock = if let (Some(c), Some(mob)) = (source.clock, source.info.mob_id) {
        const ROUTE: &str = "direct sequence TC1";
        // Fixed-size UMID text and a usize track ordinal; charge before allocating.
        budget.charge(96 + 48 + ROUTE.len())?;
        Some(SourceClock {
            effective_count: c.count,
            rate: rational(c.rate),
            display_base: c.display_base,
            drop: false,
            provenance: SourceClockProvenance {
                origin_count: c.count,
                source_offset: 0,
                source_rate: rational(rate),
                source_package: mob.urn(),
                timecode_track: format!("TC1/{}", c.track_ordinal),
                route: ROUTE.into(),
            },
        })
    } else {
        None
    };
    let start = clock
        .as_ref()
        .map(|c| c.exact_start().ok_or("qualified sequence clock overflow"))
        .transpose()?;
    let mut seq = SourceSequence {
            texts: Vec::new(),
        id: id.clone(),
        name: source.info.name.clone(),
        rate: rational(rate),
        frame_size: None,
        start,
        clock,
        tracks: vec![],
        placements: vec![],
        transitions: vec![],
        markers: vec![],
    };
    budget.charge(source.texts.len().checked_mul(size_of::<SourceText>()).ok_or("text slots budget overflow")?)?;
    seq.texts.try_reserve_exact(source.texts.len()).map_err(|_|"source text allocation failed")?;
    if seq.texts.capacity() > source.texts.len() { budget.charge((seq.texts.capacity()-source.texts.len()).checked_mul(size_of::<SourceText>()).ok_or("text capacity overflow")?)?; }
    for t in &source.texts {
        budget.charge(t.id.len() + t.content.as_ref().map_or(0, String::len) + t.reason.as_ref().map_or(0, String::len))?;
        let text = SourceText {
            id: t.id.clone(),
            kind: match t.kind { av::TextKind::SubCap => TextKind::Caption, av::TextKind::TitlerPlus => TextKind::Graphic },
            track: t.track,
            content: t.content.clone(),
            record_start: t.record_start.zip(t.rate).map(|(n,r)| time(n,r)).transpose()?,
            duration: t.length.filter(|n| *n > 0).zip(t.rate).map(|(n,r)| time(n,r)).transpose()?,
            reason: t.reason.clone(),
        };
        loss(out, budget, &t.id, "text_appearance", match t.kind {
            av::TextKind::SubCap => "SubCap font, position, appearance, animation and global-style settings remain native-only; the neutral text occurrence carries content and timing",
            av::TextKind::TitlerPlus => "Titler+ appearance, geometry, font resolution, paragraph styling and animation remain native-only; the neutral text occurrence carries content and timing",
        })?;
        if t.kind == av::TextKind::TitlerPlus
            || !t.native_properties.is_empty()
            || t.native_properties_reason.is_some()
        {
            let mut reservation = Budget { used: budget.used };
            reservation.charge(2048 + t.native_properties_reason.as_ref().map_or(0, String::len))?;
            for p in &t.native_properties { reservation.charge(1024 + p.scope.len() + p.name.len() + p.value.len())?; }
            sidecar(out, budget, "avid.text", &t.id, text_properties(t))?;
        }
        seq.texts.push(text);
    }
    let mut map = HashMap::new();
    let mut groups: HashMap<usize, Vec<(usize, &av::SourceEvent)>> = HashMap::new();
    let mut original: HashMap<usize, HashSet<usize>> = HashMap::new();
    let mut unresolved = vec![];
    for event in &source.events {
        if let Some((g, ch)) = event.event.combined {
            original.entry(g).or_default().insert(ch);
        }
    }
    let mut derived_raster: Option<(i64, i64, i64, i64)> = None;
    let mut adapter_picture_spans = HashSet::new();
    for event in &source.events {
        if event.event.kind == KIND_PICTURE {
            if let (Some(phase), Some(range)) = (event.motion_adapter_phase, &event.range) {
                budget.charge(2 * size_of::<(avid_reader::io::MobId, i64, i64, i64)>())?;
                adapter_picture_spans.insert((event.event.master, phase, range.record_start, range.length));
            }
        }
    }
    for event in &source.events {
        let e = &event.event;
        let Some(range) = &event.range else {
            unresolved.push(obj(vec![
                ("owner", s(&event.owner)),
                (
                    "reason",
                    s(event.reason.as_deref().unwrap_or("event range unresolved")),
                ),
            ]));
            continue;
        };
        let timing_aspect = if event.motion_adapter_phase.is_some() || event.saved_timewarp.is_some() {
            crate::UNQUALIFIED_ROUTE_TIMING
        } else { "source_route" };
        let Some(candidates) = copies.get(&e.master) else {
            loss(
                out,
                budget,
                id,
                timing_aspect,
                format!("{}: master clip was not converted", event.id),
            )?;
            continue;
        };
        let index = candidates
            .iter()
            .find(|(bin, _)| *bin == source.info.bin)
            .unwrap_or(&candidates[0])
            .1;
        let item = &out.graph.items[index];
        let stream = (e.kind == KIND_SOUND && item_routes[index].audio.contains(&e.master_track))
            .then_some(e.master_track);
        if e.kind == KIND_SOUND && stream.is_none() {
            loss(
                out,
                budget,
                id,
                timing_aspect,
                format!(
                    "{}: source A{} media unavailable; edit omitted",
                    event.id, e.master_track
                ),
            )?;
            continue;
        }
        if e.kind != KIND_SOUND && e.kind != KIND_PICTURE {
            loss(
                out,
                budget,
                id,
                "source_route",
                format!("{}: unsupported lane kind {}", event.id, e.kind),
            )?;
            continue;
        }
        let raster = item_routes[index].raster;
        if e.kind == KIND_PICTURE {
            let Some((w, h)) = raster else {
                loss(
                    out,
                    budget,
                    id,
                    timing_aspect,
                    format!(
                        "{}: source picture media unavailable; edit omitted",
                        event.id
                    ),
                )?;
                continue;
            };
            let choice = (range.record_start, e.track, w, h);
            if derived_raster.is_none_or(|old| choice < old) {
                derived_raster = Some(choice);
            }
        }
        let mut p = Placement {
            id: event.id.clone(),
            item: item.id.clone(),
            record_start: time(range.record_start, range.record_rate)?,
            source_in: time(range.source_in, range.source_rate)?,
            duration: time(range.length, range.record_rate)?,
            video_track: (e.kind == KIND_PICTURE).then_some(e.track),
            audio_tracks: if e.kind == KIND_SOUND {
                vec![e.track]
            } else {
                vec![]
            },
            audio_streams: stream.into_iter().collect(),
            grouped_audio: vec![],
            gain_db: 0.0,
            volume_db: None,
            volume_keys: vec![],
            track_gains_db: if e.kind == KIND_SOUND && e.edit_gain_db.is_some() {
                vec![e.edit_gain_db]
            } else {
                vec![]
            },
            track_pans: if e.kind == KIND_SOUND && e.edit_pan.is_some() {
                vec![e.edit_pan]
            } else {
                vec![]
            },
            speed: e
                .speed
                .map(|ratio| {
                    Ok::<_, String>(Speed {
                        ratio,
                        source_duration: time(range.source_length, range.source_rate)?,
                    })
                })
                .transpose()?,
            frame_map: None,
            transform: None,
            effects: vec![],
        };
        if let Some(frames) = &event.saved_timewarp {
            if e.kind != KIND_PICTURE || !e.via.is_empty() {
                loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING,
                    format!("V{}: saved Timewarp requires a direct picture edit", e.track))?;
                continue;
            }
            if !apply_saved_timewarp(frames, range, &item_routes[index], &mut p, budget)? {
                loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING,
                    format!("V{}: saved Timewarp requires verified progressive 1080p picture at the sequence rate with zero media offset and coverage for the saved source span", e.track))?;
                continue;
            }
        } else if let Some(phase) = event.motion_adapter_phase {
            let covering = e.kind == KIND_PICTURE
                || adapter_picture_spans.contains(&(e.master, phase, range.record_start, range.length));
            let tail = match motion_timing(event, range, phase, &item_routes[index], covering, &mut p, budget) {
                Ok(tail) => tail,
                Err(why) => {
                    let lane = if e.kind == KIND_PICTURE { "V" } else { "A" };
                    loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING, format!("{lane}{}: {why}", e.track))?;
                    continue;
                }
            };
            if tail > 0 && range.record_rate == av::Rate::new(24, 1).unwrap() {
                loss(out, budget, &p.id, "motion_adapter_audio_tail",
                    format!("A{}: {tail} samples beyond the linked MP4 audio extent render as silence. Avid can emit decoded AAC packet padding within that interval.", e.track))?;
            }
            sidecar(out, budget, "avid.motion_adapter", &p.id, obj(vec![
                ("phase", Value::Int(phase)),
                ("native_record_frames", Value::Int(range.length)),
                ("silent_audio_tail_samples", Value::Int(tail)),
            ]))?;
        } else if e.kind == KIND_SOUND && item_routes[index].motion_audio.contains_key(&e.master_track) {
            loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING,
                format!("A{}: linked AAC file-track origin outside the qualified Motion Adapter route", e.track))?;
            continue;
        }
        if let Some(automation) = &event.volume_automation {
            match automation {
                av::VolumeAutomation::Keys(keys) => {
                    budget.charge(keys.len() * size_of::<graph::VolumeKey>())?;
                    p.volume_keys.reserve_exact(keys.len());
                    for key in keys {
                        let local = Rational::new(
                            key.frame_numerator as i128 * range.record_rate.den as i128,
                            key.frame_denominator as i128 * range.record_rate.num as i128,
                        ).ok_or("volume key time overflows rational")?;
                        let at = p.source_in.checked_add(local).ok_or("volume key source time overflows")?;
                        let db = 20.0 * (key.gain_q29 as f64 / (1u64 << 29) as f64).log10();
                        p.volume_keys.push(graph::VolumeKey { at, db });
                    }
                }
                av::VolumeAutomation::Unsupported(reason) => {
                    loss(out, budget, &p.id, "volume_automation", *reason)?;
                }
            }
        }
        if event.picture_chain.is_empty() {
            // Active curves in the qualified transforms supersede stale inline scalars.
            let replace_static = event.picture_extension.is_some_and(av::PictureExtension::replaces_static_transform);
            effects::apply(e, !replace_static, raster, &mut p, out, budget)?;
            if let Some(mut effect) = event.picture_extension {
                if effect.requires_square_pixels() && !item_routes[index].square_pixels {
                    effect = av::PictureExtension::Unsupported("Animated crop, Warp, or trimmed Resize requires verified square-pixel progressive geometry");
                }
                if e.picture_nested {
                    effect = av::PictureExtension::Unsupported("nested picture effects require separate qualification");
                }
                effects::apply_picture_extension(effect, raster, &mut p, out, budget)?;
            }
        } else {
            effects::apply_picture_chain(&event.picture_chain, raster,
                item_routes[index].square_pixels, &mut p, out, budget)?;
        }
        let mut receipt = vec![
            ("owner", s(&event.owner)),
            ("native_kind", Value::Int(e.kind)),
            ("native_track", Value::Int(e.track)),
            ("master_track", Value::Int(e.master_track)),
            ("route", routes(&range.route)),
            ("record_rate", s(rational(range.record_rate))),
            ("source_rate", s(rational(range.source_rate))),
            ("record_length", Value::Int(range.length)),
            ("source_length", Value::Int(range.source_length)),
            ("via", Value::Arr(e.via.iter().map(s).collect())),
            ("combined", e.combined.map(|(g, ch)| Value::Arr(vec![s(g), s(ch)])).unwrap_or(Value::Null)),
        ];
        if !p.volume_keys.is_empty() {
            receipt.push(("pvol_volume_keys", Value::Bool(true)));
        }
        if event.saved_timewarp.is_some() {
            receipt.push(("saved_timewarp", Value::Bool(true)));
        }
        let reflection = |stage, native_id| {
            obj(vec![("stage", Value::Int(stage as i64)), ("effect_id", s(native_id))])
        };
        let reflections: Vec<_> = if event.picture_chain.is_empty() {
            match event.picture_extension {
                Some(av::PictureExtension::Flip { native_id, .. }) => vec![reflection(0, native_id)],
                _ => vec![],
            }
        } else {
            event.picture_chain.iter().enumerate().filter_map(|(index, stage)| {
                let av::PictureStage::Extension(av::PictureExtension::Flip { native_id, .. }) = stage else {
                    return None;
                };
                Some(reflection(index, native_id))
            }).collect()
        };
        if !reflections.is_empty() {
            receipt.push(("native_reflections", Value::Arr(reflections)));
        }
        sidecar(
            out,
            budget,
            "avid.source",
            &p.id,
            obj(receipt),
        )?;
        budget.charge(size_of::<Placement>() + p.id.len() + p.item.len() + 128)?;
        let n = seq.placements.len();
        if let Some((g, _)) = e.combined {
            groups.entry(g).or_default().push((n, event));
        }
        map.insert(event.id.clone(), p.id.clone());
        seq.placements.push(p);
    }
    // Only genuine native combiners become atomic multichannel placements. No coincident
    // independent audio/video edits are assembled here.
    let mut remove = HashSet::new();
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by_key(|(g, _)| *g);
    for (group, mut members) in groups {
        members.sort_by_key(|(_, e)| e.event.combined.unwrap().1);
        let native = &original[&group];
        let expected = source.combiner_channels.get(group).copied();
        let complete_native =
            expected.is_some_and(|n| native.len() == n && (0..n).all(|ch| native.contains(&ch)));
        let first = members[0].0;
        let mono = expected == Some(2) && complete_native && members.len() == 1;
        if mono {
            let side = members[0].1.event.combined.unwrap().1;
            let p = &mut seq.placements[first];
            if p.track_pans.is_empty() {
                p.track_pans = vec![Some(if side == 0 { -100.0 } else { 100.0 })];
            }
            loss(out,budget,&p.id,"audio_routing","native stereo combiner missing a channel; available channel placed as mono with its pan")?;
            continue;
        }
        let p = &seq.placements[first];
        let valid = complete_native
            && expected == Some(members.len())
            && (2..=8).contains(&members.len())
            && members
                .iter()
                .map(|(i, _)| seq.placements[*i].audio_streams.first().copied())
                .collect::<HashSet<_>>()
                .len()
                == members.len()
            && members.iter().all(|(i, _)| {
                let q = &seq.placements[*i];
                q.item == p.item
                    && q.record_start == p.record_start
                    && q.source_in == p.source_in
                    && q.duration == p.duration
                    && q.audio_tracks == p.audio_tracks
                    && q.audio_streams.len() == 1
                    && q.speed == p.speed
                    && q.track_gains_db == p.track_gains_db
            });
        if !valid {
            for (i, e) in members {
                remove.insert(i);
                map.remove(&e.id);
            }
            loss(out,budget,id,"audio_routing",format!("native combiner {group} has incomplete or incompatible members; retained as unresolved source facts"))?;
            continue;
        }
        let streams = members
            .iter()
            .map(|(i, _)| seq.placements[*i].audio_streams[0])
            .collect();
        let pans = members
            .iter()
            .map(|(i, _)| seq.placements[*i].track_pans.first().copied().flatten())
            .collect();
        let gain = seq.placements[first]
            .track_gains_db
            .first()
            .copied()
            .flatten();
        let track = seq.placements[first].audio_tracks[0];
        let atomic = seq.placements[first].id.clone();
        seq.placements[first].grouped_audio = vec![GroupedAudio {
            track,
            streams,
            gain_db: gain,
            pans,
        }];
        seq.placements[first].audio_tracks.clear();
        seq.placements[first].audio_streams.clear();
        seq.placements[first].track_gains_db.clear();
        seq.placements[first].track_pans.clear();
        for (i, e) in members.into_iter().skip(1) {
            remove.insert(i);
            map.insert(e.id.clone(), atomic.clone());
        }
    }
    let removed_ids: HashSet<_> = seq
        .placements
        .iter()
        .enumerate()
        .filter(|(i, _)| remove.contains(i))
        .map(|(_, p)| p.id.clone())
        .collect();
    seq.placements = seq
        .placements
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !remove.contains(i))
        .map(|(_, p)| p)
        .collect();
    // The combined first event retains its own route sidecar; full member witnesses belong
    // to the sequence receipt, so no dangling per-object sidecars survive grouping.
    out.graph
        .sidecars
        .retain(|s| !removed_ids.contains(&s.object));
    out.graph
        .losses
        .retain(|l| !removed_ids.contains(&l.object));
    for t in &source.transitions {
        let (
            Some(k),
            Some(r),
            Observed::Known(position),
            Observed::Known(length),
            Observed::Known(offset),
        ) = (kind(t.kind), t.rate, &t.position, &t.length, &t.cutpoint)
        else {
            loss(
                out,
                budget,
                id,
                "transition",
                format!("{}: native transition timing unresolved", t.id),
            )?;
            continue;
        };
        let Some(cut) = position.checked_add(*offset) else {
            return Err("transition cut overflow".into());
        };
        seq.transitions.push(SourceTransition {
            id: t.id.clone(),
            kind: k,
            track: t.track,
            cut: time(cut, r)?,
            duration: time(*length, r)?,
            cut_offset: time(*offset, r)?,
            style: format!("avid.{}", t.effect.as_deref().unwrap_or("unknown")),
            outgoing: binding(&t.outgoing, &map),
            incoming: binding(&t.incoming, &map),
        });
        budget.charge(size_of::<SourceTransition>() + t.id.len() + 256)?;
    }
    let placement_index: HashMap<_, _> = seq
        .placements
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    let mut marker_receipts = vec![];
    for mk in &source.markers {
        let mut native = sequence_marker_native(mk);
        let unqualified_duration =
            mk.marker.length.is_some_and(|n| n != 0) && mk.local_rate.is_none();
        if let Value::Obj(fields) = &mut native {
            fields.push((
                "graph_index".into(),
                if unqualified_duration {
                    Value::Null
                } else {
                    Value::Int(seq.markers.len() as i64)
                },
            ));
        }
        marker_receipts.push(native);
        if unqualified_duration {
            loss(out,budget,id,"marker_timing","native spanned marker duration has no qualified owner rate; marker retained only as native facts")?;
            continue;
        }
        let local = match (&mk.local_offset, mk.local_rate) {
            (Observed::Known(n), Some(r)) => Some(time(*n, r)?),
            _ => None,
        };
        let start = mk.record_frame.map(|n| time(n, rate)).transpose()?;
        let owner = match mk.owner_kind {
            extract::MarkerOwner::Sequence => MarkerOwner::Sequence,
            extract::MarkerOwner::Filler => MarkerOwner::Filler,
            extract::MarkerOwner::Edit => MarkerOwner::Edit,
        };
        let b = binding(&mk.binding, &map);
        let track = kind(mk.kind).map(|kind| TrackRef {
            kind,
            index: mk.track,
        });
        let mut m = marker(
            &mk.marker,
            start,
            local,
            mk.local_rate,
            mk.owner.clone(),
            owner,
            track,
            b,
        )?;
        if m.owner == MarkerOwner::Edit
            && m.binding.reason.is_none()
            && m.binding.placements.len() == 1
        {
            if let (Some(start), Some(p)) = (
                start,
                placement_index
                    .get(m.binding.placements[0].as_str())
                    .map(|i| &seq.placements[*i]),
            ) {
                if p.speed.is_none() && p.frame_map.is_none() {
                    m.source = start
                        .checked_sub(p.record_start)
                        .and_then(|d| p.source_in.checked_add(d));
                }
            }
        }
        budget.charge(
            size_of::<SourceMarker>()
                + m.name.capacity()
                + m.comment.capacity()
                + m.user.capacity()
                + m.owner_occurrence.capacity()
                + 256,
        )?;
        seq.markers.push(m);
    }
    let compatibility_frame_size = derived_raster
        .map(|(_, _, w, h)| (w, h))
        .unwrap_or((1920, 1080));
    let mut tracks = HashSet::new();
    for p in &seq.placements {
        if let Some(v) = p.video_track {
            tracks.insert((StreamKind::Video, v));
        }
        for a in &p.audio_tracks {
            tracks.insert((StreamKind::Audio, *a));
        }
        for a in &p.grouped_audio {
            tracks.insert((StreamKind::Audio, a.track));
        }
    }
    for t in &seq.transitions {
        tracks.insert((t.kind, t.track));
    }
    for m in &seq.markers {
        if let Some(t) = m.track {
            tracks.insert((t.kind, t.index));
        }
    }
    seq.tracks = tracks
        .into_iter()
        .map(|(kind, index)| Track {
            kind,
            index,
            level_db: None,
        })
        .collect();
    seq.tracks
        .sort_by_key(|t| (if t.kind == StreamKind::Video { 0 } else { 1 }, t.index));
    for issue in &source.unresolved {
        unresolved.push(obj(vec![
            ("owner", s(&issue.owner)),
            ("reason", s(&issue.reason)),
            ("kind", Value::Int(issue.kind)),
            ("track", Value::Int(issue.track)),
            ("record_start", observed(&issue.record_start)),
            (
                "record_rate",
                issue
                    .record_rate
                    .map(|r| s(rational(r)))
                    .unwrap_or(Value::Null),
            ),
            ("route", routes(&issue.route)),
        ]));
        if issue.issue_kind == av::SourceIssueKind::RouteTiming {
            let lane = if issue.kind == KIND_PICTURE { "V" } else { "A" };
            loss(
                out,
                budget,
                id,
                crate::UNQUALIFIED_ROUTE_TIMING,
                format!("{lane}{}: {}", issue.track, issue.reason),
            )?;
        } else {
            loss(out, budget, id, "source_route", issue.reason.clone())?;
        }
    }
    let legacy = source
        .info
        .start
        .as_ref()
        .map(|t| {
            obj(vec![
                ("count", s(t.frames)),
                ("rounded_base", Value::Int(t.fps)),
                ("drop", Value::Bool(t.drop)),
            ])
        })
        .unwrap_or(Value::Null);
    let compatibility = source
        .info
        .start
        .as_ref()
        .and_then(|t| {
            (!t.drop && t.fps > 0)
                .then(|| Rational::new(t.frames as i128, t.fps as i128))
                .flatten()
        })
        .unwrap_or(Rational::ZERO);
    reserve_controls(source, budget)?;
    let member_witnesses = source
        .events
        .iter()
        .map(|e| {
            obj(vec![
                ("id", s(&e.id)),
                ("owner", s(&e.owner)),
                ("placement_id", opt_s(map.get(&e.id).map(String::as_str))),
                ("master", s(e.event.master.urn())),
                ("kind", Value::Int(e.event.kind)),
                ("track", Value::Int(e.event.track)),
                ("master_track", Value::Int(e.event.master_track)),
                (
                    "combined",
                    e.event
                        .combined
                        .map(|(g, ch)| Value::Arr(vec![s(g), s(ch)]))
                        .unwrap_or(Value::Null),
                ),
                (
                    "edit_gain_db",
                    e.event
                        .edit_gain_db
                        .map(Value::Float)
                        .unwrap_or(Value::Null),
                ),
                (
                    "edit_pan",
                    e.event.edit_pan.map(Value::Float).unwrap_or(Value::Null),
                ),
                (
                    "range",
                    e.range
                        .as_ref()
                        .map(|r| {
                            obj(vec![
                                ("record_start", Value::Int(r.record_start)),
                                ("record_length", Value::Int(r.length)),
                                ("record_rate", s(rational(r.record_rate))),
                                ("source_in", Value::Int(r.source_in)),
                                ("source_length", Value::Int(r.source_length)),
                                ("source_rate", s(rational(r.source_rate))),
                                ("route", routes(&r.route)),
                            ])
                        })
                        .unwrap_or(Value::Null),
                ),
                ("reason", opt_s(e.reason.as_deref())),
                ("via", Value::Arr(e.event.via.iter().map(s).collect())),
                ("controls", effects::receipt(&e.event)),
            ])
        })
        .collect();
    let native_transitions = source
        .transitions
        .iter()
        .map(|t| {
            obj(vec![
                ("id", s(&t.id)),
                ("kind", Value::Int(t.kind)),
                ("track", Value::Int(t.track)),
                ("position", observed(&t.position)),
                ("length", observed(&t.length)),
                ("cutpoint", observed(&t.cutpoint)),
                (
                    "rate",
                    t.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                ),
                ("effect", opt_s(t.effect.as_deref())),
                ("outgoing", binding_receipt(&t.outgoing)),
                ("incoming", binding_receipt(&t.incoming)),
            ])
        })
        .collect();
    sidecar(
        out,
        budget,
        "avid.source",
        id,
        obj(vec![
            ("legacy_clock", legacy),
            ("compatibility_start", s(compatibility)),
            (
                "compatibility_frame_size",
                Value::Arr(vec![
                    Value::Int(compatibility_frame_size.0),
                    Value::Int(compatibility_frame_size.1),
                ]),
            ),
            (
                "raster_derivation",
                s(if derived_raster.is_some() {
                    "compatibility fallback only: first retained video by record start and track; actual native sequence canvas unqualified"
                } else {
                    "compatibility fallback only: default 1920x1080; no retained video raster; actual native sequence canvas unqualified"
                }),
            ),
            (
                "extraction_issues",
                Value::Arr(source.info.problems.iter().map(s).collect()),
            ),
            (
                "audio_track_formats",
                Value::Arr(
                    source
                        .info
                        .audio_track_formats
                        .iter()
                        .map(|(a, b)| Value::Arr(vec![Value::Int(*a), Value::Int(*b)]))
                        .collect(),
                ),
            ),
            ("unresolved", Value::Arr(unresolved)),
            ("native_events", Value::Arr(member_witnesses)),
            (
                "combiner_channels",
                Value::Arr(source.combiner_channels.iter().map(s).collect()),
            ),
            ("native_transitions", Value::Arr(native_transitions)),
            ("marker_native", Value::Arr(marker_receipts)),
        ].into_iter().chain((!source.group_uses.is_empty()).then(||("native_group_uses",Value::Arr(source.group_uses.iter().map(group_use_native).collect())))).collect()),
    )?;
    for issue in &source.info.problems {
        out.diagnostics
            .push(format!("{:?}: {issue}", source.info.name));
    }
    budget.charge(
        size_of::<SourceSequence>()
            + seq.id.len()
            + seq.name.capacity()
            + seq.tracks.len() * size_of::<Track>(),
    )?;
    out.graph.sequences.push(seq);
    multicams.append_uses(source, &map, out, budget)?;
    Ok(())
}

fn sequence_marker_native(m: &av::SourceMarker) -> Value {
    let mut v = marker_native(&m.marker);
    if let Value::Obj(fields) = &mut v {
        fields.extend([
            ("occurrence".into(), s(&m.id)),
            ("owner".into(), s(&m.owner)),
            ("kind".into(), Value::Int(m.kind)),
            ("track".into(), Value::Int(m.track)),
            ("local_offset".into(), observed(&m.local_offset)),
            (
                "local_rate".into(),
                m.local_rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
            ),
            (
                "record_frame".into(),
                m.record_frame.map(Value::Int).unwrap_or(Value::Null),
            ),
            ("binding".into(), binding_receipt(&m.binding)),
        ]);
    }
    v
}
fn reserve_controls(source: &av::SourceSequence, budget: &Budget) -> Result<(), String> {
    // Temporary reservation checks peak growth; the resulting owned values are
    // charged once by sidecar(), after construction. No shared payload is charged twice.
    let mut reservation = Budget { used: budget.used };
    for e in &source.events {
        effects::reserve_receipt(&e.event, &mut reservation)?;
    }
    for t in &source.texts {
        reservation.charge(2048 + t.id.len() + t.content.as_ref().map_or(0, String::len) + t.reason.as_ref().map_or(0, String::len))?;
        for p in &t.native_properties {reservation.charge(1024+p.scope.len()+p.name.len()+p.value.len())?;}
    }
    for u in &source.group_uses {
        reservation.charge(4096+u.id.len()+u.reason.as_ref().map_or(0,String::len)+u.binding.reason.as_ref().map_or(0,String::len))?;
        reservation.charge(u.alternatives.len().checked_mul(4096).ok_or("group endpoint receipt overflow")?)?;
        for id in &u.binding.placements{reservation.charge(256+id.len())?;}
    }
    Ok(())
}
fn group_use_native(u:&av::SourceGroupUse)->Value{obj(vec![
    ("is_ganged",u.is_ganged.map(Value::Bool).unwrap_or(Value::Null)),
    ("id",s(&u.id)),("kind",Value::Int(u.kind)),("track",Value::Int(u.track)),
    ("position",u.position.map(Value::Int).unwrap_or(Value::Null)),("length",u.length.map(Value::Int).unwrap_or(Value::Null)),
    ("rate",u.rate.map(|r|s(rational(r))).unwrap_or(Value::Null)),("selected",u.selected.map(Value::Int).unwrap_or(Value::Null)),
    ("alternatives",Value::Arr(u.alternatives.iter().copied().map(super::multicam::endpoint).collect())),
    ("binding",binding_receipt(&u.binding)),("reason",opt_s(u.reason.as_deref())),
])}
fn raw_sequence(seq: &av::SourceSequence) -> Value {
    let mut value = obj(vec![
        ("id", s(&seq.id)),
        ("name", s(&seq.info.name)),
        ("native_rate", raw_float(seq.info.edit_rate)),
        ("length", Value::Int(seq.info.length)),
        ("texts", Value::Arr(seq.texts.iter().map(|t| obj(vec![
            ("id", s(&t.id)),
            ("kind", s(match t.kind { av::TextKind::SubCap => "SubCap", av::TextKind::TitlerPlus => "Titler+" })),
            ("track", Value::Int(t.track)),
            ("content", opt_s(t.content.as_deref())),
            ("record_start", t.record_start.map(Value::Int).unwrap_or(Value::Null)),
            ("length", t.length.map(Value::Int).unwrap_or(Value::Null)),
            ("rate", t.rate.map(|r| s(rational(r))).unwrap_or(Value::Null)),
            ("reason", opt_s(t.reason.as_deref())),
            ("native_properties", text_properties(t)),
        ])).collect())),
        (
            "combiner_channels",
            Value::Arr(seq.combiner_channels.iter().map(s).collect()),
        ),
        (
            "legacy_start",
            seq.info
                .start
                .as_ref()
                .map(|t| {
                    obj(vec![
                        ("count", Value::Int(t.frames)),
                        ("rounded_base", Value::Int(t.fps)),
                        ("drop", Value::Bool(t.drop)),
                    ])
                })
                .unwrap_or(Value::Null),
        ),
        (
            "events",
            Value::Arr(
                seq.events
                    .iter()
                    .map(|e| {
                        obj(vec![
                            ("id", s(&e.id)),
                            ("owner", s(&e.owner)),
                            ("kind", Value::Int(e.event.kind)),
                            ("track", Value::Int(e.event.track)),
                            ("master", s(e.event.master.urn())),
                            ("master_track", Value::Int(e.event.master_track)),
                            (
                                "range",
                                e.range
                                    .as_ref()
                                    .map(|r| {
                                        obj(vec![
                                            ("record_start", Value::Int(r.record_start)),
                                            ("record_rate", s(rational(r.record_rate))),
                                            ("record_length", Value::Int(r.length)),
                                            ("source_in", Value::Int(r.source_in)),
                                            ("source_length", Value::Int(r.source_length)),
                                            ("source_rate", s(rational(r.source_rate))),
                                            ("route", routes(&r.route)),
                                        ])
                                    })
                                    .unwrap_or(Value::Null),
                            ),
                            (
                                "speed",
                                e.event
                                    .speed
                                    .map(|(n, d)| Value::Arr(vec![Value::Int(n), Value::Int(d)]))
                                    .unwrap_or(Value::Null),
                            ),
                            (
                                "combined",
                                e.event
                                    .combined
                                    .map(|(g, ch)| Value::Arr(vec![s(g), s(ch)]))
                                    .unwrap_or(Value::Null),
                            ),
                            (
                                "gain_db",
                                e.event
                                    .edit_gain_db
                                    .map(Value::Float)
                                    .unwrap_or(Value::Null),
                            ),
                            (
                                "pan",
                                e.event.edit_pan.map(Value::Float).unwrap_or(Value::Null),
                            ),
                            ("via", Value::Arr(e.event.via.iter().map(s).collect())),
                            ("controls", effects::receipt(&e.event)),
                            ("reason", opt_s(e.reason.as_deref())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "markers",
            Value::Arr(
                seq.markers
                    .iter()
                    .map(|m| {
                        obj(vec![
                            ("id", s(&m.id)),
                            ("owner", s(&m.owner)),
                            ("kind", Value::Int(m.kind)),
                            ("track", Value::Int(m.track)),
                            ("name", s(&m.marker.name)),
                            ("comment", s(&m.marker.comment)),
                            ("user", s(&m.marker.user)),
                            ("color", s(&m.marker.color)),
                            ("native", marker_native(&m.marker)),
                            (
                                "length",
                                m.marker.length.map(Value::Int).unwrap_or(Value::Null),
                            ),
                            ("local_offset", observed(&m.local_offset)),
                            (
                                "local_rate",
                                m.local_rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                            ),
                            (
                                "record_frame",
                                m.record_frame.map(Value::Int).unwrap_or(Value::Null),
                            ),
                            ("binding", binding_receipt(&m.binding)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "transitions",
            Value::Arr(
                seq.transitions
                    .iter()
                    .map(|t| {
                        obj(vec![
                            ("id", s(&t.id)),
                            ("kind", Value::Int(t.kind)),
                            ("track", Value::Int(t.track)),
                            ("position", observed(&t.position)),
                            ("length", observed(&t.length)),
                            ("cutpoint", observed(&t.cutpoint)),
                            (
                                "rate",
                                t.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                            ),
                            ("effect", opt_s(t.effect.as_deref())),
                            ("outgoing", binding_receipt(&t.outgoing)),
                            ("incoming", binding_receipt(&t.incoming)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "issues",
            Value::Arr(
                seq.unresolved
                    .iter()
                    .map(|i| {
                        obj(vec![
                            ("owner", s(&i.owner)),
                            ("reason", s(&i.reason)),
                            ("route", routes(&i.route)),
                            ("record_start", observed(&i.record_start)),
                            (
                                "record_rate",
                                i.record_rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                            ),
                            ("kind", Value::Int(i.kind)),
                            ("track", Value::Int(i.track)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]);
    if !seq.group_uses.is_empty(){if let Value::Obj(fields)=&mut value{fields.push(("native_group_uses".into(),Value::Arr(seq.group_uses.iter().map(group_use_native).collect())));}}
    if seq.texts.is_empty() { if let Value::Obj(fields) = &mut value {fields.retain(|(key,_)|key!="texts");} }
    value
}
fn text_properties(t: &av::SourceText) -> Value {
    obj(vec![
        ("status", s("stored native values with owner/list witnesses; not resolved rendering or neutral geometry")),
        ("reason", opt_s(t.native_properties_reason.as_deref())),
        ("properties", Value::Arr(t.native_properties.iter().map(|p|obj(vec![
            ("scope", s(&p.scope)), ("list", s(p.list)), ("name", s(&p.name)), ("value", s(&p.value)),
        ])).collect())),
    ])
}
