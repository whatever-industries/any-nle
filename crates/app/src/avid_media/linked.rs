//! Internal graph/2 replay to one native Avid bin.
mod markers;
mod native_bin;
mod original;
mod prepare;
mod verify;

use super::Result;
use crate::{backend_bundle::BackendTools, output, rewrap};
use avid_reader::AvbDocument;
use graph::source::{OriginalStream, SourceGraph};
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

const RATE: i64 = 24;
const RASTER: (i64, i64) = (1920, 1080);
const MAX_GRAPH_BYTES: u64 = 8 << 20;
const MAX_DONOR_BYTES: u64 = 4 << 20;

#[derive(Debug)]
struct Route<'a> {
    path: &'a str,
    name: &'a str,
    tape: Option<&'a str>,
    clock: u32,
    extent: u32,
    source_lane: i64,
    bits: Option<i64>,
    stream: &'a OriginalStream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    record: u32,
    source: u32,
    length: u32,
}

#[derive(Debug)]
struct Plan<'a> {
    name: &'a str,
    start: u32,
    length: u32,
    routes: [Route<'a>; 3],
    spans: [Span; 3],
    picture_markers: Vec<markers::Marker<'a>>,
    sequence_markers: Vec<markers::Marker<'a>>,
}

pub fn write_linked_bin(
    graph_path: &Path,
    sequence_id: &str,
    donor_path: Option<&Path>,
    destination: &Path,
    tools: &BackendTools,
) -> Result<()> {
    output::preflight(destination, None)?;
    check_extension(destination)?;
    let graph_bytes = read_small(graph_path, MAX_GRAPH_BYTES)?;
    let graph = SourceGraph::from_json_text(std::str::from_utf8(&graph_bytes)?)?;
    write_graph(
        &graph,
        sequence_id,
        donor_path,
        false,
        destination,
        tools,
        || {
            if read_small(graph_path, MAX_GRAPH_BYTES)? != graph_bytes {
                return Err("graph changed during linked bin preparation".into());
            }
            Ok(())
        },
    )
}

#[cfg(target_os = "macos")]
pub fn write_linked_project(
    project: &Path,
    sequence_id: &str,
    destination: &Path,
    tools: &BackendTools,
) -> Result<()> {
    output::preflight(destination, None)?;
    check_extension(destination)?;
    let before = rewrap::source::stamp(project)?;
    let graph = match project.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("prproj") => {
            anynle_resolve_adapter::read_premiere_avid_graph(project, sequence_id, &tools.ffprobe)?
        }
        Some(ext) if ext.eq_ignore_ascii_case("drp") => {
            anynle_resolve_adapter::read_resolve_avid_graph(project, sequence_id, &tools.ffprobe)?
        }
        _ => return Err("expected a saved .prproj or .drp project".into()),
    };
    write_graph(&graph, sequence_id, None, true, destination, tools, || {
        if rewrap::source::stamp(project)? != before {
            return Err("source project changed during linked bin preparation".into());
        }
        Ok(())
    })
}

fn check_extension(destination: &Path) -> Result<()> {
    if !destination
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("avb"))
    {
        return Err("linked bin destination needs an .avb extension".into());
    }
    Ok(())
}

fn write_graph(
    graph: &SourceGraph,
    sequence_id: &str,
    donor_path: Option<&Path>,
    captured: bool,
    destination: &Path,
    tools: &BackendTools,
    recheck_source: impl FnOnce() -> Result<()>,
) -> Result<()> {
    anynle_media_verification::linked_original::qualify(&graph.media)?;
    let plan = prepare::plan(graph, sequence_id, captured)?;
    if donor_path.is_some() && !plan.picture_markers.is_empty() {
        return Err(
            "picture markers require a newly created Avid master; use --linked-originals".into(),
        );
    }
    let donor_bytes = donor_path
        .map(|path| read_small(path, MAX_DONOR_BYTES))
        .transpose()?;
    let prepared_donor = if let Some(bytes) = &donor_bytes {
        let doc = AvbDocument::from_bytes(bytes.clone())?;
        let writer = native_bin::inspect(&doc, &plan.routes)?.prepare_write();
        Some((doc, writer))
    } else {
        None
    };
    let originals = [&plan.routes[0], &plan.routes[1]];
    let stamps = [
        rewrap::source::stamp(Path::new(originals[0].path))?,
        rewrap::source::stamp(Path::new(originals[1].path))?,
    ];
    for (route, picture) in originals.into_iter().zip([true, false]) {
        verify::original(route, picture, donor_path.is_none(), tools)?;
    }
    let doc = if let Some((mut doc, writer)) = prepared_donor {
        writer.apply(&mut doc, &plan)?;
        doc
    } else {
        original::build(&plan)?
    };
    let readback = AvbDocument::from_bytes(doc.to_bytes()?)?;
    native_bin::inspect(&readback, &plan.routes)?.check(&readback, &plan)?;
    recheck_source()?;
    if let Some((path, bytes)) = donor_path.zip(donor_bytes.as_ref()) {
        if read_small(path, MAX_DONOR_BYTES)? != *bytes {
            return Err("donor changed during linked bin preparation".into());
        }
    }
    for (route, stamp) in originals.into_iter().zip(stamps) {
        if rewrap::source::stamp(Path::new(route.path))? != stamp {
            return Err(format!("{} changed during linked bin preparation", route.path).into());
        }
    }
    readback.write_new(destination)?;
    Ok(())
}

fn read_small(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(format!("{} is not a regular file", path.display()).into());
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds the linked-bin input limit", path.display()).into());
    }
    Ok(bytes)
}
