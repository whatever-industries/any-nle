//! Rebuild a saved split-source AutoSync subclip as a reusable Premiere sequence.
use super::subclips::{field_int, mob_id};
use super::*;
use prproj::convert::prepared::SampleRepresentation;

pub(super) fn prepare(
    row: &Value,
    name: &str,
    tracks: &[Value],
    parents: &HashMap<&str, Option<&SourceItem>>,
    media: &HashMap<&str, &SourceMedia>,
    clips: &HashMap<&str, &PreparedClip>,
    sidecars: &Sidecars<'_>,
) -> Result<(Sequence, bool), &'static str> {
    let rate_label = key(row, "rate").and_then(str_value);
    let (rate, split_wavs) = match rate_label {
        Some("24") => (Rational::integer(24), false),
        Some("24000/1001") => (Rational::new(24_000, 1001).unwrap(), true),
        _ => return Err("AutoSync requires a supported exact picture rate"),
    };
    let frame = model::ticks_per_unit(fraction(rate));
    let backing = field_int(row, "backing_length").ok_or("saved backing length is unresolved")?;
    let bound = |key_name| key(row, key_name).and_then(|value| field_int(value, "value"));
    let (start, end) = bound("start")
        .zip(bound("end"))
        .ok_or("saved In and Out are unresolved")?;
    if !(0 <= start && start < end && end <= backing) || !(2..=9).contains(&tracks.len()) {
        return Err("AutoSync requires a bounded V1 range and up to eight independent mono lanes");
    }
    if split_wavs && tracks.len() != 3 {
        return Err("23.976 AutoSync requires V1 and two independent linked mono WAVs");
    }
    let native_mob = key(row, "mob").and_then(str_value);
    let from_track =
        |kind: i64, number: i64| -> Result<(&SourceItem, &PreparedClip, i64), &'static str> {
            let mut matches = tracks.iter().filter(|t| {
                field_int(t, "kind") == Some(kind) && field_int(t, "label") == Some(number)
            });
            let t = matches.next().ok_or("AutoSync source lane is missing")?;
            if matches.next().is_some()
                || key(t, "component_class").and_then(str_value) != Some("SCLP")
                || key(t, "rate").and_then(str_value) != rate_label
                || field_int(t, "length") != Some(backing)
                || field_int(t, "target_track") != Some(if split_wavs { 1 } else { number })
            {
                return Err(
                "AutoSync source lane is ambiguous or is not a direct same-rate source reference",
            );
            }
            let mob = key(t, "target_mob")
                .and_then(str_value)
                .filter(|id| mob_id(id) && Some(*id) != native_mob)
                .ok_or("AutoSync backing master identity is missing, invalid or recursive")?;
            let item =
                parents.get(mob).copied().flatten().ok_or(
                    "AutoSync backing master is missing or ambiguous in the selected bins",
                )?;
            if item.rate != Some(rate)
                || !item.markers.is_empty()
                || !item.stream_groups.is_empty()
            {
                return Err(
                "AutoSync backing master has an unsupported clock, source markers or grouped audio",
            );
            }
            let clip = clips
                .get(clip_id(&item.id).as_str())
                .copied()
                .ok_or("AutoSync master is not prepared")?;
            let offset = field_int(t, "target_offset")
                .filter(|offset| *offset >= 0)
                .ok_or("AutoSync backing offset is unresolved or negative")?;
            Ok((item, clip, offset))
        };
    let (camera, picture_clip, picture_offset) = from_track(1, 1)?;
    let PreparedClip::Original(clip) = picture_clip else {
        return Err("AutoSync requires a verified picture-only original");
    };
    let picture = clip
        .video
        .as_ref()
        .ok_or("AutoSync picture is unavailable")?;
    let backing_end = |offset: i64| offset.checked_add(backing).and_then(|n| n.checked_mul(frame));
    let fits = |offset, duration| backing_end(offset).is_some_and(|out| out <= duration);
    let picture_codec = if split_wavs { "apch" } else { "avc1" };
    if clip.audio.is_some()
        || clip.container != "mov"
        || picture.codec != picture_codec
        || picture.rate != fraction(rate)
        || (picture.width, picture.height) != (1920, 1080)
        || !fits(picture_offset, picture.duration)
    {
        return Err(if split_wavs {
            "AutoSync requires a linked 1080p23.976 ProRes HQ picture-only original with enough source frames"
        } else {
            "AutoSync requires a linked 1080p24 H.264 picture-only original with enough source frames"
        });
    }
    let [reference] = &camera.media[..] else {
        return Err("AutoSync requires exactly one picture reference");
    };
    let picture_media = &media[reference.media.as_str()];
    if reference.source_offset != Some(Rational::ZERO) || picture_media.online != Some(true) {
        return Err("AutoSync picture offset or online link is unverified");
    }
    let duration = (end - start) * frame;
    let picture_in = (picture_offset + start) * frame;
    let event = |id: &str, source_in, video_track, audio_tracks, audio_channels| Event {
        clip: id.into(),
        record_start: 0,
        duration,
        source_in,
        video_track,
        audio_tracks,
        audio_channels,
        audio: EventAudio::default(),
        stereo: vec![],
        audio_gains: vec![],
        audio_pans: vec![],
        speed_ratio: None,
        source_duration: None,
        color_correction: None,
        transform: None,
        matte: None,
        time_remap: None,
    };
    let mut events = vec![event(
        picture_clip.id(),
        picture_in,
        Some(1),
        vec![],
        vec![],
    )];
    let (sound_item, _, sound_offset) = from_track(2, 1)?;
    let mut paths = HashSet::new();
    let mut packages = HashSet::new();
    let channel_count = tracks.len() - 1;
    for lane in 1..=channel_count as i64 {
        let (sound, audio_clip, offset) = from_track(2, lane)?;
        if split_wavs {
            let end = backing_end(offset).ok_or("AutoSync WAV backing extent overflows")?;
            validate_split_wav(sound, audio_clip, media, &mut paths, end)?;
            events.push(event(audio_clip.id(), (offset + start) * frame, None, vec![lane], vec![1]));
            continue;
        }
        if sound.id != sound_item.id || sound.id == camera.id || offset != sound_offset {
            return Err("AutoSync audio lanes need one separate sound master with a common saved offset");
        }
        let mut matching_media = sound.media.iter().filter(|r| media[r.media.as_str()].stream == Some(lane));
        let reference = matching_media.next().ok_or("AutoSync sound channel has no saved media selector")?;
        let source = &media[reference.media.as_str()];
        if matching_media.next().is_some() || source.kind != StreamKind::Audio
            || reference.source_offset != Some(Rational::ZERO) || source.online != Some(true)
        {
            return Err("AutoSync sound channel has no verified online binding");
        }
        if let PreparedClip::Original(clip) = audio_clip {
            if channel_count != 8 || clip.container != "wav" || clip.video.is_some()
                || !clip.audio.as_ref().is_some_and(|a| a.channels == 8 && fits(offset, a.duration))
                || !source.original.as_ref().is_some_and(|o| o.channel_index == Some(lane - 1))
                || reference.gain_db != 0.0
            {
                return Err("AutoSync requires all eight verified original WAV mono channels");
            }
            events.push(event(audio_clip.id(), (offset + start) * frame, None, vec![lane], vec![lane]));
            continue;
        }
        let PreparedClip::Legacy(audio) = audio_clip else { unreachable!() };
        if audio.video.is_some() || audio.audio.len() != channel_count {
            return Err("AutoSync sound requires distinct managed mono MXF siblings");
        }
        let file = audio.audio.iter().find(|f| f.avid_track == Some(lane))
            .ok_or("AutoSync sound channel has no prepared mono component")?;
        let package = side(sidecars, "avid.source", &source.id)
            .and_then(|receipt| key(receipt, "file_package"))
            .and_then(str_value)
            .filter(|id| mob_id(id))
            .ok_or("AutoSync sound channel has no verified file package")?;
        if source.original.is_some()
            || file.sample_rate != 48_000
            || file.bits != 16
            || file.gain_db != 0.0
            || !file.path.to_ascii_lowercase().ends_with(".mxf")
            || !paths.insert(file.path.as_str())
            || !packages.insert(package.to_ascii_uppercase())
            || !fits(offset, file.duration)
        {
            return Err("AutoSync requires online, distinct PCM16 mono siblings with verified offsets and extents");
        }
        events.push(event(
            audio_clip.id(),
            (offset + start) * frame,
            None,
            vec![lane],
            vec![lane],
        ));
    }
    // The ordinary UME route may have no qualified display clock. In that
    // case the reusable sequence starts at zero, with its media trims intact.
    let clock = picture_media.clock.as_ref().and_then(|clock| {
        clock.exact_start().filter(|start| {
            Some(*start) == picture_media.start
                && clock.rate == rate
                && !clock.drop
        })
    });
    if split_wavs && (clip.proxy_path.is_some() || !clock.is_some_and(|start|
        ticks(start, "AutoSync source clock").ok() == Some(picture.source_start)))
    {
        return Err("23.976 AutoSync requires a verified non-drop picture source clock and original link");
    }
    let start = match clock {
        None => 0,
        Some(start) => ticks(start, "AutoSync source clock")
            .ok()
            .and_then(|ticks| ticks.checked_add(picture_in))
            .ok_or("AutoSync picture clock overflows")?,
    };
    let sequence = Sequence {
        name: name.into(),
        rate: picture.rate,
        width: picture.width,
        height: picture.height,
        events,
        markers: vec![],
        track_levels: vec![],
        audio_crossfades: vec![],
        start,
    };
    Ok((sequence, clock.is_some()))
}

fn validate_split_wav<'a>(
    sound: &SourceItem,
    clip: &'a PreparedClip,
    media: &HashMap<&str, &SourceMedia>,
    paths: &mut HashSet<&'a str>,
    backing_end: i64,
) -> Result<(), &'static str> {
    let unsupported = "23.976 AutoSync requires two distinct online mono PCM24 WAVs with verified ranges";
    let PreparedClip::Original(clip) = clip else { return Err(unsupported) };
    let [reference] = sound.media.as_slice() else { return Err(unsupported) };
    let source = media[reference.media.as_str()];
    if clip.container != "wav" || clip.video.is_some() || clip.proxy_path.is_some()
        || !clip.audio.as_ref().is_some_and(|audio| {
            audio.sample_rate == 48_000 && audio.channels == 1 && audio.stream_index == 0
                && audio.representation == SampleRepresentation::Pcm24 && backing_end <= audio.duration
        })
        || source.kind != StreamKind::Audio || source.stream != Some(1) || source.online != Some(true)
        || reference.gain_db != 0.0 || reference.source_offset != Some(Rational::ZERO)
        || !matches!(clip.channels.as_slice(), [channel] if channel.avid_track == 1 && channel.channel_index == 0)
        || !paths.insert(clip.path.as_str())
    {
        return Err(unsupported);
    }
    Ok(())
}
