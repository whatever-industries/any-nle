//! Source verification for same-rate 25 and 29.97 linked movies.
use super::*;
use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};

pub(super) fn is_25_or_2997(rate: Option<Rational>) -> bool {
    rate.is_some_and(|r| matches!((r.num(), r.den()), (25, 1) | (30_000, 1001)))
}

pub(super) fn frame_audio_extent(frames: i64, rate: av::Rate, round_up: bool) -> Option<i64> {
    let n = i128::from(frames) * 48_000 * i128::from(rate.den);
    let d = i128::from(rate.num);
    i64::try_from((n + if round_up { d - 1 } else { 0 }) / d).ok()
}

pub(super) fn select(
    path: &Path, probe: &Value, master: &av::SourceMaster, ordinal: usize,
) -> Result<Option<(Selected, bool)>, String> {
    let Some(rate) = master.rate.filter(|r| is_25_or_2997(Some(crate::rational(*r)))) else {
        return Ok(None);
    };
    let mp4 = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"));
    let mov = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mov"));
    let mxf = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mxf"));
    if !mp4 && !mov && !mxf { return Ok(None); }
    if mxf {
        if !master.originals.iter().all(|raw| raw.as_ref().is_some_and(|row|
            matches!(row.descriptor_class.as_str(), "CDCI" | "PCMA")))
        {
            return Ok(None);
        }
    } else {
        qualify_container(path, probe)?;
    }
    let Some(Value::Arr(streams)) = probe.get("streams") else {
        return Err("linked movie stream table is missing".into());
    };
    let [video, tail @ ..] = streams.as_slice() else {
        return Err("linked movie picture stream is missing".into());
    };
    let rate_exact = crate::rational(rate);
    let raster = number(video, "width").zip(number(video, "height"))
        .ok_or("linked movie raster is missing")?;
    let brand = probe.get("format").and_then(|f| f.get("tags")).and_then(|f| text(f, "major_brand"));
    let codec = match (text(video, "codec_name").as_deref(), raster) {
        (Some("h264"), (1920, 1080) | (1280, 720) | (2048, 858)) if !mxf => {
            qualify_avc_high(video, 40)?;
            "avc1"
        }
        (Some("h264"), (3840, 2160)) if mp4 => {
            qualify_avc_high(video, 51)?;
            "avc1"
        }
        (Some("hevc"), (1920, 1080)) if mp4 && text(video, "codec_tag_string").as_deref() == Some("hvc1")
            && text(video, "profile").as_deref() == Some("Main 10")
            && text(video, "pix_fmt").as_deref() == Some("yuv420p10le") => "hvc1",
        (Some("prores"), (1920, 1080)) if mov && brand.as_deref() == Some("qt  ")
            && text(video, "codec_tag_string").as_deref() == Some("apch")
            && text(video, "profile").as_deref() == Some("HQ")
            && text(video, "pix_fmt").as_deref() == Some("yuv422p10le") => "apch",
        (Some("dnxhd"), (1920, 1080)) if mxf
            && text(video, "profile").as_deref() == Some("DNXHR LB")
            && text(video, "pix_fmt").as_deref() == Some("yuv422p")
            && text(video, "color_space").as_deref() == Some("bt709")
            && text(video, "color_range").as_deref() == Some("tv") => {
            verify_op1a_package(probe, streams, master)?;
            "AVdn"
        }
        _ => return Err("linked 25/29.97 movie has an unsupported picture profile".into()),
    };
    let defaulted = qualify_movie_geometry_at_rate(video, raster, rate_exact)?;
    let frames = if mxf && ratio(video, "time_base") == Rational::new(rate.den.into(), rate.num.into()) {
        number(video, "duration_ts")
    } else {
        number(video, "nb_frames")
    }.filter(|n| *n > 0)
        .ok_or("linked movie frame count is missing")?;
    let picture_duration = crate::time(frames, rate)?;
    if number(video, "index") != Some(0) || text(video, "codec_type").as_deref() != Some("video")
        || physical_span(video)?.2 != picture_duration
        || master.master.length != frames || shared_saved_locator(master).is_none()
        || master.offsets.len() != master.master.tracks.len()
        || !master.offsets.iter().all(|o| *o == av::Observed::Known(0))
        || !master.master.other_groups.is_empty()
        || !matches!(master.master.stereo_pairs.as_slice(), [] | [(1, 2)])
    {
        return Err("linked movie picture extent, master locator or lane origin disagrees".into());
    }
    let (audio, timecode) = match tail {
        [] if codec == "apch" => (&tail[..0], None),
        [a] if is_audio(a) => (tail, None),
        [a, tc] if codec == "apch" && is_audio(a) => (&tail[..1], Some(tc)),
        [a, b] if (codec == "avc1" && mov || mxf) && is_audio(a) && is_audio(b) => (tail, None),
        _ => return Err("linked movie has unqualified additional streams".into()),
    };
    let embedded = if codec == "apch" {
        prores_origin(path, probe, video, timecode, master, rate)?
    } else {
        if video.get("tags").is_some_and(|tags| tags.get("timecode").is_some())
            || master.master.start.as_ref().is_some_and(|tc| tc.frames != 0 || tc.drop)
        {
            return Err("linked movie has an unverified embedded timecode".into());
        }
        None
    };
    let labels = master.master.tracks.iter().map(|t| (t.kind, t.index)).collect::<HashSet<_>>();
    let expected = if audio.is_empty() { 1 } else { 3 };
    if master.master.tracks.len() != expected || master.originals.len() != expected
        || labels.len() != expected || !labels.contains(&(KIND_PICTURE, 1))
        || expected == 3 && ![1, 2].iter().all(|&lane| labels.contains(&(KIND_SOUND, lane)))
    {
        return Err("linked movie requires the exact saved picture and audio lanes".into());
    }
    let track = &master.master.tracks[ordinal];
    let raw = master.originals[ordinal].as_ref().ok_or("linked movie descriptor is missing")?;
    let (stream, kind, codec, representation, bits, channels, channel, priming) = match track.kind {
        KIND_PICTURE => {
            if raw.descriptor_class != "CDCI" || raw.display_raster != Some(raster)
                || raw.file_track != Some(1) || raw.descriptor_length != frames
                || track.length != if mxf { 0 } else { frames }
                || av::Rate::from_avid(raw.descriptor_rate) != Some(rate)
                || av::Rate::from_avid(track.edit_rate) != Some(rate)
            {
                return Err("linked movie saved picture descriptor disagrees".into());
            }
            (video, StreamKind::Video, codec.to_owned(), None, None, None, None, false)
        }
        KIND_SOUND => {
            let file_track = raw.file_track.filter(|&n| n == track.index)
                .ok_or("linked movie audio selector disagrees with its saved lane")?;
            let (index, channels, channel) = if audio.len() == 2 { (file_track, 1, 0) }
                else { (1, 2, file_track - 1) };
            let stream = &audio[index as usize - 1];
            let samples = frame_audio_extent(frames, rate, mxf).ok_or("linked movie sample extent overflows")?;
            let format = text(stream, "codec_name").ok_or("linked movie audio codec is missing")?;
            if number(stream, "index") != Some(index) || number(stream, "sample_rate") != Some(48_000)
                || number(stream, "channels") != Some(channels) || raw.channels != Some(channels)
                || raw.sample_rate != Some(48_000.0) || number(stream, "duration_ts") != Some(samples)
                || ratio(stream, "time_base") != Rational::new(1, 48_000)
                || !mxf && channels == 2 && text(stream, "channel_layout").as_deref() != Some("stereo")
            {
                return Err("linked movie saved audio layout or sample clock disagrees".into());
            }
            let (representation, bits, priming) = if format == "aac" {
                qualify_stereo_aac_lc(stream)?;
                if codec == "apch" || !has_aac_priming_packet(probe, index)
                    || !same_rate_aac_master(master, samples, video, rate)
                {
                    return Err("linked movie AAC extent or saved priming route disagrees".into());
                }
                ("aac", None, true)
            } else {
                let tag = text(stream, "codec_tag_string");
                let (representation, bits) = match (format.as_str(), tag.as_deref(), mp4) {
                    ("pcm_s16le", Some("ipcm"), true) | ("pcm_s16le", Some("sowt"), false) => ("pcm16", 16),
                    ("pcm_s24le", Some("ipcm"), true) | ("pcm_s24le" | "pcm_s24be", Some("in24"), false) => ("pcm24", 24),
                    ("pcm_s16le", _, false) if mxf => ("pcm16", 16),
                    ("pcm_s24le", _, false) if mxf => ("pcm24", 24),
                    _ => return Err("linked movie audio representation is unqualified".into()),
                };
                if codec == "hvc1" || number(stream, "bits_per_sample") != Some(bits)
                    || raw.descriptor_class != "PCMA" || raw.stored_quantization_bits != Some(bits)
                    || raw.descriptor_rate != 48_000.0 || raw.descriptor_length != samples
                    || if mxf { track.length != 0 || av::Rate::from_avid(track.edit_rate) != Some(rate) }
                       else { track.edit_rate != 48_000.0 || track.length != samples }
                {
                    return Err("linked movie saved PCM representation or extent disagrees".into());
                }
                (representation, Some(bits), false)
            };
            (stream, StreamKind::Audio, format, Some(representation), bits, Some(channels), Some(channel), priming)
        }
        _ => return Err("linked movie track kind is unsupported".into()),
    };
    let (time_base, duration_ts, duration) = physical_span(stream)?;
    let is_picture = kind == StreamKind::Video;
    Ok(Some((Selected {
        path: path.into(), online: true,
        stream: OriginalStream {
            saved_locators: raw.locators.clone(), container: if mxf { "mxf" } else { "mov" }.into(),
            stream_index: number(stream, "index").ok_or("linked movie stream index is absent")?,
            time_base, start_pts: 0, duration_ts, channels, channel_index: channel,
            sample_representation: representation.map(str::to_owned),
        },
        kind, codec, raster: is_picture.then_some(raster), rate: is_picture.then_some(rate_exact),
        sample_rate: (!is_picture).then_some(48_000), bits, duration,
        motion_audio_origin: None, same_rate_aac_origin: priming, mp3_delay_samples: None,
        column_timecode: None, embedded_mov_timecode: if is_picture { embedded.map(av::Observed::Known) } else { None },
    }, defaulted)))
}

fn verify_op1a_package(probe: &Value, streams: &[Value], master: &av::SourceMaster) -> Result<(), String> {
    let format = probe.get("format").ok_or("linked MXF format declaration is absent")?;
    let tags = format.get("tags").ok_or("linked MXF package declaration is absent")?;
    let op = text(tags, "operational_pattern_ul");
    let file = master.master.tracks.first().ok_or("linked MXF has no saved file tracks")?.file_mob;
    let file_id = file.umid_hex();
    let material_id = master.master.mob_id.umid_hex();
    let matches_umid = |value: Option<String>, expected: &str| value
        .as_deref().and_then(|s| s.strip_prefix("0x")).is_some_and(|s| s.eq_ignore_ascii_case(expected));
    if text(format, "format_name").as_deref() != Some("mxf")
        || op.as_deref() != Some("060e2b34.04010101.0d010201.01010900")
        || text(tags, "timecode").as_deref() != Some("00:00:00:00")
        || !matches_umid(text(tags, "material_package_umid"), &material_id)
        || master.master.tracks.iter().any(|t| t.file_mob != file)
        || !streams.iter().all(|s| matches_umid(
            s.get("tags").and_then(|tags| text(tags, "file_package_umid")), &file_id))
    {
        return Err("linked OP1a MXF package, zero timecode or saved stream identity disagrees".into());
    }
    Ok(())
}

fn is_audio(stream: &Value) -> bool {
    text(stream, "codec_type").as_deref() == Some("audio")
}

fn physical_span(stream: &Value) -> Result<(Rational, i64, Rational), String> {
    let base = ratio(stream, "time_base").filter(|r| r.num() > 0).ok_or("linked stream time base is absent")?;
    let len = number(stream, "duration_ts").filter(|n| *n > 0).ok_or("linked stream extent is absent")?;
    if number(stream, "start_pts") != Some(0) { return Err("linked stream has a nonzero origin".into()); }
    let duration = anynle_media_verification::stream_duration(len, base)
        .ok_or("linked stream duration overflows")?;
    Ok((base, len, duration))
}

fn prores_origin(
    path: &Path, probe: &Value, video: &Value, timecode: Option<&Value>,
    master: &av::SourceMaster, rate: av::Rate,
) -> Result<Option<i64>, String> {
    let saved = master.master.start.as_ref().ok_or("saved ProRes source timecode is absent")?;
    let base = if rate.num == 25 { 25 } else { 30 };
    let day = if saved.drop { 2_589_408 } else { base * 86_400 };
    if saved.fps != base || saved.frames < 0 || saved.frames >= day
        || saved.drop && base != 30
    {
        return Err("saved ProRes source timecode is outside the qualified clock".into());
    }
    let Some(tc) = timecode else {
        if saved.frames == 0 && !saved.drop
            && video.get("tags").is_none_or(|tags| tags.get("timecode").is_none())
        {
            return Ok(None);
        }
        return Err("ProRes without an embedded timecode track requires saved zero origin".into());
    };
    let frames = saved.frames;
    let labelled = if saved.drop {
        let ten_minutes = frames / 17_982;
        let remainder = frames % 17_982;
        frames + 18 * ten_minutes + 2 * ((remainder.saturating_sub(2)).max(0) / 1_798)
    } else { frames };
    let mut label = avid_reader::extract::format_timecode(labelled, base);
    if saved.drop { label.replace_range(8..9, ";"); }
    qualify_prores_hq_parts(path, probe, video, tc, 2, crate::rational(rate), &label)?;
    Ok(Some(frames))
}
