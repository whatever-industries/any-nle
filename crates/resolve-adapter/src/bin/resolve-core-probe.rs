//! Internal qualification harness. This binary is outside the product CLI/GUI.
use anynle_resolve_adapter::{
    prepare, publish, verify_atom_binding, verify_binding, AtomBindingSpec, BindingSpec,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged)]
enum MediaBinding {
    Op1a(BindingSpec),
    OpAtom(AtomBindingSpec),
}
use graph::{source::SourceGraph, MAX_GRAPH_BYTES};
use std::{ffi::OsString, path::Path};

mod support;
use support::read;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [graph, sequence, canvas, binding, skeleton, ffprobe, output] = args.as_slice() else {
        return Err("usage: resolve-core-probe GRAPH SEQUENCE-ID 1920x1080 BINDING SKELETON FFPROBE OUTPUT.drp".into());
    };
    if canvas != "1920x1080" {
        return Err("explicit 1920x1080 destination required".into());
    }
    let graph = SourceGraph::from_json_text(&read(Path::new(graph), MAX_GRAPH_BYTES)?)?;
    let binding = match serde_json::from_str(&read(Path::new(binding), 32 << 10)?)? {
        MediaBinding::Op1a(spec) => verify_binding(spec, Path::new(ffprobe))?,
        MediaBinding::OpAtom(spec) => verify_atom_binding(spec, Path::new(ffprobe))?,
    };
    let prepared = prepare(
        &graph,
        sequence.to_str().ok_or("invalid sequence ID")?,
        (1920, 1080),
        binding,
    )?;
    let result = publish(&prepared, Path::new(skeleton), Path::new(output))?;
    for marker in prepared.omitted_markers() {
        println!("omitted marker occurrence {marker}");
    }
    if let Some(warning) = result.cleanup_warning {
        eprintln!("{warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve core probe: {error}");
        std::process::exit(1);
    }
}
