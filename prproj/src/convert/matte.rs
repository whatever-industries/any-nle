//! Port of `converter/src/avid2prproj/matte.py`: Avid AniMatte as Premiere masks on the clip's
//! Opacity. The Opacity intrinsic is materialised as py-premiere's `_materialize_opacity` does
//! (Normal blend only, 81_blend_modes); each shape is one default `AEMask2` sub-component from
//! [`crate::build::mask`] with its Path set (Premiere 26.5 path form) and per-frame keys.

use super::model::Matte;
use crate::base64::b64encode;
use crate::build::mask::{attach_sub_mask, build_mask_component, build_mask_params};
use crate::build::fmt::format_lumetri_value;
use crate::build::{leaf, top};
use crate::error::{value, Result};
use crate::model::{Project, TrackItemId};
use crate::payload::set_payload;
use crate::xml::mutations::remove_child;
use crate::xml::NodeId;

const PATH_PARAM: usize = 9;
/// ParameterIDs of Mask Opacity and Inverted (tesseract-converter, 26.5 form).
const MASK_OPACITY_ID: &str = "15";
const INVERTED_ID: &str = "17";

fn set_value(project: &mut Project, param: NodeId, text: &str) -> Result<()> {
    let Some(leaf) = project.doc.tree.find(param, "StartKeyframe")? else { return value("param has no StartKeyframe") };
    let mut fields: Vec<String> = project.doc.tree[leaf].text_or_empty().split(',').map(str::to_string).collect();
    if fields.len() < 2 {
        return value("list assignment index out of range");
    }
    fields[1] = text.to_string();
    project.doc.tree.set_text(leaf, Some(fields.join(",")));
    Ok(())
}
const OPACITY_COMPONENT_CLASS_ID: &str = "d10da199-beea-4dd1-b941-ed3a78766d50";
const OPACITY_PARAM_CLASS_ID: &str = "fe47129e-6c94-4fc0-95d5-c056a517aaf3";
const BLEND_PARAM_CLASS_ID: &str = "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8";
/// BlendMode.NORMAL: the API twin's value and the GUI value.
const NORMAL_API: i64 = 18;
const NORMAL_GUI: i64 = 0;

/// `path_payload`: one closed outline, absolute handles, flag 0 for a corner (no handles).
pub fn path_payload(points: &[[f64; 6]]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&2u32.to_le_bytes());
    body.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for &[x, y, ix, iy, ox, oy] in points {
        let corner = ix == 0.0 && iy == 0.0 && ox == 0.0 && oy == 0.0;
        body.extend_from_slice(&(if corner { 0u32 } else { 1 }).to_le_bytes());
        for v in [x, y, x + ix, y + iy, x + ox, y + oy] {
            body.extend_from_slice(&(v as f32).to_le_bytes());
        }
    }
    body.push(1);
    let mut out = Vec::with_capacity(body.len() + 13);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(0x56);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn blend_keyframe(v: i64) -> String {
    format!("-91445760000000000,{v},0,0,0,0,0,0")
}

/// `TrackItem._materialize_opacity(BlendMode.NORMAL)`.
fn materialize_opacity(project: &mut Project, chain: NodeId) -> Result<()> {
    let Some(inner) = project.doc.tree.child_by_tag(chain, "ComponentChain") else {
        return value("component chain has no ComponentChain core");
    };
    for tag in ["DefaultOpacity", "DefaultOpacityComponentID"] {
        if let Some(flag) = project.doc.tree.child_by_tag(chain, tag) {
            remove_child(&mut project.doc.tree, chain, flag)?;
        }
    }
    let tree = &mut project.doc.tree;
    let opacity = top(tree, "VideoComponentParam", OPACITY_PARAM_CLASS_ID, "10", None);
    leaf(tree, opacity, "Name", "Opacity", "\n\t\t");
    leaf(tree, opacity, "ParameterID", "1", "\n\t\t");
    leaf(tree, opacity, "StartKeyframe", "-91445760000000000,100.,0,0,0,0,0,0", "\n\t\t");
    leaf(tree, opacity, "CurrentValue", "100", "\n\t\t");
    leaf(tree, opacity, "LowerBound", "0", "\n\t\t");
    leaf(tree, opacity, "UpperBound", "100", "\n\t");
    let api = top(tree, "VideoComponentParam", BLEND_PARAM_CLASS_ID, "10", None);
    leaf(tree, api, "Name", "Blend Mode", "\n\t\t");
    leaf(tree, api, "ParameterControlType", "10", "\n\t\t");
    leaf(tree, api, "DiscontinuousInterpolate", "true", "\n\t\t");
    leaf(tree, api, "ParameterID", "2", "\n\t\t");
    leaf(tree, api, "StartKeyframe", &blend_keyframe(NORMAL_API), "\n\t\t");
    leaf(tree, api, "LowerBound", "0", "\n\t\t");
    leaf(tree, api, "UpperBound", "27", "\n\t");
    let gui = top(tree, "VideoComponentParam", BLEND_PARAM_CLASS_ID, "10", None);
    leaf(tree, gui, "Name", "Blend Mode", "\n\t\t");
    leaf(tree, gui, "DiscontinuousInterpolate", "true", "\n\t\t");
    leaf(tree, gui, "ParameterID", "3", "\n\t\t");
    leaf(tree, gui, "StartKeyframe", &blend_keyframe(NORMAL_GUI), "\n\t\t");
    leaf(tree, gui, "LowerBound", "0", "\n\t\t");
    leaf(tree, gui, "UpperBound", "31", "\n\t");
    let mut param_ids = Vec::new();
    for p in [opacity, api, gui] {
        param_ids.push(project.doc.add_object(p)?);
    }
    let tree = &mut project.doc.tree;
    let element = top(tree, "VideoFilterComponent", OPACITY_COMPONENT_CLASS_ID, "9", None);
    let core = tree.sub_element(element, "Component", &[("Version", "7")]);
    tree.set_text(core, Some("\n\t\t\t"));
    tree.set_tail(core, Some("\n\t\t"));
    let holder = tree.sub_element(core, "Params", &[("Version", "1")]);
    tree.set_text(holder, Some("\n\t\t\t\t"));
    tree.set_tail(holder, Some("\n\t\t\t"));
    for (index, id) in param_ids.iter().enumerate() {
        let entry = tree.sub_element(holder, "Param", &[("Index", &index.to_string()), ("ObjectRef", id)]);
        tree.set_tail(entry, Some(if index < param_ids.len() - 1 { "\n\t\t\t\t" } else { "\n\t\t\t" }));
    }
    leaf(tree, core, "ID", "2", "\n\t\t\t");
    leaf(tree, core, "DisplayName", "Opacity", "\n\t\t\t");
    leaf(tree, core, "Intrinsic", "true", "\n\t\t");
    leaf(tree, element, "MatchName", "AE.ADBE Opacity", "\n\t\t");
    leaf(tree, element, "VideoFilterType", "2", "\n\t");
    let component_id = project.doc.add_object(element)?;
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
    let entry = tree.sub_element(components, "Component", &[("Index", &entries.len().to_string()), ("ObjectRef", &component_id)]);
    tree.set_tail(entry, Some("\n\t\t\t"));
    Ok(())
}

fn opacity(project: &mut Project, placed: TrackItemId) -> Result<NodeId> {
    let element = project.track_item(placed).element;
    let Some(chain_ref) = project.doc.tree.find(element, "ClipTrackItem/ComponentOwner/Components")? else {
        return value("track item has no component chain");
    };
    let chain = project.doc.resolve(chain_ref)?;
    if project.doc.tree.child_by_tag(chain, "DefaultOpacity").is_some() {
        materialize_opacity(project, chain)?;
    }
    for reference in project.doc.tree.findall(chain, "ComponentChain/Components/Component")? {
        let component = project.doc.resolve(reference)?;
        if project.doc.tree.findtext(component, "MatchName")?.as_deref() == Some("AE.ADBE Opacity") {
            return Ok(component);
        }
    }
    value("clip has no Opacity component")
}

/// `apply_matte`: one Opacity mask per matte shape. Returns report notes.
pub fn apply_matte(project: &mut Project, placed: TrackItemId, matte: &Matte, source_in: i64, unit: i64) -> Result<Vec<String>> {
    let holder = opacity(project, placed)?;
    for (index, keys) in matte.shapes.iter().enumerate() {
        let param_ids = build_mask_params(&mut project.doc)?;
        let (mask_id, _) = build_mask_component(&mut project.doc, &param_ids, false, &format!("{:02}", index + 1))?;
        attach_sub_mask(&mut project.doc.tree, holder, &mask_id)?;
        let path = project.doc.by_object_id[&param_ids[PATH_PARAM]];
        let Some(start_value) = project.doc.tree.child_by_tag(path, "StartKeyframeValue") else {
            return value("mask Path has no StartKeyframeValue");
        };
        set_payload(&mut project.doc, start_value, &path_payload(&keys[0].1));
        let by_id = |project: &Project, id: &str| -> Result<NodeId> {
            for pid in &param_ids {
                let p = project.doc.by_object_id[pid];
                if project.doc.tree.findtext(p, "ParameterID")?.as_deref() == Some(id) {
                    return Ok(p);
                }
            }
            value(format!("mask has no ParameterID {id}"))
        };
        let opacity = matte.opacities.get(index).copied().unwrap_or(100.0);
        if opacity != 100.0 {
            let p = by_id(project, MASK_OPACITY_ID)?;
            set_value(project, p, &format_lumetri_value(opacity)?)?;
        }
        if matte.inverted {
            let p = by_id(project, INVERTED_ID)?;
            set_value(project, p, "true")?;
        }
        if keys.len() > 1 {
            let tree = &mut project.doc.tree;
            let control = tree.child_by_tag(path, "ParameterControlType").unwrap();
            let varying = tree.new_element("IsTimeVarying");
            tree.set_text(varying, Some("true"));
            let tail = tree.tail(control).map(String::from);
            tree.set_tail(varying, tail);
            let at = tree.children(path).iter().position(|&c| c == control).unwrap();
            tree.insert(path, at + 1, varying);
            let position = tree.child_by_tag(path, "StartKeyframePosition").unwrap();
            let text: String =
                keys.iter().map(|(frame, points)| format!("{},{};", source_in + frame * unit, b64encode(&path_payload(points)))).collect();
            let frames = tree.new_element("Keyframes");
            tree.set_text(frames, Some(text));
            let tail = tree.tail(position).map(String::from);
            tree.set_tail(frames, tail);
            let at = tree.children(path).iter().position(|&c| c == position).unwrap();
            tree.insert(path, at + 1, frames);
        }
    }
    let mut notes = Vec::new();
    if matte.shapes.len() > 1 {
        notes.push(format!(
            "{} AniMatte shapes as {} Opacity masks (Premiere adds masks together)",
            matte.shapes.len(),
            matte.shapes.len()
        ));
    }
    Ok(notes)
}
