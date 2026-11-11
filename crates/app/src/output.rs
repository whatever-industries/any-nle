//! Shared CLI output checks. Snapshot creation also checks exclusively at write time.
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

fn exists(path: &Path) -> String {
    format!("{} exists; the converter never overwrites", path.display())
}

/// Key for a new file, resolving the existing parent so relative and symlink aliases match.
fn new_path_key(path: &Path) -> Result<PathBuf, String> {
    // Unlike Path::exists, this includes dangling symlinks. None may be replaced or followed.
    match fs::symlink_metadata(path) {
        Ok(_) => return Err(exists(path)),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "output folder {} does not exist or is not a directory",
            parent.display()
        ));
    }
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: output needs a file name", path.display()))?;
    fs::canonicalize(parent)
        .map(|p| p.join(name))
        .map_err(|e| format!("{}: {e}", parent.display()))
}

/// Refuse predictable output failures before a snapshot or project can be created.
pub fn preflight(project: &Path, graph: Option<&Path>) -> Result<(), String> {
    let project_key = new_path_key(project)?;
    if let Some(path) = graph {
        let graph_key = new_path_key(path)?;
        // Avoid case-only collisions on case-insensitive destination filesystems. Also
        // refuse these names on case-sensitive hosts so the same command stays portable.
        let same_name = graph_key
            .file_name()
            .and_then(|n| n.to_str())
            .zip(project_key.file_name().and_then(|n| n.to_str()))
            .is_some_and(|(a, b)| a.to_lowercase() == b.to_lowercase());
        if graph_key == project_key || (graph_key.parent() == project_key.parent() && same_name) {
            return Err(format!(
                "{}: graph snapshot and project output must use different paths (beyond letter case)",
                path.display()
            ));
        }
    }
    Ok(())
}

/// A preflight is not a reservation: another file can appear while bins are being read.
/// create_new refuses it atomically, including any symbolic link at the snapshot path.
pub fn write_snapshot(path: &Path, text: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == ErrorKind::AlreadyExists {
                exists(path)
            } else {
                format!("{}: {e}", path.display())
            }
        })?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) struct SnapshotWriteError {
    pub message: String,
    pub retained: bool,
}

/// Validate, then stream the graph. Only errors after exclusive creation retain a partial snapshot.
pub(crate) fn write_source_snapshot(
    path: &Path,
    graph: &graph::source::SourceGraph,
) -> Result<(), SnapshotWriteError> {
    graph.encoded_len().map_err(|e| SnapshotWriteError {
        message: e.to_string(),
        retained: false,
    })?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| SnapshotWriteError {
            message: format!("{}: {e}", path.display()),
            retained: false,
        })?;
    let mut out = std::io::BufWriter::new(file);
    graph
        .write_json(&mut out)
        .map_err(|e| e.to_string())
        .and_then(|_| out.flush().map_err(|e| e.to_string()))
        .map_err(|e| SnapshotWriteError {
            message: format!("partial source snapshot retained at {}: {e}", path.display()),
            retained: true,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_created_after_preflight_is_preserved_at_snapshot_write() {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "anynle-late-snapshot-{}-{time}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        let project = dir.join("project.prproj");
        let graph = dir.join("graph.json");
        preflight(&project, Some(&graph)).unwrap();
        // Another producer creates a file after the path checks, before our write.
        fs::write(&graph, b"late file").unwrap();
        assert!(write_snapshot(&graph, "replacement").is_err());
        let source = graph::source::SourceGraph::from_json_text(
            r#"{"schema":"anynle.graph/2","project":{"name":"Synthetic snapshot"},
                "media":[],"items":[],"bins":[],"sequences":[]}"#,
        )
        .unwrap();
        let failure = write_source_snapshot(&graph, &source).err().unwrap();
        assert!(!failure.retained);
        assert_eq!(fs::read(&graph).unwrap(), b"late file");
        assert!(!project.exists());
        fs::remove_dir_all(dir).unwrap(); // Only inspected, freshly created test files.
    }
}
