//! Compare saved Resolve generators and literal title controls.
use super::{exact, graph_value, known, Diff, Timeline};
use crate::{
    generator::{self, Solid},
    nests::{int, string},
    source::{seconds, title},
};
use graph::{source::TextKind, Rational, StreamKind, Value};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

type Slot = (&'static str, i64, Rational);

struct Graphic<'a> {
    id: &'a str,
    duration: Rational,
    content: Content<'a>,
}

enum Content<'a> {
    Solid([u16; 3]),
    Text {
        text: Option<&'a str>,
        receipt: Option<&'a Value>,
    },
}

#[derive(Default)]
struct Graphics<'a> {
    slots: BTreeMap<Slot, Vec<Graphic<'a>>>,
    incomplete: bool,
}

impl<'a> Graphics<'a> {
    fn read(t: &Timeline<'a>) -> Self {
        let mut result = Self::default();
        let mut titles = HashMap::new();
        if let Some(data) = t.graph.sidecar(title::ADAPTER, &t.seq.id) {
            if let Value::Arr(rows) = data {
                for row in rows {
                    if let Some(id) = string(row, "id") {
                        titles
                            .entry(id)
                            .and_modify(|r| *r = None)
                            .or_insert(Some(row));
                    } else {
                        result.incomplete = true;
                    }
                }
            } else {
                result.incomplete = true;
            }
        }
        for text in t
            .seq
            .texts
            .iter()
            .filter(|text| text.kind == TextKind::Graphic)
        {
            let receipt = titles.remove(text.id.as_str()).flatten();
            let Some((start, duration)) = text.record_start.zip(text.duration) else {
                result.incomplete = true;
                continue;
            };
            result
                .slots
                .entry(("text_plus", text.track, start))
                .or_default()
                .push(Graphic {
                    id: &text.id,
                    duration,
                    content: Content::Text {
                        text: text.content.as_deref().filter(|_| text.reason.is_none()),
                        receipt,
                    },
                });
        }
        result.incomplete |= !titles.is_empty();
        if let Some(data) = t.graph.sidecar(generator::ADAPTER, &t.seq.id) {
            if let Value::Arr(rows) = data {
                for row in rows {
                    if let Some((slot, graphic)) = solid(t, row) {
                        result.slots.entry(slot).or_default().push(graphic);
                    } else {
                        result.incomplete = true;
                    }
                }
            } else {
                result.incomplete = true;
            }
        }
        result
    }
}

fn solid<'a>(t: &Timeline<'_>, row: &'a Value) -> Option<(Slot, Graphic<'a>)> {
    let id = string(row, "id").filter(|id| !id.is_empty())?;
    let (track, solid) = Solid::from_receipt(row).ok()?;
    if t.receipts
        .get(t.seq.id.as_str())
        .and_then(|owner| int(owner, "record_length"))
        .is_some_and(|end| solid.start + solid.duration > end)
    {
        return None;
    }
    let start = seconds(solid.start, t.seq.rate).ok()?;
    let duration = seconds(solid.duration, t.seq.rate).ok()?;
    Some((
        ("solid_color", track, start),
        Graphic {
            id,
            duration,
            content: Content::Solid(solid.rgb),
        },
    ))
}

pub(super) fn compare(left: &Timeline<'_>, right: &Timeline<'_>, diff: &mut Diff) {
    let a = Graphics::read(left);
    let b = Graphics::read(right);
    for (side, data) in [("left", &a), ("right", &b)] {
        if data.incomplete {
            diff.gap(
                "graphic",
                &json!({"side":side}),
                "a saved title or generator could not be located",
            );
        }
    }
    let left_complete = complete(left, &a);
    let right_complete = complete(right, &b);
    let slots: BTreeSet<_> = a.slots.keys().chain(b.slots.keys()).copied().collect();
    for slot @ (kind, track, start) in slots {
        let old = a.slots.get(&slot).map(Vec::as_slice).unwrap_or_default();
        let new = b.slots.get(&slot).map(Vec::as_slice).unwrap_or_default();
        let context = json!({"kind":kind, "track":track, "record_start":exact(start)});
        match (old, new) {
            ([old], [new]) => compare_pair(old, new, context, diff),
            ([row], []) if right_complete => {
                diff.change("graphic", &context, summary(row), Json::Null)
            }
            ([], [row]) if left_complete => {
                diff.change("graphic", &context, Json::Null, summary(row))
            }
            _ => diff.gap(
                "graphic",
                &context,
                "title or generator occurrence could not be paired",
            ),
        }
    }
}

fn complete(t: &Timeline<'_>, graphics: &Graphics<'_>) -> bool {
    let audio_only = |reason: &str| {
        let Some((id, _)) = reason.split_once(": ") else {
            return false;
        };
        matches!(t.graph.sidecar(crate::source::OMITTED_TRACKS_ADAPTER, &t.seq.id),
        Some(Value::Arr(rows)) if rows.iter().any(|row| {
            string(row, "id") == Some(id) && string(row, "kind") == Some(StreamKind::Audio.as_str())
        }))
    };
    !graphics.incomplete
        && !t.losses.iter().any(|loss| {
            loss.object == t.seq.id
                && (loss.aspect == "placement"
                    || loss.aspect == "track" && !audio_only(&loss.reason)
                    || loss.aspect == "tracks"
                        && loss.reason != "unsupported Resolve subtitle track")
        })
}

fn summary(row: &Graphic<'_>) -> Json {
    let mut out = json!({"id":row.id, "duration":exact(row.duration)});
    match row.content {
        Content::Solid(rgb) => out["rgb16"] = json!(rgb),
        Content::Text { text, .. } => out["text"] = json!(text),
    }
    out
}

fn compare_pair(old: &Graphic<'_>, new: &Graphic<'_>, mut context: Json, diff: &mut Diff) {
    context["left_id"] = json!(old.id);
    context["right_id"] = json!(new.id);
    diff.change(
        "graphic.duration",
        &context,
        exact(old.duration),
        exact(new.duration),
    );
    match (&old.content, &new.content) {
        (Content::Solid(a), Content::Solid(b)) => {
            diff.change("solid_color.rgb16", &context, json!(a), json!(b))
        }
        (
            Content::Text {
                text: a,
                receipt: ar,
            },
            Content::Text {
                text: b,
                receipt: br,
            },
        ) => {
            known(
                diff,
                "text_plus.text",
                &context,
                a.map(|v| json!(v)),
                b.map(|v| json!(v)),
            );
            compare_composition(*ar, *br, context, diff);
        }
        _ => unreachable!("graphics are paired within their saved kind"),
    }
}

fn compare_composition(a: Option<&Value>, b: Option<&Value>, mut context: Json, diff: &mut Diff) {
    fn hash(row: Option<&Value>) -> Option<&str> {
        string(row?, "composition_sha256")
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    }
    let hashes = (hash(a), hash(b));
    if hashes.0.is_some() && hashes.0 == hashes.1 {
        return;
    }
    for control in [
        "font",
        "style",
        "size",
        "vertical_justification",
        "horizontal_justification",
        "red",
        "green",
        "blue",
    ] {
        let saved = |row: Option<&Value>| match row.and_then(|row| row.get(control)) {
            Some(Value::Str(value)) if matches!(control, "font" | "style") => Some(json!(value)),
            Some(value @ (Value::Int(_) | Value::Float(_)))
                if !matches!(control, "font" | "style") =>
            {
                Some(graph_value(value))
            }
            _ => None,
        };
        if let (Some(left), Some(right)) = (saved(a), saved(b)) {
            diff.change(&format!("text_plus.{control}"), &context, left, right);
        }
    }
    context["left_composition_sha256"] = json!(hashes.0);
    context["right_composition_sha256"] = json!(hashes.1);
    diff.gap(
        "text_plus.composition",
        &context,
        "composition bytes differ or are unavailable",
    );
}
