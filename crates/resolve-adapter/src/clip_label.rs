//! Resolve timeline clip labels, owned by the saved edit occurrence.
use crate::Result;
use graph::{source::SourceGraph, Loss, Value};

pub(crate) const ADAPTER: &str = "resolve.clip_label";
pub(crate) const PREMIERE_ADAPTER: &str = "premiere.clip_label";
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

    pub(crate) fn saved_code(receipt: &Value) -> Option<i64> {
        match receipt.get("saved_code") {
            Some(Value::Int(code @ 2..=32)) if code % 2 == 0 => Some(*code),
            _ => None,
        }
    }

    pub(crate) fn prepare(graph: &SourceGraph, owner: &str) -> Result<Option<Self>> {
        let resolve = graph.sidecar(ADAPTER, owner);
        let premiere = graph.sidecar(PREMIERE_ADAPTER, owner);
        match (resolve, premiere) {
            (Some(_), Some(_)) => Err(format!("{owner}: conflicting source clip labels").into()),
            (Some(data), None) => Self::saved_code(data)
                .map(|code| Some(Self(code as u8)))
                .ok_or_else(|| format!("{owner}: unsupported Resolve clip label code").into()),
            (None, Some(data)) => Ok(PremiereLabel::read(data)?.map(|label| label.resolve().0)),
            (None, None) => Ok(None),
        }
    }

    pub(crate) fn recover(loss: &Loss) -> bool {
        loss.aspect == ASPECT && loss.reason == REASON
    }

    pub(crate) fn premiere(code: i64) -> Option<(&'static str, &'static str)> {
        // Native clip palette order from RESOLVE-0017. Choose named equivalents
        // first and approximate the other colors using Premiere's standard palette.
        const PALETTE: [(&str, &str); 16] = [
            ("Green", "Green"), ("Teal", "Teal"), ("Blue", "Blue"), ("Purple", "Purple"),
            ("Pink", "Rose"), ("Tan", "Tan"), ("Brown", "Brown"), ("Apricot", "Mango"),
            ("Beige", "Tan"), ("Chocolate", "Brown"), ("Lime", "Green"), ("Navy", "Blue"),
            ("Olive", "Forest"), ("Orange", "Mango"), ("Violet", "Violet"), ("Yellow", "Yellow"),
        ];
        if !(2..=32).contains(&code) || code % 2 != 0 {
            return None;
        }
        PALETTE.get((code / 2 - 1) as usize).copied()
    }

    pub(crate) fn blob(self) -> [u8; 11] {
        let mut bytes = [0; 11];
        bytes[..PREFIX.len()].copy_from_slice(&PREFIX);
        bytes[PREFIX.len()] = self.0;
        bytes
    }
}

/// Premiere stores a preference slot and a cached packed color on the edit.
#[derive(Clone, Copy)]
pub(crate) struct PremiereLabel {
    pub(crate) index: usize,
    pub(crate) packed: i64,
}

impl PremiereLabel {
    pub(crate) fn parse(name: &str, packed: &str) -> Option<Self> {
        let index: usize = name.strip_prefix(prproj::convert::markers::LABEL_PREFIX)?.parse().ok()?;
        let packed = packed.parse().ok()?;
        (index < prproj::convert::colors::LABEL_NAMES.len() && (0..=0xff_ffff).contains(&packed))
            .then_some(Self { index, packed })
    }

    fn read(data: &Value) -> Result<Option<Self>> {
        let (name, packed) = match (data.get("label_name"), data.get("packed_color")) {
            (Some(Value::Str(name)), Some(Value::Str(packed))) => (name, packed),
            _ => return Ok(None),
        };
        match data.get("default_matches_master") {
            Some(Value::Bool(true)) => Ok(None),
            Some(Value::Bool(false)) => Ok(Self::parse(name, packed)),
            _ => Err("invalid Premiere clip label receipt".into()),
        }
    }

    pub(crate) fn resolve(self) -> (ClipLabel, &'static str, &'static str) {
        // Resolve code and label, in Premiere's default palette order.
        const MAP: [(u8, &str); 16] = [
            (30, "Violet"), (24, "Navy"), (4, "Teal"), (30, "Violet"),
            (6, "Blue"), (26, "Olive"), (10, "Pink"), (28, "Orange"),
            (8, "Purple"), (6, "Blue"), (4, "Teal"), (10, "Pink"),
            (12, "Tan"), (2, "Green"), (14, "Brown"), (32, "Yellow"),
        ];
        let (code, destination) = MAP[self.index];
        (ClipLabel(code), prproj::convert::colors::LABEL_NAMES[self.index], destination)
    }
}
