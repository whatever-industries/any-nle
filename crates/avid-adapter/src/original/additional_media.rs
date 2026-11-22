//! File-relative presentation of MP3 audio and ordinary linked stills.
use super::*;
use avid_reader::extract::{Essence, KIND_PICTURE, KIND_SOUND};
use std::io::{Read, Seek, SeekFrom};

const MPEG_FRAME_SAMPLES: i64 = 1152;

pub struct Mp3Extent {
    pub channels: i64,
    pub sample_rate: i64,
    pub samples: i64,
    pub coded_samples: i64,
    pub start_samples: i64,
    avid_delay_samples: Option<i64>,
}

/// Physical presentation only. The Avid-specific Info-frame convention stays
/// private and does not change the file-relative sample origin.
pub fn inspect_mp3_presentation(path: &Path, tool: &Path) -> Result<Mp3Extent, String> {
    probe_mp3(path, tool, &probe_metadata(path, tool)?)
}

// Avid counts the leading Info frame of this MPEG-1 Layer III stereo profile.
// FFprobe skips that frame and reports a separate gapless decoder delay.
fn leading_info_frame(
    file: &mut (impl Read + Seek),
    audio_packet_position: i64,
    coded_samples: i64,
    sample_rate: i64,
) -> Option<()> {
    // MPEG-1 Layer III, unpadded 192 kb/s, stereo without CRC.
    let (frame_bytes, header) = match sample_rate {
        44_100 => (626, [0xff, 0xfb, 0xb0, 0x00]),
        48_000 => (576, [0xff, 0xfb, 0xb4, 0x00]),
        _ => return None,
    };
    let mut tag = [0; 10];
    file.read_exact(&mut tag).ok()?;
    let frame_position = if &tag[..3] == b"ID3" {
        if tag[3..6] != [4, 0, 0] || tag[6..].iter().any(|&b| b >= 128) {
            return None;
        }
        10 + tag[6..].iter().fold(0, |size, &b| size * 128 + i64::from(b))
    } else {
        0
    };
    if frame_position + frame_bytes != audio_packet_position {
        return None;
    }
    file.seek(SeekFrom::Start(frame_position as u64)).ok()?;
    let mut frame = [0; 48];
    file.read_exact(&mut frame).ok()?;
    if frame[..4] != header || &frame[36..40] != b"Info" {
        return None;
    }
    let flags = u32::from_be_bytes(frame[40..44].try_into().ok()?);
    let count = i64::from(u32::from_be_bytes(frame[44..48].try_into().ok()?));
    (flags & 1 != 0 && count * MPEG_FRAME_SAMPLES == coded_samples).then_some(())
}

fn avid_mp3_delay(
    path: &Path,
    metadata: &Value,
    coded_samples: i64,
    head_samples: i64,
    sample_rate: i64,
) -> Option<i64> {
    let Value::Arr(packets) = metadata.get("packets")? else { return None };
    let packet = packets.first()?;
    let Value::Arr(side_data) = packet.get("side_data_list")? else { return None };
    if head_samples != 1105
        || number(packet, "stream_index") != Some(0)
        || number(packet, "pts") != Some(0)
        || !side_data.iter().any(|s| text(s, "side_data_type").as_deref() == Some("Skip Samples")
            && number(s, "skip_samples") == Some(head_samples))
    {
        return None;
    }
    leading_info_frame(
        &mut std::fs::File::open(path).ok()?,
        number(packet, "pos")?,
        coded_samples,
        sample_rate,
    )?;
    Some(MPEG_FRAME_SAMPLES + head_samples)
}

fn image_kind(path: &Path) -> Option<&'static str> {
    let suffix = path.extension()?.to_str()?;
    if suffix.eq_ignore_ascii_case("jpg") || suffix.eq_ignore_ascii_case("jpeg") {
        Some("jpeg")
    } else if suffix.eq_ignore_ascii_case("png") {
        Some("png")
    } else if suffix.eq_ignore_ascii_case("tif") || suffix.eq_ignore_ascii_case("tiff") {
        Some("tiff")
    } else {
        None
    }
}

pub(super) fn single_jpeg_input(command: &mut Command, path: &Path) {
    if image_kind(path) == Some("jpeg") {
        command.args(["-f", "image2", "-pattern_type", "none"]);
    }
}

pub(super) fn probe_still(path: &Path, tool: &Path) -> Result<Option<(i64, i64)>, String> {
    let Some(kind) = image_kind(path) else { return Ok(None) };
    if kind == "tiff" {
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if check_tiff_directory(&mut file).is_none() {
            return Err("linked TIFF requires one top-left RGB image without a color profile".into());
        }
    }
    let mut command = Command::new(tool);
    single_jpeg_input(&mut command, path);
    // FFmpeg reports TIFF Orientation as an unsupported tag in strict mode,
    // even for the top-left value already checked in the image directory.
    if kind != "tiff" {
        command.args(["-err_detect", "explode"]);
    }
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-show_frames",
            "-show_entries",
            "frame=stream_index,width,height:frame_tags=Orientation,orientation:frame_side_data=side_data_type",
            "-of",
            "json",
        ])
        .arg(path);
    let output = bounded_probe(command, std::time::Duration::from_secs(30))?;
    let value = graph::json::parse(
        std::str::from_utf8(&output).map_err(|_| "still frame probe is not UTF-8")?,
    )?;
    let Some(Value::Arr(frames)) = value.get("frames") else {
        return Err("linked still did not decode to one image".into());
    };
    let [frame] = frames.as_slice() else {
        return Err("linked still did not decode to one image".into());
    };
    let raster = number(frame, "width").zip(number(frame, "height"))
        .filter(|raster| match kind {
            "tiff" => *raster == (1920, 64),
            "png" => matches!(raster, (1920, 1080) | (1536, 1024)),
            _ => *raster == (1920, 1080),
        })
        .filter(|_| number(frame, "stream_index") == Some(0))
        .ok_or("linked still decoded raster or stream is outside the supported profile")?;
    if frame.get("tags").is_some_and(|tags| {
        tags.get("Orientation")
            .or_else(|| tags.get("orientation"))
            .is_some()
    }) || !frame.get("side_data_list").is_none_or(|data| {
        matches!(data, Value::Arr(rows) if rows.iter().all(|row| {
            kind == "tiff" && matches!(text(row, "side_data_type").as_deref(),
                Some("3x3 displaymatrix" | "EXIF metadata"))
        }))
    })
    {
        return Err(
            "linked still orientation or frame side data requires separate qualification".into(),
        );
    }
    Ok(Some(raster))
}

// The TIFF demuxer presents one packet for the whole file. Validate the main
// image directory separately so a second page or an ignored Orientation tag
// cannot pass as the qualified still.
fn check_tiff_directory(file: &mut (impl Read + Seek)) -> Option<()> {
    let mut header = [0; 8];
    file.read_exact(&mut header).ok()?;
    let little = match &header[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return None,
    };
    let short = if little { u16::from_le_bytes } else { u16::from_be_bytes };
    let long = if little { u32::from_le_bytes } else { u32::from_be_bytes };
    let offset = long(header[4..].try_into().ok()?);
    if offset < 8 {
        return None;
    }
    file.seek(SeekFrom::Start(offset.into())).ok()?;
    let mut count = [0; 2];
    file.read_exact(&mut count).ok()?;
    let count = short(count);
    if !(1..=256).contains(&count) {
        return None;
    }
    let mut previous_tag = 0;
    for _ in 0..count {
        let mut entry = [0; 12];
        file.read_exact(&mut entry).ok()?;
        let tag = short([entry[0], entry[1]]);
        if tag <= previous_tag || matches!(tag, 330 | 34675) {
            return None;
        }
        previous_tag = tag;
        if tag == 274
            && (short([entry[2], entry[3]]) != 3
                || long(entry[4..8].try_into().ok()?) != 1
                || short([entry[8], entry[9]]) != 1)
        {
            return None;
        }
    }
    let mut next = [0; 4];
    file.read_exact(&mut next).ok()?;
    (next == [0; 4]).then_some(())
}

pub(super) fn probe_mp3(path: &Path, tool: &Path, metadata: &Value) -> Result<Mp3Extent, String> {
    let (format, stream) = single_stream(metadata)?;
    if format != "mp3"
        || text(stream, "codec_name").as_deref() != Some("mp3")
        || text(stream, "codec_type").as_deref() != Some("audio")
    {
        return Err("linked MP3 requires one MP3 audio stream".into());
    }
    let sample_rate = number(stream, "sample_rate")
        .filter(|rate| matches!(rate, 44_100 | 48_000))
        .ok_or("linked MP3 requires a 44.1 or 48 kHz sample clock")?;
    let channels = number(stream, "channels")
        .filter(|n| matches!(n, 1 | 2))
        .ok_or("linked MP3 channel count is unsupported")?;
    let ticks_per_sample = ratio(stream, "time_base")
        .filter(|base| base.num() > 0)
        .and_then(|base| {
            Rational::new(base.den().into(), i128::from(base.num()) * i128::from(sample_rate))
        })
        .and_then(|rate| rate.to_units(1))
        .filter(|&n| n > 0)
        .ok_or("linked MP3 time base cannot represent the sample clock")?;
    let start = number(stream, "start_pts")
        .filter(|n| *n >= 0 && *n % ticks_per_sample == 0)
        .ok_or("linked MP3 presentation start is unresolved")?;
    let mut command = Command::new(tool);
    command
        .args([
            "-v",
            "error",
            "-err_detect",
            "explode",
            "-protocol_whitelist",
            "file,pipe",
            "-show_frames",
            "-show_entries",
            "frame=stream_index,pts,nb_samples",
            "-of",
            "compact=p=0",
        ])
        .arg(path);
    let frames = bounded_probe(command, std::time::Duration::from_secs(30))?;
    let frames = std::str::from_utf8(&frames).map_err(|_| "MP3 frame probe output is not UTF-8")?;
    let (samples, coded_samples) = mp3_samples(frames, start, ticks_per_sample)?;
    Ok(Mp3Extent {
        channels,
        sample_rate,
        samples,
        coded_samples,
        start_samples: start / ticks_per_sample,
        avid_delay_samples: if channels == 2 {
            avid_mp3_delay(path, metadata, coded_samples, start / ticks_per_sample, sample_rate)
        } else {
            None
        },
    })
}

fn mp3_samples(frames: &str, start: i64, ticks: i64) -> Result<(i64, i64), String> {
    let mut lines = frames.lines().peekable();
    let mut next = start;
    let mut samples = 0_i64;
    let mut coded_samples = 0_i64;
    while let Some(line) = lines.next() {
        let mut fields = line.split('|');
        let field = |value: Option<&str>, key: &str| value?.strip_prefix(key)?.parse::<i64>().ok();
        let (Some(0), Some(pts), Some(count), None) = (
            field(fields.next(), "stream_index="),
            field(fields.next(), "pts="),
            field(fields.next(), "nb_samples="),
            fields.next(),
        ) else {
            return Err("linked MP3 frame probe is incomplete".into());
        };
        if pts != next
            || !(1..=1152).contains(&count)
            || (coded_samples > 0 && lines.peek().is_some() && count != 1152)
        {
            return Err("linked MP3 frames do not form a continuous playable sample range".into());
        }
        samples = samples
            .checked_add(count)
            .ok_or("MP3 sample extent overflow")?;
        coded_samples = coded_samples
            .checked_add(1152)
            .ok_or("MP3 coded extent overflow")?;
        next = count
            .checked_mul(ticks)
            .and_then(|n| next.checked_add(n))
            .ok_or("MP3 frame end overflow")?;
    }
    if samples == 0 {
        return Err("linked MP3 has no decoded audio samples".into());
    }
    Ok((samples, coded_samples))
}

fn single_stream(metadata: &Value) -> Result<(String, &Value), String> {
    let format = metadata
        .get("format")
        .and_then(|f| text(f, "format_name"))
        .ok_or("original format missing")?;
    if let Some(Value::Arr(streams)) = metadata.get("streams") {
        if let [stream] = streams.as_slice() {
            if number(stream, "index") == Some(0) {
                return Ok((format, stream));
            }
        }
    }
    Err("linked MP3 or still requires one physical stream at index zero".into())
}

pub(super) fn select(
    master: &av::SourceMaster,
    i: usize,
    path: &Path,
    probe: &OriginalProbe,
) -> Result<Option<Selected>, String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "mp3" && image_kind(path).is_none() {
        return Ok(None);
    }
    let track = &master.master.tracks[i];
    let raw = master.originals[i].as_ref().expect("saved original");
    if master.offsets.get(i) != Some(&av::Observed::Known(0)) {
        return Err("linked MP3 or still requires a verified zero item-to-file offset".into());
    }
    let mut mp3_delay_samples = None;
    let (container, kind, rate, raster, sample_rate, channels, channel, units) = if extension
        == "mp3"
    {
        let mp3 = probe
            .mp3
            .as_ref()
            .ok_or("linked MP3 playable extent was not verified")?;
        let rate = master.rate.ok_or("linked MP3 master clock is unknown")?;
        let channel = raw
            .file_track
            .and_then(|n| n.checked_sub(1))
            .filter(|n| (0..mp3.channels).contains(n))
            .ok_or("linked MP3 file track does not select one of its channels")?;
        if raw.descriptor_class != "MPGA"
            || (raw.descriptor_rate - mp3.sample_rate as f64 / 1152.0).abs() > 0.001
            || raw.sample_rate != Some(mp3.sample_rate as f64)
            || raw.stored_quantization_bits != Some(0)
            || raw.channels != Some(mp3.channels)
            || track.kind != KIND_SOUND
            || !matches!(track.essence.as_ref(),Some(Essence::Other(class)) if class == "MPGA")
            || raw.descriptor_length != track.length
            || master.master.tracks.len() != mp3.channels as usize
            || master
                .master
                .tracks
                .iter()
                .any(|t| t.kind != KIND_SOUND || t.file_mob != track.file_mob)
            || master.master.length <= 0
        {
            return Err("linked MP3 physical extent, stream, or audio clock disagrees with the saved Avid master".into());
        }
        if raw.descriptor_length != mp3.samples && raw.descriptor_length != mp3.coded_samples {
            if raw.descriptor_length == mp3.coded_samples + MPEG_FRAME_SAMPLES
                && rate == av::Rate::new(25, 1).unwrap()
                && master.master.length == raw.descriptor_length / (mp3.sample_rate / 25)
                && master.master.tracks.iter().all(|t| t.clip_length == master.master.length)
                && master.master.markers.is_empty()
            {
                mp3_delay_samples = mp3.avid_delay_samples;
            }
            if mp3_delay_samples.is_none() {
                return Err(format!(
                    "linked MP3 saved Avid descriptor declares {} samples. The file has {} playable samples and {} samples in its decoded MPEG frames. The saved sample range cannot be verified.",
                    raw.descriptor_length, mp3.samples, mp3.coded_samples
                ));
            }
        }
        if mp3_delay_samples.is_none()
            && i128::from(master.master.length) * i128::from(rate.den) * i128::from(mp3.sample_rate)
            > i128::from(mp3.samples) * i128::from(rate.num)
        {
            return Err(format!(
                "linked MP3 saved Avid master length of {} frames at {}/{} fps exceeds the file's {} playable samples at {} Hz.",
                master.master.length, rate.num, rate.den, mp3.samples, mp3.sample_rate
            ));
        }
        (
            "mp3",
            StreamKind::Audio,
            None,
            None,
            Some(mp3.sample_rate),
            Some(mp3.channels),
            Some(channel),
            mp3.samples,
        )
    } else {
        let (format, stream) = single_stream(&probe.metadata)?;
        let kind = image_kind(path).expect("supported still suffix");
        let raster = probe.still_decoded.ok_or("linked still was not decoded")?;
        let physical = match kind {
            "png" => {
                format == "png_pipe"
                    && text(stream, "codec_name").as_deref() == Some("png")
                    && match text(stream, "pix_fmt").as_deref() {
                        Some("rgb24") => true,
                        Some("rgba") => raster == (1920, 1080),
                        _ => false,
                    }
            }
            "tiff" => {
                format == "tiff_pipe"
                    && text(stream, "codec_name").as_deref() == Some("tiff")
                    && matches!(text(stream, "pix_fmt").as_deref(), Some("rgb48le" | "rgb48be"))
                    && raster == (1920, 64)
            }
            _ => {
                matches!(format.as_str(), "image2" | "jpeg_pipe")
                    && text(stream, "codec_name").as_deref() == Some("mjpeg")
                    && matches!(
                        text(stream, "pix_fmt").as_deref(),
                        Some("yuvj420p" | "yuvj422p" | "yuvj444p")
                    )
            }
        };
        if !physical
            || text(stream, "codec_type").as_deref() != Some("video")
            || number(stream, "width").zip(number(stream, "height")) != Some(raster)
            || !matches!(
                text(stream, "sample_aspect_ratio").as_deref(),
                None | Some("1:1" | "N/A")
            )
            || !stream
                .get("side_data_list")
                .is_none_or(|v| matches!(v, Value::Arr(a) if a.is_empty()))
            || !matches!(probe.metadata.get("packets"),Some(Value::Arr(packets)) if packets.len()==1)
            || master.master.tracks.len() != 1
            || track.kind != KIND_PICTURE
            || track.index != 1
            || master.rate != av::Rate::new(24, 1)
            || raw.descriptor_rate != 24.0
            || track.edit_rate != 24.0
            || raw.descriptor_class != "RGBA"
            || raw.display_raster != Some(raster)
            || raw.file_track != Some(1)
            || raw.descriptor_length != 1
            || track.length != 1
            || master.master.length <= 0
            || track.clip_length != master.master.length
        {
            return Err("linked still requires a matching 24 fps single-image Avid descriptor and original".into());
        }
        (
            kind,
            StreamKind::Video,
            Some(Rational::integer(24)),
            Some(raster),
            None,
            None,
            None,
            1,
        )
    };
    let time_base = Rational::new(1, sample_rate.unwrap_or(24).into()).unwrap();
    let editorial_extent = if kind == StreamKind::Video {
        master.master.length
    } else {
        units
    };
    Ok(Some(Selected {
        verified_for_native_import: false,
        path: path.into(),
        online: true,
        stream: OriginalStream {
            saved_locators: raw.locators.clone(),
            container: container.into(),
            stream_index: 0,
            channel_index: channel,
            channels,
            time_base,
            start_pts: 0,
            duration_ts: units,
            sample_representation: (kind == StreamKind::Audio).then(|| "mp3".into()),
        },
        kind,
        codec: container.into(),
        raster,
        rate,
        sample_rate,
        bits: (container == "tiff").then_some(16),
        duration: Rational::new(editorial_extent.into(), time_base.den().into())
            .ok_or("linked media duration overflow")?,
        motion_audio_origin: None,
        same_rate_aac_origin: false,
        mp3_delay_samples,
        column_timecode: None,
        embedded_mov_timecode: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiff_page_and_orientation_are_checked_even_when_the_demuxer_has_one_packet() {
        let mut tiff = *b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0";
        let check = |bytes| check_tiff_directory(&mut std::io::Cursor::new(bytes)).is_some();
        assert!(check(tiff));
        tiff[18] = 3;
        assert!(!check(tiff));
        tiff[18] = 1;
        tiff[22] = 8;
        assert!(!check(tiff));
    }

    #[test]
    fn native_mp3_delay_requires_an_info_frame_before_the_first_audio_packet() {
        let mut file = [0; 671];
        file[..10].copy_from_slice(b"ID3\x04\0\0\0\0\0\x23");
        file[45..49].copy_from_slice(&[0xff, 0xfb, 0xb4, 0x00]);
        file[81..85].copy_from_slice(b"Info");
        file[88] = 1;
        file[92] = 168;
        let qualify = |bytes, position, samples, rate| {
            leading_info_frame(&mut std::io::Cursor::new(bytes), position, samples, rate).is_some()
        };
        assert!(qualify(file, 621, 193_536, 48_000));
        assert!(!qualify(file, 620, 193_536, 48_000));
        assert!(!qualify(file, 621, 194_688, 48_000));
        assert!(!qualify(file, 671, 193_536, 44_100));
        file[47] = 0xb0;
        file[92] = 155;
        assert!(qualify(file, 671, 178_560, 44_100));
        assert!(!qualify(file, 621, 178_560, 48_000));
        file[81..85].copy_from_slice(b"Xing");
        assert!(!qualify(file, 671, 178_560, 44_100));
    }

    #[test]
    fn mp3_trims_decoder_delay_and_tail_without_losing_the_sample_grid() {
        // FFprobe's 1/14112000 time base has 294 ticks per sample at 48 kHz
        // and 320 ticks per sample at 44.1 kHz.
        for ticks in [294, 320] {
            let start = 1105 * ticks;
            let rows = |middle_count, tail_shift| {
                format!(
                    "stream_index=0|pts={start}|nb_samples=47\n\
                     stream_index=0|pts={}|nb_samples={middle_count}\n\
                     stream_index=0|pts={}|nb_samples=721\n",
                    1152 * ticks,
                    2304 * ticks + tail_shift
                )
            };
            assert_eq!(mp3_samples(&rows(1152, 0), start, ticks).unwrap(), (1920, 3456));
            assert!(mp3_samples(&rows(1152, 1), start, ticks).is_err());
            assert!(mp3_samples(&rows(1151, 0), start, ticks).is_err());
        }
    }
}
