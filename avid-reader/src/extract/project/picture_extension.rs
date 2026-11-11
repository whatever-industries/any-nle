use super::*;
mod trim_defaults;
use crate::source::{PictureExtension, PictureStage, Rate, ResizeKey, SourceEvent, LinearTrim, LinearTrimKey};

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
        ("EFF2_SBLEND", [background, picture]) => {
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
            || event.picture_extension.is_some_and(PictureExtension::replaces_static_transform);
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
                    || (matches!(self.effect, PictureExtension::Resize(_) | PictureExtension::ResizeCrop(_) | PictureExtension::Warp(_) | PictureExtension::WarpTrim(_) | PictureExtension::ResizeTrim(_))
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
    let attrs = ctx.attrs(effect);
    let one = |key, expected| attrs.iter().filter(|(k, _)| k == key)
        .map(|(_, v)| v.as_str()).eq([Some(expected)]);
    effect.class == *b"TKFX" && list.class == *b"PRLS"
        && Rate::new(24_000, 1001) == Some(rate)
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

pub(super) fn warp(ctx: &Ctx<'_>, effect: &Node) -> Option<PictureExtension> {
    let list = ctx.deref(effect.get("param_list"))?;
    let Value::RefList(items) = list.get("items")? else { return None; };
    if !items.iter().filter_map(|item| ctx.deref(Some(item))).any(|item| curve_differs_from_inline(ctx, item)) {
        return None;
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
        "3D Warp animation requires the qualified Linear motion and crop controls with neutral background",
    )))
}
