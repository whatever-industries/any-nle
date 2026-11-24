//! Read-only audit driver; caller supplies and verifies the exact input revision.
use std::path::PathBuf;
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.len() < 3 {
        return Err("usage: source_audit BIN SHA256 MEDIA_ROOT [MEDIA_ROOT...]".into());
    }
    let input = avid_reader::source::SourceInput {
        path: PathBuf::from(&a[0]),
        revision: a[1].clone(),
    };
    let name = input
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("input has no UTF-8 name")?
        .to_owned();
    let out = avid_adapter::read(
        &[input],
        &a[2..].iter().map(PathBuf::from).collect::<Vec<_>>(),
        "source audit",
        &[name],
    )?;
    eprintln!("items={} media={} sequences={} placements={} transitions={} markers={} files={} diagnostics={} routing_issues={} scan_issues={}",out.graph.items.len(),out.graph.media.len(),out.graph.sequences.len(),out.graph.sequences.iter().map(|s|s.placements.len()).sum::<usize>(),out.graph.sequences.iter().map(|s|s.transitions.len()).sum::<usize>(),out.graph.sequences.iter().map(|s|s.markers.len()).sum::<usize>(),out.files_scanned,out.diagnostics.len(),out.audio_group_issues.len(),out.scan_issues.len());
    for d in out
        .diagnostics
        .iter()
        .chain(out.audio_group_issues.iter().map(|issue| &issue.message))
        .chain(&out.scan_issues)
    {
        eprintln!("{d}");
    }
    out.graph
        .write_json(&mut std::io::stdout())
        .map_err(|e| e.to_string())
}
