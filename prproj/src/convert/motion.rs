//! Port of `converter/src/avid2prproj/motion.py`: Motion (Position, Scale, Rotation, Anchor
//! Point, Crop) on a placed clip, for converted Avid Resize and 3D Warp. The intrinsic component
//! (ID 1, `AE.ADBE Motion`) and its 11 params are copied from a Premiere 26.5 save
//! (`data/motion_template.xml`); the chain loses DefaultMotion/DefaultMotionComponentID.

use super::model::Transform;
use super::prepared::{CornerPin, PictureEffect, WarpRollInterpolation};
use crate::build::{leaf, time::Time, top};
use crate::model::{component::{set_flat_bezier_keyframes, set_param_keyframes}, enums::KeyframeInterpolation, validators::Num};
use crate::build::fmt::{format_lumetri_value, int_str, py_repr};
use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::model::{Project, TrackItemId};
use crate::xml::mutations::remove_child;
use crate::xml::NodeId;

const TEMPLATE: &str = include_str!("../../data/motion_template.xml");
const ROUNDED_CROP_TEMPLATE: &str = include_str!("../../data/rounded_crop_template.xml");
const POSITION: usize = 0;
const SCALE: usize = 1;
const SCALE_WIDTH: usize = 2;
const UNIFORM: usize = 3;
const ROTATION: usize = 4;
const ANCHOR: usize = 5;
const CROPS: [usize; 4] = [7, 8, 9, 10]; // left, top, right, bottom

/// Python `_coord`: points as plain decimals (`0.5`, whole values without a dot).
fn coord(v: f64) -> Result<String> {
    if v.is_finite() && v == v.trunc() {
        int_str(v)
    } else {
        Ok(py_repr(v))
    }
}

fn set(project: &mut Project, param: NodeId, text: &str) -> Result<()> {
    let Some(leaf) = project.doc.tree.find(param, "StartKeyframe")? else {
        return value("Motion template: param without StartKeyframe");
    };
    let mut fields: Vec<String> = project.doc.tree[leaf].text_or_empty().split(',').map(str::to_string).collect();
    if fields.len() < 2 {
        return value("list assignment index out of range");
    }
    fields[1] = text.to_string();
    project.doc.tree.set_text(leaf, Some(fields.join(",")));
    Ok(())
}

/// `apply_motion`: materialise Motion on a placed video clip and set it from a model
/// Transform. Returns report notes.
pub fn apply_motion(project: &mut Project, placed: TrackItemId, transform: &Transform, uuids: &mut dyn UuidSource) -> Result<Vec<String>> {
    apply_motion_keys(project, placed, transform, None, uuids)
}

pub(super) fn apply_motion_keys(project: &mut Project, placed: TrackItemId, transform: &Transform,
    effect: Option<PictureEffect>, uuids: &mut dyn UuidSource) -> Result<Vec<String>> {
    let keys = match effect { Some(PictureEffect::Resize(keys)) => Some(keys), _ => None };
    let roll = match effect { Some(PictureEffect::WarpRoll { keys, interpolation }) => Some((keys, interpolation)), _ => None };
    let trimmed = match effect {
        Some(PictureEffect::TrimmedWarp(trim) | PictureEffect::TrimmedResize(trim)) => Some(trim),
        _ => None,
    };
    let already = project.doc.index_payloads()?;
    let fragment = project.doc.tree.parse_fragment(TEMPLATE)?;
    let spliced = project.splice(fragment, uuids)?;
    let Some(component) = spliced.get("VideoFilterComponent") else {
        return value("Motion template has no VideoFilterComponent");
    };
    if let Some(private) = project.doc.tree.find(component, "PremiereFilterPrivateData")? {
        if project.doc.tree.get(private, "BinaryHash").is_some_and(|h| already.contains_key(h)) {
            // Stored once per BinaryHash; this copy is a reference.
            project.doc.tree.set_text(private, None::<String>);
        }
    }
    project.doc.invalidate_payloads();
    let mut params = Vec::new();
    for reference in project.doc.tree.findall(component, "Component/Params/Param")? {
        params.push(project.doc.resolve(reference)?);
    }
    if params.len() != 11 {
        return value("Motion template: expected 11 params");
    }

    let mut notes = Vec::new();
    let (x, y) = transform.position;
    set(project, params[POSITION], &format!("{}:{}", coord(0.5 + x)?, coord(0.5 + y)?))?;
    let (ax, ay) = transform.anchor;
    set(project, params[ANCHOR], &format!("{}:{}", coord(0.5 + ax)?, coord(0.5 + ay)?))?;
    let (sx, sy) = transform.scale;
    let uniform = roll.is_none() && trimmed.is_none() && sx == sy && keys.is_none_or(|keys| keys.iter().all(|k| k.scale[0] == k.scale[1]));
    set(project, params[SCALE], &format_lumetri_value(sy)?)?;
    set(project, params[SCALE_WIDTH], &format_lumetri_value(if uniform { 100.0 } else { sx })?)?;
    set(project, params[UNIFORM], if uniform { "true" } else { "false" })?;
    set(project, params[ROTATION], &format_lumetri_value(transform.rotation)?)?;
    let initial_crop = keys.and_then(|keys| keys[0].crop)
        .or_else(|| trimmed.and_then(|trim| trim.crop.map(|crop| crop[0].1)))
        .unwrap_or(transform.crop);
    for (index, v) in CROPS.into_iter().zip(initial_crop) {
        let percent = (v * 100.0).clamp(0.0, 100.0);
        if percent != v * 100.0 {
            notes.push("a crop outside Premiere's 0..100% was clamped".to_string());
        }
        set(project, params[index], &format_lumetri_value(percent)?)?;
    }
    if let Some(keys) = keys {
        set_position_keys(&mut project.doc.tree, params[POSITION], &keys.map(|k| (k.ticks, k.position)))?;
        let axis = |i: usize| keys.map(|k| (Time::new(k.ticks), Num::Float(k.scale[i]), KeyframeInterpolation::Linear));
        set_param_keyframes(&mut project.doc.tree, params[SCALE], &axis(1))?;
        if !uniform {
            set_param_keyframes(&mut project.doc.tree, params[SCALE_WIDTH], &axis(0))?;
        }
        if let [Some(first), Some(last)] = keys.map(|k| k.crop) {
            for (axis, index) in CROPS.into_iter().enumerate() {
                let edge = [first[axis], last[axis]];
                let values = [0, 1].map(|k| (Time::new(keys[k].ticks), Num::Float(edge[k] * 100.0), KeyframeInterpolation::Linear));
                set_param_keyframes(&mut project.doc.tree, params[index], &values)?;
            }
        }
    }

    if let Some(trim) = trimmed {
        let tree = &mut project.doc.tree;
        set_position_keys(tree, params[POSITION], &trim.motion.map(|k| (k.ticks, k.position)))?;
        let scalar = |(ticks, value)| (Time::new(ticks), Num::Float(value), KeyframeInterpolation::Linear);
        set_param_keyframes(tree, params[SCALE], &trim.motion.map(|k| scalar((k.ticks, k.height))))?;
        set_param_keyframes(tree, params[SCALE_WIDTH], &trim.width.map(scalar))?;
        if let Some(crop) = trim.crop {
            for (axis, index) in CROPS.into_iter().enumerate() {
                set_param_keyframes(tree, params[index], &crop.map(|(t, edges)| scalar((t, edges[axis] * 100.0))))?;
            }
        }
    }
    if let Some((keys, interpolation)) = roll {
        let tree = &mut project.doc.tree;
        write_position_keys(tree, params[POSITION], &keys.map(|k| (k.ticks, [k.x, 0.0])), interpolation)?;
        if interpolation == WarpRollInterpolation::FlatBezier {
            set_flat_bezier_keyframes(tree, params[SCALE_WIDTH], &keys.map(|k| (Time::new(k.ticks), Num::Float(k.width))))?;
            set_flat_bezier_keyframes(tree, params[ROTATION], &keys.map(|k| (Time::new(k.ticks), Num::Float(k.rotation))))?;
        } else {
            let interpolation = if interpolation == WarpRollInterpolation::Hold {
                KeyframeInterpolation::Hold
            } else {
                KeyframeInterpolation::Linear
            };
            let scalar = |ticks, value| (Time::new(ticks), Num::Float(value), interpolation);
            set_param_keyframes(tree, params[SCALE_WIDTH], &keys.map(|k| scalar(k.ticks, k.width)))?;
            set_param_keyframes(tree, params[ROTATION], &keys.map(|k| scalar(k.ticks, k.rotation)))?;
        }
    }

    let placed_element = project.track_item(placed).element;
    let Some(chain_ref) = project.doc.tree.find(placed_element, "ClipTrackItem/ComponentOwner/Components")? else {
        return value("track item has no component chain");
    };
    let chain = project.doc.resolve(chain_ref)?;
    for tag in ["DefaultMotion", "DefaultMotionComponentID"] {
        if let Some(flag) = project.doc.tree.child_by_tag(chain, tag) {
            remove_child(&mut project.doc.tree, chain, flag)?;
        }
    }
    append_component(project, chain, component)?;
    Ok(notes)
}

fn set_position_keys(tree: &mut crate::xml::Tree, param: NodeId, keys: &[(i64, [f64; 2])]) -> Result<()> {
    write_position_keys(tree, param, keys, WarpRollInterpolation::Linear)
}

fn write_position_keys(
    tree: &mut crate::xml::Tree, param: NodeId, keys: &[(i64, [f64; 2])],
    interpolation: WarpRollInterpolation,
) -> Result<()> {
    let segment = |(start, a): (i64, [f64; 2]), (end, b): (i64, [f64; 2])| {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let speed = dx.hypot(dy) / Time::new(end - start).seconds();
        (speed, dx, dy)
    };
    let mut records = String::new();
    for (i, &(ticks, position)) in keys.iter().enumerate() {
        let [x, y] = position.map(|n| 0.5 + n);
        if interpolation == WarpRollInterpolation::Hold {
            // Hold disables temporal interpolation and spatial handles.
            records.push_str(&format!("{},{}:{},4,0,0,0.16666666666666666,0,0.33333333333333331,5,4,0,0,0,0;",
                ticks, coord(x)?, coord(y)?));
            continue;
        }
        let (slope_in, in_x, in_y) = i.checked_sub(1).map_or((0.0, 0.0, 0.0), |prev| {
            let (speed, dx, dy) = segment(keys[prev], keys[i]);
            (speed, -dx / 6.0, -dy / 6.0)
        });
        let (slope_out, out_x, out_y) = keys.get(i + 1).map_or((0.0, 0.0, 0.0), |&next| {
            let (speed, dx, dy) = segment(keys[i], next);
            (speed, dx / 6.0, dy / 6.0)
        });
        if interpolation == WarpRollInterpolation::FlatBezier {
            // Keep a straight spatial path while easing progress along it.
            records.push_str(&format!("{},{}:{},5,0,0,0.4,0,0.4,5,4,{},{},{},{};",
                ticks, coord(x)?, coord(y)?, in_x, in_y, out_x, out_y));
        } else {
            records.push_str(&format!("{},{}:{},0,0,{},0.16666666666666666,{},0.16666666666666666,5,4,{},{},{},{};",
                ticks, coord(x)?, coord(y)?, slope_in, slope_out, in_x, in_y, out_x, out_y));
        }
    }
    let start = tree.child_by_tag(param, "StartKeyframe").ok_or_else(|| crate::Error::Value("Motion Position start value is missing".into()))?;
    crate::xml::mutations::insert_leaf_before(tree, param, "StartKeyframe", "IsTimeVarying", "true")?;
    let index = tree.children(param).iter().position(|&x| x == start).unwrap();
    let tail = tree.tail(start).map(str::to_owned);
    tree.set_tail(start, Some("\n\t\t"));
    let node = tree.new_element("Keyframes");
    tree.set_text(node, Some(records));
    tree.set_tail(node, tail);
    tree.insert(param, index + 1, node);
    Ok(())
}

pub(super) fn apply_flip(project: &mut Project, placed: TrackItemId, horizontal: bool, vertical: bool) -> Result<()> {
    apply_flip_at(project, placed, horizontal, vertical, 3)
}

fn apply_flip_at(project: &mut Project, placed: TrackItemId, horizontal: bool, vertical: bool, first_id: usize) -> Result<()> {
    let element = project.track_item(placed).element;
    let reference = project.doc.tree.find1(element, "ClipTrackItem/ComponentOwner/Components")
        .ok_or_else(|| crate::Error::Value("flip placement has no component chain".into()))?;
    let chain = project.doc.resolve(reference)?;
    let flips = [(horizontal, "Horizontal Flip", "AE.ADBE Horizontal Flip"),
        (vertical, "Vertical Flip", "AE.ADBE Vertical Flip")];
    for (i, (_, name, match_name)) in flips.into_iter().filter(|(apply, _, _)| *apply).enumerate() {
        let tree = &mut project.doc.tree;
        let component = crate::build::top(tree, "VideoFilterComponent", "d10da199-beea-4dd1-b941-ed3a78766d50", "9", None);
        let inner = tree.sub_element(component, "Component", &[("Version", "7")]);
        tree.set_text(inner, Some("\n\t\t\t"));
        tree.set_tail(inner, Some("\n\t\t"));
        crate::build::leaf(tree, inner, "ID", &(i + first_id).to_string(), "\n\t\t\t");
        crate::build::leaf(tree, inner, "DisplayName", name, "\n\t\t");
        crate::build::leaf(tree, component, "VideoFilterType", "2", "\n\t\t");
        crate::build::leaf(tree, component, "MatchName", match_name, "\n\t");
        project.register(component)?;
        append_component(project, chain, component)?;
    }
    Ok(())
}

pub(super) fn apply_corner_pin(
    project: &mut Project, placed: TrackItemId, pin: CornerPin, uuids: &mut dyn UuidSource,
) -> Result<()> {
    apply_corner_pin_at(project, placed, pin, uuids, 3)
}

pub(super) fn apply_corner_pin_with_flop(
    project: &mut Project, placed: TrackItemId, pin: CornerPin, flop_before_pin: bool,
    uuids: &mut dyn UuidSource,
) -> Result<()> {
    // Premiere renders these standard components in reverse saved-list order.
    // Keep crop adjacent to the pin so it still acts in the pin's input space.
    if flop_before_pin {
        apply_corner_pin_at(project, placed, pin, uuids, 3)?;
        apply_flip_at(project, placed, true, false, if pin.source_crop == [0.0; 4] { 4 } else { 5 })
    } else {
        apply_flip_at(project, placed, true, false, 3)?;
        apply_corner_pin_at(project, placed, pin, uuids, 4)
    }
}

fn apply_corner_pin_at(
    project: &mut Project, placed: TrackItemId, pin: CornerPin, uuids: &mut dyn UuidSource,
    first_id: usize,
) -> Result<()> {
    let item = project.track_item(placed).element;
    let reference = project.doc.tree.find1(item, "ClipTrackItem/ComponentOwner/Components")
        .ok_or_else(|| crate::Error::Value("Corner Pin placement has no component chain".into()))?;
    let chain = project.doc.resolve(reference)?;
    let [left, top_crop, right, bottom] = pin.source_crop;
    let cropped = pin.source_crop != [0.0; 4];
    let crop_component = if cropped {
        let fragment = project.doc.tree.parse_fragment(ROUNDED_CROP_TEMPLATE)?;
        let spliced = project.splice(fragment, uuids)?;
        let crop = spliced.get("VideoFilterComponent")
            .ok_or_else(|| crate::Error::Value("Rounded Crop template has no component".into()))?;
        let mut changed = 0;
        for reference in project.doc.tree.findall(crop, "Component/Params/Param")? {
            let param = project.doc.resolve(reference)?;
            let value = match project.doc.tree.findtext(param, "ParameterID")?.as_deref() {
                Some("8182") => coord(left * 100.0)?,
                Some("8183") => coord(right * 100.0)?,
                Some("8184") => coord(top_crop * 100.0)?,
                Some("8185") => coord(bottom * 100.0)?,
                Some("8187") => "0".into(), // Square corners.
                // Crop shrinks around the center. Move the matte to the center
                // of the saved source rectangle without moving the picture.
                Some("8188") => format!("{}:{}", coord((left - right) / 2.0)?, coord((top_crop - bottom) / 2.0)?),
                _ => continue,
            };
            set(project, param, &value)?;
            changed += 1;
        }
        if changed != 6 {
            return value("Rounded Crop template is missing required controls");
        }
        let id = project.doc.tree.find1(crop, "Component/ID")
            .ok_or_else(|| crate::Error::Value("Rounded Crop template has no component ID".into()))?;
        project.doc.tree.set_text(id, Some((first_id + 1).to_string()));
        Some(crop)
    } else {
        None
    };
    let [tl, tr, br, bl] = pin.corners;
    let tree = &mut project.doc.tree;
    let effect = top(tree, "VideoFilterComponent", "d10da199-beea-4dd1-b941-ed3a78766d50", "9", None);
    let inner = tree.sub_element(effect, "Component", &[("Version", "7")]);
    let params = tree.sub_element(inner, "Params", &[("Version", "1")]);
    tree.set_text(inner, Some("\n\t\t\t"));
    tree.set_tail(inner, Some("\n\t\t"));
    tree.set_text(params, Some("\n\t\t\t\t"));
    tree.set_tail(params, Some("\n\t\t\t"));
    leaf(tree, inner, "ID", &first_id.to_string(), "\n\t\t\t");
    leaf(tree, inner, "DisplayName", "Corner Pin", "\n\t\t");
    leaf(tree, effect, "VideoFilterType", "2", "\n\t\t");
    leaf(tree, effect, "MatchName", "AE.ADBE Corner Pin", "\n\t");
    project.register(effect)?;
    for (index, (name, [x, y])) in [
        ("Upper Left", tl), ("Upper Right", tr), ("Lower Left", bl), ("Lower Right", br),
    ].into_iter().enumerate() {
        let tree = &mut project.doc.tree;
        let param = top(tree, "PointComponentParam", "ca81d347-309b-44d2-acc7-1c572efb973c", "4", None);
        leaf(tree, param, "Name", name, "\n\t\t");
        leaf(tree, param, "ParameterID", &(index + 1).to_string(), "\n\t\t");
        let start = format!("-91445760000000000,{}:{},0,0,0,0,0,0,5,4,0,0,0,0", coord(x)?, coord(y)?);
        leaf(tree, param, "StartKeyframe", &start, "\n\t");
        let id = project.register(param)?;
        let tree = &mut project.doc.tree;
        let reference = tree.sub_element(params, "Param", &[("Index", &index.to_string()), ("ObjectRef", &id)]);
        tree.set_tail(reference, Some(if index == 3 { "\n\t\t\t" } else { "\n\t\t\t\t" }));
    }
    append_component(project, chain, effect)?;
    // Premiere 27 renders this pair as crop then pin when the pin precedes
    // Rounded Crop in the saved component list. The other order clips the output.
    if let Some(crop) = crop_component {
        append_component(project, chain, crop)?;
    }
    Ok(())
}

fn append_component(project: &mut Project, chain: NodeId, component: NodeId) -> Result<()> {
    let Some(inner) = project.doc.tree.child_by_tag(chain, "ComponentChain") else {
        return value("component chain has no ComponentChain core");
    };
    let tree = &mut project.doc.tree;
    let components = match tree.child_by_tag(inner, "Components") {
        Some(c) => c,
        None => {
            let c = tree.new_element_with("Components", &[("Version", "1")]);
            tree.set_text(c, Some("\n\t\t\t\t"));
            tree.set_tail(c, Some("\n\t\t"));
            if let Some(&last) = tree.children(inner).last() {
                tree.set_tail(last, Some("\n\t\t\t"));
            } else {
                tree.set_text(inner, Some("\n\t\t\t"));
            }
            tree.append(inner, c);
            c
        }
    };
    let entries: Vec<NodeId> = tree.children(components).iter().copied().filter(|&c| tree.tag(c) == "Component").collect();
    if let Some(&last) = entries.last() {
        tree.set_tail(last, Some("\n\t\t\t\t"));
    }
    let object_id = tree.get(component, "ObjectID").unwrap_or_default().to_string();
    let entry = tree.sub_element(components, "Component", &[("Index", &entries.len().to_string()), ("ObjectRef", &object_id)]);
    tree.set_tail(entry, Some("\n\t\t\t"));
    Ok(())
}
