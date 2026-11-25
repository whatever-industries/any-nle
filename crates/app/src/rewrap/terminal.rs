//! Verify indexed pictures and zero-only tails in the retained DNxHR LB and ProRes layouts.
use super::backend::{integer, read_stdout, run, Probe};
use super::receipts::{obj, text};
use super::source::{layout, IndexSpan};
use super::R;
use graph::json::{self, Value};
use prproj::convert::model::{ticks_per_unit, Fraction};
use prproj::hash::{hex, sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

const FRAME: u64 = 188416;
const MAX_FRAMES: i64 = 8192;
// Limit full-essence verification work independently of packet report size.
const MAX_LT_FRAMES: i64 = 32_768;
const PACKET_FIELDS: [&str; 6] = ["stream_index", "pts", "dts", "duration", "size", "pos"];
const MAX_PACKET_LINE: usize = 256;
const MAX_TAIL: u64 = 65536;
const PREFIX: [u8; 5] = [0, 0, 2, 128, 3];
#[derive(Clone, Copy, Debug, PartialEq)]
enum Profile {
    Dnx,
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    ManagedDnxHd,
    ProResFreeze,
    ProResLt,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Terminal {
    pub(super) frames: i64,
    pub(super) essence_offset: u64,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    header_prefix: Vec<u8>,
    frame_bytes: u64,
    /// Byte offsets relative to the clip-wrapped essence, including its coded end.
    indexed_offsets: Vec<u64>,
    profile: Profile,
}

/// Return the payload start, payload length and native index-key offset.
#[cfg(target_os = "macos")]
pub(crate) fn managed_dnxhd_layout(path: &Path, frames: i64) -> R<(u64, u64, u64)> {
    let layout = layout(path)?;
    let ([essence], [index]) = (layout.essence.as_slice(), layout.indexes.as_slice()) else {
        return Err("managed DNxHD needs one essence payload and one index".into());
    };
    let coded = u64::try_from(frames).ok().and_then(|n| n.checked_mul(FRAME))
        .ok_or("managed DNxHD frame extent overflows")?;
    Terminal::physical_indexed(path, frames, Profile::ManagedDnxHd, essence.bytes > coded)?;
    Ok((essence.offset, essence.bytes, index.key_offset))
}

fn require(ok: bool, why: &str) -> R<()> {
    if ok {
        Ok(())
    } else {
        Err(format!("terminal picture packet refused: {why}"))
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
fn header(file: &mut File, offset: u64, cid: u32) -> R<Vec<u8>> {
    let h = read(file, offset, 640, 640)?;
    require(
        h[..5] == PREFIX && h[5] & 2 == 0 && h[6] & 0x20 == 0,
        "not a supported progressive coding header",
    )?;
    require(
        h[0x28..0x2c] == cid.to_be_bytes()
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
        .ok_or("terminal picture index lacks required field")?;
    require(b.len() == size && size <= 8, "wrong index field width")?;
    Ok(b.iter().fold(0, |a, x| (a << 8) | u64::from(*x)))
}

fn picture_index_offsets(
    file: &mut File, indexes: &[IndexSpan], body_sid: u32, expected: usize, profile: Profile,
) -> R<Vec<u64>> {
    let index_sid = indexes.first().ok_or("terminal picture index is missing")?.index_sid;
    let clock = match profile {
        Profile::Dnx | Profile::ManagedDnxHd => 24000u64 << 32 | 1001,
        Profile::ProResFreeze | Profile::ProResLt => 24u64 << 32 | 1,
    };
    let mut offsets = Vec::with_capacity(expected);
    for index in indexes {
        let data = read(file, index.offset, index.bytes, 128 << 10)?;
        let fields = fields(&data)?;
        require(
            fields.get(&0x3c0a).is_none_or(|uid| uid.len() == 16),
            "invalid index instance identifier",
        )?;
        let start = uint(&fields, 0x3f0c, 8)?;
        let length = uint(&fields, 0x3f0d, 8)?;
        require(
            start == offsets.len() as u64 && (1..=(expected - offsets.len()) as u64).contains(&length),
            "index segments do not cover a contiguous declared picture span",
        )?;
        require(
            uint(&fields, 0x3f0b, 8)? == clock
                && uint(&fields, 0x3f05, 4)? == 0
                && uint(&fields, 0x3f08, 1)? == 0
                && fields.get(&0x3f0e).is_none_or(|b| *b == [0]),
            "unsupported index clock/duration/slices",
        )?;
        require(
            index.index_sid == index_sid && index_sid != 0
                && uint(&fields, 0x3f06, 4)? == u64::from(index_sid)
                && uint(&fields, 0x3f07, 4)? == u64::from(body_sid),
            "partition/index stream identities disagree",
        )?;
        require(
            fields.keys().all(|t| matches!(t, 0x3c0a | 0x3f05..=0x3f0e)),
            "unqualified index extension",
        )?;
        if let Some(delta) = fields.get(&0x3f09) {
            // FFmpeg may repeat a zero DeltaEntry for the one picture element.
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
            .ok_or("terminal picture index lacks entries")?;
        require(
            array.len() == 8 + length as usize * 11
                && array[..4] == (length as u32).to_be_bytes()
                && array[4..8] == 11u32.to_be_bytes(),
            "index array has wrong size",
        )?;
        for row in array[8..].chunks_exact(11) {
            require(row[..3] == [0, 0, 0x80], "index does not pin every coded boundary")?;
            offsets.push(u64::from_be_bytes(row[3..11].try_into().unwrap()));
        }
    }
    require(offsets.len() == expected, "index segments do not cover a contiguous declared picture span")?;
    Ok(offsets)
}

fn prores_lt_frame(frame: &[u8]) -> R<()> {
    let length = |field: &[u8]| field.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b));
    require(
        frame.len() >= 28 && length(&frame[..4]) == frame.len() && &frame[4..8] == b"icpf"
            && frame[11] == 0 && frame[16..20] == [7, 128, 4, 56]
            && frame[20] >> 6 == 2 && (frame[20] >> 2) & 3 == 0
            && frame[25] & 15 == 0,
        "invalid progressive ProRes frame boundary or header",
    )?;
    let header = length(&frame[8..10]);
    let picture = frame.get(8 + header..).filter(|p| header >= 20 && p.len() >= 8)
        .ok_or("invalid ProRes picture start")?;
    let header = usize::from(picture[0] >> 3);
    let picture_size = length(&picture[1..5]);
    require(header >= 8 && header < picture_size && picture_size <= picture.len(),
        "invalid ProRes picture bounds")?;
    let macroblocks = 120usize;
    let slice_width = 1 << ((picture[7] >> 4) & 3);
    let slices_per_row = macroblocks / slice_width + (macroblocks % slice_width).count_ones() as usize;
    let table_end = header + 2 * slices_per_row * 68;
    require(table_end <= picture_size, "invalid ProRes slice table bounds")?;
    let mut offset = table_end;
    for entry in picture[header..table_end].chunks_exact(2) {
        let size = length(entry);
        let end = offset + size;
        require(size >= 6 && end <= picture_size, "invalid ProRes slice boundary")?;
        let slice = &picture[offset..end];
        let header = usize::from(slice[0] >> 3);
        let y = length(&slice[2..4]);
        let cb = length(&slice[4..6]);
        require(header >= 6 && y > 0 && cb > 0 && header + y + cb < size,
            "invalid ProRes slice component lengths")?;
        offset = end;
    }
    require(offset == picture_size && picture[picture_size..].iter().all(|b| *b == 0),
        "ProRes slice table or in-frame stuffing disagrees with the saved picture")
}

impl Terminal {
    pub(super) fn matches_span(&self, start: i64, frames: i64, offset: i64, bytes: i64) -> bool {
        let span = (|| {
            let first = *self.indexed_offsets.get(usize::try_from(start).ok()?)?;
            let end = *self.indexed_offsets.get(usize::try_from(start.checked_add(frames)?).ok()?)?;
            Some((self.essence_offset.checked_add(first)?, end.checked_sub(first)?))
        })();
        frames > 0
            && span.is_some_and(|(start, extent)|
                u64::try_from(offset) == Ok(start) && u64::try_from(bytes) == Ok(extent))
    }

    fn coded_bytes(&self) -> u64 {
        self.indexed_offsets.last().copied().unwrap_or(self.frames as u64 * self.frame_bytes)
    }
    pub(super) fn filter(&self) -> String {
        // n is the zero-based packet index in this selected video stream. amount=0
        // disables noise; eq removes only the independently verified singleton tail.
        format!("noise=amount=0:drop='eq(n,{})'", self.frames)
    }
    pub(super) fn receipt(&self) -> Value {
        obj(vec![
            ("reason", text(match self.profile {
                Profile::ManagedDnxHd => "verified indexed managed DNxHD CID 1253 pictures",
                Profile::Dnx => "verified zero-only terminal packet after complete scoped DNxHR LB coded pictures; compatibility handling, not a universal padding rule",
                Profile::ProResFreeze => "verified zero-only terminal packet after the complete generated ProRes Freeze Frame",
                Profile::ProResLt => "verified zero-only terminal packet after complete indexed ProRes 422 LT pictures",
            })),
            ("packet_index", Value::Int(self.frames)),
            ("file_offset", text((self.essence_offset + self.coded_bytes()).to_string())),
            ("bytes", text(self.bytes.to_string())),
            ("sha256", text(&self.sha256)),
            ("coded_header_prefix_sha256", text(hex(&sha256(&self.header_prefix)))),
            ("coded_picture_bytes", text(self.coded_bytes().to_string())),
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
        Self::qualify_packets(path, probe, tool, Self::physical(path, frames)?)
    }
    pub(super) fn qualify_freeze(path: &Path, probe: &Probe, tool: &Path) -> R<Self> {
        require(
            probe.streams.len() == 1 && probe.streams[0].packets == 121
                && probe.streams[0].ticks == 120 * ticks_per_unit(Fraction::new(24, 1)),
            "generated freeze needs its 120-picture span plus one tail",
        )?;
        let terminal = Self::physical_profile(path, 120, Profile::ProResFreeze)?;
        Self::qualify_packets(path, probe, tool, terminal)
    }
    pub(super) fn qualify_lt(path: &Path, probe: &Probe, tool: &Path) -> R<Self> {
        let unit = ticks_per_unit(Fraction::new(24, 1));
        let [picture] = probe.streams.as_slice() else {
            return Err("terminal ProRes LT needs one picture stream".into());
        };
        let frames = picture.ticks / unit;
        require(picture.ticks % unit == 0 && matches!(picture.packets.checked_sub(frames), Some(0 | 1)),
            "ProRes LT packet count differs from the declared span by more than the optional tail")?;
        let has_tail = picture.packets != frames;
        Self::qualify_packets(path, probe, tool, Self::physical_indexed(path, frames, Profile::ProResLt, has_tail)?)
    }
    pub(super) fn recheck(&self, path: &Path, probe: &Probe, tool: &Path, rate: Fraction) -> R<Self> {
        match self.profile {
            Profile::Dnx => Self::qualify(path, probe, tool, rate),
            Profile::ManagedDnxHd => return Err("managed DNxHD is verified through its indexed layout".into()),
            Profile::ProResFreeze => Self::qualify_freeze(path, probe, tool),
            Profile::ProResLt => Self::qualify_lt(path, probe, tool),
        }
    }
    fn qualify_packets(path: &Path, probe: &Probe, tool: &Path, terminal: Self) -> R<Self> {
        let stream = probe.stream_indices[0];
        let mut cmd = Command::new(tool);
        cmd.args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mxf",
            "-select_streams",
            &stream.to_string(),
            "-show_packets",
            "-show_entries",
            "packet=stream_index,pts,dts,duration,size,pos",
            "-of",
        ]);
        if terminal.profile == Profile::ProResLt {
            cmd.arg("compact=p=0:nk=0").arg(path);
            read_stdout(&mut cmd, |out| terminal.check_packet_lines(BufReader::new(out), stream))?;
        } else {
            cmd.arg("json").arg(path);
            terminal.check_packets(&json::parse(&run(&mut cmd)?)?, stream)?;
        }
        Ok(terminal)
    }
    fn physical(path: &Path, frames: i64) -> R<Self> {
        Self::physical_profile(path, frames, Profile::Dnx)
    }
    fn physical_profile(path: &Path, frames: i64, profile: Profile) -> R<Self> {
        Self::physical_indexed(path, frames, profile, true)
    }
    fn physical_indexed(path: &Path, frames: i64, profile: Profile, has_tail: bool) -> R<Self> {
        let max_frames = if profile == Profile::ProResLt { MAX_LT_FRAMES } else { MAX_FRAMES };
        require((1..=max_frames).contains(&frames), "frame budget exceeded")?;
        let layout = layout(path)?;
        require(
            layout.essence.len() == 1 && !layout.indexes.is_empty()
                && (layout.indexes.len() == 1 || profile == Profile::ProResLt),
            "ambiguous essence/index layout",
        )?;
        let e = &layout.essence[0];
        require(
            e.key[..14] == [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 21, 1]
                && matches!(e.key[14], 6 | 13)
                && e.key[15] <= 127
                && e.body_sid != 0,
            "unsupported picture wrapping",
        )?;
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let frame_bytes = match profile {
            Profile::Dnx | Profile::ManagedDnxHd => FRAME,
            Profile::ProResLt => {
                require(e.key[14..] == [6, 1] && (!has_tail || e.bytes % 4096 == 0),
                    "unsupported ProRes LT clip wrapping")?;
                0
            },
            Profile::ProResFreeze => {
                let prefix = read(&mut file, e.offset, 28, 28)?;
                let size = u64::from(u32::from_be_bytes(prefix[..4].try_into().unwrap()));
                require(
                    e.key[14..] == [6, 1]
                        && prefix[4..8] == *b"icpf"
                        && prefix[16..20] == [0x07, 0x80, 0x04, 0x38]
                        && (28..=2 << 20).contains(&size),
                    "unsupported ProRes freeze essence header",
                )?;
                size
            }
        };
        if profile != Profile::ProResLt {
            let coded = frames as u64 * frame_bytes;
            require(!has_tail || e.bytes > coded, "no physical bytes after coded span")?;
            let bytes = e.bytes.checked_sub(coded).ok_or("coded span exceeds its physical essence")?;
            require(bytes <= MAX_TAIL && bytes < frame_bytes
                && match profile {
                    Profile::Dnx => true,
                    Profile::ManagedDnxHd => matches!(bytes, 0 | 8192),
                    _ => bytes == 32000,
                }, "terminal span exceeds bound")?;
        }
        let entries = frames as usize + usize::from(has_tail);
        let mut offsets = picture_index_offsets(&mut file, &layout.indexes, e.body_sid, entries, profile)?;
        if !has_tail && profile == Profile::ProResLt {
            offsets.push(e.bytes);
        }
        let mut header_prefix = Vec::new();
        let indexed_offsets = if profile == Profile::ProResLt {
            require(offsets[0] == 0, "ProRes index starts after its essence")?;
            for (i, range) in offsets.windows(2).enumerate() {
                let [start, end] = [range[0], range[1]];
                let size = end.checked_sub(start).filter(|n| (28..=8 << 20).contains(n))
                    .ok_or("invalid indexed ProRes frame size")?;
                require(end <= e.bytes && (!has_tail || end < e.bytes),
                    "ProRes frame crosses the physical coded extent")?;
                let frame = read(&mut file, e.offset + start, size, 8 << 20)?;
                prores_lt_frame(&frame)?;
                if i == 0 {
                    header_prefix.extend_from_slice(&frame[..28]);
                }
            }
            offsets
        } else {
            let first_picture = if profile == Profile::ProResFreeze {
                Some(read(&mut file, e.offset, frame_bytes, 2 << 20)?)
            } else {
                None
            };
            for (i, relative) in offsets.into_iter().enumerate() {
                require(relative == i as u64 * frame_bytes, "index does not pin every coded boundary")?;
                if i < frames as usize {
                    let offset = e.offset + relative;
                    let current = if let Some(first) = &first_picture {
                        let picture = read(&mut file, offset, frame_bytes, 2 << 20)?;
                        require(&picture == first, "generated ProRes pictures are not byte-identical")?;
                        picture[..28].to_vec()
                    } else {
                        header(&mut file, offset, if profile == Profile::ManagedDnxHd { 1253 } else { 1274 })?
                    };
                    if i == 0 {
                        header_prefix = current;
                    } else {
                        require(current == header_prefix, "coded header format changes between pictures")?;
                    }
                }
            }
            Vec::new()
        };
        let coded = indexed_offsets.last().copied().unwrap_or(frames as u64 * frame_bytes);
        let bytes = e.bytes.checked_sub(coded).ok_or("coded span exceeds its physical essence")?;
        require((bytes > 0) == has_tail, "physical terminal span disagrees with the packet count")?;
        require(bytes <= MAX_TAIL, "terminal span exceeds bound")?;
        let tail = read(&mut file, e.offset + coded, bytes, MAX_TAIL)?;
        require(tail.iter().all(|b| *b == 0), "nonzero terminal data")?;
        Ok(Self {
            frames,
            essence_offset: e.offset,
            bytes,
            sha256: hex(&sha256(&tail)),
            header_prefix,
            frame_bytes,
            indexed_offsets,
            profile,
        })
    }
    fn check_packets(&self, raw: &Value, stream: usize) -> R<()> {
        let Some(Value::Arr(packets)) = raw.get("packets") else {
            return Err("terminal packet probe lacks packets".into());
        };
        require(
            packets.len() == self.packet_count(),
            "packet cardinality changed",
        )?;
        for (n, p) in packets.iter().enumerate() {
            let [index, pts, dts, duration, size, pos] = PACKET_FIELDS;
            self.check_packet(
                n,
                stream,
                [
                    integer(p, index)?,
                    integer(p, pts)?,
                    integer(p, dts)?,
                    integer(p, duration)?,
                    integer(p, size)?,
                    integer(p, pos)?,
                ],
            )?;
        }
        Ok(())
    }
    fn packet_count(&self) -> usize {
        self.frames as usize + usize::from(self.bytes > 0)
    }
    fn check_packet_lines(&self, mut reader: impl BufRead, stream: usize) -> R<()> {
        let mut line = Vec::with_capacity(MAX_PACKET_LINE);
        for n in 0..self.packet_count() {
            line.clear();
            (&mut reader)
                .take(MAX_PACKET_LINE as u64)
                .read_until(b'\n', &mut line)
                .map_err(|e| e.to_string())?;
            require(!line.is_empty(), "packet cardinality changed")?;
            let record = line
                .strip_suffix(b"\n")
                .ok_or("unterminated or oversized packet row")?;
            let record = record.strip_suffix(b"\r").unwrap_or(record);
            let values = parse_packet_line(record)?;
            self.check_packet(n, stream, values)?;
        }
        require(
            reader.fill_buf().map_err(|e| e.to_string())?.is_empty(),
            "packet cardinality changed",
        )
    }
    fn check_packet(&self, n: usize, stream: usize, observed: [i64; 6]) -> R<()> {
        let start = self.indexed_offsets.get(n).copied().unwrap_or(n as u64 * self.frame_bytes);
        let size = if n == self.frames as usize {
            self.bytes
        } else {
            self.indexed_offsets.get(n + 1).map_or(self.frame_bytes, |end| end - start)
        };
        let [index, pts, dts, duration, observed_size, pos] = observed;
        require(
            index == stream as i64 && pts == n as i64 && dts == n as i64 && duration == 1
                && i128::from(pos) == i128::from(self.essence_offset) + i128::from(start)
                && i128::from(observed_size) == i128::from(size),
            "parsed packet does not match physical coded/tail boundary",
        )
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
                header(&mut file, e.offset, 1274)? == self.header_prefix,
                "output coded header format changed",
            )?;
        }
        Ok(())
    }
}
fn parse_packet_line(record: &[u8]) -> R<[i64; 6]> {
    let record = std::str::from_utf8(record).map_err(|_| "packet row is not UTF-8")?;
    let mut values = [None; 6];
    for field in record.split('|') {
        let (name, value) = field.split_once('=').ok_or("malformed packet field")?;
        let index = PACKET_FIELDS
            .iter()
            .position(|key| *key == name)
            .ok_or("unexpected packet field")?;
        require(values[index].is_none(), "duplicate packet field")?;
        values[index] = Some(value.parse().map_err(|_| format!("bad {name}"))?);
    }
    match values {
        [Some(index), Some(pts), Some(dts), Some(duration), Some(size), Some(pos)] => {
            Ok([index, pts, dts, duration, size, pos])
        }
        _ => Err("missing packet field".into()),
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
        fn write(&self, body: &[u8], fields: &BTreeMap<u16, Vec<u8>>) {
            self.write_segments(body, &[fields]);
        }
        fn write_segments(&self, body: &[u8], segments: &[&BTreeMap<u16, Vec<u8>>]) {
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
            for fields in segments {
                let mut local = Vec::new();
                for (tag, value) in *fields {
                    local.extend(tag.to_be_bytes());
                    local.extend((value.len() as u16).to_be_bytes());
                    local.extend(value);
                }
                klv(
                    &mut bytes,
                    [6, 14, 43, 52, 2, 83, 1, 1, 13, 1, 2, 1, 1, 16, 1, 0],
                    &local,
                );
            }
            fs::write(&self.0, bytes).unwrap();
        }
        fn physical(&self, body: &[u8], fields: &BTreeMap<u16, Vec<u8>>) -> R<Terminal> {
            self.write(body, fields);
            Terminal::physical(&self.0, 2)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    fn control() -> (Vec<u8>, BTreeMap<u16, Vec<u8>>) {
        dnx_control(1274, 512)
    }

    fn dnx_control(cid: u32, tail: usize) -> (Vec<u8>, BTreeMap<u16, Vec<u8>>) {
        let mut body = vec![0; FRAME as usize * 2 + tail];
        for n in 0..2 {
            let at = n * FRAME as usize;
            body[at..at + 5].copy_from_slice(&PREFIX);
            body[at + 0x28..at + 0x2c].copy_from_slice(&cid.to_be_bytes());
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
    fn managed_dnxhd_requires_indexed_cid_1253_and_zero_alignment_tail() {
        let scratch = Scratch::new();
        let (mut body, fields) = dnx_control(1253, 8192);
        scratch.write_segments(&body, &[&fields]);
        let check = || Terminal::physical_indexed(&scratch.0, 2, Profile::ManagedDnxHd, true);
        assert_eq!(check().unwrap().bytes, 8192);
        *body.last_mut().unwrap() = 1;
        scratch.write_segments(&body, &[&fields]);
        assert!(check().unwrap_err().contains("nonzero terminal data"));
        let (wrong_cid, _) = dnx_control(1274, 8192);
        scratch.write_segments(&wrong_cid, &[&fields]);
        assert!(check().unwrap_err().contains("CID/raster"));
    }

    #[test]
    fn compact_packet_witness_checks_the_full_span_at_the_frame_budget() {
        use std::fmt::Write;

        const SIZE: u64 = 46_000;
        const ESSENCE: u64 = 393_216;
        let frames = MAX_LT_FRAMES as u64;
        let terminal = Terminal {
            frames: MAX_LT_FRAMES,
            essence_offset: ESSENCE,
            bytes: 5_000,
            sha256: String::new(),
            header_prefix: Vec::new(),
            frame_bytes: 0,
            indexed_offsets: (0..=frames).map(|frame| frame * SIZE).collect(),
            profile: Profile::ProResLt,
        };
        let mut rows = String::new();
        for n in 0..frames {
            writeln!(
                rows,
                "stream_index=0|pts={n}|dts={n}|duration=1|size={SIZE}|pos={}",
                ESSENCE + n * SIZE
            )
            .unwrap();
        }
        let last_pos = ESSENCE + frames * SIZE;
        let last = format!(
            "pos={last_pos}|stream_index=0|pts={frames}|dts={frames}|size=5000|duration=1\r\n"
        );
        let check = |end: &str| terminal.check_packet_lines(rows.as_bytes().chain(end.as_bytes()), 0);
        check(&last).unwrap();
        for invalid in [
            String::new(),
            last.trim_end().to_owned(),
            format!("{last}{last}"),
            last.replace("duration=1", "duration=1|duration=1"),
            last.replace("duration=1", "duration=\u{fffd}"),
            last.replace("|duration=1", ""),
            last.replace("duration=1", "flags=K_"),
            last.replace("size=5000", "size=4999"),
            last.replace(&format!("pos={last_pos}"), &format!("pos={}", last_pos + 1)),
            format!("{}\n", "x".repeat(MAX_PACKET_LINE)),
        ] {
            assert!(check(&invalid).is_err(), "{invalid}");
        }
        // The compact path also admits a physically complete file without a tail.
        let complete = Terminal { bytes: 0, ..terminal };
        complete.check_packet_lines(rows.as_bytes(), 0).unwrap();
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
    fn prores_lt_variable_sizes_keep_frame_stuffing_and_refuse_nonzero_tail() {
        fn picture(stuffing: usize) -> Vec<u8> {
            const SLICES: usize = 15 * 68;
            let picture_size = 8 + SLICES * (2 + 9);
            let mut frame = vec![0; 28 + picture_size + stuffing];
            let size = frame.len() as u32;
            frame[..4].copy_from_slice(&size.to_be_bytes());
            frame[4..10].copy_from_slice(b"icpf\x00\x14");
            frame[16..21].copy_from_slice(&[7, 128, 4, 56, 0x80]);
            frame[28] = 8 << 3;
            frame[29..33].copy_from_slice(&(picture_size as u32).to_be_bytes());
            frame[35] = 3 << 4;
            let (table, data) = frame[36..28 + picture_size].split_at_mut(SLICES * 2);
            for size in table.chunks_exact_mut(2) {
                size[1] = 9;
            }
            for slice in data.chunks_exact_mut(9) {
                slice[0] = 6 << 3;
                slice[3] = 1;
                slice[5] = 1;
            }
            frame
        }
        let first = picture(9);
        let second = picture(12);
        let coded = first.len() + second.len();
        let mut body = [first.as_slice(), second.as_slice()].concat();
        body.resize(coded.next_multiple_of(4096), 0);
        let (_, mut index) = control();
        index.insert(0x3f0b, (24u64 << 32 | 1).to_be_bytes().to_vec());
        let rows = index.get_mut(&0x3f0a).unwrap();
        rows[22..30].copy_from_slice(&(first.len() as u64).to_be_bytes());
        rows[33..41].copy_from_slice(&(coded as u64).to_be_bytes());
        let scratch = Scratch::new();
        scratch.write(&body, &index);
        let check = || Terminal::physical_profile(&scratch.0, 2, Profile::ProResLt);
        let result = check().unwrap();
        assert_eq!(result.indexed_offsets, [0, first.len() as u64, coded as u64]);
        assert_eq!(result.bytes, (body.len() - coded) as u64);
        let offset = (result.essence_offset + first.len() as u64) as i64;
        assert!(result.matches_span(1, 1, offset, second.len() as i64));
        assert!(!result.matches_span(1, 1, offset + 1, second.len() as i64));
        assert!(!result.matches_span(1, 2, offset, second.len() as i64));
        // A segment can start in the middle of the essence. Its offsets are global.
        let split = |start: u64, len: u64| {
            let mut segment = index.clone();
            segment.insert(0x3f0c, start.to_be_bytes().to_vec());
            segment.insert(0x3f0d, len.to_be_bytes().to_vec());
            let rows = &index[&0x3f0a][8 + start as usize * 11..8 + (start + len) as usize * 11];
            segment.insert(0x3f0a, [(len as u32).to_be_bytes().as_slice(), &11u32.to_be_bytes(), rows].concat());
            segment
        };
        let first_segment = split(0, 1);
        let last_segment = split(1, 2);
        scratch.write_segments(&body, &[&first_segment, &last_segment]);
        assert_eq!(check().unwrap(), result);
        for start in [0, 2] {
            let mut disordered = last_segment.clone();
            disordered.insert(0x3f0c, (start as u64).to_be_bytes().to_vec());
            scratch.write_segments(&body, &[&first_segment, &disordered]);
            assert!(check().unwrap_err().contains("contiguous declared picture span"));
        }
        let complete_segment = split(1, 1);
        scratch.write_segments(&body[..coded], &[&first_segment, &complete_segment]);
        let complete = Terminal::physical_indexed(&scratch.0, 2, Profile::ProResLt, false).unwrap();
        assert_eq!(complete.bytes, 0);
        assert_eq!(complete.indexed_offsets, result.indexed_offsets);
        assert!(check().is_err());
        body[coded] = 1;
        scratch.write(&body, &index);
        assert!(check().unwrap_err().contains("nonzero terminal"));
        body[coded] = 0;
        body[first.len() + 37] = 0;
        scratch.write(&body, &index);
        assert!(check().unwrap_err().contains("slice boundary"));
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
    fn prores_freeze_tail_requires_equal_complete_pictures_and_zero_padding() {
        let scratch = Scratch::new();
        let (_, mut fields) = control();
        const SIZE: usize = 40960;
        let mut picture = vec![0; SIZE];
        picture[..4].copy_from_slice(&(SIZE as u32).to_be_bytes());
        picture[4..8].copy_from_slice(b"icpf");
        picture[16..20].copy_from_slice(&[0x07, 0x80, 0x04, 0x38]);
        let mut body = picture.repeat(120);
        body.resize(body.len() + 32000, 0);
        fields.insert(0x3f0b, (24u64 << 32 | 1).to_be_bytes().to_vec());
        fields.insert(0x3f0d, 121u64.to_be_bytes().to_vec());
        let mut array = Vec::from(121u32.to_be_bytes());
        array.extend(11u32.to_be_bytes());
        for n in 0..121 {
            array.extend([0, 0, 128]);
            array.extend(((n * SIZE) as u64).to_be_bytes());
        }
        fields.insert(0x3f0a, array);
        scratch.write(&body, &fields);
        let terminal = Terminal::physical_profile(&scratch.0, 120, Profile::ProResFreeze).unwrap();
        assert_eq!((terminal.frames, terminal.bytes), (120, 32000));
        for offset in [SIZE + 28, 120 * SIZE] {
            body[offset] = 1;
            scratch.write(&body, &fields);
            assert!(Terminal::physical_profile(&scratch.0, 120, Profile::ProResFreeze).is_err());
            body[offset] = 0;
        }
    }
    #[test]
    fn packet_layer_must_match_every_physical_boundary() {
        let t = Terminal {
            frames: 2,
            essence_offset: 100,
            bytes: 512,
            sha256: String::new(),
            header_prefix: Vec::new(),
            frame_bytes: FRAME,
            indexed_offsets: Vec::new(),
            profile: Profile::Dnx,
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
