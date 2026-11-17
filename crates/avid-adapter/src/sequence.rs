use super::*;
use avid_reader::source as av;
use sha2::{Digest, Sha256};
mod disabled;
mod nested_motion;

fn portable_track_name(track: &av::SourceTrackName) -> Option<&str> {
    (!track.ambiguous && !track.name.chars().any(char::is_control))
        .then_some(track.name.as_str())
}

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
pub(super) struct Mp3Timing {
    pub delay_samples: i64,
    pub sample_rate: i64,
    pub extent: Rational,
}

pub(super) struct ItemRoutes {
    pub audio: HashSet<i64>,
    pub dry_audio: HashSet<i64>,
    pub linked_audio_render: Option<(MobId, MobId)>,
    pub subclip_pcm: HashMap<i64, Rational>,
    pub raster: Option<(i64, i64)>,
    pub square_pixels: bool,
    pub fitted_png: bool,
    pub strip_tiff: bool,
    pub motion_picture: Option<Rational>,
    pub motion_audio: HashMap<i64, (i64, Rational)>,
    pub managed_motion_audio: HashMap<i64, (i64, Rational)>,
    pub pcm_prefix: Option<av::SourcePcmPrefix>,
    pub same_rate_aac: HashMap<i64, Rational>,
    pub mp3_timing: HashMap<i64, Mp3Timing>,
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

#[cfg(test)]
#[test]
fn managed_audio_adapter_sample_starts_survive_phase_wrap() {
    let ratio = (1001, 1000);
    for anchor in [204_i64, 1000, 1001, 1385, 1870, 2002] {
        let source_frame = anchor * 1000 / 1001;
        let expected_sample = i128::from(anchor) * 2000;
        for phase in [anchor, anchor % 1001] {
            assert_eq!(adapter_audio_in_samples(source_frame, phase, 0, ratio, 2002), expected_sample);
        }
    }
}

fn motion_timing(
    event: &av::SourceEvent, range: &av::QualifiedRange, phase: i64, item: &ItemRoutes,
    covering_picture: bool, p: &mut Placement, budget: &mut Budget,
) -> Result<(i64, Option<i64>), String> {
    let span_ratio = av::Rate::automatic_adapter_span(range.source_rate, range.record_rate)
        .ok_or("requires a qualified automatic adapter rate pair")?;
    if event.event.combined.is_some()
        || !event.event.via.is_empty()
        || event.picture_extension.is_some() || event.event.picture.is_some()
        || !event.picture_chain.is_empty()
        || event.event.animatte.is_some() || event.event.color.is_some()
    { return Err("requires an unstacked automatic adapter".into()); }
    let source_end = time(range.source_in.checked_add(range.source_length)
        .ok_or("source extent overflow")?, range.source_rate)?;
    if let Some(&(bits, extent)) = item.managed_motion_audio.get(&event.event.master_track)
        .filter(|_| event.event.kind == KIND_SOUND)
    {
        let unit = match (bits, span_ratio) {
            (16, (1000, 1001)) if event.volume_automation.is_none() => 2000,
            (24, (1001, 1000)) if range.length.checked_add(1) == Some(range.source_length)
                && !matches!(event.volume_automation, Some(av::VolumeAutomation::Unsupported(_))) => 2002,
            _ => return Err("requires a qualified managed mono PCM rate pair and Pan Volume envelope".into()),
        };
        if source_end > extent
            || event.pan_automation.is_some()
        {
            return Err("requires a managed mono PCM route with verified source coverage and static pan".into());
        }
        let first = adapter_audio_in_samples(range.source_in, phase, 0, span_ratio, unit);
        let start = Rational::new(first, 48_000).ok_or("managed audio origin overflow")?;
        if start.is_negative() || start.checked_add(p.duration).is_none_or(|end| end > extent) {
            return Err("managed PCM edit exceeds the verified file sample extent".into());
        }
        p.source_in = start;
        if event.motion_adapter_center_pan {
            p.track_pans = vec![Some(0.0)];
        }
        return Ok((0, Some(bits)));
    }
    if event.volume_automation.is_some()
        || (event.event.kind == KIND_SOUND && span_ratio == (1001, 1000) && range.source_length != range.length)
    {
        return Err("requires verified managed PCM24 for this source span or volume envelope".into());
    }
    let picture = item.motion_picture.ok_or("requires qualified linked picture or a supported managed mono PCM route")?;
    if item.motion_audio.len() != 2 { return Err("requires two proven linked mono lanes".into()); }
    if source_end > picture {
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
        return Ok((0, None));
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
    Ok((tail as i64, None))
}

fn apply_saved_timewarp(
    frames: &[i64], range: &av::QualifiedRange, item: &ItemRoutes,
    p: &mut Placement, budget: &mut Budget,
) -> Result<bool, String> {
    let rate = range.record_rate;
    let profile = match (rate.num, rate.den, range.length, range.source_length) {
        (24, 1, 48, 59) => range.source_in >= 0,
        (24, 1, 3..=4096, _) if range.length != 48 => range.source_in >= 0,
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
    assert_eq!(adapter_audio_in_samples(68, 68, 0, forward, 2000), 136_136);
    assert_eq!(adapter_audio_in_samples(65, 65, 0, forward, 2000), 130_130);
    assert_eq!([0, 952, 953, 954, 1103].map(|n| adapter_picture_frame(48, 49, n, reverse)),
        [48, 1000, 1000, 1001, 1150]);
    let start = adapter_audio_in_samples(48, 49, 1, reverse, 2002);
    assert_eq!(start, 100_002);
    assert_eq!(1104 * 2000 - (2_306_304 - start), 1698);
}
fn subclip_motion_receipt(motion: &av::SourceSubclipMotion) -> Value {
    let mut saved = vec![
        ("effect_id", s(if motion.advanced { "EFF_ADV_MOTION_CTL" } else { "EFF_TIMEWARP.MOTION_CTL" })),
        ("input_mob", s(motion.input_mob.urn())),
        ("input_track", Value::Int(motion.input_track)),
        ("input_start", Value::Int(motion.input_start)),
        ("input_length", Value::Int(motion.input_length)),
        ("length", Value::Int(motion.length)),
        ("span_ratio", Value::Arr(vec![Value::Int(motion.span_ratio.0), Value::Int(motion.span_ratio.1)])),
        ("speed", Value::Float(motion.speed)),
        ("speed_key_frame", Value::Arr(vec![Value::Int(motion.speed_key_frame.0), Value::Int(motion.speed_key_frame.1)])),
        ("position_start", Value::Float(motion.position[0])),
        ("position_end", Value::Float(motion.position[1])),
        ("phase_offset", Value::Int(motion.phase_offset)),
        ("offset_adjust", Value::Float(motion.offset_adjust)),
        ("render_mode", Value::Int(motion.render_mode)),
        ("ignore_render_setting", Value::Int(motion.ignore_render_setting)),
        ("new_source_calculation", Value::Bool(motion.new_source_calculation)),
        ("graph_mode", motion.graph_mode.map_or(Value::Null, Value::Int)),
    ];
    if let Some(samples) = motion.sampled_input_frames {
        let (num, den) = samples.ratio;
        let mut cadence = vec![("first", Value::Int(samples.first)), ("last", Value::Int(samples.last))];
        if den == 1 {
            cadence.push(("step", Value::Int(num)));
        } else {
            cadence.push(("source_frames_per_record_frame", Value::Arr(vec![Value::Int(num), Value::Int(den)])));
            cadence.push(("start_phase", Value::Arr(vec![Value::Int(samples.phase), Value::Int(den)])));
        }
        saved.push(("sampled_input_frames", obj(cadence)));
    }
    if let Some(file) = motion.sampled_picture_file {
        saved.push(("sampled_picture_file", s(file.urn())));
    }
    if let Some(master) = &motion.sampled_picture_master {
        saved.push(("sampled_picture_master", obj(vec![
            ("mob", s(master.mob.urn())),
            ("track", Value::Int(master.track)),
            ("source_in", Value::Int(master.source_in)),
        ])));
    }
    obj(saved)
}

fn extend_effect_issue<'a>(
    issue: &av::SourceIssue, fields: &mut Vec<(&str, Value)>,
    resolve_item: impl Fn(MobId) -> Option<&'a str>,
    color_placement: Option<&str>,
) {
    let (id, length) = match issue.issue_kind {
        av::SourceIssueKind::MatteKey { length } => ("EFF2_2DMATTE", length),
        av::SourceIssueKind::ColorAdapter { length } => ("EFF2_LUTSFX", length),
        av::SourceIssueKind::LayeredSubmaster { length, .. } => ("EFF_SUBMASTER", length),
        _ => return,
    };
    fields.push(("effect_id", s(id)));
    fields.push(("record_length", length.map(Value::Int).unwrap_or(Value::Null)));
    if let Some(adapter) = &issue.color_adapter {
        let mut facts = vec![
            ("master", s(adapter.master.urn())),
            ("master_track", Value::Int(adapter.master_track)),
            ("source_in", Value::Int(adapter.source_in)),
            ("text", s(&adapter.transform)),
            ("placement", opt_s(color_placement)),
            ("neutral_wrapper", Value::Bool(adapter.neutral_wrapper)),
        ];
        match &adapter.picture_file {
            Observed::Known(mob) => facts.push(("picture_file", s(mob.urn()))),
            Observed::Unresolved(reason) => facts.push(("source_match_reason", s(reason))),
        }
        if let Some(range) = av::SourceColorRange::from_saved_xml(&adapter.transform) {
            facts.push(("linear_range", super::color_range_receipt(range)));
        }
        if let Some(color) = adapter.wrapper_clip_color {
            facts.push(("wrapper_clip_color_rgb16", Value::Arr(
                color.into_iter().map(|value| Value::Int(i64::from(value))).collect(),
            )));
        }
        if let Some((offset, length)) = adapter.managed_picture_span {
            facts.push(("managed_picture_span", obj(vec![
                ("offset", Value::Int(offset)),
                ("length", Value::Int(length)),
            ])));
        }
        if adapter.centered_still_fit {
            facts.push(("centered_still_fit", Value::Bool(true)));
        }
        if let Some(master) = &adapter.subclip_master {
            facts.push(("subclip_master", obj(vec![
                ("mob", s(master.mob.urn())),
                ("track", Value::Int(master.track)),
                ("source_in", Value::Int(master.source_in)),
            ])));
        }
        if let Some(motion) = &adapter.subclip_motion {
            facts.push(("subclip_motion", subclip_motion_receipt(motion)));
        }
        fields.push(("color_adapter", obj(facts)));
    }
    if let av::SourceIssueKind::LayeredSubmaster { input_count, .. } = issue.issue_kind {
        let class = |class: Option<[u8; 4]>| class
            .map(|tag| s(String::from_utf8_lossy(&tag))).unwrap_or(Value::Null);
        fields.extend([
            ("input_count", Value::Int(input_count as i64)),
            ("submaster_inputs", Value::Arr(issue.submaster_inputs.iter().map(|input| {
                let mut layer = vec![
                    ("track", input.track.map(Value::Int).unwrap_or(Value::Null)),
                    ("component_class", class(input.component_class)),
                    ("segment_class", class(input.segment_class)),
                    ("segment_effect", opt_s(input.segment_effect.as_deref())),
                ];
                if let Some(source) = input.picture_source {
                    let mut reference = vec![
                        ("mob", s(source.mob.urn())),
                        ("track", Value::Int(source.track)),
                        ("source_start", Value::Int(source.source_start)),
                        ("local_start", Value::Int(source.local_start)),
                        ("length", Value::Int(source.length)),
                        ("local_length", Value::Int(source.local_length)),
                        ("rate", s(rational(source.rate))),
                    ];
                    if issue.picture_profile == Some(av::SourcePictureProfile::PlainLayeredSubmaster) {
                        reference.push(("item", opt_s(resolve_item(source.mob))));
                    }
                    layer.push(("picture_source", obj(reference)));
                }
                obj(layer)
            }).collect())),
        ]);
    }
    if let Some(profile) = issue.picture_profile {
        fields.push(("picture_profile", s(match profile {
            av::SourcePictureProfile::ImportedPngMatte => "imported_png_matte_controls",
            av::SourcePictureProfile::EmptyColorAdapter => "empty_color_adapter_controls",
            av::SourcePictureProfile::PlainLayeredSubmaster => "plain_layered_submaster",
        })));
    }
    if let Some(inputs) = issue.matte_inputs.as_deref() {
        let reference = |input: Option<av::MatteInput>| input.map(|input| obj(vec![
            ("mob", s(input.mob.urn())),
            ("track", Value::Int(input.track)),
            ("start", Value::Int(input.start)),
            ("length", Value::Int(input.length)),
            ("rate", s(rational(input.rate))),
        ])).unwrap_or(Value::Null);
        fields.push(("matte_inputs", obj(vec![
            ("background", s(match inputs.background {
                av::MatteBackground::LowerTrack => "lower_track",
                av::MatteBackground::Explicit => "explicit",
                av::MatteBackground::Unresolved => "unresolved",
            })),
            ("fill", reference(inputs.fill)),
            ("matte", reference(inputs.matte)),
        ])));
    }
    if let Some(import) = &issue.matte_import {
        let flag = |value: Option<bool>| value.map(Value::Bool).unwrap_or(Value::Null);
        fields.push(("matte_import", obj(vec![
            ("ignore_alpha", flag(import.ignore_alpha)),
            ("invert_alpha", flag(import.invert_alpha)),
            ("saved_locators", Value::Arr(import.locators.iter().map(s).collect())),
        ])));
    }
}

fn source_issue_receipt<'a>(
    issue: &av::SourceIssue, resolve_item: impl Fn(MobId) -> Option<&'a str>,
    color_placement: Option<&str>,
) -> Value {
    let mut fields = vec![
        ("owner", s(&issue.owner)),
        ("reason", s(&issue.reason)),
        ("kind", Value::Int(issue.kind)),
        ("track", Value::Int(issue.track)),
        ("record_start", observed(&issue.record_start)),
        ("record_rate", issue.record_rate.map(|r| s(rational(r))).unwrap_or(Value::Null)),
        ("route", routes(&issue.route)),
    ];
    if let av::SourceIssueKind::DisabledClip { length, backing_class, plain_mono, plain_stereo, neutral_picture } = issue.issue_kind {
        fields.extend([
            ("disabled_clip", Value::Bool(true)),
            ("record_length", Value::Int(length)),
            ("backing_class", s(String::from_utf8_lossy(&backing_class))),
        ]);
        if plain_mono {
            fields.push(("plain_mono_backing", Value::Bool(true)));
        }
        if plain_stereo {
            fields.push(("plain_stereo_backing", Value::Bool(true)));
        }
        if let Some(package) = neutral_picture {
            fields.push(("neutral_picture_file", s(package.umid_hex())));
        }
    }
    if let av::SourceIssueKind::Timewarp { length } = issue.issue_kind {
        fields.extend([("timewarp", Value::Bool(true)), ("record_length", Value::Int(length))]);
    }
    if let av::SourceIssueKind::UntranslatedEffect { effect, length } = issue.issue_kind {
        fields.extend([("effect", s(effect)), ("record_length", Value::Int(length))]);
    }
    if let Some(controls) = issue.timewarp.as_deref() {
        fields.push(("timewarp_controls", timewarp_controls(controls)));
        if let Some(owner) = &controls.resize_render_owner {
            fields.push(("resize_render_owner", s(owner)));
        }
    }
    if let Some(motion) = issue.nested_picture_motion.as_deref() {
        fields.push(("nested_picture_motion", subclip_motion_receipt(motion)));
    }
    extend_effect_issue(issue, &mut fields, resolve_item, color_placement);
    obj(fields)
}

fn timewarp_controls(controls: &av::SourceTimewarpControls) -> Value {
    let integer = |v: Option<i64>| v.map_or(Value::Null, Value::Int);
    let float = |v: Option<f64>| v.map_or(Value::Null, Value::Float);
    let flag = |v: Option<bool>| v.map_or(Value::Null, Value::Bool);
    let pair = |(num, den)| Value::Arr(vec![Value::Int(num), Value::Int(den)]);
    let input = |input: &av::TimewarpInput| obj(vec![
        ("mob", s(input.mob.urn())),
        ("track", integer(input.track)),
        ("start", integer(input.start)),
        ("length", integer(input.length)),
        ("rate", input.rate.map(|rate| s(rational(rate))).unwrap_or(Value::Null)),
    ]);
    let curves = controls.curves.iter().map(|curve| {
        let points = match &curve.points {
            Some(av::SourceTimewarpPoints::HalfFrameSamples { first, values }) => obj(vec![
                ("first_half_frame", Value::Int(*first)),
                ("values", Value::Arr(values.iter().copied().map(Value::Float).collect())),
            ]),
            Some(av::SourceTimewarpPoints::Keys(keys)) => Value::Arr(keys.iter().map(|key| {
                obj(vec![
                    ("time", pair(key.time)),
                    ("timescale", Value::Int(key.timescale)),
                    ("value", Value::Float(key.value)),
                    ("handles", Value::Arr(key.handles.into_iter().map(float).collect())),
                    ("anchor", float(key.anchor)),
                    ("flags", integer(key.flags)),
                ])
            }).collect()),
            None => Value::Null,
        };
        obj(vec![
            ("parameter", s(curve.parameter.to_string())),
            ("enabled", flag(curve.enabled)),
            ("interpolation", integer(curve.interpolation)),
            ("extrapolation", integer(curve.extrapolation)),
            ("fields", integer(curve.fields)),
            ("point_count", s(curve.point_count)),
            ("points", points),
        ])
    }).collect();
    obj(vec![
        ("span_ratio", controls.span_ratio.map(pair).unwrap_or(Value::Null)),
        ("phase_offset", integer(controls.phase_offset)),
        ("offset_adjust", float(controls.offset_adjust)),
        ("render_mode", integer(controls.render_mode)),
        ("ignore_render_setting", integer(controls.ignore_render_setting)),
        ("render_mode_name", opt_s(controls.explicit_fluid_motion())),
        ("graph_mode", integer(controls.graph_mode)),
        ("input_format", integer(controls.input_format)),
        ("output_format", integer(controls.output_format)),
        ("new_source_calculation", flag(controls.new_source_calculation)),
        ("input", controls.input.as_ref().map(input).unwrap_or(Value::Null)),
        ("precompute", controls.precompute.as_ref().map(input).unwrap_or(Value::Null)),
        ("precompute_present", Value::Bool(controls.precompute_present)),
        ("curves", Value::Arr(curves)),
    ])
}

type ColorEventIndex<'a> = HashMap<(i64, i64), (usize, Option<&'a av::SourceEvent>)>;

fn source_color_events<'a>(
    source: &'a av::SourceSequence, budget: &mut Budget,
) -> Result<ColorEventIndex<'a>, String> {
    let mut events = HashMap::new();
    for issue in &source.unresolved {
        if let (Some(adapter), Observed::Known(frame)) = (&issue.color_adapter, &issue.record_start) {
            if !matches!(adapter.picture_file, Observed::Known(_)) {
                continue;
            }
            budget.charge(96)?;
            events.entry((issue.track, *frame)).or_insert((0usize, None));
        }
    }
    if events.is_empty() {
        return Ok(events);
    }
    for event in &source.events {
        if event.event.kind != KIND_PICTURE {
            continue;
        }
        if let Some(range) = &event.range {
            if let Some((count, matched)) = events.get_mut(&(event.event.track, range.record_start)) {
                *count += 1;
                *matched = (*count == 1).then_some(event);
            }
        }
    }
    Ok(events)
}

fn color_adapter_event<'a>(
    issue: &av::SourceIssue, events: &ColorEventIndex<'a>,
) -> Option<&'a av::SourceEvent> {
    let adapter = issue.color_adapter.as_ref()?;
    let Observed::Known(_) = adapter.picture_file else { return None };
    let Observed::Known(start) = issue.record_start else { return None };
    let av::SourceIssueKind::ColorAdapter { length: Some(length) } = issue.issue_kind else {
        return None;
    };
    let (1, Some(event)) = events.get(&(issue.track, start))? else { return None };
    let range = event.range.as_ref()?;
    let (master, master_track, source_in) = if let Some(master) = &adapter.subclip_master {
        let [subclip, backing] = range.route.as_slice() else { return None };
        let Observed::Known(backing_in) = backing.source_offset else { return None };
        if subclip.mob != adapter.master || subclip.track != adapter.master_track
            || subclip.source_offset != Observed::Known(adapter.source_in)
            || subclip.length != Observed::Known(length)
            || subclip.rate != Some(range.source_rate) || backing.rate != subclip.rate
            || backing.mob != master.mob || backing.track != master.track
            || backing_in.checked_add(adapter.source_in) != Some(master.source_in)
        {
            return None;
        }
        (master.mob, master.track, master.source_in)
    } else {
        (adapter.master, adapter.master_track, adapter.source_in)
    };
    if !event.owner.strip_prefix(&issue.owner).is_some_and(|tail| tail.starts_with("/component/"))
        || event.event.master != master || event.event.master_track != master_track
        || range.source_in != source_in || range.length != length || range.source_length != length
        || Some(range.record_rate) != issue.record_rate || range.source_rate != range.record_rate
        || event.event.speed.is_some() || event.motion_adapter_phase.is_some() || event.saved_timewarp.is_some()
    {
        return None;
    }
    Some(event)
}

fn append_media_gap(
    source: &av::SourceSequence,
    sequence: &mut SourceSequence,
    event: &av::SourceEvent,
    master: &av::SourceMaster,
    reason: &str,
    unresolved: &mut Vec<Value>,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<Option<(StreamKind, i64)>, String> {
    let native = &event.event;
    let lane = if native.kind == KIND_PICTURE { "V" } else { "A" };
    let description = format!("{:?} · {lane}{} · {:?} · {}",
        source.info.name, native.track, master.master.name, event.id);
    let range = event.range.as_ref();
    let facts = range.map(|r| obj(vec![
        ("record_start", Value::Int(r.record_start)),
        ("record_length", Value::Int(r.length)),
        ("record_rate", s(rational(r.record_rate))),
        ("source_in", Value::Int(r.source_in)),
        ("source_length", Value::Int(r.source_length)),
        ("source_rate", s(rational(r.source_rate))),
        ("route", routes(&r.route)),
    ])).unwrap_or(Value::Null);
    budget.charge(event.id.len() + event.owner.len() + master.id.len() + reason.len() + 512)?;
    unresolved.push(obj(vec![
        ("occurrence", s(&event.id)), ("owner", s(&event.owner)),
        ("omitted_master", s(&master.id)), ("master_track", Value::Int(native.master_track)),
        ("track_kind", Value::Int(native.kind)), ("track", Value::Int(native.track)),
        ("saved_range", facts), ("reason", s(reason)),
    ]));
    let Some((kind, range, end)) = kind(native.kind).zip(range)
        .and_then(|(kind, r)| r.record_start.checked_add(r.length).map(|end| (kind, r, end)))
        .filter(|(_, r, end)| native.track > 0 && native.combined.is_none()
            && r.record_start >= 0 && r.length > 0 && *end <= source.info.length
            && Some(r.record_rate) == source.rate)
    else {
        loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING,
            format!("{description}: media gap requires one verified record span and lane. {reason}"))?;
        return Ok(None);
    };
    let start = time(range.record_start, range.record_rate)?;
    let duration = time(range.length, range.record_rate)?;
    let end_time = time(end, range.record_rate)?;
    let other_edit = source.events.iter().any(|other| {
        other.id != event.id && other.event.kind == native.kind && other.event.track == native.track
            && other.range.as_ref().is_none_or(|r| r.record_rate != range.record_rate
                || r.record_start.checked_add(r.length).is_none_or(|last| r.record_start < end && last > range.record_start))
    });
    let overlaps_transition = source.transitions.iter().any(|t| {
        if t.kind != native.kind || t.track != native.track { return false; }
        match (&t.position, &t.length) {
            (Observed::Known(at), Observed::Known(len)) if t.rate == Some(range.record_rate) =>
                at.checked_add(*len).is_none_or(|last| *at < end && last > range.record_start),
            _ => true,
        }
    });
    let overlaps_placement = sequence.placements.iter().chain(&sequence.disabled_placements).any(|p| {
        let same_lane = match kind {
            StreamKind::Video => p.video_track == Some(native.track),
            StreamKind::Audio => p.audio_tracks.contains(&native.track)
                || p.grouped_audio.iter().any(|group| group.track == native.track),
        };
        same_lane && p.record_start < end_time
            && p.record_start.checked_add(p.duration).is_none_or(|last| last > start)
    });
    if other_edit || overlaps_transition || overlaps_placement {
        loss(out, budget, &sequence.id, UNQUALIFIED_ROUTE_TIMING,
            format!("{description}: media gap overlaps an edit or transition. {reason}"))?;
        return Ok(None);
    }
    let name = format!("Media omitted · {lane}{}", native.track);
    let comment = format!("{description} at record frame {} for {} frames. {reason} This span is a gap.",
        range.record_start, range.length);
    budget.charge(size_of::<SourceMarker>() + name.len() + comment.len() + event.id.len() + 64)?;
    sequence.markers.push(SourceMarker {
        name, comment: comment.clone(), user: String::new(), color: None, start: Some(start), duration,
        track: None, owner: MarkerOwner::Sequence, owner_occurrence: event.id.clone(),
        owner_local: Some(start), owner_rate: Some(sequence.rate),
        binding: SourceBinding { placements: vec![], reason: None }, source: None, inherited: false,
    });
    loss(out, budget, &sequence.id, "media_gap", comment)?;
    Ok(Some((kind, native.track)))
}

pub(super) fn append(
    source: &av::SourceSequence,
    copies: &HashMap<avid_reader::io::MobId, Vec<(usize, usize)>>,
    item_routes: &[ItemRoutes],
    multicams: &super::multicam::Index<'_>,
    fallbacks: &effect_fallback::Verified,
    missing_pcm_prefix: &HashSet<avid_reader::io::MobId>,
    omitted_media: &MediaOmissions<'_>,
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
        budget.charge(issue.reason.len() + issue.owner.len() + issue.route.len() * 1024
            + if issue.matte_inputs.is_some() { 1024 } else { 0 }
            + if issue.picture_profile.is_some() { 128 } else { 0 })?;
        if !issue.submaster_inputs.is_empty() {
            budget.charge(issue.submaster_inputs.iter()
                .map(|input| 512 + input.segment_effect.as_ref().map_or(0, String::len))
                .sum::<usize>())?;
        }
        if let Some(import) = &issue.matte_import {
            budget.charge(512 + import.locators.iter().map(|p| p.len() + 64).sum::<usize>())?;
        }
        if let Some(adapter) = &issue.color_adapter {
            budget.charge(1024 + adapter.transform.len())?;
            if let Observed::Unresolved(reason) = &adapter.picture_file {
                budget.charge(reason.len())?;
            }
        }
        if issue.nested_picture_motion.is_some() {
            budget.charge(2048)?;
        }
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
            drop: c.drop,
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
            disabled_placements: Vec::new(),
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
            let mut reservation = *budget;
            reservation.charge(2048 + t.native_properties_reason.as_ref().map_or(0, String::len))?;
            for p in &t.native_properties { reservation.charge(1024 + p.scope.len() + p.name.len() + p.value.len())?; }
            sidecar(out, budget, "avid.text", &t.id, text_properties(t))?;
        }
        seq.texts.push(text);
    }
    let mut map = HashMap::new();
    let mut shifted_mp3_edits = HashSet::new();
    let mut groups: HashMap<usize, Vec<(usize, &av::SourceEvent)>> = HashMap::new();
    let mut original: HashMap<usize, HashSet<usize>> = HashMap::new();
    let mut unresolved = vec![];
    let mut media_gaps = Vec::new();
    for event in &source.events {
        if let Some((g, ch)) = event.event.combined {
            original.entry(g).or_default().insert(ch);
        }
    }
    let mut derived_raster: Option<(i64, i64, i64, i64)> = None;
    let color_events = source_color_events(source, budget)?;
    let mut centered_stills = HashSet::new();
    for issue in &source.unresolved {
        if issue.color_adapter.as_ref().is_some_and(|a| a.centered_still_fit) {
            if let Some(event) = color_adapter_event(issue, &color_events) {
                budget.charge(64)?;
                centered_stills.insert(event.id.as_str());
            }
        }
    }
    let mut uses_centered_still = false;
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
        if !copies.contains_key(&e.master) {
            if let Some(omitted) = omitted_media.get(&e.master) {
                let (master, reason) = omitted.iter()
                    .find(|(m, _)| m.master.bin == source.info.bin).unwrap_or(&omitted[0]);
                budget.charge(48)?;
                media_gaps.push((event, *master, reason.as_ref()));
                continue;
            }
        }
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
        if event.subclip_center_pan_count > 0 {
            let controls = match event.subclip_center_pan_count {
                1 => "saved subclip pan needs".into(),
                count => format!("{count} saved subclip pan controls need"),
            };
            loss(out, budget, id, "subclip_audio_mix",
                format!("A{} at record frame {}, source A{}: {controls} separate mix validation. The timeline-level controls are preserved.",
                    e.track, range.record_start, e.master_track))?;
        }
        let dry_alternative = fallbacks.alternative_gain_input(event);
        let dry_equalizer = fallbacks.alternative_equalizer_input(event);
        let (master, master_track, source_in) = dry_equalizer
            .or_else(|| dry_alternative.map(|(mob, start)| (mob, 1, start)))
            .unwrap_or((e.master, e.master_track, range.source_in));
        let dry_gain_bypass = event.dry_gain_bypass || dry_alternative.is_some();
        let equalizer = event.equalizer.as_deref();
        let audiosuite = event.audiosuite.as_deref();
        let eq_with_pitch = equalizer.is_some_and(|eq| eq.pitch.is_some());
        let eq_label = if eq_with_pitch { "Avid Pitch II and legacy EQ" } else { "Avid legacy EQ" };
        let rendered_equalizer = equalizer.and_then(|eq| eq.render).filter(|_| dry_equalizer.is_none());
        let dry_audio = if dry_gain_bypass {
            Some(("AudioSuite Gain", "gain_bypass", true))
        } else {
            equalizer.filter(|_| rendered_equalizer.is_none())
                .map(|eq| ("Avid legacy EQ", "equalizer_bypass", eq.enabled))
        };
        let timing_aspect = if audiosuite.is_some() {
            "audiosuite_unavailable"
        } else if event.motion_adapter_phase.is_some() || event.saved_timewarp.is_some()
            || dry_gain_bypass || event.gain_dry_input.is_some() {
            crate::UNQUALIFIED_ROUTE_TIMING
        } else if equalizer.is_some() {
            "equalizer_unavailable"
        } else { "source_route" };
        let Some(candidates) = copies.get(&master) else {
            if e.kind == KIND_SOUND && missing_pcm_prefix.contains(&master) {
                loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING, format!(
                    "A{} at record frame {} for {} frames: the PCM section needs both verified mono MXF siblings",
                    e.track, range.record_start, range.length
                ))?;
                continue;
            }
            let reason = if let Some(render) = audiosuite {
                unavailable_audiosuite(render, e.track, range.record_start, range.length)
            } else if rendered_equalizer.is_some() {
                format!("{}: {eq_label} on A{} at record frame {} for {} frames needs its verified linked PCM24 cache. The audio edit is omitted.",
                    event.id, e.track, range.record_start, range.length)
            } else if equalizer.is_some() {
                format!("{}: Avid legacy EQ on A{} at record frame {} for {} frames has no linked dry master. The audio edit is omitted.",
                    event.id, e.track, range.record_start, range.length)
            } else {
                format!("{}: master clip was not converted", event.id)
            };
            loss(out, budget, id, timing_aspect, reason)?;
            continue;
        };
        let index = candidates
            .iter()
            .find(|(bin, _)| *bin == source.info.bin)
            .unwrap_or(&candidates[0])
            .1;
        if let Some(render) = audiosuite {
            if item_routes[index].linked_audio_render != Some((render.file, render.origin))
                || !crate::effect_fallback::audio_render_matches(event, render.start, render.frames)
            {
                loss(out, budget, id, timing_aspect,
                    unavailable_audiosuite(render, e.track, range.record_start, range.length))?;
                continue;
            }
            loss(out, budget, &event.id, "audiosuite_render", format!(
                "{} · A{} at record frame {} for {} frames: linked the verified Avid AudioSuite {} render. The plug-in controls are baked into the audio.",
                source.info.name, e.track, range.record_start, range.length, render.name
            ))?;
        }
        if let Some(render) = rendered_equalizer {
            if item_routes[index].linked_audio_render != Some((render.file, render.origin))
                || !crate::effect_fallback::equalizer_render_matches(event, render)
            {
                loss(out, budget, id, "equalizer_unavailable", format!(
                    "{}: {eq_label} on A{} at record frame {} for {} frames needs its verified linked PCM24 cache. The audio edit is omitted.",
                    event.id, e.track, range.record_start, range.length
                ))?;
                continue;
            }
            let render_label = if eq_with_pitch { "Avid Pitch II and EQ" } else { "Avid EQ" };
            let controls = if eq_with_pitch { "Pitch II and EQ" } else { "EQ" };
            loss(out, budget, &event.id, "equalizer_render", format!(
                "{} · A{} at record frame {} for {} frames: linked the verified {render_label} render. The {controls} controls are unavailable for editing.",
                source.info.name, e.track, range.record_start, range.length
            ))?;
        }
        if let Some(prefix) = item_routes[index].pcm_prefix.filter(|_| e.kind == KIND_SOUND) {
            let rate = av::Rate::new(24, 1).unwrap();
            if range.source_rate != rate || range.record_rate != rate
                || source_in < 0 || range.source_length != range.length
                || source_in.checked_add(range.source_length).is_none_or(|end| end > prefix.frames)
                || e.speed.is_some() || event.motion_adapter_phase.is_some()
            {
                loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING, format!(
                    "A{} at record frame {} for {} frames: edit exceeds the verified PCM section or uses unsupported timing across the MPEG audio tail",
                    e.track, range.record_start, range.length
                ))?;
                continue;
            }
        }
        let stream = (e.kind == KIND_SOUND && item_routes[index].audio.contains(&master_track))
            .then_some(master_track);
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
        if event.subclip_gain.is_some() {
            let exact_rate = av::Rate::new(24, 1).unwrap();
            let source_end = source_in.checked_add(range.source_length)
                .and_then(|end| time(end, range.source_rate).ok());
            let extent = item_routes[index].subclip_pcm.get(&master_track).copied();
            let verified = e.kind == KIND_SOUND && e.combined.is_none() && e.speed.is_none()
                && event.motion_adapter_phase.is_none() && source_in >= 0
                && range.record_rate == exact_rate && range.source_rate == exact_rate
                && range.source_length == range.length
                && source_end.zip(extent).is_some_and(|(end, extent)| end <= extent);
            if !verified {
                loss(out, budget, id, "subclip_audio_mix", format!(
                    "A{} at record frame {} for {} frames: the saved subclip gain needs a verified same-rate mono PCM24 source. The audio edit is omitted. Source occurrence {}.",
                    e.track, range.record_start, range.length, event.id
                ))?;
                continue;
            }
            loss(out, budget, &event.id, "subclip_audio_mix", format!(
                "A{} at record frame {} for {} frames: used the original PCM with the independent timeline gain and pan. Avid playback bypassed the subclip gain in the tested profile.",
                e.track, range.record_start, range.length
            ))?;
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
        if let Some((effect, aspect, enabled)) = dry_audio {
            let source_end = source_in.checked_add(range.source_length)
                .and_then(|n| time(n, range.source_rate).ok());
            let extent = fallbacks.dry_audio.get(&(master, master_track)).map(|(extent, _)| extent);
            if e.combined.is_some() || source_in < 0 || !item_routes[index].dry_audio.contains(&master_track)
                || range.source_rate != range.record_rate || range.length != range.source_length
                || source_end.zip(extent).is_none_or(|(end, extent)| end > *extent)
            {
                loss(out, budget, id, timing_aspect,
                    format!("{}: {effect} dry input on A{} at record frame {} lacks verified sample coverage",
                        event.id, e.track, range.record_start))?;
                continue;
            }
            if enabled {
                let reason = if dry_equalizer.is_some() {
                    "the saved PCM24 cache is missing. Used the verified dry input."
                } else {
                    "used the verified dry input."
                };
                loss(out, budget, &event.id, aspect, format!(
                    "{} · A{} at record frame {} for {} frames: {reason} {effect} processing is omitted.",
                    source.info.name, e.track, range.record_start, range.length))?;
            }
        }
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
            // The saved 64-row test pattern is a picture element on the canvas.
            if !item_routes[index].strip_tiff && derived_raster.is_none_or(|old| choice < old) {
                derived_raster = Some(choice);
            }
        }
        let mut p = Placement {
            id: event.id.clone(),
            item: out.graph.items[index].id.clone(),
            record_start: time(range.record_start, range.record_rate)?,
            source_in: time(source_in, range.source_rate)?,
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
            let (tail, managed) = match motion_timing(event, range, phase, &item_routes[index], covering, &mut p, budget) {
                Ok(timing) => timing,
                Err(why) => {
                    let lane = if e.kind == KIND_PICTURE { "V" } else { "A" };
                    loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING, format!(
                        "{lane}{} at record frame {} for {} frames: automatic Motion Adapter {why}",
                        e.track, range.record_start, range.length
                    ))?;
                    continue;
                }
            };
            if tail > 0 && range.record_rate == av::Rate::new(24, 1).unwrap() {
                loss(out, budget, &p.id, "motion_adapter_audio_tail",
                    format!("A{}: {tail} samples beyond the linked MP4 audio extent render as silence. Avid can emit decoded AAC packet padding within that interval.", e.track))?;
            }
            let mut timing = vec![
                ("phase", Value::Int(phase)),
                ("native_record_frames", Value::Int(range.length)),
                ("silent_audio_tail_samples", Value::Int(tail)),
            ];
            match managed {
                Some(16) => timing.push(("managed_pcm16", Value::Bool(true))),
                Some(24) => timing.push(("managed_pcm24", Value::Bool(true))),
                _ => {}
            }
            sidecar(out, budget, "avid.motion_adapter", &p.id, obj(timing))?;
        } else if e.kind == KIND_SOUND && item_routes[index].motion_audio.contains_key(&e.master_track) {
            loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING,
                format!("A{}: linked AAC file-track origin outside the qualified Motion Adapter route", e.track))?;
            continue;
        }
        if e.kind == KIND_SOUND {
            if let Some(&Mp3Timing { delay_samples, sample_rate, extent }) =
                item_routes[index].mp3_timing.get(&e.master_track)
            {
                let rate = av::Rate::new(25, 1).unwrap();
                if range.source_rate != rate || range.record_rate != rate
                    || range.source_length != range.length
                    || e.speed.is_some() || !e.via.is_empty() || e.combined.is_some()
                    || event.motion_adapter_phase.is_some()
                {
                    return Err(format!("{}: linked MP3 with encoder delay requires a straight 25 fps mono edit", p.id));
                }
                let saved_in = p.source_in;
                let delay_time = Rational::new(-i128::from(delay_samples), sample_rate.into())
                    .ok_or("MP3 sample clock is invalid")?;
                p.source_in = saved_in.checked_add(delay_time).ok_or("MP3 source trim overflow")?;
                if p.source_in.is_negative() || p.source_in.checked_add(p.duration).is_none_or(|end| end > extent) {
                    return Err(format!(
                        "{}: linked MP3 edit includes samples outside its gapless playable range after the {}-sample shift",
                        p.id, delay_samples
                    ));
                }
                sidecar(out, budget, "avid.mp3_presentation", &p.id, obj(vec![
                    ("saved_source_in", s(saved_in)),
                    ("delay_samples", Value::Int(delay_samples)),
                    ("sample_rate", Value::Int(sample_rate)),
                ]))?;
                budget.charge(64)?;
                shifted_mp3_edits.insert(event.id.as_str());
            }
            if let Some(&extent) = item_routes[index].same_rate_aac.get(&e.master_track) {
                let rate = range.source_rate;
                if !matches!((rate.num, rate.den), (24, 1) | (24_000, 1001) | (25, 1) | (30_000, 1001))
                    || range.record_rate != rate
                    || e.speed.is_some() || !e.via.is_empty() || e.combined.is_some()
                    || event.motion_adapter_phase.is_some() || event.volume_automation.is_some()
                {
                    return Err(format!("{}: linked AAC origin requires a qualified straight same-rate mono edit", p.id));
                }
                let unshifted = p.source_in;
                let frame = time(1, rate)?;
                p.source_in = unshifted.checked_add(frame).ok_or("AAC source trim overflow")?;
                let end = p.source_in.checked_add(p.duration).ok_or("AAC source end overflows")?;
                if unshifted.is_negative() || p.source_in >= extent {
                    return Err(format!("{}: linked AAC edit exceeds the playable audio after its one-frame source offset", p.id));
                }
                if end > extent {
                    if !matches!((rate.num, rate.den), (24_000, 1001) | (25, 1))
                        || end.checked_sub(frame) != Some(extent)
                    {
                        return Err(format!("{}: linked AAC edit exceeds the playable audio after its one-frame source offset", p.id));
                    }
                    p.duration = p.duration.checked_sub(frame).ok_or("AAC playable duration overflows")?;
                    loss(out, budget, &p.id, "original_aac_tail",
                        format!("A{}: the last record frame exceeds the linked file's playable audio after the saved one-frame offset. Premiere leaves this span silent. Avid can play part of the final AAC packet here.", e.track))?;
                }
                sidecar(out, budget, "avid.same_rate_aac", &p.id, obj(vec![
                    ("saved_source_in", s(unshifted)),
                    ("file_track_start", Value::Int(1)),
                ]))?;
            }
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
        if let Some(av::PanAutomation::Keys(points)) = &event.pan_automation {
            if e.kind == KIND_SOUND && e.combined.is_none() && e.via.is_empty()
                && range.record_rate == rate && Some(rate) == av::Rate::new(24, 1)
                && range.source_rate == rate && range.source_length == range.length
                && event.motion_adapter_phase.is_none() && p.speed.is_none() && p.frame_map.is_none()
                && (2..=audio_pan::MAX_KEYS).contains(&points.len())
            {
                let keys = points.iter().map(|point| {
                    if point.frame_numerator < 0 || point.frame_denominator <= 0 {
                        return None;
                    }
                    Some(audio_pan::PanKey {
                        at: Rational::new(i128::from(point.frame_numerator),
                            i128::from(point.frame_denominator) * 24)?,
                        pan: Rational::new(i128::from(point.raw_pan) - 16_384, 16_384)?,
                    })
                }).collect::<Option<Vec<_>>>();
                if let Some(keys) = keys.filter(|keys| audio_pan::valid_keys(keys, p.duration)) {
                    budget.charge(keys.len() * 384 + 512)?;
                    p.effects.push(audio_pan::linear_effect(&keys));
                }
            }
        }
        let corner_pin = e.picture.as_ref().and_then(|fx| fx.corner_pin).filter(|_|
            e.kind == KIND_PICTURE && !e.picture_nested
                && e.color.is_none() && e.animatte.is_none() && event.picture_extension.is_none()
                && event.picture_chain.is_empty() && p.speed.is_none() && p.frame_map.is_none());
        if let Some(pin) = corner_pin {
            if range.source_rate != range.record_rate
                || pin.from_transform && Some(range.record_rate) != av::Rate::new(24, 1)
                || !effects::pin_has_geometry(pin, raster, item_routes[index].square_pixels)
            {
                let reason = if pin.from_transform {
                    "3D Warp Corner Pin requires verified square-pixel 24 fps picture. The camera projection requires 1920x1080."
                } else {
                    "3D Warp Corner Pin requires verified same-rate square-pixel picture."
                };
                loss(out, budget, &p.id, "picture_effect", reason)?;
            } else {
                effects::apply_corner_pin(pin, &mut p, budget)?;
                if pin.from_transform {
                    loss(out, budget, &p.id, "picture_editability",
                        "3D Warp geometry is represented by four target points. Original controls are retained in the source receipt")?;
                }
            }
        } else if event.picture_chain.is_empty() {
            // Active curves in the qualified transforms supersede stale inline scalars.
            let replace_static = event.picture_extension.as_deref().is_some_and(av::PictureExtension::replaces_static_transform);
            effects::apply(e, !replace_static, raster, &mut p, out, budget)?;
            if let Some(mut effect) = event.picture_extension.as_deref().copied() {
                if effect.requires_square_pixels() && !item_routes[index].square_pixels {
                    effect = av::PictureExtension::Unsupported("Animated crop, Warp, or trimmed Resize requires verified square-pixel progressive geometry");
                }
                if e.picture_nested {
                    effect = av::PictureExtension::Unsupported("nested picture effects require separate qualification");
                }
                effects::apply_picture_extension(effect, raster, item_routes[index].square_pixels,
                    &mut p, out, budget)?;
            }
        } else {
            effects::apply_picture_chain(&event.picture_chain, raster,
                item_routes[index].square_pixels, &mut p, out, budget)?;
        }
        if e.kind == KIND_PICTURE && item_routes[index].fitted_png {
            if e.track != 1 || !centered_stills.contains(event.id.as_str()) || !item_routes[index].square_pixels
                || p.transform.is_some() || !p.effects.is_empty() || p.speed.is_some() || p.frame_map.is_some()
            {
                return Err(format!(
                    "{} · V{} at record frame {}: linked 1536×1024 PNG needs a verified centered Avid still fit on V1",
                    source.info.name, e.track, range.record_start,
                ));
            }
            let scale = 100.0 * 1080.0 / 1024.0;
            p.transform = Some(graph::Transform {
                position: (0.0, 0.0), scale: (scale, scale), rotation: 0.0,
                anchor: (0.0, 0.0), crop: [0.0; 4],
            });
            uses_centered_still = true;
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
        if let Some((num, den)) = event.constant_speed {
            budget.charge(192)?;
            receipt.push(("constant_speed", Value::Arr(vec![Value::Int(num), Value::Int(den)])));
        }
        if event.plain_submaster {
            receipt.push(("plain_submaster", Value::Bool(true)));
        }
        let reflection = |stage, native_id| {
            obj(vec![("stage", Value::Int(stage as i64)), ("effect_id", s(native_id))])
        };
        let reflections: Vec<_> = if event.picture_chain.is_empty() {
            match event.picture_extension.as_deref().copied() {
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
    let portable_pan: HashSet<_> = seq.placements.iter()
        .filter(|edit| edit.effects.iter().any(|effect| effect.kind == audio_pan::LINEAR_KIND))
        .map(|edit| edit.id.as_str())
        .collect();
    seq.disabled_placements = disabled::capture(source, copies, &out.graph, budget)?;
    let mut reported_pan = HashSet::new();
    for event in &source.events {
        if let (Some(automation), Some(placement)) = (&event.pan_automation, map.get(&event.id)) {
            let reason = match automation {
                av::PanAutomation::Keys(_) if portable_pan.contains(placement.as_str()) => continue,
                av::PanAutomation::Keys(_) => "Avid clip pan keys are retained in source facts and omitted from the converted edit",
                av::PanAutomation::Unsupported(reason) => reason,
            };
            if reported_pan.insert((placement, reason)) {
                loss(out, budget, placement, crate::PAN_AUTOMATION_LOSS, reason)?;
            }
        }
    }
    for t in &source.transitions {
        if t.outgoing.placements.iter().chain(&t.incoming.placements)
            .any(|p| shifted_mp3_edits.contains(p.as_str()))
        {
            return Err(format!("{}: transition on MP3 with encoder delay requires separate handle verification", t.id));
        }
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
    for issue in &source.unresolved {
        if let av::SourceIssueKind::Timewarp { length } = issue.issue_kind {
            effect_fallback::prepare_timewarp(
                issue, length, rate, source.info.bin, copies, fallbacks, &mut seq, out, budget,
            )?;
        }
    }
    let mut nested_motion_placements = HashMap::new();
    for issue in &source.unresolved {
        if let Some(placement) = nested_motion::prepare(issue, source, copies, &seq, out) {
            budget.charge(size_of::<Placement>() + placement.id.len() + placement.item.len())?;
            sidecar(out, budget, "avid.nested_motion", &placement.id, obj(vec![
                ("owner", s(&issue.owner)),
            ]))?;
            nested_motion_placements.insert(issue.owner.as_str(), (issue, placement.id.clone()));
            seq.placements.push(placement);
        }
    }
    let compatibility_frame_size = if uses_centered_still {
        seq.frame_size = Some((1920, 1080));
        (1920, 1080)
    } else {
        derived_raster.map(|(_, _, w, h)| (w, h)).unwrap_or((1920, 1080))
    };
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
    for (event, master, reason) in media_gaps {
        if let Some(lane) = append_media_gap(source, &mut seq, event, master, reason,
            &mut unresolved, out, budget)?
        {
            tracks.insert(lane);
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
    let named_lanes: HashMap<_, _> = source.track_names.iter()
        .filter_map(|track| {
            let name = portable_track_name(track)?;
            let kind = kind(track.kind)?;
            Some(((kind, track.label), (name, track.plain_empty)))
        })
        .collect();
    tracks.extend(named_lanes.keys().copied());
    seq.tracks = tracks.into_iter().map(|(kind, index)| {
        let mut track = SourceTrack::new(kind, index, None);
        if let Some(&(name, empty)) = named_lanes.get(&(kind, index)) {
            budget.charge(name.len())?;
            track.name = Some(name.into());
            track.empty = empty;
        }
        Ok(track)
    }).collect::<Result<_, String>>()?;
    seq.tracks
        .sort_by_key(|t| (if t.kind == StreamKind::Video { 0 } else { 1 }, t.index));
    let mut cut_fallbacks = HashSet::new();
    for issue in &source.unresolved {
        let color_placement = color_adapter_event(issue, &color_events)
            .and_then(|event| map.get(&event.id))
            .or_else(|| nested_motion_placements.get(issue.owner.as_str()).map(|(_, id)| id))
            .map(String::as_str);
        unresolved.push(source_issue_receipt(
            issue,
            |mob| source_master_copy(mob, source.info.bin, copies, &out.graph.items)
                .map(|item| item.id.as_str()),
            color_placement,
        ));
        match issue.issue_kind {
            av::SourceIssueKind::Timewarp { .. } => {}
            av::SourceIssueKind::UntranslatedEffect { effect, length } => {
                effect_fallback::prepare_effect_gap(issue, effect, length, rate,
                    &source.transitions, &mut cut_fallbacks, &mut seq, out, budget)?;
            }
            av::SourceIssueKind::RouteTiming => {
                let lane = if issue.kind == KIND_PICTURE { "V" } else { "A" };
                loss(out, budget, id, crate::UNQUALIFIED_ROUTE_TIMING,
                    format!("{lane}{}: {}", issue.track, issue.reason))?;
            }
            av::SourceIssueKind::MatteKey { length }
            | av::SourceIssueKind::ColorAdapter { length }
            | av::SourceIssueKind::LayeredSubmaster { length, .. } => {
                if issue.picture_profile == Some(av::SourcePictureProfile::PlainLayeredSubmaster) {
                    loss(out, budget, id, "layered_submaster",
                        format!("V{} at {}: {}", issue.track, issue.owner, issue.reason))?;
                    continue;
                }
                let position = match (&issue.record_start, length, issue.record_rate) {
                    (Observed::Known(frame), Some(len), Some(rate)) =>
                        format!("at record frame {frame} for {len} {} ({} fps)",
                            if len == 1 { "frame" } else { "frames" }, rational(rate)),
                    (Observed::Known(frame), _, _) => format!("at record frame {frame}"),
                    _ => "at unresolved record position".into(),
                };
                let aspect = if matches!(issue.issue_kind, av::SourceIssueKind::ColorAdapter { .. }) {
                    "source_color_adapter"
                } else {
                    "picture_effect"
                };
                let range = color_placement.and_then(|_| {
                    let adapter = issue.color_adapter.as_ref()?;
                    if adapter.centered_still_fit || adapter.subclip_motion.is_some() {
                        return None;
                    }
                    av::SourceColorRange::from_saved_xml(&adapter.transform)
                });
                let reason = match range {
                    Some(av::SourceColorRange::VideoToFull) =>
                        "The Avid 10-bit video-to-full-range adjustment (64–940) is unapplied. The edit uses its linked picture at the destination's default levels.",
                    Some(av::SourceColorRange::FullToVideo) =>
                        "The Avid 10-bit full-to-video-range adjustment (64–940) is unapplied. The edit uses its linked picture at the destination's default levels.",
                    None => &issue.reason,
                };
                let reason = match nested_motion_placements.get(issue.owner.as_str()) {
                    Some((effect, _)) if effect.color_adapter.as_ref().and_then(|a| a.subclip_motion.as_ref())
                        .is_some_and(|motion| motion.speed == 2.0) =>
                        "Graph/2 retains the 200% picture timing. The Avid Color Adapter is unapplied.",
                    Some(_) => "Graph/2 retains the constant-speed picture timing. The Avid Color Adapter is unapplied.",
                    None => reason,
                };
                loss(out, budget, color_placement.unwrap_or(id), aspect,
                    format!("V{} {position}: {reason}", issue.track))?;
            }
            av::SourceIssueKind::DisabledClip { length, .. } => {
                let lane = if issue.kind == KIND_PICTURE { "V" } else { "A" };
                let position = match &issue.record_start {
                    Observed::Known(frame) => format!("at record frame {frame}"),
                    Observed::Unresolved(_) => "at unresolved record position".into(),
                };
                loss(out, budget, id, "disabled_clip", format!(
                    "{}: {lane}{} {position} for {length} frames. {}",
                    issue.owner, issue.track, issue.reason,
                ))?;
            }
            av::SourceIssueKind::Route => {
                let recovered = issue.nested_picture_motion.is_some()
                    && nested_motion_placements.contains_key(issue.owner.as_str());
                if !recovered && !nested_motion_placements.values().any(|(adapter, _)|
                    nested_motion::replaces_route(issue, adapter))
                {
                    loss(out, budget, id, "source_route", issue.reason.clone())?;
                }
            }
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
                ("controls", native_controls(e)),
            ])
        })
        .collect();
    let native_transitions = source
        .transitions
        .iter()
        .map(|t| {
            let mut fields = vec![
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
                ("audio_curve_attribute", t.audio_curve_attribute.map_or(Value::Null, Value::Int)),
                ("audio_curve_parameter", t.audio_curve_parameter.map_or(Value::Null, Value::Int)),
                ("outgoing", binding_receipt(&t.outgoing)),
                ("incoming", binding_receipt(&t.incoming)),
            ];
            if let Some(frames) = t.outgoing_filler {
                fields.push(("outgoing_filler_frames", Value::Int(frames)));
            }
            if let Some(frames) = t.incoming_filler {
                fields.push(("incoming_filler_frames", Value::Int(frames)));
            }
            if cut_fallbacks.contains(&t.id) {
                fields.push(("gain_gap_edges", Value::Arr(t.gain_gap_edges.iter().map(|edge| match edge {
                    Some(av::GainGapEdge::Gain(owner)) => s(owner),
                    Some(av::GainGapEdge::Filler) => Value::Bool(true),
                    None => Value::Null,
                }).collect())));
            }
            obj(fields)
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
    let unresolved_names: Vec<_> = source.track_names.iter()
        .filter(|track| portable_track_name(track).is_none())
        .map(|track| {
            let mut fields = vec![
                ("kind", Value::Int(track.kind)),
                ("track", Value::Int(track.label)),
                ("name", s(&track.name)),
                ("ambiguous", Value::Bool(track.ambiguous)),
            ];
            if track.plain_empty {
                fields.push(("plain_empty", Value::Bool(true)));
            }
            obj(fields)
        }).collect();
    if !unresolved_names.is_empty() {
        sidecar(out, budget, "avid.track_names", id, Value::Arr(unresolved_names))?;
    }
    let plain_layouts: Vec<_> = source.track_names.iter()
        .filter(|track| track.plain_layout && !track.plain_empty && portable_track_name(track).is_some())
        .map(|track| obj(vec![
            ("kind", Value::Int(track.kind)),
            ("track", Value::Int(track.label)),
        ])).collect();
    if !plain_layouts.is_empty() {
        sidecar(out, budget, "avid.plain_track_layouts", id, Value::Arr(plain_layouts))?;
    }
    for issue in &source.info.problems {
        out.diagnostics
            .push(format!("{:?}: {issue}", source.info.name));
    }
    budget.charge(
        size_of::<SourceSequence>()
            + seq.id.len()
            + seq.name.capacity()
            + seq.tracks.len() * size_of::<SourceTrack>(),
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
    let mut reservation = *budget;
    for e in &source.events {
        effects::reserve_receipt(e, &mut reservation)?;
        if let Some(automation) = &e.pan_automation {
            reservation.charge(512)?;
            if let av::PanAutomation::Keys(keys) = automation {
                reservation.charge(keys.len().checked_mul(384).ok_or("pan key receipt overflow")?)?;
            }
        }
    }
    for t in &source.texts {
        reservation.charge(2048 + t.id.len() + t.content.as_ref().map_or(0, String::len) + t.reason.as_ref().map_or(0, String::len))?;
        for p in &t.native_properties {reservation.charge(1024+p.scope.len()+p.name.len()+p.value.len())?;}
    }
    for controls in source.unresolved.iter().filter_map(|issue| issue.timewarp.as_deref()) {
        reservation.charge(4096 + controls.resize_render_owner.as_ref().map_or(0, String::len))?;
        for curve in &controls.curves {
            reservation.charge(2048)?;
            let cost = match &curve.points {
                Some(av::SourceTimewarpPoints::HalfFrameSamples { values, .. }) => values.len() * 32,
                Some(av::SourceTimewarpPoints::Keys(keys)) => keys.len() * 1024,
                None => 0,
            };
            reservation.charge(cost)?;
        }
    }
    for adapter in source.unresolved.iter().filter_map(|issue| issue.color_adapter.as_ref()) {
        if av::SourceColorRange::from_saved_xml(&adapter.transform).is_some() {
            reservation.charge(512)?;
        }
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
fn equalizer_settings(eq: &av::SourceEqualizer) -> Vec<(&'static str, Value)> {
    vec![
        ("effect_id", s("EFF_AUDIO_EQMB")),
        ("enabled", Value::Bool(eq.enabled)),
        ("filter_name", s(&eq.filter_name)),
        ("bands", Value::Arr(eq.bands.iter().map(|band| obj(vec![
            ("type", Value::Int(band.filter_type)),
            ("frequency_raw", Value::Int(band.frequency)),
            ("gain_raw", Value::Int(band.gain)),
            ("q_raw", Value::Int(band.q)),
            ("enabled", Value::Bool(band.enabled)),
        ])).collect())),
    ]
}

fn unavailable_audiosuite(render: &av::SourceAudioSuiteRender, track: i64, start: i64, frames: i64) -> String {
    format!("Avid AudioSuite {} on A{track} at record frame {start} for {frames} frames needs its verified linked PCM24 cache. The audio edit is omitted.", render.name)
}

fn native_controls(event: &av::SourceEvent) -> Value {
    let mut controls = effects::receipt(&event.event);
    if let Some(&(master, start)) = event.gain_dry_input.as_deref() {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        fields.push(("gain_dry_input".into(), obj(vec![
            ("master", s(master.urn())),
            ("source_in", Value::Int(start)),
        ])));
    } else if event.dry_gain_bypass {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        fields.push(("audiosuite_gain".into(), Value::Bool(true)));
    }
    if let Some(eq) = event.equalizer.as_deref() {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        let mut saved = equalizer_settings(eq);
        if let Some(pitch) = &eq.pitch {
            saved.push(("pitch_ii", obj(vec![
                ("master", s(pitch.master.urn())),
                ("file", s(pitch.file.urn())),
                ("origin", s(pitch.origin.urn())),
                ("declared_samples", Value::Int(pitch.samples)),
                ("chunk_bytes", Value::Int(pitch.pitch_chunk.len() as i64)),
                ("chunk_sha256", s(hex::encode(Sha256::digest(pitch.pitch_chunk)))),
            ])));
        }
        if let Some(render) = eq.render {
            let mut dry_input = vec![
                ("subclip", render.dry_subclip.map_or(Value::Null, |id| s(id.urn()))),
                ("master", s(render.dry_master.urn())),
                ("master_track", Value::Int(render.dry_master_track)),
                ("file", s(render.dry_file.urn())),
                ("track", Value::Int(render.dry_track)),
                ("start_frame", Value::Int(render.dry_start)),
            ];
            if render.dry_wrapped_master {
                dry_input.push(("master_usage", Value::Int(7)));
            }
            if let Some(gain) = render.input_gain {
                dry_input.push(("level_q29", Value::Int(gain.level_q29.into())));
                dry_input.push(("pan_set", Value::Bool(gain.pan_set)));
            }
            saved.push(("render", obj(vec![
                ("master", s(event.event.master.urn())),
                ("file", s(render.file.urn())),
                ("origin", s(render.origin.urn())),
                ("start_frame", Value::Int(render.start)),
                ("duration_frames", Value::Int(render.frames)),
                ("dry_input", obj(dry_input)),
            ])));
        }
        fields.push(("legacy_eqmb".into(), obj(saved)));
    }
    if let Some(render) = event.audiosuite.as_deref() {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        let mut input = vec![
            ("master", s(render.input_master.urn())),
            ("master_track", Value::Int(render.input_track)),
            ("file", s(render.input_file.urn())),
            ("track", Value::Int(render.file_track)),
            ("start_frame", Value::Int(render.input_start)),
        ];
        if let Some((id, track, start)) = render.input_subclip {
            input.push(("subclip", obj(vec![
                ("mob", s(id.urn())),
                ("track", Value::Int(track)),
                ("start_frame", Value::Int(start)),
            ])));
        }
        fields.push(("audiosuite_render".into(), obj(vec![
            ("name", s(&render.name)),
            ("manufacturer_id", Value::Int(render.manufacturer_id.into())),
            ("product_id", Value::Int(render.product_id.into())),
            ("plugin_id", Value::Int(render.plugin_id.into())),
            ("chunk_id", Value::Int(render.chunk_id.into())),
            ("chunk_version", Value::Int(render.chunk_version)),
            ("chunk_bytes", Value::Int(render.chunk.len() as i64)),
            ("chunk_sha256", s(hex::encode(Sha256::digest(&render.chunk)))),
            ("master", s(event.event.master.urn())),
            ("file", s(render.file.urn())),
            ("origin", s(render.origin.urn())),
            ("start_frame", Value::Int(render.start)),
            ("duration_frames", Value::Int(render.frames)),
            ("padding_samples", Value::Int(render.padding_samples)),
            ("input", obj(input)),
        ])));
    }
    if event.subclip_center_pan_count > 0 || event.subclip_gain.is_some() {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        let explicit = event.subclip_gain;
        let mut saved = vec![
            ("level_set", Value::Bool(explicit.is_some())),
            ("level_q29", Value::Int(explicit.map_or(1 << 29, |gain| i64::from(gain.level_q29)))),
            ("pan_set", Value::Bool(explicit.is_none_or(|gain| gain.pan_set))),
            ("pan", Value::Int(16_384)),
        ];
        if event.subclip_center_pan_count > 1 {
            saved.push(("wrapper_count", Value::Int(event.subclip_center_pan_count.into())));
        }
        fields.push(("subclip_pvol".into(), obj(saved)));
    }
    if event.plain_submaster {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        fields.push(("plain_submaster".into(), Value::Bool(true)));
    }
    let projective = effects::projective_stage_receipt(&event.picture_chain);
    if !projective.is_empty() {
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        fields.push(("projective_picture_stages".into(), Value::Arr(projective)));
    }
    if let Some(automation) = &event.pan_automation {
        let mut pan = vec![("control_code", Value::Int(2000))];
        match automation {
            av::PanAutomation::Keys(keys) => {
                pan.push(("saved_interp_kind", Value::Int(3)));
                pan.push(("keys", Value::Arr(keys.iter().map(|key| obj(vec![
                    ("frame_numerator", Value::Int(key.frame_numerator)),
                    ("frame_denominator", Value::Int(key.frame_denominator)),
                    ("raw_pan", Value::Int(key.raw_pan)),
                ])).collect())));
            }
            av::PanAutomation::Unsupported(reason) => pan.push(("unqualified", s(reason))),
        }
        let Value::Obj(fields) = &mut controls else { unreachable!() };
        fields.push(("pvol_pan".into(), obj(pan)));
    }
    controls
}

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
                            ("controls", native_controls(e)),
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
                    .map(|issue| {
                        let mut fields = vec![
                            ("owner", s(&issue.owner)),
                            ("reason", s(&issue.reason)),
                            ("route", routes(&issue.route)),
                            ("record_start", observed(&issue.record_start)),
                            ("record_rate", issue.record_rate.map(|r| s(rational(r))).unwrap_or(Value::Null)),
                            ("kind", Value::Int(issue.kind)),
                            ("track", Value::Int(issue.track)),
                        ];
                        extend_effect_issue(issue, &mut fields, |_| None, None);
                        obj(fields)
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
