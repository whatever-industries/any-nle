//! Standalone PCM cache and virtual routing, separate from embedded MXF audio.
use super::*;

pub(super) fn samples_per_frame(rate: Rational) -> Result<i64> {
    match (rate.num(), rate.den()) {
        (24, 1) => Ok(2000),
        (24000, 1001) => Ok(2002),
        _ => Err("unqualified standalone WAV frame basis".into()),
    }
}

pub(super) fn integer_duration(text: &str) -> Result<i64> {
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
    if fraction.abs() > whole.unsigned_abs() as f64 * f64::EPSILON || !fraction.is_finite() {
        return Err("unqualified Resolve fractional audio edit".into());
    }
    Ok(whole)
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
    if codec != "Linear PCM" || !path.to_ascii_lowercase().ends_with(".wav") {
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
        || track.real("StartTime")? != 0.0
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
    let length = samples / samples_per_frame(rate)?;
    let outer = pb_bytes(&fields, 1)?.ok_or("missing standalone WAV pool metadata")?;
    let routes = qt(pb_bytes(outer, 5)?.ok_or("missing standalone WAV virtual routes")?)?;

    let id = clip.id()?;
    let duration = Rational::new(samples.into(), 48000).ok_or("invalid WAV duration")?;
    let mut item = SourceItem {
        id: id.into(),
        name: clip.get("Name")?.into(),
        media: vec![],
        umid: None,
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
        let [start, length] = extent.as_chunks::<8>().0 else {
            return Err("invalid standalone WAV virtual extent".into());
        };
        if extent.len() != 16
            || f64::from_le_bytes(*start) != 0.0
            || !close(f64::from_le_bytes(*length), samples as f64 / 48000.0)
            || route.int("BitDepth")? != depth
            || route.text("CodecName")? != Some("Linear PCM")
        {
            return Err("standalone WAV virtual extent contradicts audio cache".into());
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
                return Err("standalone WAV virtual source contradicts audio cache".into());
            }
            let physical_channel = optional_int(&channel, "ChannelIdx")?;
            if !(0..channels).contains(&physical_channel)
                || physical.insert(physical_channel, index).is_some()
                || (grouped && key.parse::<i64>()? != physical_channel)
            {
                return Err("ambiguous standalone WAV physical channel".into());
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
        receipt(
            graph,
            &stream_id,
            vec![
                ("saved_selector", Value::Int(selector)),
                ("saved_cache_track", Value::Int(0)),
                ("saved_channel", Value::Int(channel)),
            ],
        );
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
                start: Some(Rational::ZERO),
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
            ("kind", text("standalone_wav")),
            ("cache_samples", Value::Int(samples)),
            ("audio_import_rate", text(&rate.to_string())),
        ],
    );
    let wav_stereo_pool_route = !item.stream_groups.is_empty();
    graph.items.push(item);
    Ok(Some(Master {
        rate,
        length,
        start: 0,
        audio: selectors,
        path,
        size: (0, 0),
        wav_samples: Some(samples),
        stereo_pool_route: wav_stereo_pool_route,
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
    use super::integer_duration;

    #[test]
    fn observed_binary64_residue_does_not_change_the_wav_edit() {
        assert_eq!(integer_duration("48|000000000000003d").unwrap(), 48);
        assert!(integer_duration("48|000000000000e03f").is_err());
        assert!(integer_duration("48|000000000000f87f").is_err());
        assert!(integer_duration("48|00").is_err());
    }
}
