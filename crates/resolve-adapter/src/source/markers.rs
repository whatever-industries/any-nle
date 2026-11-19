//! Sequence, Media Pool and timeline edit markers in the 21.2 lockable blob map.
use super::*;
use crate::clip_label::{self, ClipLabel};
use encoding::{fields, Wire};
use graph::{
    source::{SourceBinding, SourceMarker},
    Label, MarkerOwner, TrackRef,
};

pub(crate) const POOL_ADAPTER: &str = "resolve.pool_markers";
pub(crate) const EDIT_ADAPTER: &str = "resolve.edit_markers";
pub(crate) const EDIT_MARKER_LOSS: &str = "resolve.edit_marker_timing";

pub(super) fn read(
    project: &Node,
    graph: &mut SourceGraph,
    masters: &HashMap<&str, Master>,
    blobs: &mut Blobs,
) -> Result<()> {
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
    let placements: HashMap<_, _> = graph
        .sequences
        .iter()
        .enumerate()
        .flat_map(|(sequence, s)| {
            s.placements
                .iter()
                .enumerate()
                .map(move |(edit, placement)| (placement.id.clone(), (sequence, edit)))
        })
        .collect();
    let mut seen = HashSet::new();
    let mut colors = HashSet::new();
    let mut pool_owners = HashSet::new();
    let mut edit_owners = HashSet::new();
    let pool = graph
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.id.clone(), index))
        .collect::<HashMap<_, _>>();
    for entry in set.elements()? {
        let node = entry.entry()?;
        if node.name == "Sm2MpItemLockableBlob" {
            let owner = node.get("BlobOwner")?;
            if let (Some(&index), Some(master)) = (pool.get(owner), masters.get(owner)) {
                if let Some(rows) = read_rows(node, master.rate, owner, blobs)? {
                    if !pool_owners.insert(owner) {
                        return Err("duplicate Resolve source marker owner".into());
                    }
                    if !picture_marker_clock(master) && !wav_marker_clock(master) {
                        for row in rows {
                            omitted(
                                graph,
                                owner,
                                &row.marker.owner_occurrence,
                                "marker",
                                "Resolve source marker clock is outside the qualified profile",
                            );
                        }
                        continue;
                    }
                    let (markers, data) = marker_facts(graph, owner, rows);
                    graph.items[index].markers = markers;
                    graph.sidecars.push(Sidecar {
                        adapter: POOL_ADAPTER.into(),
                        object: owner.into(),
                        data,
                    });
                }
            }
            continue;
        }
        if node.name == "Sm2TiItemLockableBlob" {
            let owner = node.get("BlobOwner")?;
            if let Some(&(sequence, edit)) = placements.get(owner) {
                let Ok(map) = blobs.qt(node.get("FieldsBlob")?) else {
                    continue;
                };
                let Ok(data) = map.bytes("BlobData") else {
                    continue;
                };
                let mut code = ClipLabel::observed_code(data);
                if code.is_none() && data.starts_with(&10001u32.to_be_bytes()) {
                    let body = blobs.marker_data(data)?;
                    let rate = graph.sequences[sequence].rate;
                    if let Some(rows) = decode_rows(&body, rate, owner)? {
                        if !edit_owners.insert(owner) {
                            return Err("duplicate Resolve edit marker owner".into());
                        }
                        code = combined_clip_color(&body)?;
                        read_edit_rows(graph, sequence, edit, masters, owner, rows)?;
                    }
                }
                if let Some(code) = code {
                    if !colors.insert(owner) {
                        return Err("duplicate Resolve clip label owner".into());
                    }
                    graph.sidecars.push(Sidecar {
                        adapter: clip_label::ADAPTER.into(),
                        object: owner.into(),
                        data: Value::Obj(vec![
                            ("saved_code".into(), Value::Int(code)),
                            (
                                "provenance".into(),
                                text("drpkit:af3cedc0 and pr2resolve:61c8c9ec"),
                            ),
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
        let rate = graph.sequences[index].rate;
        let Some(rows) = read_rows(node, rate, owner, blobs)? else {
            continue;
        };
        let (markers, data) = marker_facts(graph, owner, rows);
        graph.sequences[index].markers.extend(markers);
        let info = graph
            .sidecars
            .iter_mut()
            .find(|s| s.adapter == ADAPTER && s.object == owner)
            .ok_or("missing Resolve sequence receipt")?;
        let Value::Obj(entries) = &mut info.data else {
            return Err("invalid Resolve sequence receipt".into());
        };
        entries.push(("markers".into(), data));
    }
    Ok(())
}

fn combined_clip_color(body: &[u8]) -> Result<Option<i64>> {
    let mut codes = fields(body)?.into_iter().filter(|(field, _)| *field == 5);
    match (codes.next(), codes.next()) {
        (None, None) => Ok(None),
        (Some((_, Wire::Int(code))), None) if code <= u8::MAX.into() => Ok(Some(code as i64)),
        _ => Err("ambiguous Resolve clip label in edit marker blob".into()),
    }
}

pub(crate) fn supported_marker_rate(rate: Rational) -> bool {
    matches!((rate.num(), rate.den()), (24, 1) | (24000, 1001))
}

fn picture_marker_clock(master: &Master) -> bool {
    supported_marker_rate(master.rate)
        && master.prores_hq
        && master.audio.is_empty()
        && master.size == (1920, 1080)
}

fn wav_marker_clock(master: &Master) -> bool {
    supported_marker_rate(master.rate)
        && master.wav_samples.is_some()
        && !master.standalone_mono_atom
        && master.start == 0
}

fn read_edit_rows(
    graph: &mut SourceGraph,
    sequence: usize,
    edit: usize,
    masters: &HashMap<&str, Master>,
    owner: &str,
    mut rows: Vec<MarkerRow>,
) -> Result<()> {
    let timeline = &graph.sequences[sequence];
    let placement = &timeline.placements[edit];
    let qualified = supported_marker_rate(timeline.rate)
        && placement.video_track == Some(1)
        && placement.audio_tracks.is_empty()
        && placement.speed.is_none()
        && placement.frame_map.is_none()
        && masters
            .get(placement.item.as_str())
            .is_some_and(|master| master.rate == timeline.rate && picture_marker_clock(master));
    let mut unresolved = Vec::new();
    for row in &mut rows {
        let marker = &mut row.marker;
        let source = seconds(row.frame, timeline.rate)?;
        let transform = qualified
            .then_some(source)
            .and_then(|source| source.checked_sub(placement.source_in))
            .filter(|elapsed| !elapsed.is_negative())
            .filter(|elapsed| {
                elapsed
                    .checked_add(marker.duration)
                    .is_some_and(|end| end <= placement.duration)
            })
            .and_then(|elapsed| {
                placement
                    .record_start
                    .checked_add(elapsed)
                    .map(|record| (elapsed, record))
            });
        marker.owner = MarkerOwner::Edit;
        marker.track = placement.video_track.map(|index| TrackRef {
            kind: StreamKind::Video,
            index,
        });
        marker.binding.placements.push(owner.into());
        if let Some((elapsed, record)) = transform {
            marker.start = Some(record);
            marker.owner_local = Some(elapsed);
            marker.source = Some(source);
        } else {
            let reason = if qualified {
                "Resolve edit marker is outside its visible trim"
            } else {
                "Resolve edit marker clock is outside the qualified picture profile"
            };
            marker.start = None;
            marker.owner_local = None;
            marker.owner_rate = None;
            marker.binding.reason = Some(reason.into());
            unresolved.push((marker.owner_occurrence.clone(), reason));
        }
    }
    for (occurrence, reason) in unresolved {
        omitted(graph, owner, &occurrence, EDIT_MARKER_LOSS, reason);
    }
    let (mut markers, mut data) = marker_facts(graph, owner, rows);
    if let Value::Arr(records) = &mut data {
        for (record, marker) in records.iter_mut().zip(&mut markers) {
            if let Value::Obj(fields) = record {
                let duration =
                    crate::prepare::frames_on(marker.duration, graph.sequences[sequence].rate)?;
                fields.push(("duration_frames".into(), Value::Int(duration)));
                if marker.binding.reason.is_some() {
                    marker.duration = Rational::ZERO;
                }
            }
        }
    }
    graph.sequences[sequence].markers.extend(markers);
    graph.sidecars.push(Sidecar {
        adapter: EDIT_ADAPTER.into(),
        object: owner.into(),
        data,
    });
    Ok(())
}

struct MarkerRow {
    frame: i64,
    marker: SourceMarker,
    color_bit: i64,
    custom: String,
}

fn read_rows(
    node: &Node,
    rate: Rational,
    owner: &str,
    blobs: &mut Blobs,
) -> Result<Option<Vec<MarkerRow>>> {
    let encoded = blobs.qt(node.get("FieldsBlob")?)?;
    let body = blobs.marker_data(encoded.bytes("BlobData")?)?;
    decode_rows(&body, rate, owner)
}

fn decode_rows(body: &[u8], rate: Rational, owner: &str) -> Result<Option<Vec<MarkerRow>>> {
    let Some(marks) = pb_bytes(body, 2)? else {
        return Ok(None);
    };
    let mut decoded = BTreeMap::new();
    for (_, value) in fields(marks)?.into_iter().filter(|(key, _)| *key == 2) {
        let Wire::Bytes(value) = value else {
            return Err("invalid Resolve marker entry".into());
        };
        let (frame, (marker, color_bit, custom)) = decode(value, rate, owner)?;
        if decoded
            .insert(
                frame,
                MarkerRow {
                    frame,
                    marker,
                    color_bit,
                    custom: custom.into(),
                },
            )
            .is_some()
        {
            return Err("duplicate Resolve marker frame".into());
        }
    }
    Ok(Some(decoded.into_values().collect()))
}

fn marker_facts(
    graph: &mut SourceGraph,
    owner: &str,
    rows: Vec<MarkerRow>,
) -> (Vec<SourceMarker>, Value) {
    let mut markers = Vec::with_capacity(rows.len());
    let mut native = Vec::with_capacity(rows.len());
    for MarkerRow {
        frame,
        marker,
        color_bit,
        custom,
    } in rows
    {
        if marker.color.is_none() && color_bit != 0 {
            loss(
                graph,
                owner,
                "marker_color",
                &format!("frame {frame}: unqualified Resolve marker color"),
            );
        }
        native.push(Value::Obj(vec![
            ("frame".into(), Value::Int(frame)),
            ("color_bit".into(), Value::Int(color_bit)),
            ("custom_data".into(), Value::Str(custom)),
        ]));
        markers.push(marker);
    }
    (markers, Value::Arr(native))
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
    let color_name = u64::try_from(color)
        .ok()
        .and_then(crate::marker_palette::name);
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
