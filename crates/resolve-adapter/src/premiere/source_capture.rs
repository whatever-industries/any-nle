//! Shared graph/2 construction for Premiere source readers.
use crate::Result;
use graph::{
    source::{SourceClock, SourceClockProvenance, SourceGraph, SourceSequence},
    Bin, Project as GraphProject, Rational, Sidecar, Value,
};
use prproj::model::{ItemId, ItemType, Project};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub(super) const ROOT_BIN_ID: &str = "premiere:root";

pub(super) struct GraphCapture {
    pub(super) graph: SourceGraph,
    pub(super) folder_ids: HashMap<ItemId, usize>,
    pub(super) sequence_bin: usize,
}

impl GraphCapture {
    pub(super) fn new(project: &Project, path: &Path, root: ItemId, panel: ItemId) -> Result<Self> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid Premiere project name")?;
        let mut graph = Self::empty_graph(name.into());
        let mut folder_ids = HashMap::from([(root, 0)]);
        let sequence_bin = owning_bin(project, panel, &mut folder_ids, &mut graph.bins)?;
        Ok(Self {
            graph,
            folder_ids,
            sequence_bin,
        })
    }

    pub(super) fn empty_graph(name: String) -> SourceGraph {
        SourceGraph {
            project: GraphProject { name },
            media: vec![],
            items: vec![],
            bins: vec![Bin {
                id: ROOT_BIN_ID.into(),
                name: "Master".into(),
                parent: None,
                items: vec![],
            }],
            sequences: vec![],
            losses: vec![],
            sidecars: vec![],
            multicams: vec![],
            multicam_uses: vec![],
        }
    }
}

pub(super) fn source_sequence(
    id: String,
    name: String,
    frame_size: Option<(i64, i64)>,
    clock: SourceClock,
) -> SourceSequence {
    SourceSequence {
        disabled_placements: vec![],
        id,
        name,
        rate: clock.rate,
        frame_size,
        start: clock.exact_start(),
        clock: Some(clock),
        tracks: vec![],
        placements: vec![],
        transitions: vec![],
        markers: vec![],
        texts: vec![],
    }
}

pub(super) fn source_receipt(
    id: String,
    bin: &str,
    frame_ticks: i64,
    zero_point_ticks: i64,
    extra_fields: impl IntoIterator<Item = (String, Value)>,
) -> Sidecar {
    let mut fields = vec![
        ("bin_id".into(), Value::Str(bin.into())),
        ("frame_ticks".into(), Value::Int(frame_ticks)),
        ("zero_point_ticks".into(), Value::Int(zero_point_ticks)),
    ];
    fields.extend(extra_fields);
    Sidecar {
        adapter: "premiere.source".into(),
        object: id,
        data: Value::Obj(fields),
    }
}

pub(super) fn source_clock(
    count: u64,
    rate: Rational,
    base: u16,
    drop: bool,
    id: &str,
    track: &str,
    route: &str,
) -> SourceClock {
    SourceClock {
        effective_count: count,
        rate,
        display_base: base,
        drop,
        provenance: SourceClockProvenance {
            origin_count: count,
            source_offset: 0,
            source_rate: rate,
            source_package: id.into(),
            timecode_track: track.into(),
            route: route.into(),
        },
    }
}

pub(super) fn owning_bin(
    project: &Project,
    item: ItemId,
    ids: &mut HashMap<ItemId, usize>,
    bins: &mut Vec<Bin>,
) -> Result<usize> {
    let mut item = project
        .item(item)
        .parent
        .ok_or("unfiled Premiere panel item")?;
    let mut ancestry = Vec::new();
    let mut seen = HashSet::new();
    while !ids.contains_key(&item) {
        if project.item(item).item_type != ItemType::Bin
            || !seen.insert(item)
            || ancestry.len() >= 64
        {
            return Err("invalid Premiere bin ancestry".into());
        }
        ancestry.push(item);
        item = project
            .item(item)
            .parent
            .ok_or("Premiere bin is detached")?;
    }
    let mut parent = ids[&item];
    for bin in ancestry.into_iter().rev() {
        if bins.len() == 256 {
            return Err("Premiere project exceeds bin limit".into());
        }
        let element = project.item(bin).element;
        let uid = project
            .doc
            .tree
            .get(element, "ObjectUID")
            .ok_or("Premiere bin lacks ObjectUID")?;
        let index = bins.len();
        bins.push(Bin {
            id: format!("premiere:bin:{uid}"),
            parent: Some(bins[parent].id.clone()),
            name: project.item_name(bin),
            items: vec![],
        });
        ids.insert(bin, index);
        parent = index;
    }
    Ok(parent)
}
