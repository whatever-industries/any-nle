//! Combine verified source sets on the same neutral Premiere timeline.
#[cfg(target_os = "macos")]
mod writer;
#[cfg(target_os = "macos")]
pub(super) use writer::write;

use super::{
    prepare,
    source_profile::{
        is_avc_override, is_avc_picture, is_managed_logging, MediaIndex, Profile, QualifiedRoute,
    },
    EditBinding, EditRate, Plan, PreparedEdit, Result, SourceSet, MANAGED_ATOM_RECEIPT,
};
use graph::source::{SourceGraph, SourceItem, SourceSequence};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SourceKey<'a> {
    AvcAac(&'a str),
    ManagedAtom(&'a str),
}

impl SourceKey<'_> {
    fn profile(self) -> Profile {
        match self {
            Self::AvcAac(_) => Profile::AvcAac,
            Self::ManagedAtom(_) => Profile::ManagedAtom,
        }
    }
}

struct Group<'a> {
    profile: Profile,
    items: Vec<&'a SourceItem>,
    overrides: usize,
    lanes: [Option<BoundLane<'a>>; 3],
}

struct BoundLane<'a> {
    source: QualifiedRoute<'a>,
    selector: Option<i64>,
}

impl<'a> Group<'a> {
    fn finish(self, media: &MediaIndex<'a>, rate: EditRate) -> Result<SourceSet<'a>> {
        let [Some(video), Some(left), Some(right)] = self.lanes else {
            return Err("each mixed-source set needs verified V1, A1 and A2 bindings".into());
        };
        self.profile
            .check_rate_and_overrides(rate, self.overrides, self.items.len())?;
        let item_ids = [&video, &left, &right].map(|lane| lane.source.item.id.as_str());
        self.profile.finish(
            media,
            self.items.into_iter(),
            [video.source.route, left.source.route, right.source.route],
            &item_ids,
            self.overrides,
            video.source.markers,
        )
    }
}

pub(super) fn applies(graph: &SourceGraph) -> bool {
    graph.media.iter().any(is_avc_picture)
        && graph
            .sidecars
            .iter()
            .any(|row| row.adapter == MANAGED_ATOM_RECEIPT)
}

fn locate_group<'a>(media: &MediaIndex<'a>, item: &'a SourceItem) -> Option<SourceKey<'a>> {
    if media.managed.contains_key(item.id.as_str()) && item.media.len() == 1 {
        return item.umid.as_deref().map(SourceKey::ManagedAtom);
    }
    if item.media.len() != 3 {
        return None;
    }
    let mut video = item.media.iter().filter_map(|binding| {
        media
            .media
            .get(binding.media.as_str())
            .copied()
            .filter(|media| is_avc_picture(media))
    });
    let [path] = video.next()?.locations.as_slice() else {
        return None;
    };
    video.next().is_none().then_some(SourceKey::AvcAac(path))
}

fn groups<'a>(
    graph: &'a SourceGraph,
    sequence: &SourceSequence,
    media: &MediaIndex<'a>,
) -> Result<(Vec<Group<'a>>, HashMap<&'a str, (usize, &'a SourceItem)>)> {
    let mut bin_owners = HashSet::new();
    for id in graph.bins.iter().flat_map(|bin| &bin.items) {
        if !bin_owners.insert(id.as_str()) {
            return Err(format!("{id}: mixed-source item has multiple source bin owners").into());
        }
    }
    let mut used_media = HashSet::new();
    let mut group_ids = HashMap::new();
    let mut items = HashMap::new();
    let mut groups = Vec::new();
    for item in &graph.items {
        if !bin_owners.contains(item.id.as_str()) {
            return Err(format!("{}: mixed-source item needs one source bin", item.id).into());
        }
        if item
            .media
            .iter()
            .any(|binding| !used_media.insert(binding.media.as_str()))
        {
            return Err(format!(
                "{}: mixed-source item has an ambiguous media owner",
                item.id
            )
            .into());
        }
        let key = locate_group(media, item)
            .ok_or_else(|| format!("{}: unqualified mixed-source original", item.id))?;
        let group = *group_ids.entry(key).or_insert_with(|| {
            let index = groups.len();
            groups.push(Group {
                profile: key.profile(),
                items: Vec::new(),
                overrides: 0,
                lanes: [None, None, None],
            });
            index
        });
        groups[group].items.push(item);
        items.insert(item.id.as_str(), (group, item));
    }
    if used_media.len() != graph.media.len()
        || ![Profile::AvcAac, Profile::ManagedAtom]
            .iter()
            .all(|profile| groups.iter().any(|group| group.profile == *profile))
    {
        return Err("mixed-source graph needs linked AVC/AAC and managed OP-Atom media without unowned streams".into());
    }
    for group in &groups {
        let expected = if group.profile == Profile::ManagedAtom {
            3
        } else {
            2
        };
        if group.items.is_empty()
            || group.items.len() > expected
            || (group.profile == Profile::ManagedAtom && group.items.len() != expected)
        {
            return Err(group.profile.layout_error().into());
        }
    }

    for row in &graph.sidecars {
        let group = items
            .get(row.object.as_str())
            .map(|(group, _)| &mut groups[*group]);
        let accepted = match (row.adapter.as_str(), group) {
            ("premiere.source" | "premiere.media_capture", None) => row.object == sequence.id,
            ("premiere.media_identity" | "premiere.audio_groups", Some(_)) => true,
            (MANAGED_ATOM_RECEIPT, Some(group)) => group.profile == Profile::ManagedAtom,
            ("premiere.logging", Some(group)) => {
                group.profile == Profile::ManagedAtom && is_managed_logging(row)
            }
            ("premiere.clip_markers", Some(group)) => group.profile == Profile::AvcAac,
            (_, Some(group)) if group.profile == Profile::AvcAac && is_avc_override(row) => {
                group.overrides += 1;
                true
            }
            _ => false,
        };
        if !accepted {
            return Err(format!(
                "{}: unsupported Avid source receipt {}",
                row.object, row.adapter
            )
            .into());
        }
    }
    Ok((groups, items))
}

pub(super) fn plan<'a>(graph: &'a SourceGraph, sequence: &'a SourceSequence) -> Result<Plan<'a>> {
    let rate = EditRate::from_graph(sequence.rate)?;
    if !matches!(rate, EditRate::Fps23976) {
        return Err("mixed-source linked media requires 23.976 fps".into());
    }
    let start = prepare::timeline_clock(rate, sequence.start)
        .ok_or("sequence needs a known whole-frame start")?;
    if !prepare::neutral_sequence(graph, sequence, start, rate, true) {
        return Err("mixed-source links require a neutral 1080p V1/A1/A2 graph".into());
    }
    let media = MediaIndex::new(graph);
    let (mut groups, items) = groups(graph, sequence, &media)?;
    let mut edits = [Vec::new(), Vec::new(), Vec::new()];
    let mut transitions = [Vec::new(), Vec::new(), Vec::new()];

    for (lane, (kind, _, label)) in prepare::LANES.into_iter().enumerate() {
        let picture = lane == 0;
        let mut staged = Vec::new();
        for edit in prepare::lane_edits(sequence, lane)? {
            if !prepare::neutral_edit(edit, picture) {
                return Err(format!("{}: expected a neutral edit on {label}", edit.id).into());
            }
            let &(group_id, item) = items.get(edit.item.as_str()).ok_or("missing edit item")?;
            let group = &mut groups[group_id];
            let picture_item = group.lanes[0]
                .as_ref()
                .map(|entry| entry.source.item.id.as_str());
            let entry = &mut group.lanes[lane];
            let selector = edit.audio_streams.first().copied();
            if entry.is_none() {
                *entry = Some(BoundLane {
                    source: group.profile.qualify_route(
                        &media,
                        item,
                        edit,
                        kind,
                        lane,
                        rate,
                        true,
                        picture_item,
                    )?,
                    selector,
                });
            }
            let bound = entry.as_ref().expect("linked source lane was prepared");
            if bound.source.item.id != item.id
                || bound.selector != selector
                || !picture && edit.audio_streams.len() != 1
            {
                return Err(format!(
                    "{}: conflicting {label} selector for its source set",
                    edit.id
                )
                .into());
            }
            let span = prepare::prepare_span(
                edit,
                bound.source.binding.source_offset,
                bound.source.route.extent,
                rate,
            )?;
            let binding = EditBinding {
                group: group_id,
                route: lane,
                track: bound.source.route.source_lane,
            };
            staged.push((edit.id.as_str(), PreparedEdit { span, binding }));
        }
        if staged.is_empty() {
            return Err(format!("{label}: missing edit").into());
        }
        transitions[lane] = prepare::prepare_transitions(sequence, lane, rate, &mut staged)?;
        edits[lane] = staged.into_iter().map(|(_, edit)| edit).collect();
    }

    let sources = groups
        .into_iter()
        .map(|group| group.finish(&media, rate))
        .collect::<Result<Vec<_>>>()?;
    prepare::timeline_plan(sequence, rate, start, sources, edits, transitions)
}
