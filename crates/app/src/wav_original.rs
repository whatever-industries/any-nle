//! Verify the saved eight-mono Avid import against the original BWF.
use crate::rewrap::source;
use crate::source_input;
use avid_adapter::original::{probe_offline_atom, resolve, RootMapping};
use avid_adapter::ReadResult;
use graph::source::{OriginalStream, SourceGraph, SourceItem};
use graph::{Rational, StreamKind, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const CHANNELS: usize = 8;
const SAMPLE_RATE: i64 = 48_000;
const PCM_KEY: [u8; 16] = [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 22, 1, 2, 1];

fn integer(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v.get(key)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

pub(crate) fn select(
    read: &mut ReadResult,
    mappings: &[RootMapping],
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if !read
        .graph
        .sidecars
        .iter()
        .any(|s| s.adapter == "avid.audio_import")
    {
        return Ok(());
    }
    let items: HashMap<_, _> = read
        .graph
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let media: HashMap<_, _> = read
        .graph
        .media
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.as_str(), i))
        .collect();
    let unsupported: HashSet<_> = read
        .graph
        .sequences
        .iter()
        .flat_map(|s| {
            s.placements
                .iter()
                .filter(|p| {
                    s.rate != Rational::integer(24)
                        || p.speed.is_some()
                        || p.frame_map.is_some()
                        || !p.grouped_audio.is_empty()
                })
                .map(|p| p.item.as_str())
        })
        .collect();
    let mut claimed = HashSet::new();
    let mut selected = Vec::new();
    for receipt in read
        .graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.audio_import")
    {
        check()?;
        let Some(item) = items.get(receipt.object.as_str()).copied() else {
            continue;
        };
        if unsupported.contains(item.id.as_str())
            || !item.markers.is_empty()
            || !item.stream_groups.is_empty()
            || !read.graph.multicams.is_empty()
        {
            continue;
        }
        let Some(candidate) = candidate(&read.graph, item, &media, &receipt.data, mappings) else {
            continue;
        };
        if candidate
            .tracks
            .iter()
            .any(|track| claimed.contains(&track.index))
        {
            continue;
        }
        match verify(&candidate, ffprobe, check) {
            Ok(pin) => {
                claimed.extend(candidate.tracks.iter().map(|track| track.index));
                selected.push((candidate, pin));
            }
            Err(reason) => {
                check()?;
                read.diagnostics.push(format!(
                    "{}: original WAV could not be verified ({reason}). Using managed MXFs.",
                    item.name,
                ));
            }
        }
    }
    for (candidate, pin) in selected {
        let path = candidate
            .path
            .into_os_string()
            .into_string()
            .map_err(|_| "qualified WAV path is not UTF-8")?;
        for (lane, track) in candidate.tracks.into_iter().enumerate() {
            let media = &mut read.graph.media[track.index];
            media.locations = vec![path.clone()];
            media.codec = Some("pcm_s16le".into());
            media.original = Some(OriginalStream {
                saved_locators: vec![candidate.saved.clone()],
                container: "wav".into(),
                stream_index: 0,
                time_base: Rational::new(1, SAMPLE_RATE.into()).unwrap(),
                start_pts: 0,
                duration_ts: candidate.samples,
                channels: Some(CHANNELS as i64),
                channel_index: Some(lane as i64),
                sample_representation: Some("pcm16".into()),
            });
        }
        read.original_inputs.push(pin);
    }
    Ok(())
}

struct ManagedTrack {
    index: usize,
    path: PathBuf,
    package: String,
}

struct Candidate {
    master: String,
    saved: String,
    path: PathBuf,
    samples: i64,
    timestamp: u64,
    tracks: [ManagedTrack; CHANNELS],
}
fn candidate(
    graph: &SourceGraph,
    item: &SourceItem,
    media: &HashMap<&str, usize>,
    row: &Value,
    mappings: &[RootMapping],
) -> Option<Candidate> {
    let Value::Arr(locators) = row.get("saved_locators")? else {
        return None;
    };
    let [Value::Str(saved)] = locators.as_slice() else {
        return None;
    };
    let path = resolve(std::slice::from_ref(saved), mappings).ok()?;
    path.to_str()?;
    let start = integer(row, "source_timecode_frame")?;
    let samples = integer(row, "frames")?.checked_mul(2000)?;
    // RIFF and the initial 24p qualification admit only a complete PCM16 payload.
    if samples <= 0
        || samples.checked_mul(16)? > i64::from(u32::MAX)
        || item.media.len() != CHANNELS
    {
        return None;
    }
    let Value::Arr(lanes) = row.get("lanes")? else {
        return None;
    };
    if lanes.len() != CHANNELS {
        return None;
    }
    let physical = text(row, "physical_package")?;
    let mut tracks = Vec::with_capacity(CHANNELS);
    let mut paths = HashSet::new();
    for (offset, lane) in lanes.iter().enumerate() {
        let label = offset as i64 + 1;
        if integer(lane, "master_track")? != label || integer(lane, "physical_track")? != label {
            return None;
        }
        let mut matches = item.media.iter().filter_map(|binding| {
            let index = *media.get(binding.media.as_str())?;
            (graph.media[index].stream == Some(label)).then_some((binding, index))
        });
        let (binding, index) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let m = &graph.media[index];
        let [mxf] = m.locations.as_slice() else {
            return None;
        };
        let clock = m.clock.as_ref()?;
        if m.kind != StreamKind::Audio
            || m.online != Some(true)
            || m.original.is_some()
            || m.sample_rate != Some(SAMPLE_RATE)
            || m.bits != Some(16)
            || m.duration.to_units(SAMPLE_RATE) != Some(samples)
            || binding.source_offset != Some(Rational::ZERO)
            || clock.rate != Rational::integer(24)
            || clock.display_base != 24
            || clock.drop
            || clock.provenance.source_offset != 0
            || clock.provenance.source_package != physical
            || clock.effective_count != u64::try_from(start).ok()?
            || m.start != clock.exact_start()
            || !paths.insert(mxf)
        {
            return None;
        }
        tracks.push(ManagedTrack {
            index,
            path: PathBuf::from(mxf),
            package: text(lane, "file_package")?.to_owned(),
        });
    }
    Some(Candidate {
        master: item.umid.clone()?,
        saved: saved.clone(),
        path,
        samples,
        timestamp: u64::try_from(start.checked_mul(2000)?).ok()?,
        tracks: tracks.try_into().ok()?,
    })
}

fn verify(
    candidate: &Candidate,
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<avid_reader::source::SourceInput, String> {
    let mut pins = source_input::pin_checked(std::slice::from_ref(&candidate.path), check)?;
    let mut wav = File::open(&candidate.path).map_err(|e| e.to_string())?;
    let samples = u64::try_from(candidate.samples).map_err(|e| e.to_string())?;
    let offset =
        read_bwf_audio(&mut wav, samples, candidate.timestamp).map_err(|e| e.to_string())?;
    wav.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut originals = Vec::with_capacity(CHANNELS);
    let mut stamps = Vec::with_capacity(CHANNELS);
    for track in &candidate.tracks {
        check()?;
        let path = &track.path;
        stamps.push(source::stamp(path)?);
        let id = avid_reader::mxf::read_identity(path).map_err(|err| err.to_string())?;
        if id.file_packages.as_slice() != [track.package.as_str()]
            || id.material_packages.as_slice() != [candidate.master.as_str()]
            || id.file_tracks != [(SAMPLE_RATE, 1, candidate.samples)]
            || id.file_essence_tracks != 1
        {
            return Err("managed MXF identity differs from the saved route".into());
        }
        let info = probe_offline_atom(path, ffprobe)?;
        verify_mono_stream(&info, candidate.samples)?;
        let spans = source::essence_spans(path)?;
        let [span] = spans.as_slice() else {
            return Err("managed MXF has ambiguous essence".into());
        };
        if span.key != PCM_KEY || span.bytes != samples * 2 {
            return Err("managed MXF sample extent differs".into());
        }
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(span.offset))
            .map_err(|e| e.to_string())?;
        originals.push(file);
    }
    let mut interleaved = [0u8; 16 * 2048];
    let mut mono = [0u8; 2 * 2048];
    let mut remaining = samples;
    while remaining > 0 {
        check()?;
        let frames = remaining.min(2048) as usize;
        wav.read_exact(&mut interleaved[..frames * 16])
            .map_err(|e| e.to_string())?;
        for (channel, mxf) in originals.iter_mut().enumerate() {
            mxf.read_exact(&mut mono[..frames * 2])
                .map_err(|e| e.to_string())?;
            if interleaved[..frames * 16]
                .chunks_exact(16)
                .zip(mono[..frames * 2].chunks_exact(2))
                .any(|(a, b)| a[channel * 2..channel * 2 + 2] != *b)
            {
                return Err(format!(
                    "original WAV channel {} differs from managed MXF PCM",
                    channel + 1
                ));
            }
        }
        remaining -= frames as u64;
    }
    for (track, before) in candidate.tracks.iter().zip(stamps) {
        if source::stamp(&track.path)? != before {
            return Err("managed MXF changed during comparison".into());
        }
    }
    source_input::recheck_checked(&pins, check)?;
    Ok(pins.pop().unwrap())
}

fn verify_mono_stream(info: &Value, samples: i64) -> Result<(), String> {
    let op = info
        .get("format")
        .and_then(|f| f.get("tags"))
        .and_then(|tags| text(tags, "operational_pattern_ul"));
    if op != Some("060e2b34.04010102.0d010201.10020000") {
        return Err("managed audio is outside the qualified OP-Atom profile".into());
    }
    let Some(Value::Arr(streams)) = info.get("streams") else {
        return Err("missing MXF stream facts".into());
    };
    let mut mono = None;
    for stream in streams {
        match text(stream, "codec_type") {
            Some("audio") if mono.is_none() => mono = Some(stream),
            // Avid writes placeholders for siblings in the material package.
            // The independent single PCM KLV check rules out ancillary essence.
            Some("data") if stream.get("codec_name").is_none() => {}
            _ => return Err("managed MXF contains unexpected streams".into()),
        }
    }
    let mono = mono.ok_or("managed MXF has no PCM stream")?;
    if text(mono, "codec_name") != Some("pcm_s16le")
        || integer(mono, "channels") != Some(1)
        || integer(mono, "bits_per_sample") != Some(16)
        || text(mono, "sample_rate") != Some("48000")
        || text(mono, "time_base") != Some("1/48000")
        || integer(mono, "duration_ts") != Some(samples)
        || integer(mono, "start_pts") != Some(0)
    {
        return Err("managed MXF is outside the PCM16 mono profile".into());
    }
    Ok(())
}

fn read_bwf_audio(file: &mut File, samples: u64, timestamp: u64) -> std::io::Result<u64> {
    let bytes = file.metadata()?.len();
    bwf_audio(file, bytes, samples, timestamp)
}

fn bwf_audio(
    file: &mut (impl Read + Seek),
    bytes: u64,
    samples: u64,
    timestamp: u64,
) -> std::io::Result<u64> {
    let invalid = |msg| std::io::Error::new(std::io::ErrorKind::InvalidData, msg);
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;
    let end = u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap())) + 8;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" || end != bytes {
        return Err(invalid("expected one complete RIFF/WAVE container"));
    }
    let mut format = false;
    let mut broadcast = false;
    let mut audio = None;
    // Header traversal is bounded even when arbitrary unknown chunks are present.
    for _ in 0..128 {
        if file.stream_position()? == end {
            return if format && broadcast {
                audio.ok_or_else(|| invalid("missing BWF audio"))
            } else {
                Err(invalid("missing qualified BWF format or origin"))
            };
        }
        let mut h = [0u8; 8];
        file.read_exact(&mut h)?;
        let pos = file.stream_position()?;
        let len = u64::from(u32::from_le_bytes(h[4..].try_into().unwrap()));
        let next = pos + len + (len & 1);
        if next > end {
            return Err(invalid("RIFF chunk exceeds file bounds"));
        }
        match &h[..4] {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                if format || len != 16 {
                    return Err(invalid("unsupported or duplicate WAV format"));
                }
                file.read_exact(&mut fmt)?;
                if fmt != [1, 0, 8, 0, 128, 187, 0, 0, 0, 184, 11, 0, 16, 0, 16, 0] {
                    return Err(invalid("expected eight 48 kHz PCM16 channels"));
                }
                format = true;
            }
            b"bext" => {
                if broadcast || len < 346 {
                    return Err(invalid("invalid or duplicate BWF origin"));
                }
                file.seek(SeekFrom::Start(pos + 338))?;
                let mut raw = [0u8; 8];
                file.read_exact(&mut raw)?;
                if u64::from_le_bytes(raw) != timestamp {
                    return Err(invalid("BWF origin differs from saved Avid clock"));
                }
                broadcast = true;
            }
            b"data" => {
                if audio.is_some() || len != samples * 16 {
                    return Err(invalid("WAV sample extent differs"));
                }
                audio = Some(pos);
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(next))?;
    }
    Err(invalid("RIFF chunk limit exceeded"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn accepts_equal_extent_only_with_saved_bwf_origin() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&[1, 0, 8, 0, 128, 187, 0, 0, 0, 184, 11, 0, 16, 0, 16, 0]);
        wav.extend_from_slice(b"bext");
        wav.extend_from_slice(&346u32.to_le_bytes());
        wav.extend_from_slice(&[0; 338]);
        wav.extend_from_slice(&12_000u64.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&16u32.to_le_bytes());
        let offset = wav.len() as u64;
        wav.extend_from_slice(&[0; 16]);
        let bytes = wav.len() as u64;
        wav[4..8].copy_from_slice(&((bytes - 8) as u32).to_le_bytes());
        let parse =
            |length, samples, clock| bwf_audio(&mut Cursor::new(&wav), length, samples, clock);
        assert_eq!(parse(bytes, 1, 12_000).unwrap(), offset);
        assert!(parse(bytes, 1, 12_001)
            .unwrap_err()
            .to_string()
            .contains("origin"));
        assert!(parse(bytes, 2, 12_000)
            .unwrap_err()
            .to_string()
            .contains("extent"));
        assert!(parse(bytes - 1, 1, 12_000).is_err());
    }
}
