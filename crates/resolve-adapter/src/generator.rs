//! Resolve Solid Color on the existing linked picture lane.
use crate::Result;
use graph::{
    source::{SourceGraph, SourceSequence},
    Value,
};

pub(crate) const ADAPTER: &str = "resolve.generators";
const PREFIX: &str = "000000020000002f800a2c080a18004a004a2408061a0f0a0d320b01";
const SUFFIX: &str = "0000220f0a0d320b00ffff0000000000000000";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Solid {
    pub(crate) start: i64,
    pub(crate) duration: i64,
    pub(crate) rgb: [u16; 3],
}

pub(crate) fn rgb(encoded: &str) -> Option<[u16; 3]> {
    if encoded.is_empty() {
        return Some([0; 3]);
    }
    let values = encoded.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let bytes = hex::decode(values).ok()?;
    let [255, 255, r0, r1, g0, g1, b0, b1] = bytes.as_slice() else {
        return None;
    };
    Some([
        u16::from_be_bytes([*r0, *r1]),
        u16::from_be_bytes([*g0, *g1]),
        u16::from_be_bytes([*b0, *b1]),
    ])
}

impl Solid {
    pub(crate) fn effect(&self) -> String {
        let [r, g, b] = self.rgb;
        format!("{PREFIX}ffff{r:04x}{g:04x}{b:04x}{SUFFIX}")
    }

    pub(crate) fn receipt(&self, id: &str, track: i64) -> Value {
        Value::Obj(vec![
            ("id".into(), Value::Str(id.into())),
            ("kind".into(), Value::Str("Solid Color".into())),
            ("video_track".into(), Value::Int(track)),
            ("start_frames".into(), Value::Int(self.start)),
            ("duration_frames".into(), Value::Int(self.duration)),
            (
                "rgb16".into(),
                Value::Arr(self.rgb.map(|value| Value::Int(value.into())).into()),
            ),
        ])
    }
}

pub(crate) fn prepare(graph: &SourceGraph, seq: &SourceSequence) -> Result<Option<Solid>> {
    let Some(data) = graph.sidecar(ADAPTER, &seq.id) else {
        return Ok(None);
    };
    let Value::Arr(rows) = data else {
        return Err("invalid Resolve generator record".into());
    };
    let [row] = rows.as_slice() else {
        return Err("expected one Resolve Solid Color generator".into());
    };
    let (Some(Value::Int(start)), Some(Value::Int(duration)), Some(Value::Arr(rgb))) = (
        row.get("start_frames"),
        row.get("duration_frames"),
        row.get("rgb16"),
    ) else {
        return Err("incomplete Resolve Solid Color generator".into());
    };
    if !matches!(row.get("kind"), Some(Value::Str(kind)) if kind == "Solid Color")
        || row.get("video_track") != Some(&Value::Int(1))
        || *start < 0
        || *duration <= 0
    {
        return Err("expected a positive Resolve Solid Color V1 span".into());
    }
    let [Value::Int(r), Value::Int(g), Value::Int(b)] = rgb.as_slice() else {
        return Err("invalid Resolve generator color".into());
    };
    Ok(Some(Solid {
        start: *start,
        duration: *duration,
        rgb: [u16::try_from(*r)?, u16::try_from(*g)?, u16::try_from(*b)?],
    }))
}
