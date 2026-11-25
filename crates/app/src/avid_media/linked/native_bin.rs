//! Write timelines over native source objects and verify the saved bin links.
use super::{markers, transition, EditRate, Plan, PreparedEdit, Result, Route, TIMECODE_BASE};
use avid_reader::{
    document::{AvbDocument, MobKind},
    document_attributes_extra::{AttributeValue, DocumentAttributesExt},
    io::MobId,
    objects::{Node, Value},
    primitives_extra::new_unique_mob_id,
};
use std::{
    collections::{HashMap, HashSet},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Link {
    master: MobId,
    file: MobId,
    physical: MobId,
}

pub(super) struct Donor<'a> {
    items: &'a [Value],
    sequence: Option<u32>,
    links: [Link; 3],
}

pub(super) fn node(doc: &AvbDocument, id: u32) -> Result<&Node> {
    doc.node(id)
        .ok_or_else(|| format!("missing or opaque donor object {id}").into())
}

pub(super) fn index(owner: &Node, key: &str) -> Result<u32> {
    owner
        .get(key)
        .and_then(Value::as_index)
        .ok_or_else(|| format!("{}: missing donor {key} reference", owner.class_str()).into())
}

pub(super) fn reference<'a>(doc: &'a AvbDocument, owner: &Node, key: &str) -> Result<&'a Node> {
    node(doc, index(owner, key)?)
}

pub(super) fn number(owner: &Node, key: &str) -> Result<i64> {
    owner
        .get(key)
        .and_then(Value::as_int)
        .ok_or_else(|| format!("{}: missing donor {key}", owner.class_str()).into())
}

pub(super) fn mob_id(owner: &Node) -> Result<MobId> {
    match owner.get("mob_id") {
        Some(Value::MobId(id)) => Ok(*id),
        _ => Err("missing native MobID".into()),
    }
}

pub(super) fn list<'a>(owner: &'a Node, key: &str) -> Result<&'a [Value]> {
    match owner.get(key) {
        Some(Value::List(values)) => Ok(values),
        _ => Err(format!("missing donor {key} list").into()),
    }
}

pub(super) fn embedded(value: &Value) -> Result<&Node> {
    match value {
        Value::Node(value) => Ok(value),
        _ => Err("expected an embedded donor record".into()),
    }
}

pub(super) fn track<'a>(
    doc: &'a AvbDocument,
    mob: &Node,
    kind: i64,
    lane: i64,
) -> Result<&'a Node> {
    let mut matching = None;
    for track in list(mob, "tracks")? {
        let track = embedded(track)?;
        let component = reference(doc, track, "component")?;
        if number(track, "index")? == lane && number(component, "media_kind_id")? == kind {
            if matching.replace(component).is_some() {
                return Err("ambiguous donor track".into());
            }
            if !matches!(track.get("start_pos"), None | Some(Value::Int(0))) {
                return Err("donor track has an unsupported start position".into());
            }
        }
    }
    matching.ok_or_else(|| format!("missing donor lane {kind}/{lane}").into())
}

fn parts<'a>(doc: &'a AvbDocument, sequence: &'a Node) -> Result<Vec<&'a Node>> {
    if sequence.class != *b"SEQU" {
        return Err("expected an Avid sequence component".into());
    }
    let Some(Value::RefList(parts)) = sequence.get("components") else {
        return Err("missing donor sequence components".into());
    };
    parts
        .iter()
        .map(|value| {
            node(
                doc,
                value
                    .as_index()
                    .ok_or("invalid donor component reference")?,
            )
        })
        .collect()
}

pub(super) fn leaf<'a>(
    doc: &'a AvbDocument,
    component: &'a Node,
    class: [u8; 4],
    rate: &Value,
) -> Result<&'a Node> {
    let segments = if component.class == *b"SEQU" {
        parts(doc, component)?
    } else {
        vec![component]
    };
    let mut match_leaf = None;
    for segment in segments {
        if segment.get("edit_rate") != Some(rate) {
            return Err("donor component has a different edit rate".into());
        }
        if segment.class == class && match_leaf.is_none() {
            match_leaf = Some(segment);
        } else if segment.class != *b"FILL" || number(segment, "length")? != 0 {
            return Err("unexpected component in linked master source route".into());
        }
    }
    match_leaf.ok_or_else(|| "missing saved donor source component".into())
}

pub(super) fn attribute<'a>(doc: &'a AvbDocument, owner: &Node, key: &str) -> Option<&'a Value> {
    let attributes = reference(doc, owner, "attributes").ok()?;
    let Value::Attrs(values) = attributes.get("entries")? else {
        return None;
    };
    let mut matches = values.iter().filter(|(name, _)| name == key);
    let (_, value) = matches.next()?;
    matches.next().is_none().then_some(value)
}

pub(super) fn file_locator(doc: &AvbDocument, descriptor: &Node, expected: &str) -> Result<()> {
    let locator = reference(doc, descriptor, "locator")?;
    if locator.class != *b"FILE"
        || ["path_utf8", "path_posix"]
            .iter()
            .any(|field| locator.get(field).and_then(Value::as_str) != Some(expected))
    {
        return Err(format!("{expected}: donor original locators disagree").into());
    }
    Ok(())
}

fn bind(
    doc: &AvbDocument,
    mobs: &HashMap<MobId, (u32, bool)>,
    master: &Node,
    route: &Route<'_>,
    kind: i64,
    rate: EditRate,
) -> Result<Link> {
    let edit_rate = Value::Float(rate.saved());
    let (class, provider, track_count) = if kind == 1 {
        (*b"CDCI", "87a0584d-cafa-41f5-9a68-f0cadefdbb71", 1)
    } else {
        (*b"PCMA", "3711d3cc-62d0-49d7-b0ae-c118101d1a16", 2)
    };
    let source = |clip: &Node| -> Result<(&Node, MobId)> {
        let id = mob_id(clip)?;
        let (native_id, false) = *mobs.get(&id).ok_or("unregistered donor source MobID")? else {
            return Err("donor file or physical source is registered as an exposed item".into());
        };
        let source = node(doc, native_id)?;
        if number(source, "mob_type_id")? != 3
            || source.get("edit_rate") != Some(&edit_rate)
            || number(source, "usage_code")? != 0
            || number(clip, "track_id")? != route.source_lane
            || number(clip, "start_time")? != 0
            || number(clip, "length")? != i64::from(route.extent)
        {
            return Err("donor source offset, extent or track ID disagrees".into());
        }
        Ok((source, id))
    };
    if number(master, "usage_code")? != 0
        || master.get("edit_rate") != Some(&edit_rate)
        || number(master, "length")? != i64::from(route.extent)
        || attribute(doc, master, "_IMPORTED_AMA") != Some(&Value::Int(1))
        || list(master, "tracks")?.len() != track_count
    {
        return Err("donor master is outside the linked source profile".into());
    }
    if let Some(tape) = route.tape {
        let user = attribute(doc, master, "_USER")
            .and_then(Value::as_index)
            .and_then(|id| doc.node(id));
        let tape_agrees = matches!(user.and_then(|attrs| attrs.get("entries")),
            Some(Value::Attrs(values)) if values.iter().filter(|(name, _)| name == "TapeID")
                .map(|(_, value)| value.as_str()).eq([Some(tape)]));
        if !tape_agrees {
            return Err("donor reel disagrees with the graph item".into());
        }
    }
    let master_track = track(doc, master, kind, route.source_lane)?;
    if kind == 2
        && attribute(doc, master_track, "_SAVED_AAF_CHANNELID")
            != Some(&Value::Int(route.source_lane))
    {
        return Err("donor WAV channel ID disagrees".into());
    }
    let (file, file_id) = source(leaf(doc, master_track, *b"SCLP", &edit_rate)?)?;
    let (physical, physical_id) = source(leaf(
        doc,
        track(doc, file, kind, route.source_lane)?,
        *b"SCLP",
        &edit_rate,
    )?)?;
    let descriptor = reference(doc, file, "descriptor")?;
    let locator = reference(doc, descriptor, "locator")?;
    let embedded = reference(doc, descriptor, "physical_media")?;
    let upstream = reference(doc, physical, "descriptor")?;
    let uuid = descriptor.get("uuid");
    if descriptor.class != class
        || embedded.class != *b"MDES"
        || upstream.class != *b"MDES"
        || locator.class != *b"MSML"
        || mob_id(locator)? != file_id
        || file_id == physical_id
        || !matches!(uuid, Some(Value::Uuid(uuid)) if uuid.to_string() == provider)
        || embedded.get("uuid") != uuid
        || upstream.get("uuid") != uuid
    {
        return Err("donor linked descriptor, provider or file identity disagrees".into());
    }
    file_locator(doc, embedded, route.path)?;
    file_locator(doc, upstream, route.path)?;
    let clock = leaf(doc, track(doc, physical, 3, 1)?, *b"TCCP", &edit_rate)?;
    if number(clock, "start")? != i64::from(route.clock)
        || number(clock, "fps")? != TIMECODE_BASE
        || number(clock, "flags")? != 0
        || number(clock, "length")? < i64::from(route.extent)
    {
        return Err("donor source timecode disagrees".into());
    }
    let format_match = if kind == 1 {
        number(descriptor, "length")? == i64::from(route.extent)
            && number(descriptor, "display_width")? == 1920
            && number(descriptor, "display_height")? == 1080
            && number(descriptor, "frame_layout")? == 0
            && matches!(descriptor.get("aspect_ratio"), Some(Value::List(aspect))
                if matches!(aspect.as_slice(), [Value::Int(16), Value::Int(9)]))
            && matches!(
                descriptor.get("alpha_sampled_width"),
                None | Some(Value::Int(0))
            )
            && descriptor.get("edit_rate") == Some(&edit_rate)
    } else {
        number(descriptor, "channels")? == 2
            && descriptor.get("sample_rate") == Some(&Value::Float(48_000.0))
            && Some(number(descriptor, "quantization_bits")?) == route.bits
            && number(descriptor, "length")? == i64::from(route.extent) * rate.samples_per_frame()
    };
    if !format_match {
        return Err("donor descriptor layout or extent disagrees".into());
    }
    Ok(Link {
        master: mob_id(master)?,
        file: file_id,
        physical: physical_id,
    })
}

pub(super) fn inspect<'a>(
    doc: &'a AvbDocument,
    routes: &[Route<'_>; 3],
    rate: EditRate,
) -> Result<Donor<'a>> {
    let root = doc.root().ok_or("missing donor bin root")?;
    let items = list(root, "items")?;
    if !matches!(&root.class, b"ABIN" | b"BINF") || !matches!(items.len(), 6 | 7) {
        return Err("donor needs two linked masters, four sources and at most one sequence".into());
    }
    let mut sequence = None;
    let mut mobs = HashMap::new();
    let mut masters = Vec::new();
    for item in items {
        let item = embedded(item)?;
        let id = index(item, "mob")?;
        let mob = node(doc, id)?;
        let Some(Value::Bool(placed)) = item.get("user_placed") else {
            return Err("missing donor bin registration state".into());
        };
        if mob.class != *b"CMPO" || mobs.insert(mob_id(mob)?, (id, *placed)).is_some() {
            return Err("duplicate or invalid registered donor mob".into());
        }
        match (number(mob, "mob_type_id")?, *placed) {
            (1, true) if sequence.is_none() => sequence = Some(id),
            (2, true) => masters.push(mob),
            (3, false) => {}
            _ => return Err("unexpected donor bin item".into()),
        }
    }
    let [picture, stereo] = masters.as_slice() else {
        return Err("donor requires exactly two registered master items".into());
    };
    let (picture, stereo) = if list(picture, "tracks")?.len() == 1 {
        (*picture, *stereo)
    } else {
        (*stereo, *picture)
    };
    let video = bind(doc, &mobs, picture, &routes[0], 1, rate)?;
    let left = bind(doc, &mobs, stereo, &routes[1], 2, rate)?;
    let right = bind(doc, &mobs, stereo, &routes[2], 2, rate)?;
    if left != right
        || HashSet::from([
            video.master,
            video.file,
            video.physical,
            left.master,
            left.file,
            left.physical,
        ])
        .len()
            != 6
    {
        return Err("donor originals have conflicting source identities".into());
    }
    Ok(Donor {
        items,
        sequence,
        links: [video, left, right],
    })
}

impl Donor<'_> {
    pub(super) fn prepare_write(&self) -> Writer {
        let items = self
            .items
            .iter()
            .filter_map(|item| match item {
                Value::Node(item_node)
                    if item_node.get("mob").and_then(Value::as_index) == self.sequence =>
                {
                    None
                }
                _ => Some(item.clone()),
            })
            .collect::<Vec<_>>();
        Writer {
            items,
            links: self.links,
        }
    }

    pub(super) fn check(&self, doc: &AvbDocument, plan: &Plan<'_>) -> Result<()> {
        let sequence_id = self
            .sequence
            .ok_or("generated sequence was not registered")?;
        check_timeline(
            doc,
            plan,
            sequence_id,
            &[self.links.map(|link| link.master)],
        )?;
        self.check_picture_markers(doc, plan)
    }

    fn check_picture_markers(&self, doc: &AvbDocument, plan: &Plan<'_>) -> Result<()> {
        if !plan.sources[0].picture_markers.is_empty() {
            let picture_id = self
                .items
                .iter()
                .filter_map(|item| embedded(item).ok()?.get("mob")?.as_index())
                .find(|id| {
                    doc.node(*id).and_then(|node| mob_id(node).ok()) == Some(self.links[0].master)
                })
                .ok_or("generated picture master was not registered")?;
            markers::check(doc, picture_id, &plan.sources[0].picture_markers)?;
        }
        Ok(())
    }
}

pub(super) fn check_timeline(
    doc: &AvbDocument,
    plan: &Plan<'_>,
    sequence_id: u32,
    masters: &[[MobId; 3]],
) -> Result<()> {
    let sequence = node(doc, sequence_id)?;
    let timecode = track(doc, sequence, 3, 1)?;
    let edit_rate = Value::Float(plan.rate.saved());
    if sequence.get("name").and_then(Value::as_str) != Some(plan.name)
        || sequence.get("edit_rate") != Some(&edit_rate)
        || number(sequence, "length")? != i64::from(plan.length)
        || list(sequence, "tracks")?.len() != 4
        || timecode.class != *b"TCCP"
        || number(timecode, "start")? != i64::from(plan.start)
        || number(timecode, "length")? != i64::from(plan.length)
        || number(timecode, "fps")? != TIMECODE_BASE
        || number(timecode, "flags")? != 0
    {
        return Err("generated timeline identity, extent or clock disagrees".into());
    }
    markers::check(doc, sequence_id, &plan.sequence_markers)?;
    for (i, (kind, lane)) in [(1, 1), (2, 1), (2, 2)].into_iter().enumerate() {
        let component = track(doc, sequence, kind, lane)?;
        let parts = parts(doc, component)?;
        let expected = timeline_parts(&plan.edits[i], &plan.transitions[i], plan.length);
        if component.get("edit_rate") != Some(&edit_rate) || parts.len() != expected.len() {
            return Err("generated edit has the wrong component layout".into());
        }
        let mut position = 0i64;
        for (saved, expected) in parts.into_iter().zip(expected) {
            let length = number(saved, "length")?;
            let valid = match expected {
                TimelinePart::Gap(length) => {
                    saved.class == *b"FILL" && number(saved, "length")? == i64::from(length)
                }
                TimelinePart::Clip(PreparedEdit { span, binding }) => {
                    saved.class == *b"SCLP"
                        && length == i64::from(span.length)
                        && position == i64::from(span.record)
                        && number(saved, "start_time")? == i64::from(span.source)
                        && number(saved, "track_id")? == binding.track
                        && mob_id(saved)? == master(masters, binding)?
                }
                TimelinePart::Transition(t) => transition::matches(doc, saved, kind == 1, t),
            };
            if !valid
                || saved.get("edit_rate") != Some(&edit_rate)
                || number(saved, "media_kind_id")? != kind
            {
                return Err(format!("generated {kind}/{lane} edit disagrees").into());
            }
            position += if saved.class == *b"TNFX" {
                -length
            } else {
                length
            };
        }
        if position != i64::from(plan.length) {
            return Err(format!("generated {kind}/{lane} extent disagrees").into());
        }
    }
    Ok(())
}

pub(super) struct Writer {
    items: Vec<Value>,
    links: [Link; 3],
}

impl Writer {
    pub(super) fn apply(self, doc: &mut AvbDocument, plan: &Plan<'_>) -> Result<()> {
        let timestamp = u32::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        let uid = u64::from_be_bytes(new_unique_mob_id()?.material.data4);
        doc.set_field(doc.root_id(), "uid", Value::UInt(uid))?;
        doc.set_field(doc.root_id(), "items", Value::List(self.items))?;
        write_timeline(doc, plan, &[self.links.map(|link| link.master)], timestamp)
    }
}

enum TimelinePart {
    Gap(u32),
    Clip(PreparedEdit),
    Transition(transition::Transition),
}

fn timeline_parts(
    edits: &[PreparedEdit],
    transitions: &[transition::Transition],
    length: u32,
) -> Vec<TimelinePart> {
    let mut parts = Vec::with_capacity(edits.len() * 2 + transitions.len() + 1);
    let mut transitions = transitions.iter().peekable();
    let mut end = 0;
    for (i, &edit) in edits.iter().enumerate() {
        let span = edit.span;
        if i == 0 || span.record > end {
            parts.push(TimelinePart::Gap(span.record - end));
        }
        parts.push(TimelinePart::Clip(edit));
        end = span.record + span.length;
        if let Some(t) = transitions.next_if(|t| t.after_clip == i) {
            parts.push(TimelinePart::Transition(*t));
            end -= t.duration;
        }
    }
    parts.push(TimelinePart::Gap(length - end));
    parts
}

fn master(masters: &[[MobId; 3]], binding: super::EditBinding) -> Result<MobId> {
    masters
        .get(binding.group)
        .and_then(|group| group.get(binding.route))
        .copied()
        .ok_or_else(|| "prepared edit is missing its native source master".into())
}

pub(super) fn write_timeline(
    doc: &mut AvbDocument,
    plan: &Plan<'_>,
    masters: &[[MobId; 3]],
    timestamp: u32,
) -> Result<()> {
    let mut tracks = Vec::new();
    let edit_rate = plan.rate.saved();
    for (i, (kind, lane)) in [(1, 1), (2, 1), (2, 2)].into_iter().enumerate() {
        let components = timeline_parts(&plan.edits[i], &plan.transitions[i], plan.length)
            .into_iter()
            .map(|part| -> Result<_> {
                Ok(match part {
                    TimelinePart::Gap(length) => doc.create_filler(edit_rate, kind, length)?,
                    TimelinePart::Clip(PreparedEdit { span, binding }) => doc.create_source_clip(
                        edit_rate,
                        kind,
                        span.length,
                        master(masters, binding)?,
                        binding.track.try_into()?,
                        span.source.try_into()?,
                    )?,
                    TimelinePart::Transition(t) => transition::write(doc, edit_rate, kind, t)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let segment = if plan.transitions[i].is_empty() {
            doc.create_sequence(edit_rate, kind, &components)?
        } else {
            doc.create_sequence_with_transitions(edit_rate, kind, &components)?
        };
        tracks.push((lane, segment));
    }
    let timecode = doc.create_timecode(edit_rate, plan.length, TIMECODE_BASE as u16, plan.start)?;
    let marker_list = markers::write(doc, &plan.sequence_markers)?;
    let mob = doc.create_composition(
        plan.name,
        MobKind::Composition,
        new_unique_mob_id()?,
        plan.length,
        timestamp,
    )?;
    doc.set_field(mob, "edit_rate", Value::Float(edit_rate))?;
    let attrs = index(node(doc, mob)?, "attributes")?;
    markers::attach(doc, attrs, marker_list)?;
    doc.set_attribute(attrs, "_VERSION", AttributeValue::Integer(2))?;
    doc.set_attribute(attrs, "SEQUERNCE_FORMAT_TYPE", AttributeValue::Integer(10))?;
    doc.set_attribute(
        attrs,
        "SEQUERNCE_FORMAT_STRING",
        AttributeValue::Text(plan.rate.format().into()),
    )?;
    doc.append_track(mob, 1, timecode)?;
    for (lane, segment) in tracks {
        doc.append_track(mob, lane, segment)?;
    }
    doc.add_bin_item(mob)?;
    doc.finish_bin_copy()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avid_media::linked::{EditBinding, Span};

    #[test]
    fn timeline_uses_the_prepared_source_and_track_for_each_edit() {
        let mut doc = AvbDocument::new_empty_bin(1, "linked timeline test", 0, false);
        let mut masters = [[new_unique_mob_id().unwrap(); 3]; 2];
        masters[1] = [new_unique_mob_id().unwrap(); 3];
        let plan = Plan {
            name: "Mixed sources",
            rate: EditRate::Fps23976,
            start: 86_400,
            length: 24,
            sources: Vec::new(),
            edits: std::array::from_fn(|route| {
                [0, 1]
                    .map(|group| PreparedEdit {
                        span: Span {
                            record: group as u32 * 12,
                            source: 5 + group as u32,
                            length: 12,
                        },
                        binding: EditBinding {
                            group,
                            route,
                            track: if route == 0 { 1 } else { 2 - group as i64 },
                        },
                    })
                    .to_vec()
            }),
            transitions: std::array::from_fn(|_| Vec::new()),
            sequence_markers: Vec::new(),
        };
        write_timeline(&mut doc, &plan, &masters, 0).unwrap();
        let decoded = AvbDocument::from_bytes(doc.to_bytes().unwrap()).unwrap();
        let sequence = (1..=decoded.object_count() as u32)
            .find(|&id| decoded.node(id).is_some_and(|node| node.class == *b"CMPO"))
            .unwrap();
        let audio = track(&decoded, node(&decoded, sequence).unwrap(), 2, 1).unwrap();
        let clips: Vec<_> = parts(&decoded, audio)
            .unwrap()
            .into_iter()
            .filter(|node| node.class == *b"SCLP")
            .map(|node| (mob_id(node).unwrap(), number(node, "track_id").unwrap()))
            .collect();
        assert_eq!(clips, [(masters[0][1], 2), (masters[1][1], 1)]);
        check_timeline(&decoded, &plan, sequence, &masters).unwrap();
        masters.swap(0, 1);
        assert!(check_timeline(&decoded, &plan, sequence, &masters).is_err());
    }
}
