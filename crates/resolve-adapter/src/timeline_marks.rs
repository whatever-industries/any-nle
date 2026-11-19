//! Resolve's timeline-relative In/Out frames. Out is the last marked frame.
use crate::{source::ADAPTER, Result};
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Rational, Value,
};
use prproj::build::time::TICKS_PER_SECOND;

pub(crate) const FIELD: &str = "saved_timeline_marks";
pub(crate) const PREMIERE_ADAPTER: &str = "premiere.timeline_marks";
pub(crate) const LOSS: (&str, &str) = (
    "timeline_marks",
    "Resolve timeline marks retained in graph; destination mapping is unqualified",
);
pub(crate) const TAGS: [(&str, &str); 4] = [
    ("MarkInVideo", "video_in"),
    ("MarkOutVideo", "video_out"),
    ("MarkInAudio", "audio_in"),
    ("MarkOutAudio", "audio_out"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineMarks([Option<i64>; 4]);

impl TimelineMarks {
    pub(crate) fn from_premiere(
        graph: &SourceGraph,
        sequence: &SourceSequence,
        length: i64,
    ) -> Result<Option<Self>> {
        let Some(data) = graph.sidecar(PREMIERE_ADAPTER, &sequence.id) else {
            return Ok(None);
        };
        let invalid = || format!("{}: contradictory Premiere timeline mark receipt", sequence.id);
        let Value::Obj(_) = data else {
            return Err(invalid().into());
        };
        let read = |name| -> Result<Option<i64>> {
            let Some(row) = data.get(name) else {
                return Ok(None);
            };
            let ticks = match row.get("ticks") {
                Some(Value::Int(value)) if *value >= 0 => *value,
                _ => return Err(invalid().into()),
            };
            let time = Rational::new(ticks.into(), TICKS_PER_SECOND.into()).ok_or_else(invalid)?;
            let frame = crate::prepare::frames_on(time, sequence.rate).ok();
            if let Some(saved) = row.get("saved_frame") {
                if frame.map(Value::Int).as_ref() != Some(saved) {
                    return Err(invalid().into());
                }
            }
            Ok(frame)
        };
        let (start, end) = (read("in")?, read("out")?);
        // A saved subframe endpoint cannot be treated as an absent mark.
        // Resolve stores Out as the last included frame.
        if (start.is_none() && data.get("in").is_some())
            || (end.is_none() && data.get("out").is_some())
            || start.is_some_and(|frame| frame >= length)
            || end.is_some_and(|frame| frame == 0 || frame > length)
            || matches!((start, end), (Some(start), Some(end)) if start >= end)
        {
            return Ok(None);
        }
        let out = end.map(|frame| frame - 1);
        Ok((start.is_some() || out.is_some()).then_some(Self([start, out, start, out])))
    }

    pub(crate) fn prepare(graph: &SourceGraph, id: &str, length: i64) -> Result<Option<Self>> {
        let Some(Self(values)) = Self::saved(graph, id, length)? else {
            return Ok(None);
        };
        if values[..2] != values[2..] {
            return Err(format!(
                "{id}: independent Resolve audio and video marks change on import"
            )
            .into());
        }
        Ok(Some(Self(values)))
    }

    /// Read the saved range, including independent audio and video marks.
    pub(crate) fn saved(graph: &SourceGraph, id: &str, length: i64) -> Result<Option<Self>> {
        let Some(data) = graph.sidecar(ADAPTER, id).and_then(|row| row.get(FIELD)) else {
            return Ok(None);
        };
        let invalid = || format!("{id}: unsupported Resolve timeline marks");
        let Value::Obj(fields) = data else {
            return Err(invalid().into());
        };
        if fields
            .iter()
            .any(|(key, _)| !TAGS.iter().any(|(_, saved)| key == saved))
        {
            return Err(invalid().into());
        }
        let mut values = [None; 4];
        for ((_, field), slot) in TAGS.into_iter().zip(&mut values) {
            match data.get(field) {
                None => {}
                Some(Value::Str(value)) if value.is_empty() => {}
                Some(Value::Str(value)) if value.bytes().all(|b| b.is_ascii_digit()) => {
                    let frame = value.parse::<i64>().map_err(|_| invalid())?;
                    if frame >= length {
                        return Err(invalid().into());
                    }
                    *slot = Some(frame);
                }
                _ => return Err(invalid().into()),
            }
        }
        if values
            .chunks_exact(2)
            .any(|pair| matches!(pair, [Some(a), Some(b)] if a > b))
        {
            return Err(invalid().into());
        }
        Ok(values.iter().any(Option::is_some).then_some(Self(values)))
    }

    pub(crate) fn frames(self) -> [Option<i64>; 4] {
        self.0
    }

    pub(crate) fn fields(self) -> impl Iterator<Item = (&'static str, i64)> {
        TAGS.into_iter()
            .zip(self.0)
            .filter_map(|((tag, _), n)| Some((tag, n?)))
    }

    pub(crate) fn recover(loss: &Loss, sequence: &str) -> bool {
        loss.object == sequence && loss.aspect == LOSS.0 && loss.reason == LOSS.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::{json, Project, Sidecar};

    #[test]
    fn premiere_range_preserves_the_exclusive_end_without_rounding() {
        let sequence = SourceSequence {
            id: "timeline".into(),
            name: "Timeline".into(),
            rate: Rational::integer(24),
            frame_size: None,
            start: None,
            clock: None,
            tracks: vec![],
            placements: vec![],
            disabled_placements: vec![],
            transitions: vec![],
            markers: vec![],
            texts: vec![],
        };
        let mut graph = SourceGraph {
            project: Project { name: "Test".into() },
            bins: vec![],
            media: vec![],
            items: vec![],
            sequences: vec![sequence.clone()],
            losses: vec![],
            sidecars: vec![Sidecar {
                adapter: PREMIERE_ADAPTER.into(),
                object: sequence.id.clone(),
                data: Value::Null,
            }],
            multicams: vec![],
            multicam_uses: vec![],
        };
        for (receipt, expected) in [
            (
                r#"{"in":{"ticks":21168000000,"saved_frame":2},"out":{"ticks":148176000000,"saved_frame":14}}"#,
                Some(TimelineMarks([Some(2), Some(13), Some(2), Some(13)])),
            ),
            (
                r#"{"in":{"ticks":0,"saved_frame":0},"out":{"ticks":148176000000}}"#,
                Some(TimelineMarks([Some(0), Some(13), Some(0), Some(13)])),
            ),
            (r#"{"in":{"ticks":21168000000},"out":{"ticks":148176000001}}"#, None),
            (r#"{"out":{"ticks":148176000001}}"#, None),
            (r#"{"in":{"ticks":148176000000}}"#, None),
            (r#"{"out":{"ticks":0}}"#, None),
            (
                r#"{"in":{"ticks":0}}"#,
                Some(TimelineMarks([Some(0), None, Some(0), None])),
            ),
            (r#"{}"#, None),
            (
                r#"{"in":{"ticks":21168000000}}"#,
                Some(TimelineMarks([Some(2), None, Some(2), None])),
            ),
            (
                r#"{"out":{"ticks":148176000000}}"#,
                Some(TimelineMarks([None, Some(13), None, Some(13)])),
            ),
            (
                r#"{"out":{"ticks":10584000000}}"#,
                Some(TimelineMarks([None, Some(0), None, Some(0)])),
            ),
            (
                r#"{"in":{"ticks":137592000000},"out":{"ticks":148176000000}}"#,
                Some(TimelineMarks([Some(13), Some(13), Some(13), Some(13)])),
            ),
        ] {
            graph.sidecars[0].data = json::parse(receipt).unwrap();
            assert_eq!(TimelineMarks::from_premiere(&graph, &sequence, 14).unwrap(), expected);
        }
        graph.sidecars[0].data = json::parse(r#"{"out":{"ticks":148176000000,"saved_frame":13}}"#).unwrap();
        assert!(TimelineMarks::from_premiere(&graph, &sequence, 14).is_err());
    }
}
