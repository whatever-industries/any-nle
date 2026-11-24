//! Bounded, donor-backed AVB round trips for saved Avid controls.
mod clocks;
mod timewarp;

use super::Result;
use crate::{
    output,
    rewrap::source::{stamp, Stamp},
};
use avid_reader::{
    graph::direct_refs,
    io::MobId,
    objects::{Node, Value},
    source::Rate,
    AvbDocument,
};
use graph::{source::SourceGraph, MarkerOwner, Rational, StreamKind, Value as Json};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    graph: PathBuf,
    donor: PathBuf,
    output: PathBuf,
    owners: Vec<String>,
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(format!("{} must be a regular file", path.display()).into());
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds the AVB round-trip input limit", path.display()).into());
    }
    Ok(bytes)
}

struct Donor<'a> {
    doc: &'a AvbDocument,
    mobs: HashMap<MobId, Vec<u32>>,
    references: HashMap<u32, usize>,
}

impl<'a> Donor<'a> {
    fn new(doc: &'a AvbDocument) -> Self {
        let mut mobs: HashMap<MobId, Vec<u32>> = HashMap::new();
        let mut references = HashMap::new();
        for id in 1..=doc.object_count() as u32 {
            if let Some(node) = doc.node(id) {
                if node.class == *b"CMPO" {
                    if let Some(Value::MobId(mob)) = node.get("mob_id") {
                        mobs.entry(*mob).or_default().push(id);
                    }
                }
                for target in direct_refs(node) {
                    *references.entry(target).or_default() += 1;
                }
            }
        }
        Self {
            doc,
            mobs,
            references,
        }
    }

    fn mob(&self, key: &str) -> Result<u32> {
        match self.mobs.get(&key.parse::<MobId>()?).map(Vec::as_slice) {
            Some([id]) => Ok(*id),
            _ => Err(format!("missing or ambiguous donor MobID {key}").into()),
        }
    }

    fn node(&self, id: u32) -> Result<&'a Node> {
        self.doc
            .node(id)
            .ok_or_else(|| format!("missing or opaque donor object {id}").into())
    }

    fn unshared(&self, id: u32) -> Result<()> {
        if self.references.get(&id) != Some(&1) {
            return Err(format!("donor object {id} is shared or unowned").into());
        }
        Ok(())
    }

    fn track(&self, mob: u32, kind: i64, label: i64) -> Result<u32> {
        let mut candidates = Vec::new();
        for track in tracks(self.node(mob)?)? {
            if track.get("index").and_then(Value::as_int) != Some(label) {
                continue;
            }
            let component = reference(track, "component")?;
            if integer(self.node(component)?, "media_kind_id")? == kind {
                if !matches!(track.get("start_pos"), None | Some(Value::Int(0))) {
                    return Err("donor track has a nonzero start position".into());
                }
                candidates.push(component);
            }
        }
        match candidates.as_slice() {
            [id] => Ok(*id),
            _ => Err(format!("missing or ambiguous donor lane {kind}/{label}").into()),
        }
    }

    fn segment(&self, track: u32, start: i64, length: i64, class: [u8; 4]) -> Result<u32> {
        let node = self.node(track)?;
        let mut at = 0_i64;
        let mut found = None;
        let mut inspect = |id| -> Result<()> {
            let node = self.node(id)?;
            let span = integer(node, "length")?;
            if span < 0 || node.class == *b"TNFX" {
                return Err("donor lane has unsupported transition timing".into());
            }
            if node.class == class && at == start && span == length {
                if found.replace(id).is_some() {
                    return Err("ambiguous donor segment".into());
                }
            }
            at = at.checked_add(span).ok_or("donor segment clock overflow")?;
            Ok(())
        };
        if node.class == *b"SEQU" {
            for component in refs(node, "components")? {
                inspect(component.as_index().ok_or("missing donor segment")?)?;
            }
        } else {
            inspect(track)?;
        }
        found.ok_or_else(|| {
            format!("donor lacks the exact saved interval [{start}, +{length})").into()
        })
    }
}

fn tracks(node: &Node) -> Result<Vec<&Node>> {
    let Some(Value::List(tracks)) = node.get("tracks") else {
        return Err("donor object has no decoded tracks".into());
    };
    tracks
        .iter()
        .map(|value| match value {
            Value::Node(track) => Ok(track.as_ref()),
            _ => Err("malformed donor track".into()),
        })
        .collect()
}

fn integer(node: &Node, key: &str) -> Result<i64> {
    node.get(key)
        .and_then(Value::as_int)
        .ok_or_else(|| format!("invalid donor {} field {key}", node.class_str()).into())
}

fn reference(node: &Node, key: &str) -> Result<u32> {
    node.get(key)
        .and_then(Value::as_index)
        .ok_or_else(|| format!("missing donor {} reference {key}", node.class_str()).into())
}

fn refs<'a>(node: &'a Node, key: &str) -> Result<&'a [Value]> {
    match node.get(key) {
        Some(Value::RefList(list)) => Ok(list),
        _ => Err(format!("missing donor {} reference list {key}", node.class_str()).into()),
    }
}

fn plain_clip<'a>(donor: &Donor<'a>, component: u32) -> Result<&'a Node> {
    let node = donor.node(component)?;
    if node.class == *b"SCLP" {
        return Ok(node);
    }
    if node.class != *b"SEQU" {
        return Err("expected a plain donor source clip".into());
    }
    let mut clip = None;
    for item in refs(node, "components")? {
        let node = donor.node(item.as_index().ok_or("missing source clip segment")?)?;
        if node.class == *b"FILL" && integer(node, "length")? == 0 {
            continue;
        }
        if node.class != *b"SCLP" || clip.replace(node).is_some() {
            return Err("expected a single plain donor source clip".into());
        }
    }
    clip.ok_or_else(|| "missing donor source clip".into())
}

fn text<'a>(node: &'a Json, key: &str) -> Result<&'a str> {
    match node.get(key) {
        Some(Json::Str(text)) => Ok(text),
        _ => Err(format!("graph needs saved {key}").into()),
    }
}

fn number(node: &Json, key: &str) -> Result<i64> {
    match node.get(key) {
        Some(Json::Int(number)) => Ok(*number),
        _ => Err(format!("graph needs integer {key}").into()),
    }
}

fn list<'a>(node: &'a Json, key: &str) -> Result<&'a [Json]> {
    match node.get(key) {
        Some(Json::Arr(values)) => Ok(values),
        _ => Err(format!("graph needs saved list {key}").into()),
    }
}

fn only<T>(iter: impl IntoIterator<Item = T>, what: &str) -> Result<T> {
    let mut matches = iter.into_iter();
    let first = matches.next().ok_or_else(|| format!("missing {what}"))?;
    if matches.next().is_some() {
        return Err(format!("ambiguous {what}").into());
    }
    Ok(first)
}

fn rate(node: &Node) -> Result<Rational> {
    let Some(Value::Float(saved)) = node.get("edit_rate") else {
        return Err("missing donor rate".into());
    };
    let rate = Rate::from_avid(*saved).ok_or("unsupported donor rate")?;
    Rational::new(rate.num.into(), rate.den.into()).ok_or_else(|| "invalid donor rate".into())
}

fn frames(time: Rational, rate: Rational) -> Result<i64> {
    let count = time.to_units(rate.num());
    count
        .filter(|count| count % rate.den() == 0)
        .map(|count| count / rate.den())
        .ok_or_else(|| "graph time is off its native frame grid".into())
}

fn check_selector(donor: &Donor<'_>, id: u32, saved: &Json) -> Result<()> {
    let node = donor.node(id)?;
    if node.class != *b"SLCT"
        || integer(node, "media_kind_id")? != number(saved, "kind")?
        || integer(node, "length")? != number(saved, "length")?
        || rate(node)?.to_string() != text(saved, "rate")?
        || integer(node, "selected")? != number(saved, "selected")?
    {
        return Err("donor selector clock, interval, kind or selection disagrees".into());
    }
    let alternatives = list(saved, "alternatives")?;
    let tracks = tracks(node)?;
    if tracks.len() != alternatives.len() || tracks.is_empty() {
        return Err("donor selector alternatives disagree".into());
    }
    for (track, saved) in tracks.into_iter().zip(alternatives) {
        let saved = saved.get("endpoint").unwrap_or(saved);
        let endpoint = donor.node(reference(track, "component")?)?;
        let expected = text(saved, "mob")?.parse::<MobId>()?;
        if endpoint.class != *b"SCLP"
            || text(saved, "class")? != "SCLP"
            || endpoint.get("mob_id") != Some(&Value::MobId(expected))
            || integer(endpoint, "media_kind_id")? != number(saved, "kind")?
            || integer(endpoint, "track_id")? != number(saved, "track")?
            || integer(endpoint, "start_time")? != number(saved, "start")?
            || integer(endpoint, "length")? != number(saved, "length")?
            || rate(endpoint)?.to_string() != text(saved, "rate")?
        {
            return Err("donor selector source references or trims disagree".into());
        }
    }
    Ok(())
}

#[derive(Default)]
struct Changes {
    writes: Vec<(u32, &'static str, Value)>,
    items: HashSet<String>,
    timewarps: Vec<timewarp::Check>,
}

impl Changes {
    fn set(&mut self, donor: &Donor<'_>, id: u32, key: &'static str, value: Value) -> Result<()> {
        donor
            .node(id)?
            .get(key)
            .ok_or_else(|| format!("donor object {id} has no {key}"))?;
        donor.unshared(id)?;
        if self
            .writes
            .iter()
            .any(|(owner, field, _)| *owner == id && *field == key)
        {
            return Err(format!("duplicate write to donor {id}/{key}").into());
        }
        self.writes.push((id, key, value));
        Ok(())
    }
}

fn attributes(node: &Node) -> Result<&[(String, Value)]> {
    match node.get("entries") {
        Some(Value::Attrs(values)) if node.class == *b"ATTR" => Ok(values),
        _ => Err("malformed native marker attributes".into()),
    }
}

fn marker_write(donor: &Donor<'_>, selector: u32, row: &Json, changes: &mut Changes) -> Result<()> {
    let fields = row.get("native").unwrap_or(row);
    let frame = number(fields, "frame")?;
    if frame < 0
        || frame >= integer(donor.node(selector)?, "length")?
        || !matches!(fields.get("length"), Some(Json::Null))
        || !matches!(fields.get("track_attr"), None | Some(Json::Null))
    {
        return Err(
            "only an in-range V1 selector point with no track override can be written".into(),
        );
    }
    let guid = text(fields, "id")?;
    let _: MobId = guid.parse()?;
    donor.unshared(selector)?;
    let attr = reference(donor.node(selector)?, "attributes")?;
    donor.unshared(attr)?;
    let list = only(
        attributes(donor.node(attr)?)?
            .iter()
            .filter(|(key, _)| key == "_TMP_CRM"),
        "native marker list",
    )?
    .1
    .as_index()
    .ok_or("invalid native marker list")?;
    donor.unshared(list)?;
    let marker = only(refs(donor.node(list)?, "items")?, "donor marker slot")?
        .as_index()
        .ok_or("invalid donor marker reference")?;
    let point = donor.node(marker)?;
    if donor.node(list)?.class != *b"TMCS"
        || point.class != *b"TMBC"
        || integer(point, "position")? != 0
        || integer(point, "comp_offset")? != frame
        || !matches!(point.get("mob_id"), Some(Value::MobId(id)) if id.umid_hex() == "0".repeat(64))
    {
        return Err("donor marker slot has a different position or shape".into());
    }
    donor.unshared(marker)?;
    let point_attrs = reference(point, "attributes")?;
    let mut values = attributes(donor.node(point_attrs)?)?.to_vec();
    let color = text(row, "color")?;
    let rgb = match fields.get("rgb16") {
        Some(Json::Arr(rgb)) => rgb
            .iter()
            .map(|value| match value {
                Json::Int(channel) if (0..=65535).contains(channel) => Ok(Value::Int(*channel)),
                _ => Err("invalid graph marker RGB"),
            })
            .collect::<std::result::Result<Vec<_>, _>>()?,
        _ => return Err("missing graph marker RGB".into()),
    };
    if rgb.len() != 3 || color != "Green" {
        return Err("the initial selector-marker profile requires native Green".into());
    }
    for (key, value) in [
        ("_ATN_CRM_MARKNAME", text(row, "name")?),
        ("_ATN_CRM_COM", text(row, "comment")?),
        ("_ATN_CRM_USER", text(row, "user")?),
        ("_ATN_CRM_COLOR", color),
        ("_ATN_CRM_COLOR_EXTENDED", color),
        ("_ATN_CRM_ID", guid),
    ] {
        if let Some(existing) = values.iter_mut().find(|(name, _)| name == key) {
            existing.1 = Value::Str(value.into());
        } else {
            values.push((key.into(), Value::Str(value.into())));
        }
    }
    changes.set(donor, point_attrs, "entries", Value::Attrs(values))?;
    changes.set(donor, marker, "color", Value::List(rgb))
}

fn group_markers(
    graph: &SourceGraph,
    donor: &Donor<'_>,
    id: &str,
    saved: &Json,
    changes: &mut Changes,
) -> Result<()> {
    let group = only(
        graph.multicams.iter().filter(|group| group.id == id),
        "graph Group",
    )?;
    let mob = donor.mob(text(saved, "mob")?)?;
    if integer(donor.node(mob)?, "usage_code")? != 4 {
        return Err("donor composition is not a reusable Group".into());
    }
    let lanes = list(saved, "lanes")?;
    if lanes.len() != group.lanes.len() || group.reason.is_some() {
        return Err("Group output lanes are unresolved".into());
    }
    for lane in lanes {
        let selector = donor.track(mob, number(lane, "kind")?, number(lane, "label")?)?;
        check_selector(donor, selector, lane)?;
    }
    let selector = donor.track(mob, 1, 1)?;
    let marker = only(list(saved, "markers")?, "Group selector marker")?;
    if text(marker, "owner")? != "output_selector"
        || number(marker, "kind")? != 1
        || number(marker, "track")? != 1
        || text(marker, "local_rate")? != rate(donor.node(selector)?)?.to_string()
        || number(
            marker
                .get("local_offset")
                .ok_or("unknown Group marker offset")?,
            "value",
        )? != number(
            marker.get("native").ok_or("missing Group marker facts")?,
            "frame",
        )?
    {
        return Err("Group marker does not belong to the qualified V1 selector clock".into());
    }
    for lane in &group.lanes {
        for alternative in &lane.alternatives {
            changes.items.extend(
                alternative
                    .pieces
                    .iter()
                    .filter_map(|piece| piece.item.clone()),
            );
        }
    }
    marker_write(donor, selector, marker, changes)
}

fn edit_marker(
    graph: &SourceGraph,
    donor: &Donor<'_>,
    owner: &str,
    changes: &mut Changes,
) -> Result<()> {
    let (sequence, marker) = only(
        graph.sequences.iter().flat_map(|sequence| {
            sequence
                .markers
                .iter()
                .filter(move |marker| marker.owner_occurrence == owner)
                .map(move |marker| (sequence, marker))
        }),
        "graph edit marker",
    )?;
    if marker.owner != MarkerOwner::Edit
        || marker.duration != Rational::ZERO
        || marker.inherited
        || !marker
            .track
            .is_some_and(|track| track.kind == StreamKind::Video && track.index == 1)
    {
        return Err("only a saved Group edit V1 point is qualified".into());
    }
    let native = graph
        .sidecar("avid.source", &sequence.id)
        .ok_or("missing native sequence facts")?;
    let usage_id = format!("{owner}/group-use");
    let usage = only(
        list(native, "native_group_uses")?
            .iter()
            .filter(|row| text(row, "id").ok() == Some(&usage_id)),
        "saved Group edit",
    )?;
    let position = number(usage, "position")?;
    let length = number(usage, "length")?;
    let mob = donor.mob(text(native, "mob_id")?)?;
    let track = donor.track(mob, 1, 1)?;
    let selector = donor.segment(track, position, length, *b"SLCT")?;
    check_selector(donor, selector, usage)?;
    for lane in list(native, "native_group_uses")? {
        if number(lane, "position").ok() == Some(position)
            && number(lane, "length").ok() == Some(length)
            && number(lane, "kind").ok() == Some(2)
        {
            let track = donor.track(mob, 2, number(lane, "track")?)?;
            check_selector(
                donor,
                donor.segment(track, position, length, *b"SLCT")?,
                lane,
            )?;
        }
    }
    let local = frames(
        marker.owner_local.ok_or("unknown edit marker position")?,
        sequence.rate,
    )?;
    let start = frames(
        marker.start.ok_or("unknown record marker position")?,
        sequence.rate,
    )?;
    let saved = only(
        list(native, "marker_native")?
            .iter()
            .filter(|row| text(row, "owner").ok() == Some(owner)),
        "saved edit marker fields",
    )?;
    if position.checked_add(local) != Some(start)
        || number(
            saved
                .get("local_offset")
                .ok_or("unknown native marker clock")?,
            "value",
        )? != local
        || text(saved, "local_rate")? != sequence.rate.to_string()
        || text(usage, "rate")? != sequence.rate.to_string()
    {
        return Err("saved Group edit and record marker positions disagree".into());
    }
    for binding in &marker.binding.placements {
        let placement = only(
            sequence
                .placements
                .iter()
                .filter(|item| item.id == *binding),
            "edit marker placement",
        )?;
        changes.items.insert(placement.item.clone());
    }
    let row = Json::Obj(vec![
        ("name".into(), Json::Str(marker.name.clone())),
        ("comment".into(), Json::Str(marker.comment.clone())),
        ("user".into(), Json::Str(marker.user.clone())),
        (
            "color".into(),
            Json::Str(
                marker
                    .color
                    .as_ref()
                    .ok_or("missing marker color")?
                    .name
                    .clone(),
            ),
        ),
        ("native".into(), {
            let Json::Obj(mut fields) = saved.clone() else {
                return Err("invalid marker facts".into());
            };
            fields.retain(|(key, _)| key != "frame");
            fields.push(("frame".into(), Json::Int(local)));
            Json::Obj(fields)
        }),
    ]);
    marker_write(donor, selector, &row, changes)
}

fn verify_media(
    graph: &SourceGraph,
    donor: &Donor<'_>,
    items: &HashSet<String>,
) -> Result<Vec<(PathBuf, Stamp)>> {
    let mut verified = HashMap::new();
    for id in items {
        let item = only(
            graph.items.iter().filter(|item| item.id == *id),
            "referenced graph media item",
        )?;
        if item.media.is_empty() {
            return Err(format!("{} has no verified media bindings", item.name).into());
        }
        let master = donor.mob(
            item.umid
                .as_deref()
                .ok_or("media item has no saved Avid identity")?,
        )?;
        for binding in &item.media {
            let media = only(
                graph.media.iter().filter(|media| media.id == binding.media),
                "physical media record",
            )?;
            let [location] = media.locations.as_slice() else {
                return Err(format!("{} needs an exact saved linked path", item.name).into());
            };
            let path = PathBuf::from(location);
            let native = graph
                .sidecar("avid.source", &media.id)
                .ok_or("missing native media witness")?;
            let package = text(native, "file_package")?;
            donor.mob(package)?;
            let component = donor.track(
                master,
                number(native, "native_kind")?,
                number(native, "native_track")?,
            )?;
            let source = plain_clip(donor, component)?;
            if source.get("mob_id") != Some(&Value::MobId(package.parse()?)) {
                return Err(format!("{} has a different donor file package", item.name).into());
            }
            if let Some((_, previous)) = verified.get(&path) {
                if *previous != package {
                    return Err(format!("{} names different graph packages", path.display()).into());
                }
                continue;
            }
            let state = stamp(&path)?;
            let identity = avid_reader::mxf::read_identity(&path)?;
            if identity.file_packages.as_slice() != [package] {
                return Err(format!(
                    "{} does not contain the expected MXF file package",
                    path.display()
                )
                .into());
            }
            verified.insert(path, (state, package));
        }
    }
    Ok(verified
        .into_iter()
        .map(|(path, (stamp, _))| (path, stamp))
        .collect())
}

pub fn roundtrip_avb(request: &Path, review: bool) -> Result<serde_json::Value> {
    const MAX_GRAPH: u64 = 32 << 20;
    const MAX_DONOR: u64 = 16 << 20;
    let request: Request = serde_json::from_slice(&read(request, 64 << 10)?)?;
    let mut owners = HashSet::new();
    if request.owners.is_empty()
        || request.owners.len() > 64
        || !request
            .owners
            .iter()
            .all(|id| !id.is_empty() && owners.insert(id))
    {
        return Err("select between one and 64 distinct graph owner IDs".into());
    }
    if !request
        .output
        .extension()
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case("avb"))
    {
        return Err("output must have an .avb extension".into());
    }
    output::preflight(&request.output, None)?;
    let graph_bytes = read(&request.graph, MAX_GRAPH)?;
    let graph = SourceGraph::from_json_text(std::str::from_utf8(&graph_bytes)?)?;
    let donor_bytes = read(&request.donor, MAX_DONOR)?;
    let mut doc = AvbDocument::from_bytes(donor_bytes.clone())?;
    let donor = Donor::new(&doc);
    let mut changes = Changes::default();
    let mut rows = Vec::new();
    let mut refusals = Vec::new();
    for owner in &request.owners {
        let mut plan = Changes::default();
        let result = if let Some(saved) = graph.sidecar("avid.group", owner) {
            group_markers(&graph, &donor, owner, saved, &mut plan)
        } else if let Some(saved) = graph.sidecar("avid.aux_clocks", owner) {
            clocks::prepare(&graph, &donor, owner, saved, &mut plan)
        } else if let Some(prepared) = timewarp::prepare(&graph, &donor, owner, &mut plan) {
            prepared
        } else {
            edit_marker(&graph, &donor, owner, &mut plan)
        };
        match result {
            Ok(()) => {
                changes.items.extend(plan.items);
                changes.writes.extend(plan.writes);
                changes.timewarps.extend(plan.timewarps);
                rows.push(json!({"owner": owner, "status": "ready"}));
            }
            Err(error) => {
                refusals.push(format!("{owner}: {error}"));
                rows.push(
                    json!({"owner": owner, "status": "refused", "reason": error.to_string()}),
                );
            }
        }
    }
    if !refusals.is_empty() {
        return if review {
            Ok(json!({"status": "refused", "owners": rows}))
        } else {
            Err(refusals.join("\n").into())
        };
    }
    let media = verify_media(&graph, &donor, &changes.items)?;
    let mut fields = HashSet::new();
    for (id, key, value) in &changes.writes {
        if !fields.insert((*id, *key)) {
            return Err(format!("selected owners collide at donor object {id}/{key}").into());
        }
        if doc.node(*id).and_then(|node| node.get(key)) != Some(value) {
            doc.set_field(*id, key, value.clone())?;
        }
    }
    let bytes = doc.to_bytes()?;
    timewarp::verify(&bytes, &changes.timewarps)?;
    let staged = AvbDocument::from_bytes(bytes)?;
    for (id, key, expected) in &changes.writes {
        if staged.node(*id).and_then(|node| node.get(key)) != Some(expected) {
            return Err(format!("staged donor read-back disagrees at object {id}/{key}").into());
        }
    }
    if read(&request.graph, MAX_GRAPH)? != graph_bytes
        || read(&request.donor, MAX_DONOR)? != donor_bytes
    {
        return Err("graph or donor changed during AVB preparation".into());
    }
    for (path, before) in &media {
        if stamp(path)? != *before {
            return Err(format!("{} changed during AVB preparation", path.display()).into());
        }
    }
    if !review {
        staged.write_new(&request.output)?;
    }
    Ok(
        json!({"status": if review { "ready" } else { "written" }, "owners": rows, "verified_media": media.len()}),
    )
}
