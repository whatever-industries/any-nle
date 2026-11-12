//! Two-key Linear position and uniform scale on the item-source clock.
use crate::{Effect, Rational, Value};

pub const LINEAR_KIND: &str = "picture.motion_linear";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionKey {
    /// Exact seconds on the item-source clock, independent of record position.
    pub at: Rational,
    /// Frame fractions from center, X right and Y down.
    pub position: (f64, f64),
    pub scale_percent: f64,
}

pub fn linear_effect(keys: [MotionKey; 2]) -> Effect {
    let keys = keys.map(|key| {
        Value::Obj(vec![
            ("at".into(), Value::Str(key.at.to_string())),
            (
                "position".into(),
                Value::Arr(vec![
                    Value::Float(key.position.0),
                    Value::Float(key.position.1),
                ]),
            ),
            ("scale_percent".into(), Value::Float(key.scale_percent)),
        ])
    });
    Effect {
        kind: LINEAR_KIND.into(),
        params: Value::Obj(vec![
            ("clock".into(), Value::Str("item_source".into())),
            ("interpolation".into(), Value::Str("linear".into())),
            ("keys".into(), Value::Arr(keys.into())),
        ]),
    }
}

pub fn read_linear(effect: &Effect) -> Option<[MotionKey; 2]> {
    let Value::Obj(fields) = &effect.params else {
        return None;
    };
    if effect.kind != LINEAR_KIND
        || fields.len() != 3
        || !matches!(effect.params.get("clock"), Some(Value::Str(s)) if s == "item_source")
        || !matches!(effect.params.get("interpolation"), Some(Value::Str(s)) if s == "linear")
    {
        return None;
    }
    let Value::Arr(keys) = effect.params.get("keys")? else {
        return None;
    };
    let [first, last] = keys.as_slice() else {
        return None;
    };
    let keys = [read_key(first)?, read_key(last)?];
    (!keys[0].at.is_negative() && keys[0].at < keys[1].at).then_some(keys)
}

fn read_key(value: &Value) -> Option<MotionKey> {
    let Value::Obj(fields) = value else {
        return None;
    };
    let (Some(Value::Str(at)), Some(Value::Arr(position)), Some(scale)) = (
        value.get("at"),
        value.get("position"),
        value.get("scale_percent"),
    ) else {
        return None;
    };
    let [x, y] = position.as_slice() else {
        return None;
    };
    let scale_percent = number(scale)?;
    (fields.len() == 3 && scale_percent > 0.0).then_some(MotionKey {
        at: Rational::parse(at)?,
        position: (number(x)?, number(y)?),
        scale_percent,
    })
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::Float(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}
