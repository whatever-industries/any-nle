//! Internal qualification harness for multi-timeline linked projects.
use anynle_resolve_adapter::{linked, publish_linked_with_templates, turnover::Templates};
use graph::source::{SourceGraph, MAX_SOURCE_GRAPH_BYTES};
use std::{ffi::OsString, path::Path};

mod support;
use support::read;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let accept_losses = args.first().is_some_and(|arg| arg == "--accept-losses");
    if accept_losses {
        args.remove(0);
    }
    let mut fairlight = None;
    let mut nested = None;
    while args.len() >= 2 {
        let slot = match args[args.len() - 2].to_str() {
            Some("--fairlight") => &mut fairlight,
            Some("--nested") => &mut nested,
            _ => break,
        };
        if slot.is_some() {
            return Err("duplicate Resolve template option".into());
        }
        *slot = args.pop().map(Into::into);
        args.pop();
    }
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
        _ => return Err("usage: resolve-linked-probe [--accept-losses] GRAPH SPEC MONO-LAYOUT.drp RATE-PROFILE.drp FFPROBE OUTPUT.drp [ATOM-TEMPLATE.drp [GROUP-TEMPLATE.drp]] [--fairlight TWO-BUS.drp] [--nested NEST-TEMPLATE.drp]".into()),
    };
    let graph = SourceGraph::from_json_text(&read(Path::new(graph), MAX_SOURCE_GRAPH_BYTES)?)?;
    let spec = serde_json::from_str(&read(Path::new(spec), 2 << 20)?)?;
    let project = linked::prepare(&graph, spec, Path::new(probe))?;
    if std::env::var_os("ANYNLE_RESOLVE_IMPORT_TRACE").is_some() {
        let (direct, assisted) = project.media_counts();
        eprintln!("Media originals: {direct} standalone, {assisted} assisted");
    }
    for loss in project.transition_approximation_losses().chain(project.empty_nest_audio_losses())
        .chain(project.stereo_nest_edge_losses()).chain(project.color_adjust_losses())
    {
        eprintln!("{} ({}): {}", loss.object, loss.aspect, loss.reason);
    }
    if !accept_losses && project.transition_approximation_losses().next().is_some() {
        return Err("Resolve transition approximations require --accept-losses".into());
    }
    if !accept_losses && project.empty_nest_audio_losses().next().is_some() {
        return Err("Resolve empty nest audio omissions require --accept-losses".into());
    }
    if !accept_losses && project.stereo_nest_edge_losses().next().is_some() {
        return Err("Resolve stereo nest audio edge approximations require --accept-losses".into());
    }
    if !accept_losses && project.color_adjust_losses().next().is_some() {
        return Err("Resolve Color Mode Adjust approximations require --accept-losses".into());
    }
    let published = publish_linked_with_templates(
        &project,
        &Templates {
            layout: layout.into(),
            profile: profile.into(),
            atom: atom.map(Path::to_owned),
            group: group.map(Path::to_owned),
            fairlight,
            nested,
        },
        Path::new(output),
    )?;
    for marker in project.omitted_markers() {
        println!("omitted marker occurrence {marker}");
    }
    for loss in project.marker_ownership_losses() {
        println!("{} ({}): {}", loss.object, loss.aspect, loss.reason);
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
