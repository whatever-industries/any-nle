//! Check the saved stream, clock, and range against the online original.
use super::{Result, Route, RASTER, RATE};
use crate::{
    avid_media::{frame_extent, Stream},
    backend_bundle::BackendTools,
    rewrap,
};
use avid_reader::extract::format_timecode;
use graph::Rational;
use serde::Deserialize;
use std::{fs, path::Path, process::Command};

#[derive(Deserialize)]
struct PhysicalMetadata {
    streams: Vec<PhysicalStream>,
    format: Option<WavMetadata>,
}

#[derive(Deserialize)]
struct WavMetadata {
    tags: Option<WavTags>,
}

#[derive(Deserialize)]
struct WavTags {
    time_reference: Option<String>,
    timecode: Option<String>,
}

#[derive(Deserialize)]
struct PhysicalStream {
    #[serde(flatten)]
    stream: Stream,
    codec_tag_string: String,
    profile: Option<String>,
    avg_frame_rate: Option<String>,
    nb_frames: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    duration_ts: i64,
    start_pts: Option<i64>,
    bits_per_sample: Option<i64>,
    tags: Option<TimecodeTag>,
}

#[derive(Deserialize)]
struct TimecodeTag {
    timecode: Option<String>,
}

pub(super) fn original(
    route: &Route<'_>,
    picture: bool,
    generate: bool,
    tools: &BackendTools,
) -> Result<()> {
    let path = Path::new(route.path);
    if fs::canonicalize(path)? != path {
        return Err(format!(
            "{}: original locator must be an exact canonical path",
            path.display()
        )
        .into());
    }
    let format = if picture { "mov" } else { "wav" };
    let data = rewrap::run_media_tool(Command::new(&tools.ffprobe)
        .args(["-v", "error", "-protocol_whitelist", "file,pipe", "-f", format,
            "-show_entries",
            "stream=index,codec_type,codec_name,codec_tag_string,profile,avg_frame_rate,nb_frames,time_base,start_pts,duration_ts,width,height,pix_fmt,sample_aspect_ratio,channels,sample_rate,bits_per_sample,color_range,color_space,color_transfer,color_primaries:stream_tags=timecode:format_tags=time_reference,timecode",
            "-of", "json"]).arg(path))?;
    let metadata: PhysicalMetadata = serde_json::from_str(&data)?;
    let stream = match metadata.streams.as_slice() {
        [video, tc] if picture => {
            let label = format_timecode(i64::from(route.clock), RATE);
            let has_label = |s: &PhysicalStream| {
                s.tags.as_ref().and_then(|tags| tags.timecode.as_deref()) == Some(label.as_str())
            };
            if video.stream.codec_type != "video"
                || video.stream.codec_name.as_deref() != Some("prores")
                || video.codec_tag_string != "apch"
                || video.profile.as_deref() != Some("HQ")
                || video.avg_frame_rate.as_deref() != Some("24/1")
                || video.start_pts != Some(0)
                || video.stream.width.zip(video.stream.height) != Some(RASTER)
                || video.stream.pix_fmt.as_deref() != Some("yuv422p10le")
                || !matches!(
                    video.stream.sample_aspect_ratio.as_deref(),
                    None | Some("1:1")
                )
                || tc.stream.index != 1
                || tc.stream.codec_type != "data"
                || tc.codec_tag_string != "tmcd"
                || tc.nb_frames.as_deref() != Some("1")
                || tc.start_pts != Some(0)
                || tc.stream.time_base != video.stream.time_base
                || tc.duration_ts != video.duration_ts
                || !has_label(tc)
                || !has_label(video)
            {
                return Err(format!(
                    "{}: ProRes picture or embedded timecode disagrees",
                    path.display()
                )
                .into());
            }
            if generate
                && (video.color_range.as_deref() != Some("tv")
                    || video.color_space.as_deref() != Some("bt709")
                    || video
                        .stream
                        .color_transfer
                        .as_deref()
                        .is_some_and(|v| v != "bt709")
                    || video
                        .stream
                        .color_primaries
                        .as_deref()
                        .is_some_and(|v| v != "bt709"))
            {
                return Err(format!(
                    "{}: generating Avid links requires Rec.709 video levels",
                    path.display()
                )
                .into());
            }
            video
        }
        [audio]
            if !picture
                && audio.stream.codec_type == "audio"
                && matches!(
                    (audio.stream.codec_name.as_deref(), audio.bits_per_sample),
                    (Some("pcm_s16le"), Some(16)) | (Some("pcm_s24le"), Some(24))
                )
                && audio.bits_per_sample == route.bits
                && audio.stream.channels == Some(2)
                && audio.stream.sample_rate.as_deref() == Some("48000") =>
        {
            if metadata
                .format
                .as_ref()
                .and_then(|format| format.tags.as_ref())
                .is_some_and(|tags| {
                    tags.time_reference
                        .as_deref()
                        .is_some_and(|start| start != "0")
                        || tags
                            .timecode
                            .as_deref()
                            .is_some_and(|start| start != "00:00:00:00")
                })
            {
                return Err(format!(
                    "{}: waveform has an unqualified embedded start",
                    path.display()
                )
                .into());
            }
            audio
        }
        _ => return Err(format!("{}: original stream layout disagrees", path.display()).into()),
    };
    let observed = frame_extent(
        &tools.ffprobe,
        path,
        &stream.stream,
        picture.then_some(Rational::integer(RATE)),
    )?;
    let expected_units = i64::from(route.extent) * if picture { 1 } else { 2000 };
    if stream.stream.index != 0
        || stream.start_pts.unwrap_or(0) != 0
        || Rational::parse(&stream.stream.time_base) != Some(route.stream.time_base)
        || stream.duration_ts != route.stream.duration_ts
        || observed.first_pts != 0
        || observed.units != expected_units
    {
        return Err(format!(
            "{}: original clock or extent disagrees with graph/2",
            path.display()
        )
        .into());
    }
    Ok(())
}
