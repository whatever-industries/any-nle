//! Materialized Premiere clip parameters, in their saved coordinate and time domains.
use crate::Result;
use graph::{Sidecar, StreamKind, Value};
use prproj::{
    model::{
        component::{component_inner, decode_value, ParamValue, AUDIO_SCALAR_CLASS},
        Project, TrackItemId,
    },
    xml::{NodeId, Tree},
};

pub(super) const ADAPTER: &str = "premiere.component_parameters";
const POINT: &str = "ca81d347-309b-44d2-acc7-1c572efb973c";
const SCALAR: &str = "fe47129e-6c94-4fc0-95d5-c056a517aaf3";
const CHECKBOX: &str = "cc12343e-f113-4d3b-ae05-b287db77d461";
const MAX_PARAMETER_BYTES: usize = 32 * 1024;
const MAX_CURVE_KEYS: usize = 1024;
const MAX_SEQUENCE_PARAMETER_BYTES: usize = 1024 * 1024;
const MAX_SEQUENCE_RECORDS: usize = 8192;
const MAX_ENTRY_BYTES: usize = 512;

#[derive(Default)]
pub(super) struct Budget {
    bytes: usize,
    records: usize,
}

impl Budget {
    fn charge(&mut self, bytes: usize, records: usize) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= MAX_SEQUENCE_PARAMETER_BYTES)
            .ok_or("Premiere component parameter byte budget exceeded")?;
        self.records = self
            .records
            .checked_add(records)
            .filter(|n| *n <= MAX_SEQUENCE_RECORDS)
            .ok_or("Premiere component key record budget exceeded")?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Parameter {
    name: &'static str,
    class: &'static str,
    tag: &'static str,
    units: &'static str,
}

const POSITION: Parameter = Parameter {
    name: "position",
    class: POINT,
    tag: "PointComponentParam",
    units: "premiere_normalized_xy",
};
const SCALE: Parameter = Parameter {
    name: "scale",
    class: SCALAR,
    tag: "VideoComponentParam",
    units: "percent",
};
const SCALE_WIDTH: Parameter = Parameter {
    name: "scale_width",
    ..SCALE
};
const UNIFORM_SCALE: Parameter = Parameter {
    name: "uniform_scale",
    class: CHECKBOX,
    tag: "VideoComponentParam",
    units: "boolean",
};
const VOLUME: Parameter = Parameter {
    name: "level",
    class: AUDIO_SCALAR_CLASS,
    tag: "AudioComponentParam",
    units: "premiere_clip_volume_level",
};

/// Capture Premiere 27's current saved project format without upgrading older inputs.
pub(super) fn is_current_project(project: &Project) -> Result<bool> {
    let tree = &project.doc.tree;
    let mut refs = tree
        .children(project.doc.root)
        .iter()
        .copied()
        .filter(|&node| tree.tag(node) == "Project" && tree.get(node, "ObjectRef").is_some());
    let Some(reference) = refs.next() else {
        return Ok(false);
    };
    if refs.next().is_some() {
        return Err("duplicate Premiere project reference".into());
    }
    let native = project.doc.resolve(reference)?;
    Ok(tree.tag(native) == "Project" && tree.get(native, "Version") == Some("46"))
}

pub(super) fn capture(
    project: &Project,
    clip: TrackItemId,
    kind: StreamKind,
    object: &str,
    budget: &mut Budget,
) -> Result<Option<Sidecar>> {
    let tree = &project.doc.tree;
    let mut components = Vec::new();
    for (index, &component) in project.track_item(clip).components.iter().enumerate() {
        let data = project.component(component);
        let element = data.element;
        let match_name = project.component_match_name(component);
        let Some(owner) = component_inner(tree, element) else {
            continue;
        };
        let motion = match_name == "AE.ADBE Motion"
            && kind == StreamKind::Video
            && tree.tag(element) == "VideoFilterComponent";
        let volume = match_name == "Internal Volume Mono"
            && kind == StreamKind::Audio
            && tree.tag(element) == "AudioFilterComponent";
        if !(motion || volume) || field(tree, owner, "Intrinsic")? != Some("true") {
            continue;
        }
        let mut parameters = Vec::new();
        if motion {
            for &param in &data.properties {
                let profile = match field(tree, param, "ParameterID")? {
                    Some("1") => POSITION,
                    Some("2") => SCALE,
                    Some("3") => SCALE_WIDTH,
                    Some("4") => UNIFORM_SCALE,
                    _ => continue,
                };
                capture_parameter(tree, param, profile, budget, &mut parameters)?;
            }
        } else {
            for reference in tree.findall(owner, "Params/Param")? {
                if tree.get(reference, "Index") == Some("1") {
                    let param = project.doc.resolve(reference)?;
                    capture_parameter(tree, param, VOLUME, budget, &mut parameters)?;
                }
            }
        }
        if !parameters.is_empty() {
            components.push(Value::Obj(vec![
                ("chain_index".into(), Value::Int(i64::try_from(index)?)),
                ("match_name".into(), Value::Str(match_name)),
                ("native_object_id".into(), native_id(tree, element)?),
                ("parameters".into(), Value::Arr(parameters)),
            ]));
        }
    }
    Ok((!components.is_empty()).then(|| Sidecar {
        adapter: ADAPTER.into(),
        object: object.into(),
        data: Value::Arr(components),
    }))
}

fn capture_parameter(
    tree: &Tree,
    param: NodeId,
    profile: Parameter,
    budget: &mut Budget,
    parameters: &mut Vec<Value>,
) -> Result<()> {
    if tree.tag(param) != profile.tag || tree.get(param, "ClassID") != Some(profile.class) {
        return Ok(());
    }
    if parameters
        .iter()
        .any(|p| matches!(p.get("parameter"), Some(Value::Str(name)) if name == profile.name))
    {
        return Err(format!("duplicate saved Premiere {} parameter", profile.name).into());
    }
    let start = field(tree, param, "StartKeyframe")?;
    let current = field(tree, param, "CurrentValue")?;
    let keys = field(tree, param, "Keyframes")?.unwrap_or("");
    if keys.len() > MAX_PARAMETER_BYTES {
        return Err(format!("Premiere {} keyframe data exceeds limit", profile.name).into());
    }
    let flag = match field(tree, param, "IsTimeVarying")? {
        Some("true") => Value::Bool(true),
        Some("false") => Value::Bool(false),
        None => Value::Null,
        _ => return Err("invalid Premiere IsTimeVarying flag".into()),
    };
    if start.is_none() && current.is_none() && keys.is_empty() {
        return Ok(());
    }
    let mut records = Vec::new();
    for entry in keys.split_terminator(';') {
        if records.len() == MAX_CURVE_KEYS {
            return Err(format!("Premiere {} key count exceeds limit", profile.name).into());
        }
        records.push(record(entry, profile, budget)?);
    }
    let start = start
        .map(|entry| record(entry, profile, budget))
        .transpose()?
        .unwrap_or(Value::Null);
    let current = current
        .map(|entry| {
            budget.charge(entry.len(), 1)?;
            numeric(entry, profile)
        })
        .transpose()?
        .unwrap_or(Value::Null);
    parameters.push(Value::Obj(vec![
        ("parameter".into(), Value::Str(profile.name.into())),
        ("native_object_id".into(), native_id(tree, param)?),
        ("units".into(), Value::Str(profile.units.into())),
        ("is_time_varying".into(), flag),
        ("start_keyframe".into(), start),
        ("current_value".into(), current),
        ("keyframes".into(), Value::Arr(records)),
    ]));
    Ok(())
}

pub(super) fn field<'a>(tree: &'a Tree, node: NodeId, tag: &str) -> Result<Option<&'a str>> {
    let mut matches = tree
        .children(node)
        .iter()
        .copied()
        .filter(|&child| tree.tag(child) == tag);
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() || !tree.children(first).is_empty() {
        return Err(format!("ambiguous Premiere component field {tag}").into());
    }
    Ok(tree.text(first).map(str::trim).filter(|v| !v.is_empty()))
}

fn native_id(tree: &Tree, element: NodeId) -> Result<Value> {
    let id = tree
        .get(element, "ObjectID")
        .ok_or("missing Premiere component ObjectID")?;
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid Premiere component ObjectID".into());
    }
    Ok(Value::Str(id.into()))
}

fn numeric(raw: &str, profile: Parameter) -> Result<Value> {
    if raw.len() > MAX_ENTRY_BYTES {
        return Err("Premiere component value exceeds limit".into());
    }
    match decode_value(raw)? {
        ParamValue::List(v)
            if profile.class == POINT && v.len() == 2 && v.iter().all(|n| n.is_finite()) =>
        {
            Ok(Value::Arr(v.into_iter().map(Value::Float).collect()))
        }
        ParamValue::Bool(v) if profile.class == CHECKBOX => Ok(Value::Bool(v)),
        ParamValue::Float(v)
            if matches!(profile.class, SCALAR | AUDIO_SCALAR_CLASS) && v.is_finite() =>
        {
            Ok(Value::Float(v))
        }
        _ => Err(format!("invalid saved Premiere {} value", profile.name).into()),
    }
}

fn record(entry: &str, profile: Parameter, budget: &mut Budget) -> Result<Value> {
    if entry.len() > MAX_ENTRY_BYTES {
        return Err("Premiere component key record exceeds limit".into());
    }
    budget.charge(entry.len(), 1)?;
    let fields: Vec<_> = entry.trim().split(',').collect();
    if fields.len() != if profile.class == POINT { 14 } else { 8 } {
        return Err(format!(
            "unexpected saved Premiere {} key record width",
            profile.name
        )
        .into());
    }
    if fields[4..]
        .iter()
        .any(|field| !field.parse::<f64>().is_ok_and(f64::is_finite))
    {
        return Err(format!(
            "invalid saved Premiere {} auxiliary key fields",
            profile.name
        )
        .into());
    }
    Ok(Value::Obj(vec![
        ("ticks".into(), Value::Int(fields[0].parse()?)),
        ("value".into(), numeric(fields[1], profile)?),
        (
            "interpolation_code_2".into(),
            Value::Int(fields[2].parse()?),
        ),
        (
            "interpolation_code_3".into(),
            Value::Int(fields[3].parse()?),
        ),
        // Retain numeric auxiliary fields without assigning a tangent unit or
        // evaluating the curve. Their original spellings are meaningful evidence.
        (
            "auxiliary_fields".into(),
            Value::Arr(
                fields[4..]
                    .iter()
                    .map(|s| Value::Str((*s).into()))
                    .collect(),
            ),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameter(tree: &mut Tree, profile: Parameter) -> NodeId {
        tree.new_element_with(
            profile.tag,
            &[("ObjectID", "9"), ("ClassID", profile.class)],
        )
    }

    fn leaf(tree: &mut Tree, param: NodeId, name: &str, value: &str) {
        let child = tree.sub_element(param, name, &[]);
        tree.set_text(child, Some(value));
    }

    #[test]
    fn saved_position_retains_native_ticks_and_separate_start_value() {
        let mut tree = Tree::new();
        let param = parameter(&mut tree, POSITION);
        leaf(
            &mut tree,
            param,
            "StartKeyframe",
            "-91445760000000000,0.625:0.375,0,0,0,0,0,0,5,4,0,0,0,0",
        );
        leaf(&mut tree, param, "Keyframes", "10584000001,0.25:0.5,0,5,0,0,0,0,5,4,-0.125,0,0,0;21168000000,0.625:0.375,4,0,0,0,0,0,5,4,0,0,0,0;");
        let mut result = vec![];
        capture_parameter(&tree, param, POSITION, &mut Budget::default(), &mut result).unwrap();
        assert_eq!(result.len(), 1);
        let data = &result[0];
        assert_eq!(data.get("is_time_varying"), Some(&Value::Null));
        assert_eq!(data.get("current_value"), Some(&Value::Null));
        let start = data.get("start_keyframe").unwrap();
        assert_eq!(
            start.get("ticks"),
            Some(&Value::Int(-91_445_760_000_000_000))
        );
        let Some(Value::Arr(keys)) = data.get("keyframes") else {
            panic!("missing keys")
        };
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].get("ticks"), Some(&Value::Int(10_584_000_001)));
        assert_eq!(
            keys[0].get("value"),
            Some(&Value::Arr(vec![Value::Float(0.25), Value::Float(0.5)]))
        );
        assert_eq!(keys[0].get("interpolation_code_3"), Some(&Value::Int(5)));
        assert_eq!(keys[1].get("interpolation_code_2"), Some(&Value::Int(4)));
        assert_eq!(start.get("value"), keys[1].get("value"));
        assert!(
            matches!(keys[0].get("auxiliary_fields"), Some(Value::Arr(fields)) if fields[6] == Value::Str("-0.125".into()))
        );
    }

    #[test]
    fn saved_volume_keeps_raw_level_and_static_scale_requires_saved_data() {
        let mut tree = Tree::new();
        let level = parameter(&mut tree, VOLUME);
        leaf(&mut tree, level, "CurrentValue", "0.177827939391");
        leaf(
            &mut tree,
            level,
            "StartKeyframe",
            "-91445760000000000,0.35,0,0,0,0,0,0",
        );
        let scale = parameter(&mut tree, SCALE);
        let mut budget = Budget::default();
        let mut result = vec![];
        capture_parameter(&tree, level, VOLUME, &mut budget, &mut result).unwrap();
        capture_parameter(&tree, scale, SCALE, &mut budget, &mut result).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].get("current_value"),
            Some(&Value::Float(0.177827939391))
        );
        assert_eq!(
            result[0].get("start_keyframe").unwrap().get("value"),
            Some(&Value::Float(0.35))
        );
        assert_eq!(result[0].get("keyframes"), Some(&Value::Arr(vec![])));
    }

    #[test]
    fn malformed_records_duplicate_fields_and_excessive_curves_refuse() {
        let mut budget = Budget::default();
        for entry in ["10,1,0", "10,1,0,0,0,0,0,NaN", "10,1e999,0,0,0,0,0,0"] {
            assert!(record(entry, SCALE, &mut budget).is_err(), "{entry}");
        }
        let mut tree = Tree::new();
        let param = parameter(&mut tree, SCALE);
        leaf(&mut tree, param, "CurrentValue", "50");
        leaf(&mut tree, param, "CurrentValue", "51");
        assert!(capture_parameter(&tree, param, SCALE, &mut budget, &mut vec![]).is_err());

        let param = parameter(&mut tree, SCALE);
        leaf(
            &mut tree,
            param,
            "Keyframes",
            &"1,2,0,0,0,0,0,0;".repeat(MAX_CURVE_KEYS + 1),
        );
        let error = capture_parameter(&tree, param, SCALE, &mut budget, &mut vec![]).unwrap_err();
        assert!(error.to_string().contains("key count exceeds limit"));

        let mut exhausted = Budget {
            bytes: MAX_SEQUENCE_PARAMETER_BYTES,
            records: 0,
        };
        assert!(record("10,1,0,0,0,0,0,0", SCALE, &mut exhausted).is_err());
    }
}
