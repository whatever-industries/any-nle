//! Internal qualification harness for two-camera, fixed-audio Avid Groups.
use anynle_resolve_adapter::{prepare_group, publish_group, verify_group_binding, BindingSpec};
use graph::{source::SourceGraph, MAX_GRAPH_BYTES};
use std::{ffi::OsString, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

mod support;
use support::read;

fn run() -> Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [graph, sequence, canvas, origin, a, b, skeleton, ffprobe, output] = args.as_slice() else {
        return Err("usage: resolve-group-probe GRAPH SEQUENCE-ID 1920x1080 ORIGIN|source A.json B.json SKELETON FFPROBE OUTPUT.drp".into());
    };
    if canvas != "1920x1080" {
        return Err("explicit 1920x1080 destination required".into());
    }
    let origin = origin.to_str().ok_or("invalid origin")?;
    let origin = if origin == "source" {
        None
    } else {
        Some(origin.parse()?)
    };
    let graph = SourceGraph::from_json_text(&read(Path::new(graph), MAX_GRAPH_BYTES)?)?;
    let verify = |arg| -> Result<_> {
        let spec: BindingSpec = serde_json::from_str(&read(Path::new(arg), 32 << 10)?)?;
        verify_group_binding(spec, Path::new(ffprobe))
    };
    let prepared = prepare_group(
        &graph,
        sequence.to_str().ok_or("invalid sequence ID")?,
        (1920, 1080),
        origin,
        [verify(a)?, verify(b)?],
    )?;
    let result = publish_group(&prepared, Path::new(skeleton), Path::new(output))?;
    for notice in prepared.notices() {
        println!("{notice}");
    }
    if let Some(warning) = result.cleanup_warning {
        eprintln!("{warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve Group probe: {error}");
        std::process::exit(1);
    }
}
