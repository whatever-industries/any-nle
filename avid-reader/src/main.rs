use std::path::{Path, PathBuf};
use std::process::ExitCode;

use avid_reader::extract::{extract_project_paths, format_timecode, BinContents};
use avid_reader::json::Json;
use avid_reader::model::{build, MediaIndex};
use avid_reader::{dump::Dumper, mxf, AvbFile};

const USAGE: &str = "usage:
  avid-reader dump BIN.avb                      pyavb-shaped dump of the bin's mobs
  avid-reader mxf FILE.mxf                      package UMIDs from an MXF header
  avid-reader sequences BIN.avb...              sequences, events and problems (text)
  avid-reader model BIN.avb... --media DIR [--media DIR...] [--project NAME] [--bin-mode separate|merged|grouped]
                                                avid2prproj.model/1 JSON on stdout, problems on stderr";

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("avid-reader: {}", message);
    ExitCode::from(1)
}

fn usage() -> ExitCode {
    eprintln!("{}", USAGE);
    ExitCode::from(2)
}

fn selected_bins(paths: &[String]) -> Vec<PathBuf> {
    let selected = avid_reader::inputs::unique_bins(paths);
    for path in selected.skipped {
        eprintln!("avid-reader: repeated bin path skipped: {}", path.display());
    }
    selected.paths
}

fn kind(k: i64) -> &'static str {
    if k == 1 {
        "V"
    } else {
        "A"
    }
}

fn print_sequences(contents: &BinContents) {
    let names: std::collections::HashMap<_, _> = contents.masters.iter().map(|m| (m.mob_id, m.name.clone())).collect();
    for s in &contents.sequences {
        let tc = s.start.as_ref().map(|t| format_timecode(t.frames, t.fps)).unwrap_or_default();
        println!("SEQUENCE {:?} rate {} length {} start {}", s.name, s.edit_rate, s.length, tc);
        for e in &s.events {
            let speed = match e.speed {
                Some((n, d)) => format!(" for {} source frames, speed_ratio {}/{}", e.source_length, n, d),
                None => String::new(),
            };
            let mut via = if e.via.is_empty() { String::new() } else { format!("  via {}", e.via.join("; ")) };
            if e.edit_gain_db.is_some() || e.edit_pan.is_some() {
                via = format!("  [edit gain {:?} dB, pan {:?}]{}", e.edit_gain_db.map(|g| (g * 100.0).round() / 100.0), e.edit_pan, via);
            }
            if let Some((g, c)) = e.combined {
                via = format!("  [multichannel edit {} channel {}]{}", g, c + 1, via);
            }
            println!(
                "  {}{} rec {}..{} <- {:?} {}{} in {}{}{}",
                kind(e.kind),
                e.track,
                e.record_start,
                e.record_start + e.length,
                names.get(&e.master).cloned().unwrap_or_else(|| e.master.urn()),
                kind(e.kind),
                e.master_track,
                e.source_in,
                speed,
                via
            );
        }
        println!("  audio track formats (label, _TRACK_FORMAT): {:?}", s.audio_track_formats);
        for t in &s.transitions {
            println!("  transition {}{} at {} len {} cut {} {:?}", kind(t.kind), t.track, t.position, t.length, t.cutpoint, t.effect);
        }
        for p in &s.problems {
            println!("  PROBLEM {}", p);
        }
    }
    for k in &contents.skipped {
        println!("SKIPPED {:?}: {}", k.name, k.reason);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((c, rest)) if !rest.is_empty() => (c.as_str(), rest),
        _ => return usage(),
    };
    let out = match command {
        "dump" if rest.len() == 1 => match AvbFile::open(Path::new(&rest[0])) {
            Ok(file) => Dumper::new(&file).mobs(),
            Err(e) => return fail(format_args!("{}: {}", rest[0], e)),
        },
        "mxf" if rest.len() == 1 => match mxf::read_identity(Path::new(&rest[0])) {
            Ok(id) => {
                let list = |v: &[String]| Json::Array(v.iter().map(|s| Json::str(s.clone())).collect());
                Json::obj()
                    .with("material_packages", list(&id.material_packages))
                    .with(
                        "picture_essence_coding",
                        id.picture_essence_coding
                            .map(|ul| Json::str(ul.iter().map(|b| format!("{:02X}", b)).collect::<String>()))
                            .unwrap_or(Json::Null),
                    )
                    .with("source_packages", list(&id.source_packages))
                    .with("file_packages", list(&id.file_packages))
                    .with(
                        "file_tracks",
                        Json::Array(
                            id.file_tracks
                                .iter()
                                .map(|(n, d, len)| Json::obj().with("rate", Json::str(format!("{}/{}", n, d))).with("duration", Json::Int(*len)))
                                .collect(),
                        ),
                    )
                    .with(
                        "timecodes",
                        Json::Array(
                            id.timecodes
                                .iter()
                                .map(|(f, b, d)| {
                                    Json::obj().with("start", Json::UInt(*f)).with("base", Json::Int(*b as i64)).with("drop", Json::Bool(*d))
                                })
                                .collect(),
                        ),
                    )
            }
            Err(e) => return fail(format_args!("{}: {}", rest[0], e)),
        },
        "sequences" => {
            let bins = selected_bins(rest);
            let contents = match extract_project_paths(&bins) {
                Ok(contents) => contents,
                Err(e) => return fail(e),
            };
            print_sequences(&contents);
            return ExitCode::SUCCESS;
        }
        "model" => {
            let mut media: Vec<String> = Vec::new();
            let mut project = None;
            let mut bin_mode = "separate".to_string();
            let mut bins = Vec::new();
            let mut it = rest.iter();
            while let Some(arg) = it.next() {
                if !arg.starts_with("--") {
                    bins.push(arg.clone());
                    continue;
                }
                let Some(value) = it.next().cloned() else { return usage() };
                match arg.as_str() {
                    "--media" => media.push(value),
                    "--project" => project = Some(value),
                    "--bin-mode" if ["separate", "merged", "grouped"].contains(&value.as_str()) => bin_mode = value,
                    _ => return usage(),
                }
            }
            if media.is_empty() {
                return usage();
            }
            if bins.is_empty() {
                return usage();
            }
            let bins = selected_bins(&bins);
            let contents = match extract_project_paths(&bins) {
                Ok(contents) => contents,
                Err(e) => return fail(e),
            };
            let roots: Vec<&Path> = media.iter().map(Path::new).collect();
            let index = match MediaIndex::scan_all(&roots) {
                Ok(i) => i,
                Err(e) => return fail(e),
            };
            let bin_names: Vec<String> = bins
                .iter()
                .map(|b| b.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
                .collect();
            let project = project.unwrap_or_else(|| bin_names[0].clone());
            let conversion = build(&contents, &index, &project, &bin_names, &bin_mode);
            eprintln!(
                "avid-reader: {} master clip(s), {} sequence(s) read; {} MXF file(s) scanned, {} unreadable, {} ambiguous identities",
                contents.masters.len(),
                contents.sequences.len(),
                index.files_scanned,
                index.unreadable.len(),
                index.ambiguous.len()
            );
            for (path, e) in &index.unreadable {
                eprintln!("  unreadable MXF {}: {}", path.display(), e);
            }
            for p in &conversion.problems {
                eprintln!("  {}", p);
            }
            conversion.model
        }
        _ => return usage(),
    };
    print!("{}", out.to_pretty());
    ExitCode::SUCCESS
}
