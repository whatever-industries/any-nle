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
        .ok_or("invalid Avid bin name")?;
    let revision = media_digest(&bin)?;
    let source = SourceInput {
        path: bin.clone(),
        revision: hex::encode(revision),
    };
    let read =
        avid_adapter::read_with_ffprobe(&[source], media_roots, name, &[name.to_owned()], ffprobe)?;
    let graph = &read.graph;
    let mut matches = graph
        .sequences
        .iter()
        .filter(|s| s.id == sequence || s.name == sequence);
    let selected = matches.next().ok_or("selected Avid sequence is absent")?;
    if matches.next().is_some() {
        return Err("ambiguous Avid sequence selector; use the exact graph sequence ID".into());
    }
    let bindings = bind_sequence(graph, selected, ffprobe)?;
    let prepared = prepare_bindings(graph, &selected.id, (1920, 1080), bindings)?;
    let result = publish_checked(&prepared, skeleton, output, || {
        if media_digest(&bin)? != revision {
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
    verify_atom(item.id.clone(), [picture, a1, a2], ffprobe)
}
