//! Saved clip-owned Color Mode Adjust in Premiere 27.
use super::components::field;
use crate::Result;
use graph::{Sidecar, Value};
use prproj::{
    model::{ItemId, Project, TrackItemId},
    xml::NodeId,
};
use sha2::{Digest, Sha256};

pub(crate) const ADAPTER: &str = "premiere.color_mode_exposure";
const CONTROL_PROFILE: &str = "829284cfd5976cc94cce11fc126856f0d59998cb06065b8d28254e11b5199916";
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
        && matches!(row.get("operation"), Some(Value::Str(s)) if s == "clip Adjust")
        && matches!(row.get("controls_sha256"), Some(Value::Str(s)) if s == CONTROL_PROFILE)
        && matches!(row.get("source_color"), Some(Value::Str(s)) if s == SOURCE_COLOR)
        && matches!(row.get("output_color"), Some(Value::Str(s)) if s == OUTPUT_COLOR)
        && row.get("qualified_pipeline") == Some(&Value::Bool(true)))
    .then_some(value)
}

pub(super) fn capture(
    project: &Project,
    sequence: NodeId,
    clip: TrackItemId,
    source: ItemId,
    occurrence: &str,
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
    let contrast =
        static_value(project, contrast_param)?.filter(|v| (0.0..=2.0).contains(v) && *v != 1.0);
    let pivot =
        static_value(project, pivot_param)?.filter(|v| (-3.0..=3.0).contains(v) && *v != 0.0);
    if exposure.is_none() && contrast.is_none() && pivot.is_none() {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut budget = 64 * 1024;
    let normalized_exposure = exposure.and(exposure_param);
    fingerprint(
        project,
        root,
        normalized_exposure,
        &mut hash,
        0,
        &mut budget,
    )?;
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
fn fingerprint(
    project: &Project,
    node: NodeId,
    exposure: Option<NodeId>,
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
    let text = tree.text(node).unwrap_or("").trim();
    if tree.tag(node) == "StartKeyframe" && exposure.is_some() && tree.parent(node) == exposure {
        let mut values: Vec<_> = text.split(',').take(9).collect();
        values[1] = "0";
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
                    fingerprint(project, nested, exposure, hash, depth + 1, budget)?;
                }
            }
            "Params" => {
                for param in ordered_references(project, tree.children(child), "Param")? {
                    fingerprint(project, param, exposure, hash, depth + 1, budget)?;
                }
            }
            _ => fingerprint(project, child, exposure, hash, depth + 1, budget)?,
        }
    }
    *budget = budget
        .checked_sub(1)
        .ok_or("Premiere Color Mode controls exceed their byte budget")?;
    hash.update([0]);
    Ok(())
}
