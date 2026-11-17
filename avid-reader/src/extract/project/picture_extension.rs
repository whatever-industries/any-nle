use super::*;
mod trim_defaults;
mod spectramatte;
pub(super) use spectramatte::spectramatte_warp;
pub(super) use spectramatte::ID as SPECTRAMATTE_ID;
use crate::source::{PictureExtension, PictureStage, Rate, ResizeKey, SourceEvent, LinearTrim, LinearTrimKey, WarpRollInterpolation, WarpRollKey};

#[derive(Debug)]
pub(super) struct Capture {
    position: i64,
    length: Option<i64>,
    rate: Option<Rate>,
    effect: PictureExtension,
}

#[derive(Debug)]
pub(super) struct OrderedChain {
    pub(super) position: i64,
    pub(super) length: i64,
    pub(super) rate: Rate,
    pub(super) stages: Vec<PictureStage>,
}

/// Verify the native input spine, then reuse the decoded stages from the
/// postorder program. Resolving to one event alone cannot prove a single input.
pub(super) fn ordered_chain<'a>(
    ctx: &Ctx<'a>,
    mut node: &'a Node,
    ops: &[Op],
    before: usize,
    position: i64,
) -> Option<OrderedChain> {
    if node.class != *b"TKFX" {
        return None;
    }
    let [Op::Source(_), remaining @ ..] = ops else { return None };
    let length = int(node, "length").filter(|n| *n > 0)?;
    let rate = float(node, "edit_rate").and_then(Rate::from_avid)?;
    let mut effects = 0;
    let mut depth = 0;
    loop {
        if depth > MAX_NESTING {
            return None;
        }
        if int(node, "media_kind_id") != Some(KIND_PICTURE)
            || node.get("edit_rate").is_some() && float(node, "edit_rate").and_then(Rate::from_avid) != Some(rate)
        {
            return None;
        }
        node = match &node.class {
            b"SCLP" if int(node, "length") == Some(length) => break,
            b"SEQU" => single_picture_segment(ctx, node, length)?,
            b"TKFX" if int(node, "length") == Some(length) => {
                effects += 1;
                ordered_picture_input(ctx, node, length, rate)?
            }
            _ => return None,
        };
        depth += 1;
    }
    if effects < 2 {
        return None;
    }

    let mut stages = Vec::with_capacity(effects);
    for op in remaining {
        match op {
            Op::Picture { before: start, value } if *start == before => {
                stages.push(PictureStage::Static(value.clone()));
            }
            Op::PictureExtension { before: start, capture } if *start == before => {
                match capture.effect {
                    PictureExtension::Unsupported(_) | PictureExtension::UnsupportedTransform(_) => return None,
                    PictureExtension::Flip { .. } => {}
                    _ => {
                        // The animation supersedes the inline values of this
                        // same TKFX. The static operation immediately precedes it.
                        let Some(PictureStage::Static(_)) = stages.pop() else { return None };
                    }
                }
                stages.push(PictureStage::Extension(capture.effect));
            }
            Op::PictureChain { before: start, .. } if *start == before => {}
            _ => return None,
        }
    }
    (stages.len() == effects).then_some(OrderedChain { position, length, rate, stages })
}

fn single_picture_segment<'a>(ctx: &Ctx<'a>, sequence: &Node, length: i64) -> Option<&'a Node> {
    let Value::RefList(children) = sequence.get("components")? else { return None };
    let mut picture = None;
    for child in children {
        let child = ctx.deref(Some(child))?;
        match int(child, "length")? {
            0 if child.class == *b"FILL" => {}
            n if n == length && child.class != *b"FILL" && picture.is_none() => picture = Some(child),
            _ => return None,
        }
    }
    picture
}

fn ordered_picture_input<'a>(ctx: &Ctx<'a>, effect: &Node, length: i64, rate: Rate) -> Option<&'a Node> {
    // Work from the raw list so dangling inputs cannot look like one valid input.
    let Value::List(tracks) = effect.get("tracks")? else { return None };
    let input = |track: &Value| {
        let Value::Node(track) = track else { return None };
        ctx.deref(track.get("component"))
    };
    if float(effect, "edit_rate").and_then(Rate::from_avid) != Some(rate) {
        return None;
    }
    match (effect.get("effect_id")?.as_str()?, tracks.as_slice()) {
        ("EFF2_BLEND_RESIZE", [picture]) => input(picture),
        (name, [picture]) if flip(name).is_some() => input(picture),
        ("EFF2_SBLEND" | spectramatte::ID, [background, picture]) => {
            let (Value::Node(back_track), Value::Node(picture_track)) = (background, picture) else { return None };
            if (int(back_track, "index"), int(picture_track, "index")) != (Some(1), Some(2)) {
                return None;
            }
            let background = input(background)?;
            let picture = input(picture)?;
            let fill_length = |node: &Node| {
                (node.class == *b"FILL" && int(node, "media_kind_id") == Some(KIND_PICTURE))
                    .then(|| int(node, "length").filter(|n| *n >= 0)).flatten()
            };
            let span = if background.class == *b"SEQU" {
                let Value::RefList(children) = background.get("components")? else { return None };
                children.iter().try_fold(0i64, |sum, child| {
                    sum.checked_add(fill_length(ctx.deref(Some(child))?)?)
                })?
            } else {
                fill_length(background)?
            };
            (span == length && int(background, "media_kind_id") == Some(KIND_PICTURE))
                .then_some(picture)
        }
        _ => None,
    }
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
        let replaces_static_transform = (self.effect.replaces_static_transform() && !event.event.picture_nested)
            || event.picture_extension.as_deref().is_some_and(PictureExtension::replaces_static_transform);
        let unsupported = |reason| {
            if replaces_static_transform {
                PictureExtension::UnsupportedTransform(reason)
            } else {
                PictureExtension::Unsupported(reason)
            }
        };
        if event.picture_extension.is_some() || event.event.picture_nested {
            return unsupported("nested picture effects require separate qualification");
        }
        if !single
            || event.event.speed.is_some()
            || event.range.as_ref().is_none_or(|r| {
                r.record_start != self.position
                    || Some(r.length) != self.length
                    || Some(r.record_rate) != self.rate
                    || (matches!(self.effect, PictureExtension::Resize(_) | PictureExtension::ResizeCrop(_) | PictureExtension::Warp(_) | PictureExtension::WarpRoll { .. } | PictureExtension::WarpTrim(_) | PictureExtension::ResizeTrim(_))
                        && r.source_rate != r.record_rate)
            })
        {
            return unsupported("picture effect does not own one qualified edit span");
        }
        self.effect
    }
}

pub(super) fn flip(effect: &str) -> Option<PictureExtension> {
    let (native_id, horizontal, vertical) = [
        ("EFF2_BLEND_FLIP_VERT", false, true),
        ("EFF2_BLEND_FLIP_HORZ", true, false),
        ("EFF2_BLEND_FLIP_BOTH", true, true),
    ].into_iter().find(|(id, _, _)| *id == effect)?;
    Some(PictureExtension::Flip {
        native_id,
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

fn point_coordinates(n: &Node) -> Option<(i64, f64)> {
    let Value::List(offset) = n.get("offset")? else {
        return None;
    };
    let [Value::Int(frame), Value::Int(1)] = offset.as_slice() else {
        return None;
    };
    let value = float(n, "value")?;
    value.is_finite().then_some((*frame, value))
}

fn point(value: &Value) -> Option<(i64, f64)> {
    point_with_flag(value, 4)
}

fn point_with_flag(value: &Value, flag: i64) -> Option<(i64, f64)> {
    let Value::Node(n) = value else { return None; };
    if int(n, "timescale") != Some(flag) {
        return None;
    }
    point_coordinates(n)
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
    let has_multi_key_motion = items.iter().filter_map(|r| ctx.deref(Some(r))).any(|item| {
        matches!(item.get("uuid"), Some(Value::Uuid(id)) if RESIZE_IDS.contains(&file_order_hex(id).as_str()))
            && matches!(ctx.deref(item.get("control_track")).and_then(|c| c.get("control_points")),
                Some(Value::List(points)) if points.len() > 2)
    });
    if has_multi_key_motion {
        return Some(resize_trim(ctx, effect, list, items, length, rate)
            .map(PictureExtension::ResizeTrim)
            .unwrap_or(PictureExtension::UnsupportedTransform(
                "Trimmed Resize requires qualified Linear motion and crop with a neutral background",
            )));
    }

    if items.iter().filter_map(|r| ctx.deref(Some(r))).any(|item| {
        matches!(item.get("uuid"), Some(Value::Uuid(uuid)) if RESIZE_CROP.contains(&file_order_hex(uuid).as_str()))
            && has_curve(item)
            && ctx.deref(item.get("control_track")).and_then(|curve| curve.get("control_points"))
                .is_none_or(|points| {
                    let Value::List(values) = points else { return true; };
                    values.is_empty() || values.iter().any(|p| point(p).is_none_or(|(_, v)|
                        Some(v) != float(item, "value")))
                })
    }) {
        return Some(resize_crop(ctx, effect, list, items, length, rate)
            .map(PictureExtension::ResizeCrop)
            .unwrap_or(PictureExtension::UnsupportedTransform(
                "Resize crop requires the qualified Linear motion and crop controls with neutral background",
            )));
    }
    let mut curves = [None; 4];
    let mut animated = false;
    let mut unsupported = false;
    let mut independent_scale = None;
    let mut background_seen = [false; 3];
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
        if let Some(index) = RESIZE_BACKGROUND.iter().position(|(key, _)| *key == id) {
            unsupported |= std::mem::replace(&mut background_seen[index], true);
            unsupported |= float(item, "value") != Some(0.0) && int(item, "value") != Some(0);
            if has_curve(item) {
                let pair = ctx.deref(item.get("control_track")).and_then(|curve| {
                    let Value::List(points) = curve.get("control_points")? else {
                        return None;
                    };
                    linear_pair(item, curve, points, length, rate)
                });
                unsupported |= pair != Some([0.0; 2]);
                animated |= pair.is_none_or(|[first, last]| first != last);
            }
            continue;
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
        let pair = linear_pair(item, curve, points, length, rate);
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
        crop: [0.0; 4],
    })))
}

const RESIZE_CROP: [&str; 4] = [
    "2e81568d7e84d511935a50f857c10000", // left
    "3081568d7e84d511935a50f857c10000", // top
    "2f81568d7e84d511935a50f857c10000", // right
    "3181568d7e84d511935a50f857c10000", // bottom
];
const RESIZE_SWITCHES: [(&str, f64); 3] = [
    ("2b81568d7e84d511935a50f857c10000", 0.0), // Fixed Aspect
    ("4d81568d7e84d511935a50f857c10000", 0.0),
    ("5381568d7e84d511935a50f857c10000", 0.0),
];
const RESIZE_BACKGROUND: [(&str, f64); 3] = [
    ("4781568d7e84d511935a50f857c10000", 0.0),
    ("4881568d7e84d511935a50f857c10000", 0.0),
    ("4981568d7e84d511935a50f857c10000", 0.0),
];

fn resize_crop(ctx: &Ctx<'_>, effect: &Node, list: &Node, items: &[Value], length: i64, rate: Rate) -> Option<[ResizeKey; 2]> {
    let tracks = ctx.tracks(effect);
    if !qualified_controls(ctx, effect, list, rate, "Resize")
        || tracks.iter().map(|track| int(track, "index")).ne([Some(1)])
    {
        return None;
    }
    let pairs = control_pairs(
        ctx, items, length, rate, &RESIZE_IDS, &RESIZE_CROP, &RESIZE_SWITCHES, &RESIZE_BACKGROUND,
    )?;
    normalize(length, pairs, false)
}

// AVB-0033 and AVB-0037. The promoted effect stores active Position Y in the opposite
// direction. The other three curve values share the Resize coordinate space.
const WARP_MOTION: [&str; 4] = [
    "aa81568d7e84d511935a50f857c10000",
    "ab81568d7e84d511935a50f857c10000",
    "b381568d7e84d511935a50f857c10000",
    "b481568d7e84d511935a50f857c10000",
];
const WARP_SWITCHES: [(&str, f64); 7] = [
    ("2b81568d7e84d511935a50f857c10000", 0.0), // Fixed Aspect
    ("4d81568d7e84d511935a50f857c10000", 0.0),
    ("5381568d7e84d511935a50f857c10000", 0.0),
    ("a981568d7e84d511935a50f857c10000", 1.0), // Position
    ("b281568d7e84d511935a50f857c10000", 1.0), // Scaling
    ("cb81568d7e84d511935a50f857c10000", 1.0), // Crop
    ("0682568d7e84d511935a50f857c10000", 1.0), // Background Color
];
const WARP_CROP: [&str; 4] = [
    "cc81568d7e84d511935a50f857c10000", // left
    "ce81568d7e84d511935a50f857c10000", // top
    "cd81568d7e84d511935a50f857c10000", // right
    "cf81568d7e84d511935a50f857c10000", // bottom
];
const WARP_BACKGROUND: [(&str, f64); 3] = [
    ("0782568d7e84d511935a50f857c10000", 0.0),
    ("0882568d7e84d511935a50f857c10000", 0.0),
    ("0982568d7e84d511935a50f857c10000", 0.0),
];

fn linear_pair(
    item: &Node,
    curve: &Node,
    points: &[Value],
    length: i64,
    rate: Rate,
) -> Option<[f64; 2]> {
    if !curve_header(item, curve, length, rate, 3) {
        return None;
    }
    let [a, b] = points else { return None; };
    let ((0, a), (last, b)) = (point(a)?, point(b)?) else { return None; };
    (last == length - 1 && last > 0).then_some([a, b])
}

fn curve_header(item: &Node, curve: &Node, length: i64, rate: Rate, mode: i64) -> bool {
    curve.class == *b"PRCL"
        && item.get("enable") == Some(&Value::Bool(true))
        && int(curve, "interp_kind") == Some(mode)
        && int(curve, "value_type") == Some(2)
        && int(curve, "extrap_kind") == Some(1)
        && int(curve, "fields") == Some(1)
        && int(curve, "length") == Some(length)
        && float(curve, "edit_rate").and_then(Rate::from_avid) == Some(rate)
}

fn trim_point(p: &Value, flag: i64) -> Option<(i64, f64)> {
    let Value::Node(node) = p else { return None; };
    let Value::List(props) = node.get("pp")? else { return None; };
    let [a, b, c, d, e] = props.as_slice() else { return None; };
    for (value, code) in [a, b, c, d, e].into_iter().zip(5..=9) {
        let Value::Node(prop) = value else { return None; };
        if int(prop, "code") != Some(code)
            || (if code == 9 {
                int(prop, "type") != Some(1) || int(prop, "value") != Some(1)
            } else {
                int(prop, "type") != Some(2) || float(prop, "value").is_none_or(|v| !v.is_finite())
            })
        {
            return None;
        }
    }
    point_with_flag(p, flag)
}

// The native Resize returns retain all three points even when one or more
// lie beyond the edit. Treat their times as saved facts, independent of flag.
fn resize_trim(ctx: &Ctx<'_>, effect: &Node, list: &Node, items: &[Value], length: i64, rate: Rate) -> Option<LinearTrim> {
    if !qualified_controls(ctx, effect, list, rate, "Resize")
        || ctx.tracks(effect).iter().map(|track| int(track, "index")).ne([Some(1)])
        || items.len() != RESIZE_IDS.len() + RESIZE_CROP.len() + RESIZE_SWITCHES.len() + RESIZE_BACKGROUND.len()
    {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut motion = [None; 4];
    let mut crop = [None; 4];
    let mut fixed_width = false;
    let mut neutral_times = None;
    for item in items {
        let item = ctx.deref(Some(item))?;
        let Value::Uuid(uuid) = item.get("uuid")? else { return None; };
        let id = file_order_hex(uuid);
        if item.class != *b"PRIT" || !seen.insert(id.clone())
            || item.get("enable") != Some(&Value::Bool(true))
        {
            return None;
        }
        if let Some((_, expected)) = RESIZE_SWITCHES.iter().find(|(key, _)| *key == id) {
            if has_curve(item) || int(item, "value").map(|n| n as f64) != Some(*expected) {
                return None;
            }
            continue;
        }
        let curve = ctx.deref(item.get("control_track"))?;
        let Value::List(points) = curve.get("control_points")? else { return None; };
        let [a, b, c] = points.as_slice() else { return None; };
        if !curve_header(item, curve, length, rate, 3) {
            return None;
        }
        let axis = RESIZE_IDS.iter().position(|key| *key == id);
        let flag = if axis == Some(2) && point_with_flag(a, 1).is_some() { 1 } else { 4 };
        let keys = [trim_point(a, flag)?, trim_point(b, flag)?, trim_point(c, flag)?];
        if let Some(axis) = axis {
            motion[axis] = Some(keys);
            fixed_width |= flag == 1;
        } else if let Some(edge) = RESIZE_CROP.iter().position(|key| *key == id) {
            crop[edge] = Some(keys);
        } else {
            let known = RESIZE_BACKGROUND.iter().any(|(key, _)| *key == id);
            let times = keys.map(|(t, _)| t);
            if !known || keys.iter().any(|(_, value)| *value != 0.0)
                || neutral_times.replace(times).is_some_and(|old| old != times)
            {
                return None;
            }
        }
    }
    let [Some(x), Some(y), Some(width), Some(height)] = motion else { return None; };
    let last = length - 1;
    let times = x.map(|(t, _)| t);
    let width_times = width.map(|(t, _)| t);
    let covers_edit = |[a,b,c]: [i64; 3]| a <= 0 && c >= last && a < b && b < c;
    if !covers_edit(times) || !covers_edit(width_times)
        || !((times[0] == 0 && width_times[0] == 0) || (times[2] == last && width_times[2] == last))
        || neutral_times != Some(times) || y.map(|(t, _)| t) != times || height.map(|(t, _)| t) != times
        || (!fixed_width && width_times != times)
        || width.iter().chain(&height).any(|(_, v)| *v <= 0.0 || *v > 10_000.0)
    {
        return None;
    }
    let motion = [0, 1, 2].map(|i| LinearTrimKey {
        frame: times[i],
        position: [x[i].1 / 999.0, y[i].1 / 999.0],
        height: height[i].1,
    });
    Some(LinearTrim {
        motion, width, fixed_width,
        crop: trim_crop(crop, times, last, false)?,
    })
}

fn warp_trim(
    ctx: &Ctx<'_>,
    items: &[Value],
    length: i64,
    rate: Rate,
    compact_promotion: bool,
) -> Option<LinearTrim> {
    use trim_defaults::WARP_TRIM_DEFAULTS;
    let mut seen = std::collections::HashSet::new();
    let mut motion = [None; 4];
    let mut crop = [None; 4];
    let mut fixed_width = false;
    let mut neutral_times = None;
    let mut midpoint = None;
    for item in items {
        let item = ctx.deref(Some(item))?;
        let Value::Uuid(uuid) = item.get("uuid")? else { return None; };
        let id = file_order_hex(uuid);
        if item.class != *b"PRIT" || !seen.insert(id.clone()) || item.get("enable") != Some(&Value::Bool(true)) {
            return None;
        }
        if let Some((_, expected)) = WARP_SWITCHES.iter().find(|(key, _)| *key == id) {
            if has_curve(item) || float(item, "value").or_else(|| int(item, "value").map(|v| v as f64)) != Some(*expected) {
                return None;
            }
            continue;
        }
        let curve = ctx.deref(item.get("control_track"))?;
        let Value::List(points) = curve.get("control_points")? else { return None; };
        if let Ok(index) = WARP_TRIM_DEFAULTS.binary_search_by_key(&id.as_str(), |(key, _, _)| *key) {
            if compact_promotion {
                return None;
            }
            let (_, mode, expected) = WARP_TRIM_DEFAULTS[index];
            let [only] = points.as_slice() else { return None; };
            let (at, value) = trim_point(only, 4)?;
            if !curve_header(item, curve, length, rate, mode) || value != expected
                || float(item, "value") != Some(0.0)
                || midpoint.replace(at).is_some_and(|old| old != at)
            {
                return None;
            }
            continue;
        }
        if !curve_header(item, curve, length, rate, 3) {
            return None;
        }
        let [a, b, c] = points.as_slice() else { return None; };
        let axis = WARP_MOTION.iter().position(|key| *key == id);
        fixed_width |= !compact_promotion && axis == Some(2) && trim_point(a, 1).is_some();
        let flag = if axis == Some(2) && fixed_width { 1 } else { 4 };
        let keys = [trim_point(a, flag)?, trim_point(b, flag)?, trim_point(c, flag)?];
        if let Some(axis) = axis {
            motion[axis] = Some(keys);
        } else if let Some(edge) = WARP_CROP.iter().position(|key| *key == id) {
            crop[edge] = Some(keys);
        } else {
            let (_, neutral) = WARP_BACKGROUND.iter().find(|(key, _)| *key == id)?;
            if keys.iter().any(|(_, v)| v != neutral)
                || neutral_times.replace(keys.map(|(t, _)| t)).is_some_and(|old| old != keys.map(|(t, _)| t))
            {
                return None;
            }
        }
    }
    let [Some(x), Some(y), Some(width), Some(height)] = motion else { return None; };
    let last = length - 1;
    let times = x.map(|(t, _)| t);
    let [a, b, c] = width.map(|(t, _)| t);
    let width_span = if fixed_width {
        (a < 0 && c == last) || (a == 0 && c > last)
    } else {
        [a, b, c] == times
    };
    if neutral_times != Some(times) || (!compact_promotion && midpoint != Some(times[1]))
        || times[0] != 0 || times[2] != last || !(0 < times[1] && times[1] < last)
        || y.map(|(t, _)| t) != times || height.map(|(t, _)| t) != times
        || !width_span || !(0 < b && b < last)
        || width.iter().chain(&height).any(|(_, v)| *v <= 0.0 || *v > 10_000.0)
    {
        return None;
    }
    let motion = [0, 1, 2].map(|i| LinearTrimKey {
        frame: times[i],
        position: [x[i].1 / 999.0, -y[i].1 / 999.0],
        height: height[i].1,
    });
    let crop = trim_crop(crop, times, last, true)?;
    if compact_promotion && !crop.is_some_and(|keys| keys.map(|(frame, _)| frame) == times) {
        return None;
    }
    Some(LinearTrim {
        motion, width, fixed_width,
        crop,
    })
}

fn has_curve(item: &Node) -> bool {
    !matches!(item.get("control_track"), None | Some(Value::Null | Value::Ref(0)))
}

pub(super) fn static_projective_warp(ctx: &Ctx<'_>, effect: &Node) -> Option<crate::transform::CornerPin> {
    let mut values = compact_static_warp_controls(ctx, effect, true)?;
    let count = values.len() + crate::transform::COMPACT_WARP_DEFAULTS.len();
    let mut take = |prefix: &str| values.remove(&format!("{prefix}568d7e84d511935a50f857c10000"));
    let (corners, from_transform) = match (take("0a82"), take("c881")) {
        (Some(1.0), None) if matches!(count, 18 | 23) => {
            let mut corners = [[0.0; 2]; 4];
            for (corner, (x, y)) in corners.iter_mut().zip([
                ("0b82", "0c82"), ("0d82", "0e82"), ("0f82", "1082"), ("1182", "1282"),
            ]) {
                *corner = [0.5 + take(x)? / 999.0, 0.5 - take(y)? / 999.0];
            }
            (corners, false)
        }
        (None, Some(1.0)) if matches!(count, 11..=14) => {
            (skew_corners(&mut take)?, true)
        }
        (None, None) if matches!(count, 13..=15 | 24) => {
            (rotation_corners(&mut take, count == 24)?, true)
        }
        _ => return None,
    };
    let crop = match take("cb81") {
        None => [0.0; 4],
        Some(1.0) => static_warp_crop(&mut take)?,
        _ => return None,
    };
    if !values.is_empty() || !valid_crop(crop)
        || corners.iter().flatten().any(|v| !(-1.0..=2.0).contains(v))
    {
        return None;
    }
    // A convex quad in the observed winding excludes crossed, mirrored and
    // collapsed corners. Leave their behavior to a native render comparison.
    for i in 0..4 {
        let [a, b, c] = [corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4]];
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross <= 1e-9 {
            return None;
        }
    }
    Some(crate::transform::CornerPin { corners, crop, from_transform })
}

pub(super) fn compact_affine_warp(ctx: &Ctx<'_>, effect: &Node) -> bool {
    compact_static_warp_controls(ctx, effect, false).is_some_and(|values| {
        if values.len() == 15 {
            isolated_roll_controls(values)
        } else {
            affine_crop_controls(values)
        }
    })
}

fn isolated_roll_controls(mut values: HashMap<String, f64>) -> bool {
    // The compact static Z Rotation control has four enabled groups.
    // Require neutral position, scale, X/Y rotation, and Z axis offset.
    let mut take = |prefix: &str| values.remove(&format!("{prefix}568d7e84d511935a50f857c10000"));
    for (id, expected) in [
        ("a981", 1.0), ("b281", 1.0), ("b781", 1.0), ("bc81", 1.0),
        ("aa81", 0.0), ("ab81", 0.0), ("ac81", 0.0),
        ("b381", 100.0), ("b481", 100.0),
        ("b881", 0.0), ("b981", 0.0),
        ("bf81", 0.0),
    ] {
        if take(id) != Some(expected) {
            return false;
        }
    }
    let (Some(x), Some(y), Some(roll)) = (take("bd81"), take("be81"), take("ba81")) else {
        return false;
    };
    (-30.0..=30.0).contains(&roll)
        && (0.0..=100.0).contains(&x) && (0.0..=100.0).contains(&y)
        && (x == 0.0 || y == 0.0)
        && ((x == 0.0 && y == 0.0) || (0.0..=30.0).contains(&roll))
        && values.is_empty()
}

fn affine_crop_controls(mut values: HashMap<String, f64>) -> bool {
    let mut take = |prefix: &str| values.remove(&format!("{prefix}568d7e84d511935a50f857c10000"));
    if take("cb81") != Some(1.0) || !static_warp_crop(&mut take).is_some_and(valid_crop) {
        return false;
    }
    match take("b281") {
        None => {}
        Some(1.0) if take("b381").is_some_and(|v| (60.0..=100.0).contains(&v))
            && take("b481").is_some_and(|v| (90.0..=100.0).contains(&v)) => {}
        _ => return false,
    }
    match take("b781") {
        None => {}
        Some(1.0) if take("ba81").is_some_and(|v| (0.0..=30.0).contains(&v)) => {}
        _ => return false,
    }
    values.is_empty()
}

fn static_warp_crop(take: &mut impl FnMut(&str) -> Option<f64>) -> Option<[f64; 4]> {
    Some([
        (take("cc81")? + 999.0) / 1998.0,
        (999.0 - take("ce81")?) / 1998.0,
        (999.0 - take("cd81")?) / 1998.0,
        (take("cf81")? + 999.0) / 1998.0,
    ])
}

fn compact_static_warp_controls(
    ctx: &Ctx<'_>, effect: &Node, allow_reflections: bool,
) -> Option<HashMap<String, f64>> {
    // Match the compact, curve-free controls in the retained geometry renders.
    // Unknown controls may affect geometry or the compositing result.
    let length = int(effect, "length").filter(|&n| n > 0)?;
    let rate = float(effect, "edit_rate").and_then(Rate::from_avid)?;
    if effect.get("effect_id").and_then(Value::as_str) != Some("EFF2_SBLEND")
        || Rate::new(24, 1) != Some(rate)
        || !effect_envelope(ctx, effect, "3DWarp")
    {
        return None;
    }
    let mut input = ordered_picture_input(ctx, effect, length, rate)?;
    for _ in 0..MAX_NESTING {
        match &input.class {
            b"SEQU" if int(input, "media_kind_id") == Some(KIND_PICTURE)
                && (input.get("edit_rate").is_none()
                    || float(input, "edit_rate").and_then(Rate::from_avid) == Some(rate)) => {
                input = single_picture_segment(ctx, input, length)?;
            }
            b"TKFX" if allow_reflections
                && int(input, "length") == Some(length)
                && int(input, "media_kind_id") == Some(KIND_PICTURE)
                && flip(input.get("effect_id").and_then(Value::as_str)?).is_some() => {
                input = ordered_picture_input(ctx, input, length, rate)?;
            }
            _ => break,
        }
    }
    if input.class != *b"SCLP" || int(input, "length") != Some(length)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
        || float(input, "edit_rate").and_then(Rate::from_avid) != Some(rate)
    {
        return None;
    }
    let list = ctx.deref(effect.get("param_list")).filter(|node| node.class == *b"PRLS")?;
    let Value::RefList(items) = list.get("items")? else { return None };
    if !matches!(items.len(), 11..=19 | 23 | 24) {
        return None;
    }
    let mut values = HashMap::new();
    for item in items {
        let item = ctx.deref(Some(item)).filter(|node| node.class == *b"PRIT")?;
        if has_curve(item) || item.get("enable") != Some(&Value::Bool(true)) {
            return None;
        }
        let Value::Uuid(id) = item.get("uuid")? else { return None };
        let value = float(item, "value").or_else(|| int(item, "value").map(|value| value as f64))?;
        if !value.is_finite() || values.insert(file_order_hex(id), value).is_some() {
            return None;
        }
    }
    let mut take = |prefix: &str| values.remove(&format!("{prefix}568d7e84d511935a50f857c10000"));
    for (id, expected) in crate::transform::COMPACT_WARP_DEFAULTS {
        if take(id) != Some(expected) {
            return None;
        }
    }
    Some(values)
}

fn skew_corners(take: &mut impl FnMut(&str) -> Option<f64>) -> Option<[[f64; 2]; 4]> {
    let skew_x = take("c981").unwrap_or(0.0);
    let skew_y = take("ca81").unwrap_or(0.0);
    let roll = match take("b781") {
        None => 0.0,
        Some(1.0) if skew_y == 0.0 => take("ba81")?,
        _ => return None,
    };
    if skew_x.abs() > 200.0 || !(-150.0..=200.0).contains(&skew_y)
        || roll.abs() > 30.0 || (skew_x == 0.0 && skew_y == 0.0)
    {
        return None;
    }
    let (sin, cos) = roll.to_radians().sin_cos();
    Some([[-960.0, -540.0], [960.0, -540.0], [960.0, 540.0], [-960.0, 540.0]].map(|[x, y]| {
        // Both shears use the original coordinates. Avid applies Z rotation
        // after that shear, about the same picture center.
        let sx = x - 1.5 * skew_x / 999.0 * y;
        let sy = y - (2.0 / 3.0) * skew_y / 999.0 * x;
        [0.5 + (cos * sx - sin * sy) / 1920.0, 0.5 + (sin * sx + cos * sy) / 1080.0]
    }))
}

fn rotation_corners(
    take: &mut impl FnMut(&str) -> Option<f64>, model_controls: bool,
) -> Option<[[f64; 2]; 4]> {
    if take("b781") != Some(1.0) {
        return None;
    }
    let pitch = take("b881")?;
    let yaw = take("b981")?;
    let roll = take("ba81")?;
    let max_roll = if model_controls { 35.0 } else { 0.0 };
    if pitch.abs() > 25.0 || yaw.abs() > 30.0 || roll.abs() > max_roll {
        return None;
    }
    // The retained 24-control profile combines model transforms with the
    // default camera. The compact camera profile uses neutral model controls.
    let (scale, axis, position) = if model_controls {
        if [take("a981"), take("b281"), take("bc81")] != [Some(1.0); 3] {
            return None;
        }
        let scale = [take("b381")?, take("b481")?];
        let axis = [take("bd81")?, take("be81")?, take("bf81")?];
        let position = [take("aa81")?, take("ab81")?, take("ac81")?];
        if pitch == 0.0 && yaw == 0.0 {
            // The isolated depth calibration keeps every other model control
            // neutral, with the same enabled gates as the combined profile.
            if roll != 0.0 || scale != [100.0; 2] || axis != [0.0; 3]
                || position[0] != 0.0 || position[1] != 0.0
                || position[2] == 0.0 || position[2].abs() > 100.0
            {
                return None;
            }
        } else if !(60.0..=100.0).contains(&scale[0]) || !(90.0..=100.0).contains(&scale[1])
            || axis.iter().zip([80.0, 60.0, 25.0]).any(|(v, max)| v.abs() > max)
            || position.iter().zip([40.0, 30.0, 20.0]).any(|(v, max)| v.abs() > max)
        {
            return None;
        }
        (scale.map(|v| v / 100.0), axis, position)
    } else {
        if pitch == 0.0 && yaw == 0.0 {
            return None;
        }
        ([1.0; 2], [0.0; 3], [0.0; 3])
    };
    let (camera_x, camera_y, focal) = match take("c481") {
        None => (0.0, 0.0, 2400.0),
        Some(1.0) if !model_controls => {
            let x = take("c581");
            let y = take("c681");
            let z = take("c781");
            if [x, y, z].iter().flatten().count() > 1
                || x.is_some_and(|v| v.abs() > 200.0)
                || y.is_some_and(|v| v.abs() > 200.0)
                || z.is_some_and(|v| !(250.0..=750.0).contains(&v))
            {
                return None;
            }
            (x.unwrap_or(0.0), y.unwrap_or(0.0), z.map_or(2400.0, |v| v * 9600.0 / 999.0))
        }
        _ => return None,
    };
    let [scale_x, scale_y] = scale;
    let units = [1920.0 / 999.0, -1080.0 / 999.0, 9600.0 / 999.0];
    let [axis_x, axis_y, axis_z] = [
        axis[0] * units[0] * scale_x, axis[1] * units[1] * scale_y, axis[2] * units[2],
    ];
    let [tx, ty, tz] = [position[0] * units[0], position[1] * units[1], position[2] * units[2]];
    let (roll_sin, roll_cos) = roll.to_radians().sin_cos();
    let (pitch_sin, pitch_cos) = pitch.to_radians().sin_cos();
    let (yaw_sin, yaw_cos) = yaw.to_radians().sin_cos();
    let mut corners = [[0.0; 2]; 4];
    for (corner, [x, y]) in corners.iter_mut().zip([
        [-960.0, -540.0], [960.0, -540.0], [960.0, 540.0], [-960.0, 540.0],
    ]) {
        let (x, y) = (x * scale_x - axis_x, y * scale_y - axis_y);
        // Avid rotates Z, then X, then Y about the scaled Axis point.
        let (x, y) = (roll_cos * x - roll_sin * y, roll_sin * x + roll_cos * y);
        let z = -axis_z;
        let (y, z) = (pitch_cos * y - pitch_sin * z, pitch_sin * y + pitch_cos * z);
        let (x, z) = (yaw_cos * x + yaw_sin * z, -yaw_sin * x + yaw_cos * z);
        let (x, y, z) = (x + axis_x + tx, y + axis_y + ty, z + axis_z + tz);
        let denominator = 2.0 * focal - 2400.0 - z;
        // Depth is affine across the source plane. Requiring positive depth
        // at all four corners excludes a projective pole anywhere in the image.
        if denominator <= 0.0 {
            return None;
        }
        let magnification = focal / denominator;
        *corner = [
            0.5 + magnification * (x / 1920.0 - camera_x / 999.0),
            0.5 + magnification * (y / 1080.0 + camera_y / 999.0),
        ];
    }
    Some(corners)
}

#[cfg(test)]
mod projective_tests {
    use super::{affine_crop_controls, isolated_roll_controls, rotation_corners, skew_corners};
    use std::collections::HashMap;

    #[test]
    fn compact_roll_requires_all_neutral_controls_and_a_bounded_angle() {
        let id = |prefix: &str| format!("{prefix}568d7e84d511935a50f857c10000");
        let controls: HashMap<_, _> = [
            ("a981", 1.0), ("b281", 1.0), ("b781", 1.0), ("bc81", 1.0),
            ("aa81", 0.0), ("ab81", 0.0), ("ac81", 0.0),
            ("b381", 100.0), ("b481", 100.0),
            ("b881", 0.0), ("b981", 0.0), ("ba81", -30.0),
            ("bd81", 0.0), ("be81", 0.0), ("bf81", 0.0),
        ].into_iter().map(|(key, value)| (id(key), value)).collect();
        assert!(isolated_roll_controls(controls.clone()));
        for (key, value) in [("bd81", 1.0), ("ba81", 31.0), ("bc81", 0.0), ("b881", 1.0), ("bf81", 1.0)] {
            let mut changed = controls.clone();
            changed.insert(id(key), value);
            assert!(!isolated_roll_controls(changed), "{key}");
        }
        let mut axis = controls;
        axis.insert(id("ba81"), 30.0);
        axis.insert(id("bd81"), 100.0);
        assert!(isolated_roll_controls(axis.clone()));
        axis.insert(id("be81"), 100.0);
        assert!(!isolated_roll_controls(axis));
    }

    #[test]
    fn compact_affine_crop_refuses_unmeasured_controls() {
        let measured = [
            ("cb81", 1.0), ("cc81", -799.0), ("cd81", 699.0),
            ("ce81", 839.0), ("cf81", -759.0),
            ("b281", 1.0), ("b381", 60.0), ("b481", 90.0), ("b781", 1.0), ("ba81", 30.0),
        ];
        let id = |prefix: &str| format!("{prefix}568d7e84d511935a50f857c10000");
        let controls: HashMap<_, _> = measured.into_iter().map(|(key, value)| (id(key), value)).collect();
        assert!(affine_crop_controls(controls.clone()));
        for (key, value) in [("b881", 0.0), ("c081", 1.0), ("ba81", 31.0), ("cc81", 800.0)] {
            let mut changed = controls.clone();
            changed.insert(id(key), value);
            assert!(!affine_crop_controls(changed), "{key}");
        }
    }

    #[test]
    fn skew_axes_use_the_same_untransformed_coordinates() {
        let mut values = HashMap::from([("c981", 200.0), ("ca81", -150.0)]);
        let [tl, tr, br, bl] = skew_corners(&mut |key| values.remove(key)).unwrap();
        // The horizontal edge keeps the full source width, even with both axes active.
        assert!((tr[0] - tl[0] - 1.0).abs() < 1e-12);
        assert!((br[1] - tr[1] - 1.0).abs() < 1e-12);
        assert!(tr[1] > tl[1] && bl[0] < tl[0]);
        let mut values = HashMap::from([("c981", 200.0), ("ca81", -150.0), ("b781", 1.0), ("ba81", 30.0)]);
        assert!(skew_corners(&mut |key| values.remove(key)).is_none());
    }

    #[test]
    fn projected_yaw_uses_signed_depth_and_rejects_combined_camera_axes() {
        let controls = [("b781", 1.0), ("b881", 0.0), ("b981", 30.0), ("ba81", 0.0)];
        let mut values = HashMap::from(controls);
        let [tl, tr, br, bl] = rotation_corners(&mut |key| values.remove(key), false).unwrap();
        // Positive yaw brings the left edge closer. Its projected height is
        // 1 / (1 - 480/2400), while the right edge uses the opposite depth.
        assert!((bl[1] - tl[1] - 1.25).abs() < 1e-12);
        assert!((br[1] - tr[1] - 5.0 / 6.0).abs() < 1e-12);
        let mut values = HashMap::from(controls);
        values.extend([("c481", 1.0), ("c581", 100.0), ("c681", 100.0)]);
        assert!(rotation_corners(&mut |key| values.remove(key), false).is_none());
        let mut values = HashMap::from(controls);
        values.extend([("c481", 1.0), ("c781", 0.0)]);
        assert!(rotation_corners(&mut |key| values.remove(key), false).is_none());
    }

    #[test]
    fn rotated_picture_uses_the_scaled_axis_as_its_pivot() {
        let mut values = HashMap::from([
            ("b781", 1.0), ("b881", 0.0), ("b981", 30.0), ("ba81", 0.0),
            ("a981", 1.0), ("b281", 1.0), ("bc81", 1.0),
            ("b381", 60.0), ("b481", 90.0), ("bd81", 50.0), ("be81", 0.0), ("bf81", 0.0),
            ("aa81", 0.0), ("ab81", 0.0), ("ac81", 0.0),
        ]);
        let [tl, tr, br, bl] = rotation_corners(&mut |key| values.remove(key), true).unwrap();
        // For pure yaw the projected vertical edge lengths reveal each edge's
        // depth directly, including the shared shift from the scaled X pivot.
        let left_depth = 2400.0 * (1.0 - 0.9 / (bl[1] - tl[1]));
        let right_depth = 2400.0 * (1.0 - 0.9 / (br[1] - tr[1]));
        assert!((left_depth - right_depth - 576.0).abs() < 1e-9);
        assert!(((left_depth + right_depth) / 2.0 - 28_800.0 / 999.0).abs() < 1e-9);
    }

    #[test]
    fn isolated_position_depth_uses_the_default_camera() {
        let controls = [
            ("b781", 1.0), ("b881", 0.0), ("b981", 0.0), ("ba81", 0.0),
            ("a981", 1.0), ("b281", 1.0), ("bc81", 1.0),
            ("b381", 100.0), ("b481", 100.0), ("bd81", 0.0), ("be81", 0.0), ("bf81", 0.0),
            ("aa81", 0.0), ("ab81", 0.0),
        ];
        for (depth, expected_width) in [(100.0, 999.0 / 599.0), (-100.0, 999.0 / 1399.0)] {
            let mut values = HashMap::from(controls);
            values.insert("ac81", depth);
            let [tl, tr, br, bl] = rotation_corners(&mut |key| values.remove(key), true).unwrap();
            assert!(values.is_empty());
            assert!((tr[0] - tl[0] - expected_width).abs() < 1e-12);
            assert!((bl[1] - tl[1] - expected_width).abs() < 1e-12);
            assert!((tl[0] + br[0] - 1.0).abs() < 1e-12);
            assert!((tl[1] + br[1] - 1.0).abs() < 1e-12);
        }
        for (control, value) in [
            ("ac81", 0.0), ("ac81", 101.0), ("aa81", 1.0), ("b381", 99.0),
            ("ba81", 1.0), ("bd81", 1.0), ("c481", 1.0),
        ] {
            let mut values = HashMap::from(controls);
            values.insert("ac81", 100.0);
            values.insert(control, value);
            assert!(rotation_corners(&mut |key| values.remove(key), true).is_none(), "{control}");
        }
    }
}

// A malformed or unresolved control reference cannot be used as a static value.
fn curve_differs_from_inline(ctx: &Ctx<'_>, item: &Node) -> bool {
    has_curve(item) && ctx.deref(item.get("control_track"))
        .and_then(|curve| curve.get("control_points"))
        .is_none_or(|points| {
            let Value::List(values) = points else { return true; };
            let [single] = values.as_slice() else { return true; };
            point(single).is_none_or(|(_, value)| float(item, "value") != Some(value))
        })
}

fn qualified_controls(ctx: &Ctx<'_>, effect: &Node, list: &Node, rate: Rate, name: &str) -> bool {
    list.class == *b"PRLS" && Rate::new(24_000, 1001) == Some(rate)
        && effect_envelope(ctx, effect, name)
}

fn effect_envelope(ctx: &Ctx<'_>, effect: &Node, name: &str) -> bool {
    let attrs = ctx.attrs(effect);
    let one = |key, expected| attrs.iter().filter(|(k, _)| k == key)
        .map(|(_, v)| v.as_str()).eq([Some(expected)]);
    effect.class == *b"TKFX"
        && matches!(effect.get("precomputed"), None | Some(Value::Null | Value::Ref(0)))
        && matches!(effect.get("trackman"), None | Some(Value::Null | Value::Ref(0)))
        && int(effect, "info_is_reversed") == Some(0)
        && int(effect, "info_smooth") == Some(0)
        && one("_EFFECT_PLUGIN_NAME", name)
        && one("_EFFECT_PLUGIN_PRODUCT_ID", "1cf8c0c4-8536-45ab-aace-02abfeed5fdd")
}

fn control_pairs(
    ctx: &Ctx<'_>,
    items: &[Value],
    length: i64,
    rate: Rate,
    motion: &[&str; 4],
    crop: &[&str; 4],
    switches: &[(&str, f64)],
    background: &[(&str, f64)],
) -> Option<[[f64; 2]; 8]> {
    if items.len() != motion.len() + crop.len() + switches.len() + background.len() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut pairs = [None; 8];
    for item in items {
        let item = ctx.deref(Some(item))?;
        let Value::Uuid(uuid) = item.get("uuid")? else { return None; };
        let id = file_order_hex(uuid);
        if item.class != *b"PRIT" || !seen.insert(id.clone())
            || item.get("enable") != Some(&Value::Bool(true))
        {
            return None;
        }
        if let Some((_, expected)) = switches.iter().find(|(key, _)| *key == id) {
            if has_curve(item) || float(item, "value").or_else(|| int(item, "value").map(|v| v as f64)) != Some(*expected) {
                return None;
            }
            continue;
        }
        let curve = ctx.deref(item.get("control_track"))?;
        let Value::List(points) = curve.get("control_points")? else { return None; };
        let pair = linear_pair(item, curve, points, length, rate)?;
        if let Some(axis) = motion.iter().chain(crop).position(|key| *key == id) {
            pairs[axis] = Some(pair);
        } else if !background.iter().any(|(key, value)| *key == id && pair == [*value; 2]) {
            return None;
        }
    }
    let [Some(x), Some(y), Some(sx), Some(sy), Some(l), Some(t), Some(r), Some(b)] = pairs else { return None; };
    Some([x, y, sx, sy, l, t, r, b])
}

fn normalized_crop([l, t, r, b]: [f64; 4], warp: bool) -> [f64; 4] {
    if warp {
        [(l + 999.0) / 1998.0, (999.0 - t) / 1998.0, (999.0 - r) / 1998.0, (b + 999.0) / 1998.0]
    } else {
        [l / 999.0, t / 999.0, (0.0 - r) / 999.0, (0.0 - b) / 999.0]
    }
}

fn valid_crop([l, t, r, b]: [f64; 4]) -> bool {
    [l, t, r, b].iter().all(|v| (0.0..1.0).contains(v)) && l + r < 1.0 && t + b < 1.0
}

fn trim_crop(
    edges: [Option<[(i64, f64); 3]>; 4],
    motion_times: [i64; 3],
    last: i64,
    warp: bool,
) -> Option<Option<[(i64, [f64; 4]); 3]>> {
    let [Some(l), Some(t), Some(r), Some(b)] = edges else { return None; };
    let times = l.map(|(at, _)| at);
    let [first, middle, end] = times;
    if [t, r, b].iter().any(|edge| edge.map(|(at, _)| at) != times)
        || !(first < middle && middle < end && first <= 0 && end >= last)
        || (first != 0 && end != last)
    {
        return None;
    }
    let crop = [0, 1, 2].map(|i| (times[i], normalized_crop([l[i].1, t[i].1, r[i].1, b[i].1], warp)));
    if crop.iter().any(|&(_, values)| !valid_crop(values)) {
        return None;
    }
    if crop.iter().all(|&(_, values)| values == [0.0; 4]) {
        // Preserve the original neutral-profile check.
        return (times == motion_times).then_some(None);
    }
    Some(Some(crop))
}

fn normalize(length: i64, pairs: [[f64; 2]; 8], warp: bool) -> Option<[ResizeKey; 2]> {
    let keys = [0, 1].map(|k| {
        let [x, y, sx, sy, l, t, r, b] = pairs.map(|pair| pair[k]);
        let crop = normalized_crop([l, t, r, b], warp);
        ResizeKey {
            frame: if k == 0 { 0 } else { length - 1 },
            position: [x / 999.0, (if warp { -y } else { y }) / 999.0],
            scale: [sx, sy],
            crop,
        }
    });
    keys.iter().all(|k| {
        k.scale.iter().all(|&v| v > 0.0 && v <= 10_000.0)
            && valid_crop(k.crop)
    }).then_some(keys)
}

fn warp_roll(ctx: &Ctx<'_>, effect: &Node, list: &Node, items: &[Value]) -> Option<PictureExtension> {
    let rate = Rate::new(24, 1)?;
    let length = int(effect, "length").filter(|&n| (3..=256).contains(&n))?;
    if list.class != *b"PRLS" || items.len() != 16 || !effect_envelope(ctx, effect, "3DWarp") {
        return None;
    }
    let input = ordered_picture_input(ctx, effect, length, rate)?;
    let input = if input.class == *b"SEQU" { single_picture_segment(ctx, input, length)? } else { input };
    if input.class != *b"SCLP" || int(input, "length") != Some(length)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
        || float(input, "edit_rate").and_then(Rate::from_avid) != Some(rate)
    {
        return None;
    }
    let mut controls = HashMap::new();
    for item in items {
        let item = ctx.deref(Some(item)).filter(|n| n.class == *b"PRIT")?;
        let Value::Uuid(id) = item.get("uuid")? else { return None };
        if item.get("enable") != Some(&Value::Bool(true)) || controls.insert(file_order_hex(id), item).is_some() {
            return None;
        }
    }
    let mut take = |prefix: &str| controls.remove(&format!("{prefix}568d7e84d511935a50f857c10000"));
    for (id, expected) in crate::transform::COMPACT_WARP_DEFAULTS.into_iter()
        .chain([("a981", 1.0), ("b281", 1.0), ("b481", 100.0), ("b781", 1.0)])
    {
        let n = take(id)?;
        if has_curve(n) || float(n, "value").or_else(|| int(n, "value").map(|v| v as f64)) != Some(expected) {
            return None;
        }
    }
    let read = |n: &Node| -> Option<(WarpRollInterpolation, [(i64, f64); 3])> {
        let curve = ctx.deref(n.get("control_track")).filter(|c| c.class == *b"PRCL")?;
        let mode = int(curve, "interp_kind")?;
        let interpolation = match mode {
            2 => WarpRollInterpolation::Shelf,
            3 => WarpRollInterpolation::Linear,
            5 => WarpRollInterpolation::Bezier,
            6 => WarpRollInterpolation::Spline,
            _ => return None,
        };
        if !curve_header(n, curve, length, rate, mode) {
            return None;
        }
        let Value::List(points) = curve.get("control_points")? else { return None };
        let [a, b, c] = points.as_slice() else { return None };
        let result = [trim_point(a, 4)?, trim_point(b, 4)?, trim_point(c, 4)?];
        let [(first, _), (middle, _), (last, _)] = result;
        if first != 0 || !(1..length-1).contains(&middle) || last != length - 1 {
            return None;
        }
        if matches!(interpolation, WarpRollInterpolation::Bezier | WarpRollInterpolation::Spline)
            && !warp_roll_flat_handles([a, b, c], middle, last - middle)
        {
            return None;
        }
        Some((interpolation, result))
    };
    let (mode, x) = read(take("aa81")?)?;
    let (width_mode, width) = read(take("b381")?)?;
    let (rotation_mode, rotation) = read(take("ba81")?)?;
    if !controls.is_empty() || width_mode != mode || rotation_mode != mode
        || x.map(|k| k.0) != width.map(|k| k.0)
        || x.map(|k| k.0) != rotation.map(|k| k.0)
        || x.iter().any(|k| !(-60.0..=90.0).contains(&k.1))
        || width.iter().any(|k| !(70.0..=100.0).contains(&k.1))
        || rotation.iter().any(|k| !(-15.0..=25.0).contains(&k.1))
    {
        return None;
    }
    Some(PictureExtension::WarpRoll {
        keys: std::array::from_fn(|i| WarpRollKey {
            frame: x[i].0, x: x[i].1 / 999.0, width: width[i].1, rotation: rotation[i].1,
        }),
        interpolation: mode,
    })
}

fn warp_roll_flat_handles(points: [&Value; 3], first_span: i64, second_span: i64) -> bool {
    let first = first_span as f64 * 0.4;
    let second = second_span as f64 * 0.4;
    points.into_iter().zip([[-first, first], [-first, second], [-second, second]])
        .all(|(point, [incoming, outgoing])| {
            let Value::Node(point) = point else { return false };
            let Some(Value::List(props)) = point.get("pp") else { return false };
            let [a, b, c, d, _] = props.as_slice() else { return false };
            [a, b, c, d].into_iter().zip([incoming, 0.0, outgoing, 0.0])
                .all(|(prop, expected)| {
                    matches!(prop, Value::Node(prop)
                        if float(prop, "value").is_some_and(|v| (v - expected).abs() <= 1e-12))
                })
        })
}

pub(super) fn warp(ctx: &Ctx<'_>, effect: &Node) -> Option<PictureExtension> {
    let list = ctx.deref(effect.get("param_list"))?;
    let Value::RefList(items) = list.get("items")? else { return None; };
    if !items.iter().filter_map(|item| ctx.deref(Some(item))).any(|item| curve_differs_from_inline(ctx, item)) {
        return None;
    }
    if let Some(roll) = warp_roll(ctx, effect, list, items) {
        return Some(roll);
    }
    let candidate = (|| {
        let length = int(effect, "length")?;
        let rate = float(effect, "edit_rate").and_then(Rate::from_avid)?;
        let tracks = ctx.tracks(effect);
        let [background, picture] = tracks.as_slice() else { return None; };
        let background = ctx.deref(background.get("component"))?;
        let picture = ctx.deref(picture.get("component"))?;
        let full_span_background = if background.class == *b"FILL" {
            int(background, "length") == Some(length)
        } else {
            let parts = ctx.segments(background);
            parts.iter().all(|(_, n)| int(n, "length").is_some_and(|l| l == 0 || l == length))
                && parts.iter().filter(|(_, n)| int(n, "length") == Some(length)).count() == 1
        };
        if !qualified_controls(ctx, effect, list, rate, "3DWarp")
            || int(background, "media_kind_id") != Some(KIND_PICTURE)
            || int(picture, "media_kind_id") != Some(KIND_PICTURE)
            || !full_span_background || !ctx.is_empty(background)
            || tracks.iter().map(|track| int(track, "index")).ne([Some(1), Some(2)])
        {
            return None;
        }
        // A native Resize promotion can keep the compact control set after a
        // trim, even when its curves have three points. The expanded set adds
        // singleton defaults that must still pass the full-profile checks.
        let compact_promotion = items.len()
            == WARP_MOTION.len() + WARP_CROP.len() + WARP_SWITCHES.len() + WARP_BACKGROUND.len();
        if compact_promotion {
            if let Some(pairs) = control_pairs(
                ctx, items, length, rate, &WARP_MOTION, &WARP_CROP, &WARP_SWITCHES, &WARP_BACKGROUND,
            ) {
                return normalize(length, pairs, true).map(PictureExtension::Warp);
            }
        } else if items.len() != 108 {
            return None;
        }
        warp_trim(ctx, items, length, rate, compact_promotion).map(PictureExtension::WarpTrim)
    })();
    Some(candidate.unwrap_or(PictureExtension::UnsupportedTransform(
        "3D Warp animation uses an unsupported control set, interpolation or tangent shape",
    )))
}
