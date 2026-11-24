//! Internal media-only staging entry point.
use anynle_app::{avid_media, backend_bundle};
use std::{fs::File, io::Read, path::Path};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [mode, volume] if mode == "--plan-drive" => {
            println!("{}", serde_json::to_string_pretty(&avid_media::plan_drive(Path::new(volume))?)?);
        }
        [mode, request, options @ ..] if mode == "--roundtrip-avb"
            && (options.is_empty() || matches!(options, [flag] if flag == "--review")) =>
        {
            let report = avid_media::roundtrip_avb(Path::new(request), !options.is_empty())?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        [mode, request, stage, tools @ ..] if mode == "--stage" && matches!(tools.len(), 0 | 2) => {
            let mut bytes = Vec::new();
            File::open(request)?.take(65_537).read_to_end(&mut bytes)?;
            if bytes.len() > 65_536 { return Err("media staging request exceeds 64 KiB".into()); }
            let request = serde_json::from_slice(&bytes)?;
            let backend = backend_bundle::resolve(tools.first().map(Path::new), tools.get(1).map(Path::new), false)?;
            let receipt = avid_media::stage(&request, Path::new(stage), &backend)?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        [mode, manifest, folder, output, tools @ ..] if mode == "--master-bin" && matches!(tools.len(), 0 | 2) => {
            let backend = backend_bundle::resolve(tools.first().map(Path::new), tools.get(1).map(Path::new), false)?;
            avid_media::write_master_bin(Path::new(manifest), Path::new(folder), Path::new(output), &backend)?;
        }
        [mode, graph, sequence, donor, output, tools @ ..] if mode == "--linked-bin" && matches!(tools.len(), 0 | 2) => {
            let backend = backend_bundle::resolve(tools.first().map(Path::new), tools.get(1).map(Path::new), false)?;
            let sequence = sequence.to_str().ok_or("sequence ID must be UTF-8")?;
            avid_media::write_linked_bin(Path::new(graph), sequence, Some(Path::new(donor)), Path::new(output), &backend)?;
        }
        [mode, graph, sequence, output, tools @ ..] if mode == "--linked-originals" && matches!(tools.len(), 0 | 2) => {
            let backend = backend_bundle::resolve(tools.first().map(Path::new), tools.get(1).map(Path::new), false)?;
            let sequence = sequence.to_str().ok_or("sequence ID must be UTF-8")?;
            avid_media::write_linked_bin(Path::new(graph), sequence, None, Path::new(output), &backend)?;
        }
        #[cfg(target_os = "macos")]
        [mode, project, sequence, output, tools @ ..] if mode == "--linked-project" && matches!(tools.len(), 0 | 2) => {
            let backend = backend_bundle::resolve(tools.first().map(Path::new), tools.get(1).map(Path::new), false)?;
            let sequence = sequence.to_str().ok_or("sequence ID must be UTF-8")?;
            avid_media::write_linked_project(Path::new(project), sequence, Path::new(output), &backend)?;
        }
        _ => return Err("usage: avid-media-probe --plan-drive VOLUME\n       avid-media-probe --roundtrip-avb REQUEST.json [--review]\n       avid-media-probe --stage REQUEST.json NEW_STAGING_FOLDER [FFMPEG FFPROBE]\n       avid-media-probe --master-bin STAGE.json INDEXED_MXF_FOLDER NEW.avb [FFMPEG FFPROBE]\n       avid-media-probe --linked-bin GRAPH.json SEQUENCE_ID DONOR.avb NEW.avb [FFMPEG FFPROBE]\n       avid-media-probe --linked-originals GRAPH.json SEQUENCE_ID NEW.avb [FFMPEG FFPROBE]\n       avid-media-probe --linked-project INPUT SEQUENCE_ID NEW.avb [FFMPEG FFPROBE] (macOS)".into()),
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Avid media staging: {error}");
            std::process::ExitCode::from(1)
        }
    }
}
