//! Port of `converter/src/avid2prproj/lumetri.py`: Lumetri Color on a placed clip, for converted
//! Avid Color Correction. The effect is copied from a Premiere-saved template
//! (`data/lumetri_template.xml`); RGB Curves are the 2080-byte arbitrary-data param after
//! "HDR Range" (four 520-byte records: u32 0, u32 point count, 32 `(f64, f64)` pairs).

use std::path::Path;

use super::avid_color::Curves;
use crate::build::fmt::format_lumetri_value;
use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::model::{Project, TrackItemId};
use crate::payload::set_payload;
use crate::xml::NodeId;

const TEMPLATE: &str = include_str!("../../data/lumetri_template.xml");
const BLOB_TEMPLATE: &str = include_str!("../../data/lumetri_look_blob.xml");

pub const CURVE_ORDER: [&str; 4] = ["master", "red", "green", "blue"];
/// Position in the template's Params list (after "HDR Range").
pub const CURVES_PARAM_INDEX: usize = 53;
pub const RECORD_SIZE: usize = 520;
pub const MAX_POINTS: usize = 32;

pub const LOOK_PATH_PARAM: usize = 32;
pub const EMBEDDED_LUT_PARAM: usize = 122;
pub const BLOB_PARAM: usize = 0;
pub const COLOR_SPACE_PARAM: usize = 37;

fn param(project: &Project, component: NodeId, index: usize) -> Result<NodeId> {
    let tree = &project.doc.tree;
    match tree.find(component, &format!("Component/Params/Param[@Index='{index}']"))? {
        Some(reference) => project.doc.resolve(reference),
        None => value("'NoneType' object has no attribute 'get'"),
    }
}

/// The first param with this display name (Lumetri repeats some names in later sections; the
/// first is the Basic Correction one).
fn named(project: &Project, component: NodeId, name: &str) -> Result<NodeId> {
    let tree = &project.doc.tree;
    for reference in tree.findall(component, "Component/Params/Param")? {
        let p = project.doc.resolve(reference)?;
        if tree.findtext(p, "Name")?.as_deref() == Some(name) {
            return Ok(p);
        }
    }
    value(format!("Lumetri template has no {} param", super::model::py_str_repr(name)))
}

fn set_scalar(project: &mut Project, param: NodeId, v: f64) -> Result<()> {
    let Some(leaf) = project.doc.tree.find(param, "StartKeyframe")? else {
        return value("'NoneType' object has no attribute 'text'");
    };
    let text = project.doc.tree[leaf].text_or_empty().to_string();
    let mut fields: Vec<String> = text.split(',').map(str::to_string).collect();
    if fields.len() < 2 {
        return value("list assignment index out of range");
    }
    fields[1] = format_lumetri_value(v)?;
    project.doc.tree.set_text(leaf, Some(fields.join(",")));
    Ok(())
}

/// Python `min(max(v, 0.0), 1.0)`.
fn clamp01(v: f64) -> f64 {
    let m = if 0.0 > v { 0.0 } else { v };
    if 1.0 < m {
        1.0
    } else {
        m
    }
}

/// `curve_payload`: the 2080-byte RGB Curves value. Points are clamped to 0..1; untouched
/// curves are the identity (0,0)-(1,1). Returns the payload and report notes.
pub fn curve_payload(curves: &Curves) -> Result<(Vec<u8>, Vec<String>)> {
    let mut notes = Vec::new();
    let mut out = Vec::with_capacity(4 * RECORD_SIZE);
    for name in CURVE_ORDER {
        let given = curves.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
        let points: Vec<(f64, f64)> = if given.is_empty() { vec![(0.0, 0.0), (1.0, 1.0)] } else { given.to_vec() };
        if points.len() > MAX_POINTS {
            return value(format!("{name} curve has {} points; Lumetri stores at most {MAX_POINTS}", points.len()));
        }
        let clamped: Vec<(f64, f64)> = points.iter().map(|&(x, y)| (clamp01(x), clamp01(y))).collect();
        // Python compares tuples element-wise by identity-or-equality: an untouched NaN is equal.
        let changed = points.iter().zip(&clamped).any(|(p, c)| (p.0 != c.0 && !p.0.is_nan()) || (p.1 != c.1 && !p.1.is_nan()));
        if changed {
            notes.push(format!("{name} curve points outside Premiere's range were clamped"));
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(clamped.len() as u32).to_le_bytes());
        let mut flat: Vec<f64> = clamped.iter().flat_map(|p| [p.0, p.1]).collect();
        flat.resize(2 * MAX_POINTS, 0.0);
        for v in flat {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    Ok((out, notes))
}

/// `apply_lumetri`: add a Lumetri Color effect to a placed video clip and set it. Returns report
/// notes.
pub fn apply_lumetri(
    project: &mut Project,
    placed: TrackItemId,
    saturation: Option<f64>,
    curves: Option<&Curves>,
    uuids: &mut dyn UuidSource,
) -> Result<Vec<String>> {
    let already = project.doc.index_payloads()?;
    let fragment = project.doc.tree.parse_fragment(TEMPLATE)?;
    let spliced = project.splice(fragment, uuids)?;
    let Some(component) = spliced.get("VideoFilterComponent") else {
        return value("Lumetri template has no VideoFilterComponent");
    };
    let count = project.doc.tree.findall(component, "Component/Params/Param")?.len();
    let mut params = Vec::with_capacity(count);
    for i in 0..count {
        params.push(param(project, component, i)?);
    }
    // Premiere stores a binary payload once per BinaryHash; later copies are empty elements.
    for &p in &params {
        if let Some(v) = project.doc.tree.find(p, "StartKeyframeValue")? {
            if project.doc.tree.get(v, "BinaryHash").is_some_and(|h| already.contains_key(h)) {
                project.doc.tree.set_text(v, None::<String>);
            }
        }
    }
    project.doc.invalidate_payloads();

    let mut notes = Vec::new();
    let exposure = named(project, component, "Exposure")?;
    set_scalar(project, exposure, 0.0)?; // the template's own +1.0
    if let Some(s) = saturation {
        let p = named(project, component, "Saturation")?;
        set_scalar(project, p, s)?;
    }
    if let Some(curves) = curves.filter(|c| !c.is_empty()) {
        let Some(value_element) = params.get(CURVES_PARAM_INDEX).and_then(|&p| project.doc.tree.find1(p, "StartKeyframeValue")) else {
            return value("Lumetri template: RGB Curves param not where expected");
        };
        let stored = project.doc.payload(value_element)?.unwrap_or_default();
        if stored.len() != 4 * RECORD_SIZE {
            return value("Lumetri template: RGB Curves param not where expected");
        }
        let (payload, n) = curve_payload(curves)?;
        notes = n;
        set_payload(&mut project.doc, value_element, &payload);
    }

    let tree = &project.doc.tree;
    let placed_element = project.track_item(placed).element;
    let Some(chain_ref) = tree.find(placed_element, "ClipTrackItem/ComponentOwner/Components")? else {
        return value("'NoneType' object has no attribute 'get'");
    };
    let chain = project.doc.resolve(chain_ref)?;
    let Some(inner) = project.doc.tree.find(chain, "ComponentChain")? else {
        return value("'NoneType' object has no attribute 'find'");
    };
    let components = match project.doc.tree.child_by_tag(inner, "Components") {
        Some(c) => c,
        None => {
            let tree = &mut project.doc.tree;
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
    let entries: Vec<NodeId> = project.doc.tree.children(components).iter().copied().filter(|&c| project.doc.tree.tag(c) == "Component").collect();
    let mut used: Vec<i128> = vec![2]; // intrinsic Motion 1, Opacity 2
    for &entry in &entries {
        let target = project.doc.resolve(entry)?;
        if let Some(existing) = project.doc.tree.findtext(target, "Component/ID")? {
            if !existing.is_empty() && existing.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = existing.parse::<i128>() {
                    used.push(n);
                }
            }
        }
    }
    let next = used.iter().max().copied().unwrap_or(2) + 1;
    let Some(id_element) = project.doc.tree.find(component, "Component/ID")? else {
        return value("'NoneType' object has no attribute 'text'");
    };
    project.doc.tree.set_text(id_element, Some(next.to_string()));
    if let Some(&last) = entries.last() {
        project.doc.tree.set_tail(last, Some("\n\t\t\t\t"));
    }
    let object_id = project.doc.tree.get(component, "ObjectID").unwrap_or_default().to_string();
    let tree = &mut project.doc.tree;
    let entry = tree.sub_element(components, "Component", &[("Index", &entries.len().to_string()), ("ObjectRef", &object_id)]);
    tree.set_tail(entry, Some("\n\t\t\t"));
    Ok(notes)
}

/// `write_cube` text: a `.cube` 3D LUT.
pub fn cube_text(points: &[(f64, f64, f64)], size: i64, title: &str) -> String {
    use crate::build::fmt::fmt_f;
    // Match the Python oracle: only the optional .cube title is restricted to ASCII.
    let title: String = title.chars().map(|c| if (' '..='~').contains(&c) && c != '"' { c } else { '?' }).collect();
    let mut lines = vec![format!("TITLE \"{title}\""), format!("LUT_3D_SIZE {size}"), "DOMAIN_MIN 0.0 0.0 0.0".to_string(), "DOMAIN_MAX 1.0 1.0 1.0".to_string()];
    lines.extend(points.iter().map(|p| format!("{} {} {}", fmt_f(p.0, 6), fmt_f(p.1, 6), fmt_f(p.2, 6))));
    lines.join("\n") + "\n"
}

/// `write_cube`: write [`cube_text`] to `path` (ASCII, with a safe title).
pub fn write_cube(path: &Path, points: &[(f64, f64, f64)], size: i64, title: &str) -> Result<()> {
    let text = cube_text(points, size, title);
    if !text.is_ascii() {
        return value("'ascii' codec can't encode character");
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// `embedded_lut`: Premiere's "Embedded LUTs" value: 16-byte ID, u32 length of the rest, `ITX3`,
/// size, channels 3, 0, f32 domain min x3 and max x3, then `size^3` f32 RGB, red fastest.
pub fn embedded_lut(lut_id: [u8; 16], points: &[(f64, f64, f64)], size: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(40 + points.len() * 12);
    body.extend_from_slice(b"ITX3");
    for v in [size, 3, 0] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0.0f32, 0.0, 0.0, 1.0, 1.0, 1.0] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    for p in points {
        for v in [p.0, p.1, p.2] {
            body.extend_from_slice(&(v as f32).to_le_bytes());
        }
    }
    let mut out = lut_id.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// `apply_lumetri_lut`: Lumetri Color with a Creative Look LUT embedded in the project, so it
/// renders even if the `.cube` file is missing. The XML Blob is required: without it the Look's
/// name shows but the LUT is not applied.
pub fn apply_lumetri_lut(
    project: &mut Project,
    placed: TrackItemId,
    points: &[(f64, f64, f64)],
    size: u32,
    cube_path: &Path,
    uuids: &mut dyn UuidSource,
) -> Result<()> {
    apply_lumetri(project, placed, None, None, uuids)?;
    let placed_element = project.track_item(placed).element;
    let Some(chain_ref) = project.doc.tree.find(placed_element, "ClipTrackItem/ComponentOwner/Components")? else {
        return value("'NoneType' object has no attribute 'get'");
    };
    let chain = project.doc.resolve(chain_ref)?;
    let Some(list) = project.doc.tree.find(chain, "ComponentChain/Components")? else {
        return value("'NoneType' object is not subscriptable");
    };
    let Some(&last) = project.doc.tree.children(list).last() else { return value("list index out of range") };
    let component = project.doc.resolve(last)?;
    let refs = project.doc.tree.findall(component, "Component/Params/Param")?;
    let params = refs.into_iter().map(|r| project.doc.resolve(r)).collect::<Result<Vec<_>>>()?;

    let lut_id = uuids.uuid4().bytes();
    let path = cube_path.to_string_lossy().into_owned();
    let lut_hex = lut_id.iter().map(|b| format!("{b:02X}")).collect::<String>();
    let with_path = BLOB_TEMPLATE.replace("{{LUT_PATH}}", &path).replace("{{LUT_ID}}", &lut_hex);
    let guid = uuids.uuid4().to_string();
    let blob = with_path.replace("{{GUID}}", &guid);

    let slot = |index: usize| -> Result<NodeId> {
        params
            .get(index)
            .and_then(|&p| project.doc.tree.find1(p, "StartKeyframeValue"))
            .ok_or_else(|| crate::Error::Value("'NoneType' object has no attribute 'get'".into()))
    };
    let (look, embedded, blob_slot) = (slot(LOOK_PATH_PARAM)?, slot(EMBEDDED_LUT_PARAM)?, slot(BLOB_PARAM)?);
    let utf16: Vec<u8> = (path + "\0").encode_utf16().flat_map(u16::to_le_bytes).collect();
    set_payload(&mut project.doc, look, &utf16);
    set_payload(&mut project.doc, embedded, &embedded_lut(lut_id, points, size));
    set_payload(&mut project.doc, blob_slot, blob.as_bytes());
    let Some(leaf) = params.get(COLOR_SPACE_PARAM).and_then(|&p| project.doc.tree.find1(p, "StartKeyframe")) else {
        return value("'NoneType' object has no attribute 'text'");
    };
    let text = project.doc.tree[leaf].text_or_empty().to_string();
    let mut fields: Vec<String> = text.split(',').map(str::to_string).collect();
    if fields.len() < 2 {
        return value("list assignment index out of range");
    }
    fields[1] = "28".to_string(); // Premiere 26.5's own value (popup, no trailing dot)
    project.doc.tree.set_text(leaf, Some(fields.join(",")));
    Ok(())
}
