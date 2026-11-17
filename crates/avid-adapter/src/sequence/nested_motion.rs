//! Place a qualified constant-speed trim over its original picture master.
use super::*;

/// Preserve the reader's route receipt, while clearing only the failure
/// for the input actually replaced by this verified nested placement.
pub(super) fn replaces_route(route: &av::SourceIssue, adapter: &av::SourceIssue) -> bool {
    let (Some(saved), av::SourceIssueKind::ColorAdapter { length: Some(length) }) =
        (&adapter.color_adapter, adapter.issue_kind) else { return false };
    let [input] = route.route.as_slice() else { return false };
    route.reason == "source composition branch is not qualified"
        && route.owner.strip_prefix(&adapter.owner).is_some_and(|tail| tail.starts_with("/component/"))
        && route.kind == adapter.kind && route.track == adapter.track
        && route.record_start == adapter.record_start && route.record_rate == adapter.record_rate
        && input.mob == saved.master && input.track == saved.master_track
        && input.rate == adapter.record_rate
        && input.source_offset == Observed::Known(saved.source_in)
        && input.length == Observed::Known(length)
}

pub(super) fn prepare(
    issue: &av::SourceIssue,
    source: &av::SourceSequence,
    copies: &HashMap<MobId, Vec<(usize, usize)>>,
    sequence: &SourceSequence,
    out: &ReadResult,
) -> Option<Placement> {
    let standalone = issue.nested_picture_motion.is_some();
    let (frames, motion, expansion, external) = match issue.issue_kind {
        av::SourceIssueKind::Route if standalone => {
            let [input] = issue.route.as_slice() else { return None };
            let Observed::Known(frames) = input.length else { return None };
            if source.transitions.iter().any(|transition|
                transition.kind == issue.kind && transition.track == issue.track)
            {
                return None;
            }
            (frames, issue.nested_picture_motion.as_deref()?, false, false)
        }
        av::SourceIssueKind::ColorAdapter { length: Some(frames) } => {
            let adapter = issue.color_adapter.as_deref()?;
            if !adapter.neutral_wrapper && adapter.managed_picture_span.is_none() {
                return None;
            }
            let expansion = av::SourceColorRange::from_saved_xml(&adapter.transform)
                == Some(av::SourceColorRange::VideoToFull);
            let external = adapter.managed_picture_span.is_some()
                && av::is_canonical_external_cube(&adapter.transform);
            (frames, adapter.subclip_motion.as_deref()?, expansion, external)
        }
        _ => return None,
    };
    let samples = motion.sampled_input_frames?;
    let (num, den) = samples.ratio;
    let file = motion.sampled_picture_file?;
    let Observed::Known(record) = issue.record_start else { return None };
    let rate = issue.record_rate?;
    let last_offset = i64::try_from((i128::from(frames.checked_sub(1)?) * i128::from(num)
        + i128::from(samples.phase)).checked_div(i128::from(den))?).ok()?;
    if issue.kind != KIND_PICTURE || issue.track != 1
        || !(standalone || expansion || external)
        || sequence.rate != rational(rate) || record < 0
        || frames <= 0 || !(0..den).contains(&samples.phase)
        || samples.first.checked_add(last_offset) != Some(samples.last)
    {
        return None;
    }
    let master = motion.sampled_picture_master.unwrap_or(av::ColorAdapterMaster {
        mob: motion.input_mob, track: motion.input_track, source_in: samples.first,
    });
    let item = source_master_copy(master.mob, source.info.bin, copies, &out.graph.items)?;
    let mut pictures = item.media.iter().filter_map(|binding| {
        let media = out.graph.media.iter().find(|media| media.id == binding.media)?;
        (media.kind == StreamKind::Video).then_some((binding, media))
    });
    let (binding, media) = pictures.next()?;
    let media_facts = out.graph.sidecar("avid.source", &media.id)?;
    let Value::Str(package) = media_facts.get("file_package")? else { return None };
    let physical_clock = media.original.is_some() || media.clock.as_ref().is_some_and(|clock|
        clock.provenance.route == "file_source_clip_to_physical_timecode_track"
            && clock.exact_start().is_some() && clock.exact_start() == media.start);
    let supported_picture = standalone || (media.original.is_none() && rate == Rate::new(24, 1)?
        && media.start == Some(Rational::ZERO) && media.frame_size == Some((1920, 1080))
        && match (media.codec.as_deref(), media_facts.get("picture_coding")) {
            (Some("AVdn"), Some(Value::Str(coding))) => coding == "060E2B340401010A0401020271130000" && expansion,
            (Some("apcs"), Some(Value::Str(coding))) => coding == "060E2B340401010D0401020203060200" && (num, den) == (2, 1),
            _ => false,
        });
    let source_clock = i128::from(rate.num) * i128::from(den);
    let rate_den = i128::from(rate.den);
    let source_length = Rational::new(
        i128::from(frames).checked_mul(i128::from(num))?.checked_mul(rate_den)?, source_clock)?;
    let source_in = Rational::new(
        (i128::from(master.source_in) * i128::from(den) + i128::from(samples.phase))
            .checked_mul(rate_den)?, source_clock,
    )?;
    let source_end = source_in.checked_add(source_length)?;
    if pictures.next().is_some() || !item.markers.is_empty()
        || binding.source_offset != Some(Rational::ZERO)
        || media.online != Some(true) || media.locations.len() != 1
        || media.rate != Some(rational(rate)) || !supported_picture || !physical_clock
        || !package.eq_ignore_ascii_case(&file.umid_hex()) || source_in < Rational::ZERO
        || source_end > media.duration
    {
        return None;
    }
    let start = time(record, rate).ok()?;
    let duration = time(frames, rate).ok()?;
    let end = start.checked_add(duration)?;
    let overlap = |at: Rational, length: Rational| {
        at < end && at.checked_add(length).is_none_or(|limit| limit > start)
    };
    if sequence.placements.iter().any(|p| p.video_track == Some(issue.track) && overlap(p.record_start, p.duration))
        || sequence.transitions.iter().any(|t| t.kind == StreamKind::Video && t.track == issue.track
            && t.cut.checked_sub(t.cut_offset).is_none_or(|at| overlap(at, t.duration)))
    {
        return None;
    }
    Some(Placement {
        id: format!("{}/nested-motion", issue.owner),
        item: item.id.clone(),
        record_start: start,
        source_in,
        duration,
        video_track: Some(issue.track),
        audio_tracks: vec![],
        audio_streams: vec![],
        grouped_audio: vec![],
        gain_db: 0.0,
        volume_db: None,
        volume_keys: vec![],
        track_gains_db: vec![],
        track_pans: vec![],
        speed: Some(Speed { ratio: (den, num), source_duration: source_length }),
        frame_map: None,
        transform: None,
        effects: vec![],
    })
}
