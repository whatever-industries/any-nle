//! Retain AVB stream routing and verify the physical DNxHD Direct Out profile.
use super::*;
use avid_reader::mxf;
use std::io::{Read, Seek, SeekFrom};
use std::process::Command;
use std::time::Duration;

pub const VERIFIED_ADAPTER: &str = "avid.managed_op1a";

pub(super) fn bind(
    facts: &av::SourceProject,
    index: &index::MediaIndex,
    originals: &mut original::OriginalIndex,
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), SourceReadIssue> {
    let mut revisions = HashMap::new();
    for source in &facts.masters {
        if originals.omitted_unused.contains_key(&source.id) {
            continue;
        }
        if source.managed_multistream.is_none()
            && !source.master.tracks.is_empty()
            && source.master.tracks.iter().enumerate().all(|(i, _)|
                originals.tracks.get(&(source.id.clone(), i))
                    .is_some_and(|track| track.stream.container == "mxf"))
        {
            continue;
        }
        let Some(saved) = &source.managed_multistream else {
            if source.master.tracks.iter().any(
                |track| matches!(&track.essence, Some(Essence::Other(class)) if class == "MULD"),
            ) {
                return Err(SourceReadIssue::MediaScan(format!(
                    "{}: managed OP1a lacks a complete saved descriptor and track route",
                    source.master.name
                )));
            }
            continue;
        };
        let issue =
            |reason| SourceReadIssue::MediaScan(format!("{}: {reason}", source.master.name));
        let path = index
            .files
            .get(&saved.file_package.umid_hex())
            .ok_or_else(|| issue("managed OP1a file package needs one online MXF".into()))?;
        if source
            .master
            .tracks
            .iter()
            .enumerate()
            .any(|(i, _)| originals.tracks.contains_key(&(source.id.clone(), i)))
        {
            return Err(issue(
                "managed OP1a and camera-original bindings conflict".into(),
            ));
        }
        let verified = verify_direct_out(source, path, ffprobe, check).map_err(issue)?;
        if let Some(previous) = revisions.insert(path, verified.media_revision.clone()) {
            if previous != verified.media_revision {
                return Err(issue(
                    "managed OP1a file changed between source masters".into(),
                ));
            }
        } else {
            originals.original_inputs.push(av::SourceInput {
                path: path.clone(),
                revision: verified.media_revision.clone(),
            });
        }
        originals.managed_op1a.insert(source.id.clone(), verified);
    }
    Ok(())
}

pub(super) fn add_media(
    source: &av::SourceMaster,
    track: &extract::MediaTrack,
    id: &str,
    audio_index: usize,
    verified: &VerifiedManagedDirectOut,
    index: &index::MediaIndex,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<SourceMedia, MasterError> {
    let route = verified
        .tracks
        .iter()
        .find(|route| route.kind == track.kind && route.master_label == track.index)
        .ok_or("managed OP1a verified track is missing")?;
    let path = index
        .files
        .get(&track.file_mob.umid_hex())
        .and_then(|path| path.to_str())
        .ok_or("managed OP1a path is unavailable or not UTF-8")?;
    let (stream_id, kind, media_rate, duration) = if route.audio_ordinal.is_some() {
        (
            format!("{id}:audio:{audio_index}"),
            StreamKind::Audio,
            Rational::integer(48_000),
            Rational::from_units(verified.samples_per_channel as i64, 48_000),
        )
    } else {
        (
            format!("{id}:video"),
            StreamKind::Video,
            Rational::integer(24),
            Rational::from_units(i64::from(verified.picture_frames), 24),
        )
    };
    let duration = duration.ok_or("managed OP1a duration is outside the source clock")?;
    let audio = kind == StreamKind::Audio;
    let q = &verified.clock;
    budget
        .charge(
            size_of::<SourceMedia>()
                + size_of::<SourceItemMedia>()
                + stream_id.len() * 2
                + path.len()
                + q.source_package.len()
                + q.timecode_track.len()
                + q.route.len()
                + 256,
        )
        .map_err(MasterError::Budget)?;
    let source_clock = SourceClock {
        effective_count: q.effective_count,
        rate: Rational::integer(24),
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: q.origin_count,
            source_offset: 0,
            source_rate: media_rate,
            source_package: q.source_package.clone(),
            timecode_track: q.timecode_track.clone(),
            route: q.route.clone(),
        },
    };
    let saved = source
        .managed_multistream
        .as_ref()
        .ok_or("managed OP1a saved tracks are missing")?;
    sidecar(
        out,
        budget,
        VERIFIED_ADAPTER,
        &stream_id,
        obj(vec![
            ("file_package", s(saved.file_package.umid_hex())),
            (
                "physical_track_number",
                Value::Int(route.physical_track_number.into()),
            ),
            ("mxf_track_id", Value::Int(route.mxf_track_id.into())),
            (
                "decoder_stream_index",
                Value::Int(route.decoder_stream_index),
            ),
            (
                "audio_ordinal",
                route.audio_ordinal.map(Value::Int).unwrap_or(Value::Null),
            ),
            ("sha256", s(&verified.media_revision)),
        ]),
    )
    .map_err(MasterError::Budget)?;
    Ok(SourceMedia {
        id: stream_id,
        kind,
        locations: vec![path.into()],
        codec: (!audio).then(|| "AVdn".into()),
        frame_size: (!audio).then_some((1920, 1080)),
        rate: (!audio).then_some(media_rate),
        sample_rate: audio.then_some(48_000),
        bits: audio.then_some(24),
        duration,
        start: source_clock.exact_start(),
        clock: Some(source_clock),
        stream: audio.then_some(track.index),
        online: Some(true),
        original: None,
    })
}

#[derive(Debug)]
pub struct VerifiedManagedDirectOut {
    pub media_revision: String,
    pub clock: mxf::QualifiedClock,
    pub picture_frames: u32,
    pub samples_per_channel: u64,
    pub tracks: Vec<VerifiedManagedTrack>,
}

#[derive(Debug)]
pub struct VerifiedManagedTrack {
    pub kind: i64,
    pub master_label: i64,
    pub file_label: i64,
    pub physical_track_number: u32,
    pub mxf_track_id: u32,
    pub decoder_stream_index: i64,
    pub audio_ordinal: Option<i64>,
}

/// Read-only verification. Callers must also recheck the returned file revision before publication.
pub fn verify_direct_out(
    source: &av::SourceMaster,
    path: &Path,
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<VerifiedManagedDirectOut, String> {
    const MAX_MANAGED_MXF_BYTES: u64 = 256 * 1024 * 1024;
    let saved = source
        .managed_multistream
        .as_ref()
        .ok_or("no saved managed OP1a routes")?;
    let rate24 = Rate::new(24, 1);
    let rate48k = Rate::new(48_000, 1);
    if source.rate != rate24
        || source.rendered_freeze
        || source.rendered_gain.is_some()
        || !source.master.stereo_pairs.is_empty()
        || !source.master.other_groups.is_empty()
        || saved.tracks.len() != 3
        || saved.descriptors.len() != 3
    {
        return Err(
            "managed OP1a requires one picture and two independent mono tracks at 24 fps".into(),
        );
    }
    let saved_labels: HashSet<_> = saved
        .tracks
        .iter()
        .map(|t| (t.kind, t.master_label))
        .collect();
    if saved_labels != HashSet::from([(KIND_PICTURE, 1), (KIND_SOUND, 1), (KIND_SOUND, 2)]) {
        return Err("managed OP1a master track labels are missing or ambiguous".into());
    }
    let metadata = path.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_MANAGED_MXF_BYTES {
        return Err("managed OP1a source exceeds the bounded file profile".into());
    }
    let revision = revision::file_revision_checked(path, check).map_err(|e| e.to_string())?;
    let header = mxf::read_managed_direct_out(path).map_err(|e| e.to_string())?;
    let frames = i64::from(header.picture_frames);
    let samples = frames * (48_000 / 24);
    if header.material_package != source.master.mob_id.umid_hex()
        || header.file_package != saved.file_package.umid_hex()
        || source.master.length != frames
        || source.master.start.as_ref().is_some_and(|clock| {
            clock.frames as u64 != header.clock.effective_count || clock.fps != 24 || clock.drop
        })
    {
        return Err(
            "managed OP1a package identity, saved extent or source clock disagrees with the AVB"
                .into(),
        );
    }
    let mut pictures = 0;
    for descriptor in &saved.descriptors {
        match &descriptor.class {
            b"CDCI"
                if descriptor.edit_rate == rate24
                    && descriptor.length == Some(frames)
                    && descriptor.bits == Some(8)
                    && descriptor.width == Some(1920)
                    && descriptor.height == Some(1080)
                    && descriptor.frame_layout == Some(0)
                    && descriptor.display_aspect == Some((16, 9))
                    && descriptor.resolution_id == Some(1253)
                    && descriptor.compression == Some(*b"AVHD") =>
            {
                pictures += 1
            }
            b"PCMA"
                if descriptor.edit_rate == rate48k
                    && descriptor.sample_rate == rate48k
                    && descriptor.length == Some(samples)
                    && descriptor.bits == Some(24)
                    && descriptor.channels == Some(1) => {}
            _ => {
                return Err(
                    "saved managed OP1a descriptor disagrees with the DNxHD or PCM extent".into(),
                )
            }
        }
    }
    if pictures != 1 {
        return Err("saved managed OP1a picture descriptor is missing or ambiguous".into());
    }
    let mut seen = HashSet::new();
    let mut matched = Vec::with_capacity(3);
    for track in &saved.tracks {
        let number = track
            .physical_track_number
            .ok_or("missing saved managed OP1a physical track number")?;
        let physical = header
            .tracks
            .iter()
            .find(|t| t.physical_track_number == number)
            .ok_or("saved managed OP1a physical track number is absent from the MXF")?;
        if !seen.insert(number)
            || physical.sound != (track.kind == KIND_SOUND)
            || track.file_source_track != Some(i64::from(physical.upstream_track_number))
            || track.master_offset != Some(0)
            || track.file_source_offset != Some(0)
            || track.file_source_rate != rate24
            || track.file_source_length != Some(frames)
            || track.file_source_track != Some(track.file_label)
            || !track
                .file_source_package
                .is_some_and(|id| id.umid_hex() == header.clock.source_package)
        {
            return Err(
                "managed OP1a track route is ambiguous or its clock disagrees with the MXF".into(),
            );
        }
        matched.push((track, physical));
    }
    check()?;
    let probe = probe_packets(path, ffprobe)?;
    let streams = inspect_packets(path, &header, &probe, check)?;
    let tracks = matched
        .into_iter()
        .map(|(saved, physical)| {
            let (index, ordinal) = streams[&physical.physical_track_number];
            VerifiedManagedTrack {
                kind: saved.kind,
                master_label: saved.master_label,
                file_label: saved.file_label,
                physical_track_number: physical.physical_track_number,
                mxf_track_id: physical.track_id,
                decoder_stream_index: index,
                audio_ordinal: ordinal,
            }
        })
        .collect();
    if revision::file_revision_checked(path, check).map_err(|e| e.to_string())? != revision {
        return Err("managed OP1a bytes changed during physical stream verification".into());
    }
    Ok(VerifiedManagedDirectOut {
        media_revision: revision,
        clock: header.clock,
        picture_frames: header.picture_frames,
        samples_per_channel: samples as u64,
        tracks,
    })
}

fn probe_packets(path: &Path, ffprobe: &Path) -> Result<Value, String> {
    let mut command = Command::new(ffprobe);
    command.args(["-v", "error", "-show_streams", "-show_packets", "-show_entries",
        "stream=index,codec_type,codec_name,profile,pix_fmt,width,height,r_frame_rate,time_base,start_pts,duration_ts,sample_aspect_ratio,field_order,sample_rate,channels,bits_per_sample:packet=stream_index,pos,pts,dts,duration,size",
        "-of", "json"]).arg(path);
    let bytes = original::bounded_probe(command, Duration::from_secs(30))?;
    graph::json::parse(std::str::from_utf8(&bytes).map_err(|_| "managed OP1a probe is not UTF-8")?)
}

fn inspect_packets(
    path: &Path,
    header: &mxf::ManagedDirectOut,
    probe: &Value,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<HashMap<u32, (i64, Option<i64>)>, String> {
    let Some(Value::Arr(streams)) = probe.get("streams") else {
        return Err("managed OP1a probe has no streams".into());
    };
    let Some(Value::Arr(packets)) = probe.get("packets") else {
        return Err("managed OP1a probe has no packets".into());
    };
    let frames = i64::from(header.picture_frames);
    if streams.len() != 3 || packets.len() != 3 * frames as usize {
        return Err("managed OP1a physical stream or packet count disagrees with the MXF".into());
    }
    let number = original::number;
    let text = |value: &Value, field, expected| matches!(value.get(field), Some(Value::Str(s)) if s == expected);
    let mut stream_kinds = HashMap::new();
    let mut audio_ordinal = 0;
    for (position, stream) in streams.iter().enumerate() {
        let index = number(stream, "index").ok_or("missing managed OP1a decoder stream index")?;
        if index != position as i64 || number(stream, "start_pts") != Some(0) {
            return Err("managed OP1a stream index or origin is inconsistent".into());
        }
        let ordinal = if text(stream, "codec_type", "audio")
            && text(stream, "codec_name", "pcm_s24le")
            && number(stream, "sample_rate") == Some(48_000)
            && number(stream, "channels") == Some(1)
            && number(stream, "bits_per_sample") == Some(24)
            && text(stream, "time_base", "1/48000")
            && number(stream, "duration_ts") == Some(frames * 2000)
        {
            let ordinal = audio_ordinal;
            audio_ordinal += 1;
            Some(ordinal)
        } else if text(stream, "codec_type", "video")
            && text(stream, "codec_name", "dnxhd")
            && text(stream, "profile", "DNXHD")
            && text(stream, "pix_fmt", "yuv422p")
            && text(stream, "sample_aspect_ratio", "1:1")
            && text(stream, "field_order", "progressive")
            && text(stream, "r_frame_rate", "24/1")
            && text(stream, "time_base", "1/24")
            && number(stream, "width") == Some(1920)
            && number(stream, "height") == Some(1080)
            && number(stream, "duration_ts") == Some(frames)
        {
            None
        } else {
            return Err(
                "managed OP1a decoder stream is outside the qualified picture or PCM profile"
                    .into(),
            );
        };
        stream_kinds.insert(index, ordinal);
    }
    if audio_ordinal != 2 {
        return Err("managed OP1a requires two physical mono PCM streams".into());
    }
    let mut input = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let file_size = input.metadata().map_err(|e| e.to_string())?.len();
    let mut previous_packet_end = 0;
    let mut mapping = HashMap::new();
    let mut counts = HashMap::new();
    for packet in packets {
        check()?;
        let index = number(packet, "stream_index").ok_or("missing managed OP1a packet stream")?;
        let ordinal = *stream_kinds
            .get(&index)
            .ok_or("unknown managed OP1a packet stream")?;
        let position = number(packet, "pos")
            .and_then(|n| u64::try_from(n).ok())
            .ok_or("missing managed OP1a packet position")?;
        let size = number(packet, "size")
            .and_then(|n| u64::try_from(n).ok())
            .ok_or("missing managed OP1a packet size")?;
        let expected_size = if ordinal.is_some() { 6000 } else { 188416 };
        if position < previous_packet_end || size != expected_size {
            return Err("managed OP1a packet position or payload size is inconsistent".into());
        }
        let (key, end) = packet_key(&mut input, file_size, position, size)?;
        previous_packet_end = end;
        let track = header
            .tracks
            .iter()
            .find(|t| t.matches_packet_key(key))
            .ok_or("managed OP1a packet key does not identify a saved file track")?;
        let physical = track.physical_track_number;
        let count = counts.entry(physical).or_insert(0);
        let step = if track.sound { 2000 } else { 1 };
        if track.sound != ordinal.is_some()
            || number(packet, "pts") != Some(*count * step)
            || number(packet, "dts") != Some(*count * step)
            || number(packet, "duration") != Some(step)
            || mapping
                .insert(physical, (index, ordinal))
                .is_some_and(|prior| prior != (index, ordinal))
        {
            return Err("managed OP1a packet clock or track-to-stream assignment changed".into());
        }
        *count += 1;
    }
    if mapping.len() != 3
        || mapping
            .values()
            .map(|(index, _)| *index)
            .collect::<HashSet<_>>()
            .len()
            != 3
        || counts.values().any(|&count| count != frames)
    {
        return Err("managed OP1a decoder streams do not match the complete file tracks".into());
    }
    Ok(mapping)
}

fn packet_key(
    input: &mut (impl Read + Seek),
    file_size: u64,
    position: u64,
    size: u64,
) -> Result<([u8; 16], u64), String> {
    let read_error = |error: std::io::Error| format!("managed OP1a packet header: {error}");
    input.seek(SeekFrom::Start(position)).map_err(read_error)?;
    let mut header = [0_u8; 17];
    input.read_exact(&mut header).map_err(read_error)?;
    let count = usize::from(header[16] & 0x7f);
    let (length, extra) = if header[16] < 0x80 {
        (u64::from(header[16]), 0)
    } else {
        if !(1..=8).contains(&count) {
            return Err("managed OP1a packet has an invalid BER length".into());
        }
        let mut bytes = [0_u8; 8];
        input
            .read_exact(&mut bytes[8 - count..])
            .map_err(read_error)?;
        (u64::from_be_bytes(bytes), count as u64)
    };
    let end = position
        .checked_add(17 + extra)
        .and_then(|p| p.checked_add(length))
        .filter(|&end| end <= file_size && length == size)
        .ok_or("managed OP1a packet payload does not match the complete KLV extent")?;
    Ok((header[..16].try_into().unwrap(), end))
}

#[cfg(test)]
mod tests {
    use super::packet_key;
    use std::io::Cursor;

    #[test]
    fn packet_extent_requires_the_complete_length_and_payload() {
        let key = [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 0x16, 2, 1, 0];
        let mut file = key.to_vec();
        file.extend_from_slice(&[0x82, 0, 3, 1, 2, 3]);
        let length = file.len() as u64;
        let mut reader = Cursor::new(&file);
        assert_eq!(
            packet_key(&mut reader, length, 0, 3).unwrap(),
            (key, length)
        );
        assert!(packet_key(&mut reader, length - 1, 0, 3).is_err());
        assert!(packet_key(&mut reader, length, 0, 2).is_err());
        file[16] = 0x80;
        assert!(packet_key(&mut Cursor::new(file), length, 0, 3).is_err());
    }
}

pub(super) fn saved_facts(
    managed: &av::ManagedMultistream,
    budget: &Budget,
) -> Result<Value, String> {
    let mut reservation = *budget;
    reservation.charge(512 + managed.tracks.len() * 1500 + managed.descriptors.len() * 1200)?;
    let int = |number: Option<i64>| number.map(Value::Int).unwrap_or(Value::Null);
    let rate = |rate: Option<Rate>| rate.map(|r| s(rational(r))).unwrap_or(Value::Null);
    let routes = managed
        .tracks
        .iter()
        .map(|track| {
            obj(vec![
                ("kind", Value::Int(track.kind)),
                ("master_label", Value::Int(track.master_label)),
                ("file_label", Value::Int(track.file_label)),
                (
                    "physical_track_number",
                    int(track.physical_track_number.map(i64::from)),
                ),
                ("master_offset", int(track.master_offset)),
                (
                    "file_source_package",
                    track
                        .file_source_package
                        .map(|id| s(id.umid_hex()))
                        .unwrap_or(Value::Null),
                ),
                ("file_source_track", int(track.file_source_track)),
                ("file_source_offset", int(track.file_source_offset)),
                ("file_source_length", int(track.file_source_length)),
                ("file_source_rate", rate(track.file_source_rate)),
            ])
        })
        .collect();
    let descriptors = managed
        .descriptors
        .iter()
        .map(|descriptor| {
            obj(vec![
                ("class", s(String::from_utf8_lossy(&descriptor.class))),
                ("length", int(descriptor.length)),
                ("edit_rate", rate(descriptor.edit_rate)),
                ("sample_rate", rate(descriptor.sample_rate)),
                ("bits", int(descriptor.bits)),
                ("channels", int(descriptor.channels)),
                ("width", int(descriptor.width)),
                ("height", int(descriptor.height)),
                ("frame_layout", int(descriptor.frame_layout)),
                (
                    "display_aspect",
                    descriptor
                        .display_aspect
                        .map(|(n, d)| Value::Arr(vec![Value::Int(n), Value::Int(d)]))
                        .unwrap_or(Value::Null),
                ),
                ("resolution_id", int(descriptor.resolution_id)),
                (
                    "compression",
                    descriptor
                        .compression
                        .map(|code| s(String::from_utf8_lossy(&code)))
                        .unwrap_or(Value::Null),
                ),
            ])
        })
        .collect();
    Ok(obj(vec![
        ("file_package", s(managed.file_package.umid_hex())),
        ("tracks", Value::Arr(routes)),
        ("descriptors", Value::Arr(descriptors)),
    ]))
}
