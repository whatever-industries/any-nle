//! Link verified managed OP1a streams through Premiere's native AV master writer.
use avid_adapter::managed_multistream::VERIFIED_ADAPTER;
use graph::{source::SourceGraph, StreamKind, Value};
use prproj::{
    convert::{
        prepared::{PreparedClip, PreparedProjectInput},
        rewrap::MediaRelink,
        writer::{
            prepare_typed_project_for_premiere_27, DestinationBinLayout, PreparedProject, Result,
            TypedMedia, WriteError,
        },
    },
    UuidSource,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub(crate) fn has_verified_streams(graph: &SourceGraph) -> bool {
    graph
        .sidecars
        .iter()
        .any(|sidecar| sidecar.adapter == VERIFIED_ADAPTER)
}

pub(crate) fn prepare(
    graph: &SourceGraph,
    input: &mut PreparedProjectInput,
    output: &Path,
    ids: &mut dyn UuidSource,
    layout: Option<&DestinationBinLayout>,
    offline: &HashSet<&str>,
) -> Result<PreparedProject> {
    let invalid =
        |reason: &str| WriteError::Other(prproj::Error::Value(format!("managed OP1a: {reason}")));
    let facts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|sidecar| sidecar.adapter == VERIFIED_ADAPTER)
        .map(|sidecar| (sidecar.object.as_str(), &sidecar.data))
        .collect();
    let media: HashMap<_, _> = graph
        .media
        .iter()
        .map(|media| (media.id.as_str(), media))
        .collect();
    let mut clips = HashSet::new();
    let mut bindings = Vec::new();
    let mut all_paths: HashSet<String> = graph
        .media
        .iter()
        .flat_map(|media| media.locations.iter().cloned())
        .collect();
    let prepared_items: HashMap<_, _> = input
        .clips
        .iter()
        .enumerate()
        .filter_map(|(index, clip)| match clip {
            PreparedClip::Legacy(clip) => Some((clip.id.clone(), index)),
            PreparedClip::Original(_) => None,
        })
        .collect();
    for item in &graph.items {
        let selected = item
            .media
            .iter()
            .filter(|binding| facts.contains_key(binding.media.as_str()))
            .count();
        if selected == 0 {
            continue;
        }
        if selected != 3 || item.media.len() != 3 || !item.stream_groups.is_empty() {
            return Err(invalid("requires one picture and two verified mono routes"));
        }
        let clip_id = item.id.strip_prefix("clip:").unwrap_or(&item.id);
        let index = *prepared_items
            .get(clip_id)
            .ok_or_else(|| invalid("prepared clip is missing"))?;
        let PreparedClip::Legacy(clip) = &mut input.clips[index] else {
            unreachable!()
        };
        if !clips.insert(clip.id.clone()) {
            return Err(invalid("prepared clip identity is ambiguous"));
        }
        let mut shared_path = None;
        for binding in &item.media {
            let physical = media[binding.media.as_str()];
            let [path] = physical.locations.as_slice() else {
                return Err(invalid("requires a unique MXF path"));
            };
            if physical.online != Some(true)
                || offline.contains(path.as_str())
                || shared_path
                    .replace(path)
                    .is_some_and(|previous| previous != path)
            {
                return Err(invalid("all routes must link to the same online MXF"));
            }
            let ordinal = facts[binding.media.as_str()].get("audio_ordinal");
            let (slot, tag, ordinal) = match (physical.kind, physical.stream, ordinal) {
                (StreamKind::Video, None, Some(Value::Null)) => (
                    &mut clip
                        .video
                        .as_mut()
                        .ok_or_else(|| invalid("missing picture"))?
                        .path,
                    "V1",
                    None,
                ),
                (StreamKind::Audio, Some(label @ (1 | 2)), Some(Value::Int(ordinal)))
                    if *ordinal == label - 1 =>
                {
                    let audio = clip
                        .audio
                        .iter_mut()
                        .find(|audio| audio.avid_track == Some(label))
                        .ok_or_else(|| invalid("missing prepared mono track"))?;
                    (
                        &mut audio.path,
                        if label == 1 { "A1" } else { "A2" },
                        Some(*ordinal as u32),
                    )
                }
                _ => {
                    return Err(invalid(
                        "verified selectors are outside the Premiere Direct Out profile",
                    ))
                }
            };
            if slot != path {
                return Err(invalid("prepared path disagrees with the physical source"));
            }
            // The writer needs one Media view per stream. Every
            // temporary path is replaced before the document can be published.
            let source_path = format!("{path}.anynle-{}-{tag}.mxf", clip.id);
            if !all_paths.insert(source_path.clone()) {
                return Err(invalid(
                    "temporary stream identifier collides with a media path",
                ));
            }
            *slot = source_path.clone();
            bindings.push(MediaRelink {
                source_path,
                target_path: path.clone(),
                audio_stream: ordinal,
                separate_mono: false,
                pixel_aspect: None,
            });
        }
    }
    if clips.is_empty() || bindings.len() != facts.len() {
        return Err(invalid(
            "verified graph stream does not belong to a prepared clip",
        ));
    }
    let media = TypedMedia::VerifiedOp1a { bindings: &bindings, clips: &clips, offline };
    prepare_typed_project_for_premiere_27(input, output, ids, media, layout)
}
