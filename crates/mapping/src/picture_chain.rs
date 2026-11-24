//! Map an ordered Avid picture chain onto the Motion and Corner Pin stages
//! available to the Premiere and Resolve writers. Each adapter checks its own
//! geometry and decides how to report an unsupported stage.
use graph::picture_chain::{Reflection, CORNER_PIN_KIND, KIND, REFLECTION_KIND};
use graph::{Effect, Transform, Value};

pub fn is_reflection_kind(kind: &str) -> bool {
    matches!(kind, REFLECTION_KIND | "avid.flip" | "resolve.flip")
}

pub fn is_corner_pin_stage(stage: &Value) -> bool {
    matches!(stage.get("kind"), Some(Value::Str(kind)) if kind == CORNER_PIN_KIND)
}

pub struct OrderedCornerPin<'a> {
    pub pin_params: &'a Value,
    pub reflection: Option<Reflection>,
    pub reflection_before_pin: bool,
}

/// Read the isolated pin and its adjacent stage. A missing reflection marks
/// a different kind of adjacent stage for the destination to handle.
pub fn read_ordered_corner_pin<'a>(
    stages: &'a [Value],
    params: &Value,
) -> Result<Option<OrderedCornerPin<'a>>, Error> {
    if !matches!(params, Value::Obj(fields) if fields.len() == 1)
        || !(2..=16).contains(&stages.len())
    {
        return Err(Error::Malformed);
    }
    let [first, second] = stages else { return Ok(None); };
    let (pin, adjacent, reflection_before_pin) = if is_corner_pin_stage(first) {
        (first, second, false)
    } else if is_corner_pin_stage(second) {
        (second, first, true)
    } else {
        return Ok(None);
    };
    if !matches!(pin, Value::Obj(fields) if fields.len() == 2) {
        return Err(Error::Malformed);
    }
    let pin_params = pin.get("params").ok_or(Error::Malformed)?;
    let Some(Value::Str(kind)) = adjacent.get("kind") else {
        return Err(Error::Malformed);
    };
    let reflection = if is_reflection_kind(kind) {
        if !matches!(adjacent, Value::Obj(fields) if fields.len() == 2) {
            return Err(Error::Malformed);
        }
        Some(Reflection::from_params(adjacent.get("params").ok_or(Error::Malformed)?)
            .ok_or(Error::Malformed)?)
    } else {
        None
    };
    Ok(Some(OrderedCornerPin { pin_params, reflection, reflection_before_pin }))
}

pub struct Motion<'a> {
    pub kind: &'a str,
    pub params: &'a Value,
    pub transform: Transform,
}

impl Motion<'_> {
    pub fn is_static(&self) -> bool {
        matches!(self.kind, "avid.resize_static" | "avid.warp_static")
    }
}

/// Both destinations have one native Motion slot. Any number of reflections can
/// be combined per axis on either side of that slot. Keep both parities until
/// the destination has accounted for its own order of crop, flip and motion.
pub struct Chain<'a> {
    pub before: Reflection,
    pub motion: Option<Motion<'a>>,
    pub after: Reflection,
}

#[derive(Debug)]
pub enum Error {
    Malformed,
    Unsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed ordered Avid picture chain",
            Self::Unsupported(reason) => reason,
        })
    }
}
impl std::error::Error for Error {}

impl<'a> Chain<'a> {
    pub fn read(effect: &'a Effect) -> Result<Self, Error> {
        let Value::Obj(fields) = &effect.params else {
            return Err(Error::Malformed);
        };
        let [(name, Value::Arr(stages))] = fields.as_slice() else {
            return Err(Error::Malformed);
        };
        if effect.kind != KIND || name != "stages" || !(2..=16).contains(&stages.len()) {
            return Err(Error::Malformed);
        }
        let mut result = Self {
            before: Reflection::default(),
            motion: None,
            after: Reflection::default(),
        };
        for stage in stages {
            let Value::Obj(fields) = stage else {
                return Err(Error::Malformed);
            };
            let (Some(Value::Str(kind)), Some(params @ Value::Obj(_))) =
                (stage.get("kind"), stage.get("params"))
            else {
                return Err(Error::Malformed);
            };
            if is_reflection_kind(kind) {
                if fields.len() != 2 {
                    return Err(Error::Malformed);
                }
                let flip = Reflection::from_params(params).ok_or(Error::Malformed)?;
                let slot = if result.motion.is_some() {
                    &mut result.after
                } else {
                    &mut result.before
                };
                *slot = slot.then(flip);
            } else {
                if kind == CORNER_PIN_KIND {
                    return Err(Error::Unsupported(
                        "ordered Corner Pin is retained in graph/2; destination effect order is unqualified",
                    ));
                }
                if !matches!(
                    kind.as_str(),
                    "avid.resize_static"
                        | "avid.warp_static"
                        | "avid.resize_linear"
                        | "avid.warp_linear"
                        | "avid.resize_crop_linear"
                        | "avid.warp_crop_linear"
                        | "avid.resize_trim_linear"
                        | "avid.warp_trim_linear"
                        | "avid.resize_trim_crop_linear"
                        | "avid.warp_trim_crop_linear"
                ) {
                    return Err(Error::Unsupported(
                        "ordered picture chain has an unsupported stage",
                    ));
                }
                if result.motion.is_some() {
                    return Err(Error::Unsupported(
                        "ordered picture chain needs more than one Motion stage",
                    ));
                }
                if fields.len() != 3 {
                    return Err(Error::Malformed);
                }
                let transform = read_transform(stage.get("transform").ok_or(Error::Malformed)?)?;
                if transform.rotation != 0.0 || transform.anchor != (0.0, 0.0) {
                    return Err(Error::Unsupported(
                        "ordered picture chain requires neutral rotation and a centered anchor",
                    ));
                }
                let motion = Motion {
                    kind,
                    params,
                    transform,
                };
                if motion.is_static() && !matches!(params, Value::Obj(fields) if fields.is_empty())
                {
                    return Err(Error::Malformed);
                }
                result.motion = Some(motion);
            }
        }
        Ok(result)
    }
}

pub fn read_transform(v: &Value) -> Result<Transform, Error> {
    let Value::Obj(fields) = v else {
        return Err(Error::Malformed);
    };
    if fields.len() != 5 {
        return Err(Error::Malformed);
    }
    let number = |v: &Value| match v {
        Value::Float(f) if f.is_finite() => Ok(*f),
        Value::Int(i) => Ok(*i as f64),
        _ => Err(Error::Malformed),
    };
    let list = |key| match v.get(key) {
        Some(Value::Arr(values)) => Ok(values.as_slice()),
        _ => Err(Error::Malformed),
    };
    let pair = |key| {
        let [a, b] = list(key)? else {
            return Err(Error::Malformed);
        };
        Ok((number(a)?, number(b)?))
    };
    let [l, t, r, b] = list("crop")? else {
        return Err(Error::Malformed);
    };
    Ok(Transform {
        position: pair("position")?,
        scale: pair("scale")?,
        rotation: number(v.get("rotation").ok_or(Error::Malformed)?)?,
        anchor: pair("anchor")?,
        crop: [number(l)?, number(t)?, number(r)?, number(b)?],
    })
}
