use super::*;
use crate::source::{PictureExtension, Rate, ResizeKey, SourceEvent};

#[derive(Debug)]
pub(super) struct Capture {
    position: i64,
    length: Option<i64>,
    rate: Option<Rate>,
    effect: PictureExtension,
}

impl Capture {
    pub(super) fn new(node: &Node, position: i64, effect: PictureExtension) -> Self {
        Self {
            position,
            length: int(node, "length"),
            rate: float(node, "edit_rate").and_then(Rate::from_avid),
            effect,
        }
    }

    pub(super) fn resolve(&self, single: bool, event: &SourceEvent) -> PictureExtension {
        if event.picture_extension.is_some() || event.event.picture_nested {
            return PictureExtension::Unsupported(
                "nested picture effects require separate qualification",
            );
        }
        if !single
            || event.event.speed.is_some()
            || event.range.as_ref().is_none_or(|r| {
                r.record_start != self.position
                    || Some(r.length) != self.length
                    || Some(r.record_rate) != self.rate
                    || (matches!(self.effect, PictureExtension::Resize(_))
                        && r.source_rate != r.record_rate)
            })
        {
            return PictureExtension::Unsupported(
                "picture effect does not own one qualified edit span",
            );
        }
        self.effect
    }
}

pub(super) fn flip(effect: &str) -> Option<PictureExtension> {
    let (horizontal, vertical) = match effect {
        "EFF2_BLEND_FLIP_VERT" => (false, true),
        "EFF2_BLEND_FLIP_HORZ" => (true, false),
        "EFF2_BLEND_FLIP_BOTH" => (true, true),
        _ => return None,
    };
    Some(PictureExtension::Flip {
        horizontal,
        vertical,
    })
}

const RESIZE_IDS: [&str; 4] = [
    "2581568d7e84d511935a50f857c10000",
    "2681568d7e84d511935a50f857c10000",
    "2981568d7e84d511935a50f857c10000",
    "2a81568d7e84d511935a50f857c10000",
];

fn point(value: &Value) -> Option<(i64, f64)> {
    let Value::Node(n) = value else { return None };
    let Value::List(offset) = n.get("offset")? else {
        return None;
    };
    let [Value::Int(frame), Value::Int(1)] = offset.as_slice() else {
        return None;
    };
    let value = float(n, "value")?;
    (int(n, "timescale") == Some(4) && value.is_finite()).then_some((*frame, value))
}

fn changing(points: &[Value]) -> bool {
    let Some(first) = points.first().and_then(point) else {
        return !points.is_empty();
    };
    points
        .iter()
        .any(|p| point(p).is_none_or(|(_, v)| v != first.1))
}

pub(super) fn resize(ctx: &Ctx<'_>, effect: &Node) -> Option<PictureExtension> {
    let length = int(effect, "length")?;
    if length <= 1 {
        return None;
    }
    let rate = float(effect, "edit_rate").and_then(Rate::from_avid)?;
    let list = ctx.deref(effect.get("param_list"))?;
    let Value::RefList(items) = list.get("items")? else {
        return None;
    };
    let mut curves = [None; 4];
    let mut animated = false;
    let mut unsupported = false;
    let mut independent_scale = None;
    for item in items.iter().filter_map(|r| ctx.deref(Some(r))) {
        let Some(Value::Uuid(uuid)) = item.get("uuid") else {
            continue;
        };
        let id = file_order_hex(uuid);
        if id == "2b81568d7e84d511935a50f857c10000" {
            unsupported |= independent_scale
                .replace(int(item, "value") == Some(0))
                .is_some();
        }
        let Some(curve) = ctx.deref(item.get("control_track")) else {
            continue;
        };
        let Some(Value::List(points)) = curve.get("control_points") else {
            continue;
        };
        let moves = changing(points);
        animated |= moves;
        let Some(axis) = RESIZE_IDS.iter().position(|&known| known == id) else {
            unsupported |= moves;
            continue;
        };
        let pair = (|| {
            if curve.class != *b"PRCL"
                || item.get("enable") != Some(&Value::Bool(true))
                || int(curve, "interp_kind") != Some(3)
                || int(curve, "value_type") != Some(2)
                || int(curve, "extrap_kind") != Some(1)
                || int(curve, "fields") != Some(1)
                || int(curve, "length") != Some(length)
                || float(curve, "edit_rate").and_then(Rate::from_avid) != Some(rate)
            {
                return None;
            }
            let [a, b] = points.as_slice() else {
                return None;
            };
            let ((0, a), (last, b)) = (point(a)?, point(b)?) else {
                return None;
            };
            if last != length - 1 || last <= 0 {
                return None;
            }
            Some([a, b])
        })();
        if let Some(pair) = pair {
            unsupported |= curves[axis].replace(pair).is_some();
        } else {
            unsupported = true;
        }
    }
    if !animated {
        return None;
    }
    let [Some(x), Some(y), Some(sx), Some(sy)] = curves else {
        return Some(PictureExtension::Unsupported(
            "Resize animation lacks four qualified Linear curves",
        ));
    };
    if unsupported
        || independent_scale != Some(true)
        || sx
            .into_iter()
            .chain(sy)
            .any(|v| !(0.0..=10000.0).contains(&v))
    {
        return Some(PictureExtension::Unsupported(
            "Resize animation has unsupported timing, crop, or other controls",
        ));
    }
    Some(PictureExtension::Resize([0, 1].map(|k| ResizeKey {
        frame: if k == 0 { 0 } else { length - 1 },
        position: [x[k] / 999.0, y[k] / 999.0],
        scale: [sx[k], sy[k]],
    })))
}
