//! Omit unsupported native occurrences only when their saved record spans
//! are established and the remaining edits carry the destination extent.
use super::{frames_on, ticks, PreparedTrackMulticam, SourceGraph, SourceSequence, TextKind};
use crate::{dissolve, nests, Result};
use graph::{Loss, Rational, StreamKind, Value};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Omissions {
    losses: Vec<Loss>,
    superseded: HashMap<String, (&'static str, &'static str)>,
    required_end: Option<(i64, String)>,
}

impl Omissions {
    pub fn is_empty(&self) -> bool {
        self.losses.is_empty()
    }

    pub fn approval_error(&self, sequence: &str) -> Box<dyn std::error::Error> {
        let first = &self.losses[0];
        format!(
            "{sequence}: {} unsupported occurrence(s) require --accept-losses, including {} ({})",
            self.losses.len(),
            first.object,
            first.reason
        )
        .into()
    }

    pub fn replaces(&self, loss: &Loss, sequence: &str) -> bool {
        loss.object == sequence
            && loss.reason.split_once(": ").is_some_and(|(id, reason)| {
                self.superseded
                    .get(id)
                    .is_some_and(|&(aspect, expected)| loss.aspect == aspect && reason == expected)
            })
    }

    pub fn into_losses(self) -> Vec<Loss> {
        self.losses
    }

    pub fn check_prepared_end(
        &self,
        input: &prproj::convert::prepared::PreparedProjectInput,
    ) -> Result<()> {
        let Some((required_end, id)) = &self.required_end else {
            return Ok(());
        };
        let covered = input.sequences[0].events.iter().any(|edit| {
            edit.record_start
                .checked_add(edit.duration)
                .is_some_and(|end| end >= *required_end)
        }) || input
            .multicam_tracks
            .iter()
            .flat_map(|group| &group.uses)
            .any(|edit| {
                edit.start
                    .checked_add(edit.duration)
                    .is_some_and(|end| end >= *required_end)
            });
        if !covered {
            return Err(
                format!("{id}: prepared edits do not cover the omitted record extent").into(),
            );
        }
        Ok(())
    }
}

pub(super) fn collect<'a>(
    graph: &'a SourceGraph,
    sequence: &'a SourceSequence,
    groups: &[PreparedTrackMulticam],
    claimed: &HashSet<&str>,
    origin: i64,
) -> Result<Omissions> {
    let transitions = rows(graph, sequence, dissolve::ADAPTER)?;
    let nested = rows(graph, sequence, nests::ADAPTER)?;
    if transitions.is_empty() && nested.is_empty() && sequence.texts.is_empty() {
        return Ok(Omissions::default());
    }
    let Some(Value::Int(saved_length)) = graph
        .sidecar(crate::source::ADAPTER, &sequence.id)
        .and_then(|row| row.get("record_length"))
    else {
        return Err(format!(
            "{}: omission requires a saved Resolve record extent",
            sequence.id
        )
        .into());
    };
    let saved_length = Rational::integer(*saved_length);
    let frame = ticks(
        Rational::new(sequence.rate.den().into(), sequence.rate.num().into()),
        &sequence.id,
    )?;
    let ordinary_end = sequence
        .placements
        .iter()
        .filter(|edit| !claimed.contains(edit.id.as_str()))
        .try_fold(0, |end, edit| {
            Ok::<_, Box<dyn std::error::Error>>(end.max(ticks(
                edit.record_start.checked_add(edit.duration),
                &edit.id,
            )?))
        })?;
    let retained_end =
        groups
            .iter()
            .flat_map(|group| &group.uses)
            .try_fold(ordinary_end, |end, edit| {
                edit.start
                    .checked_add(edit.duration)
                    .map(|out| end.max(out))
                    .ok_or("multicam record extent overflow")
            })?;
    let lanes: HashSet<_> = sequence
        .tracks
        .iter()
        .map(|track| (track.kind, track.index))
        .collect();
    let mut ids: HashSet<_> = sequence
        .placements
        .iter()
        .map(|edit| edit.id.as_str())
        .chain(
            graph
                .multicam_uses
                .iter()
                .filter(|edit| edit.sequence == sequence.id)
                .map(|edit| edit.id.as_str()),
        )
        .collect();
    let mut omissions = Omissions::default();
    let mut omit = |id: &'a str,
                    kind,
                    track,
                    start: Rational,
                    duration: Rational,
                    name: &str,
                    aspect: &str,
                    source_loss: Option<(&'static str, &'static str)>|
     -> Result<()> {
        let end = start
            .checked_add(duration)
            .filter(|end| {
                start >= Rational::ZERO && duration > Rational::ZERO && *end <= saved_length
            })
            .ok_or_else(|| {
                format!("{id}: omitted {name} has no exact span inside the Resolve timeline")
            })?;
        if !lanes.contains(&(kind, track)) {
            return Err(format!("{id}: omitted {name} has no declared Resolve track").into());
        }
        if id.is_empty() || !ids.insert(id) {
            return Err(format!("{id}: ambiguous omitted {name} occurrence").into());
        }
        // Require the retained extent to cover the exact end. Only the
        // verification threshold is rounded up, since no edit is placed.
        let scaled = i128::from(end.num()) * i128::from(frame);
        let denominator = i128::from(end.den());
        let end_ticks = i64::try_from((scaled + denominator - 1) / denominator)
            .ok()
            .filter(|end| *end <= retained_end)
            .ok_or_else(|| format!("{id}: omitting {name} would shorten the timeline"))?;
        if omissions
            .required_end
            .as_ref()
            .is_none_or(|(max, _)| end_ticks > *max)
        {
            omissions.required_end = Some((end_ticks, id.into()));
        }
        let lane = if kind == StreamKind::Video { 'V' } else { 'A' };
        omissions.losses.push(Loss {
            object: id.into(),
            aspect: aspect.into(),
            reason: format!(
                "{name} omitted on {lane}{track} at record frames [{start}, {end}). Retained edits keep their positions."
            ),
        });
        if let Some(source_loss) = source_loss {
            omissions.superseded.insert(id.into(), source_loss);
        }
        Ok(())
    };
    for row in transitions {
        let id = field(row, "id").unwrap_or("<missing ID>");
        let fail =
            || format!("{id}: unsupported Resolve transition has no verified omission range");
        let kind = track_kind(row, "kind").ok_or_else(fail)?;
        let track = number(row, "track").ok_or_else(fail)?;
        let coordinate = |key, origin| {
            field(row, key)
                .and_then(|value| match kind {
                    StreamKind::Audio => crate::source::exact_frame_offset(value, origin),
                    StreamKind::Video => value
                        .parse::<i64>()
                        .ok()
                        .and_then(|frame| frame.checked_sub(origin))
                        .map(Rational::integer),
                })
                .ok_or_else(fail)
        };
        let start = coordinate("saved_start", origin)?;
        let duration = coordinate("saved_duration", 0)?;
        if field(row, "native_track_id").is_none_or(str::is_empty)
            || number(row, "timeline_origin_frames") != Some(origin)
            || number(row, "track_rate_numerator") != Some(sequence.rate.num())
            || number(row, "track_rate_denominator") != Some(sequence.rate.den())
            || field(row, "flags") != Some("0")
            || !matches!(field(row, "position"), Some("1" | "2" | "3"))
            || !matches!(field(row, "alignment_type"), Some("1" | "2" | "3"))
        {
            return Err(fail().into());
        }
        omit(
            id,
            kind,
            track,
            start,
            duration,
            "Resolve transition",
            "transition_omitted",
            Some(("transition", dissolve::LOSS)),
        )?;
    }
    for row in nested {
        let id = field(row, "id").unwrap_or("<missing ID>");
        let fail = || format!("{id}: unsupported Resolve nest has no verified omission range");
        if !matches!(field(row, "kind"), Some("timeline" | "compound"))
            || field(row, "record_rate").and_then(Rational::parse) != Some(sequence.rate)
            || field(row, "child_sequence").is_none_or(str::is_empty)
        {
            return Err(fail().into());
        }
        omit(
            id,
            track_kind(row, "track_kind").ok_or_else(fail)?,
            number(row, "track").ok_or_else(fail)?,
            Rational::integer(number(row, "record_start_frames").ok_or_else(fail)?),
            Rational::integer(number(row, "record_duration_frames").ok_or_else(fail)?),
            "Resolve nest",
            "nest_omitted",
            Some(("placement", nests::LOSS)),
        )?;
    }
    for title in &sequence.texts {
        if title.kind != TextKind::Graphic {
            return Err(format!(
                "{}: subtitles on a Resolve multicam timeline are unqualified",
                title.id
            )
            .into());
        }
        let exact = |value: Option<Rational>| {
            value
                .and_then(|t| frames_on(t, sequence.rate).ok())
                .map(Rational::integer)
                .ok_or_else(|| {
                    format!(
                        "{}: Resolve title span is unresolved or outside the frame grid",
                        title.id
                    )
                })
        };
        omit(
            &title.id,
            StreamKind::Video,
            title.track,
            exact(title.record_start)?,
            exact(title.duration)?,
            "Resolve title",
            "title_omitted",
            None,
        )?;
    }
    Ok(omissions)
}

fn rows<'a>(
    graph: &'a SourceGraph,
    sequence: &SourceSequence,
    adapter: &str,
) -> Result<&'a [Value]> {
    match graph.sidecar(adapter, &sequence.id) {
        None => Ok(&[]),
        Some(Value::Arr(rows)) if !rows.is_empty() => Ok(rows),
        _ => Err(format!("{}: invalid {adapter} receipt", sequence.id).into()),
    }
}

fn field<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    match row.get(key)? {
        Value::Str(value) => Some(value),
        _ => None,
    }
}

fn number(row: &Value, key: &str) -> Option<i64> {
    match row.get(key)? {
        Value::Int(value) => Some(*value),
        _ => None,
    }
}

fn track_kind(row: &Value, key: &str) -> Option<StreamKind> {
    match field(row, key)? {
        "video" => Some(StreamKind::Video),
        "audio" => Some(StreamKind::Audio),
        _ => None,
    }
}
