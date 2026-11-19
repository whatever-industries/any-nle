//! Standalone PCM cache and virtual routing for WAV and mono OP-Atom audio.
use super::*;
use crate::linked::samples_per_frame;

pub(super) fn integer_wav_frame(text: &str) -> Result<i64> {
    integer_frame(text, false)
}

pub(super) fn integer_atom_frame(text: &str) -> Result<i64> {
    integer_frame(text, true)
}

fn integer_frame(text: &str, mono_atom: bool) -> Result<i64> {
    let Some((whole, fraction)) = text.split_once('|') else {
        return Ok(text.parse()?);
    };
    if fraction.len() != 16 {
        return Err("invalid Resolve fractional audio duration encoding".into());
    }
    let whole: i64 = whole.parse()?;
    let fraction = f64::from_le_bytes(hex::decode(fraction)?.as_slice().try_into()?);
    // Resolve sometimes serializes binary64 noise beside an integral frame
    // count. Keep true fractional edits outside this frame-aligned profile.
    // A nonzero MXF origin adds binary64 cancellation at the file clock scale.
    let tolerance = if mono_atom {
        1e-9
    } else {
        (whole.unsigned_abs() as f64 * f64::EPSILON).min(1e-9)
    };
    if fraction.abs() <= tolerance {
        return Ok(whole);
    }
    // Native WAV durations can also save the frame immediately below the
    // boundary, with a positive remainder that lands on the next frame.
    if !mono_atom && (fraction - 1.0).abs() <= tolerance {
        return whole.checked_add(1).ok_or_else(|| "Resolve audio frame overflow".into());
    }
    Err("unqualified Resolve fractional audio edit".into())
}

pub(super) fn identity_timemap(node: &Node, samples: i64, rate: Rational) -> Result<bool> {
    let bytes = hex_bytes(node.get("MediaTimemapBA")?)?;
    let [2, value @ ..] = bytes.as_slice() else {
        return Ok(false);
    };
    let saved = f64::from_be_bytes(value.try_into()?);
    let expected = (samples - samples_per_frame(rate)?) as f64 / 48000.0;
    Ok(close(saved, expected))
}

pub(super) fn stereo_channel(node: &Node, blobs: &mut Blobs) -> Result<Option<i64>> {
    let virtual_track = blobs.qt(node.get("VirtualAudioTrackBA")?)?;
    if virtual_track.int("AudioType")? != 1 || node.get("MediaTrackIdx")? != "0" {
        return Ok(None);
    }
    Ok(match virtual_track.bytes("ChannelsBA")? {
        [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 1] => Some(0),
        [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 2] => Some(1),
        _ => None,
    })
}

fn close(value: f64, expected: f64) -> bool {
    (value - expected).abs() <= expected.abs().max(1.0) * 4.0 * f64::EPSILON
}

pub(super) fn read(
    clip: &Node,
    blobs: &mut Blobs,
    graph: &mut SourceGraph,
) -> Result<Option<Master>> {
    let mut entries = clip.one("EmbeddedAudioVec")?.elements()?;
    let Some(entry) = entries.next() else {
        return Ok(None);
    };
    if entries.next().is_some() {
        return Ok(None);
    }
    let node = entry.entry()?;
    expect(node, "BtAudioInfo")?;
    let (path, codec) = cache(node, blobs)?;
    let suffix = path.rsplit_once('.').map(|(_, ext)| ext);
    let atom = suffix.is_some_and(|ext| ext.eq_ignore_ascii_case("mxf"));
    if codec != "Linear PCM" || !(atom || suffix.is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))) {
        return Ok(None);
    }
    let tracks = blobs.qt(node.get("TracksBA")?)?;
    if !tracks.keys().eq(["0"]) {
        return Ok(None);
    }
    let track = qt(tracks.bytes("0")?)?;
    let channels = track.int("NumChannels")?;
    let samples = track.int("Duration")?;
    let depth = track.int("BitDepth")?;
    let bits = match depth {
        1 => 16,
        2 => 24,
        _ => return Ok(None),
    };
    if !matches!(channels, 1 | 2)
        || track.int("SampleRate")? != 48000
        || track.int("IdxTrack")? != 0
        || (!atom && track.real("StartTime")? != 0.0)
        || track.text("CodecName")? != Some("Linear PCM")
        || samples <= 0
    {
        return Ok(None);
    }
    let fields = blobs.envelope(clip.get("FieldsBlob")?)?;
    let timing = pb_bytes(&fields, 2)?.ok_or("missing standalone WAV import clock")?;
    let timing_fields = encoding::fields(timing)?;
    let [(1, encoding::Wire::Fixed(value))] = timing_fields.as_slice() else {
        return Err("unsupported standalone WAV import clock".into());
    };
    if value.len() != 8 {
        return Err("invalid standalone WAV import clock size".into());
    }
    let mut rate_bytes = [0; 16];
    rate_bytes[..8].copy_from_slice(value);
    let rate = rate(&rate_bytes)?;
    let samples_per_frame = samples_per_frame(rate)?;
    let length = samples / samples_per_frame;
    let start_seconds = track.real("StartTime")?;
    let start = if atom { frames(start_seconds, rate)? } else { 0 };
    let mxf_packages = if atom {
        if rate != crate::prepare::rate() || channels != 1 || samples % samples_per_frame != 0
            || !(0..crate::DAY).contains(&start) || length <= 0
        {
            return Ok(None);
        }
        if !track.keys().any(|key| key == "MXFMetadata") {
            return Ok(None);
        }
        let meta = qt(track.bytes("MXFMetadata")?)?;
        let file: [u8; 32] = meta.bytes("sourceMobId")?.try_into()?;
        let material: [u8; 32] = meta.bytes("masterMobId")?.try_into()?;
        if file == [0; 32] || material == [0; 32] || file == material {
            return Err("invalid standalone MXF package identity".into());
        }
        Some((hex::encode_upper(file), hex::encode_upper(material)))
    } else {
        None
    };
    let outer = pb_bytes(&fields, 1)?.ok_or("missing standalone WAV pool metadata")?;
    let routes = qt(pb_bytes(outer, 5)?.ok_or("missing standalone WAV virtual routes")?)?;

    let id = clip.id()?;
    let duration = Rational::new(samples.into(), 48000).ok_or("invalid WAV duration")?;
    let mut item = SourceItem {
        id: id.into(),
        name: clip.get("Name")?.into(),
        media: vec![],
        umid: mxf_packages.as_ref().map(|(_, material)| material.clone()),
        tape: None,
        rate: None,
        color_rgb16: None,
        stream_groups: vec![],
        markers: vec![],
    };
    let mut selectors = BTreeMap::new();
    let mut physical = BTreeMap::new();
    for selector in routes.keys() {
        let index: i64 = selector.parse()?;
        if !(0..channels).contains(&index) {
            return Ok(None);
        }
        let route = qt(routes.bytes(selector)?)?;
        let extent = route.bytes("MediaExtents")?;
        let [route_start, length] = extent.as_chunks::<8>().0 else {
            return Err("invalid standalone WAV virtual extent".into());
        };
        let route_start = f64::from_le_bytes(*route_start);
        let start_matches = if atom { close(route_start, start_seconds) } else { route_start == 0.0 };
        if extent.len() != 16
            || !start_matches
            || !close(f64::from_le_bytes(*length), samples as f64 / 48000.0)
            || route.int("BitDepth")? != depth
            || route.text("CodecName")? != Some("Linear PCM")
        {
            return Err("standalone PCM virtual extent contradicts audio cache".into());
        }
        let mapping = qt(route.bytes("ChannelVecBA")?)?;
        let grouped = mapping.keys().eq(["0", "1"]);
        if grouped {
            if index != 0 || channels != 2 || !routes.keys().eq(["0"]) || route.int("Type")? != 0 {
                return Ok(None);
            }
            item.stream_groups.push(vec![1, 2]);
        } else if !mapping.keys().eq(["0"]) || route.keys().any(|key| key == "Type") {
            return Ok(None);
        }
        for key in mapping.keys() {
            let channel = qt(mapping.bytes(key)?)?;
            if channel.text("MediaRef")? != Some(node.id()?)
                || optional_int(&channel, "TrackIdx")? != 0
            {
                return Err("standalone PCM virtual source contradicts audio cache".into());
            }
            let physical_channel = optional_int(&channel, "ChannelIdx")?;
            if !(0..channels).contains(&physical_channel)
                || physical.insert(physical_channel, index).is_some()
                || (grouped && key.parse::<i64>()? != physical_channel)
            {
                return Err("ambiguous standalone PCM physical channel".into());
            }
            if !grouped {
                selectors.insert(index, physical_channel);
            }
        }
    }
    if physical.len() != channels as usize {
        return Ok(None);
    }
    for (channel, selector) in physical {
        let label = channel + 1;
        let stream_id = format!("{id}/audio/{label}");
        let mut fields = vec![
            ("saved_selector", Value::Int(selector)),
            ("saved_cache_track", Value::Int(0)),
            ("saved_channel", Value::Int(channel)),
        ];
        if let Some((file, _)) = &mxf_packages {
            fields.push(("saved_file_package", text(file)));
        }
        receipt(graph, &stream_id, fields);
        add_media(
            graph,
            &mut item,
            SourceMedia {
                id: stream_id,
                kind: StreamKind::Audio,
                locations: vec![path.clone()],
                codec: Some("pcm".into()),
                frame_size: None,
                rate: None,
                sample_rate: Some(48000),
                bits: Some(bits),
                duration,
                start: Some(seconds(start, rate)?),
                clock: None,
                stream: Some(label),
                online: None,
                original: None,
            },
        );
    }
    receipt(
        graph,
        id,
        vec![
            ("kind", text(if atom { "standalone_mono_atom" } else { "standalone_wav" })),
            ("cache_samples", Value::Int(samples)),
            ("audio_import_rate", text(&rate.to_string())),
        ],
    );
    let wav_stereo_pool_route = !item.stream_groups.is_empty();
    let native_stereo_pcm = !atom && wav_stereo_pool_route
        && clip.maybe("VirtualAudioTracksBA")?.is_none_or(str::is_empty);
    metadata::capture(clip, node, None, blobs, graph)?;
    graph.items.push(item);
    Ok(Some(Master {
        rate,
        length,
        start,
        audio: selectors,
        path,
        size: (0, 0),
        prores_hq: false,
        camera_pcm: false,
        native_mono_movie: false,
        wav_samples: Some(samples),
        standalone_mono_atom: atom,
        stereo_pool_route: wav_stereo_pool_route,
        native_stereo_pcm,
        split_atom_audio: false,
    }))
}

fn optional_int(map: &encoding::Map, key: &str) -> Result<i64> {
    if map.keys().any(|field| field == key) {
        map.int(key)
    } else {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{integer_atom_frame, integer_wav_frame};

    #[test]
    fn observed_binary64_residue_does_not_change_the_wav_edit() {
        assert_eq!(integer_wav_frame("48|000000000000003d").unwrap(), 48);
        assert_eq!(integer_wav_frame("7|000000000000d03c").unwrap(), 7);
        assert_eq!(integer_wav_frame("23|e0ffffffffffef3f").unwrap(), 24);
        let sample = hex::encode((1.0f64 / 1920.0).to_le_bytes());
        assert!(integer_wav_frame(&format!("7|{sample}")).is_err());
        let almost = hex::encode((1.0f64 - 1.0 / 2000.0).to_le_bytes());
        assert!(integer_wav_frame(&format!("23|{almost}")).is_err());
        assert!(integer_wav_frame("48|000000000000e03f").is_err());
        assert!(integer_wav_frame("48|000000000000f87f").is_err());
        assert!(integer_wav_frame("48|00").is_err());
    }

    #[test]
    fn nonzero_atom_clock_accepts_saved_float_residue_but_refuses_a_sample_offset() {
        assert_eq!(integer_atom_frame("24|00000000007c953d").unwrap(), 24);
        assert_eq!(integer_atom_frame("5|000000000076943d").unwrap(), 5);
        let sample = hex::encode((1.0f64 / 2002.0).to_le_bytes());
        assert!(integer_atom_frame(&format!("5|{sample}")).is_err());
    }
}
