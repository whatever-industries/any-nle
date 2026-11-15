//! Linear mono clip pan automation on the edit-local clock.
use crate::{Effect, Rational, Value};

pub const LINEAR_KIND: &str = "audio.pan_linear";
pub const MAX_KEYS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanKey {
    /// Exact seconds from the start of the edit.
    pub at: Rational,
    /// Bipolar pan. Left is -1, center is 0 and right is +1.
    pub pan: Rational,
}

pub fn valid_keys(keys: &[PanKey], duration: Rational) -> bool {
    (2..=MAX_KEYS).contains(&keys.len())
        && keys.iter().all(|key| {
            !key.at.is_negative() && key.at <= duration
                && (Rational::integer(-1)..=Rational::integer(1)).contains(&key.pan)
        })
        && keys.windows(2).all(|pair| pair[0].at < pair[1].at)
}

pub fn linear_effect(keys: &[PanKey]) -> Effect {
    Effect {
        kind: LINEAR_KIND.into(),
        params: Value::Obj(vec![
            ("clock".into(), Value::Str("edit_local".into())),
            ("interpolation".into(), Value::Str("linear".into())),
            ("keys".into(), Value::Arr(keys.iter().map(|key| {
                Value::Obj(vec![
                    ("at".into(), Value::Str(key.at.to_string())),
                    ("pan".into(), Value::Str(key.pan.to_string())),
                ])
            }).collect())),
        ]),
    }
}

pub fn read_linear(effect: &Effect, duration: Rational) -> Option<Vec<PanKey>> {
    let Value::Obj(fields) = &effect.params else { return None };
    if effect.kind != LINEAR_KIND || fields.len() != 3
        || !matches!(effect.params.get("clock"), Some(Value::Str(s)) if s == "edit_local")
        || !matches!(effect.params.get("interpolation"), Some(Value::Str(s)) if s == "linear")
    {
        return None;
    }
    let Value::Arr(keys) = effect.params.get("keys")? else { return None };
    if !(2..=MAX_KEYS).contains(&keys.len()) {
        return None;
    }
    let keys: Vec<_> = keys.iter().map(|key| {
        let Value::Obj(fields) = key else { return None };
        let (Some(Value::Str(at)), Some(Value::Str(pan))) = (key.get("at"), key.get("pan")) else {
            return None;
        };
        (fields.len() == 2).then_some(PanKey {
            at: Rational::parse(at)?,
            pan: Rational::parse(pan)?,
        })
    }).collect::<Option<_>>()?;
    valid_keys(&keys, duration).then_some(keys)
}
