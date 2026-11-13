use crate::{prepare::utf8_path, timecode, Result, DAY};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{ChildStdout, Command, Stdio},
    time::UNIX_EPOCH,
};

const MAX_MEDIA_BYTES: u64 = 2 << 30;
const MAX_PROBE_BYTES: u64 = 128 << 10;

/// Binding from a prior essence verification. The digest ties the probe and
/// generated archive to that verified file. Stream order is part of the receipt.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingSpec {
    pub item_id: String,
    pub path: PathBuf,
    pub sha256: String,
    pub streams: [StreamSelector; 3],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamSelector {
    pub media_id: String,
    pub index: usize,
    pub channel: Option<usize>,
}

pub struct VerifiedBinding {
    pub(crate) item_id: String,
    pub(crate) path: PathBuf,
    pub(crate) streams: [StreamSelector; 3],
    pub(crate) frames: i64,
    pub(crate) timecode_frames: i64,
    pub(crate) source_package: [u8; 32],
    pub(crate) modified_micros: u64,
    pub(crate) tagged_rec709: bool,
    pub(crate) digest: [u8; 32],
    pub(crate) atom: Option<crate::atom::AtomMedia>,
}

impl VerifiedBinding {
    pub(crate) fn recheck(&self) -> Result<()> {
        if let Some(atom) = &self.atom {
            atom.recheck()?;
        } else if media_digest(&self.path)? != self.digest {
            return Err("verified OP1a bytes changed".into());
        }
        Ok(())
    }
}

pub(crate) fn media_digest(path: &Path) -> Result<[u8; 32]> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_MEDIA_BYTES {
        return Err("media must be a regular file within the 2 GiB research limit".into());
    }
    let mut read = File::open(path)?.take(MAX_MEDIA_BYTES + 1);
    let mut hash = Sha256::new();
    let count = io::copy(&mut read, &mut hash)?;
    if count != meta.len() {
        return Err("media changed while hashing".into());
    }
    Ok(hash.finalize().into())
}

/// Check the declared PCM data against bytes actually present. A demuxer can
/// estimate duration on a damaged WAV, so its duration alone is insufficient.
pub(crate) fn check_wav_extent(path: &Path, bits: i64, channels: i64, samples: i64) -> Result<()> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut header = [0; 12];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RIFF"
        || &header[8..] != b"WAVE"
        || u64::from(u32::from_le_bytes(header[4..8].try_into()?)) + 8 != len
    {
        return Err("WAV RIFF extent disagrees with file size".into());
    }
    let expected = u64::try_from(i128::from(samples) * i128::from(channels) * i128::from(bits / 8))?;
    let mut offset = 12;
    let mut found_data = false;
    for _ in 0..1024 {
        if offset == len && found_data {
            return Ok(());
        }
        if offset + 8 > len {
            break;
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk)?;
        let size = u64::from(u32::from_le_bytes(chunk[4..].try_into()?));
        offset += 8 + size + size % 2;
        if &chunk[..4] == b"data" {
            if found_data || size != expected {
                return Err("WAV PCM data extent disagrees with the verified sample count".into());
            }
            found_data = true;
        }
    }
    Err("WAV has truncated PCM data or an unsupported chunk layout".into())
}

#[cfg(test)]
mod wav_extent_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn verifies_padded_pcm24_chunk_and_refuses_a_short_file() {
        let mut wav = tempfile::NamedTempFile::new().unwrap();
        // Three mono PCM24 samples require one RIFF padding byte.
        wav.write_all(b"RIFF\x2e\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\xbb\0\0\x80\x32\x02\0\x03\0\x18\0data\x09\0\0\0").unwrap();
        wav.write_all(&[0; 10]).unwrap();
        assert!(check_wav_extent(wav.path(), 24, 1, 3).is_ok());
        assert!(check_wav_extent(wav.path(), 24, 1, 2).is_err());
        wav.as_file().set_len(52).unwrap();
        assert!(check_wav_extent(wav.path(), 24, 1, 3).is_err());
    }
}

fn ffprobe(tool: &Path, path: &Path) -> Result<Value> {
    probe_file(tool, path, "mxf")
}

pub(crate) fn probe_file(tool: &Path, path: &Path, format: &str) -> Result<Value> {
    probe_json(
        tool,
        path,
        &[
            "-f",
            format,
            "-count_packets",
            "-show_streams",
            "-show_format",
        ],
    )
}

pub(crate) fn probe_json(tool: &Path, path: &Path, args: &[&str]) -> Result<Value> {
    let mut child = Command::new(tool)
        .args(["-v", "error", "-protocol_whitelist", "file,pipe"])
        .args(args)
        .args(["-of", "json"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut output = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or("missing ffprobe output")?
        .take(MAX_PROBE_BYTES + 1)
        .read_to_end(&mut output);
    if read.is_err() || output.len() as u64 > MAX_PROBE_BYTES {
        let _ = child.kill();
    }
    let status = child.wait()?;
    read?;
    if !status.success() || output.len() as u64 > MAX_PROBE_BYTES {
        return Err("bounded ffprobe inspection failed".into());
    }
    Ok(serde_json::from_slice(&output)?)
}

pub(crate) fn verify_mov_packet_grid(
    tool: &Path,
    path: &Path,
    frame_count: i64,
    ticks_per_frame: i64,
    reordered: bool,
) -> Result<()> {
    if ticks_per_frame <= 0 || frame_count <= 0 {
        return Err("invalid MOV picture packet grid".into());
    }
    if reordered && frame_count > 1_000_000 {
        return Err("AVC packet grid exceeds its inspection limit".into());
    }
    inspect_movie_packets(
        tool,
        path,
        "v:0",
        "packet=pts,dts,duration",
        "csv=p=0",
        |output| {
            let mut row = String::new();
            let mut seen = if reordered {
                vec![false; frame_count as usize]
            } else {
                vec![]
            };
            let mut previous_dts: Option<i64> = None;
            for index in 0..frame_count {
                let pts = index
                    .checked_mul(ticks_per_frame)
                    .ok_or("MOV packet clock overflow")?;
                row.clear();
                (&mut *output).take(96).read_line(&mut row)?;
                let mut fields = row.trim_end().split(',').map(str::parse::<i64>);
                let valid = if !row.ends_with('\n') {
                    false
                } else if reordered {
                    match (fields.next(), fields.next(), fields.next(), fields.next()) {
                        (Some(Ok(p)), Some(Ok(d)), Some(Ok(n)), None)
                            if p >= 0
                                && p % ticks_per_frame == 0
                                && n == ticks_per_frame
                                && previous_dts.is_none_or(|old| old.checked_add(n) == Some(d)) =>
                        {
                            previous_dts = Some(d);
                            usize::try_from(p / ticks_per_frame)
                                .ok()
                                .and_then(|i| seen.get_mut(i))
                                .is_some_and(|slot| !std::mem::replace(slot, true))
                        }
                        _ => false,
                    }
                } else {
                    fields.eq([Ok(pts), Ok(pts), Ok(ticks_per_frame)])
                };
                if !valid {
                    return Err(format!(
                        "non-constant or incomplete MOV packet clock at frame {index}"
                    )
                    .into());
                }
            }
            if !output.fill_buf()?.is_empty() {
                return Err("MOV packet count exceeds declared picture duration".into());
            }
            Ok(())
        },
    )
}

pub(crate) fn verify_aac_packet_grid(tool: &Path, path: &Path, samples: i64) -> Result<()> {
    if !(1..=2_000_000_000).contains(&samples) {
        return Err("invalid or over-budget AAC sample extent".into());
    }
    inspect_movie_packets(
        tool,
        path,
        "a:0",
        "packet=pts,dts,duration:packet_side_data=skip_samples,discard_padding",
        "compact=p=0:nk=1",
        |output| {
            let mut row = String::new();
            (&mut *output).take(96).read_line(&mut row)?;
            if row.trim_end() != "-1024|-1024|1024|1024|0" {
                return Err("expected one AAC priming packet with 1024 skipped samples".into());
            }
            for sample in (0..samples).step_by(1024) {
                row.clear();
                (&mut *output).take(96).read_line(&mut row)?;
                if !row.ends_with('\n')
                    || !row.trim_end().split('|').map(str::parse::<i64>).eq([
                        Ok(sample),
                        Ok(sample),
                        Ok((samples - sample).min(1024)),
                    ])
                {
                    return Err(
                        "AAC packets disagree with the declared playable sample interval".into(),
                    );
                }
            }
            if !output.fill_buf()?.is_empty() {
                return Err(
                    "AAC packet count exceeds its declared playable sample interval".into(),
                );
            }
            Ok(())
        },
    )
}

fn inspect_movie_packets(
    tool: &Path,
    path: &Path,
    stream: &str,
    entries: &str,
    format: &str,
    inspect: impl FnOnce(&mut BufReader<ChildStdout>) -> Result<()>,
) -> Result<()> {
    let mut child = Command::new(tool)
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mov",
            "-select_streams",
            stream,
            "-show_entries",
            entries,
            "-of",
            format,
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = inspect(&mut BufReader::new(
        child.stdout.take().ok_or("missing MOV packet inspection")?,
    ));
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    if !status.success() {
        return Err("MOV packet inspection failed".into());
    }
    Ok(())
}

pub(crate) fn tc_frames(text: &str) -> Result<i64> {
    let parts = text
        .split(':')
        .map(str::parse::<i64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let [h, m, s, f] = parts.as_slice() else {
        return Err("missing exact media timecode".into());
    };
    if !(0..24).contains(h) || !(0..60).contains(m) || !(0..60).contains(s) || !(0..24).contains(f)
    {
        return Err("media timecode outside non-drop 24-base profile".into());
    }
    let frames = h * 86400 + m * 1440 + s * 24 + f;
    if frames >= DAY || timecode(frames) != text {
        return Err("noncanonical media timecode".into());
    }
    Ok(frames)
}

pub fn verify_binding(spec: BindingSpec, tool: &Path) -> Result<VerifiedBinding> {
    verify(spec, tool, false)
}

pub fn verify_group_binding(spec: BindingSpec, tool: &Path) -> Result<VerifiedBinding> {
    verify(spec, tool, true)
}

fn verify(spec: BindingSpec, tool: &Path, permit_untagged: bool) -> Result<VerifiedBinding> {
    let path = fs::canonicalize(&spec.path)?;
    utf8_path(&path)?;
    if !path.is_absolute() || path.extension().and_then(|s| s.to_str()) != Some("mxf") {
        return Err("expected an absolute verified MXF file".into());
    }
    let digest: [u8; 32] = hex::decode(&spec.sha256)?
        .try_into()
        .map_err(|_| "bad verified digest")?;
    if media_digest(&path)? != digest {
        return Err("OP1a file does not match its verified digest".into());
    }
    if spec.streams.iter().enumerate().any(|(index, stream)| {
        stream.media_id.is_empty()
            || stream.index != index
            || stream.channel != (index > 0).then_some(0)
    }) {
        return Err("expected physical selectors V0 and mono A1/A2 channel 0".into());
    }
    let probe = ffprobe(tool, &path)?;
    let tags = &probe["format"]["tags"];
    let op = tags["operational_pattern_ul"]
        .as_str()
        .unwrap_or_default()
        .replace('.', "");
    if !op.starts_with("060e2b34040101") || op.get(16..28) != Some("0d0102010101") {
        return Err("expected OP1a operational pattern".into());
    }
    let timecode_frames = tc_frames(tags["timecode"].as_str().ok_or("missing OP1a timecode")?)?;
    let streams = probe["streams"]
        .as_array()
        .ok_or("missing MXF stream inspection")?;
    let [v, a1, a2] = streams.as_slice() else {
        return Err("expected exactly three essence streams".into());
    };
    let frames = v["duration_ts"]
        .as_i64()
        .filter(|n| (1..=i64::from(i32::MAX)).contains(n))
        .ok_or("missing bounded video frame count")?;
    let packet_count = v["nb_read_packets"]
        .as_str()
        .and_then(|n| n.parse::<i64>().ok());
    let tagged_rec709 = v["color_transfer"] == "bt709" && v["color_primaries"] == "bt709";
    let untagged_control = permit_untagged
        && timecode_frames == 0
        && ["color_transfer", "color_primaries"]
            .iter()
            .all(|key| v[*key].is_null() || v[*key] == "unknown");
    if v["index"] != 0
        || v["codec_type"] != "video"
        || v["codec_name"] != "dnxhd"
        || v["profile"] != "DNXHR LB"
        || v["width"] != 1920
        || v["height"] != 1080
        || v["coded_width"] != 1920
        || v["coded_height"] != 1088
        || v["sample_aspect_ratio"] != "1:1"
        || v["field_order"] != "progressive"
        || v["pix_fmt"] != "yuv422p"
        || v["r_frame_rate"] != "24000/1001"
        || v["time_base"] != "1001/24000"
        || v["start_pts"] != 0
        || packet_count != Some(frames)
        || v["color_range"] != "tv"
        || v["color_space"] != "bt709"
        || !(tagged_rec709 || untagged_control)
    {
        return Err("unsupported DNxHR LB video raster, clock, packets or Rec.709 metadata".into());
    }
    let package = v["tags"]["file_package_umid"]
        .as_str()
        .and_then(|s| s.strip_prefix("0x"))
        .ok_or("missing file-package UMID")?;
    let source_package: [u8; 32] = hex::decode(package)?
        .try_into()
        .map_err(|_| "invalid file-package UMID")?;
    for (index, audio) in [a1, a2].into_iter().enumerate() {
        if audio["index"] != index + 1
            || audio["codec_type"] != "audio"
            || audio["codec_name"] != "pcm_s24le"
            || audio["sample_rate"] != "48000"
            || audio["time_base"] != "1/48000"
            || audio["start_pts"] != 0
            || audio["duration_ts"] != frames * 2002
            || audio["channels"] != 1
            || audio["bits_per_sample"] != 24
            || audio["tags"]["file_package_umid"] != v["tags"]["file_package_umid"]
        {
            return Err("unsupported or mismatched mono PCM24 stream".into());
        }
    }
    let modified_micros = fs::metadata(&path)?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_micros()
        .try_into()?;
    let verified = VerifiedBinding {
        item_id: spec.item_id,
        path,
        streams: spec.streams,
        frames,
        timecode_frames,
        source_package,
        modified_micros,
        tagged_rec709,
        digest,
        atom: None,
    };
    verified.recheck()?;
    Ok(verified)
}
