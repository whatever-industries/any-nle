//! Premiere sequence and source clip markers with bounded receipts.
use crate::Result;
use graph::{
    source::{SourceBinding, SourceGraph, SourceMarker, SourceSequence},
    Label, Loss, MarkerOwner, Rational, Sidecar, Value,
};
use prproj::{
    build::{marker::packed_to_index, time::TICKS_PER_SECOND},
    convert::markers::{
        MAX_MARKER_CUSTOM_DATA, SEQUENCE_MARKER_COLORS, SEQUENCE_MARKER_DATA, SOURCE_MARKER_COLORS,
        SOURCE_MARKER_DATA,
    },
    model::{marker::marker_guid, ItemId, Project},
    xml::{NodeId, Tree},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

use super::marker_provenance::{decode_custom_data, valid_uuid};

pub(crate) const SEQUENCE_MARKER_DATA_ADAPTER: &str = "premiere.sequence_marker_custom_data";
pub(crate) const SOURCE_MARKER_DATA_ADAPTER: &str = "premiere.source_marker_custom_data";
pub(super) const SOURCE_MARKER_COLORS_ADAPTER: &str = "premiere.source_marker_color_provenance";
const SEQUENCE_MARKER_COLORS_ADAPTER: &str = "premiere.sequence_marker_color_provenance";

const MAX_INLINE: usize = 4096;
const COLORS: [&str; 8] = [
    "Green", "Red", "Magenta", "Orange", "Yellow", "White", "Blue", "Cyan",
];

pub(super) fn capture(
    tree: &Tree,
    sequence_node: NodeId,
    nodes: &[NodeId],
    sequence: &mut SourceSequence,
    graph: &mut SourceGraph,
) -> Result<()> {
    sequence.markers.extend(capture_markers(
        tree,
        nodes,
        &sequence.id,
        Some(sequence.rate),
        Scope::Sequence,
        graph,
    ));
    if let Some(data) = marker_custom_data(
        tree,
        sequence_node,
        nodes,
        &sequence.id,
        &sequence.markers,
        Scope::Sequence,
    )? {
        graph.sidecars.push(Sidecar {
            adapter: SEQUENCE_MARKER_DATA_ADAPTER.into(),
            object: sequence.id.clone(),
            data,
        });
    }
    if let Some(data) = restore_marker_colors(
        tree,
        sequence_node,
        nodes,
        &sequence.id,
        &mut sequence.markers,
        Scope::Sequence,
    )? {
        graph.sidecars.push(Sidecar {
            adapter: SEQUENCE_MARKER_COLORS_ADAPTER.into(),
            object: sequence.id.clone(),
            data,
        });
    }
    Ok(())
}

fn marker_custom_data(
    tree: &Tree,
    element: NodeId,
    nodes: &[NodeId],
    owner: &str,
    markers: &[SourceMarker],
    scope: Scope,
) -> Result<Option<Value>> {
    let field = match scope {
        Scope::Sequence => SEQUENCE_MARKER_DATA,
        Scope::SourceClip => SOURCE_MARKER_DATA,
    };
    let label = scope.label();
    let mut fields = tree
        .findall(element, scope.property_path())?
        .into_iter()
        .flat_map(|bag| tree.iter_tag(bag, field));
    let Some(node) = fields.next() else {
        return Ok(None);
    };
    let invalid = |reason: &str| format!("{owner}: Premiere {label} marker custom data {reason}");
    if fields.next().is_some() {
        return Err(invalid("is duplicated").into());
    }
    let body = tree
        .text(node)
        .filter(|body| body.len() <= 8192)
        .ok_or_else(|| invalid("is empty or exceeds its size limit"))?;
    let fields: Vec<_> = body.split_ascii_whitespace().collect();
    let ["1", uid, entries] = fields.as_slice() else {
        return Err(invalid("has an invalid format").into());
    };
    if Some(*uid) != scope.owner_uid(owner) || !valid_uuid(uid) {
        return Err(invalid(&format!("does not identify this {label}")).into());
    }
    let attached = attached_marker_ordinals(tree, nodes);
    let occurrences: HashSet<_> = markers
        .iter()
        .map(|marker| marker.owner_occurrence.as_str())
        .collect();
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut budget = MAX_MARKER_CUSTOM_DATA;
    for entry in entries.split(',') {
        let (guid, encoded) = entry
            .split_once(':')
            .ok_or_else(|| invalid("has an invalid marker entry"))?;
        if !valid_uuid(guid) || !seen.insert(guid) || rows.len() == 128 {
            return Err(invalid("has invalid, duplicate or too many marker IDs").into());
        }
        let ordinal = attached
            .get(guid)
            .copied()
            .flatten()
            .ok_or_else(|| invalid("references a missing or ambiguous marker"))?;
        let occurrence = format!("{owner}/marker/{ordinal}");
        if !occurrences.contains(occurrence.as_str()) {
            let reason = match scope {
                Scope::Sequence => "references an unsupported sequence marker",
                Scope::SourceClip => "references an unsupported source clip marker",
            };
            return Err(invalid(reason).into());
        }
        let custom = decode_custom_data(encoded, &mut budget).map_err(invalid)?;
        rows.push(Value::Obj(vec![
            ("marker_guid".into(), Value::Str(guid.into())),
            ("source_marker".into(), Value::Str(occurrence)),
            ("custom_data".into(), Value::Str(custom)),
        ]));
    }
    Ok(Some(Value::Arr(rows)))
}

fn attached_marker_ordinals(tree: &Tree, nodes: &[NodeId]) -> HashMap<String, Option<usize>> {
    let mut attached = HashMap::new();
    for (index, &node) in nodes.iter().enumerate() {
        let bounded_payload = tree
            .iter_tag(node, "DVAMarker")
            .next()
            .and_then(|payload| tree.text(payload))
            .is_some_and(|payload| payload.len() <= MAX_INLINE);
        if bounded_payload {
            if let Ok(guid) = marker_guid(tree, node) {
                attached
                    .entry(guid)
                    .and_modify(|ordinal| *ordinal = None)
                    .or_insert(Some(index + 1));
            }
        }
    }
    attached
}

fn restore_marker_colors(
    tree: &Tree,
    element: NodeId,
    nodes: &[NodeId],
    owner: &str,
    markers: &mut [SourceMarker],
    scope: Scope,
) -> Result<Option<Value>> {
    let field = match scope {
        Scope::Sequence => SEQUENCE_MARKER_COLORS,
        Scope::SourceClip => SOURCE_MARKER_COLORS,
    };
    let mut fields = tree
        .findall(element, scope.property_path())?
        .into_iter()
        .flat_map(|bag| tree.iter_tag(bag, field));
    let Some(node) = fields.next() else {
        return Ok(None);
    };
    let label = match scope {
        Scope::Sequence => "sequence",
        Scope::SourceClip => "source",
    };
    let invalid =
        |reason: &str| format!("{owner}: Premiere {label} marker color provenance {reason}");
    if fields.next().is_some() {
        return Err(invalid("is duplicated").into());
    }
    let body = tree
        .text(node)
        .filter(|text| text.len() <= 8192)
        .ok_or_else(|| invalid("is empty or exceeds its size limit"))?;
    let fields: Vec<_> = body.split_ascii_whitespace().collect();
    let ["1", uid, entries] = fields.as_slice() else {
        return Err(invalid("has an invalid format").into());
    };
    if scope.owner_uid(owner) != Some(*uid) || !valid_uuid(uid) {
        let reason = format!("does not identify this {}", scope.label());
        return Err(invalid(&reason).into());
    }
    let attached = attached_marker_ordinals(tree, nodes);
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries.split(',') {
        let fields: Vec<_> = entry.split(':').collect();
        let [guid, original, preview] = fields.as_slice() else {
            return Err(invalid("has an invalid marker entry").into());
        };
        if !valid_uuid(guid) || !seen.insert(*guid) || rows.len() == 128 {
            return Err(invalid("has invalid, duplicate or too many marker IDs").into());
        }
        let ordinal = attached
            .get(*guid)
            .copied()
            .flatten()
            .ok_or_else(|| invalid("references a missing or ambiguous marker"))?;
        let occurrence = format!("{owner}/marker/{ordinal}");
        let marker = markers
            .iter_mut()
            .find(|marker| marker.owner_occurrence == occurrence)
            .ok_or_else(|| match scope {
                Scope::Sequence => invalid("references an unsupported sequence marker"),
                Scope::SourceClip => invalid("references an unsupported source clip marker"),
            })?;
        let color = SavedMarkerColor::read(marker, original, preview).map_err(invalid)?;
        let replacement = color.restored_label();
        let mut fields = vec![
            ("marker_guid".into(), Value::Str((*guid).into())),
            ("source_marker".into(), Value::Str(occurrence)),
        ];
        fields.extend(color.receipt());
        if let Some(replacement) = replacement {
            marker.color = Some(replacement);
        }
        rows.push(Value::Obj(fields));
    }
    Ok(Some(Value::Arr(rows)))
}

pub(super) struct SavedMarkerColor<'a> {
    original: &'static str,
    fallback: &'static str,
    observed: &'a str,
}

impl<'a> SavedMarkerColor<'a> {
    pub(super) fn read(
        marker: &'a SourceMarker,
        original: &str,
        preview: &str,
    ) -> std::result::Result<Self, &'static str> {
        let original = original
            .parse::<u64>()
            .ok()
            .and_then(crate::marker_palette::resolve_only_color)
            .ok_or("has an invalid Resolve-only color")?;
        let fallback = preview
            .parse::<usize>()
            .ok()
            .and_then(|i| COLORS.get(i).copied())
            .ok_or("has an invalid Premiere color index")?;
        let observed = &marker
            .color
            .as_ref()
            .filter(|color| color.palette == "premiere")
            .ok_or("cannot identify the saved Premiere color")?
            .name;
        Ok(Self {
            original,
            fallback,
            observed,
        })
    }

    fn restored_label(&self) -> Option<Label> {
        (self.fallback == self.observed).then(|| Label {
            palette: "resolve".into(),
            name: self.original.into(),
        })
    }

    pub(super) fn receipt(&self) -> [(String, Value); 4] {
        [
            ("resolve_color".into(), Value::Str(self.original.into())),
            ("premiere_fallback".into(), Value::Str(self.fallback.into())),
            ("premiere_observed".into(), Value::Str(self.observed.into())),
            (
                "restored".into(),
                Value::Bool(self.fallback == self.observed),
            ),
        ]
    }
}

pub(super) fn capture_item(
    project: &Project,
    item: ItemId,
    id: &str,
    rate: Option<Rational>,
    graph: &mut SourceGraph,
) -> Result<Vec<SourceMarker>> {
    let markers = project.item_markers(item);
    if markers.len() > super::MAX_EDITS {
        return Err(format!("Premiere source clip {id}: marker count exceeds edit limit").into());
    }
    let tree = &project.doc.tree;
    let mut captured = capture_markers(tree, markers, id, rate, Scope::SourceClip, graph);
    if let Some(data) = marker_custom_data(
        tree,
        project.item(item).element,
        markers,
        id,
        &captured,
        Scope::SourceClip,
    )? {
        graph.sidecars.push(Sidecar {
            adapter: SOURCE_MARKER_DATA_ADAPTER.into(),
            object: id.into(),
            data,
        });
    }
    if let Some(data) = restore_marker_colors(
        tree,
        project.item(item).element,
        markers,
        id,
        &mut captured,
        Scope::SourceClip,
    )? {
        graph.sidecars.push(Sidecar {
            adapter: SOURCE_MARKER_COLORS_ADAPTER.into(),
            object: id.into(),
            data,
        });
    }
    Ok(captured)
}

#[derive(Clone, Copy)]
enum Scope {
    Sequence,
    SourceClip,
}

impl Scope {
    fn property_path(self) -> &'static str {
        match self {
            Self::Sequence => "Node/Properties",
            Self::SourceClip => "ProjectItem/Node/Properties",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Sequence => "sequence",
            Self::SourceClip => "source clip",
        }
    }

    fn owner_uid(self, owner: &str) -> Option<&str> {
        match self {
            Self::Sequence => Some(owner),
            Self::SourceClip => owner.strip_prefix("premiere:item:"),
        }
    }
}

fn capture_markers(
    tree: &Tree,
    nodes: &[NodeId],
    id: &str,
    rate: Option<Rational>,
    scope: Scope,
    graph: &mut SourceGraph,
) -> Vec<SourceMarker> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let (adapter, owner) = match scope {
        Scope::Sequence => ("premiere.markers", "sequence"),
        Scope::SourceClip => ("premiere.clip_markers", "source clip"),
    };
    let mut receipts = Vec::with_capacity(nodes.len());
    let mut markers = Vec::with_capacity(nodes.len());
    for (ordinal, &node) in nodes.iter().enumerate() {
        let occurrence = format!("{id}/marker/{}", ordinal + 1);
        let mut fields = vec![("occurrence".into(), Value::Str(occurrence.clone()))];
        if let Some(id) = tree.get(node, "ObjectID") {
            fields.push(("native_object_id".into(), Value::Str(id.into())));
        }
        let mut children = tree.iter_tag(node, "DVAMarker");
        let payload = children.next().and_then(|child| tree.text(child));
        let duplicate_payload = children.next().is_some();
        if let Some(payload) = payload {
            fields.push(("byte_count".into(), Value::Int(payload.len() as i64)));
            fields.push((
                "sha256".into(),
                Value::Str(hex::encode(Sha256::digest(payload.as_bytes()))),
            ));
            if payload.len() <= MAX_INLINE {
                fields.push(("saved_payload".into(), Value::Str(payload.into())));
            }
        }
        let decoded = match (payload, duplicate_payload) {
            (_, true) => Err("duplicate Premiere marker payload".into()),
            (None, false) => Err("missing Premiere marker payload".into()),
            (Some(payload), false) => decode(payload, rate, &occurrence),
        };
        match decoded {
            Ok(marker) => {
                if let Scope::Sequence = scope {
                    record_color_substitution(&marker, id, &mut graph.losses);
                }
                markers.push(marker);
            }
            Err(reason) => graph.losses.push(Loss {
                object: id.into(),
                aspect: "premiere.marker".into(),
                reason: format!("Premiere {owner} marker {occurrence} omitted: {reason}"),
            }),
        }
        receipts.push(Value::Obj(fields));
    }
    graph.sidecars.push(Sidecar {
        adapter: adapter.into(),
        object: id.into(),
        data: Value::Arr(receipts),
    });
    markers
}

fn record_color_substitution(marker: &SourceMarker, id: &str, losses: &mut Vec<Loss>) {
    if let Some((color, target)) = marker.color.as_ref().and_then(|color| {
        crate::marker_palette::premiere_substitute(&color.name).map(|target| (color, target))
    }) {
        losses.push(Loss {
            object: id.into(),
            aspect: "premiere.marker".into(),
            reason: format!(
                "Premiere sequence marker {}: Resolve uses {target} for {} if the marker is written",
                marker.owner_occurrence, color.name
            ),
        });
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    #[serde(rename = "DVAMarker")]
    marker: SavedMarker,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMarker {
    #[serde(rename = "mMarkerID")]
    _id: String,
    #[serde(rename = "mType")]
    kind: String,
    #[serde(rename = "mName", default)]
    name: String,
    #[serde(rename = "mComment", default)]
    comment: String,
    #[serde(rename = "mStartTime")]
    start: SavedTime,
    #[serde(rename = "mDuration", default)]
    duration: SavedTime,
    #[serde(rename = "mCuePointList", default)]
    cues: Vec<ColorCue>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTime {
    ticks: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColorCue {
    #[serde(rename = "mKey")]
    key: String,
    #[serde(rename = "mValue")]
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedColor {
    color: i64,
}

fn decode(payload: &str, rate: Option<Rational>, occurrence: &str) -> Result<SourceMarker> {
    if payload.len() > MAX_INLINE {
        return Err("Premiere marker payload exceeds the interpretation limit".into());
    }
    let SavedMarker {
        kind,
        name,
        comment,
        start,
        duration,
        cues,
        ..
    } = serde_json::from_str::<Payload>(payload)
        .map_err(|_| "unqualified Premiere marker payload fields")?
        .marker;
    if kind != "Comment" {
        return Err("only Premiere Comment markers map to Resolve".into());
    }
    if start.ticks < 0 || duration.ticks < 0 || start.ticks.checked_add(duration.ticks).is_none() {
        return Err("invalid Premiere marker span".into());
    }
    let seconds = |ticks: i64| {
        Rational::new(ticks.into(), TICKS_PER_SECOND.into()).ok_or("invalid Premiere marker clock")
    };
    let start = seconds(start.ticks)?;
    let color = match cues.as_slice() {
        [] => 0,
        [cue] if cue.key.starts_with("keywordExtDVAv1_") => {
            let packed: PackedColor =
                serde_json::from_str(&cue.value).map_err(|_| "unqualified Premiere marker cue")?;
            packed_to_index(packed.color).ok_or("unknown Premiere marker color")?
        }
        _ => return Err("unqualified Premiere marker cues".into()),
    };
    Ok(SourceMarker {
        name,
        comment,
        user: String::new(),
        color: Some(Label {
            palette: "premiere".into(),
            name: COLORS[color].into(),
        }),
        start: Some(start),
        duration: seconds(duration.ticks)?,
        track: None,
        owner: MarkerOwner::Sequence,
        owner_occurrence: occurrence.into(),
        owner_local: Some(start),
        owner_rate: rate,
        binding: SourceBinding {
            placements: vec![],
            reason: None,
        },
        source: None,
        inherited: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marker_custom_data_binds_to_guid_within_each_owner() {
        let uid = "00000000-0000-4000-8000-000000000001";
        let first = "00000000-0000-4000-8000-000000000002";
        let second = "00000000-0000-4000-8000-000000000003";
        for (scope, tag, owner) in [
            (Scope::Sequence, SEQUENCE_MARKER_DATA, uid.to_owned()),
            (
                Scope::SourceClip,
                SOURCE_MARKER_DATA,
                format!("premiere:item:{uid}"),
            ),
        ] {
            let mut tree = Tree::new();
            let element = tree.new_element("ClipProjectItem");
            let owner_node = match scope {
                Scope::Sequence => element,
                Scope::SourceClip => tree.sub_element(element, "ProjectItem", &[]),
            };
            let node = tree.sub_element(owner_node, "Node", &[]);
            let bag = tree.sub_element(node, "Properties", &[]);
            let property = tree.sub_element(bag, tag, &[]);
            let body = format!("1 {uid} {second}:c3a9,{first}:41");
            tree.set_text(property, Some(&body));
            let mut nodes = Vec::new();
            let mut markers = Vec::new();
            for (index, guid) in [first, second].into_iter().enumerate() {
                let marker = tree.new_element("Marker");
                let field = tree.sub_element(marker, "DVAMarker", &[]);
                let payload = json!({"DVAMarker":{
                    "mMarkerID":guid,"mType":"Comment",
                    "mStartTime":{"ticks":index as i64 * (TICKS_PER_SECOND / 24)}
                }})
                .to_string();
                tree.set_text(field, Some(&payload));
                nodes.push(marker);
                markers.push(
                    decode(
                        &payload,
                        Some(Rational::integer(24)),
                        &format!("{owner}/marker/{}", index + 1),
                    )
                    .unwrap(),
                );
            }
            let Some(Value::Arr(rows)) =
                marker_custom_data(&tree, element, &nodes, &owner, &markers, scope).unwrap()
            else {
                panic!("missing marker receipt")
            };
            assert_eq!(
                rows[0].get("source_marker"),
                Some(&Value::Str(format!("{owner}/marker/2")))
            );
            assert_eq!(rows[0].get("custom_data"), Some(&Value::Str("é".into())));
            assert_eq!(
                rows[1].get("source_marker"),
                Some(&Value::Str(format!("{owner}/marker/1")))
            );
            assert_eq!(rows[1].get("custom_data"), Some(&Value::Str("A".into())));
            assert!(marker_custom_data(
                &tree,
                element,
                &[nodes[0], nodes[1], nodes[1]],
                &owner,
                &markers,
                scope,
            )
            .is_err());
        }
    }

    #[test]
    fn saved_recolor_takes_precedence_over_original_resolve_color() {
        let uid = "00000000-0000-4000-8000-000000000001";
        let guid = "00000000-0000-4000-8000-000000000002";
        for (scope, tag) in [
            (Scope::Sequence, SEQUENCE_MARKER_COLORS),
            (Scope::SourceClip, SOURCE_MARKER_COLORS),
        ] {
            let owner = match scope {
                Scope::Sequence => uid.to_owned(),
                Scope::SourceClip => format!("premiere:item:{uid}"),
            };
            let mut tree = Tree::new();
            let item = tree.new_element("ClipProjectItem");
            let owner_node = match scope {
                Scope::Sequence => item,
                Scope::SourceClip => tree.sub_element(item, "ProjectItem", &[]),
            };
            let node = tree.sub_element(owner_node, "Node", &[]);
            let properties = tree.sub_element(node, "Properties", &[]);
            let property = tree.sub_element(properties, tag, &[]);
            tree.set_text(property, Some(&format!("1 {uid} {guid}:128:0")));
            let marker = tree.new_element("Marker");
            let node = tree.sub_element(marker, "DVAMarker", &[]);
            let payload = json!({"DVAMarker":{
                "mMarkerID":guid,"mType":"Comment","mStartTime":{"ticks":0}
            }})
            .to_string();
            tree.set_text(node, Some(&payload));
            let original = decode(
                &payload,
                Some(Rational::integer(24)),
                &format!("{owner}/marker/1"),
            )
            .unwrap();
            for (saved, expected, restored) in [("Green", "Purple", true), ("Red", "Red", false)] {
                let mut marker_state = [original.clone()];
                marker_state[0].color.as_mut().unwrap().name = saved.into();
                let Some(Value::Arr(rows)) =
                    restore_marker_colors(&tree, item, &[marker], &owner, &mut marker_state, scope)
                        .unwrap()
                else {
                    panic!("missing marker color provenance");
                };
                let color = marker_state[0].color.as_ref().unwrap();
                assert_eq!(color.name, expected);
                assert_eq!(color.palette, if restored { "resolve" } else { "premiere" });
                assert_eq!(rows[0].get("restored"), Some(&Value::Bool(restored)));
            }
        }
    }

    #[test]
    fn comment_marker_clock_is_relative_and_exact() {
        let value = json!({"DVAMarker":{
            "mMarkerID":"point","mType":"Comment","mName":"café",
            "mStartTime":{"ticks":10_594_584_000i64}
        }});
        let rate = Some(crate::prepare::rate());
        let m = decode(&value.to_string(), rate, "sequence/marker/1").unwrap();
        assert_eq!(m.start, Rational::new(1001, 24000));
        assert_eq!(m.owner_local, m.start);
        assert_eq!(m.duration, Rational::ZERO);
        assert_eq!(m.color.unwrap().name, "Green");

        let mut missing = value.clone();
        missing["DVAMarker"]
            .as_object_mut()
            .unwrap()
            .remove("mStartTime");
        assert!(decode(&missing.to_string(), rate, "marker").is_err());
        let duplicate = value
            .to_string()
            .replace("\"ticks\":10594584000", "\"ticks\":0,\"ticks\":10594584000");
        assert!(decode(&duplicate, rate, "marker").is_err());
    }
}
