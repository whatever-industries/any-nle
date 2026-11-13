//! Compare saved clip plug-in records on paired audio edits.
use super::{graph_value, Diff};
use graph::Value;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};

type Plugins<'a> = BTreeMap<u32, &'a Value>;

fn records(receipt: Option<&Value>) -> Option<(&str, Plugins<'_>)> {
    let Some(receipt) = receipt else {
        return Some(("", Plugins::new()));
    };
    let (Some(Value::Str(hash)), Some(Value::Arr(rows))) =
        (receipt.get("clip_fx_sha256"), receipt.get("plugins"))
    else {
        return None;
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut plugins = Plugins::new();
    for row in rows {
        let (
            Some(Value::Int(id)),
            Some(Value::Str(name)),
            Some(Value::Int(flags)),
            Some(Value::Obj(parameters)),
        ) = (
            row.get("effect_id"),
            row.get("name"),
            row.get("state_flags"),
            row.get("raw_parameters"),
        )
        else {
            return None;
        };
        let id = u32::try_from(*id).ok()?;
        if !matches!(name.as_str(), "Voice Isolation" | "Dialogue Leveler")
            || u32::try_from(*flags).is_err()
            || !parameters.iter().all(|(_, value)| {
                matches!(value, Value::Float(n) if (0.0..=1.0).contains(n))
                    || matches!(value, Value::Int(0 | 1))
            })
            || plugins.insert(id, row).is_some()
        {
            return None;
        }
    }
    Some((hash, plugins))
}

pub(super) fn compare(a: Option<&Value>, b: Option<&Value>, context: &Json, diff: &mut Diff) {
    if a.is_none() && b.is_none() {
        return;
    }
    let (Some((a_hash, left)), Some((b_hash, right))) = (records(a), records(b)) else {
        diff.gap(
            "edit.clip_fairlight",
            context,
            "saved clip plug-in records are malformed or ambiguous",
        );
        return;
    };
    let value = |row: Option<&&Value>| {
        row.map_or(Json::Null, |row| {
            json!({
                "name": row.get("name").map(graph_value),
                "state_flags": row.get("state_flags").map(graph_value),
                "raw_parameters": row.get("raw_parameters").map(graph_value),
            })
        })
    };
    for id in left
        .keys()
        .chain(right.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        let mut context = context.clone();
        context["effect_id"] = json!(id);
        diff.change(
            "edit.clip_fairlight",
            &context,
            value(left.get(&id)),
            value(right.get(&id)),
        );
    }
    if a_hash != b_hash {
        let mut context = context.clone();
        context["left_clip_fx_sha256"] = json!(a.map(|_| a_hash));
        context["right_clip_fx_sha256"] = json!(b.map(|_| b_hash));
        diff.gap(
            "edit.clip_fairlight_payload",
            &context,
            "clip FX payload differs beyond what the decoded controls establish",
        );
    }
}
