//! Resolve's timeline-relative In/Out frames. Out is the last marked frame.
use crate::{source::ADAPTER, Result};
use graph::{source::SourceGraph, Loss, Value};

pub(crate) const FIELD: &str = "saved_timeline_marks";
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
        if values[0] == Some(0) {
            return Err(format!("{id}: Resolve drops a frame-zero In mark on import").into());
        }
        Ok(Some(Self(values)))
    }

    /// Read the saved range, including values that do not survive a Resolve import.
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
