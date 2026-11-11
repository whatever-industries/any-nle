//! Internal Avid-to-Resolve qualification harness.
use anynle_resolve_adapter::convert_avid_atom;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [bin, sequence, skeleton, ffprobe, output, roots @ ..] = args.as_slice() else {
        return Err("usage: resolve-avid-probe BIN.avb SEQUENCE SKELETON.drp FFPROBE OUTPUT.drp MEDIA_ROOT...".into());
    };
    let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
    let result = convert_avid_atom(
        Path::new(bin),
        &roots,
        sequence.to_str().ok_or("invalid Avid sequence selector")?,
        Path::new(skeleton),
        Path::new(ffprobe),
        Path::new(output),
    )?;
    for notice in result.notices {
        eprintln!("{notice}");
    }
    for marker in result.omitted_markers {
        println!("omitted marker occurrence {marker}");
    }
    if let Some(warning) = result.cleanup_warning {
        eprintln!("{warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve Avid probe: {error}");
        std::process::exit(1);
    }
}
