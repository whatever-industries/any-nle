//! UUIDs and the injectable UUID source.
//!
//! Python's writer calls `uuid.uuid4()` for ObjectUIDs, ClipIDs, FileKeys,
//! marker IDs and LUT IDs. Inside `avid2prproj.determinism.seeded_ids(seed)`
//! the n-th call (n from 1) returns
//! `uuid.UUID(bytes=sha256(f"{seed}:{n}").digest()[:16], version=4)`.
//! [`SeededUuids`] reproduces that sequence exactly; [`RandomUuids`] is the
//! normal-run source. Code that mints UUIDs takes a `&mut dyn UuidSource`, and
//! one source must be threaded through a whole write so the counter advances
//! in the same order as Python's global one.

use crate::hash::{hex, sha256};
use std::fmt;

/// A 128-bit UUID. `Display` gives Python's `str(uuid)` (lower-case, hyphenated).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    /// Python `uuid.UUID(bytes=b, version=4)`: sets the version nibble to 4
    /// and the RFC 4122 variant bits.
    pub fn from_bytes_v4(mut bytes: [u8; 16]) -> Uuid {
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Uuid(bytes)
    }

    /// The 16 raw bytes (Python `uuid.bytes`).
    pub fn bytes(&self) -> [u8; 16] {
        self.0
    }

    /// 32 lower-case hex digits, no hyphens (Python `uuid.hex`).
    pub fn hex(&self) -> String {
        hex(&self.0)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let h = self.hex();
        write!(f, "{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }
}

/// Where fresh version-4 UUIDs come from (Python `uuid.uuid4`).
pub trait UuidSource {
    /// The next UUID.
    fn uuid4(&mut self) -> Uuid;
}

/// Random version-4 UUIDs (normal runs). Reads the OS random device; falls
/// back to std's randomly keyed hasher where there is none.
#[derive(Default)]
pub struct RandomUuids {
    counter: u64,
}

impl RandomUuids {
    /// A new random source.
    pub fn new() -> Self {
        Self::default()
    }
}

impl UuidSource for RandomUuids {
    fn uuid4(&mut self) -> Uuid {
        let mut bytes = [0u8; 16];
        let from_os = std::fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
            .is_ok();
        if !from_os {
            use std::hash::{BuildHasher, Hasher};
            let time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            for half in 0..2 {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u64(self.counter);
                h.write_u128(time);
                h.write_u8(half);
                bytes[half as usize * 8..half as usize * 8 + 8]
                    .copy_from_slice(&h.finish().to_le_bytes());
            }
        }
        self.counter += 1;
        Uuid::from_bytes_v4(bytes)
    }
}

/// The deterministic sequence of `avid2prproj.determinism.seeded_ids(seed)`.
#[derive(Clone, Debug)]
pub struct SeededUuids {
    seed: String,
    counter: u64,
}

impl SeededUuids {
    /// A sequence starting at call #1, like a fresh `seeded_ids(seed)` context.
    pub fn new(seed: impl Into<String>) -> Self {
        SeededUuids { seed: seed.into(), counter: 0 }
    }

    /// How many UUIDs have been handed out so far.
    pub fn count(&self) -> u64 {
        self.counter
    }
}

impl UuidSource for SeededUuids {
    fn uuid4(&mut self) -> Uuid {
        self.counter += 1;
        let digest = sha256(format!("{}:{}", self.seed, self.counter).as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Uuid::from_bytes_v4(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_is_v4() {
        let mut r = RandomUuids::new();
        let a = r.uuid4();
        let b = r.uuid4();
        assert_ne!(a, b);
        let s = a.to_string();
        assert_eq!(s.len(), 36);
        assert_eq!(&s[14..15], "4");
        assert!(matches!(&s[19..20], "8" | "9" | "a" | "b"));
    }
}
