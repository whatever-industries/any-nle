//! Publication of an exclusively owned sibling. New-file publication never replaces.
use std::io;
use std::path::Path;

/// None means publication moved the stage. Some means a hard link published it and
/// stage cleanup failed; the target is nevertheless complete and must be retained.
pub(crate) fn publish_new(stage: &Path, target: &Path) -> io::Result<Option<String>> {
    if exclusive_rename(stage, target)? {
        return Ok(None);
    }
    publish_link(stage, target, |p| std::fs::remove_file(p))
}

fn publish_link(
    stage: &Path,
    target: &Path,
    cleanup: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<Option<String>> {
    // Same-directory stages stay on the target filesystem. hard_link is atomic
    // and refuses incumbent files, directories and dangling symbolic links.
    // Filesystems supporting neither primitive fail safely; no replacing fallback.
    std::fs::hard_link(stage, target)?;
    Ok(cleanup(stage).err().map(|e| {
        format!(
            "project published at {}; staging cleanup failed for {}: {e}",
            target.display(),
            stage.display()
        )
    }))
}

#[cfg(target_os = "macos")]
fn exclusive_rename(stage: &Path, target: &Path) -> io::Result<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn renamex_np(
            old: *const std::ffi::c_char,
            new: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    let old = CString::new(stage.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in staging path"))?;
    let new = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in project path"))?;
    // Darwin sys/stdio.h defines RENAME_EXCL as 4. Support depends on the filesystem.
    if unsafe { renamex_np(old.as_ptr(), new.as_ptr(), 4) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(22 | 45 | 78 | 102) => Ok(false), // EINVAL/ENOTSUP/ENOSYS/EOPNOTSUPP
        _ => Err(error),
    }
}

#[cfg(not(target_os = "macos"))]
fn exclusive_rename(_: &Path, _: &Path) -> io::Result<bool> {
    // std has no portable exclusive rename. Its no-replace hard-link primitive
    // above is safe on supporting filesystems on these hosts too.
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "anynle-publish-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn late_files_directories_and_symlinks_are_preserved() {
        let s = Scratch::new();
        let stage = s.0.join("stage");
        let target = s.0.join("target");
        std::fs::write(&stage, b"prepared").unwrap();
        std::fs::write(&target, b"incumbent").unwrap();
        assert!(publish_new(&stage, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"incumbent");
        std::fs::remove_file(&target).unwrap();
        std::fs::create_dir(&target).unwrap();
        assert!(publish_new(&stage, &target).is_err());
        assert!(target.is_dir());
        std::fs::remove_dir(&target).unwrap();
        #[cfg(unix)]
        {
            let absent = s.0.join("absent");
            std::os::unix::fs::symlink(&absent, &target).unwrap();
            assert!(publish_new(&stage, &target).is_err());
            assert!(std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(!absent.exists());
            std::fs::remove_file(&target).unwrap();
        }
        publish_new(&stage, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"prepared");
        assert!(!stage.exists());
    }
    #[test]
    fn successful_link_with_cleanup_failure_is_still_published() {
        // Exercise the portable publication/cleanup boundary independently of
        // macOS's rename fast path.
        let s = Scratch::new();
        let stage = s.0.join("stage");
        let target = s.0.join("target");
        std::fs::write(&stage, b"prepared").unwrap();
        let warning = publish_link(&stage, &target, |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected cleanup failure",
            ))
        })
        .unwrap()
        .unwrap();
        assert!(warning.contains("project published"));
        assert_eq!(std::fs::read(&target).unwrap(), b"prepared");
        assert!(stage.exists());
    }
}
