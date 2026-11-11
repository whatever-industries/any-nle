//! Byte-preserving PCM24 WAV preparation for saved Gain renders and dry audio.
use super::backend::probe;
use super::receipts::{obj, text};
use super::source::{essence_spans, stamp, Stamp};
use super::R;
use graph::{json::Value, source::SourceGraph};
use prproj::convert::{
    model::{Clip, TICKS_PER_SECOND},
    prepared::{ClipMetadata, OriginalAudio, OriginalChannel, OriginalClip, SampleRepresentation},
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const GAIN_OP: &str = "060e2b34040101020d01020110000000";
const RATE: i64 = 48_000;
const ALIGN: u32 = 3;
const PCM_KEY: [u8; 16] = [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1, 22, 1, 2, 1];

pub(crate) type GainPaths<'a> = HashMap<&'a str, (&'a str, &'a str)>;

pub(crate) fn gain_paths(graph: &SourceGraph) -> GainPaths<'_> {
    let origins: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.audiosuite_gain")
        .filter_map(|s| match s.data.get("origin_package") {
            Some(Value::Str(origin)) => Some((s.object.as_str(), origin.as_str())),
            _ => None,
        })
        .collect();
    if origins.is_empty() {
        return HashMap::new();
    }
    let files: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.source" && origins.contains_key(s.object.as_str()))
        .filter_map(|s| match s.data.get("file_package") {
            Some(Value::Str(file)) => Some((s.object.as_str(), file.as_str())),
            _ => None,
        })
        .collect();
    graph
        .media
        .iter()
        .filter_map(|m| {
            Some((
                m.locations.first()?.as_str(),
                (*files.get(m.id.as_str())?, *origins.get(m.id.as_str())?),
            ))
        })
        .collect()
}

#[derive(PartialEq)]
pub(super) struct MonoPcm {
    pub(super) source: String,
    path: PathBuf,
    stamp: Stamp,
    offset: u64,
    bytes: u32,
    gain: bool,
    hash: [u8; 32],
    material: String,
    file: String,
}

fn hash(reader: impl Read) -> R<[u8; 32]> {
    prproj::hash::sha256_reader(reader).map_err(|e| e.to_string())
}

fn audio_clip(clip: &Clip) -> R<&prproj::convert::model::AudioFile> {
    match clip.audio.as_slice() {
        [a] if clip.video.is_none()
            && clip.stereo_pairs.is_empty()
            && clip.multichannel_groups.is_empty()
            && a.avid_track == Some(1)
            && a.sample_rate == RATE
            && a.bits == 24 =>
        {
            Ok(a)
        }
        _ => Err(
            "Gain preparation requires discrete mono A1, 48 kHz PCM24 audio-only sources".into(),
        ),
    }
}

impl MonoPcm {
    pub(super) fn plan(clip: &Clip, render: Option<(&str, &str)>, ffprobe: &Path) -> R<Self> {
        let a = audio_clip(clip)?;
        let gain = render.is_some();
        let sample_unit = TICKS_PER_SECOND / RATE;
        let samples = a.duration / sample_unit;
        if samples <= 0
            || a.duration % sample_unit != 0
            || (gain && samples != RATE && samples != 2 * RATE)
        {
            return Err("unqualified mono PCM24 playable span".into());
        }
        let bytes = u32::try_from(
            samples
                .checked_mul(i64::from(ALIGN))
                .ok_or("PCM length overflow")?,
        )
        .ok()
        .filter(|n| *n <= u32::MAX - 37)
        .ok_or("PCM length exceeds RIFF WAV limits")?;
        let path = fs::canonicalize(&a.path).map_err(|e| e.to_string())?;
        let before = stamp(&path)?;
        let id = avid_reader::mxf::read_identity(&path).map_err(|e| e.to_string())?;
        let (Some(material), [file]) = (clip.material_umid.as_ref(), id.file_packages.as_slice())
        else {
            return Err("mono PCM requires exact MXF package identity".into());
        };
        if id.material_packages.len() != 1
            || id.material_packages.first() != Some(material)
            || id.file_tracks != [(RATE, 1, samples)]
            || render.is_some_and(|(expected_file, origin)| {
                a.start != 0
                    || !id.timecodes.is_empty()
                    || file != expected_file
                    || !id.source_packages.iter().any(|p| p == origin)
            })
        {
            return Err(
                "mono PCM MXF package identity or playable sample clock disagrees with the model"
                    .into(),
            );
        }
        let p = probe(ffprobe, &path)?;
        let [stream] = p.streams.as_slice() else {
            return Err("mono PCM MXF needs one essence stream".into());
        };
        if p.op != GAIN_OP
            || !p.references.is_empty()
            || stream.kind != "audio"
            || stream.codec != "pcm_s24le"
            || stream.channels != 1
            || stream.samples != RATE
            || stream.bits != 24
            || stream.ticks != a.duration
        {
            return Err("mono PCM MXF stream is outside the qualified Avid profile".into());
        }
        let spans = essence_spans(&path)?;
        let [span] = spans.as_slice() else {
            return Err("mono PCM MXF needs one sound KLV".into());
        };
        if span.key != PCM_KEY || span.bytes != u64::from(bytes) + if gain { 6 } else { 0 } {
            return Err(
                "mono PCM MXF physical span disagrees with the playable sample count".into(),
            );
        }
        let mut pcm = Self {
            source: a.path.clone(),
            path,
            stamp: before,
            offset: span.offset,
            bytes,
            gain,
            hash: [0; 32],
            material: material.clone(),
            file: file.clone(),
        };
        pcm.hash = pcm.source_hash()?;
        Ok(pcm)
    }

    fn input(&self) -> R<BufReader<File>> {
        if stamp(&self.path)? != self.stamp {
            return Err("mono PCM input changed since preflight".into());
        }
        let mut file = BufReader::new(File::open(&self.path).map_err(|e| e.to_string())?);
        file.seek(SeekFrom::Start(self.offset))
            .map_err(|e| e.to_string())?;
        Ok(file)
    }

    fn finish_input(&self, input: &mut impl Read) -> R<()> {
        if self.gain {
            let mut tail = [0; 6];
            input.read_exact(&mut tail).map_err(|e| e.to_string())?;
            if tail != [0; 6] {
                return Err("AudioSuite Gain render has nonzero terminal padding".into());
            }
        }
        if stamp(&self.path)? != self.stamp {
            return Err("mono PCM input changed while reading".into());
        }
        Ok(())
    }

    fn source_hash(&self) -> R<[u8; 32]> {
        let mut input = self.input()?;
        let mut payload = (&mut input).take(u64::from(self.bytes));
        let digest = hash(&mut payload)?;
        if payload.limit() != 0 {
            return Err("truncated mono PCM input".into());
        }
        self.finish_input(&mut input)?;
        Ok(digest)
    }

    pub(super) fn size(&self) -> u64 {
        u64::from(self.bytes) + 45
    }

    pub(super) fn write(&self, partial: &Path, output: &Path) -> R<Value> {
        if self.source_hash()? != self.hash {
            return Err("mono PCM payload changed after preflight".into());
        }
        let mut input = self.input()?;
        let header = wave_header(self.bytes);
        let result = (|| -> io::Result<()> {
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(partial)?;
            out.write_all(&header)?;
            let copied = io::copy(&mut (&mut input).take(u64::from(self.bytes)), &mut out)?;
            if copied != u64::from(self.bytes) {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            if self.bytes % 2 != 0 {
                out.write_all(&[0])?;
            }
            out.flush()
        })();
        result.map_err(|e| format!("{}: {e}", partial.display()))?;
        self.finish_input(&mut input)?;
        let mut verify = BufReader::new(File::open(partial).map_err(|e| e.to_string())?);
        let mut actual = [0; 44];
        verify.read_exact(&mut actual).map_err(|e| e.to_string())?;
        let mut payload = (&mut verify).take(u64::from(self.bytes));
        if actual != header || hash(&mut payload)? != self.hash || payload.limit() != 0 {
            return Err("mono WAV output failed exact PCM verification".into());
        }
        fs::hard_link(partial, output)
            .map_err(|e| format!("refusing to replace {}: {e}", output.display()))?;
        fs::remove_file(partial).map_err(|e| e.to_string())?;
        Ok(obj(vec![
            ("status", text("verified mono PCM24 WAV")),
            ("source", text(&self.source)),
            ("output", text(output.to_string_lossy())),
            ("material_package", text(&self.material)),
            ("file_package", text(&self.file)),
            (
                "playable_samples",
                Value::Int(i64::from(self.bytes / ALIGN)),
            ),
            (
                "excluded_zero_samples",
                Value::Int(if self.gain { 2 } else { 0 }),
            ),
            ("essence_sha256", text(prproj::hash::hex(&self.hash))),
        ]))
    }
}

fn wave_header(bytes: u32) -> [u8; 44] {
    let mut header =
        *b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\0\0\0\0\0\0\0\0\x03\0\x18\0data\0\0\0\0";
    header[4..8].copy_from_slice(&(bytes + 36 + bytes % 2).to_le_bytes());
    header[24..28].copy_from_slice(&(RATE as u32).to_le_bytes());
    header[28..32].copy_from_slice(&(RATE as u32 * ALIGN).to_le_bytes());
    header[40..44].copy_from_slice(&bytes.to_le_bytes());
    header
}

pub(super) fn writer_clip(clip: &Clip, output: &Path) -> R<OriginalClip> {
    let target = output.to_str().ok_or("mono WAV output path is not UTF-8")?;
    let a = audio_clip(clip)?;
    let label = a.avid_track.unwrap();
    Ok(OriginalClip {
        metadata: ClipMetadata {
            id: clip.id.clone(),
            name: clip.name.clone(),
            material_umid: clip.material_umid.clone(),
            tape_name: clip.tape_name.clone(),
            avid: clip.avid.clone(),
            time_display: clip.time_display,
            rate: clip.rate,
            markers: clip.markers.clone(),
        },
        path: target.into(),
        container: "wav".into(),
        video: None,
        audio: Some(OriginalAudio {
            sample_rate: RATE,
            channels: 1,
            duration: a.duration,
            stream_index: 0,
            representation: SampleRepresentation::Pcm24,
        }),
        channels: vec![OriginalChannel {
            avid_track: label,
            channel_index: 0,
            gain_db: a.gain_db,
        }],
        groups: vec![vec![label]],
    })
}
