//! Port of `converter/src/avid2prproj/motion.py`: Motion (Position, Scale, Rotation, Anchor
//! Point, Crop) on a placed clip, for converted Avid Resize and 3D Warp. The intrinsic component
//! (ID 1, `AE.ADBE Motion`) and its 11 params are copied from a Premiere 26.5 save
//! (`data/motion_template.xml`); the chain loses DefaultMotion/DefaultMotionComponentID.

use super::model::Transform;
use super::prepared::MotionKey;
use crate::build::time::Time;
use crate::model::{component::set_param_keyframes, enums::KeyframeInterpolation, validators::Num};
use crate::build::fmt::{format_lumetri_value, int_str, py_repr};
use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::model::{Project, TrackItemId};
use crate::xml::mutations::remove_child;
use crate::xml::NodeId;

const TEMPLATE: &str = include_str!("../../data/motion_template.xml");
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
    keys: Option<[MotionKey; 2]>, uuids: &mut dyn UuidSource) -> Result<Vec<String>> {
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
    let uniform = sx == sy && keys.is_none_or(|keys| keys.iter().all(|k| k.scale[0] == k.scale[1]));
    set(project, params[SCALE], &format_lumetri_value(sy)?)?;
    set(project, params[SCALE_WIDTH], &format_lumetri_value(if uniform { 100.0 } else { sx })?)?;
    set(project, params[UNIFORM], if uniform { "true" } else { "false" })?;
    set(project, params[ROTATION], &format_lumetri_value(transform.rotation)?)?;
    for (index, v) in CROPS.into_iter().zip(transform.crop) {
        let percent = (v * 100.0).clamp(0.0, 100.0);
        if percent != v * 100.0 {
            notes.push("a crop outside Premiere's 0..100% was clamped".to_string());
        }
        set(project, params[index], &format_lumetri_value(percent)?)?;
    }
    if let Some(keys) = keys {
        set_position_keys(&mut project.doc.tree, params[POSITION], keys)?;
        let axis = |i: usize| keys.map(|k| (Time::new(k.ticks), Num::Float(k.scale[i]), KeyframeInterpolation::Linear));
        set_param_keyframes(&mut project.doc.tree, params[SCALE], &axis(1))?;
        if !uniform {
            set_param_keyframes(&mut project.doc.tree, params[SCALE_WIDTH], &axis(0))?;
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

fn set_position_keys(tree: &mut crate::xml::Tree, param: NodeId, keys: [MotionKey; 2]) -> Result<()> {
    let [a, b] = keys;
    let dx = b.position[0] - a.position[0];
    let dy = b.position[1] - a.position[1];
    let seconds = Time::new(b.ticks - a.ticks).seconds();
    let speed = dx.hypot(dy) / seconds;
    let mut records = String::new();
    for (i, key) in keys.into_iter().enumerate() {
        let [x, y] = key.position.map(|n| 0.5 + n);
        let (slope_in, slope_out, in_x, in_y, out_x, out_y) = if i == 0 {
            (0.0, speed, 0.0, 0.0, dx / 6.0, dy / 6.0)
        } else {
            (speed, 0.0, -dx / 6.0, -dy / 6.0, 0.0, 0.0)
        };
        records.push_str(&format!("{},{}:{},0,0,{},0.16666666666666666,{},0.16666666666666666,5,4,{},{},{},{};",
            key.ticks, coord(x)?, coord(y)?, slope_in, slope_out, in_x, in_y, out_x, out_y));
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
        crate::build::leaf(tree, inner, "ID", &(i + 3).to_string(), "\n\t\t\t");
        crate::build::leaf(tree, inner, "DisplayName", name, "\n\t\t");
        crate::build::leaf(tree, component, "VideoFilterType", "2", "\n\t\t");
        crate::build::leaf(tree, component, "MatchName", match_name, "\n\t");
        project.register(component)?;
        append_component(project, chain, component)?;
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
