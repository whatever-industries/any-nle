//! Read-only source fingerprints, physical PCM spans and volume space checks.
use super::receipts::{obj, strings, text};
use super::R;
use graph::json::Value;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stamp {
    pub(super) len: u64,
    pub(super) modified: SystemTime,
}
pub(super) fn stamp(path: &Path) -> R<Stamp> {
    let m = fs::metadata(path).map_err(|e| e.to_string())?;
    if !m.is_file() {
        return Err("media input is not a regular file".into());
    }
    Ok(Stamp {
        len: m.len(),
        modified: m.modified().map_err(|e| e.to_string())?,
    })
}
pub(super) struct EssenceSpan {
    pub(super) kind: u8,
    pub(super) bytes: u64,
    pub(super) offset: u64,
    pub(super) key: [u8; 16],
    pub(super) body_sid: u32,
}
pub(super) struct IndexSpan {
    pub(super) offset: u64,
    pub(super) bytes: u64,
    pub(super) index_sid: u32,
}
pub(super) struct Layout {
    pub(super) essence: Vec<EssenceSpan>,
    pub(super) indexes: Vec<IndexSpan>,
}

// Walk complete KLV boundaries. UUID-keyed private sets are skipped only within
// declared header metadata, as in IDENTITY-0006; never resynchronize arbitrary bytes.
pub(super) fn essence_spans(path: &Path) -> R<Vec<EssenceSpan>> {
    Ok(layout(path)?.essence)
}
pub(super) fn layout(path: &Path) -> R<Layout> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let mut spans = Vec::new();
    let mut indexes = Vec::new();
    let (mut body_sid, mut index_sid) = (0, 0);
    let mut header_bytes = None;
    let mut header_end = None;
    let mut entries = 0usize;
    while file.stream_position().map_err(|e| e.to_string())? < size {
        entries += 1;
        if entries > 1_000_000 {
            return Err("MXF KLV entry budget exceeded".into());
        }
        let offset = file.stream_position().map_err(|e| e.to_string())?;
        let mut key = [0u8; 16];
        file.read_exact(&mut key)
            .map_err(|_| "truncated MXF KLV key")?;
        let mut first = [0u8; 1];
        file.read_exact(&mut first)
            .map_err(|_| "truncated MXF BER length")?;
        let len = if first[0] < 128 {
            first[0] as u64
        } else {
            let count = (first[0] & 127) as usize;
            if count == 0 || count > 8 {
                return Err("invalid MXF BER length".into());
            }
            let mut b = [0u8; 8];
            file.read_exact(&mut b[8 - count..])
                .map_err(|_| "truncated MXF BER length")?;
            u64::from_be_bytes(b)
        };
        let pos = file.stream_position().map_err(|e| e.to_string())?;
        let end = pos
            .checked_add(len)
            .filter(|n| *n <= size)
            .ok_or("MXF KLV extends beyond file")?;
        if key[..4] != [6, 14, 43, 52]
            && !header_end.is_some_and(|limit| offset < limit && end <= limit)
        {
            return Err(
                "unqualified MXF run-in or trailing/private bytes outside header metadata".into(),
            );
        }
        const PARTITION: [u8; 13] = [6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1];
        if key[..13] == PARTITION && matches!(key[13], 2..=4) && key[15] == 0 {
            if len < 64 {
                return Err("MXF partition pack too short".into());
            }
            let mut pack = [0u8; 64];
            file.read_exact(&mut pack).map_err(|e| e.to_string())?;
            body_sid = u32::from_be_bytes(pack[60..64].try_into().unwrap());
            index_sid = u32::from_be_bytes(pack[48..52].try_into().unwrap());
            let count = u64::from_be_bytes(pack[32..40].try_into().unwrap());
            if count > avid_reader::mxf::MAX_HEADER_BYTES {
                return Err("MXF header metadata exceeds read limit".into());
            }
            header_bytes = (count > 0).then_some(count);
            header_end = None;
        } else if key[..13] == PARTITION && key[13..] == [5, 1, 0] {
            let count = header_bytes
                .take()
                .ok_or("MXF primer without declared header metadata")?;
            let limit = offset
                .checked_add(count)
                .filter(|n| *n <= size && *n >= end)
                .ok_or("invalid MXF header metadata span")?;
            header_end = Some(limit);
        }
        if header_end.is_some_and(|limit| offset < limit && end > limit) {
            return Err("MXF KLV crosses header metadata boundary".into());
        }
        if key[..12] == [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1] {
            spans.push(EssenceSpan {
                kind: key[12],
                bytes: len,
                offset: pos,
                key,
                body_sid,
            });
        }
        if key == [6, 14, 43, 52, 2, 83, 1, 1, 13, 1, 2, 1, 1, 16, 1, 0] {
            indexes.push(IndexSpan {
                offset: pos,
                bytes: len,
                index_sid,
            });
        }
        file.seek(SeekFrom::Start(end)).map_err(|e| e.to_string())?;
    }
    Ok(Layout {
        essence: spans,
        indexes,
    })
}

// Count actual sound payloads without loading essence or trusting descriptor duration.
pub(super) fn pcm_essence_bytes(path: &Path) -> R<u64> {
    essence_spans(path)?
        .iter()
        .filter(|s| s.kind == 0x16)
        .try_fold(0u64, |sum, s| {
            sum.checked_add(s.bytes)
                .ok_or_else(|| "PCM payload size overflow".into())
        })
}

pub(super) fn free_space(parent: &Path) -> R<u64> {
    // available_space queries bytes available to this user on the target volume;
    // fs2 uses GetDiskFreeSpaceExW on Windows and statvfs on Unix. Do not use a
    // subprocess or total free blocks (which can include privileged reserves).
    fs2::available_space(parent).map_err(|e| format!("cannot read available disk space: {e}"))
}

#[cfg(test)]
mod space_tests {
    use super::*;

    #[test]
    fn available_space_queries_existing_volume() {
        let parent = std::env::temp_dir();
        assert!(free_space(&parent).unwrap() <= fs2::total_space(&parent).unwrap());
    }

    #[test]
    fn available_space_missing_path_refuses() {
        let missing = std::env::temp_dir().join(format!("any-nle-space-missing-{}", std::process::id()));
        assert!(!missing.exists());
        assert!(free_space(&missing).unwrap_err().starts_with("cannot read available disk space:"));
    }
}

pub(super) fn id_value(path: &Path) -> R<Value> {
    let id = avid_reader::mxf::read_identity(path).map_err(|e| e.to_string())?;
    if id.file_packages.len() != 1 || id.material_packages.len() != 1 {
        return Err("ambiguous/missing MXF file or material package identity".into());
    }
    Ok(obj(vec![
        ("file_packages", strings(&id.file_packages)),
        ("material_packages", strings(&id.material_packages)),
        ("source_packages", strings(&id.source_packages)),
        (
            "file_start_timecode",
            id.file_start_timecode()
                .map(|(frames, base, drop)| {
                    obj(vec![
                        ("start_frames", text(frames.to_string())),
                        ("rounded_base", Value::Int(i64::from(base))),
                        ("drop_frame", Value::Bool(drop)),
                    ])
                })
                .unwrap_or(Value::Null),
        ),
        (
            "timecodes",
            Value::Arr(
                id.timecodes
                    .iter()
                    .map(|(frames, base, drop)| {
                        obj(vec![
                            ("start_frames", text(frames.to_string())),
                            ("rounded_base", Value::Int(i64::from(*base))),
                            ("drop_frame", Value::Bool(*drop)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "file_tracks",
            Value::Arr(
                id.file_tracks
                    .iter()
                    .map(|(n, d, length)| {
                        Value::Arr(vec![Value::Int(*n), Value::Int(*d), Value::Int(*length)])
                    })
                    .collect(),
            ),
        ),
    ]))
}
