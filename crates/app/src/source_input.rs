//! Revision pins are computed with bounded streaming I/O. The original bin stays in place.
use avid_reader::source::SourceInput;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

fn revision(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    prproj::hash::sha256_reader(BufReader::with_capacity(64 << 10, file))
        .map(|h| prproj::hash::hex(&h))
        .map_err(|e| format!("{}: {e}", path.display()))
}
/// Streaming revision reads remain cancellable without buffering a whole media file.
fn checked_revision(
    path: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<String, String> {
    struct Checked<'a> {
        file: BufReader<File>,
        check: &'a mut dyn FnMut() -> Result<(), String>,
    }
    impl std::io::Read for Checked<'_> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            (self.check)().map_err(std::io::Error::other)?;
            std::io::Read::read(&mut self.file, b)
        }
    }
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    prproj::hash::sha256_reader(Checked {
        file: BufReader::with_capacity(64 << 10, file),
        check,
    })
    .map(|h| prproj::hash::hex(&h))
    .map_err(|e| format!("{}: {e}", path.display()))
}
pub fn pin_checked(
    paths: &[PathBuf],
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Vec<SourceInput>, String> {
    paths
        .iter()
        .map(|path| {
            Ok(SourceInput {
                path: path.clone(),
                revision: checked_revision(path, check)?,
            })
        })
        .collect()
}
pub fn recheck_checked(
    inputs: &[SourceInput],
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    for i in inputs {
        if checked_revision(&i.path, check)? != i.revision {
            return Err(format!(
                "Source media changed since qualification: {}; retained outputs are preserved",
                i.path.display()
            ));
        }
    }
    Ok(())
}
pub fn pin(paths: &[PathBuf]) -> Result<Vec<SourceInput>, String> {
    paths
        .iter()
        .map(|path| {
            Ok(SourceInput {
                path: path.clone(),
                revision: revision(path)?,
            })
        })
        .collect()
}
pub fn recheck(inputs: &[SourceInput]) -> Result<(), String> {
    for input in inputs {
        if revision(&input.path)? != input.revision {
            return Err(format!("source bin changed since interpretation: {}; conversion stopped; existing outputs retained",input.path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_bin_refuses_and_original_is_not_written() {
        let path = std::env::temp_dir().join(format!(
            "anynle-source-pin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"first").unwrap();
        let inputs = pin(&[path.clone()]).unwrap();
        recheck(&inputs).unwrap();
        std::fs::write(&path, b"other").unwrap();
        assert!(recheck(&inputs).unwrap_err().contains("changed"));
        assert_eq!(std::fs::read(&path).unwrap(), b"other");
        std::fs::remove_file(path).unwrap();
    }
}
