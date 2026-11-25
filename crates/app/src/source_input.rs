//! Revision pins are computed with bounded streaming I/O. The original bin stays in place.
use anynle_media_verification::source_file::open_regular;
use avid_reader::source::SourceInput;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OriginalStamp {
    pub(crate) path: PathBuf,
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity_and_change_time: (u64, u64, i64, i64),
}

pub(crate) fn original_stamp(path: &Path) -> Result<OriginalStamp, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{}: original is not a file", path.display()));
    }
    #[cfg(unix)]
    let identity_and_change_time = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    Ok(OriginalStamp {
        path: path.into(),
        len: metadata.len(),
        modified: metadata
            .modified()
            .map_err(|e| format!("{}: {e}", path.display()))?,
        #[cfg(unix)]
        identity_and_change_time,
    })
}

pub(crate) fn recheck_originals(originals: &[OriginalStamp]) -> Result<(), String> {
    recheck_originals_checked(originals, &mut || Ok(()))
}

pub(crate) fn recheck_originals_checked(
    originals: &[OriginalStamp],
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    for old in originals {
        check()?;
        if &original_stamp(&old.path)? != old {
            return Err(format!(
                "Source file changed after pre-flight: {}",
                old.path.display()
            ));
        }
    }
    Ok(())
}

fn revision(path: &Path) -> Result<String, String> {
    checked_revision(path, &mut || Ok(()))
}
/// Streaming revision reads remain cancellable without buffering a whole media file.
pub(crate) fn checked_revision(
    path: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<String, String> {
    let io_error = |e| format!("{}: {e}", path.display());
    let mut file = open_regular(path).map_err(io_error)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 << 10];
    loop {
        check().map_err(|e| format!("{}: {e}", path.display()))?;
        match file.read(&mut buffer) {
            Ok(0) => return Ok(prproj::hash::hex(&hash.finalize())),
            Ok(n) => hash.update(&buffer[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e)),
        }
    }
}
pub fn pin_checked(
    paths: &[PathBuf],
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Vec<SourceInput>, String> {
    paths.iter().map(|path| pin_file_checked(path, check)).collect()
}

pub(crate) fn pin_file_checked(
    path: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<SourceInput, String> {
    Ok(SourceInput {
        path: path.to_owned(),
        revision: checked_revision(path, check)?,
    })
}

pub fn recheck_checked(
    inputs: &[SourceInput],
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    inputs.iter().try_for_each(|input| recheck_file_checked(input, check))
}

pub(crate) fn recheck_file_checked(
    input: &SourceInput,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if checked_revision(&input.path, check)? != input.revision {
        return Err(format!(
            "Source media changed since qualification: {}; retained outputs are preserved",
            input.path.display()
        ));
    }
    Ok(())
}

pub(crate) fn recheck_file_and_stamp(
    input: &SourceInput,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<OriginalStamp, String> {
    let stamp = original_stamp(&input.path)?;
    recheck_file_checked(input, check)?;
    Ok(stamp)
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
    fn revision_matches_existing_digest_and_can_cancel_between_chunks() {
        let path = tempfile_path("streaming");
        let data: Vec<u8> = (0..((64 << 10) + 33)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let expected = prproj::hash::hex(&prproj::hash::sha256(&data));
        assert_eq!(revision(&path).unwrap(), expected);
        let mut calls = 0;
        let result = pin_checked(std::slice::from_ref(&path), &mut || {
            calls += 1;
            if calls == 2 {
                Err("cancelled test".into())
            } else {
                Ok(())
            }
        });
        assert!(result.unwrap_err().contains("cancelled test"));
        assert_eq!(calls, 2);
        std::fs::remove_file(&path).unwrap();
    }

    fn tempfile_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "anynle-source-pin-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn changed_bin_refuses_and_original_is_not_written() {
        let path = tempfile_path("changed-bin");
        std::fs::write(&path, b"first").unwrap();
        let inputs = pin(&[path.clone()]).unwrap();
        recheck(&inputs).unwrap();
        std::fs::write(&path, b"other").unwrap();
        assert!(recheck(&inputs).unwrap_err().contains("changed"));
        assert_eq!(std::fs::read(&path).unwrap(), b"other");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn directory_is_refused_before_hashing() {
        let path = tempfile_path("directory");
        fs::create_dir(&path).unwrap();
        let result = revision(&path);
        fs::remove_dir(&path).unwrap();
        assert_eq!(
            result.unwrap_err(),
            format!("{}: expected a regular source file", path.display())
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_file_hashes_but_symlink_to_pipe_does_not_wait() {
        use std::{os::unix::fs::symlink, process::Command, sync::mpsc, thread, time::Duration};

        let dir = tempfile_path("link");
        fs::create_dir(&dir).unwrap();
        let file = dir.join("media");
        let pipe = dir.join("pipe");
        let link = dir.join("original");
        assert!(Command::new("mkfifo").arg(&pipe).status().unwrap().success());
        fs::write(&file, b"media").unwrap();
        symlink(&file, &link).unwrap();
        assert_eq!(revision(&link).unwrap(), revision(&file).unwrap());
        fs::remove_file(&link).unwrap();
        symlink(&pipe, &link).unwrap();
        let (send, receive) = mpsc::channel();
        let path = link.clone();
        thread::spawn(move || send.send(revision(&path)).ok());
        let result = receive.recv_timeout(Duration::from_secs(5));
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            result.expect("checking a named pipe blocked").unwrap_err(),
            format!("{}: expected a regular source file", link.display())
        );
    }
}
