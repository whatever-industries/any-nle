//! Internal Resolve source harness, separate from the product CLI.
use anynle_resolve_adapter::source;
use std::{ffi::OsString, fs, path::Path};
use tempfile::NamedTempFile;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [input, output] = args.as_slice() else {
        return Err("usage: resolve-source-probe INPUT.drp OUTPUT.graph.json".into());
    };
    let output = Path::new(output);
    let graph = source::read(Path::new(input))?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut stage = NamedTempFile::new_in(parent)?;
    graph.write_json(stage.as_file_mut())?;
    stage.as_file().sync_all()?;
    fs::hard_link(stage.path(), output)?;
    let staged = stage.path().display().to_string();
    if let Err(error) = stage.close() {
        eprintln!(
            "published {}, retained staging file {staged}: {error}",
            output.display()
        );
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve source probe: {error}");
        std::process::exit(1);
    }
}
