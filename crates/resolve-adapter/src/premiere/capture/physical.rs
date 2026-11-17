//! File checks for Premiere source capture, independent of Resolve's writer profiles.
use crate::{linked::extent, media, Result};
use graph::{Rational, StreamKind};
use serde_json::Value;
use std::{fs, path::Path};

pub(super) fn file_state(path: &Path) -> Result<media::FileStamp> {
    media::stamp(fs::metadata(path)?)
}

pub(super) struct Physical {
    pub container: &'static str,
    pub video: Option<Stream>,
    pub audio: Vec<Stream>,
    pub timecode: Option<String>,
    pub package: Option<String>,
    pub file_package: Option<String>,
    pub state: media::FileStamp,
}

pub(super) struct Stream {
    pub index: i64,
    pub codec: String,
    pub time_base: Rational,
    pub count: i64,
    pub duration: Rational,
    pub size: Option<(i64, i64)>,
    pub rate: Option<Rational>,
    pub channels: Option<i64>,
    pub bits: Option<i64>,
}

fn s<'a>(v: &'a Value, field: &str) -> &'a str {
    v[field].as_str().unwrap_or_default()
}

fn n(v: &Value, field: &str) -> Option<i64> {
    v[field]
        .as_i64()
        .or_else(|| v[field].as_str()?.parse().ok())
}

fn ratio(v: &Value, field: &str) -> Result<Rational> {
    let (num, den) = s(v, field)
        .split_once('/')
        .ok_or("missing physical clock")?;
    Rational::new(num.parse()?, den.parse()?)
        .filter(|r| r.num() > 0)
        .ok_or_else(|| "invalid physical clock".into())
}

pub(super) fn display_base(rate: Rational) -> Result<u16> {
    match (rate.num(), rate.den()) {
        (24, 1) | (24000, 1001) => Ok(24),
        (25, 1) => Ok(25),
        (30000, 1001) => Ok(30),
        _ => Err("Premiere linked media capture requires 23.976, 24, 25 or 29.97 fps".into()),
    }
}

pub(super) fn inspect(path: &Path, ffprobe: &Path) -> Result<Physical> {
    let state = file_state(path)?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let container = match extension.as_str() {
        "mov" | "mp4" => "mov",
        "mxf" => "mxf",
        "wav" => "wav",
        _ => return Err("unsupported Premiere linked file extension".into()),
    };
    let probe = media::probe_file(ffprobe, path, container)?;
    let format = &probe["format"];
    let tags = &format["tags"];
    let package = match (container, s(format, "format_name"), s(tags, "major_brand")) {
        ("mov", "mov,mp4,m4a,3gp,3g2,mj2", "qt  ") if extension == "mov" => None,
        ("mov", "mov,mp4,m4a,3gp,3g2,mj2", "isom" | "mp42" | "mp41") if extension == "mp4" => None,
        ("wav", "wav", _) => None,
        ("mxf", "mxf", _)
            if s(tags, "operational_pattern_ul") == "060e2b34.04010101.0d010201.01010900" =>
        {
            let id = s(tags, "material_package_umid");
            if !umid(id) {
                return Err("OP1a material package identity is missing".into());
            }
            Some(id[2..].to_ascii_uppercase())
        }
        _ => return Err("linked file container does not match the Premiere locator".into()),
    };
    let streams = probe["streams"]
        .as_array()
        .ok_or("missing physical stream table")?;
    if !(1..=4).contains(&streams.len()) {
        return Err("unqualified Premiere original stream count".into());
    }
    let mut video = None;
    let mut audio = Vec::new();
    let mut timecode = None;
    let mut file_package = None;
    for (index, row) in streams.iter().enumerate() {
        if n(row, "index") != Some(index as i64) {
            return Err("ambiguous physical stream order".into());
        }
        if package.is_some() {
            let id = s(&row["tags"], "file_package_umid");
            if !umid(id) || file_package.replace(id).is_some_and(|old| old != id) {
                return Err("inconsistent OP1a file package identity".into());
            }
        }
        match s(row, "codec_type") {
            "video" if index == 0 && container != "wav" => {
                video = Some(inspect_video(row, &extension, path, ffprobe)?);
            }
            "audio" if timecode.is_none() && audio.len() < 2 => {
                audio.push(inspect_audio(row, &extension, path, ffprobe)?);
            }
            "data"
                if extension == "mov"
                    && s(row, "codec_tag_string") == "tmcd"
                    && timecode.is_none() =>
            {
                let picture = &streams[0];
                let label = s(&row["tags"], "timecode");
                if s(&picture["tags"], "timecode") != label
                    || label.is_empty()
                    || n(row, "start_pts") != Some(0)
                    || n(row, "nb_read_packets") != Some(1)
                    || ratio(row, "time_base")? != ratio(picture, "time_base")?
                    || n(row, "duration_ts") != n(picture, "duration_ts")
                {
                    return Err("QuickTime timecode track contradicts the picture clock".into());
                }
                timecode = Some(label.into());
            }
            _ => return Err("unqualified additional stream in Premiere original".into()),
        }
    }
    if container == "wav" && audio.len() != 1
        || container != "wav" && video.is_none()
        || audio.len() == 2
            && audio
                .iter()
                .any(|a| a.channels != Some(1) || a.codec == "aac")
    {
        return Err("unqualified Premiere original stream layout".into());
    }
    if container == "mxf" {
        let tc = s(tags, "timecode");
        if tc != "00:00:00:00" {
            return Err("unqualified OP1a source timecode".into());
        }
        timecode = Some(tc.into());
    } else if timecode.is_none() && !s(&streams[0]["tags"], "timecode").is_empty() {
        return Err("picture reports a timecode without a verified track".into());
    }
    if file_state(path)? != state {
        return Err("Premiere linked media changed while probing".into());
    }
    Ok(Physical {
        container,
        video,
        audio,
        timecode,
        package,
        file_package: file_package.map(|id| id[2..].to_ascii_uppercase()),
        state,
    })
}

fn umid(s: &str) -> bool {
    s.strip_prefix("0x")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn span(row: &Value, implicit_zero: bool) -> Result<(Rational, i64, Rational)> {
    let time_base = ratio(row, "time_base")?;
    let count = n(row, "duration_ts")
        .filter(|&n| n > 0)
        .ok_or("missing playable stream extent")?;
    if n(row, "start_pts") != Some(0) && !(implicit_zero && row.get("start_pts").is_none()) {
        return Err("unsupported nonzero physical packet origin".into());
    }
    Ok((time_base, count, extent(count, time_base)?))
}

fn inspect_video(row: &Value, extension: &str, path: &Path, ffprobe: &Path) -> Result<Stream> {
    let size = n(row, "width")
        .zip(n(row, "height"))
        .ok_or("missing physical picture raster")?;
    let codec = s(row, "codec_name");
    let tag = s(row, "codec_tag_string");
    let profile = s(row, "profile");
    let pix = s(row, "pix_fmt");
    let supported = match (extension, codec, tag, profile, pix) {
        ("mp4" | "mov", "h264", "avc1", "High", "yuv420p") => match size {
            (1280, 720) | (1920, 1080) | (2048, 858) => {
                n(row, "level").is_some_and(|v| (1..=42).contains(&v))
            }
            (3840, 2160) => n(row, "level").is_some_and(|v| (50..=52).contains(&v)),
            _ => false,
        },
        ("mp4", "hevc", "hvc1", "Main 10", "yuv420p10le")
        | ("mov", "prores", "apch", "HQ", "yuv422p10le")
        | ("mxf", "dnxhd", _, "DNXHR LB", "yuv422p") => size == (1920, 1080),
        _ => false,
    };
    if !supported
        || s(row, "field_order") != "progressive"
        || !matches!(s(row, "sample_aspect_ratio"), "1:1" | "N/A" | "")
        || !["color_space", "color_transfer", "color_primaries"]
            .iter()
            .all(|f| matches!(s(row, f), "" | "unknown" | "bt709"))
        || !matches!(s(row, "color_range"), "tv" | "unknown" | "")
    {
        return Err("unqualified Premiere original picture profile".into());
    }
    let rate = ratio(row, "avg_frame_rate")?;
    display_base(rate)?;
    if ratio(row, "r_frame_rate")? != rate {
        return Err("physical picture rates disagree".into());
    }
    let (time_base, count, duration) = span(row, extension == "wav")?;
    let frames = crate::prepare::frames_on(duration, rate)?;
    if frames <= 0 || n(row, "nb_read_packets") != Some(frames) {
        return Err("physical picture packet count contradicts its frame extent".into());
    }
    let ticks_per_frame = count / frames;
    if ticks_per_frame <= 0 || count % frames != 0 {
        return Err("physical picture does not have a whole packet grid".into());
    }
    media::verify_picture_packet_grid(
        ffprobe,
        path,
        if extension == "mxf" { "mxf" } else { "mov" },
        frames,
        ticks_per_frame,
        matches!(codec, "h264" | "hevc"),
    )?;
    Ok(Stream {
        index: 0,
        codec: codec.into(),
        time_base,
        count,
        duration,
        size: Some(size),
        rate: Some(rate),
        channels: None,
        bits: None,
    })
}

fn inspect_audio(row: &Value, extension: &str, path: &Path, ffprobe: &Path) -> Result<Stream> {
    let index = n(row, "index").ok_or("missing physical audio stream index")?;
    let (time_base, count, duration) = span(row, extension == "wav")?;
    let channels = n(row, "channels")
        .filter(|&n| (1..=2).contains(&n))
        .ok_or("unsupported physical audio layout")?;
    let stereo = match s(row, "channel_layout") {
        "stereo" => true,
        "" | "unknown" => matches!(extension, "mxf" | "wav"),
        _ => false,
    };
    if n(row, "sample_rate") != Some(48000)
        || time_base != Rational::new(1, 48000).unwrap()
        || channels == 2 && !stereo
    {
        return Err("unqualified Premiere original audio clock or layout".into());
    }
    let codec = s(row, "codec_name");
    let tag = s(row, "codec_tag_string");
    let bits = match (extension, codec, tag) {
        ("mp4", "aac", "mp4a") | ("mov", "aac", "mp4a")
            if s(row, "profile") == "LC" && channels == 2 && index == 1 =>
        {
            media::verify_aac_packet_grid(ffprobe, path, count)?;
            None
        }
        ("mp4", "pcm_s16le", "ipcm")
        | ("mov", "pcm_s16le", "sowt")
        | ("mxf" | "wav", "pcm_s16le", _) => Some(16),
        ("mp4", "pcm_s24le", "ipcm")
        | ("mov", "pcm_s24le" | "pcm_s24be", "in24")
        | ("mxf" | "wav", "pcm_s24le", _) => Some(24),
        _ => return Err("unqualified Premiere original sample representation".into()),
    };
    if let Some(bits) = bits {
        if n(row, "bits_per_sample") != Some(bits)
            || n(row, "nb_read_packets").is_none_or(|n| n <= 0)
        {
            return Err("physical PCM bit depth or extent is absent".into());
        }
        if extension == "wav" {
            media::check_wav_extent(path, bits, channels, count)?;
        } else {
            media::verify_pcm_packet_grid(
                ffprobe,
                path,
                if extension == "mxf" { "mxf" } else { "mov" },
                index,
                count,
                bits / 8 * channels,
            )?;
        }
    }
    Ok(Stream {
        index,
        codec: codec.into(),
        time_base,
        count,
        duration,
        size: None,
        rate: None,
        channels: Some(channels),
        bits,
    })
}

impl Stream {
    pub(super) fn kind(&self) -> StreamKind {
        if self.channels.is_some() {
            StreamKind::Audio
        } else {
            StreamKind::Video
        }
    }
}
