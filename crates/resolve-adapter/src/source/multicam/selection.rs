//! Check a saved Resolve angle and the file edit derived from it.
use crate::prepare::frames_on;
use graph::{
    source::{Multicam, MulticamUse},
    Placement, Rational, StreamKind, Value,
};
use std::ops::Range;

pub(in crate::source) const OBSERVED_GAP: &str = "selected Resolve angle includes an observed gap";

pub(in crate::source) struct Selection<'a> {
    pub angle: &'a str,
    pub audio_selector: Option<i64>,
    pub leaf: Option<(Range<i64>, &'a str, Rational)>,
}

impl Selection<'_> {
    pub fn agrees(&self, usage: &MulticamUse, placement: &Placement, visible: &Range<i64>) -> bool {
        let Some((range, item, source_in)) = &self.leaf else {
            return false;
        };
        let lane_agrees = match usage.kind {
            StreamKind::Video => {
                placement.video_track == Some(usage.track) && placement.audio_tracks.is_empty()
            }
            StreamKind::Audio => {
                placement.video_track.is_none()
                    && placement.audio_tracks == [usage.track]
                    && usage
                        .lane
                        .is_some_and(|lane| placement.audio_streams == [lane])
            }
        };
        lane_agrees
            && range == visible
            && placement.item == *item
            && placement.source_in == *source_in
            && placement.speed.is_none()
            && placement.frame_map.is_none()
    }
}

pub(in crate::source) fn range(
    start: Rational,
    duration: Rational,
    rate: Rational,
) -> Option<Range<i64>> {
    let start = frames_on(start, rate).ok()?;
    let length = frames_on(duration, rate).ok()?;
    (length > 0).then_some(start..start.checked_add(length)?)
}

pub(in crate::source) fn select<'a>(
    usage: &MulticamUse,
    group: &'a Multicam,
    rate: Rational,
    owner: &'a Value,
    edit: &'a Value,
    record: &Range<i64>,
) -> Option<Selection<'a>> {
    if usage.reason.is_some()
        || group.reason.is_some()
        || group.rate != Some(rate)
        || crate::nests::string(owner, "kind") != Some("multicam")
    {
        return None;
    }
    let selected = usize::try_from(usage.selected?).ok()?;
    let angle = crate::nests::string(edit, "angle_id").filter(|name| !name.is_empty())?;
    let audio_selector = match usage.kind {
        StreamKind::Video => {
            let Value::Arr(angles) = owner.get("video_angles")? else {
                return None;
            };
            if angles.len() != 2
                || !matches!(angles.get(selected), Some(Value::Str(name)) if name == angle)
                || angles
                    .iter()
                    .filter(|v| matches!(v, Value::Str(name) if name == angle))
                    .count()
                    != 1
            {
                return None;
            }
            None
        }
        StreamKind::Audio => {
            let selector = crate::nests::int(edit, "saved_audio_selector")?;
            if !matches!(selector, 0 | 1)
                || usage.lane != Some(selector + 1)
                || selected != 0
                || crate::nests::string(owner, "reference_audio_angle") != Some(angle)
            {
                return None;
            }
            Some(selector)
        }
    };
    Some(Selection {
        angle,
        audio_selector,
        leaf: leaf(usage, group, selected, rate, record)?,
    })
}

// None is unqualified. Some(None) is an explicitly saved gap throughout.
fn leaf<'a>(
    usage: &MulticamUse,
    group: &'a Multicam,
    angle: usize,
    rate: Rational,
    record: &Range<i64>,
) -> Option<Option<(Range<i64>, &'a str, Rational)>> {
    let mut lanes = group
        .lanes
        .iter()
        .filter(|l| l.kind == usage.kind && Some(l.index) == usage.lane);
    let lane = lanes.next()?;
    let supported_lane = match usage.kind {
        StreamKind::Video => lane.index == 1 && lane.alternatives.len() == 2,
        StreamKind::Audio => matches!(lane.index, 1 | 2) && lane.alternatives.len() == 1,
    };
    if !supported_lane || lanes.next().is_some() {
        return None;
    }
    let alternative = lane.alternatives.get(angle)?;
    if alternative.reason.is_some() || alternative.pieces.len() > 3 {
        return None;
    }
    let start = frames_on(usage.source_in?, rate).ok()?;
    let end = start.checked_add(record.end - record.start)?;
    let mut previous_end = 0;
    let mut leaf = None;
    for piece in &alternative.pieces {
        let span = range(piece.start, piece.duration, rate)?;
        if span.start != previous_end {
            return None;
        }
        previous_end = span.end;
        let visible = start.max(span.start)..end.min(span.end);
        match (&piece.item, piece.stream, piece.source_in) {
            (None, None, None) => {}
            (Some(item), Some(stream), Some(source_in))
                if stream == lane.index && !visible.is_empty() && leaf.is_none() =>
            {
                let offset = Rational::new(
                    i128::from(visible.start - span.start) * i128::from(rate.den()),
                    i128::from(rate.num()),
                )?;
                leaf = Some((
                    (record.start + (visible.start - start))
                        ..(record.start + (visible.end - start)),
                    item.as_str(),
                    source_in.checked_add(offset)?,
                ));
            }
            (Some(_), Some(stream), Some(_)) if stream == lane.index && visible.is_empty() => {}
            _ => return None,
        }
    }
    if previous_end < end || frames_on(group.duration?, rate).ok()? != previous_end {
        return None;
    }
    // The reader's observed-gap reason is valid only when the selected
    // alternative independently proves that part of the interval is empty.
    let has_gap = leaf
        .as_ref()
        .is_none_or(|(visible, _, _)| visible != record);
    match (has_gap, usage.binding.reason.as_deref()) {
        (false, None) | (true, Some(OBSERVED_GAP)) => Some(leaf),
        _ => None,
    }
}
