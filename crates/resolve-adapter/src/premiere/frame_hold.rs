//! Capture Premiere's saved source-timecode Frame Hold.
use crate::{retime::hold_count, Result};
use graph::{source::SourceGraph, Sidecar, Value};
use prproj::model::{Project, TrackItemId};

pub(super) struct FrameHold(i64);

impl FrameHold {
    pub(super) fn read(project: &Project, clip: TrackItemId) -> Result<Option<Self>> {
        let clip = project.track_item(clip).clip;
        let tree = &project.doc.tree;
        match (tree.findtext(clip, "FrameHold")?, tree.findtext(clip, "FrameHoldStart")?) {
            (None, None) => Ok(None),
            (Some(mode), Some(ticks)) if mode == "5" && tree.children(clip).len() == 3 => {
                Ok(Some(Self(ticks.parse()?)))
            }
            _ => Err("unsupported Premiere Frame Hold mode".into()),
        }
    }

    pub(super) fn capture(
        self,
        occurrence: &str,
        span: i64,
        frame_ticks: i64,
        available: i64,
        used: &mut usize,
        graph: &mut SourceGraph,
    ) -> Result<Vec<i64>> {
        let ticks = self.0;
        let frame = ticks / frame_ticks;
        if ticks < 0 || ticks % frame_ticks != 0 || frame >= available {
            return Err("Premiere Frame Hold selects a picture outside the exact source grid".into());
        }
        let count = hold_count(span / frame_ticks).ok_or("unsupported Premiere Frame Hold length")?;
        *used = used.checked_add(count)
            .filter(|count| *count <= graph::MAX_GRAPH_BYTES / size_of::<i64>())
            .ok_or("Premiere held-frame map budget exceeded")?;
        graph.sidecars.push(Sidecar {
            adapter: "premiere.frame_hold".into(),
            object: occurrence.into(),
            data: Value::Obj(vec![
                ("mode".into(), Value::Int(5)),
                ("source_ticks".into(), Value::Int(ticks)),
            ]),
        });
        Ok(vec![frame; count])
    }
}
