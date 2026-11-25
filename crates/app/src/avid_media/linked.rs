//! Internal graph/2 replay to one native Avid bin.
#[cfg(target_os = "macos")]
mod managed_atom;
mod markers;
mod mixed;

const MANAGED_ATOM_RECEIPT: &str = "premiere.managed_op_atom";
mod mp4;
mod native_bin;
mod original;
mod prepare;
mod source_profile;
use source_profile::{SourceKind, SourceSet};
mod transition;
mod verify;

use super::Result;
use crate::{backend_bundle::BackendTools, output, rewrap};
use avid_reader::AvbDocument;
use graph::source::{OriginalStream, SourceGraph, MAX_SOURCE_GRAPH_BYTES};
use graph::Rational;
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
};

const TIMECODE_BASE: i64 = 24;
const RASTER: (i64, i64) = (1920, 1080);
const MAX_DONOR_BYTES: u64 = 4 << 20;

#[derive(Clone, Copy, Debug)]
enum EditRate {
    Fps24,
    Fps23976,
}

impl EditRate {
    fn from_graph(rate: Rational) -> Result<Self> {
        match (rate.num(), rate.den()) {
            (24, 1) => Ok(Self::Fps24),
            (24_000, 1001) => Ok(Self::Fps23976),
            _ => Err("linked Avid output requires a 24 or 23.976 fps timeline".into()),
        }
    }

    fn exact(self) -> Rational {
        match self {
            Self::Fps24 => Rational::integer(24),
            Self::Fps23976 => Rational::new(24_000, 1001).unwrap(),
        }
    }

    fn saved(self) -> f64 {
        match self {
            Self::Fps24 => 24.0,
            Self::Fps23976 => 23.976,
        }
    }

    fn format(self) -> &'static str {
        match self {
            Self::Fps24 => "HD 1080p/24",
            Self::Fps23976 => "HD 1080p/23.976",
        }
    }

    fn frames(self, time: Rational) -> Option<i64> {
        let rate = self.exact();
        Rational::new(
            i128::from(time.num()) * i128::from(rate.num()),
            i128::from(time.den()) * i128::from(rate.den()),
        )?
        .to_units(1)
    }

    fn samples_per_frame(self) -> i64 {
        let rate = self.exact();
        48_000 * rate.den() / rate.num()
    }
}

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
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    managed: Option<ManagedBinding<'a>>,
}

#[derive(Clone, Copy, Debug)]
struct ManagedBinding<'a> {
    material: &'a str,
    receipt: &'a graph::Value,
    clock: &'a graph::source::SourceClock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    record: u32,
    source: u32,
    length: u32,
}

#[derive(Clone, Copy, Debug)]
struct EditBinding {
    group: usize,
    route: usize,
    track: i64,
}

#[derive(Clone, Copy, Debug)]
struct PreparedEdit {
    span: Span,
    binding: EditBinding,
}

#[derive(Debug)]
struct Plan<'a> {
    name: &'a str,
    rate: EditRate,
    start: u32,
    length: u32,
    sources: Vec<SourceSet<'a>>,
    edits: [Vec<PreparedEdit>; 3],
    transitions: [Vec<transition::Transition>; 3],
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
    let graph_bytes = read_small(graph_path, MAX_SOURCE_GRAPH_BYTES as u64)?;
    let graph = SourceGraph::from_json_text(std::str::from_utf8(&graph_bytes)?)?;
    write_graph(
        &graph,
        sequence_id,
        donor_path,
        false,
        destination,
        tools,
        || {
            if read_small(graph_path, MAX_SOURCE_GRAPH_BYTES as u64)? != graph_bytes {
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
    if plan.sources.len() > 1 {
        if donor_path.is_some() {
            return Err("mixed-source links require --linked-originals".into());
        }
        #[cfg(target_os = "macos")]
        return mixed::write(&plan, destination, tools, recheck_source);
        #[cfg(not(target_os = "macos"))]
        return Err("mixed-source linked bin output currently requires macOS".into());
    }
    let source = &plan.sources[0];
    if source.kind == SourceKind::ManagedAtom {
        if donor_path.is_some() {
            return Err("managed OP-Atom links require --linked-originals".into());
        }
        #[cfg(target_os = "macos")]
        return managed_atom::write(&plan, destination, tools, recheck_source);
        #[cfg(not(target_os = "macos"))]
        return Err("managed OP-Atom linked bin output currently requires macOS".into());
    }
    if donor_path.is_some() && matches!(source.kind, SourceKind::AvcAac { .. }) {
        return Err("embedded AVC/AAC links require the original-file writer".into());
    }
    if donor_path.is_some() && !source.picture_markers.is_empty() {
        return Err(
            "picture markers require a newly created Avid master; use --linked-originals".into(),
        );
    }
    let donor_bytes = donor_path
        .map(|path| read_small(path, MAX_DONOR_BYTES))
        .transpose()?;
    let prepared_donor = if let Some(bytes) = &donor_bytes {
        let doc = AvbDocument::from_bytes(bytes.clone())?;
        let writer = native_bin::inspect(&doc, &source.routes, plan.rate)?.prepare_write();
        Some((doc, writer))
    } else {
        None
    };
    let mut originals = vec![source.routes[0].path];
    if source.kind == SourceKind::Separate {
        originals.push(source.routes[1].path);
    }
    let stamps = originals
        .iter()
        .map(|path| rewrap::source::stamp(Path::new(path)))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let avc = if matches!(source.kind, SourceKind::AvcAac { .. }) {
        Some(verify::avc_aac(source, plan.rate, tools)?)
    } else {
        for (route, picture) in source.routes.iter().zip([true, false]) {
            verify::original(route, plan.rate, picture, donor_path.is_none(), tools)?;
        }
        None
    };
    let doc = if let Some((mut doc, writer)) = prepared_donor {
        writer.apply(&mut doc, &plan)?;
        doc
    } else {
        original::build(&plan, avc.as_ref())?
    };
    let readback = AvbDocument::from_bytes(doc.to_bytes()?)?;
    if let Some(avc) = avc.as_ref() {
        mp4::check(&readback, &plan, avc)?;
    } else {
        native_bin::inspect(&readback, &source.routes, plan.rate)?.check(&readback, &plan)?;
    }
    recheck_source()?;
    if let Some((path, bytes)) = donor_path.zip(donor_bytes.as_ref()) {
        if read_small(path, MAX_DONOR_BYTES)? != *bytes {
            return Err("donor changed during linked bin preparation".into());
        }
    }
    for (path, stamp) in originals.into_iter().zip(stamps) {
        if rewrap::source::stamp(Path::new(path))? != stamp {
            return Err(format!("{path} changed during linked bin preparation").into());
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
