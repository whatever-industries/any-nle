//! Internal qualification harness for multi-timeline linked projects.
use anynle_resolve_adapter::{linked, publish_linked_with_templates, turnover::Templates};
use graph::{source::SourceGraph, MAX_GRAPH_BYTES};
use std::{ffi::OsString, path::Path};

mod support;
use support::read;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let fairlight = if args.len() >= 2 && args[args.len() - 2] == "--fairlight" {
        let path = args.pop().map(Into::into);
        args.pop();
        path
    } else {
        None
    };
    let (graph, spec, layout, profile, probe, output, atom, group) = match args.as_slice() {
        [graph, spec, layout, profile, probe, output] => {
            (graph, spec, layout, profile, probe, output, None, None)
        }
        [graph, spec, layout, profile, probe, output, atom] => {
            (graph, spec, layout, profile, probe, output, Some(Path::new(atom)), None)
        }
        [graph, spec, layout, profile, probe, output, atom, group] => {
            (graph, spec, layout, profile, probe, output, Some(Path::new(atom)), Some(Path::new(group)))
        }
        _ => return Err("usage: resolve-linked-probe GRAPH SPEC MONO-LAYOUT.drp RATE-PROFILE.drp FFPROBE OUTPUT.drp [ATOM-TEMPLATE.drp [GROUP-TEMPLATE.drp]] [--fairlight TWO-BUS.drp]".into()),
    };
    let graph = SourceGraph::from_json_text(&read(Path::new(graph), MAX_GRAPH_BYTES)?)?;
    let spec = serde_json::from_str(&read(Path::new(spec), 2 << 20)?)?;
    let project = linked::prepare(&graph, spec, Path::new(probe))?;
    let published = publish_linked_with_templates(
        &project,
        &Templates {
            layout: layout.into(),
            profile: profile.into(),
            atom: atom.map(Path::to_owned),
            group: group.map(Path::to_owned),
            fairlight,
        },
        Path::new(output),
    )?;
    for marker in project.omitted_markers() {
        println!("omitted marker occurrence {marker}");
    }
    if let Some(id) = project.omitted_active_timeline() {
        println!("Saved active timeline {id} was omitted. Using the first exported timeline.");
    }
    if let Some(warning) = published.cleanup_warning {
        eprintln!("{warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve linked project probe: {error}");
        std::process::exit(1);
    }
}
