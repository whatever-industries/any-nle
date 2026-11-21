#![allow(dead_code)]
//! Shared paths for the byte checks. Fixtures live outside the repository;
//! tests that need one skip (with a note) when it is absent.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Gzip (deflate) bytes are compared with Python's only where the crate links
/// the same zlib as the oracle (macOS system libz 1.2.12). Elsewhere only the
/// decompressed XML identity is guaranteed.
pub const GZIP_BYTES_GUARANTEED: bool = cfg!(target_os = "macos");

/// Root that the paths in `tests/data/*.tsv` (`premiere-format-research/...`, `converter/...`)
/// are relative to. `PRPROJ_CORPUS_BASE` wins; otherwise it is derived from this checkout: in the
/// Any NLE layout `<workspace>/local/corpus-base/` (named links to `knowledge/premiere` and
/// `converter`), in the old layout the folder holding both repositories. Keep in step with
/// `tools/corpus_base.py`.
pub fn base() -> PathBuf {
    if let Some(b) = std::env::var_os("PRPROJ_CORPUS_BASE") {
        return PathBuf::from(b);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.canonicalize().unwrap_or(manifest).join("..");
    let workspace = repo.canonicalize().unwrap_or(repo).join("..");
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let linked = workspace.join("local/corpus-base");
    if linked.is_dir() {
        linked
    } else {
        workspace
    }
}

pub fn fixtures() -> PathBuf {
    base().join("premiere-format-research/local/fixtures/py-premiere-adb9e57/minimal")
}

pub fn golden() -> PathBuf {
    base().join("converter/local/golden/out")
}

pub fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

pub fn read_data(name: &str) -> String {
    std::fs::read_to_string(data(name)).unwrap()
}

/// `Some(bytes)` or a skip note.
pub fn fixture(path: PathBuf) -> Option<Vec<u8>> {
    match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("SKIP: fixture absent: {}", path.display());
            None
        }
    }
}

/// A test-owned scratch directory under TMPDIR. Keep its contents on panic.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(prefix: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::path::absolute(std::env::temp_dir()).unwrap();
        let ordinal = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = root.join(format!("{prefix}-{}-{stamp}-{ordinal}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Keep failed comparisons available for diagnosis.
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
