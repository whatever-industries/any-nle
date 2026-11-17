//! Source verification for linked movies with exact matching clocks.
use super::*;
use avid_reader::extract::{KIND_PICTURE, KIND_SOUND};

pub(super) fn is_25_or_2997(rate: Option<Rational>) -> bool {
    rate.is_some_and(|r| matches!((r.num(), r.den()), (25, 1) | (30_000, 1001)))
}

pub(super) fn custom_prores_raster(video: &Value) -> bool {
    text(video, "codec_name").as_deref() == Some("prores")
        && ratio(video, "avg_frame_rate").is_some_and(|r|
            matches!((r.num(), r.den()), (24, 1) | (24_000, 1001) | (25, 1) | (30_000, 1001)))
        && number(video, "width").zip(number(video, "height"))
            .is_some_and(|(w, h)| (w, h) != (1920, 1080)
                && (16..=16_384).contains(&w) && (16..=16_384).contains(&h))
}

pub(super) fn frame_audio_extent(frames: i64, rate: av::Rate, round_up: bool) -> Option<i64> {
    let n = i128::from(frames).checked_mul(48_000)?.checked_mul(i128::from(rate.den))?;
    let d = i128::from(rate.num);
    i64::try_from(n.checked_add(if round_up { d - 1 } else { 0 })? / d).ok()
}

pub(super) fn select(
    path: &Path, probe: &Value, master: &av::SourceMaster, ordinal: usize,
) -> Result<Option<(Selected, bool)>, String> {
    let picture_only_avc = avc_picture_only_rate(path, probe);
    let mov = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mov"));
    let custom_prores = mov && matches!(probe.get("streams"),
        Some(Value::Arr(streams)) if streams.first().is_some_and(custom_prores_raster));
    let Some(rate) = master.rate else {
        return if custom_prores {
            Err("linked ProRes HQ master edit rate is unresolved".into())
        } else if picture_only_avc.is_some() {
            Err("linked picture-only AVC master edit rate is unresolved".into())
        } else {
            Ok(None)
        };
    };
    if !is_25_or_2997(Some(crate::rational(rate))) && picture_only_avc.is_none() && !custom_prores {
        return Ok(None);
    }
    let mp4 = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"));
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
    let avc_pcm = !mxf && is_avc_pcm_movie(streams);
    let codec = match (text(video, "codec_name").as_deref(), raster) {
        (Some("h264"), _) if avc_pcm => "avc1",
        (Some("h264"), _) if picture_only_avc.is_some() => "avc1",
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
        (Some("prores"), _) if mov && (raster == (1920, 1080) || custom_prores)
            && brand.as_deref() == Some("qt  ")
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
        _ if custom_prores => return Err("linked custom-raster ProRes requires QuickTime ProRes 422 HQ/apch, 10-bit 4:2:2".into()),
        _ => return Err("linked 25/29.97 movie has an unsupported picture profile".into()),
    };
    let defaulted = if avc_pcm {
        qualify_avc_pcm(streams, path, master)?
    } else {
        qualify_movie_geometry_at_rate(video, raster, rate_exact)?
    };
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
        [] if codec == "apch" || picture_only_avc.is_some() => (&tail[..0], None),
        [a] if is_audio(a) => (tail, None),
        [tc] if custom_prores => (&tail[..0], Some(tc)),
        [a, tc] if codec == "apch" && is_audio(a) => (&tail[..1], Some(tc)),
        [a, b] if (codec == "avc1" && mov || mxf) && is_audio(a) && is_audio(b) => (tail, None),
        _ => return Err("linked movie has unqualified additional streams".into()),
    };
    let embedded = if codec == "apch" {
        prores_origin(path, probe, video, timecode, 1 + audio.len() as i64, master, rate)?
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
        || expected == 1 && (picture_only_avc.is_some() || custom_prores)
            && !master.master.stereo_pairs.is_empty()
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

// The original 1080p24 admission stays in its existing route.
pub(super) fn avc_picture_only_rate(path: &Path, probe: &Value) -> Option<Rational> {
    if !path.extension().is_some_and(|ext|
        ext.eq_ignore_ascii_case("mov") || ext.eq_ignore_ascii_case("mp4"))
    {
        return None;
    }
    let Value::Arr(streams) = probe.get("streams")? else { return None; };
    let [video] = streams.as_slice() else { return None; };
    let rate = ratio(video, "avg_frame_rate")?;
    (text(video, "codec_name").as_deref() == Some("h264")
        && matches!((rate.num(), rate.den()), (24, 1) | (24_000, 1001) | (25, 1) | (30_000, 1001))
        && !(rate == Rational::integer(24) && number(video, "width") == Some(1920)
            && number(video, "height") == Some(1080) && number(video, "level") == Some(40)))
        .then_some(rate)
}

pub(super) fn prove_avc_picture_cadence(
    path: &Path, tool: &Path, probe: &Value, rate: Rational,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    qualify_container(path, probe)?;
    let Some(Value::Arr(streams)) = probe.get("streams") else {
        return Err("linked picture-only AVC stream table is missing".into());
    };
    let [video] = streams.as_slice() else {
        return Err("linked picture-only AVC requires exactly one stream".into());
    };
    let raster = number(video, "width").zip(number(video, "height"))
        .filter(|(w, h)| (16..=16_384).contains(w) && (16..=16_384).contains(h))
        .ok_or("linked picture-only AVC raster is missing or out of range")?;
    let level = number(video, "level").filter(|n| matches!(n, 40 | 51))
        .ok_or("linked picture-only AVC requires H.264 High level 4.0 or 5.1")?;
    qualify_avc_high(video, level)?;
    qualify_movie_geometry_at_rate(video, raster, rate)?;
    let (base, extent, _) = physical_span(video)?;
    let count = number(video, "nb_frames").filter(|&n| (1..=(1 << 20)).contains(&n))
        .ok_or("linked picture-only AVC needs a bounded declared frame count")?;
    let frame = Rational::new(
        i128::from(rate.den()) * i128::from(base.den()),
        i128::from(rate.num()) * i128::from(base.num()),
    ).filter(|ticks| ticks.den() == 1 && ticks.num() > 0)
        .map(Rational::num)
        .ok_or("linked picture-only AVC frame duration does not fit its file clock")?;
    if count.checked_mul(frame) != Some(extent) {
        return Err("linked picture-only AVC extent disagrees with its frame count".into());
    }
    let mut command = Command::new(tool);
    command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mov",
        "-select_streams", "v:0", "-show_packets", "-show_entries",
        "packet=pts,duration:packet_side_data=", "-of", "csv=p=0"]).arg(path);
    run_probe(command, PICTURE_PACKET_IDLE_LIMIT, check, move |stdout, progress| {
        verify_avc_picture_packets(BufReader::new(stdout), count as usize, frame, || {
            progress.fetch_add(1, Ordering::Relaxed);
        })
    }).map_err(|reason| format!("linked picture-only AVC packet proof failed: {reason}"))
}

fn verify_avc_picture_packets(
    mut packets: impl BufRead, count: usize, frame: i64, mut mark_verified: impl FnMut(),
) -> Result<(), String> {
    // AVC packets can arrive in decode order. Every presentation slot must
    // still occur exactly once on the file's declared frame grid.
    const WORD_BITS: usize = u64::BITS as usize;
    let mut seen = vec![0_u64; count.div_ceil(WORD_BITS)];
    let mut verified = 0;
    let mut bytes = Vec::new();
    while let Some(row) = read_picture_packet_row(&mut packets, &mut bytes)? {
        let (pts, duration) = row.split_once(',').ok_or("linked AVC packet clock is missing")?;
        let pts = pts.parse::<i64>().ok().filter(|&n| n >= 0 && n % frame == 0);
        let slot = pts.and_then(|n| usize::try_from(n / frame).ok()).filter(|&n| n < count);
        let discontinuous = "linked picture-only AVC packet clock is discontinuous";
        let slot = slot.ok_or(discontinuous)?;
        let bit = 1 << (slot % WORD_BITS);
        let word = &mut seen[slot / WORD_BITS];
        if duration.parse::<i64>().ok() != Some(frame) || *word & bit != 0 {
            return Err(discontinuous.into());
        }
        *word |= bit;
        verified += 1;
        mark_verified();
    }
    if verified != count {
        return Err("linked picture-only AVC packet count disagrees with the declared extent".into());
    }
    Ok(())
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
    path: &Path, probe: &Value, video: &Value, timecode: Option<&Value>, timecode_index: i64,
    master: &av::SourceMaster, rate: av::Rate,
) -> Result<Option<i64>, String> {
    let saved = master.master.start.as_ref().ok_or("saved ProRes source timecode is absent")?;
    let base = match (rate.num, rate.den) {
        (24, 1) | (24_000, 1001) => 24,
        (25, 1) => 25,
        (30_000, 1001) => 30,
        _ => return Err("saved ProRes source timecode has an unsupported rate".into()),
    };
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
    verify_prores_hq_timecode_track(path, probe, video, tc, timecode_index, crate::rational(rate), &label)?;
    Ok(Some(frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_packet_proof_accepts_decode_order_with_exact_presentation_grid() {
        assert!(verify_avc_picture_packets(&b"0,1001\n2002,1001\n1001,1001\n"[..], 3, 1001, || {}).is_ok());
        for rows in [
            "0,1001\n1001,1001\n1001,1001\n",
            "0,1001\n1000,1001\n2002,1001\n",
            "0,1001\n1001,1000\n2002,1001\n",
            "0,1001\n1001,1001\n",
            "0,1001\n1001,1001\n2002,1001\n3003,1001\n",
        ] {
            assert!(verify_avc_picture_packets(rows.as_bytes(), 3, 1001, || {}).is_err(), "{rows}");
        }
    }

    #[test]
    fn picture_packet_proof_streams_a_long_reordered_report() {
        use std::fmt::Write;
        let count = 81_600;
        let frame = 10_000;
        let mut report = String::new();
        for group in (0..count).step_by(4) {
            for slot in [0, 3, 1, 2] {
                writeln!(report, "{},{frame}", (group + slot) as i64 * frame).unwrap();
            }
        }
        assert!(report.len() > 1 << 20);
        verify_avc_picture_packets(report.as_bytes(), count, frame, || {}).unwrap();
        let last_row = report[..report.len() - 1].rfind('\n').unwrap() + 1;
        let error = verify_avc_picture_packets(&report.as_bytes()[..last_row], count, frame, || {})
            .unwrap_err();
        assert!(error.contains("packet count"));
        report.replace_range(last_row.., "815990000,10000\n");
        let error = verify_avc_picture_packets(report.as_bytes(), count, frame, || {}).unwrap_err();
        assert!(error.contains("clock is discontinuous"));
    }

    #[test]
    fn linked_pcm_extent_rounds_the_full_duration_once() {
        for (rate, samples) in [
            ((24, 1), 96_000), ((24_000, 1001), 96_096),
            ((25, 1), 92_160), ((30_000, 1001), 76_876),
        ] {
            assert_eq!(frame_audio_extent(48, av::Rate::new(rate.0, rate.1).unwrap(), false), Some(samples));
        }
        let ntsc = av::Rate::new(30_000, 1001).unwrap();
        assert_eq!(frame_audio_extent(48, ntsc, true), Some(76_877));
        assert_eq!(frame_audio_extent(50, ntsc, false), Some(80_080));
        assert_eq!(frame_audio_extent(i64::MAX, ntsc, false), None);
    }
}
