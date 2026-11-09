//! Enforce the existing AVB byte budget on the opened handle and during reading.
use crate::file::MAX_FILE_BYTES;
use crate::io::{Error, Result};
use std::io::{self, Read};
use std::path::Path;

pub(crate) fn read_avb(path: &Path) -> Result<Vec<u8>> {
    let io_error = |error| Error::new(format!("{}: {error}", path.display()), 0);
    let mut file = std::fs::File::open(path).map_err(io_error)?;
    // Check the same handle that will supply the bytes, rather than reopening a path
    // whose target could have changed since the metadata check.
    let size = file.metadata().map_err(io_error)?.len();
    if size > MAX_FILE_BYTES {
        return Err(Error::new(format!("file is {size} bytes; limit is {MAX_FILE_BYTES}"), 0));
    }
    read_limited(&mut file, MAX_FILE_BYTES as usize, size as usize).map_err(io_error)
}

fn read_limited(source: &mut impl Read, limit: usize, size_hint: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(size_hint.min(limit));
    let mut chunk = [0; 64 * 1024];
    loop {
        let remaining = limit - bytes.len();
        // One extra byte detects growth or a stream with unhelpful metadata. It
        // stays in the stack buffer, so crossing the limit cannot double the Vec.
        let request = chunk.len().min(remaining.saturating_add(1));
        let count = match source.read(&mut chunk[..request]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(bytes);
        }
        if count > remaining {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("file exceeds limit of {limit} bytes")));
        }
        let needed = bytes.len() + count;
        if needed > bytes.capacity() {
            // A stale/zero size hint must not cause a geometric capacity request
            // above the limit even when the accepted bytes themselves fit.
            let capacity = bytes.capacity().saturating_mul(2).max(needed).min(limit);
            bytes.reserve_exact(capacity - bytes.len());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stream {
        remaining: Option<usize>,
        consumed: usize,
        short: usize,
        interrupt: bool,
        fail_at: Option<usize>,
    }
    impl Stream {
        fn new(length: Option<usize>, short: usize) -> Self {
            Self {
                remaining: length,
                consumed: 0,
                short,
                interrupt: false,
                fail_at: None,
            }
        }
    }
    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.fail_at.is_some_and(|at| self.consumed >= at) {
                return Err(io::Error::new(io::ErrorKind::Other, "synthetic read failure"));
            }
            let count = buf.len().min(self.short).min(self.remaining.unwrap_or(usize::MAX));
            for (i, value) in buf[..count].iter_mut().enumerate() {
                *value = ((self.consumed + i) % 251) as u8;
            }
            self.consumed += count;
            if let Some(remaining) = &mut self.remaining {
                *remaining -= count;
            }
            Ok(count)
        }
    }

    #[test]
    fn short_and_interrupted_reads_preserve_bytes_up_to_exact_limit() {
        for length in [0, 1, 31, 65_536, 65_539] {
            for hint in [0, 1, length, usize::MAX] {
                let mut source = Stream::new(Some(length), 7);
                source.interrupt = true;
                let bytes = read_limited(&mut source, length, hint).unwrap();
                assert_eq!(bytes, (0..length).map(|i| (i % 251) as u8).collect::<Vec<_>>());
                assert_eq!(source.consumed, length);
                assert!(bytes.capacity() <= length, "unbounded capacity: {} > {length}", bytes.capacity());
            }
        }
    }

    #[test]
    fn endless_or_underreported_inputs_stop_after_limit_plus_one_byte() {
        for limit in [0, 1, 31, 65_536, 65_539] {
            for short in [7, usize::MAX] {
                let mut source = Stream::new(None, short);
                let error = read_limited(&mut source, limit, 0).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidData);
                assert_eq!(source.consumed, limit + 1);
            }
        }
    }

    #[test]
    fn a_shrinking_size_hint_is_not_an_exact_length_requirement() {
        let mut source = Stream::new(Some(5), 7);
        assert_eq!(read_limited(&mut source, 32, 32).unwrap(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn io_failure_never_returns_partial_success() {
        let mut source = Stream::new(Some(30), 7);
        source.fail_at = Some(7);
        let error = read_limited(&mut source, 32, 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(source.consumed, 7);
    }
}
