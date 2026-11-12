//! Resolve timeline clip labels, owned by the saved edit occurrence.
use crate::Result;
use graph::{source::SourceGraph, Loss, Value};

pub(crate) const ADAPTER: &str = "resolve.clip_label";
const ASPECT: &str = "clip_label";
pub(crate) const REASON: &str =
    "Resolve clip label retained in graph; destination mapping is unqualified";
const PREFIX: [u8; 10] = [0, 0, 0x27, 0x11, 0, 0, 0, 3, 0x80, 0x28];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipLabel(u8);

impl ClipLabel {
    pub(crate) fn observed_code(data: &[u8]) -> Option<i64> {
        match data.strip_prefix(&PREFIX)? {
            [code] => Some(i64::from(*code)),
            _ => None,
        }
    }

    pub(crate) fn prepare(graph: &SourceGraph, owner: &str) -> Result<Option<Self>> {
        let Some(data) = graph.sidecar(ADAPTER, owner) else {
            return Ok(None);
        };
        match data.get("saved_code") {
            Some(Value::Int(code @ 2..=32)) if code % 2 == 0 => Ok(Some(Self(*code as u8))),
            _ => Err(format!("{owner}: unsupported Resolve clip label code").into()),
        }
    }

    pub(crate) fn recover(loss: &Loss) -> bool {
        loss.aspect == ASPECT && loss.reason == REASON
    }

    pub(crate) fn blob(self) -> [u8; 11] {
        let mut bytes = [0; 11];
        bytes[..PREFIX.len()].copy_from_slice(&PREFIX);
        bytes[PREFIX.len()] = self.0;
        bytes
    }
}
