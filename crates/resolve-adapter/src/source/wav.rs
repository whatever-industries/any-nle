//! Standalone audio caches and their saved physical channel routing.
use super::*;
use crate::linked::samples_per_frame;

#[derive(Debug)]
pub(super) struct Standalone {
    samples: i64,
    sample_rate: i64,
    start: Rational,
    grouped: bool,
}

/// A stereo A1 at 24 fps is observed for standalone WAV edits.
pub(super) fn all_stereo_wav_edits(
    track: &Node,
    masters: &HashMap<&str, Master>,
    rate: Rational,
) -> Result<bool> {
    let items = track.one("Items")?;
    if items.children.is_empty() {
        return Ok(false);
    }
    for item in items.elements()? {
        let clip = item.entry()?;
        if clip.name != "Sm2TiAudioClip"
            || !clip
                .maybe("MediaRef")?
                .and_then(|id| masters.get(id))
                .is_some_and(|master| {
                    master.native_stereo_pcm && master.wav_samples.is_some() && master.rate == rate
                })
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn whole_frame(frame: &timing::FrameField, mono_atom: bool) -> Option<i64> {
    let timing::FrameField { whole, fraction } = *frame;
    // Resolve sometimes serializes binary64 noise beside an integral frame
    // count. Keep true fractional edits outside this frame-aligned profile.
    // A nonzero MXF origin adds binary64 cancellation at the file clock scale.
    let tolerance = if mono_atom {
        1e-9
    } else {
        (whole.unsigned_abs() as f64 * f64::EPSILON).min(1e-9)
    };
    if fraction.abs() <= tolerance {
        return Some(whole);
    }
    // Native WAV durations can also save the frame immediately below the
    // boundary, with a positive remainder that lands on the next frame.
    if !mono_atom && (fraction - 1.0).abs() <= tolerance {
        return whole.checked_add(1);
    }
    None
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

#[allow(clippy::too_many_arguments)]
pub(super) fn edit(
    master: &Master,
    node: &Node,
    grid: Grid,
    track: i64,
    stereo: bool,
    owner: &str,
    graph: &mut SourceGraph,
    blobs: &mut Blobs,
) -> Result<Option<Placement>> {
    let saved = master.standalone_audio.as_ref().ok_or("missing standalone audio profile")?;
    let encoded = hex_bytes(node.get("MediaTimemapBA")?)?;
    let identity = match encoded.as_slice() {
        [2, endpoint @ ..] if endpoint.len() == 8 => {
            let end = saved.samples as f64 / saved.sample_rate as f64
                - master.rate.den() as f64 / master.rate.num() as f64;
            close(f64::from_be_bytes(endpoint.try_into()?), end)
        }
        _ => false,
    };
    if !identity || master.rate != grid.rate
        || encoding::rate(&hex_bytes(node.get("MediaFrameRate")?)?)? != master.rate
    {
        record_unqualified_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
        return Ok(None);
    }
    let Some(edit) = grid.audio_edit(node, true, false)? else {
        record_unqualified_timing(graph, node, owner, master.rate, grid.rate, blobs)?;
        return Ok(None);
    };
    if edit.source_in.checked_add(edit.length).is_none_or(|end| end > master.length)
        || !timing::clock_matches(node.get("MediaStartTime")?, saved.start, Rational::integer(saved.sample_rate))?
    {
        return Err("Resolve standalone edit contradicts the saved audio start or extent".into());
    }
    report_locator(master, node, owner, graph)?;
    let media_track = node.get("MediaTrackIdx")?;
    let audio = if stereo && saved.grouped {
        if media_track != "0" || node.get("VirtualAudioTrackBA")? != crate::stereo::SELECTOR {
            omitted(graph, owner, node.id()?, "audio_routing", "unqualified Resolve standalone stereo selection");
            return Ok(None);
        }
        None
    } else if !stereo {
        let channel = if saved.grouped {
            stereo_channel(node, blobs)?
        } else {
            mono(node, blobs, false, false)?.and_then(|selector| master.audio.get(&selector).copied())
        };
        let Some(channel) = channel else {
            omitted(graph, owner, node.id()?, "audio_routing", "unqualified Resolve standalone mono selection");
            return Ok(None);
        };
        Some(channel + 1)
    } else {
        omitted(graph, owner, node.id()?, "audio_routing", "standalone audio has no saved stereo group");
        return Ok(None);
    };
    let id = node.id()?;
    let mut out = placement(id, node.get("MediaRef")?, grid, edit, StreamKind::Audio, track, audio)?;
    if stereo {
        out.audio_tracks.clear();
        let mut group = crate::stereo::group();
        group.track = track;
        out.grouped_audio.push(group);
    }
    if node.maybe("EffectFiltersBA")?.is_some_and(|body| !body.is_empty()) {
        omitted(graph, owner, id, "effect", "Resolve standalone audio effect controls are unqualified");
    }
    Ok(Some(out))
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
    let suffix = suffix.map(str::to_ascii_lowercase);
    let graph_codec = match (codec.as_str(), suffix.as_deref()) {
        ("Linear PCM", Some("wav" | "aif" | "aiff" | "mxf")) => "pcm",
        ("MP3", Some("mp3")) => "mp3",
        ("AAC", Some("m4a")) => "aac",
        _ => return Ok(None),
    };
    let tracks = blobs.qt(node.get("TracksBA")?)?;
    if !tracks.keys().eq(["0"]) {
        return Ok(None);
    }
    let track = qt(tracks.bytes("0")?)?;
    let channels = track.int("NumChannels")?;
    let samples = track.int("Duration")?;
    let sample_rate = track.int("SampleRate")?;
    let depth = track.int("BitDepth")?;
    let bits = match (graph_codec, depth) {
        ("pcm", 1) => Some(16),
        ("pcm", 2) => Some(24),
        // Depth code 3 does not identify the sample representation on its own.
        (_, 3) if !atom => None,
        _ => return Ok(None),
    };
    if !matches!(channels, 1 | 2)
        || !matches!(sample_rate, 44100 | 48000 | 96000)
        || atom && sample_rate != 48000
        || track.int("IdxTrack")? != 0
        || samples <= 0
    {
        return Ok(None);
    }
    if track.text("CodecName")? != Some(codec.as_str()) {
        return Err("standalone audio cache codecs disagree".into());
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
    let frame_count = i128::from(samples) * i128::from(rate.num())
        / (i128::from(sample_rate) * i128::from(rate.den()));
    let length = i64::try_from(frame_count)?;
    let start_seconds = track.real("StartTime")?;
    let start = if atom { frames(start_seconds, rate)? } else { 0 };
    let source_start = if atom { seconds(start, rate)? } else { encoding::sample_start(start_seconds, sample_rate)? };
    let qualified_wav = suffix.as_deref() == Some("wav") && source_start == Rational::ZERO
        && sample_rate == 48000 && bits.is_some() && samples_per_frame(rate).is_ok();
    let mxf_packages = if atom {
        if rate != crate::prepare::rate() || channels != 1 || samples % samples_per_frame(rate)? != 0
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
    let duration = Rational::from_units(samples, sample_rate).ok_or("invalid standalone audio duration")?;
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
        let start_matches = close(route_start, start_seconds);
        let route_depth = route.keys().any(|key| key == "BitDepth").then(|| route.int("BitDepth")).transpose()?;
        let depth_matches = route_depth == Some(depth)
            || bits.is_none() && route_depth.is_none();
        if extent.len() != 16
            || !start_matches
            || !close(f64::from_le_bytes(*length), samples as f64 / sample_rate as f64)
            || !depth_matches
            || route.text("CodecName")? != Some(codec.as_str())
        {
            return Err("standalone virtual audio extent or format contradicts cache".into());
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
                codec: Some(graph_codec.into()),
                frame_size: None,
                rate: None,
                sample_rate: Some(sample_rate),
                bits,
                duration,
                start: Some(source_start),
                clock: None,
                stream: Some(label),
                online: None,
                original: None,
            },
        );
    }
    let kind = if atom { "standalone_mono_atom" } else if qualified_wav { "standalone_wav" } else { "standalone_audio" };
    let mut saved = vec![
        ("kind", text(kind)),
        ("cache_samples", Value::Int(samples)),
        ("audio_import_rate", text(&rate.to_string())),
    ];
    if !atom && !qualified_wav {
        saved.push(("saved_bit_depth_code", Value::Int(depth)));
    }
    receipt(graph, id, saved);
    let wav_stereo_pool_route = !item.stream_groups.is_empty();
    let native_stereo_pcm = qualified_wav && wav_stereo_pool_route
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
        saved_aac_clock: false,
        external_audio: false,
        wav_samples: (atom || qualified_wav).then_some(samples),
        standalone_audio: (!atom).then_some(Standalone {
            samples, sample_rate, start: source_start, grouped: wav_stereo_pool_route,
        }),
        still: false,
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
    use super::*;

    fn integer_frame(text: &str, mono_atom: bool) -> Result<i64> {
        whole_frame(&timing::FrameField::parse(text)?, mono_atom).ok_or_else(|| "not whole frame".into())
    }

    fn integer_wav_frame(text: &str) -> Result<i64> {
        integer_frame(text, false)
    }

    fn integer_atom_frame(text: &str) -> Result<i64> {
        integer_frame(text, true)
    }

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
