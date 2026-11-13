//! Internal Resolve source harness, separate from the product CLI.
use anynle_resolve_adapter::source::{self, conform};
use std::{ffi::OsString, fs, path::Path};
use tempfile::NamedTempFile;

mod support;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let [mode, left, left_id, right, right_id, output, options @ ..] = args.as_slice() {
        if mode == "--conform" {
            let options = conform_options(options)?;
            return publish(Path::new(output), |file| {
                conform::write(
                    Path::new(left),
                    text(left_id)?,
                    Path::new(right),
                    text(right_id)?,
                    options,
                    file,
                )
            });
        }
    }
    match args.as_slice() {
        [mode, input, id, ffprobe, output, flags @ ..] if mode == "--premiere" => {
            let accept_metadata_losses = match flags {
                [] => false,
                [flag] if flag == "--accept-losses" => true,
                _ => return Err("expected only --accept-losses after the Premiere output path".into()),
            };
            let graph = graph::source::SourceGraph::from_json_text(&support::read(
                Path::new(input), graph::MAX_GRAPH_BYTES,
            )?)?;
            for loss in anynle_resolve_adapter::premiere_export::write_with_options(
                graph, text(id)?, Path::new(ffprobe), Path::new(output), accept_metadata_losses,
            )? {
                eprintln!("{} ({}): {}", loss.object, loss.aspect, loss.reason);
            }
            Ok(())
        }
        [mode, input, output] if mode == "--inventory-effects" => {
            publish(Path::new(output), |file| {
                source::inventory::write_effects(Path::new(input), file)
            })
        }
        [mode, input, output] if mode == "--inspect-archive" => {
            publish(Path::new(output), |file| {
                source::inventory::write_archive(Path::new(input), file)
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
        _ => Err("usage: resolve-source-probe INPUT.drp OUTPUT.graph.json\n       resolve-source-probe --inspect-archive INPUT OUTPUT.json\n       resolve-source-probe --inspect-drt INPUT.drt OUTPUT.json\n       resolve-source-probe --graph-drt INPUT.drt OUTPUT.graph.json\n       resolve-source-probe --diff LEFT LEFT_TIMELINE_ID RIGHT RIGHT_TIMELINE_ID OUTPUT.json\n       resolve-source-probe --diff-blob LEFT LEFT_OBJECT_ID RIGHT RIGHT_OBJECT_ID FIELD OUTPUT.json\n       resolve-source-probe --inventory-effects INPUT OUTPUT.json\n       resolve-source-probe --premiere INPUT.graph.json SEQUENCE_ID FFPROBE OUTPUT.prproj [--accept-losses]\n       resolve-source-probe --conform REFERENCE REFERENCE_ID CANDIDATE CANDIDATE_ID OUTPUT.json [--all-frames] [--moves MOVES.json [--checksums MANIFEST.json]] [--renders REFERENCE.mov CANDIDATE.mov FFMPEG FFPROBE]".into()),
    }
}

fn conform_options(
    mut args: &[OsString],
) -> Result<conform::Options<'_>, Box<dyn std::error::Error>> {
    let mut options = conform::Options::default();
    let mut moves = None;
    let mut checksums = None;
    while !args.is_empty() {
        args = match args {
            [flag, rest @ ..] if flag == "--all-frames" && !options.all_frames => {
                options.all_frames = true;
                rest
            }
            [flag, path, rest @ ..] if flag == "--moves" && moves.is_none() => {
                moves = Some(Path::new(path));
                rest
            }
            [flag, path, rest @ ..] if flag == "--checksums" && checksums.is_none() => {
                checksums = Some(Path::new(path));
                rest
            }
            [flag, reference, candidate, ffmpeg, ffprobe, rest @ ..]
                if flag == "--renders" && options.renders.is_none() =>
            {
                options.renders = Some(conform::Renders {
                    reference: Path::new(reference),
                    candidate: Path::new(candidate),
                    ffmpeg: Path::new(ffmpeg),
                    ffprobe: Path::new(ffprobe),
                });
                rest
            }
            _ => return Err("invalid or repeated conform option".into()),
        };
    }
    if checksums.is_some() && moves.is_none() {
        return Err("--checksums requires --moves".into());
    }
    options.moves = moves.map(|moves| conform::MoveOptions { moves, checksums });
    Ok(options)
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
