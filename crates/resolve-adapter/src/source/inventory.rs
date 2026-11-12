//! Saved timeline effect payloads, independent of graph admission.
use super::{
    archive_is_drt, drt, encoding, expect, read_members, report, tracks, xml, Node, ADAPTER,
};
use crate::{dissolve, Result};
use graph::{source::SourceGraph, StreamKind, Value};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    path::Path,
};

const FIELD: &str = "EffectFiltersBA";

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Recognition {
    Recognized,
    Unqualified,
    NotEvaluated,
}

#[derive(Serialize)]
struct Effect<'a> {
    sequence_id: Option<&'a str>,
    container_id: Option<&'a str>,
    track_kind: &'static str,
    track_index: usize,
    object_id: Option<&'a str>,
    object_class: &'a str,
    field: &'static str,
    size_bytes: Option<usize>,
    sha256: Option<String>,
    payload_error: Option<&'static str>,
    selector: Option<Selector<'a>>,
    selector_error: Option<&'static str>,
    recognition: Recognition,
}

#[derive(Serialize)]
struct Selector<'a> {
    object_id: &'a str,
    field: &'static str,
}

#[derive(Serialize)]
struct ScanGap<'a> {
    member: &'a str,
    track_id: Option<&'a str>,
    reason: String,
}

#[derive(Serialize)]
struct Inventory<'a> {
    schema: &'static str,
    interpretation_error: Option<String>,
    interpretation_gaps: Vec<serde_json::Value>,
    scan_gaps: Vec<ScanGap<'a>>,
    effects: Vec<Effect<'a>>,
}

pub fn write_effects(path: &Path, writer: &mut impl Write) -> Result<()> {
    let timeline_export = archive_is_drt(path)?;
    let members = xml::read(path)?;
    let mut inventory = saved_effects(&members);
    let graph = if timeline_export {
        drt::inspect_members(&members).and_then(|mut inspection| {
            inventory.interpretation_gaps = inspection.take_gaps();
            inspection.into_graph()
        })
    } else {
        read_members(&members, false)
    };
    match graph {
        Ok(graph) => apply_recognition(&graph, &mut inventory.effects),
        Err(error) => inventory.interpretation_error = Some(error.to_string()),
    }
    report::write(writer, &inventory)
}

fn saved_effects(members: &BTreeMap<String, Node>) -> Inventory<'_> {
    let duplicates = duplicate_ids(members);
    let mut inventory = Inventory {
        schema: "anynle.resolve-effect-inventory/1",
        interpretation_error: None,
        interpretation_gaps: Vec::new(),
        scan_gaps: Vec::new(),
        effects: Vec::new(),
    };
    for (member, container) in members
        .iter()
        .filter(|(name, _)| name.starts_with("SeqContainer/"))
    {
        let tracks =
            match expect(container, "Sm2SequenceContainer").and_then(|()| tracks(container)) {
                Ok(tracks) => tracks,
                Err(error) => {
                    inventory.scan_gaps.push(ScanGap {
                        member,
                        track_id: None,
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
        let mut indexes = [0, 0];
        for (kind, track) in tracks {
            let index = if kind == StreamKind::Video {
                &mut indexes[0]
            } else {
                &mut indexes[1]
            };
            *index += 1;
            let sequence_id = track
                .maybe("Sequence")
                .ok()
                .flatten()
                .map(str::trim)
                .filter(|id| !id.is_empty());
            let items = track.one("Items").and_then(Node::elements);
            let mut gap = |error: Box<dyn std::error::Error>| {
                inventory.scan_gaps.push(ScanGap {
                    member,
                    track_id: track.id.as_deref(),
                    reason: error.to_string(),
                });
            };
            match items {
                Err(error) => gap(error),
                Ok(items) => {
                    for entry in items {
                        match entry.entry() {
                            Ok(item) => {
                                if let Some(effect) = saved_effect(
                                    sequence_id,
                                    container,
                                    kind,
                                    *index,
                                    item,
                                    &duplicates,
                                ) {
                                    inventory.effects.push(effect);
                                }
                            }
                            Err(error) => gap(error),
                        }
                    }
                }
            }
        }
    }
    inventory
}

fn duplicate_ids(members: &BTreeMap<String, Node>) -> HashSet<&str> {
    fn visit<'a>(node: &'a Node, seen: &mut HashSet<&'a str>, duplicates: &mut HashSet<&'a str>) {
        if let Some(id) = node.id.as_deref() {
            if !seen.insert(id) {
                duplicates.insert(id);
            }
        }
        for child in &node.children {
            visit(child, seen, duplicates);
        }
    }

    let (mut seen, mut duplicates) = (HashSet::new(), HashSet::new());
    for root in members.values() {
        visit(root, &mut seen, &mut duplicates);
    }
    duplicates
}

fn saved_effect<'a>(
    sequence_id: Option<&'a str>,
    container: &'a Node,
    kind: StreamKind,
    track_index: usize,
    item: &'a Node,
    duplicates: &HashSet<&str>,
) -> Option<Effect<'a>> {
    let (size_bytes, sha256, payload_error) = match item.maybe(FIELD) {
        Ok(None | Some("")) => return None,
        Ok(Some(encoded)) if encoded.len() > 2 * encoding::MAX_BLOB => {
            if encoded.len() % 2 == 0 && encoded.bytes().all(|b| b.is_ascii_hexdigit()) {
                (Some(encoded.len() / 2), None, Some("over_limit"))
            } else {
                (None, None, Some("invalid_hex"))
            }
        }
        Ok(Some(encoded)) => match encoding::hex_bytes(encoded) {
            Ok(bytes) => (
                Some(bytes.len()),
                Some(hex::encode(Sha256::digest(bytes))),
                None,
            ),
            Err(_) => (None, None, Some("invalid_hex")),
        },
        Err(_) => (None, None, Some("invalid_field")),
    };
    let object_id = item.id.as_deref();
    let selector_error = match object_id {
        None => Some("missing_object_id"),
        Some(id) if duplicates.contains(id) => Some("duplicate_object_id"),
        Some(_) => payload_error,
    };
    let selector = object_id
        .filter(|_| selector_error.is_none())
        .map(|object_id| Selector {
            object_id,
            field: FIELD,
        });
    Some(Effect {
        sequence_id,
        container_id: container.id.as_deref(),
        track_kind: kind.as_str(),
        track_index,
        object_id,
        object_class: &item.name,
        field: FIELD,
        size_bytes,
        sha256,
        payload_error,
        selector,
        selector_error,
        recognition: Recognition::NotEvaluated,
    })
}

fn apply_recognition(graph: &SourceGraph, effects: &mut [Effect<'_>]) {
    let mut status = HashMap::new();
    let fades: HashSet<_> = graph
        .sidecars
        .iter()
        .filter(|s| {
            s.adapter == ADAPTER && string(&s.data, "fade_shape") == Some("linear_amplitude")
        })
        .map(|s| s.object.as_str())
        .collect();
    for sidecar in &graph.sidecars {
        if sidecar.adapter != dissolve::ADAPTER {
            continue;
        }
        if let Value::Arr(rows) = &sidecar.data {
            for row in rows {
                if let Some(id) = string(row, "id") {
                    let recognition =
                        if string(row, "interpretation") == Some(dissolve::INTERPRETATION) {
                            Recognition::Recognized
                        } else {
                            Recognition::Unqualified
                        };
                    status.insert((sidecar.object.as_str(), id), recognition);
                }
            }
        }
    }
    for sequence in &graph.sequences {
        for p in &sequence.placements {
            let recognition = if p.transform.is_some()
                || !p.effects.is_empty()
                || p.volume_db.is_some()
                || !p.volume_keys.is_empty()
                || fades.contains(p.id.as_str())
            {
                Recognition::Recognized
            } else {
                Recognition::Unqualified
            };
            status.insert((sequence.id.as_str(), p.id.as_str()), recognition);
        }
    }
    for loss in &graph.losses {
        if loss.aspect == "effect" {
            if let Some((id, _)) = loss.reason.split_once(": ") {
                status.insert((loss.object.as_str(), id), Recognition::Unqualified);
            }
        }
    }
    for effect in effects {
        if effect.selector.is_none() {
            continue;
        }
        let (Some(sequence), Some(id)) = (effect.sequence_id, effect.object_id) else {
            continue;
        };
        if let Some(recognition) = status.get(&(sequence, id)) {
            effect.recognition = *recognition;
        }
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value.get(key)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}
