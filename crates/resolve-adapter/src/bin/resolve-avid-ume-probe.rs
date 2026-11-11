//! Internal Avid UME to Resolve qualification harness.
use anynle_resolve_adapter::convert_avid_ume;
use avid_adapter::original::{OriginalOptions, RootMapping};
use std::{ffi::OsString, path::Path};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let [bin, sequence, layout, profile, ffprobe, output, roots @ ..] = args.as_slice() else {
        return Err("usage: resolve-avid-ume-probe BIN SEQUENCE LAYOUT.drp RATE.drp FFPROBE OUTPUT.drp [SAVED_ROOT CURRENT_ROOT ...]".into());
    };
    if !roots.chunks_exact(2).remainder().is_empty() {
        return Err("each saved root requires a current root".into());
    }
    let options = OriginalOptions {
        ffprobe: ffprobe.into(),
        root_mappings: roots
            .chunks_exact(2)
            .map(|pair| RootMapping {
                saved_root: (&pair[0]).into(),
                current_root: (&pair[1]).into(),
            })
            .collect(),
    };
    let result = convert_avid_ume(
        Path::new(bin),
        sequence.to_str().ok_or("invalid sequence selector")?,
        &options,
        Path::new(layout),
        Path::new(profile),
        Path::new(output),
    )?;
    for notice in result.notices {
        eprintln!("{notice}");
    }
    for marker in result.omitted_markers {
        eprintln!("omitted marker occurrence {marker}");
    }
    if let Some(warning) = result.cleanup_warning {
        eprintln!("{warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve Avid UME probe: {error}");
        std::process::exit(1);
    }
}
