//! Saved effect identifiers, Corner Pin coordinates and reflection geometry.
use crate::{Effect, Value};

pub const KIND: &str = "avid.picture_chain";
pub const REFLECTION_KIND: &str = "picture.reflection";
pub const CORNER_PIN_KIND: &str = "picture.corner_pin";

/// Full-image edge coordinates in TL, TR, BR, BL order, followed by source
/// crop margins in LTRB order. Destinations validate their supported geometry.
pub fn corner_pin_geometry(params: &Value) -> Option<([[f64; 2]; 4], [f64; 4])> {
    let Value::Obj(fields) = params else { return None };
    let Value::Arr(points) = params.get("target_corners_tl_tr_br_bl")? else { return None };
    let [tl, tr, br, bl] = points.as_slice() else { return None };
    let source_crop = params.get("source_crop_ltrb");
    if fields.len() != 1 + usize::from(source_crop.is_some()) {
        return None;
    }
    let crop = match source_crop {
        Some(value) => numbers(value)?,
        None => [0.0; 4],
    };
    Some(([numbers(tl)?, numbers(tr)?, numbers(br)?, numbers(bl)?], crop))
}

fn numbers<const N: usize>(value: &Value) -> Option<[f64; N]> {
    let Value::Arr(values) = value else { return None };
    if values.len() != N {
        return None;
    }
    let mut result = [0.0; N];
    for (output, input) in result.iter_mut().zip(values) {
        *output = match input {
            Value::Float(value) => *value,
            Value::Int(value) => *value as f64,
            _ => return None,
        };
    }
    Some(result)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reflection {
    pub horizontal: bool,
    pub vertical: bool,
}

impl Reflection {
    pub fn from_params(params: &Value) -> Option<Self> {
        let Value::Obj(fields) = params else { return None };
        let (Some(Value::Bool(horizontal)), Some(Value::Bool(vertical))) =
            (params.get("horizontal"), params.get("vertical"))
        else {
            return None;
        };
        (fields.len() == 2 && (*horizontal || *vertical)).then_some(Self {
            horizontal: *horizontal,
            vertical: *vertical,
        })
    }

    pub fn effect(self) -> Effect {
        Effect {
            kind: REFLECTION_KIND.into(),
            params: Value::Obj(vec![
                ("horizontal".into(), Value::Bool(self.horizontal)),
                ("vertical".into(), Value::Bool(self.vertical)),
            ]),
        }
    }

    pub fn then(self, other: Self) -> Self {
        Self {
            horizontal: self.horizontal ^ other.horizontal,
            vertical: self.vertical ^ other.vertical,
        }
    }

    pub fn active(self) -> bool {
        self.horizontal || self.vertical
    }

    pub fn point(self, [x, y]: [f64; 2]) -> [f64; 2] {
        [
            if self.horizontal { -x } else { x },
            if self.vertical { -y } else { y },
        ]
    }

    pub fn crop<T>(self, mut edges: [T; 4]) -> [T; 4] {
        if self.horizontal {
            edges.swap(0, 2);
        }
        if self.vertical {
            edges.swap(1, 3);
        }
        edges
    }
}
