//! Sequence-owned markers in the 21.2 project lockable blob map.
use super::*;
use crate::clip_label::{self, ClipLabel};
use encoding::{fields, Wire};
use graph::{
    source::{SourceBinding, SourceMarker},
    Label, MarkerOwner,
};

pub(super) fn read(project: &Node, graph: &mut SourceGraph, blobs: &mut Blobs) -> Result<()> {
    let Some(map) = project.optional("LockableBlobMap")? else {
        return Ok(());
    };
    let Some(node) = map.optional("Sm2LockableBlobMap")? else {
        return Ok(());
    };
    let Some(set) = node.optional("LocableBlobSet")? else {
        return Ok(());
    };
    let by_id: HashMap<_, _> = graph
        .sequences
        .iter()
        .enumerate()
        .map(|(i, sequence)| (sequence.id.clone(), i))
        .collect();
    let placements: HashSet<_> = graph.sequences.iter().flat_map(|s| {
        s.placements.iter().map(|placement| placement.id.clone())
    }).collect();
    let mut seen = HashSet::new();
    let mut colors = HashSet::new();
    for entry in set.elements()? {
        let node = entry.entry()?;
        if node.name == "Sm2TiItemLockableBlob" {
            let owner = node.get("BlobOwner")?;
            if placements.contains(owner) {
                if let Some(code) = clip_color(node, blobs)? {
                    if !colors.insert(owner) {
                        return Err("duplicate Resolve clip label owner".into());
                    }
                    graph.sidecars.push(Sidecar {
                        adapter: clip_label::ADAPTER.into(),
                        object: owner.into(),
                        data: Value::Obj(vec![
                            ("saved_code".into(), Value::Int(code)),
                            ("provenance".into(), text("drpkit:af3cedc0 and pr2resolve:61c8c9ec")),
                        ]),
                    });
                    loss(graph, owner, "clip_label", clip_label::REASON);
                }
            }
            continue;
        }
        if node.name != "Sm2SequenceLockableBlob" {
            continue;
        }
        let owner = node.get("BlobOwner")?;
        if !seen.insert(owner) {
            return Err("duplicate Resolve sequence marker owner".into());
        }
        let Some(index) = by_id.get(owner).copied() else {
            let object = graph
                .bins
                .first()
                .ok_or("missing Resolve marker project owner")?
                .id
                .clone();
            omitted(
                graph,
                &object,
                owner,
                "marker",
                "unbound Resolve sequence metadata",
            );
            continue;
        };
        let encoded = blobs.qt(node.get("FieldsBlob")?)?;
        let body = blobs.marker_data(encoded.bytes("BlobData")?)?;
        let Some(marks) = pb_bytes(&body, 2)? else {
            continue;
        };
        let sequence = &mut graph.sequences[index];
        let mut decoded = BTreeMap::new();
        for (_, value) in fields(marks)?.into_iter().filter(|(key, _)| *key == 2) {
            let Wire::Bytes(value) = value else {
                return Err("invalid Resolve marker entry".into());
            };
            let (frame, marker) = decode(value, sequence.rate, owner)?;
            if decoded.insert(frame, marker).is_some() {
                return Err("duplicate Resolve marker frame".into());
            }
        }
        let mut native = Vec::new();
        let mut unknown = Vec::new();
        for (frame, (marker, color_bit, custom)) in decoded {
            if marker.color.is_none() && color_bit != 0 {
                unknown.push(frame);
            }
            native.push(Value::Obj(vec![
                ("frame".into(), Value::Int(frame)),
                ("color_bit".into(), Value::Int(color_bit)),
                ("custom_data".into(), text(custom)),
            ]));
            sequence.markers.push(marker);
        }
        let info = graph
            .sidecars
            .iter_mut()
            .find(|s| s.adapter == ADAPTER && s.object == owner)
            .ok_or("missing Resolve sequence receipt")?;
        let Value::Obj(entries) = &mut info.data else {
            return Err("invalid Resolve sequence receipt".into());
        };
        entries.push(("markers".into(), Value::Arr(native)));
        for frame in unknown {
            loss(
                graph,
                owner,
                "marker_color",
                &format!("frame {frame}: unqualified Resolve marker color"),
            );
        }
    }
    Ok(())
}

fn clip_color(node: &Node, blobs: &mut Blobs) -> Result<Option<i64>> {
    let encoded = node.get("FieldsBlob")?;
    let Ok(map) = blobs.qt(encoded) else { return Ok(None) };
    let Ok(data) = map.bytes("BlobData") else { return Ok(None) };
    // Only this exact record is identified as a clip label. Other lockable
    // item state can use a different encoding or a different protobuf.
    Ok(ClipLabel::observed_code(data))
}

fn decode<'a>(
    entry: &'a [u8],
    rate: Rational,
    owner: &str,
) -> Result<(i64, (SourceMarker, i64, &'a str))> {
    let key = pb_bytes(entry, 1)?.ok_or("missing Resolve marker frame key")?;
    let count = match fields(key)?.as_slice() {
        [] => 0,
        [(1, Wire::Int(value))] if value % 2 == 0 => i64::try_from(value / 2)?,
        _ => return Err("unsupported Resolve marker frame key".into()),
    };
    let wrap = pb_bytes(entry, 2)?.ok_or("missing Resolve marker value")?;
    if wrap.len() < 8
        || wrap[..4] != 2u32.to_be_bytes()
        || u32::from_be_bytes(wrap[4..8].try_into()?) as usize != wrap.len() - 8
    {
        return Err("malformed Resolve marker value header".into());
    }
    let body = pb_bytes(&wrap[8..], 1)?.ok_or("missing Resolve marker fields")?;
    let fields = fields(body)?;
    let color = match fields
        .iter()
        .filter(|(k, _)| *k == 1)
        .map(|(_, value)| *value)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => 0,
        [Wire::Int(n)] => i64::try_from(*n)?,
        _ => return Err("ambiguous Resolve marker color".into()),
    };
    let strings = fields
        .iter()
        .filter(|(k, _)| *k == 3)
        .map(|(_, value)| {
            if let Wire::Bytes(value) = value {
                return Ok(std::str::from_utf8(value)?);
            }
            Err("invalid Resolve marker text field".into())
        })
        .collect::<Result<Vec<_>>>()?;
    let [note, length, name] = strings.as_slice() else {
        return Err("unqualified Resolve marker text layout".into());
    };
    let length = length.parse::<i64>()?;
    if length < 1 || count > 10_000_000 || length > 10_000_000 {
        return Err("Resolve marker extent is outside source profile".into());
    }
    let custom = pb_bytes(body, 6)?
        .map(std::str::from_utf8)
        .transpose()?
        .unwrap_or("");
    let start = seconds(count, rate)?;
    let color_name = u64::try_from(color).ok().and_then(crate::marker_palette::name);
    Ok((
        count,
        (
            SourceMarker {
                name: (*name).into(),
                comment: (*note).into(),
                user: String::new(),
                color: color_name.map(|name| Label {
                    palette: "resolve".into(),
                    name: name.into(),
                }),
                start: Some(start),
                duration: seconds(length, rate)?,
                track: None,
                owner: MarkerOwner::Sequence,
                owner_occurrence: format!("{owner}/marker/{count}"),
                owner_local: Some(start),
                owner_rate: Some(rate),
                binding: SourceBinding {
                    placements: vec![],
                    reason: None,
                },
                source: None,
                inherited: false,
            },
            color,
            custom,
        ),
    ))
}
