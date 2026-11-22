//! Bounded picture verification for the RED2 recording layout used by the
//! linked Premiere export. Premiere reads the essence from the first part.
use crate::{
    media::{stamp, FileStamp},
    prepare::frames_on,
    Result,
};
use graph::{
    source::{OriginalStream, SourceMedia, VERIFIED_LINKED_R3D_TIMECODE},
    Rational, StreamKind,
};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_PARTS: u16 = 999;
const FOOTER_OFFSET: i64 = -4096;
const HEADER_SIZE: usize = 161;
const FOOTER_SIZE: usize = 104;
const TIMECODE_DAY: u64 = 24 * 60 * 60 * 24;

pub(super) struct Recording {
    parts: Vec<(PathBuf, PathBuf, FileStamp)>,
    rate: Rational,
    raster: (i64, i64),
    timecode: u64,
    frames: i64,
}

pub(super) fn is_red(source: &SourceMedia) -> bool {
    source.codec.as_deref() == Some("RED")
        || source.locations.first().is_some_and(|locator| {
            Path::new(locator)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("r3d"))
        })
}

impl Recording {
    pub(super) fn open(first: &Path) -> Result<Self> {
        let name = first
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("missing RED recording name")?;
        let (stem, ext) = name
            .rsplit_once('.')
            .ok_or("missing RED recording extension")?;
        let prefix = stem
            .strip_suffix("_001")
            .filter(|s| !s.is_empty())
            .ok_or("RED link must start at recording part _001")?;
        if !first.is_absolute() || !ext.eq_ignore_ascii_case("r3d") {
            return Err("expected an absolute RED recording locator".into());
        }
        let first = Part::open(first, 1)?;
        let mut recording = Self {
            parts: Vec::new(),
            rate: first.rate,
            raster: first.raster,
            timecode: first.timecode,
            frames: 0,
        };
        let id = first.id;
        let folder = first
            .path
            .parent()
            .ok_or("missing RED recording folder")?
            .to_owned();
        let mut part = first;
        for number in 1..=MAX_PARTS {
            if part.id != id || part.rate != recording.rate || part.raster != recording.raster {
                return Err(
                    "RED recording parts disagree about clip identity, rate or raster".into(),
                );
            }
            let complete = match (&part.end, number) {
                (b"REOB", 1) | (b"REOC", 2..) => true,
                (b"REOF", _) => false,
                _ => return Err("unexpected RED recording part footer".into()),
            };
            if part.timecode != (recording.timecode + recording.frames as u64) % TIMECODE_DAY {
                return Err("RED recording part has discontinuous source timecode".into());
            }
            recording.frames += part.frames;
            if recording.frames != part.cumulative_frames {
                return Err("RED recording part has a discontinuous picture extent".into());
            }
            recording
                .parts
                .push((part.path, part.canonical, part.stamp));
            if complete {
                recording.recheck()?;
                return Ok(recording);
            }
            let next = folder.join(format!("{prefix}_{:03}.{ext}", number + 1));
            if number == MAX_PARTS {
                break;
            }
            part = Part::open(&next, number + 1)
                .map_err(|error| format!("RED recording part {}: {error}", next.display()))?;
        }
        Err("RED recording part limit exceeded".into())
    }

    pub(super) fn bind(&self, source: &mut SourceMedia) -> Result<()> {
        if source.kind != StreamKind::Video
            || source.stream != Some(1)
            || source.codec.as_deref() != Some("RED")
            || source.rate != Some(self.rate)
            || source.frame_size != Some(self.raster)
            || source.online == Some(false)
            || frames_on(source.duration, self.rate).ok() != Some(self.frames)
        {
            return Err("saved Resolve RED picture facts contradict the recording".into());
        }
        let clock = source
            .clock
            .as_mut()
            .filter(|clock| {
                clock.effective_count == self.timecode
                    && !clock.drop
                    && clock.display_base == 24
                    && clock.rate == self.rate
                    && clock.exact_start() == source.start
            })
            .ok_or("saved Resolve RED timecode contradicts the recording")?;
        let time_base = Rational::new(self.rate.den().into(), self.rate.num().into()).unwrap();
        let original = OriginalStream {
            saved_locators: source.locations.clone(),
            container: "r3d".into(),
            stream_index: 0,
            channel_index: None,
            channels: None,
            time_base,
            start_pts: 0,
            duration_ts: self.frames,
            sample_representation: None,
        };
        if source
            .original
            .as_ref()
            .is_some_and(|saved| saved != &original)
        {
            return Err("contradictory original RED stream facts".into());
        }
        clock.provenance.route = VERIFIED_LINKED_R3D_TIMECODE.into();
        source.original = Some(original);
        Ok(())
    }

    pub(super) fn recheck(&self) -> Result<()> {
        self.recheck_at(&self.parts[0].0)
    }

    pub(super) fn recheck_at(&self, first: &Path) -> Result<()> {
        let folder = first.parent().ok_or("missing RED recording folder")?;
        if first.file_name() != self.parts[0].0.file_name() {
            return Err("verified RED recording name changed".into());
        }
        for (path, canonical, original) in &self.parts {
            let path = folder.join(path.file_name().ok_or("missing RED recording part name")?);
            if fs::canonicalize(&path)? != *canonical || stamp(fs::metadata(&path)?)? != *original {
                return Err(
                    format!("verified RED recording part changed: {}", path.display()).into(),
                );
            }
        }
        Ok(())
    }
}

struct Part {
    path: PathBuf,
    canonical: PathBuf,
    stamp: FileStamp,
    id: [u8; 16],
    raster: (i64, i64),
    rate: Rational,
    timecode: u64,
    end: [u8; 4],
    frames: i64,
    cumulative_frames: i64,
}

impl Part {
    fn open(path: &Path, number: u16) -> Result<Self> {
        let canonical = fs::canonicalize(path)?;
        let mut file = File::open(&canonical)?;
        let stamp = stamp(file.metadata()?)?;
        if stamp.bytes < 8192 {
            return Err("truncated RED recording part".into());
        }
        let mut header = [0; HEADER_SIZE];
        file.read_exact(&mut header)?;
        file.seek(SeekFrom::End(FOOTER_OFFSET))?;
        let mut footer = [0; FOOTER_SIZE];
        file.read_exact(&mut footer)?;
        Self::parse(path.to_owned(), canonical, stamp, number, &header, &footer)
    }

    fn parse(
        path: PathBuf,
        canonical: PathBuf,
        stamp: FileStamp,
        number: u16,
        header: &[u8; HEADER_SIZE],
        footer: &[u8; FOOTER_SIZE],
    ) -> Result<Self> {
        if &header[4..16] != b"RED2\x05\x10R2\x05\x01\x0c\x00"
            || !(HEADER_SIZE as u32..=4096).contains(&word(&header[..4]))
            || &header[68..72] != b"rdi\x01"
            || &header[124..128] != b"REDT"
            || &footer[80..84] != b"rdi\x01"
            || !(FOOTER_SIZE as u32..=4096).contains(&word(&footer[..4]))
            || header[8..50] != footer[8..50]
            || header[48..50] != number.to_be_bytes()
        {
            return Err("unsupported or incomplete RED2 recording part".into());
        }
        let raster = (
            i64::from(word(&header[76..80])),
            i64::from(word(&header[80..84])),
        );
        let rate = match &header[84..92] {
            b"\x11\x02\x03\xe8\x00\x00\x5d\xc0" => Rational::integer(24),
            b"\x11\x02\x03\xe9\x00\x00\x5d\xc0" => Rational::new(24000, 1001).unwrap(),
            _ => return Err("unsupported RED2 source clock".into()),
        };
        let frames = i64::from(word(&footer[92..96]));
        let cumulative_frames = i64::from(word(&footer[100..104]));
        if raster != (5120, 2700) || frames == 0 || cumulative_frames < frames {
            return Err("unqualified RED2 picture raster or indexed extent".into());
        }
        // REDT starts with edge timecode (0x1000), followed by time of day
        // (0x1001). Resolve's saved clock and Premiere's default display
        // both use the latter. At 24000/1001 the display base is still 24.
        if &header[130..134] != b"\x00\x0f\x10\x00"
            || &header[145..149] != b"\x00\x0f\x10\x01"
            || header[160] != 0
        {
            return Err("missing RED2 time-of-day timecode".into());
        }
        let timecode =
            parse_timecode(&header[149..160]).ok_or("invalid RED2 time-of-day timecode")?;
        Ok(Self {
            path,
            canonical,
            stamp,
            raster,
            rate,
            timecode,
            frames,
            cumulative_frames,
            id: header[16..32].try_into().unwrap(),
            end: footer[4..8].try_into().unwrap(),
        })
    }
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().unwrap())
}

fn parse_timecode(bytes: &[u8]) -> Option<u64> {
    let &[h, hh, b':', m, mm, b':', s, ss, b':', f, ff] = bytes else {
        return None;
    };
    let pair = |a: u8, b: u8| {
        (a.is_ascii_digit() && b.is_ascii_digit())
            .then(|| u64::from(a - b'0') * 10 + u64::from(b - b'0'))
    };
    let (h, m, s, f) = (pair(h, hh)?, pair(m, mm)?, pair(s, ss)?, pair(f, ff)?);
    (h < 24 && m < 60 && s < 60 && f < 24).then_some(((h * 60 + m) * 60 + s) * 24 + f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_part(
        path: &Path,
        number: u16,
        identity: u8,
        footer_tag: &[u8; 4],
        frames: u32,
        cumulative_frames: u32,
    ) {
        let mut bytes = [0; 8192];
        let (header, footer) = bytes.split_at_mut(4096);
        header[..4].copy_from_slice(&1026u32.to_be_bytes());
        header[4..16].copy_from_slice(b"RED2\x05\x10R2\x05\x01\x0c\x00");
        header[16..32].fill(identity);
        header[48..50].copy_from_slice(&number.to_be_bytes());
        header[68..72].copy_from_slice(b"rdi\x01");
        header[76..80].copy_from_slice(&5120u32.to_be_bytes());
        header[80..84].copy_from_slice(&2700u32.to_be_bytes());
        header[84..92].copy_from_slice(b"\x11\x02\x03\xe9\x00\x00\x5d\xc0");
        header[124..128].copy_from_slice(b"REDT");
        header[130..161]
            .copy_from_slice(b"\x00\x0f\x10\x0001:00:00:00\x00\x0f\x10\x0112:00:00:06\x00");
        if number == 2 {
            header[149..160].copy_from_slice(b"12:00:16:22");
        }
        footer[..4].copy_from_slice(&1066u32.to_be_bytes());
        footer[4..8].copy_from_slice(footer_tag);
        footer[8..50].copy_from_slice(&header[8..50]);
        footer[80..84].copy_from_slice(b"rdi\x01");
        footer[92..96].copy_from_slice(&frames.to_be_bytes());
        footer[100..104].copy_from_slice(&cumulative_frames.to_be_bytes());
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn verifies_red_parts_and_detects_replacement_or_incomplete_span() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("Camera_001.R3D");
        let second = dir.path().join("Camera_002.R3D");
        write_part(&first, 1, 1, b"REOF", 400, 400);
        assert!(Recording::open(&first)
            .err()
            .unwrap()
            .to_string()
            .contains("_002.R3D"));
        write_part(&second, 2, 2, b"REOC", 300, 700);
        assert!(Recording::open(&first)
            .err()
            .unwrap()
            .to_string()
            .contains("clip identity"));
        write_part(&second, 2, 1, b"REOC", 300, 701);
        assert!(Recording::open(&first)
            .err()
            .unwrap()
            .to_string()
            .contains("discontinuous"));
        write_part(&second, 2, 1, b"REOC", 300, 700);
        let recording = Recording::open(&first).unwrap();
        assert_eq!(recording.frames, 700);
        assert_eq!(recording.rate, Rational::new(24000, 1001).unwrap());
        assert_eq!(recording.timecode, 12 * 60 * 60 * 24 + 6);
        fs::write(&second, b"replacement").unwrap();
        assert!(recording
            .recheck()
            .unwrap_err()
            .to_string()
            .contains("changed"));
    }

    #[test]
    fn rejects_invalid_timecode_fields() {
        for label in [
            b"24:00:00:00",
            b"01:60:00:00",
            b"01:01:60:00",
            b"01:01:01:24",
            b"01:ab:00:00",
        ] {
            assert!(parse_timecode(label).is_none());
        }
    }
}
