//! Base64 with Python's exact behaviour.
//!
//! `b64encode` is the standard padded alphabet. `b64decode` ports the
//! non-strict `binascii.a2b_base64` of CPython 3.14 (the oracle interpreter;
//! what `base64.b64decode(s)` uses): bytes outside the alphabet and `=` are
//! skipped (decoding does NOT stop at a pad, unlike Python <= 3.12), pads only
//! count toward completing the final quantum, and a final quantum of one
//! character, or of two/three characters without enough pads after the last
//! data character, is an error. Checked against Python 3.14.4 on 300k random
//! inputs; `tests/data/base64_cases.tsv` holds oracle cases.

use crate::error::{Error, Result};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Python `base64.b64encode(data).decode("ascii")`.
pub fn b64encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

fn sextet(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Python `base64.b64decode(s)` (non-strict).
///
/// A `str` argument with non-ASCII characters raises in Python; so does this.
pub fn b64decode(s: &str) -> Result<Vec<u8>> {
    if !s.is_ascii() {
        return Err(Error::Value(
            "string argument should contain only ASCII characters".into(),
        ));
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut quad_pos = 0u8;
    let mut left: u8 = 0;
    let mut pads = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' {
            // Python 3.14: a pad no longer ends decoding; it only counts
            // toward completing the final quantum.
            if quad_pos >= 2 {
                pads += 1;
            }
            continue;
        }
        let Some(v) = sextet(c) else { continue };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = v;
            }
            1 => {
                quad_pos = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    match quad_pos {
        0 => Ok(out),
        1 => Err(Error::Value(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more than a multiple of 4",
            s.bytes().filter(|&c| sextet(c).is_some()).count()
        ))),
        _ if u32::from(quad_pos) + pads >= 4 => Ok(out),
        _ => Err(Error::Value("Incorrect padding".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for n in 0..40u8 {
            let data: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(b64decode(&b64encode(&data)).unwrap(), data);
        }
        assert_eq!(b64encode(b"<Lumetri/>"), "PEx1bWV0cmkvPg==");
    }

    #[test]
    fn python_quirks() {
        // Non-alphabet bytes are skipped; Python 3.14 keeps decoding after a pad.
        assert_eq!(b64decode("pKUC\nWDKm/0S4ybgf AAAAQA==\n\t\t").unwrap().len(), 16);
        assert!(b64decode("QQ==QUJD").is_err());
        assert!(b64decode("QUJ=QUJD").is_err());
        assert!(b64decode("QUJDRA==x").is_err());
        assert_eq!(b64decode("QUI=x").unwrap(), [65, 66, 49]);
        assert_eq!(b64decode("A=B=C=D").unwrap(), [0, 16, 131]);
        assert_eq!(b64decode("QQ=").unwrap_err(), Error::Value("Incorrect padding".into()));
        assert!(b64decode("QUJ").is_err());
        assert!(b64decode("Q").is_err());
    }
}
