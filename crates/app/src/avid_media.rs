//! Media-only staging for future Avid destinations. The receipt is the handoff to
//! project preparation and native indexing.
use crate::{backend_bundle::BackendTools, rewrap};
use graph::Rational;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

mod avb;
mod linked;
mod master;
pub use linked::write_linked_bin;
#[cfg(target_os = "macos")]
pub use linked::write_linked_project;
pub use master::write_master_bin;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const TICKS: i64 = prproj::convert::model::TICKS_PER_SECOND;
const MEDIA_DIR: &str = "Avid MediaFiles";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PictureProfile {
    Lb,
    #[default]
    Sq,
    Hqx,
}

impl PictureProfile {
    pub const ALL: [Self; 3] = [Self::Lb, Self::Sq, Self::Hqx];
    pub fn label(self) -> &'static str {
        match self {
            Self::Lb => "DNxHR LB · 8-bit",
            Self::Sq => "DNxHR SQ · 8-bit",
            Self::Hqx => "DNxHR HQX · 10-bit",
        }
    }
    fn encoding(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Lb => ("dnxhr_lb", "yuv422p", "DNXHR LB"),
            Self::Sq => ("dnxhr_sq", "yuv422p", "DNXHR SQ"),
            Self::Hqx => ("dnxhr_hqx", "yuv422p10le", "DNXHR HQX"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioProfile {
    Pcm16,
    #[default]
    Pcm24,
}

impl AudioProfile {
    pub const ALL: [Self; 2] = [Self::Pcm24, Self::Pcm16];
    pub fn label(self) -> &'static str {
        match self {
            Self::Pcm16 => "PCM 16-bit · 48 kHz",
            Self::Pcm24 => "PCM 24-bit · 48 kHz",
        }
    }
    fn encoding(self) -> (&'static str, i64) {
        match self {
            Self::Pcm16 => ("pcm_s16le", 16),
            Self::Pcm24 => ("pcm_s24le", 24),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DrivePlan {
    pub mxf_folder: PathBuf,
    pub available_bytes: u64,
    pub databases: IndexPlan,
}

#[derive(Debug, Serialize)]
pub struct IndexPlan {
    pub media_objects: &'static str,
    pub file_ids: &'static str,
    pub action: &'static str,
}

impl IndexPlan {
    fn pending() -> Self {
        Self {
            media_objects: "msmMMOB.mdb",
            file_ids: "msmFMID.pmr",
            action: "refresh_in_media_composer",
        }
    }
}

/// Calculate an unused standalone drive folder without changing the volume.
pub fn plan_drive(root: &Path) -> Result<DrivePlan> {
    let root = fs::canonicalize(root)?;
    let meta = fs::metadata(&root)?;
    if !meta.is_dir() {
        return Err("select a media drive".into());
    }
    #[cfg(unix)]
    let is_volume = {
        use std::os::unix::fs::MetadataExt;
        root.parent()
            .map(|parent| fs::metadata(parent).map(|parent| parent.dev() != meta.dev()))
            .transpose()?
            .unwrap_or(true)
    };
    #[cfg(not(unix))]
    let is_volume = root.parent().is_none();
    if !is_volume {
        return Err("select the root of a mounted media drive".into());
    }
    if meta.permissions().readonly() {
        return Err("the selected media drive is read-only".into());
    }
    Ok(DrivePlan {
        mxf_folder: next_folder(&root)?,
        available_bytes: fs2::available_space(&root)?,
        databases: IndexPlan::pending(),
    })
}

fn next_folder(root: &Path) -> Result<PathBuf> {
    let media = root.join(MEDIA_DIR);
    let mxf = media.join("MXF");
    for path in [&media, &mxf] {
        match fs::symlink_metadata(path) {
            Ok(info) if !info.is_dir() || info.file_type().is_symlink() => {
                return Err(format!("unsafe Avid media directory {}", path.display()).into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(mxf.join("1")),
            Err(error) => return Err(error.into()),
        }
    }
    let mut highest = 0u32;
    for (index, entry) in fs::read_dir(&mxf)?.enumerate() {
        if index >= 10_000 {
            return Err("too many existing MXF folders to plan a fresh folder".into());
        }
        let name = entry?.file_name();
        if let Some(number) = name
            .to_str()
            .filter(|s| s.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
        {
            highest = highest.max(number);
        }
    }
    Ok(mxf.join(
        highest
            .checked_add(1)
            .ok_or("no unused numbered MXF folder")?
            .to_string(),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioChannel {
    pub stream: u32,
    pub channel: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub source: PathBuf,
    pub source_item_id: Option<String>,
    pub edit_rate: String,
    pub timecode: String,
    pub video_stream: Option<u32>,
    #[serde(default)]
    pub audio_channels: Vec<AudioChannel>,
    #[serde(default)]
    pub picture_profile: PictureProfile,
    #[serde(default)]
    pub audio_profile: AudioProfile,
}

#[derive(Debug, Serialize)]
pub struct Receipt {
    pub schema: &'static str,
    pub source: PathBuf,
    pub source_item_id: Option<String>,
    pub edit_rate: String,
    pub timecode: String,
    pub mxf_folder: PathBuf,
    pub databases: IndexPlan,
    pub files: Vec<MediaFile>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MediaFile {
    pub name: String,
    pub file_package_umid: String,
    pub material_package_umid: String,
    pub source_stream: u32,
    pub source_channel: Option<u32>,
    pub source_first_pts: i64,
    pub source_time_base: String,
    pub frames: Option<i64>,
    pub samples: Option<i64>,
    pub encoding: String,
}

#[derive(Deserialize)]
struct Metadata {
    streams: Vec<Stream>,
}

#[derive(Deserialize)]
struct Stream {
    index: u32,
    codec_type: String,
    codec_name: Option<String>,
    time_base: String,
    width: Option<i64>,
    height: Option<i64>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
}

#[derive(Clone, Copy)]
struct Extent {
    first_pts: i64,
    units: i64,
}

/// Create candidate OP-Atom files in a fresh, isolated staging tree. Publication
/// of its manifest follows independent header, stream and extent checks.
pub fn stage(req: &Request, destination: &Path, tools: &BackendTools) -> Result<Receipt> {
    let (rate, base, rate_arg) = match req.edit_rate.as_str() {
        "24000/1001" => (Rational::new(24000, 1001).unwrap(), 24, "24000/1001"),
        "24" | "24/1" => (Rational::integer(24), 24, "24/1"),
        "25" | "25/1" => (Rational::integer(25), 25, "25/1"),
        "30000/1001" => (Rational::new(30000, 1001).unwrap(), 30, "30000/1001"),
        _ => return Err("unsupported Avid staging edit rate".into()),
    };
    let clock = timecode(&req.timecode, base, rate_arg == "30000/1001")?;
    if req.video_stream.is_none() && req.audio_channels.is_empty() {
        return Err("select a picture stream or a mono channel".into());
    }
    if req.audio_channels.len() > 64
        || req.audio_channels.iter().collect::<HashSet<_>>().len() != req.audio_channels.len()
    {
        return Err("select up to 64 unique physical audio channels".into());
    }
    if req
        .source_item_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control))
    {
        return Err("invalid source item identity".into());
    }
    let source = fs::canonicalize(&req.source)?;
    if !source
        .extension()
        .and_then(|suffix| suffix.to_str())
        .is_some_and(|suffix| {
            [
                "mov", "mp4", "m4v", "mxf", "mkv", "avi", "wav", "aif", "aiff",
            ]
            .iter()
            .any(|allowed| suffix.eq_ignore_ascii_case(allowed))
        })
    {
        return Err("expected a local movie or audio file".into());
    }
    let stamp = rewrap::source::stamp(&source)?;
    let parent = fs::canonicalize(
        destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let root = parent.join(
        destination
            .file_name()
            .ok_or("give the staging folder a fresh name")?,
    );
    if root
        .components()
        .any(|part| part.as_os_str().eq_ignore_ascii_case(MEDIA_DIR))
    {
        return Err("staging must be outside Avid MediaFiles".into());
    }
    match fs::symlink_metadata(&root) {
        Ok(_) => return Err("the staging destination already exists".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let metadata: Metadata = serde_json::from_str(&rewrap::run_media_tool(
        Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-show_streams",
                "-of",
                "json",
            ])
            .arg(&source),
    )?)?;
    let stream = |index, kind| {
        metadata
            .streams
            .iter()
            .find(|stream| stream.index == index && stream.codec_type == kind)
            .ok_or("selected physical stream absent")
    };
    let video = req
        .video_stream
        .map(|index| -> Result<_> {
            let stream = stream(index, "video")?;
            let raster = (stream.width, stream.height);
            let is_yuv = match stream.pix_fmt.as_deref() {
                Some("yuv420p" | "yuv422p") => true,
                Some("yuv420p10le" | "yuv422p10le") => req.picture_profile == PictureProfile::Hqx,
                _ => false,
            };
            if !matches!(raster, (Some(1920), Some(1080)) | (Some(3840), Some(2160)))
                || !is_yuv
                || stream
                    .sample_aspect_ratio
                    .as_deref()
                    .is_some_and(|ratio| ratio != "1:1")
                || matches!(
                    stream.color_transfer.as_deref(),
                    Some("smpte2084" | "arib-std-b67")
                )
                || stream.color_primaries.as_deref() == Some("bt2020")
            {
                return Err(
                    "picture needs square-pixel HD or UHD SDR YUV and sufficient DNx bit depth"
                        .into(),
                );
            }
            Ok((
                stream,
                frame_extent(&tools.ffprobe, &source, stream, Some(rate))?,
            ))
        })
        .transpose()?;
    let mut audio = BTreeMap::new();
    for &AudioChannel {
        stream: index,
        channel,
    } in &req.audio_channels
    {
        let stream = stream(index, "audio")?;
        if stream
            .channels
            .is_none_or(|count| channel >= count || count > 64)
            || stream.sample_rate.as_deref() != Some("48000")
            || !matches!(
                stream.codec_name.as_deref(),
                Some("pcm_s16le" | "pcm_s16be" | "pcm_s24le" | "pcm_s24be")
            )
        {
            return Err(format!("stream {index} channel {channel}: expected a verified 48 kHz PCM16/PCM24 source channel").into());
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = audio.entry(index) {
            entry.insert((stream, frame_extent(&tools.ffprobe, &source, stream, None)?));
        }
    }
    if rewrap::source::stamp(&source)? != stamp {
        return Err("the original changed during inspection".into());
    }

    fs::create_dir(&root)?;
    let folder = root.join(MEDIA_DIR).join("MXF").join("1");
    fs::create_dir_all(&folder)?;
    let mut files = Vec::new();
    let encode = |stream: &Stream, name: &str, args: &[&str]| -> Result<rewrap::AtomEssence> {
        let path = folder.join(name);
        rewrap::run_media_tool(
            Command::new(&tools.ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-n",
                    "-xerror",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-i",
                ])
                .arg(&source)
                .arg("-map")
                .arg(format!("0:{}", stream.index))
                .args(["-map_metadata", "-1", "-map_chapters", "-1"])
                .args(args)
                .args(["-timecode", &req.timecode, "-f", "mxf_opatom"])
                .arg(&path),
        )?;
        let atom = rewrap::inspect_staged_atom(&tools.ffprobe, &path)?;
        if atom.timecode.as_deref() != Some(req.timecode.as_str())
            || atom.header_timecode != Some(clock)
        {
            return Err(
                format!("{name}: staged timecode disagrees with the requested clock").into(),
            );
        }
        Ok(atom)
    };
    if let Some((stream, extent)) = video {
        let name = "Picture.mxf";
        let (profile, pix, label) = req.picture_profile.encoding();
        let atom = encode(
            stream,
            name,
            &[
                "-an",
                "-c:v",
                "dnxhd",
                "-profile:v",
                profile,
                "-pix_fmt",
                pix,
                "-fps_mode",
                "passthrough",
            ],
        )?;
        let duration = Rational::new(
            i128::from(extent.units) * i128::from(rate.den()),
            i128::from(rate.num()),
        )
        .and_then(|seconds| seconds.to_units(TICKS));
        if atom.codec != "dnxhd"
            || atom.profile != label
            || atom
                .header_picture_profile
                .map(avid_reader::mxf::PictureProfile::probe_name)
                != Some(label)
            || atom.pix != pix
            || atom.packets != extent.units
            || Some(atom.ticks) != duration
            || (Some(atom.raster.0), Some(atom.raster.1)) != (stream.width, stream.height)
        {
            return Err(
                "staged DNx picture disagrees with the selected raster, cadence or profile".into(),
            );
        }
        files.push(media_file(name.into(), atom, stream, extent, None, label));
    }
    for (
        lane,
        &AudioChannel {
            stream: index,
            channel,
        },
    ) in req.audio_channels.iter().enumerate()
    {
        let (stream, extent) = audio[&index];
        let name = format!("Audio-{:02}.mxf", lane + 1);
        let (codec, bits) = req.audio_profile.encoding();
        let atom = encode(
            stream,
            &name,
            &[
                "-vn",
                "-af",
                &format!("pan=mono|c0=c{channel}"),
                "-ar",
                "48000",
                "-c:a",
                codec,
                "-mxf_audio_edit_rate",
                rate_arg,
            ],
        )?;
        let payload_size = rewrap::pcm_essence_bytes(&folder.join(&name))?;
        let expected_size = u64::try_from(extent.units)?.checked_mul(u64::try_from(bits / 8)?);
        if atom.codec != codec
            || atom.sample_rate != 48000
            || atom.channels != 1
            || atom.bits != bits
            || Some(atom.ticks) != extent.units.checked_mul(TICKS / 48000)
            || Some(payload_size) != expected_size
        {
            return Err(format!(
                "{name}: staged mono PCM disagrees with the source sample extent or profile"
            )
            .into());
        }
        files.push(media_file(name, atom, stream, extent, Some(channel), codec));
    }
    let mut package_ids = HashSet::new();
    if !files.iter().all(|file| {
        package_ids.insert(file.file_package_umid.as_str())
            && package_ids.insert(file.material_package_umid.as_str())
    }) {
        return Err("duplicate staged MXF package identity".into());
    }
    if rewrap::source::stamp(&source)? != stamp {
        return Err("the original changed during encoding".into());
    }
    let receipt = Receipt {
        schema: "anynle.avid-media-stage/1",
        source,
        source_item_id: req.source_item_id.clone(),
        edit_rate: rate.to_string(),
        timecode: req.timecode.clone(),
        mxf_folder: PathBuf::from(MEDIA_DIR).join("MXF").join("1"),
        databases: IndexPlan::pending(),
        files,
    };
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("anynle-avid-media.json"))?;
    serde_json::to_writer_pretty(&mut output, &receipt)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    Ok(receipt)
}

fn media_file(
    name: String,
    atom: rewrap::AtomEssence,
    stream: &Stream,
    extent: Extent,
    channel: Option<u32>,
    encoding: &str,
) -> MediaFile {
    MediaFile {
        name,
        file_package_umid: atom.file,
        material_package_umid: atom.material,
        source_stream: stream.index,
        source_channel: channel,
        source_first_pts: extent.first_pts,
        source_time_base: stream.time_base.clone(),
        frames: channel.is_none().then_some(extent.units),
        samples: channel.map(|_| extent.units),
        encoding: encoding.into(),
    }
}

fn frame_extent(
    tool: &Path,
    path: &Path,
    stream: &Stream,
    rate: Option<Rational>,
) -> Result<Extent> {
    let time_base = Rational::parse(&stream.time_base).ok_or("missing source time base")?;
    let unit_rate = rate.unwrap_or(Rational::integer(48000));
    let time_units = Rational::new(
        i128::from(time_base.num()) * i128::from(unit_rate.num()),
        i128::from(time_base.den()) * i128::from(unit_rate.den()),
    )
    .filter(|unit| unit.num() > 0)
    .ok_or("invalid source clock")?;
    let index = stream.index.to_string();
    let result = rewrap::read_media_tool(
        Command::new(tool)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-select_streams",
                &index,
                "-show_frames",
                "-show_entries",
                "frame=best_effort_timestamp,nb_samples,interlaced_frame",
                "-of",
                "compact=p=0",
            ])
            .arg(path),
        |data| {
            let mut reader = BufReader::new(data);
            let mut units = 0i64;
            let mut origin = None;
            loop {
                let mut line = String::new();
                if (&mut reader)
                    .take(16_385)
                    .read_line(&mut line)
                    .map_err(|err| err.to_string())?
                    == 0
                {
                    break;
                }
                if line.len() > 16_384 {
                    return Err("source frame report exceeds the line limit".into());
                }
                let get = |key: &str| {
                    line.trim()
                        .split('|')
                        .filter_map(|field| field.split_once('='))
                        .find_map(|(name, value)| {
                            (name == key).then(|| value.parse::<i64>().ok()).flatten()
                        })
                };
                let Some(pts) = get("best_effort_timestamp") else {
                    if line.trim().is_empty() {
                        continue;
                    }
                    return Err("source frame has no exact timestamp".into());
                };
                let first = *origin.get_or_insert(pts);
                if pts
                    .checked_sub(first)
                    .map(|delta| i128::from(delta) * i128::from(time_units.num()))
                    != Some(i128::from(units) * i128::from(time_units.den()))
                {
                    return Err("source frame or sample clock is discontinuous".into());
                }
                let count = if rate.is_some() {
                    if get("interlaced_frame") != Some(0) {
                        return Err("source picture needs a progressive frame clock".into());
                    }
                    1
                } else {
                    get("nb_samples")
                        .filter(|value| *value > 0)
                        .ok_or("audio frame lacks a sample count")?
                };
                units = units.checked_add(count).ok_or("source extent overflow")?;
            }
            Ok(Extent {
                first_pts: origin.ok_or("source stream decoded no frames")?,
                units,
            })
        },
    )?;
    Ok(result)
}

fn timecode(value: &str, base: u16, ntsc: bool) -> Result<(u64, u16, bool)> {
    let bytes = value.as_bytes();
    if bytes.len() != 11
        || bytes[2] != b':'
        || bytes[5] != b':'
        || !matches!(bytes[8], b':' | b';')
        || [0, 1, 3, 4, 6, 7, 9, 10]
            .iter()
            .any(|&index| !bytes[index].is_ascii_digit())
    {
        return Err("expected an exact HH:MM:SS:FF source timecode".into());
    }
    let count =
        |index: usize| u64::from(bytes[index] - b'0') * 10 + u64::from(bytes[index + 1] - b'0');
    let (hours, minutes, seconds, frames) = (count(0), count(3), count(6), count(9));
    let drop = bytes[8] == b';';
    if hours >= 24
        || minutes >= 60
        || seconds >= 60
        || frames >= u64::from(base)
        || drop && (!ntsc || minutes % 10 != 0 && seconds == 0 && frames < 2)
    {
        return Err("timecode label does not exist on the chosen edit clock".into());
    }
    let dropped = if drop {
        2 * (hours * 54 + minutes - minutes / 10)
    } else {
        0
    };
    Ok((
        ((hours * 60 + minutes) * 60 + seconds) * u64::from(base) + frames - dropped,
        base,
        drop,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prproj::{RandomUuids, UuidSource};

    #[test]
    fn standalone_numbering_skips_existing_media_and_preserves_indexes() {
        let root =
            std::env::temp_dir().join(format!("avid-folder-plan-{}", RandomUuids::new().uuid4()));
        fs::create_dir(&root).unwrap();
        let mxf = root.join(MEDIA_DIR).join("MXF");
        assert_eq!(next_folder(&root).unwrap(), mxf.join("1"));
        assert!(!mxf.exists());
        let index_dir = mxf.join("9");
        fs::create_dir_all(&index_dir).unwrap();
        let old_index = index_dir.join(IndexPlan::pending().media_objects);
        fs::write(&old_index, b"existing database").unwrap();
        fs::create_dir(mxf.join("Workstation.1")).unwrap();
        fs::write(mxf.join("10"), b"occupied").unwrap();
        assert_eq!(next_folder(&root).unwrap(), mxf.join("11"));
        assert_eq!(fs::read(&old_index).unwrap(), b"existing database");
        assert!(!mxf.join("11").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drop_frame_timecode_rejects_missing_labels_at_the_minute_boundary() {
        assert_eq!(timecode("00:01:00;02", 30, true).unwrap(), (1800, 30, true));
        assert_eq!(
            timecode("00:10:00;00", 30, true).unwrap(),
            (17_982, 30, true)
        );
        assert_eq!(
            timecode("00:01:00:00", 30, true).unwrap(),
            (1800, 30, false)
        );
        assert!(timecode("00:01:00;00", 30, true).is_err());
        assert!(timecode("00:01:00;02", 24, false).is_err());
        assert!(timecode("00:00:00:24", 24, false).is_err());
    }
}
