use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Follow links to regular files and reject non-file inputs before reading.
pub fn open_regular(path: &Path) -> io::Result<File> {
    let non_file = || io::Error::new(io::ErrorKind::InvalidInput, "expected a regular source file");
    if !fs::metadata(path)?.is_file() {
        return Err(non_file());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        // The path could have become a FIFO since the metadata lookup.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(non_file());
    }
    Ok(file)
}
