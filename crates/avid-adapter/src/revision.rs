//! Streaming SHA-256 revisions for linked media and verified stills.
use anynle_media_verification::source_file::open_regular;
use sha2::{Digest, Sha256};
use std::io::{self, Read};

/// Hash the complete reader without retaining input bytes or accepting partial digests.
pub(super) fn sha256_reader(mut reader: impl Read) -> io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(hash.finalize().into()),
            Ok(n) => hash.update(&buffer[..n]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

pub fn file_revision_checked(
    path: &std::path::Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> std::io::Result<String> {
    struct Checked<'a> {
        file: std::io::BufReader<std::fs::File>,
        check: &'a mut dyn FnMut() -> Result<(), String>,
        since: usize,
    }
    impl std::io::Read for Checked<'_> {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            if self.since >= 64 * 1024 {
                (self.check)().map_err(std::io::Error::other)?;
                self.since = 0;
            }
            let n = std::io::Read::read(&mut self.file, b)?;
            self.since += n;
            Ok(n)
        }
    }
    check().map_err(std::io::Error::other)?;
    let file = open_regular(path)?;
    let digest = sha256_reader(Checked {
        file: std::io::BufReader::with_capacity(64 * 1024, file),
        check,
        since: 0,
    })?;
    Ok(hex::encode(digest))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn hex(d: [u8; 32]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn original_content_reads_can_cancel_between_chunks() {
        let p = std::env::temp_dir().join(format!("anynle-revision-cancel-{}", std::process::id()));
        std::fs::write(&p, vec![0u8; 256 * 1024]).unwrap();
        let mut calls = 0;
        let err = file_revision_checked(&p, &mut || {
            calls += 1;
            if calls >= 2 {
                Err("cancelled hash".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(err.to_string().contains("cancelled hash"));
        assert_eq!(calls, 2);
        std::fs::remove_file(p).unwrap();
    }
    #[test]
    fn streaming_revision_has_standard_vectors_and_propagates_failures() {
        assert_eq!(
            hex(sha256_reader(&b""[..]).unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(sha256_reader(&b"abc"[..]).unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(sha256_reader(vec![b'a'; 1_000_000].as_slice()).unwrap()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        struct Short {
            bytes: &'static [u8],
            interrupt: bool,
        }
        impl std::io::Read for Short {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if !self.interrupt {
                    self.interrupt = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                if self.bytes.is_empty() {
                    return Ok(0);
                }
                out[0] = self.bytes[0];
                self.bytes = &self.bytes[1..];
                Ok(1)
            }
        }
        assert_eq!(
            hex(sha256_reader(Short {
                bytes: b"abc",
                interrupt: false
            })
            .unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        struct Failed;
        impl std::io::Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::Other.into())
            }
        }
        assert!(sha256_reader(Failed).is_err());
    }
}
