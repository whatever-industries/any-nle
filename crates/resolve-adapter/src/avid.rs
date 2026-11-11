//! Automatic direct OP-Atom binding for the qualified single-timeline Avid route.
use crate::{
    archive::{check_output, publish_checked},
    atom::{verify_atom, AtomInput},
    media::media_digest,
    prepare::{prepare_bindings, MAX_CORE_MEDIA_ITEMS},
    Result, StreamSelector,
};
use avid_reader::source::SourceInput;
use graph::{
    source::{SourceGraph, SourceItem, SourceSequence},
    StreamKind,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub struct AvidAtomConversion {
    pub notices: Vec<String>,
    pub omitted_markers: Vec<String>,
    pub cleanup_warning: Option<String>,
}

/// Resolve a single saved sequence's UME links through the shared Avid adapter.
pub fn convert_avid_ume(
    bin: &Path,
    sequence: &str,
    options: &avid_adapter::original::OriginalOptions,
    layout: &Path,
    profile: &Path,
    output: &Path,
) -> Result<AvidAtomConversion> {
    check_output(output)?;
    let (source, revision, name) = bin_input(bin)?;
    let read = avid_adapter::read_with_originals(
        std::slice::from_ref(&source),
        &[],
        &name,
        std::slice::from_ref(&name),
        options,
    )?;
    let selected = select_sequence(&read.graph, sequence)?;
    let spec = crate::linked::avid_original_spec(&read.graph, selected, &read.original_inputs)?;
    let prepared = crate::linked::prepare(&read.graph, spec, &options.ffprobe)?;
    let result = crate::archive::publish_turnover_checked(
        &prepared,
        layout,
        profile,
        None,
        None,
        output,
        || {
            if media_digest(&source.path)? != revision {
                return Err("Avid bin changed since interpretation".into());
            }
            Ok(())
        },
    )?;
    let mut notices = read.diagnostics;
    notices.extend(read.scan_issues);
    notices.extend(read.audio_group_issues);
    Ok(AvidAtomConversion {
        notices,
        omitted_markers: prepared.omitted_markers().to_vec(),
        cleanup_warning: result.cleanup_warning,
    })
}

/// Convert one saved Avid bin using explicitly supplied native media roots.
pub fn convert_avid_atom(
    bin: &Path,
    media_roots: &[PathBuf],
    sequence: &str,
    skeleton: &Path,
    ffprobe: &Path,
    output: &Path,
) -> Result<AvidAtomConversion> {
    if media_roots.is_empty() {
        return Err("provide at least one Avid media root".into());
    }
    check_output(output)?;
    let (source, revision, name) = bin_input(bin)?;
    let read = avid_adapter::read_with_ffprobe(
        std::slice::from_ref(&source),
        media_roots,
        &name,
        std::slice::from_ref(&name),
        ffprobe,
    )?;
    let graph = &read.graph;
    let selected = select_sequence(graph, sequence)?;
    let bindings = bind_sequence(graph, selected, ffprobe)?;
    let prepared = prepare_bindings(graph, &selected.id, (1920, 1080), bindings)?;
    let result = publish_checked(&prepared, skeleton, output, || {
        if media_digest(&source.path)? != revision {
            return Err("Avid bin changed since interpretation".into());
        }
        Ok(())
    })?;
    let mut notices = read.diagnostics;
    notices.extend(read.scan_issues);
    notices.extend(read.audio_group_issues);
    Ok(AvidAtomConversion {
        notices,
        omitted_markers: prepared.omitted_markers().to_vec(),
        cleanup_warning: result.cleanup_warning,
    })
}

fn bin_input(bin: &Path) -> Result<(SourceInput, [u8; 32], String)> {
    let bin = std::fs::canonicalize(bin)?;
    if !bin
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("avb"))
    {
        return Err("expected an Avid .avb bin".into());
    }
    let name = bin
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or("invalid Avid bin name")?
        .to_owned();
    let revision = media_digest(&bin)?;
    let source = SourceInput {
        path: bin,
        revision: hex::encode(revision),
    };
    Ok((source, revision, name))
}

fn select_sequence<'a>(graph: &'a SourceGraph, sequence: &str) -> Result<&'a SourceSequence> {
    if let Some(exact) = graph.sequences.iter().find(|s| s.id == sequence) {
        return Ok(exact);
    }
    let mut matches = graph.sequences.iter().filter(|s| s.name == sequence);
    let selected = matches.next().ok_or("selected Avid sequence is absent")?;
    if matches.next().is_some() {
        return Err("ambiguous Avid sequence selector; use the exact graph sequence ID".into());
    }
    Ok(selected)
}

fn bind_sequence(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    ffprobe: &Path,
) -> Result<Vec<crate::VerifiedBinding>> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for placement in &sequence.placements {
        if seen.insert(placement.item.as_str()) {
            if items.len() == MAX_CORE_MEDIA_ITEMS {
                return Err("Resolve media item count outside supported range".into());
            }
            let item = graph
                .items
                .iter()
                .find(|item| item.id == placement.item)
                .ok_or("Avid media item absent")?;
            items.push(item);
        }
    }
    if items.is_empty() {
        return Err("selected Avid sequence has no resolved edits".into());
    }
    items
        .into_iter()
        .map(|item| bind_item(graph, item, ffprobe))
        .collect()
}

fn bind_item(
    graph: &SourceGraph,
    item: &SourceItem,
    ffprobe: &Path,
) -> Result<crate::VerifiedBinding> {
    verify_atom(item.id.clone(), atom_inputs(graph, item)?, ffprobe)
}

fn atom_inputs(graph: &SourceGraph, item: &SourceItem) -> Result<[AtomInput; 3]> {
    if item.media.len() != 3 || !item.stream_groups.is_empty() {
        return Err(
            "direct Avid OP-Atom requires one picture MXF and both independent mono siblings"
                .into(),
        );
    }
    let mut siblings: [Option<AtomInput>; 3] = [None, None, None];
    for link in &item.media {
        let source = graph
            .media
            .iter()
            .find(|media| media.id == link.media)
            .ok_or("Avid source media absent")?;
        let index = match (source.kind, source.stream) {
            (StreamKind::Video, None) => 0,
            (StreamKind::Audio, Some(1)) => 1,
            (StreamKind::Audio, Some(2)) => 2,
            _ => return Err("unsupported saved Avid sibling track label".into()),
        };
        let [path] = source.locations.as_slice() else {
            return Err("Avid sibling requires one resolved MXF locator".into());
        };
        let input = AtomInput {
            path: PathBuf::from(path),
            stream: StreamSelector {
                media_id: source.id.clone(),
                index,
                channel: (index > 0).then_some(0),
            },
            expected_sha256: None,
        };
        if siblings[index].replace(input).is_some() {
            return Err("duplicate saved Avid sibling track label".into());
        }
    }
    let [Some(picture), Some(a1), Some(a2)] = siblings else {
        return Err("missing saved Avid picture or mono sibling label".into());
    };
    Ok([picture, a1, a2])
}

/// Interpret all selected bins once. Assign each selected item its saved bin and
/// bind a complete original or a complete OP-Atom family.
pub fn prepare_avid_turnover(
    bins: &[PathBuf],
    media_roots: &[PathBuf],
    sequences: &[String],
    options: &avid_adapter::original::OriginalOptions,
    project_root: Option<&Path>,
) -> Result<crate::turnover::Turnover> {
    use crate::linked::{GroupSpec, ProjectSpec, TimelineSpec};
    if !(1..=256).contains(&bins.len()) || !(1..=64).contains(&sequences.len()) {
        return Err("select 1–256 Avid bins and 1–64 sequences".into());
    }
    let mut inputs = Vec::new();
    let mut guards = Vec::new();
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for bin in bins {
        let (input, digest, name) = bin_input(bin)?;
        if !seen.insert(input.path.clone()) {
            return Err("duplicate Avid bin path".into());
        }
        guards.push((input.path.clone(), digest));
        inputs.push(input);
        names.push(name);
    }
    let project_root = project_root.map(std::fs::canonicalize).transpose()?;
    let project_name = match &project_root {
        Some(root) if root.is_dir() => root
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid Avid project root name")?,
        Some(_) => return Err("Avid project root must be a directory".into()),
        None => &names[0],
    };
    let mut read =
        avid_adapter::read_with_originals(&inputs, media_roots, project_name, &names, options)?;
    let owning_bin_ids: Vec<_> = read.graph.bins.iter().map(|bin| bin.id.clone()).collect();
    if let Some(root) = &project_root {
        retain_project_folders(&mut read.graph, &inputs, root)?;
    }
    let graph = &read.graph;
    let mut spec = ProjectSpec {
        root_bin_id: None,
        timelines: Vec::new(),
        media: Vec::new(),
        atom_media: Vec::new(),
    };
    let mut required = Vec::new();
    let mut unique = HashSet::new();
    let bin_id = |index| -> Result<String> {
        Ok(owning_bin_ids
            .get(index)
            .cloned()
            .ok_or("saved Avid owning bin is absent")?)
    };
    for selector in sequences {
        let sequence = select_sequence(graph, selector)?;
        let owner = read
            .sequence_bins
            .get(&sequence.id)
            .ok_or("no saved sequence owner")?;
        let group = group_selection(graph, sequence)?
            .map(|(group_id, cameras)| {
                let index = read
                    .group_bins
                    .get(group_id)
                    .ok_or("no saved Group owner")?;
                Ok::<_, Box<dyn std::error::Error>>(GroupSpec {
                    bin_id: bin_id(*index)?,
                    cameras: cameras.map(str::to_owned),
                })
            })
            .transpose()?;
        for item in sequence
            .placements
            .iter()
            .map(|edit| edit.item.as_str())
            .chain(
                group
                    .iter()
                    .flat_map(|group| group.cameras.iter().map(String::as_str)),
            )
        {
            if unique.insert(item.to_owned()) {
                if required.len() == 256 {
                    return Err("selected media exceed the Resolve project limit".into());
                }
                required.push(item.to_owned());
            }
        }
        spec.timelines.push(TimelineSpec {
            id: sequence.id.clone(),
            bin_id: bin_id(*owner)?,
            canvas: (1920, 1080),
            group,
        });
    }
    for id in required {
        let item = graph
            .items
            .iter()
            .find(|item| item.id == id)
            .ok_or("selected item is absent")?;
        let mut owners = graph.bins.iter().filter(|bin| bin.items.contains(&item.id));
        let owner = owners.next().ok_or("selected media has no owning bin")?;
        if owners.next().is_some() {
            return Err(format!("{id}: media bin ownership is ambiguous").into());
        }
        let streams = item
            .media
            .iter()
            .map(|link| {
                graph
                    .media
                    .iter()
                    .find(|media| media.id == link.media)
                    .ok_or("selected stream is absent")
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let original_count = streams
            .iter()
            .filter(|media| media.original.is_some())
            .count();
        if streams.is_empty() || (original_count != 0 && original_count != streams.len()) {
            return Err(format!("{id}: incomplete or mixed original-file binding").into());
        }
        if original_count == 0 {
            let siblings =
                atom_inputs(graph, item)?.map(|input| -> Result<crate::atom::SiblingSpec> {
                    let sha256 = hex::encode(media_digest(&input.path)?);
                    Ok(crate::atom::SiblingSpec {
                        path: input.path,
                        sha256,
                        media_id: input.stream.media_id,
                        index: input.stream.index,
                        channel: input.stream.channel,
                    })
                });
            let [picture, a1, a2] = siblings;
            spec.atom_media.push(crate::linked::AtomSpec {
                bin_id: owner.id.clone(),
                binding: crate::AtomBindingSpec {
                    item_id: item.id.clone(),
                    siblings: [picture?, a1?, a2?],
                },
            });
        } else {
            spec.media.push(crate::linked::original_media_spec(
                item,
                &owner.id,
                &streams,
                &read.original_inputs,
            )?);
        }
    }
    let mut notices = read.diagnostics;
    notices.extend(read.scan_issues);
    notices.extend(read.audio_group_issues);
    crate::turnover::Turnover::prepare(read.graph, spec, &options.ffprobe, guards, notices, vec![])
}

fn group_selection<'a>(
    graph: &'a SourceGraph,
    sequence: &SourceSequence,
) -> Result<Option<(&'a str, [&'a str; 2])>> {
    let mut uses = graph
        .multicam_uses
        .iter()
        .filter(|use_| use_.sequence == sequence.id);
    let Some(first) = uses.next() else {
        return Ok(None);
    };
    let [id] = first.candidates.as_slice() else {
        return Err("unresolved Group identity".into());
    };
    if uses.any(|use_| use_.candidates.as_slice() != std::slice::from_ref(id)) {
        return Err("multiple Group identities in one Resolve timeline are unsupported".into());
    }
    let group = graph
        .multicams
        .iter()
        .find(|group| &group.id == id)
        .ok_or("Group is absent")?;
    let lane = group
        .lanes
        .iter()
        .find(|lane| lane.kind == StreamKind::Video && lane.index == 1)
        .ok_or("Group picture lane is absent")?;
    let [a, b] = lane.alternatives.as_slice() else {
        return Err("expected two camera alternatives".into());
    };
    fn camera(alt: &graph::source::MulticamAlternative) -> Result<&str> {
        let mut items = alt.pieces.iter().filter_map(|piece| piece.item.as_deref());
        let item = items.next().ok_or("Group camera lacks a source item")?;
        if items.next().is_some() {
            return Err("Group camera has multiple source spans".into());
        }
        Ok(item)
    }
    Ok(Some((id, [camera(a)?, camera(b)?])))
}

fn retain_project_folders(
    graph: &mut SourceGraph,
    inputs: &[SourceInput],
    root: &Path,
) -> Result<()> {
    let mut folders = std::collections::HashMap::<PathBuf, String>::new();
    for (index, input) in inputs.iter().enumerate() {
        let relative = input
            .path
            .strip_prefix(root)
            .map_err(|_| "Avid bin is outside the project root")?;
        let folder = relative.parent().ok_or("Avid bin has no relative folder")?;
        let mut path = PathBuf::new();
        let mut parent = None;
        for component in folder.components() {
            let name = component
                .as_os_str()
                .to_str()
                .ok_or("invalid Avid folder name")?;
            path.push(component);
            let id = if let Some(id) = folders.get(&path) {
                id.clone()
            } else {
                if graph.bins.len() >= 256 {
                    return Err("Avid folder hierarchy exceeds the Resolve bin limit".into());
                }
                let id = format!("avid:folder:{}", folders.len());
                graph.bins.push(graph::Bin {
                    id: id.clone(),
                    name: name.into(),
                    parent: parent.clone(),
                    items: vec![],
                });
                folders.insert(path.clone(), id.clone());
                id
            };
            parent = Some(id);
        }
        graph.bins[index].parent = parent;
    }
    // The graph requires parents to precede their children. Native bin
    // ownership uses the input ordinal and is resolved independently.
    graph.bins.rotate_right(folders.len());
    Ok(())
}
