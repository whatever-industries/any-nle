//! Scoped compatibility for one zero-only packet after complete DNxHR LB pictures.
//! This is not a generic MXF padding rule. Keep normal parsing and all source guards.
use super::backend::{integer, run, Probe};
use super::receipts::{obj, text};
use super::source::layout;
use super::R;
use graph::json::{self, Value};
use prproj::convert::model::{ticks_per_unit, Fraction};
use prproj::hash::{hex, sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

const FRAME: u64 = 188416;
const MAX_FRAMES: i64 = 8192;
const MAX_TAIL: u64 = 65536;
const PREFIX: [u8; 5] = [0, 0, 2, 128, 3];
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Terminal {
    pub(super) frames: i64,
    pub(super) essence_offset: u64,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    header_prefix: Vec<u8>,
}
fn require(ok: bool, why: &str) -> R<()> {
    if ok {
        Ok(())
    } else {
        Err(format!("terminal DNx packet refused: {why}"))
    }
}
fn read(file: &mut File, offset: u64, size: u64, max: u64) -> R<Vec<u8>> {
    require(
        size <= max
            && offset
                .checked_add(size)
                .is_some_and(|n| file.metadata().is_ok_and(|m| n <= m.len())),
        "read exceeds bounded file span",
    )?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut data = vec![0; size as usize];
    file.read_exact(&mut data).map_err(|e| e.to_string())?;
    Ok(data)
}
fn header(file: &mut File, offset: u64) -> R<Vec<u8>> {
    let h = read(file, offset, 640, 640)?;
    require(
        h[..5] == PREFIX && h[5] & 2 == 0 && h[6] & 0x20 == 0,
        "not a supported progressive coding header",
    )?;
    require(
        h[0x28..0x2c] == 1274u32.to_be_bytes()
            && h[0x18..0x1a] == 1080u16.to_be_bytes()
            && h[0x1a..0x1c] == 1920u16.to_be_bytes(),
        "coded CID/raster disagree",
    )?;
    Ok(h[..48].to_vec())
}
fn fields(data: &[u8]) -> R<BTreeMap<u16, &[u8]>> {
    let mut at = 0;
    let mut out = BTreeMap::new();
    while at < data.len() {
        require(data.len() - at >= 4, "truncated index field")?;
        let tag = u16::from_be_bytes(data[at..at + 2].try_into().unwrap());
        let n = u16::from_be_bytes(data[at + 2..at + 4].try_into().unwrap()) as usize;
        at += 4;
        require(n <= data.len() - at, "index field exceeds local set")?;
        require(
            out.insert(tag, &data[at..at + n]).is_none(),
            "duplicate index field",
        )?;
        at += n;
    }
    Ok(out)
}
fn uint(fields: &BTreeMap<u16, &[u8]>, tag: u16, size: usize) -> R<u64> {
    let b = fields
        .get(&tag)
        .ok_or("terminal DNx index lacks required field")?;
    require(b.len() == size && size <= 8, "wrong index field width")?;
    Ok(b.iter().fold(0, |a, x| (a << 8) | u64::from(*x)))
}
impl Terminal {
    pub(super) fn filter(&self) -> String {
        // n is the zero-based packet index in this selected video stream. amount=0
        // disables noise; eq removes only the independently verified singleton tail.
        format!("noise=amount=0:drop='eq(n,{})'", self.frames)
    }
    pub(super) fn receipt(&self) -> Value {
        obj(vec![
            ("reason", text("verified zero-only terminal packet after complete scoped DNxHR LB coded pictures; compatibility handling, not a universal padding rule")),
            ("packet_index", Value::Int(self.frames)),
            ("file_offset", text((self.essence_offset + self.frames as u64 * FRAME).to_string())),
            ("bytes", text(self.bytes.to_string())),
            ("sha256", text(&self.sha256)),
            ("coded_header_prefix_sha256", text(hex(&sha256(&self.header_prefix)))),
            ("coded_picture_bytes", text((self.frames as u64 * FRAME).to_string())),
            ("filter", text(self.filter())),
        ])
    }
    pub(super) fn qualify(path: &Path, probe: &Probe, tool: &Path, rate: Fraction) -> R<Self> {
        require(
            rate == Fraction::new(24000, 1001) && probe.streams.len() == 1,
            "unsupported rate/layout",
        )?;
        let s = &probe.streams[0];
        let unit = ticks_per_unit(rate);
        require(
            s.kind == "video"
                && s.codec == "dnxhd"
                && s.profile == "DNXHR LB"
                && s.width == 1920
                && s.height == 1080
                && s.pix == "yuv422p"
                && s.rate == "24000/1001"
                && s.ticks % unit == 0,
            "unsupported picture descriptor",
        )?;
        let frames = s.ticks / unit;
        require(
            (1..=MAX_FRAMES).contains(&frames) && s.packets == frames + 1,
            "not a bounded N+1 packet case",
        )?;
        let terminal = Self::physical(path, frames)?;
        let raw = json::parse(&run(Command::new(tool)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-f",
                "mxf",
                "-select_streams",
                &probe.stream_indices[0].to_string(),
                "-show_packets",
                "-show_entries",
                "packet=stream_index,pts,dts,duration,size,pos",
                "-of",
                "json",
            ])
            .arg(path))?)?;
        terminal.check_packets(&raw, probe.stream_indices[0])?;
        Ok(terminal)
    }
    fn physical(path: &Path, frames: i64) -> R<Self> {
        require((1..=MAX_FRAMES).contains(&frames), "frame budget exceeded")?;
        let layout = layout(path)?;
        require(
            layout.essence.len() == 1 && layout.indexes.len() == 1,
            "ambiguous essence/index layout",
        )?;
        let e = &layout.essence[0];
        let index = &layout.indexes[0];
        require(
            e.key[..14] == [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 21, 1]
                && matches!(e.key[14], 6 | 13)
                && e.key[15] <= 127
                && e.body_sid != 0,
            "unsupported picture wrapping",
        )?;
        let coded = frames as u64 * FRAME;
        require(e.bytes > coded, "no physical bytes after coded span")?;
        let bytes = e.bytes - coded;
        require(
            bytes <= MAX_TAIL && bytes < FRAME,
            "terminal span exceeds bound",
        )?;
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let data = read(&mut file, index.offset, index.bytes, 128 << 10)?;
        let fields = fields(&data)?;
        require(
            fields.get(&0x3c0a).is_none_or(|uid| uid.len() == 16),
            "invalid index instance identifier",
        )?;
        require(
            uint(&fields, 0x3f0b, 8)? == (24000u64 << 32 | 1001)
                && uint(&fields, 0x3f0c, 8)? == 0
                && uint(&fields, 0x3f0d, 8)? == frames as u64 + 1
                && uint(&fields, 0x3f05, 4)? == 0
                && uint(&fields, 0x3f08, 1)? == 0
                && fields.get(&0x3f0e).is_none_or(|b| *b == [0]),
            "unsupported index clock/duration/slices",
        )?;
        require(
            uint(&fields, 0x3f06, 4)? == u64::from(index.index_sid)
                && index.index_sid != 0
                && uint(&fields, 0x3f07, 4)? == u64::from(e.body_sid),
            "partition/index stream identities disagree",
        )?;
        // No interleaved elements, temporal or slice offsets may change the lookup.
        require(
            fields.keys().all(|t| matches!(t, 0x3c0a | 0x3f05..=0x3f0e)),
            "unqualified index extension",
        )?;
        if let Some(delta) = fields.get(&0x3f09) {
            // FFmpeg's single-picture files may repeat the same zero DeltaEntry.
            // All entries must still point to the sole element, with no slice,
            // temporal or relative byte displacement.
            require(delta.len() >= 8, "truncated delta index")?;
            let count = u32::from_be_bytes(delta[..4].try_into().unwrap()) as usize;
            require(
                count <= 2
                    && delta[4..8] == 6u32.to_be_bytes()
                    && delta.len() == 8 + count * 6
                    && delta[8..].iter().all(|b| *b == 0),
                "nonzero/interleaved delta index",
            )?;
        }
        let array = fields
            .get(&0x3f0a)
            .ok_or("terminal DNx index lacks entries")?;
        require(
            array.len() == 8 + (frames as usize + 1) * 11
                && array[..4] == (frames as u32 + 1).to_be_bytes()
                && array[4..8] == 11u32.to_be_bytes(),
            "index array has wrong size",
        )?;
        let mut header_prefix = Vec::new();
        for i in 0..=frames as usize {
            let row = &array[8 + i * 11..8 + (i + 1) * 11];
            require(
                row[0] == 0
                    && row[1] == 0
                    && row[2] == 0x80
                    && u64::from_be_bytes(row[3..11].try_into().unwrap()) == i as u64 * FRAME,
                "index does not pin every coded boundary",
            )?;
            if i < frames as usize {
                let current = header(&mut file, e.offset + i as u64 * FRAME)?;
                if i == 0 {
                    header_prefix = current;
                } else {
                    require(
                        current == header_prefix,
                        "coded header format changes between pictures",
                    )?;
                }
            }
        }
        let tail = read(&mut file, e.offset + coded, bytes, MAX_TAIL)?;
        require(tail.iter().all(|b| *b == 0), "nonzero terminal data")?;
        Ok(Self {
            frames,
            essence_offset: e.offset,
            bytes,
            sha256: hex(&sha256(&tail)),
            header_prefix,
        })
    }
    fn check_packets(&self, raw: &Value, stream: usize) -> R<()> {
        let Some(Value::Arr(packets)) = raw.get("packets") else {
            return Err("terminal packet probe lacks packets".into());
        };
        require(
            packets.len() == self.frames as usize + 1,
            "packet cardinality changed",
        )?;
        for (n, p) in packets.iter().enumerate() {
            let size = if n == self.frames as usize {
                self.bytes
            } else {
                FRAME
            };
            require(
                integer(p, "stream_index")? == stream as i64
                    && integer(p, "pts")? == n as i64
                    && integer(p, "dts")? == n as i64
                    && integer(p, "duration")? == 1
                    && integer(p, "pos")? as i128
                        == i128::from(self.essence_offset) + n as i128 * i128::from(FRAME)
                    && integer(p, "size")? as i128 == i128::from(size),
                "parsed packet does not match physical coded/tail boundary",
            )?;
        }
        Ok(())
    }
    pub(super) fn verify_output(&self, path: &Path) -> R<()> {
        let layout = layout(path)?;
        let pictures: Vec<_> = layout.essence.iter().filter(|e| e.kind == 0x15).collect();
        require(
            pictures.len() == self.frames as usize,
            "output picture boundaries/count changed",
        )?;
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        for e in pictures {
            require(
                e.bytes == FRAME && e.key[14] == 0x0c,
                "output is not exact frame-wrapped DNx picture",
            )?;
            require(
                header(&mut file, e.offset)? == self.header_prefix,
                "output coded header format changed",
            )?;
        }
        Ok(())
    }
}
pub(super) fn require_filter(tool: &Path) -> R<()> {
    let help = run(Command::new(tool).args(["-hide_banner", "-h", "bsf=noise"]))?;
    require(
        help.contains("Bit stream filter noise")
            && help.contains("amount")
            && help.contains("drop"),
        "required byte-preserving packet filter unavailable",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(
                std::env::temp_dir()
                    .join(format!("anynle-terminal-{}-{n}-{}.mxf", std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed))),
            )
        }
        fn physical(&self, body: &[u8], fields: &BTreeMap<u16, Vec<u8>>) -> R<Terminal> {
            // Deliberately minimal physical framing control, not a native/application fixture.
            let mut bytes = Vec::new();
            fn klv(out: &mut Vec<u8>, key: [u8; 16], value: &[u8]) {
                out.extend(key);
                out.push(0x88);
                out.extend((value.len() as u64).to_be_bytes());
                out.extend(value);
            }
            let mut partition = vec![0; 104];
            partition[48..52].copy_from_slice(&2u32.to_be_bytes());
            partition[60..64].copy_from_slice(&1u32.to_be_bytes());
            klv(
                &mut bytes,
                [6, 14, 43, 52, 2, 5, 1, 1, 13, 1, 2, 1, 1, 3, 4, 0],
                &partition,
            );
            klv(
                &mut bytes,
                [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 21, 1, 6, 1],
                body,
            );
            let mut local = Vec::new();
            for (tag, value) in fields {
                local.extend(tag.to_be_bytes());
                local.extend((value.len() as u16).to_be_bytes());
                local.extend(value);
            }
            klv(
                &mut bytes,
                [6, 14, 43, 52, 2, 83, 1, 1, 13, 1, 2, 1, 1, 16, 1, 0],
                &local,
            );
            fs::write(&self.0, bytes).unwrap();
            Terminal::physical(&self.0, 2)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    fn control() -> (Vec<u8>, BTreeMap<u16, Vec<u8>>) {
        let mut body = vec![0; FRAME as usize * 2 + 512];
        for n in 0..2 {
            let at = n * FRAME as usize;
            body[at..at + 5].copy_from_slice(&PREFIX);
            body[at + 0x28..at + 0x2c].copy_from_slice(&1274u32.to_be_bytes());
            body[at + 0x18..at + 0x1a].copy_from_slice(&1080u16.to_be_bytes());
            body[at + 0x1a..at + 0x1c].copy_from_slice(&1920u16.to_be_bytes());
        }
        let mut array = Vec::from(3u32.to_be_bytes());
        array.extend(11u32.to_be_bytes());
        for n in 0..3 {
            array.extend([0, 0, 128]);
            array.extend((n * FRAME).to_be_bytes());
        }
        let fields = BTreeMap::from([
            (0x3f0b, (24000u64 << 32 | 1001).to_be_bytes().to_vec()),
            (0x3f0c, 0u64.to_be_bytes().to_vec()),
            (0x3f0d, 3u64.to_be_bytes().to_vec()),
            (0x3f05, 0u32.to_be_bytes().to_vec()),
            (0x3f06, 2u32.to_be_bytes().to_vec()),
            (0x3f07, 1u32.to_be_bytes().to_vec()),
            (0x3f08, vec![0]),
            (0x3f0a, array),
        ]);
        (body, fields)
    }
    #[test]
    fn scoped_zero_tail_is_pinned_without_stripping_internal_zeros() {
        let (body, fields) = control();
        let scratch = Scratch::new();
        let t = scratch.physical(&body, &fields).unwrap();
        assert_eq!(t.frames, 2);
        assert_eq!(t.bytes, 512);
        assert_eq!(t.sha256, hex(&sha256(&vec![0; 512])));
        assert_eq!(t.filter(), "noise=amount=0:drop='eq(n,2)'");
        // The coded control contains many zeros, all of which remain inside the coded span.
        assert_eq!(t.essence_offset, 154);
    }
    #[test]
    fn extra_picture_nonzero_tail_bad_headers_and_indexes_refuse() {
        let (body, fields) = control();
        let scratch = Scratch::new();
        for variant in 0..13 {
            let mut b = body.clone();
            let mut f = fields.clone();
            match variant {
                0 => {
                    b.pop();
                    b.push(1);
                }
                1 => {
                    b.truncate(2 * FRAME as usize);
                }
                2 => {
                    b.truncate(2 * FRAME as usize);
                    let frame = b[..FRAME as usize].to_vec();
                    b.extend(frame);
                }
                3 => {
                    b[FRAME as usize + 0x2b] ^= 1;
                }
                4 => {
                    b[FRAME as usize + 0x1b] ^= 1;
                }
                5 => {
                    f.insert(0x3f0d, 2u64.to_be_bytes().to_vec());
                }
                6 => {
                    f.get_mut(&0x3f0a).unwrap()[29] ^= 1;
                }
                7 => {
                    f.insert(0x3f07, 3u32.to_be_bytes().to_vec());
                }
                8 => {
                    f.insert(0x3f08, vec![1]);
                }
                9 => {
                    f.insert(0x3f0c, 1u64.to_be_bytes().to_vec());
                }
                10 => {
                    f.insert(0x3f09, vec![0; 14]);
                }
                11 => {
                    b[5] |= 2;
                }
                12 => {
                    b[FRAME as usize + 0x21] ^= 0x20;
                }
                _ => unreachable!(),
            }
            assert!(scratch.physical(&b, &f).is_err(), "variant {variant}");
        }
        assert!(super::fields(&[0x3f, 0x05, 0, 8, 0]).is_err());
        assert!(super::fields(&[0x3f, 0x08, 0, 1, 0, 0x3f, 0x08, 0, 1, 0]).is_err());
    }
    #[test]
    fn packet_layer_must_match_every_physical_boundary() {
        let t = Terminal {
            frames: 2,
            essence_offset: 100,
            bytes: 512,
            sha256: String::new(),
            header_prefix: Vec::new(),
        };
        let make = || {
            Value::Obj(vec![(
                "packets".into(),
                Value::Arr(
                    (0..3)
                        .map(|n| {
                            obj(vec![
                                ("stream_index", Value::Int(3)),
                                ("pts", Value::Int(n)),
                                ("dts", Value::Int(n)),
                                ("duration", Value::Int(1)),
                                ("size", Value::Int(if n == 2 { 512 } else { FRAME as i64 })),
                                ("pos", Value::Int(100 + n * FRAME as i64)),
                            ])
                        })
                        .collect(),
                ),
            )])
        };
        let good = make();
        t.check_packets(&good, 3).unwrap();
        for key in ["stream_index", "pts", "dts", "duration", "size", "pos"] {
            let mut raw = make();
            let Value::Obj(root) = &mut raw else {
                unreachable!()
            };
            let Value::Arr(packets) = &mut root[0].1 else {
                unreachable!()
            };
            let Value::Obj(fields) = &mut packets[2] else {
                unreachable!()
            };
            let item = fields.iter_mut().find(|x| x.0 == key).unwrap();
            item.1 = Value::Int(-1);
            assert!(t.check_packets(&raw, 3).is_err(), "{key}");
        }
        assert!(t.check_packets(&Value::Obj(vec![]), 3).is_err());
    }
}
