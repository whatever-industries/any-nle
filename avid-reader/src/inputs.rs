//! Filesystem selection shared by the reader and converter CLIs, before bin decoding.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct BinInputs {
    /// First selected spelling of each path, in selection order (also used for bin names).
    pub paths: Vec<PathBuf>,
    /// Repeated selections, in argument order, for the caller's report.
    pub skipped: Vec<PathBuf>,
}

/// Coalesce existing canonical paths, without merging distinct files by bytes or MobID.
/// Unresolvable paths remain inputs so the ordinary decoder reports their I/O errors.
pub fn unique_bins<P: AsRef<Path>>(paths: &[P]) -> BinInputs {
    let mut seen = HashSet::new();
    let mut result = BinInputs::default();
    for path in paths {
        let path = path.as_ref();
        if let Ok(canonical) = std::fs::canonicalize(path) {
            if !seen.insert(canonical) {
                result.skipped.push(path.to_path_buf());
                continue;
            }
        }
        result.paths.push(path.to_path_buf());
    }
    result
}
