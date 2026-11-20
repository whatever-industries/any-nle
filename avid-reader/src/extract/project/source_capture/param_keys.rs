//! Bounded observations of numeric PRCL keys. Times remain in saved coordinates.
use super::*;

pub(super) fn key_parts(raw: &Value) -> Option<(&Node, i64, i64, i64, f64)> {
    let Value::Node(node) = raw else { return None; };
    let Value::List(offset) = node.get("offset")? else { return None; };
    let [Value::Int(num), Value::Int(den)] = offset.as_slice() else { return None; };
    let value = match node.get("value")? {
        Value::Float(value) => *value,
        Value::Int(value) => *value as f64,
        _ => return None,
    };
    (*den > 0 && value.is_finite()).then_some((node, *num, *den, int(node, "timescale")?, value))
}

pub(super) fn observe(points: &[Value], budget: &SharedBudget) -> Option<Vec<SourceParamKey>> {
    const MAX_KEYS: usize = 64;
    if points.len() > MAX_KEYS { return None; }
    let mut keys = Vec::new();
    if !reserve(budget, &mut keys, points.len()) { return None; }
    for raw in points {
        let Some(key) = observe_key(raw) else {
            budget.borrow_mut().release(keys.capacity() * size_of::<SourceParamKey>());
            return None;
        };
        keys.push(key);
    }
    Some(keys)
}

fn observe_key(raw: &Value) -> Option<SourceParamKey> {
    let (node, num, den, timescale, value) = key_parts(raw)?;
    let Value::List(properties) = node.get("pp")? else { return None; };
    let mut key = SourceParamKey {
        time: (num, den), timescale, value, handles: [None; 4], anchor: None, flags: None,
    };
    for property in properties {
        let Value::Node(property) = property else { return None; };
        match int(property, "code")? {
            code @ 5..=8 if int(property, "type") == Some(2) => {
                let v = float(property, "value").filter(|v| v.is_finite())?;
                if key.handles[(code - 5) as usize].replace(v).is_some() { return None; }
            }
            14 if int(property, "type") == Some(2) => {
                let v = float(property, "value").filter(|v| v.is_finite())?;
                if key.anchor.replace(v).is_some() { return None; }
            }
            9 if int(property, "type") == Some(1) => {
                if key.flags.replace(int(property, "value")?).is_some() { return None; }
            }
            _ => return None,
        }
    }
    Some(key)
}
