//! Point markers on generated Avid compositions.
use super::{EditRate, Result};
use avid_reader::{
    document_attributes_extra::{AttributeValue, DocumentAttributesExt},
    io::MobId,
    objects::{Node, Value},
    primitives_extra::new_unique_mob_id,
    AvbDocument,
};
use graph::{source::SourceMarker, MarkerOwner, Rational};
use std::collections::HashSet;

#[derive(Debug)]
pub(super) struct Marker<'a> {
    frame: i64,
    name: &'a str,
    comment: &'a str,
    user: &'a str,
    color: &'static str,
    rgb: [i64; 3],
}

pub(super) fn prepare<'a>(
    markers: &'a [SourceMarker],
    offset: Option<Rational>,
    extent: u32,
    edit_rate: EditRate,
) -> Result<Vec<Marker<'a>>> {
    if markers.len() > i16::MAX as usize {
        return Err("Avid marker list exceeds the native time-crumb count".into());
    }
    let mut result = Vec::with_capacity(markers.len());
    let mut frames = HashSet::with_capacity(markers.len());
    for marker in markers {
        let refusal = |reason: &str| format!("{}: {reason}", marker.owner_occurrence);
        if marker.owner != MarkerOwner::Sequence
            || marker.track.is_some()
            || marker.source.is_some()
            || marker.inherited
            || marker.duration != Rational::ZERO
            || marker.binding.reason.is_some()
            || !marker.binding.placements.is_empty()
            || marker
                .owner_rate
                .is_some_and(|rate| rate != edit_rate.exact())
            || marker.owner_local.is_some() && marker.owner_local != marker.start
        {
            return Err(
                refusal("Avid requires an owner-level point marker on a known clock").into(),
            );
        }
        let frame = marker
            .start
            .zip(offset)
            .and_then(|(start, offset)| start.checked_add(offset))
            .and_then(|time| edit_rate.frames(time))
            .filter(|frame| {
                *frame >= 0 && *frame < i64::from(extent) && *frame <= i64::from(i32::MAX)
            })
            .ok_or_else(|| refusal("Avid marker is outside the owner or off its frame grid"))?;
        let (color, rgb) = match marker
            .color
            .as_ref()
            .map(|color| (color.palette.as_str(), color.name.as_str()))
        {
            Some(("avid" | "premiere" | "resolve", "Green")) => ("Green", [13107, 52428, 13107]),
            Some(("avid" | "premiere" | "resolve", "Red")) => ("Red", [41471, 12134, 6564]),
            Some(("avid" | "premiere" | "resolve", "Blue")) => ("Blue", [13107, 13107, 52428]),
            _ => {
                return Err(refusal(
                    "Avid marker needs a qualified Green, Red or Blue palette entry",
                )
                .into())
            }
        };
        if !frames.insert(frame) {
            return Err(refusal("coincident Avid markers on one owner are unqualified").into());
        }
        result.push(Marker {
            frame,
            name: &marker.name,
            comment: &marker.comment,
            user: &marker.user,
            color,
            rgb,
        });
    }
    result.sort_by_key(|marker| marker.frame);
    Ok(result)
}

fn fields<'a>(marker: &'a Marker<'_>) -> [(&'static str, &'a str); 5] {
    [
        ("_ATN_CRM_COLOR_EXTENDED", marker.color),
        ("_ATN_CRM_COLOR", marker.color),
        ("_ATN_CRM_MARKNAME", marker.name),
        ("_ATN_CRM_COM", marker.comment),
        ("_ATN_CRM_USER", marker.user),
    ]
}

fn rgb(marker: &Marker<'_>) -> Value {
    Value::List(marker.rgb.into_iter().map(Value::Int).collect())
}

/// Allocate the attribute payload, marker and list before creating the owner.
pub(super) fn write(doc: &mut AvbDocument, markers: &[Marker<'_>]) -> Result<Option<u32>> {
    if markers.is_empty() {
        return Ok(None);
    }
    let mut items = Vec::with_capacity(markers.len());
    for marker in markers {
        let id = new_unique_mob_id()?.umid_hex().to_ascii_lowercase();
        let id = format!(
            "{}-{}-{}-{}-{}",
            &id[..26],
            &id[26..32],
            &id[32..48],
            &id[48..60],
            &id[60..]
        );
        let attributes = doc.add_object(Node {
            class: *b"ATTR",
            type_name: "Attributes",
            fields: vec![(
                "entries",
                Value::Attrs(
                    fields(marker)
                        .into_iter()
                        .map(|(key, value)| (key.into(), Value::Str(value.into())))
                        .chain([("_ATN_CRM_ID".into(), Value::Str(id))])
                        .collect(),
                ),
            )],
        })?;
        let object = doc.add_object(Node {
            class: *b"TMBC",
            type_name: "Marker",
            fields: vec![
                ("position", Value::Int(0)),
                (
                    "mob_id",
                    Value::MobId(MobId::from_pyavb_bytes_le(&[0; 32])?),
                ),
                ("comp_offset", Value::Int(marker.frame)),
                ("attributes", Value::Ref(attributes)),
                ("color", rgb(marker)),
                ("handled_codes", Value::Bool(true)),
            ],
        })?;
        items.push(Value::Ref(object));
    }
    Ok(Some(doc.add_object(Node {
        class: *b"TMCS",
        type_name: "TimeCrumbList",
        fields: vec![("items", Value::RefList(items))],
    })?))
}

pub(super) fn attach(doc: &mut AvbDocument, attributes: u32, list: Option<u32>) -> Result<()> {
    if let Some(list) = list {
        doc.set_attribute(attributes, "_TMP_CRM", AttributeValue::Object(Some(list)))?;
    }
    Ok(())
}

pub(super) fn check(doc: &AvbDocument, owner: u32, expected: &[Marker<'_>]) -> Result<()> {
    let bad = || "generated Avid marker attachment, order or value disagrees";
    let id = |node: &Node, key| node.get(key).and_then(Value::as_index).ok_or_else(bad);
    let attributes = id(doc.node(owner).ok_or_else(bad)?, "attributes")?;
    let attrs = |object| match doc.node(object) {
        Some(node) if node.class == *b"ATTR" => match node.get("entries") {
            Some(Value::Attrs(entries)) => Ok(entries.as_slice()),
            _ => Err(bad()),
        },
        _ => Err(bad()),
    };
    let mut attachments = attrs(attributes)?
        .iter()
        .filter(|(key, _)| key == "_TMP_CRM");
    let Some((_, list)) = attachments.next() else {
        return if expected.is_empty() {
            Ok(())
        } else {
            Err(bad().into())
        };
    };
    let list = list.as_index().ok_or_else(bad)?;
    if attachments.next().is_some()
        || !(list < attributes && attributes < owner && owner < doc.root_id())
        || doc.root_id() as usize != doc.object_count()
    {
        return Err(bad().into());
    }
    let node = doc
        .node(list)
        .filter(|node| node.class == *b"TMCS")
        .ok_or_else(bad)?;
    let Some(Value::RefList(items)) = node.get("items") else {
        return Err(bad().into());
    };
    if items.len() != expected.len() {
        return Err(bad().into());
    }
    let null_mob = Value::MobId(MobId::from_pyavb_bytes_le(&[0; 32])?);
    for (entry, marker) in items.iter().zip(expected) {
        let entry = entry.as_index().filter(|id| *id < list).ok_or_else(bad)?;
        let node = doc
            .node(entry)
            .filter(|node| node.class == *b"TMBC")
            .ok_or_else(bad)?;
        let attr = id(node, "attributes")?;
        let mut entries = attrs(attr)?.iter();
        if attr >= entry
            || node.get("position") != Some(&Value::Int(0))
            || node.get("mob_id") != Some(&null_mob)
            || node.get("handled_codes") != Some(&Value::Bool(true))
            || node.get("comp_offset") != Some(&Value::Int(marker.frame))
            || node.get("color") != Some(&rgb(marker))
            || !fields(marker).into_iter().all(|(k, v)| {
                entries
                    .next()
                    .is_some_and(|(key, value)| key == k && value.as_str() == Some(v))
            })
            || !entries.next().is_some_and(|(k, v)| {
                k == "_ATN_CRM_ID" && v.as_str().is_some_and(|id| id.parse::<MobId>().is_ok())
            })
            || entries.next().is_some()
        {
            return Err(bad().into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use avid_reader::document::MobKind;
    use graph::{source::SourceBinding, Label};

    #[test]
    fn point_marker_requires_dependencies_before_its_owner() -> Result<()> {
        let start = Rational::new(5, 24).unwrap();
        let source = [SourceMarker {
            name: "Point".into(),
            comment: "Check owner offset".into(),
            user: "Editor".into(),
            color: Some(Label {
                palette: "avid".into(),
                name: "Green".into(),
            }),
            start: Some(start),
            duration: Rational::ZERO,
            track: None,
            owner: MarkerOwner::Sequence,
            owner_occurrence: "owner/point".into(),
            owner_local: Some(start),
            owner_rate: Some(Rational::integer(24)),
            binding: SourceBinding {
                placements: vec![],
                reason: None,
            },
            source: None,
            inherited: false,
        }];
        let markers = prepare(&source, Rational::new(1, 24), 10, EditRate::Fps24)?;
        assert_eq!(markers[0].frame, 6);
        let mut doc = AvbDocument::new_empty_bin(1, "2026/01/01 00:00:00", 0, false);
        let list = write(&mut doc, &markers)?;
        let owner =
            doc.create_composition("Marked clip", MobKind::Master, new_unique_mob_id()?, 10, 0)?;
        let attributes = doc
            .node(owner)
            .unwrap()
            .get("attributes")
            .unwrap()
            .as_index()
            .unwrap();
        attach(&mut doc, attributes, list)?;
        doc.add_bin_item(owner)?;
        doc.finish_bin_copy()?;
        let readback = AvbDocument::from_bytes(doc.to_bytes()?)?;
        check(&readback, owner, &markers)?;

        let late_list = write(&mut doc, &markers)?;
        attach(&mut doc, attributes, late_list)?;
        doc.finish_bin_copy()?;
        let readback = AvbDocument::from_bytes(doc.to_bytes()?)?;
        assert!(check(&readback, owner, &markers).is_err());
        Ok(())
    }
}
