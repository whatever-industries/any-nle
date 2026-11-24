//! Byte-preserving mono WAV preparation for saved Gain renders and wide rewraps.
use super::backend::probe;
use super::planning::PrepareIssue;
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
const BEXT_BYTES: u32 = 602;
const BEXT_CHUNK_BYTES: u32 = 8 + BEXT_BYTES;
const BEXT_TIME_REFERENCE_OFFSET: usize = 338;
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
    bits: u16,
    time_reference: i64,
    gain: bool,
    hash: [u8; 32],
    material: String,
    file: String,
}

fn hash(reader: impl Read) -> R<[u8; 32]> {
    prproj::hash::sha256_reader(reader).map_err(|e| e.to_string())
}

pub(super) fn audio_clip(clip: &Clip) -> R<&prproj::convert::model::AudioFile> {
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
    pub(super) fn input_path(&self) -> &Path {
        &self.path
    }

    pub(super) fn plan(
        clip: &Clip,
        render: Option<(&str, &str)>,
        ffprobe: &Path,
    ) -> Result<Self, PrepareIssue> {
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
        let file_issue = PrepareIssue::source_file(&a.path);
        let path = fs::canonicalize(&a.path).map_err(|e| file_issue(e.to_string()))?;
        let before = stamp(&path).map_err(&file_issue)?;
        let id = avid_reader::mxf::read_identity(&path).map_err(|e| file_issue(e.to_string()))?;
        let (Some(material), [file]) = (clip.material_umid.as_ref(), id.file_packages.as_slice())
        else {
            return Err(file_issue("mono PCM requires exact MXF package identity".into()));
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
            return Err(file_issue(
                "mono PCM MXF package identity or playable sample clock disagrees with the model".into(),
            ));
        }
        let p = probe(ffprobe, &path).map_err(&file_issue)?;
        let [stream] = p.streams.as_slice() else {
            return Err(file_issue("mono PCM MXF needs one essence stream".into()));
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
            return Err(file_issue("mono PCM MXF stream is outside the qualified Avid profile".into()));
        }
        let spans = essence_spans(&path).map_err(&file_issue)?;
        let [span] = spans.as_slice() else {
            return Err(file_issue("mono PCM MXF needs one sound KLV".into()));
        };
        if span.key != PCM_KEY || span.bytes != u64::from(bytes) + if gain { 6 } else { 0 } {
            return Err(file_issue(
                "mono PCM MXF physical span disagrees with the playable sample count".into(),
            ));
        }
        let mut pcm = Self {
            source: a.path.clone(),
            path,
            stamp: before,
            offset: span.offset,
            bytes,
            bits: 24,
            time_reference: 0,
            gain,
            hash: [0; 32],
            material: material.clone(),
            file: file.clone(),
        };
        pcm.hash = pcm.source_hash().map_err(file_issue)?;
        Ok(pcm)
    }

    pub(super) fn plan_wide_sibling(
        input: &super::Input,
        start: i64,
        saved_path: &str,
    ) -> Result<Self, PrepareIssue> {
        let file_issue = PrepareIssue::source_file(saved_path);
        let stream = &input.probe.streams[0];
        let unit = TICKS_PER_SECOND / RATE;
        let samples = stream.ticks / unit;
        let (bits, bytes_per_sample) = match (stream.codec.as_str(), stream.bits) {
            ("pcm_s16le", 16) => (16, 2),
            ("pcm_s24le", 24) => (24, 3),
            _ => {
                return Err(file_issue(
                    "wide rewrap requires independent mono 48 kHz PCM16 or PCM24 MXFs".into(),
                ));
            }
        };
        if stream.kind != "audio"
            || stream.samples != RATE || stream.channels != 1 || !input.probe.references.is_empty()
            || samples <= 0 || stream.ticks % unit != 0
        {
            return Err(file_issue(
                "wide rewrap requires independent mono 48 kHz PCM16 or PCM24 MXFs".into(),
            ));
        }
        let time_reference = bwf_time_reference(start)?;
        let overhead = u32::from(time_reference != 0) * BEXT_CHUNK_BYTES;
        let bytes = samples.checked_mul(bytes_per_sample).ok_or("PCM length overflow")?;
        let bytes = u32::try_from(bytes)
            .ok()
            .filter(|n| *n <= u32::MAX - 37 - overhead)
            .ok_or("PCM length exceeds RIFF WAV limits")?;
        let id = avid_reader::mxf::read_identity(&input.path).map_err(|e| file_issue(e.to_string()))?;
        let ([material], [file]) = (id.material_packages.as_slice(), id.file_packages.as_slice())
        else { return Err(file_issue("wide audio needs one material and file package per sibling".into())); };
        let spans = essence_spans(&input.path).map_err(&file_issue)?;
        let [span] = spans.as_slice() else { return Err(file_issue("wide audio needs one sound KLV".into())); };
        if !id.file_tracks.contains(&(RATE, 1, samples))
            || id.file_tracks.iter().any(|&(num, den, length)| {
                num <= 0 || den <= 0 || length <= 0
                    || (i128::from(length) * i128::from(den)).checked_mul(i128::from(RATE))
                        != Some(i128::from(samples) * i128::from(num))
            })
            || span.kind != 0x16 || span.bytes != u64::from(bytes)
        {
            return Err(file_issue("wide audio sample extent disagrees with its physical PCM payload".into()));
        }
        let mut pcm = Self {
            source: input.aliases[0].clone(),
            path: input.path.clone(),
            stamp: input.stamp.clone(),
            offset: span.offset,
            bytes,
            bits,
            time_reference,
            gain: false,
            hash: [0; 32],
            material: material.clone(),
            file: file.clone(),
        };
        pcm.hash = pcm.source_hash().map_err(file_issue)?;
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
        u64::from(self.bytes) + 45 + u64::from(self.time_reference != 0) * u64::from(BEXT_CHUNK_BYTES)
    }

    pub(super) fn write(&self, partial: &Path, output: &Path) -> R<Value> {
        if self.source_hash()? != self.hash {
            return Err("mono PCM payload changed after preflight".into());
        }
        let mut input = self.input()?;
        let header = wave_header(self.bytes, self.bits, self.time_reference);
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
        let mut actual = vec![0; header.len()];
        verify.read_exact(&mut actual).map_err(|e| e.to_string())?;
        let mut payload = (&mut verify).take(u64::from(self.bytes));
        if actual != header || hash(&mut payload)? != self.hash || payload.limit() != 0 {
            return Err("mono WAV output failed exact PCM verification".into());
        }
        fs::hard_link(partial, output)
            .map_err(|e| format!("refusing to replace {}: {e}", output.display()))?;
        fs::remove_file(partial).map_err(|e| e.to_string())?;
        let mut receipt = vec![
            ("status", text(format!("verified mono PCM{} WAV", self.bits))),
            ("source", text(&self.source)),
            ("output", text(output.to_string_lossy())),
            ("material_package", text(&self.material)),
            ("file_package", text(&self.file)),
            (
                "playable_samples",
                Value::Int(i64::from(self.bytes / u32::from(self.bits / 8))),
            ),
            (
                "excluded_zero_samples",
                Value::Int(if self.gain { 2 } else { 0 }),
            ),
            ("essence_sha256", text(prproj::hash::hex(&self.hash))),
        ];
        if self.time_reference != 0 {
            receipt.push(("bwf_time_reference_samples", Value::Int(self.time_reference)));
        }
        Ok(obj(receipt))
    }
}

fn bwf_time_reference(ticks: i64) -> R<i64> {
    let unit = TICKS_PER_SECOND / RATE;
    if ticks < 0 || ticks % unit != 0 {
        return Err("wide rewrap source clock falls outside the exact 48 kHz WAV sample grid".into());
    }
    Ok(ticks / unit)
}

fn wave_header(bytes: u32, bits: u16, time_reference: i64) -> Vec<u8> {
    let mut header =
        *b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\0\0\0\0\0\0\0\0\x03\0\x18\0data\0\0\0\0";
    let overhead = u32::from(time_reference != 0) * BEXT_CHUNK_BYTES;
    header[4..8].copy_from_slice(&(bytes + 36 + bytes % 2 + overhead).to_le_bytes());
    header[24..28].copy_from_slice(&(RATE as u32).to_le_bytes());
    let align = bits / 8;
    header[28..32].copy_from_slice(&(RATE as u32 * u32::from(align)).to_le_bytes());
    header[32..34].copy_from_slice(&align.to_le_bytes());
    header[34..36].copy_from_slice(&bits.to_le_bytes());
    header[40..44].copy_from_slice(&bytes.to_le_bytes());
    if time_reference == 0 {
        return header.to_vec();
    }
    // EBU Tech 3285 version 0 stores the clock as a sample count since midnight.
    let mut bext = [0; BEXT_BYTES as usize];
    bext[BEXT_TIME_REFERENCE_OFFSET..BEXT_TIME_REFERENCE_OFFSET + 8]
        .copy_from_slice(&time_reference.to_le_bytes());
    let mut out = Vec::with_capacity(header.len() + BEXT_CHUNK_BYTES as usize);
    out.extend_from_slice(&header[..12]);
    out.extend_from_slice(b"bext");
    out.extend_from_slice(&BEXT_BYTES.to_le_bytes());
    out.extend_from_slice(&bext);
    out.extend_from_slice(&header[12..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_wav_stores_only_exact_source_sample_references() {
        let ticks = 945_743_904_000_000;
        let reference = bwf_time_reference(ticks).unwrap();
        assert_eq!(reference, 178_712_000);
        assert!(bwf_time_reference(ticks + 1).is_err());
        assert!(bwf_time_reference(-TICKS_PER_SECOND).is_err());

        let header = wave_header(3, 24, reference);
        assert_eq!(&header[..12], b"RIFF\x8a\x02\0\0WAVE");
        assert_eq!(&header[12..20], b"bext\x5a\x02\0\0");
        assert_eq!(&header[358..366], &reference.to_le_bytes());
        assert_eq!(&header[646..], b"data\x03\0\0\0");
        let ordinary = wave_header(4, 16, bwf_time_reference(0).unwrap());
        assert_eq!(ordinary.len(), 44);
        assert_eq!(&ordinary[36..], b"data\x04\0\0\0");
    }

    #[test]
    fn size_review_lists_each_published_media_recipe() {
        use super::super::{backend::Probe, Input, Job, Plan};
        use prproj::convert::model::Fraction;

        let stamp = Stamp {
            len: 4,
            modified: std::time::UNIX_EPOCH,
        };
        let path = |name: &str| PathBuf::from(name);
        let input = |name: &str| Input {
            path: path(name),
            aliases: vec![],
            stamp: stamp.clone(),
            probe: Probe {
                raw: Value::Null,
                streams: vec![],
                op: String::new(),
                tc: None,
                stream_indices: vec![],
                references: vec![],
            },
            identity: Value::Null,
            terminal: None,
        };
        let pcm = |name: &str, gain| MonoPcm {
            source: name.into(),
            path: path(name),
            stamp: stamp.clone(),
            offset: 0,
            bytes: 0,
            bits: 24,
            time_reference: 0,
            gain,
            hash: [0; 32],
            material: String::new(),
            file: String::new(),
        };
        let job = |inputs, wide_audio: Vec<MonoPcm>| Job {
            inputs,
            clips: vec![],
            tc: None,
            rate: Fraction::new(24, 1),
            mov: false,
            pixel_aspect: None,
            separate_mono: !wide_audio.is_empty(),
            wide_audio,
        };
        let plan = Plan {
            destination: path("Generated"),
            inputs: vec![input("picture.mxf"), input("A1.mxf"), input("A2.mxf")],
            jobs: vec![
                job(vec![0, 1], vec![]),
                job(vec![0, 1, 2], vec![pcm("A1.mxf", false), pcm("A2.mxf", false)]),
            ],
            mono: vec![pcm("gain.mxf", true)],
            ffmpeg: PathBuf::new(),
            ffprobe: PathBuf::new(),
            versions: vec![],
            estimate: 0,
            free: 0,
        };
        assert_eq!(plan.estimate_sources(), [
            vec![path("picture.mxf"), path("A1.mxf")],
            vec![path("picture.mxf")],
            vec![path("A1.mxf")],
            vec![path("A2.mxf")],
            vec![path("gain.mxf")],
        ]);
    }
}

pub(super) fn writer_clip(clip: &Clip, output: &Path) -> R<OriginalClip> {
    let target = output.to_str().ok_or("mono WAV output path is not UTF-8")?;
    let a = audio_clip(clip)?;
    let label = a.avid_track.unwrap();
    Ok(OriginalClip {
        premiere_assisted: false,
        separate_mono_streams: false,
        proxy_path: None,
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
            source_start: 0,
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
