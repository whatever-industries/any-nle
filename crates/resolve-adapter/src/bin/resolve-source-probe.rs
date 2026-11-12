//! Internal Resolve source harness, separate from the product CLI.
use anynle_resolve_adapter::source;
use std::{ffi::OsString, fs, path::Path};
use tempfile::NamedTempFile;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [mode, input, output] if mode == "--inventory-effects" => {
            publish(Path::new(output), |file| {
                source::inventory::write_effects(Path::new(input), file)
            })
        }
        [mode, left, left_id, right, right_id, output] if mode == "--diff" => {
            let (left_id, right_id) = (text(left_id)?, text(right_id)?);
            publish(Path::new(output), |file| {
                source::compare::timelines(Path::new(left), left_id, Path::new(right), right_id, file)
            })
        }
        [mode, left, left_id, right, right_id, field, output] if mode == "--diff-blob" => {
            let (left_id, right_id, field) = (text(left_id)?, text(right_id)?, text(field)?);
            publish(Path::new(output), |file| {
                source::compare::blobs(Path::new(left), left_id, Path::new(right), right_id, field, file)
            })
        }
        [mode, input, output] if mode == "--inspect-drt" => {
            let inspection = source::drt::inspect(Path::new(input))?;
            publish(Path::new(output), |file| inspection.write_json(file))
        }
        [mode, input, output] if mode == "--graph-drt" => {
            let graph = source::drt::inspect(Path::new(input))?.into_graph()?;
            publish(Path::new(output), |file| Ok(graph.write_json(file)?))
        }
        [input, output] if !input.to_string_lossy().starts_with("--") => {
            let input = Path::new(input);
            if input.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("drt")) {
                return Err("use --inspect-drt or --graph-drt for a Resolve timeline export".into());
            }
            let graph = source::read(input)?;
            publish(Path::new(output), |file| Ok(graph.write_json(file)?))
        }
        _ => Err("usage: resolve-source-probe INPUT.drp OUTPUT.graph.json\n       resolve-source-probe --inspect-drt INPUT.drt OUTPUT.json\n       resolve-source-probe --graph-drt INPUT.drt OUTPUT.graph.json\n       resolve-source-probe --diff LEFT LEFT_TIMELINE_ID RIGHT RIGHT_TIMELINE_ID OUTPUT.json\n       resolve-source-probe --diff-blob LEFT LEFT_OBJECT_ID RIGHT RIGHT_OBJECT_ID FIELD OUTPUT.json\n       resolve-source-probe --inventory-effects INPUT OUTPUT.json".into()),
    }
}

fn text(arg: &std::ffi::OsStr) -> Result<&str, Box<dyn std::error::Error>> {
    arg.to_str()
        .ok_or_else(|| "Resolve comparison selectors must be UTF-8".into())
}

fn publish(
    output: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut stage = NamedTempFile::new_in(parent)?;
    write(stage.as_file_mut())?;
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
