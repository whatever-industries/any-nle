//! Saved clip-owned Color Mode Adjust in Premiere 27.
use super::components::field;
use crate::Result;
use graph::{Sidecar, Value};
use prproj::{
    model::{ItemId, Project, TrackItemId},
    xml::NodeId,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub(crate) const ADAPTER: &str = "premiere.color_mode_exposure";
pub(crate) const ADJUST_LOSS: &str =
    "Premiere Color Mode Adjust is retained in graph/2. The saved controls or color pipeline are outside the qualified Resolve mapping";

#[derive(Default)]
pub(super) struct SavedPayloads<'a> {
    index: Option<HashMap<&'a str, Option<&'a str>>>,
}

impl<'a> SavedPayloads<'a> {
    fn text(&mut self, project: &'a Project, node: NodeId) -> &'a str {
        let tree = &project.doc.tree;
        let text = tree.text(node).unwrap_or("").trim();
        let Some(key) = tree.get(node, "BinaryHash") else {
            return text;
        };
        let index = self.index.get_or_insert_with(|| {
            let mut rows = HashMap::new();
            for id in tree.iter(project.doc.root) {
                let Some(hash) = tree.get(id, "BinaryHash") else { continue };
                let payload = tree[id].text_or_empty().trim();
                if payload.is_empty() {
                    continue;
                }
                if !rows.contains_key(hash) && rows.len() == 65_536 {
                    continue;
                }
                let saved = rows.entry(hash).or_insert(Some(payload));
                if *saved != Some(payload) {
                    *saved = None;
                }
            }
            rows
        });
        index.get(key).copied().flatten().unwrap_or("")
    }
}
const CONTROL_PROFILE: &str = "829284cfd5976cc94cce11fc126856f0d59998cb06065b8d28254e11b5199916";
// Static Adjust with the three supported scalars normalized and all shared
// binary values resolved. A repeated edit can refer to the first stored copy.
const ADJUST_PROFILE: &str = "c819c1741a439cebd703ffecf243cfb6bdf713ed3086d257ac6106ecf7a5721b";
const SOURCE_COLOR: &str =
    r#"{"baseColorProfile":{"colorProfileName":"BT.709 RGB Full"},"baseProfileType":1}"#;
const OUTPUT_COLOR: &str = r#"{"baseColorProfile":{"colorProfileName":"BT.709,8-bit,Display-Referred"},"baseProfileType":1,"colorSpaceMetadata":{"peakLuminance":100}}"#;
const COLOR_MANAGEMENT: &str = r#"{"autoToneMapEnabled":true,"enableLogColorManagement":2,"graphicsWhiteLuminance":203,"lutInterpolationMethod":1}"#;

fn in_range(value: f64) -> bool {
    (-5.0..=5.0).contains(&value)
}

pub(crate) fn qualified_exposure(row: &Value) -> Option<f64> {
    let value = match row.get("exposure")? {
        Value::Float(v) => *v,
        Value::Int(v) => *v as f64,
        _ => return None,
    };
    (in_range(value)
        && row.get("contrast").is_none()
        && row.get("pivot").is_none()
        && matches!(row.get("controls_sha256"), Some(Value::Str(s)) if s == CONTROL_PROFILE)
        && qualified_pipeline(row))
    .then_some(value)
}

fn qualified_pipeline(row: &Value) -> bool {
    matches!(row.get("operation"), Some(Value::Str(s)) if s == "clip Adjust")
        && matches!(row.get("source_color"), Some(Value::Str(s)) if s == SOURCE_COLOR)
        && matches!(row.get("output_color"), Some(Value::Str(s)) if s == OUTPUT_COLOR)
        && row.get("qualified_pipeline") == Some(&Value::Bool(true))
}

pub(crate) fn qualified_adjust(row: &Value) -> Option<crate::clip_fusion::Adjust> {
    if !qualified_pipeline(row)
        || !matches!(row.get("adjust_controls_sha256"), Some(Value::Str(s)) if s == ADJUST_PROFILE)
    {
        return None;
    }
    let scalar = |key, default| match row.get(key) {
        Some(Value::Float(value)) => Some(*value),
        Some(Value::Int(value)) => Some(*value as f64),
        None => Some(default),
        _ => None,
    };
    let exposure = scalar("exposure", 0.0)?;
    let contrast = scalar("contrast", 1.0)?;
    let pivot = scalar("pivot", 0.0)?;
    ((-1.5..=1.0).contains(&exposure) && (0.5..=2.0).contains(&contrast)
        && (-1.0..=1.0).contains(&pivot) && (contrast != 1.0 || pivot != 0.0))
        .then_some(crate::clip_fusion::Adjust { exposure, contrast, pivot })
}

pub(super) fn capture<'a>(
    project: &'a Project,
    sequence: NodeId,
    clip: TrackItemId,
    source: ItemId,
    occurrence: &str,
    payloads: &mut SavedPayloads<'a>,
) -> Result<Option<Sidecar>> {
    let track_item = project.track_item(clip);
    let ([component], []) = (
        track_item.components.as_slice(),
        track_item.selection_components.as_slice(),
    ) else {
        return Ok(None);
    };
    let tree = &project.doc.tree;
    let root = project.component(*component).element;
    let mut operation = root;
    for kind in ["65537", "65541", "65538"] {
        let types: Vec<_> = tree.iter_tag(operation, "CompoundComponentType").collect();
        if tree.tag(operation) != "CompoundComponent"
            || !matches!(types.len(), 1 | 2)
            || types.iter().any(|&id| tree.text(id) != Some(kind))
        {
            return Ok(None);
        }
        let children = chain(project, operation)?;
        if kind == "65538" {
            let Some(&contrast) = children.first() else {
                return Ok(None);
            };
            operation = contrast;
        } else {
            let [only] = children.as_slice() else {
                return Ok(None);
            };
            operation = *only;
        }
    }
    if field(tree, operation, "MatchName")? != Some("AE.ADBE Color And Contrast") {
        return Ok(None);
    }
    let [mut exposure_param, mut contrast_param, mut pivot_param] = [None; 3];
    for reference in tree.findall(operation, "Component/Params/Param")? {
        let param = project.doc.resolve(reference)?;
        let (slot, name) = match field(tree, param, "ParameterID")? {
            Some("2") => (&mut exposure_param, "exposure"),
            Some("4") => (&mut contrast_param, "contrast"),
            Some("5") => (&mut pivot_param, "pivot"),
            _ => continue,
        };
        if slot.replace(param).is_some() {
            return Err(format!("duplicate Premiere Color Mode global {name}").into());
        }
    }
    let exposure = static_value(project, exposure_param)?.filter(|v| in_range(*v));
    let saved_contrast = static_value(project, contrast_param)?;
    let saved_pivot = static_value(project, pivot_param)?;
    let contrast = saved_contrast.filter(|v| (0.0..=2.0).contains(v) && *v != 1.0);
    let pivot = saved_pivot.filter(|v| (-3.0..=3.0).contains(v) && *v != 0.0);
    if exposure.is_none() && contrast.is_none() && pivot.is_none() {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut budget = 64 * 1024;
    let normalized_exposure = exposure.and(exposure_param).map(|id| (id, "0"));
    fingerprint(
        project,
        root,
        normalized_exposure.as_slice(),
        None,
        &mut hash,
        0,
        &mut budget,
    )?;
    let adjust_controls = if let (Some(exposure), Some(contrast_id), Some(pivot_id)) = (
        normalized_exposure,
        saved_contrast.filter(|v| (0.0..=2.0).contains(v)
            && (contrast.is_some() || pivot.is_some())).and(contrast_param),
        saved_pivot.filter(|v| (-3.0..=3.0).contains(v)).and(pivot_param),
    ) {
        let mut hash = Sha256::new();
        let mut budget = 64 * 1024;
        fingerprint(project, root, &[exposure, (contrast_id, "1."), (pivot_id, "0.")],
            Some(payloads), &mut hash, 0, &mut budget)?;
        Some(hex::encode(hash.finalize()))
    } else {
        None
    };
    let mut output = None;
    let mut video_group_seen = false;
    let mut known_pipeline = false;
    for reference in tree.findall(sequence, "TrackGroups/TrackGroup/Second")? {
        let group = project.doc.resolve(reference)?;
        if tree.tag(group) == "VideoTrackGroup" {
            if std::mem::replace(&mut video_group_seen, true) {
                return Err("multiple Premiere color pipeline owners".into());
            }
            output = field(tree, group, "OutputColorSpace")?;
            known_pipeline = field(tree, group, "ColorManagementSettings")?
                == Some(COLOR_MANAGEMENT)
                && field(tree, group, "FrameRect")? == Some("0,0,1920,1080")
                && field(tree, group, "AutoInputGamutCompressionEnabled")? == Some("true")
                && field(tree, group, "ToneMappingDesaturation")? == Some("0.5")
                && field(tree, group, "WorkingColorSpace")?.is_none()
                && chain(project, group)?.is_empty()
                && field(tree, track_item.element, "ToneMapSettings")?
                    == Some(r#"{"peak":-1,"version":3}"#);
        }
    }
    let source_color = project
        .media_element(source)?
        .and_then(|media| tree.find1(media, "VideoStream"))
        .map(|stream| -> Result<_> {
            Ok(field(
                tree,
                project.doc.resolve(stream)?,
                "OriginalColorSpace",
            )?)
        })
        .transpose()?
        .flatten();
    let mut data = vec![("operation".into(), Value::Str("clip Adjust".into()))];
    for (name, saved) in [
        ("exposure", exposure),
        ("contrast", contrast),
        ("pivot", pivot),
    ] {
        if let Some(value) = saved {
            data.push((name.into(), Value::Float(value)));
        }
    }
    data.extend([
        (
            "controls_sha256".into(),
            Value::Str(hex::encode(hash.finalize())),
        ),
        ("qualified_pipeline".into(), Value::Bool(known_pipeline)),
    ]);
    if let Some(witness) = adjust_controls {
        data.push(("adjust_controls_sha256".into(), Value::Str(witness)));
    }
    for (name, value) in [("source_color", source_color), ("output_color", output)] {
        if let Some(value) = value.filter(|value| value.len() <= 1024) {
            data.push((name.into(), Value::Str(value.into())));
        }
    }
    Ok(Some(Sidecar {
        adapter: ADAPTER.into(),
        object: occurrence.into(),
        data: Value::Obj(data),
    }))
}

fn static_value(project: &Project, param: Option<NodeId>) -> Result<Option<f64>> {
    let Some(param) = param else { return Ok(None) };
    let tree = &project.doc.tree;
    let Some(start) = field(tree, param, "StartKeyframe")? else {
        return Ok(None);
    };
    let parts: Vec<_> = start.split(',').take(9).collect();
    let [time, value, "0", "0", "0", "0", "0", "0"] = parts.as_slice() else {
        return Ok(None);
    };
    if time.parse::<i64>().ok() != Some(super::controls::DEFAULT_TICKS)
        || field(tree, param, "Keyframes")?.is_some()
        || !matches!(field(tree, param, "IsTimeVarying")?, None | Some("false"))
    {
        return Ok(None);
    }
    Ok(value.parse::<f64>().ok())
}

fn chain(project: &Project, owner: NodeId) -> Result<Vec<NodeId>> {
    let tree = &project.doc.tree;
    let Some(reference) = tree.find1(owner, "ComponentOwner/Components") else {
        return Ok(vec![]);
    };
    let chain = project.doc.resolve(reference)?;
    let references = tree.findall(chain, "ComponentChain/Components/Component")?;
    ordered_references(project, &references, "Component")
}

fn ordered_references(project: &Project, references: &[NodeId], tag: &str) -> Result<Vec<NodeId>> {
    let tree = &project.doc.tree;
    references
        .iter()
        .enumerate()
        .map(|(index, &reference)| {
            if tree.tag(reference) != tag
                || tree
                    .get(reference, "Index")
                    .and_then(|n| n.parse::<usize>().ok())
                    != Some(index)
            {
                return Err("contradictory Premiere Color Mode control order".into());
            }
            Ok(project.doc.resolve(reference)?)
        })
        .collect()
}

// Compare the complete ordered Adjust controls to the neutral witness,
// normalizing only the chosen Exposure value and saved component IDs.
fn fingerprint<'a>(
    project: &'a Project,
    node: NodeId,
    normalized: &[(NodeId, &str)],
    mut payloads: Option<&mut SavedPayloads<'a>>,
    hash: &mut Sha256,
    depth: usize,
    budget: &mut usize,
) -> Result<()> {
    if depth > 16 {
        return Err("Premiere Color Mode component nesting exceeds its limit".into());
    }
    let tree = &project.doc.tree;
    let mut add = |text: &str| -> Result<()> {
        *budget = budget
            .checked_sub(text.len() + 1)
            .ok_or("Premiere Color Mode controls exceed their byte budget")?;
        hash.update(text.as_bytes());
        hash.update([0]);
        Ok(())
    };
    add(tree.tag(node))?;
    let mut attrs: Vec<_> = tree[node]
        .attrs
        .iter()
        .filter(|(k, _)| k != "ObjectID")
        .collect();
    attrs.sort_unstable();
    for (key, value) in attrs {
        add(key)?;
        add(value)?;
    }
    add("")?;
    let text = if let Some(cache) = payloads.as_deref_mut() {
        cache.text(project, node)
    } else {
        tree.text(node).unwrap_or("").trim()
    };
    let adjustment = normalized.iter().find(|(id, _)| tree.parent(node) == Some(*id));
    if let Some((_, neutral)) = adjustment.filter(|_| tree.tag(node) == "StartKeyframe") {
        let mut values: Vec<_> = text.split(',').take(9).collect();
        values[1] = neutral;
        add(&values.join(","))?;
    } else {
        add(text)?;
    }
    for &child in tree.children(node) {
        match tree.tag(child) {
            "InstanceName" | "DisplayName" => (),
            "Node"
                if tree.children(child).len() == 1
                    && tree.child_by_tag(child, "Properties").is_some_and(|bag| {
                        matches!(tree.children(bag), [flag] if matches!(tree.tag(*flag),
                            "ECP.Filter.Expanded" | "ECP.Custom.Expanded"))
                    }) =>
            {
                ()
            }
            "ComponentOwner" => {
                for nested in chain(project, node)? {
                    fingerprint(project, nested, normalized, payloads.as_deref_mut(), hash, depth + 1, budget)?;
                }
            }
            "Params" => {
                for param in ordered_references(project, tree.children(child), "Param")? {
                    fingerprint(project, param, normalized, payloads.as_deref_mut(), hash, depth + 1, budget)?;
                }
            }
            _ => fingerprint(project, child, normalized, payloads.as_deref_mut(), hash, depth + 1, budget)?,
        }
    }
    *budget = budget
        .checked_sub(1)
        .ok_or("Premiere Color Mode controls exceed their byte budget")?;
    hash.update([0]);
    Ok(())
}
