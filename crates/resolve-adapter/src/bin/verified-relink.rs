//! Local, explicitly verified relocation of media in supported native projects.
use anynle_resolve_adapter::relink;
use std::{ffi::OsString, path::Path};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let published = match args.as_slice() {
        [command, project, manifest] if command == "capture" => {
            relink::capture(Path::new(project), None, Path::new(manifest))?
        }
        [command, project, moves, output, optional @ ..]
            if command == "apply" && optional.len() <= 1 =>
        {
            let moves = relink::read_moves(Path::new(moves))?;
            relink::apply(
                Path::new(project), &moves, Path::new(output), Path::new("ffprobe"),
                optional.first().map(Path::new),
            )?
        }
        _ => return Err(
            "usage: verified-relink capture PROJECT MANIFEST.json\n       verified-relink apply PROJECT MOVES.json OUTPUT [MANIFEST.json]"
                .into(),
        ),
    };
    if let Some(warning) = published.cleanup_warning {
        eprintln!("{warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("verified relink: {error}");
        std::process::exit(1);
    }
}
