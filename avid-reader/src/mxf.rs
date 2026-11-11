//! Bounded MXF header reader: just enough of SMPTE ST 377-1 to learn which package UMIDs a
//! file carries, so bin source mobs can be relinked to OP-Atom media by identity rather than
//! by filename. Reads only the header partition; essence is never touched.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::io::{Error, Result};

/// Header metadata is normally tens of kilobytes; Avid pads it with KAG fill. Never read more.
pub const MAX_HEADER_BYTES: u64 = 16 * 1024 * 1024;

const UL_PREFIX: [u8; 4] = [0x06, 0x0E, 0x2B, 0x34];
/// Partition pack: 06 0E 2B 34 02 05 01 01 0D 01 02 01 01 kk ss 00 (kk 02 = header).
const PARTITION: [u8; 13] = [0x06, 0x0E, 0x2B, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0D, 0x01, 0x02, 0x01, 0x01];
/// Local sets in the header metadata: 06 0E 2B 34 02 53 01 01 0D 01 01 01 01 01 xx 00.
const LOCAL_SET: [u8; 14] = [0x06, 0x0E, 0x2B, 0x34, 0x02, 0x53, 0x01, 0x01, 0x0D, 0x01, 0x01, 0x01, 0x01, 0x01];
const MATERIAL_PACKAGE: u8 = 0x36;
const SOURCE_PACKAGE: u8 = 0x37;
const ESSENCE_CONTAINER_DATA: u8 = 0x23;
const TIMECODE_COMPONENT: u8 = 0x14;
const TRACK: u8 = 0x3B;
const SEQUENCE: u8 = 0x0F;
const SOURCE_CLIP: u8 = 0x11;
const TAG_COMPONENTS: u16 = 0x1001;
const TAG_SOURCE_PACKAGE: u16 = 0x1101;
const TAG_SOURCE_START: u16 = 0x1201;
const TAG_INSTANCE_UID: u16 = 0x3C0A;
const TAG_PACKAGE_TRACKS: u16 = 0x4403;
const TAG_TRACK_EDIT_RATE: u16 = 0x4B01;
const TAG_TRACK_ORIGIN: u16 = 0x4B02;
const TAG_TRACK_SEQUENCE: u16 = 0x4803;
const TAG_DURATION: u16 = 0x0202;
const TAG_START_TIMECODE: u16 = 0x1501;
const TAG_ROUNDED_TC_BASE: u16 = 0x1502;
const TAG_DROP_FRAME: u16 = 0x1503;
const TAG_PACKAGE_UID: u16 = 0x4401;
const TAG_LINKED_PACKAGE_UID: u16 = 0x2701;
/// CDCI/RGBA Generic Picture Essence Descriptor, SMPTE ST 377-1 / RDD 44.
const CDCI_DESCRIPTOR: u8 = 0x28;
const RGBA_DESCRIPTOR: u8 = 0x29;
const TAG_PICTURE_ESSENCE_CODING: u16 = 0x3201;
/// SMPTE RDD 44 Table 5: ProRes 422 LT picture-coding label.
pub const PRORES_422_LT_CODING: [u8; 16] = [
    0x06, 0x0E, 0x2B, 0x34, 0x04, 0x01, 0x01, 0x0D,
    0x04, 0x01, 0x02, 0x02, 0x03, 0x06, 0x02, 0x00,
];
/// SMPTE RDD 44 ProRes 422 HQ picture-coding label, seen in the qualified freeze render.
pub const PRORES_422_HQ_CODING: [u8; 16] = [
    0x06, 0x0E, 0x2B, 0x34, 0x04, 0x01, 0x01, 0x0D,
    0x04, 0x01, 0x02, 0x02, 0x03, 0x06, 0x04, 0x00,
];
/// Exact native DNxHR LB picture-coding label observed in the paired native fixture.
/// FFmpeg n8.1's VC-3 codec-UL family corroborates this label; RI00 alone is generic.
pub const DNXHR_LB_CODING: [u8; 16] = [
    0x06, 0x0E, 0x2B, 0x34, 0x04, 0x01, 0x01, 0x0D,
    0x04, 0x01, 0x02, 0x02, 0x71, 0x28, 0x00, 0x00,
];

/// A qualified file clock with the exact physical timecode-track rate retained.
/// The rounded display base is never a substitute for this rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedClock {
    pub effective_count: u64,
    pub rate: (i64, i64),
    pub rounded_base: u16,
    pub drop: bool,
    /// File SourceClip offset, in `source_rate` units, before exact conversion.
    pub source_offset: i64,
    pub source_rate: (i64, i64),
    pub origin_count: u64,
    pub source_package: String,
    pub timecode_track: String,
    pub route: String,
}

impl QualifiedClock {
    fn valid(&self) -> bool {
        if self.rate.0 <= 0 || self.rate.1 <= 0 || self.source_rate.0 <= 0
            || self.source_rate.1 <= 0 || self.rounded_base == 0 || self.source_offset < 0 {
            return false;
        }
        let numerator = i128::from(self.source_offset)
            .checked_mul(i128::from(self.rate.0))
            .and_then(|n| n.checked_mul(i128::from(self.source_rate.1)));
        let denominator = i128::from(self.rate.1).checked_mul(i128::from(self.source_rate.0));
        let (Some(n), Some(d)) = (numerator, denominator) else { return false; };
        n % d == 0 && i128::from(self.origin_count).checked_add(n / d)
            == Some(i128::from(self.effective_count))
    }
    fn same_clock(&self, other: &Self) -> bool {
        self.effective_count == other.effective_count
            && self.rate == other.rate
            && self.rounded_base == other.rounded_base
            && self.drop == other.drop
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MxfIdentity {
    /// 32-byte UMIDs as 64 uppercase hex digits.
    pub material_packages: Vec<String>,
    pub source_packages: Vec<String>,
    /// The source package(s) the essence container data links to: the file package.
    pub file_packages: Vec<String>,
    /// Every TimecodeComponent: (start frame, rounded base, drop frame).
    pub timecodes: Vec<(u64, u16, bool)>,
    /// Essence track(s) of the file package: (edit rate num, den, sequence duration). This is
    /// the length Premiere's own MXF import uses (PRPROJ-0018: SPOOGE 1,064,000 samples, while
    /// the Avid descriptor says 1,062,001).
    pub file_tracks: Vec<(i64, i64, i64)>,
    /// Present only when all picture descriptors in this bounded header agree.
    pub picture_essence_coding: Option<[u8; 16]>,
    /// Effective file-track timecodes resolved through SourceClip -> physical package.
    pub file_timecodes: Vec<(u64, u16, bool)>,
    /// Exact-rate witnesses, one per resolved file essence track. Separate from the
    /// legacy tuple API so stronger qualification cannot change its behavior.
    pub qualified_file_clocks: Vec<QualifiedClock>,
    /// Every file essence sequence has one zero-origin SourceClip with null upstream ID.
    /// This permits the existing standalone-file clock fallback, not unresolved physical routes.
    pub file_origins_zero: bool,
    /// File sequences other than direct TimecodeComponent metadata tracks.
    pub file_essence_tracks: usize,
}

impl MxfIdentity {
    /// The file's start timecode when all its timecode components agree (Avid OP-Atom files
    /// carry one, on the source package the file package derives from).
    pub fn start_timecode(&self) -> Option<(u64, u16, bool)> {
        let first = *self.timecodes.first()?;
        self.timecodes.iter().all(|t| *t == first).then_some(first)
    }

    /// Rewrap clock: retain the raw physical-origin components above, but apply
    /// qualified file SourceClip offsets at the referenced timecode track's rate.
    /// Without a resolved physical clock, the old clock is safe only at zero origin.
    pub fn file_start_timecode(&self) -> Option<(u64, u16, bool)> {
        if self.file_essence_tracks > 0 && self.file_timecodes.len() == self.file_essence_tracks {
            let first = self.file_timecodes[0];
            return self.file_timecodes.iter().all(|tc| *tc == first).then_some(first);
        }
        self.file_origins_zero.then(|| self.start_timecode()).flatten()
    }

    /// No rounded-rate or zero-origin guess is made when an exact witness is absent.
    /// Different physical routes may establish the same clock; provenance is retained
    /// per route but is not itself a clock disagreement.
    pub fn qualified_file_clock(&self) -> Option<QualifiedClock> {
        if self.file_essence_tracks == 0
            || self.qualified_file_clocks.len() != self.file_essence_tracks
        {
            return None;
        }
        let first = self.qualified_file_clocks.first()?;
        self.qualified_file_clocks
            .iter()
            .all(|clock| clock.valid() && clock.same_clock(first))
            .then(|| first.clone())
    }
}

pub fn read_identity(path: &Path) -> Result<MxfIdentity> {
    let file = File::open(path).map_err(|e| Error::new(format!("{}: {}", path.display(), e), 0))?;
    let mut data = Vec::new();
    file.take(MAX_HEADER_BYTES)
        .read_to_end(&mut data)
        .map_err(|e| Error::new(format!("{}: {}", path.display(), e), 0))?;
    parse_identity(&data)
}

fn ber_length(data: &[u8], pos: &mut usize) -> Result<usize> {
    let first = *data.get(*pos).ok_or_else(|| Error::new("truncated BER length", *pos))?;
    *pos += 1;
    if first < 0x80 {
        return Ok(first as usize);
    }
    let n = (first & 0x7F) as usize;
    if n == 0 || n > 8 || *pos + n > data.len() {
        return Err(Error::new("bad BER length", *pos - 1));
    }
    let mut v: u64 = 0;
    for &b in &data[*pos..*pos + n] {
        v = (v << 8) | b as u64;
    }
    *pos += n;
    usize::try_from(v).map_err(|_| Error::new("BER length overflow", *pos))
}

/// One KLV: (key, value range). Fails if the value runs past the buffer.
fn klv(data: &[u8], pos: usize) -> Result<([u8; 16], usize, usize)> {
    if pos + 16 > data.len() {
        return Err(Error::new("truncated KLV key", pos));
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&data[pos..pos + 16]);
    let mut p = pos + 16;
    let len = ber_length(data, &mut p)?;
    if len > data.len() - p {
        return Err(Error::new(format!("KLV value of {} bytes runs past the header buffer", len), pos));
    }
    Ok((key, p, p + len))
}

fn hex32(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02X}", x)).collect()
}

pub fn parse_identity(data: &[u8]) -> Result<MxfIdentity> {
    // The header partition pack must come first (no run-in support).
    let (key, start, end) = klv(data, 0)?;
    if key[..13] != PARTITION || key[13] != 0x02 {
        return Err(Error::new("not an MXF header partition", 0));
    }
    if end - start < 64 {
        return Err(Error::new("partition pack too short", start));
    }
    let header_byte_count = u64::from_be_bytes(data[start + 32..start + 40].try_into().unwrap());
    if header_byte_count == 0 {
        return Err(Error::new("header partition has no header metadata", start));
    }
    let mut pos = end;
    // Fill may sit between the partition pack and the primer; HeaderByteCount starts at the primer.
    let mut header_end: Option<usize> = None;
    let mut id = MxfIdentity::default();
    // Local sets by InstanceUID, for following strong references (kind, value range).
    let mut sets: std::collections::HashMap<Vec<u8>, (u8, usize, usize)> = std::collections::HashMap::new();
    let mut source_packages: Vec<(String, usize, usize)> = Vec::new();
    let mut ambiguous_instance_ids = false;
    let mut picture_codings = Vec::<[u8; 16]>::new();
    while pos < data.len() {
        if let Some(limit) = header_end {
            if pos >= limit {
                break;
            }
        }
        // Avid audio headers also carry private sets keyed by non-SMPTE UUIDs (KB IDENTITY-0006);
        // only framing matters for those, so any 16-byte key is accepted and skipped.
        let (key, vstart, vend) = klv(data, pos)?;
        let is_fill = key[..4] == UL_PREFIX && key[4] == 0x01 && key[8..13] == [0x03, 0x01, 0x02, 0x10, 0x01];
        let is_primer = key[..13] == PARTITION && key[13] == 0x05 && key[14] == 0x01;
        if header_end.is_none() && !is_fill {
            if key[..4] != UL_PREFIX {
                return Err(Error::new("expected a SMPTE UL before the primer pack", pos));
            }
            if !is_primer {
                return Err(Error::new("header metadata does not start with a primer pack", pos));
            }
            let limit = (pos as u64).saturating_add(header_byte_count);
            if limit > data.len() as u64 {
                return Err(Error::new("header metadata is larger than the read limit", pos));
            }
            header_end = Some(limit as usize);
        }
        if key[..14] == LOCAL_SET && key[15] == 0x00 {
            if let Some(uid) = local_item(&data[vstart..vend], TAG_INSTANCE_UID, vstart)? {
                if sets.insert(uid.to_vec(), (key[14], vstart, vend)).is_some() {
                    ambiguous_instance_ids = true;
                }
            }
        }
        if key[..14] == LOCAL_SET && key[15] == 0x00 && (key[14] == CDCI_DESCRIPTOR || key[14] == RGBA_DESCRIPTOR) {
            if let Some(coding) = local_item(&data[vstart..vend], TAG_PICTURE_ESSENCE_CODING, vstart)? {
                if coding.len() != 16 {
                    return Err(Error::new("picture essence coding is not a 16-byte UL", vstart));
                }
                let coding: [u8; 16] = coding.try_into().unwrap();
                if !picture_codings.contains(&coding) {
                    picture_codings.push(coding);
                }
            }
        }
        if key[..14] == LOCAL_SET && key[15] == 0x00 && key[14] == TIMECODE_COMPONENT {
            let set = &data[vstart..vend];
            let start = local_item(set, TAG_START_TIMECODE, vstart)?;
            let base = local_item(set, TAG_ROUNDED_TC_BASE, vstart)?;
            let drop = local_item(set, TAG_DROP_FRAME, vstart)?;
            if let (Some(start), Some(base)) = (start, base) {
                if start.len() != 8 || base.len() != 2 || drop.map_or(false, |d| d.len() != 1) {
                    return Err(Error::new("malformed timecode component", vstart));
                }
                id.timecodes.push((
                    u64::from_be_bytes(start.try_into().unwrap()),
                    u16::from_be_bytes(base.try_into().unwrap()),
                    drop.map_or(false, |d| d[0] != 0),
                ));
            }
        }
        if key[..14] == LOCAL_SET && key[15] == 0x00 {
            let kind = key[14];
            if kind == MATERIAL_PACKAGE || kind == SOURCE_PACKAGE || kind == ESSENCE_CONTAINER_DATA {
                let wanted = if kind == ESSENCE_CONTAINER_DATA { TAG_LINKED_PACKAGE_UID } else { TAG_PACKAGE_UID };
                if let Some(uid) = local_item(&data[vstart..vend], wanted, vstart)? {
                    if uid.len() != 32 {
                        return Err(Error::new("package UID is not 32 bytes", vstart));
                    }
                    let uid = hex32(uid);
                    match kind {
                        MATERIAL_PACKAGE => id.material_packages.push(uid),
                        SOURCE_PACKAGE => {
                            source_packages.push((uid.clone(), vstart, vend));
                            id.source_packages.push(uid)
                        }
                        _ => id.file_packages.push(uid),
                    }
                }
            }
        }
        pos = vend;
    }
    if header_end.is_none() {
        return Err(Error::new("no header metadata found", pos));
    }
    if picture_codings.len() == 1 {
        id.picture_essence_coding = Some(picture_codings[0]);
    }
    // File package -> its tracks -> each track's sequence duration and edit rate.
    let mut origins = Vec::new();
    for (uid, s0, s1) in &source_packages {
        if !id.file_packages.contains(uid) {
            continue;
        }
        let Some(batch) = local_item(&data[*s0..*s1], TAG_PACKAGE_TRACKS, *s0)? else { continue };
        if batch.len() < 8 {
            continue;
        }
        let count = u32::from_be_bytes(batch[0..4].try_into().unwrap()) as usize;
        let size = u32::from_be_bytes(batch[4..8].try_into().unwrap()) as usize;
        if size != 16 || batch.len() < 8 + count * 16 {
            return Err(Error::new("malformed package track batch", *s0));
        }
        for i in 0..count {
            let track_uid = &batch[8 + i * 16..8 + (i + 1) * 16];
            let Some(&(TRACK, t0, t1)) = sets.get(track_uid) else { continue };
            let track = &data[t0..t1];
            let (Some(rate), Some(seq_ref)) = (local_item(track, TAG_TRACK_EDIT_RATE, t0)?, local_item(track, TAG_TRACK_SEQUENCE, t0)?) else { continue };
            if rate.len() != 8 {
                continue;
            }
            let Some(&(SEQUENCE, q0, q1)) = sets.get(seq_ref) else { continue };
            let Some(duration) = local_item(&data[q0..q1], TAG_DURATION, q0)? else { continue };
            if duration.len() != 8 {
                continue;
            }
            id.file_tracks.push((
                i32::from_be_bytes(rate[0..4].try_into().unwrap()) as i64,
                i32::from_be_bytes(rate[4..8].try_into().unwrap()) as i64,
                i64::from_be_bytes(duration.try_into().unwrap()),
            ));
            let refs = local_item(&data[q0..q1], TAG_COMPONENTS, q0)?
                .map(references).transpose()?.unwrap_or_default();
            if let [only] = refs.as_slice() {
                if sets.get(*only).is_some_and(|(kind, _, _)| *kind == TIMECODE_COMPONENT) {
                    continue; // A direct file-package TC track is metadata, not an essence origin.
                }
            }
            id.file_essence_tracks += 1;
            if !zero_track_origin(track, t0)? { continue; }
            // Clock interpretation is a narrower bounded addition to identity scanning.
            if id.file_essence_tracks <= 64 {
                if let [only] = refs.as_slice() {
                    if let Some(&(SOURCE_CLIP, c0, c1)) = sets.get(*only) {
                        if let Some(start) = local_item(&data[c0..c1], TAG_SOURCE_START, c0)? {
                            if start.len() == 8 {
                                if local_item(&data[c0..c1], TAG_SOURCE_PACKAGE, c0)? == Some(&[0;32][..]) {
                                    origins.push(i64::from_be_bytes(start.try_into().unwrap()));
                                }
                            }
                        }
                        let source_rate = (i32::from_be_bytes(rate[..4].try_into().unwrap()),
                                           i32::from_be_bytes(rate[4..].try_into().unwrap()));
                        let clocks = file_clock_witnesses(data, &sets, &source_packages, c0, c1, source_rate)?;
                        if let Some(first) = clocks.first() {
                            let tuple = (first.effective_count, first.rounded_base, first.drop);
                            if clocks.iter().all(|c| (c.effective_count, c.rounded_base, c.drop) == tuple) {
                                id.file_timecodes.push(tuple);
                            }
                            if !ambiguous_instance_ids && clocks.iter().all(|c| c.valid() && c.same_clock(first)) {
                                id.qualified_file_clocks.push(first.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    id.file_origins_zero = id.file_essence_tracks > 0 && origins.len() == id.file_essence_tracks
        && origins.iter().all(|n| *n == 0);
    Ok(id)
}

fn references(batch: &[u8]) -> Result<Vec<&[u8]>> {
    if batch.len() < 8 {
        return Err(Error::new("truncated strong-reference batch", 0));
    }
    let count = u32::from_be_bytes(batch[..4].try_into().unwrap()) as usize;
    let width = u32::from_be_bytes(batch[4..8].try_into().unwrap());
    if width != 16 || (batch.len() - 8) / 16 != count || (batch.len() - 8) % 16 != 0 {
        return Err(Error::new("malformed strong-reference batch", 0));
    }
    // Wider graphs stay readable as identities, but are not clock-qualified here.
    if count > 64 {
        return Ok(Vec::new());
    }
    Ok(batch[8..].chunks_exact(16).collect())
}

fn zero_track_origin(track: &[u8], base: usize) -> Result<bool> {
    if let Some(origin) = local_item(track, TAG_TRACK_ORIGIN, base)? {
        if origin.len() != 8 || origin != [0; 8] {
            return Ok(false); // Still an identity; its clock is unqualified.
        }
    }
    Ok(true)
}

// Narrow one-SourceClip file route; no nested compositions, rounding or guessed origins.
fn file_clock_witnesses(
    data: &[u8],
    sets: &std::collections::HashMap<Vec<u8>, (u8, usize, usize)>,
    packages: &[(String, usize, usize)],
    c0: usize,
    c1: usize,
    rate: (i32, i32),
) -> Result<Vec<QualifiedClock>> {
    if packages.len() > 1024 {
        return Ok(Vec::new());
    }
    let clip = &data[c0..c1];
    let (Some(package), Some(start)) = (
        local_item(clip, TAG_SOURCE_PACKAGE, c0)?,
        local_item(clip, TAG_SOURCE_START, c0)?,
    ) else {
        return Ok(Vec::new());
    };
    if package.len() != 32 || start.len() != 8 || rate.0 <= 0 || rate.1 <= 0 {
        return Ok(Vec::new());
    }
    let start = i64::from_be_bytes(start.try_into().unwrap());
    if start < 0 {
        return Ok(Vec::new());
    }
    let uid = hex32(package);
    let selected: Vec<_> = packages.iter().filter(|(id, _, _)| *id == uid).collect();
    let [(_, p0, p1)] = selected.as_slice() else {
        return Ok(Vec::new());
    };
    let Some(tracks) = local_item(&data[*p0..*p1], TAG_PACKAGE_TRACKS, *p0)? else {
        return Ok(Vec::new());
    };
    let mut clocks = Vec::new();
    for track in references(tracks)? {
        let Some(&(TRACK, t0, t1)) = sets.get(track) else {
            continue;
        };
        if !zero_track_origin(&data[t0..t1], t0)? { return Ok(Vec::new()); }
        let (Some(physical_rate), Some(sequence)) = (
            local_item(&data[t0..t1], TAG_TRACK_EDIT_RATE, t0)?,
            local_item(&data[t0..t1], TAG_TRACK_SEQUENCE, t0)?,
        ) else {
            continue;
        };
        if physical_rate.len() != 8 {
            continue;
        }
        let (n, d) = (
            i32::from_be_bytes(physical_rate[..4].try_into().unwrap()),
            i32::from_be_bytes(physical_rate[4..].try_into().unwrap()),
        );
        if n <= 0 || d <= 0 {
            continue;
        }
        let Some(&(SEQUENCE, q0, q1)) = sets.get(sequence) else {
            continue;
        };
        let Some(components) = local_item(&data[q0..q1], TAG_COMPONENTS, q0)? else {
            continue;
        };
        let refs = references(components)?;
        let [only] = refs.as_slice() else {
            continue;
        };
        let Some(&(TIMECODE_COMPONENT, v0, v1)) = sets.get(*only) else {
            continue;
        };
        let set = &data[v0..v1];
        let (Some(frames), Some(base), Some(drop)) = (
            local_item(set, TAG_START_TIMECODE, v0)?,
            local_item(set, TAG_ROUNDED_TC_BASE, v0)?,
            local_item(set, TAG_DROP_FRAME, v0)?,
        ) else {
            continue;
        };
        if frames.len() != 8 || base.len() != 2 || drop.len() != 1 || drop[0] > 1 {
            continue;
        }
        let numerator = i128::from(start) * i128::from(n) * i128::from(rate.1);
        let denominator = i128::from(d) * i128::from(rate.0);
        if numerator % denominator != 0 {
            return Ok(Vec::new());
        }
        let value =
            i128::from(u64::from_be_bytes(frames.try_into().unwrap())) + numerator / denominator;
        let Ok(value) = u64::try_from(value) else {
            return Ok(Vec::new());
        };
        let reduce = |n: i64, d: i64| {
            let (mut a, mut b) = (n, d);
            while b != 0 { (a, b) = (b, a % b); }
            (n / a, d / a)
        };
        clocks.push(QualifiedClock {
            effective_count: value,
            rate: reduce(i64::from(n), i64::from(d)),
            rounded_base: u16::from_be_bytes(base.try_into().unwrap()),
            drop: drop[0] != 0,
            source_offset: start,
            source_rate: reduce(i64::from(rate.0), i64::from(rate.1)),
            origin_count: u64::from_be_bytes(frames.try_into().unwrap()),
            source_package: uid.clone(),
            timecode_track: hex32(track),
            route: "file_source_clip_to_physical_timecode_track".into(),
        });
    }
    Ok(clocks)
}

/// Find a 2-byte-tag, 2-byte-length local item in a set's value.
fn local_item(set: &[u8], tag: u16, base: usize) -> Result<Option<&[u8]>> {
    let mut p = 0;
    while p < set.len() {
        if p + 4 > set.len() {
            return Err(Error::new("truncated local set item", base + p));
        }
        let t = u16::from_be_bytes([set[p], set[p + 1]]);
        let len = u16::from_be_bytes([set[p + 2], set[p + 3]]) as usize;
        p += 4;
        if len > set.len() - p {
            return Err(Error::new("local set item runs past its set", base + p));
        }
        if t == tag {
            return Ok(Some(&set[p..p + len]));
        }
        p += len;
    }
    Ok(None)
}

/// Every `.mxf` under `root` (depth-limited), sorted for deterministic output.
pub fn find_mxf_files(root: &Path, max_depth: usize, max_files: usize) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| Error::new(format!("{}: {}", dir.display(), e), 0))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::new(format!("{}: {}", dir.display(), e), 0))?;
            let kind = entry.file_type().map_err(|e| Error::new(e.to_string(), 0))?;
            let path = entry.path();
            if kind.is_dir() && depth < max_depth {
                stack.push((path, depth + 1));
            } else if kind.is_file()
                && path.extension().map_or(false, |e| e.eq_ignore_ascii_case("mxf"))
                && !path.file_name().map_or(false, |n| n.to_string_lossy().starts_with("._"))
            {
                out.push(path);
                if out.len() > max_files {
                    return Err(Error::new(format!("more than {} MXF files under {}", max_files, root.display()), 0));
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(kind: u8, items: &[(u16, &[u8])]) -> Vec<u8> {
        let mut value = Vec::new();
        for (tag, v) in items {
            value.extend_from_slice(&tag.to_be_bytes());
            value.extend_from_slice(&(v.len() as u16).to_be_bytes());
            value.extend_from_slice(v);
        }
        let mut out = LOCAL_SET.to_vec();
        out.extend_from_slice(&[kind, 0x00, 0x83]);
        out.extend_from_slice(&(value.len() as u32).to_be_bytes()[1..]);
        out.extend_from_slice(&value);
        out
    }

    fn header(body: &[u8]) -> Vec<u8> {
        let mut out = PARTITION.to_vec();
        out.extend_from_slice(&[0x02, 0x04, 0x00, 0x58]);
        let mut pack = vec![0u8; 88];
        pack[32..40].copy_from_slice(&(body.len() as u64).to_be_bytes());
        out.extend_from_slice(&pack);
        out.extend_from_slice(body);
        out
    }

    fn primer() -> Vec<u8> {
        let mut out = PARTITION.to_vec();
        out.extend_from_slice(&[0x05, 0x01, 0x00, 0x08, 0, 0, 0, 0, 0, 0, 0, 18]);
        out
    }

    #[test]
    fn finds_material_source_and_file_packages() {
        let m = [0xAAu8; 32];
        let f = [0xBBu8; 32];
        let t = [0xCCu8; 32];
        let mut body = primer();
        body.extend(set(MATERIAL_PACKAGE, &[(0x3C0A, &[0; 16]), (TAG_PACKAGE_UID, &m)]));
        body.extend(set(SOURCE_PACKAGE, &[(TAG_PACKAGE_UID, &f)]));
        body.extend(set(SOURCE_PACKAGE, &[(TAG_PACKAGE_UID, &t)]));
        body.extend(set(ESSENCE_CONTAINER_DATA, &[(TAG_LINKED_PACKAGE_UID, &f)]));
        body.extend(set(TIMECODE_COMPONENT, &[(TAG_ROUNDED_TC_BASE, &24u16.to_be_bytes()), (TAG_START_TIMECODE, &86400u64.to_be_bytes()), (TAG_DROP_FRAME, &[0])]));
        let id = parse_identity(&header(&body)).unwrap();
        assert_eq!(id.start_timecode(), Some((86400, 24, false)));
        assert_eq!(id.material_packages, vec!["AA".repeat(32)]);
        assert_eq!(id.source_packages, vec!["BB".repeat(32), "CC".repeat(32)]);
        assert_eq!(id.file_packages, vec!["BB".repeat(32)]);
    }

    #[test]
    fn truncations_fail_cleanly() {
        let mut body = primer();
        body.extend(set(SOURCE_PACKAGE, &[(TAG_PACKAGE_UID, &[1; 32])]));
        let full = header(&body);
        for n in 0..full.len() {
            assert!(parse_identity(&full[..n]).is_err(), "prefix {}", n);
        }
        assert!(parse_identity(&full).is_ok());
    }

    #[test]
    fn reads_only_unambiguous_bounded_picture_coding() {
        let mut body = primer();
        body.extend(set(CDCI_DESCRIPTOR, &[(TAG_PICTURE_ESSENCE_CODING, &PRORES_422_LT_CODING)]));
        assert_eq!(parse_identity(&header(&body)).unwrap().picture_essence_coding, Some(PRORES_422_LT_CODING));
        let mut other = PRORES_422_LT_CODING;
        other[14] = 3; // ProRes 422, not LT.
        body.extend(set(CDCI_DESCRIPTOR, &[(TAG_PICTURE_ESSENCE_CODING, &other)]));
        assert_eq!(parse_identity(&header(&body)).unwrap().picture_essence_coding, None);
        let mut malformed = primer();
        malformed.extend(set(CDCI_DESCRIPTOR, &[(TAG_PICTURE_ESSENCE_CODING, &[0; 15])]));
        assert!(parse_identity(&header(&malformed)).is_err());
    }

    #[test]
    fn file_clock_follows_physical_source_and_exact_rate_conversion() {
        let fixture = |offset: i64| {
            let file = [0xBB;32]; let physical = [0xCC;32];
            let batch = |uid: u8| { let mut b = vec![0,0,0,1,0,0,0,16]; b.extend([uid;16]); b };
            let mut body = primer();
            body.extend(set(SOURCE_PACKAGE, &[(TAG_PACKAGE_UID,&file),(TAG_PACKAGE_TRACKS,&batch(1))]));
            body.extend(set(SOURCE_PACKAGE, &[(TAG_PACKAGE_UID,&physical),(TAG_PACKAGE_TRACKS,&batch(2))]));
            body.extend(set(ESSENCE_CONTAINER_DATA, &[(TAG_LINKED_PACKAGE_UID,&file)]));
            body.extend(set(TRACK, &[(TAG_INSTANCE_UID,&[1;16]),(TAG_TRACK_SEQUENCE,&[3;16]),(TAG_TRACK_EDIT_RATE,&[0,0,187,128,0,0,0,1])]));
            body.extend(set(TRACK, &[(TAG_INSTANCE_UID,&[2;16]),(TAG_TRACK_SEQUENCE,&[4;16]),(TAG_TRACK_EDIT_RATE,&[0,0,93,192,0,0,3,233]),(TAG_TRACK_ORIGIN,&0i64.to_be_bytes())]));
            body.extend(set(SEQUENCE, &[(TAG_INSTANCE_UID,&[3;16]),(TAG_COMPONENTS,&batch(5)),(TAG_DURATION,&72072i64.to_be_bytes())]));
            body.extend(set(SEQUENCE, &[(TAG_INSTANCE_UID,&[4;16]),(TAG_COMPONENTS,&batch(6))]));
            body.extend(set(SOURCE_CLIP, &[(TAG_INSTANCE_UID,&[5;16]),(TAG_SOURCE_PACKAGE,&physical),(TAG_SOURCE_START,&offset.to_be_bytes())]));
            body.extend(set(TIMECODE_COMPONENT, &[(TAG_INSTANCE_UID,&[6;16]),(TAG_START_TIMECODE,&1387746u64.to_be_bytes()),(TAG_ROUNDED_TC_BASE,&24u16.to_be_bytes()),(TAG_DROP_FRAME,&[0])]));
            header(&body)
        };
        let id = parse_identity(&fixture(4004)).unwrap();
        assert_eq!(id.start_timecode(), Some((1387746,24,false))); // Raw origin unchanged.
        assert_eq!(id.file_start_timecode(), Some((1387748,24,false))); // 4004 samples = 2 frames.
        let exact = id.qualified_file_clock().unwrap();
        assert_eq!(exact.effective_count, 1387748);
        assert_eq!(exact.rate, (24000,1001));
        assert_eq!(exact.source_rate, (48000,1));
        assert_eq!(exact.source_offset, 4004);
        assert_eq!(exact.origin_count, 1387746);
        assert_eq!(exact.source_package, "CC".repeat(32));
        assert_eq!(parse_identity(&fixture(0)).unwrap().file_start_timecode(), Some((1387746,24,false)));
        assert_eq!(parse_identity(&fixture(4003)).unwrap().file_start_timecode(), None); // No rounding.
        assert_eq!(parse_identity(&fixture(-4004)).unwrap().file_start_timecode(), None);
        let mut unresolved = fixture(4004);
        let at = unresolved.windows(32).rposition(|b| b == [0xCC;32]).unwrap();
        unresolved[at..at+32].fill(0xDD);
        assert_eq!(parse_identity(&unresolved).unwrap().file_start_timecode(), None);
        let mut wide = vec![0,0,0,65,0,0,0,16]; wide.extend([1;65*16]);
        assert!(references(&wide).unwrap().is_empty());
        let mut origin = fixture(0);
        let at = origin.windows(4).position(|b| b == [0x4b,2,0,8]).unwrap();
        origin[at+4..at+12].copy_from_slice(&1i64.to_be_bytes());
        let id = parse_identity(&origin).unwrap(); // Identity remains available.
        assert_eq!(id.start_timecode(), Some((1387746,24,false)));
        assert_eq!(id.file_start_timecode(), None); // No fallback around unknown origins.
    }
    #[test]
    fn exact_clock_agreement_includes_rate_but_not_route_identity() {
        let clock = QualifiedClock {
            effective_count: 100, rate: (24,1), rounded_base: 24, drop: false,
            source_offset: 0, source_rate: (48000,1), origin_count: 100,
            source_package: "AA".repeat(32), timecode_track: "01".repeat(16), route: "first route".into(),
        };
        let mut id = MxfIdentity { file_essence_tracks: 2,
            file_timecodes: vec![(100,24,false);2], qualified_file_clocks: vec![clock.clone(),clock], ..Default::default() };
        id.qualified_file_clocks[1].source_package = "BB".repeat(32);
        id.qualified_file_clocks[1].timecode_track = "02".repeat(16);
        id.qualified_file_clocks[1].route = "other route".into();
        assert!(id.qualified_file_clock().is_some());
        id.qualified_file_clocks[1].rate = (24000,1001);
        assert_eq!(id.file_start_timecode(), Some((100,24,false))); // Legacy tuple remains unchanged.
        assert!(id.qualified_file_clock().is_none());
        id.qualified_file_clocks[1].rate = (24,1);
        id.qualified_file_clocks[1].source_rate = (0,1);
        assert!(id.qualified_file_clock().is_none());
        id.qualified_file_clocks.clear();
        id.file_origins_zero = true;
        id.timecodes = vec![(100,24,false)];
        assert_eq!(id.file_start_timecode(), Some((100,24,false)));
        assert!(id.qualified_file_clock().is_none()); // No fabricated exact-rate witness.
    }

}
