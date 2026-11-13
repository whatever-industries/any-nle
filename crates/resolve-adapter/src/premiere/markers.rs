//! Premiere sequence marker facts and bounded receipts.
use crate::Result;
use graph::{
    source::{SourceBinding, SourceMarker, SourceSequence},
    Label, Loss, MarkerOwner, Rational, Sidecar, Value,
};
use prproj::{
    build::{marker::packed_to_index, time::TICKS_PER_SECOND},
    xml::{NodeId, Tree},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_INLINE: usize = 4096;
const COLORS: [&str; 8] = [
    "Green", "Red", "Magenta", "Orange", "Yellow", "White", "Blue", "Cyan",
];

pub(super) fn capture(
    tree: &Tree,
    nodes: &[NodeId],
    sequence: &mut SourceSequence,
    sidecars: &mut Vec<Sidecar>,
    losses: &mut Vec<Loss>,
) {
    if nodes.is_empty() {
        return;
    }
    let mut receipts = Vec::with_capacity(nodes.len());
    for (ordinal, &node) in nodes.iter().enumerate() {
        let occurrence = format!("{}/marker/{}", sequence.id, ordinal + 1);
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
            (Some(payload), false) => decode(payload, sequence.rate, &occurrence),
        };
        match decoded {
            Ok(marker) => {
                if let Some((color, target)) = marker.color.as_ref().and_then(|color| {
                    crate::marker_palette::premiere_substitute(&color.name)
                        .map(|target| (color, target))
                }) {
                    losses.push(Loss {
                        object: sequence.id.clone(),
                        aspect: "premiere.marker".into(),
                        reason: format!(
                            "Premiere sequence marker {occurrence}: Resolve uses {target} for {} if the marker is written",
                            color.name
                        ),
                    });
                }
                sequence.markers.push(marker);
            }
            Err(reason) => losses.push(Loss {
                object: sequence.id.clone(),
                aspect: "premiere.marker".into(),
                reason: format!("Premiere sequence marker {occurrence} omitted: {reason}"),
            }),
        }
        receipts.push(Value::Obj(fields));
    }
    sidecars.push(Sidecar {
        adapter: "premiere.markers".into(),
        object: sequence.id.clone(),
        data: Value::Arr(receipts),
    });
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

fn decode(payload: &str, rate: Rational, occurrence: &str) -> Result<SourceMarker> {
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
        owner_rate: Some(rate),
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
    fn comment_marker_clock_is_relative_and_exact() {
        let value = json!({"DVAMarker":{
            "mMarkerID":"point","mType":"Comment","mName":"café",
            "mStartTime":{"ticks":10_594_584_000i64}
        }});
        let rate = crate::prepare::rate();
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
