//! Source chain decoding and reflection math shared by destination adapters.
use crate::{Effect, Transform, Value};

pub const KIND: &str = "avid.picture_chain";
pub const REFLECTION_KIND: &str = "picture.reflection";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reflection {
    pub horizontal: bool,
    pub vertical: bool,
}

impl Reflection {
    pub fn is_effect_kind(kind: &str) -> bool {
        matches!(kind, REFLECTION_KIND | "avid.flip" | "resolve.flip")
    }

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
            if Reflection::is_effect_kind(kind) {
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

fn read_transform(v: &Value) -> Result<Transform, Error> {
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
