//! Opt-in external FFmpeg 8.1 OP-Atom -> OP1a preparation; no original-file writes.
//! Plan construction is read-only. Execution and receipts keep their existing
//! validation order, exclusive writes and retention of partial outputs on failure.
mod backend;
mod execution;
mod planning;
mod mov;
mod mono;
mod receipts;
pub(crate) mod source;
mod terminal;
mod validation;

#[cfg(target_os = "macos")]
pub(crate) use terminal::managed_dnxhd_layout;

pub(crate) use backend::{read_stdout as read_media_tool, run as run_media_tool};

pub(crate) fn inspect_staged_atom(tool: &std::path::Path, path: &std::path::Path) -> R<AtomEssence> {
    let header = avid_reader::mxf::read_identity(path).map_err(|error| error.to_string())?;
    let probe = backend::probe(tool, path)?;
    let ([stream], [material], [file]) = (probe.streams.as_slice(), header.material_packages.as_slice(), header.file_packages.as_slice()) else {
        return Err("expected one staged file package, material package and essence stream".into());
    };
    if probe.op != backend::OP_ATOM || !probe.references.is_empty() {
        return Err("expected a standalone MXF OP-Atom essence file".into());
    }
    backend::verify_stream_package(&probe, &header.file_packages)?;
    Ok(AtomEssence {
        file: file.clone(), material: material.clone(), timecode: probe.tc, header_timecode: header.start_timecode(),
        profile: stream.profile.clone(),
        header_picture_profile: header.picture_essence_coding.and_then(avid_reader::mxf::PictureProfile::from_label),
        picture_descriptor: header.picture_descriptor,
        picture_essence_coding: header.picture_essence_coding,
        codec: stream.codec.clone(), ticks: stream.ticks,
        packets: stream.packets, raster: (stream.width, stream.height), pix: stream.pix.clone(),
        channels: stream.channels, sample_rate: stream.samples, bits: stream.bits,
    })
}

pub(crate) struct AtomEssence {
    pub file: String,
    pub material: String,
    pub timecode: Option<String>,
    pub header_timecode: Option<(u64, u16, bool)>,
    pub profile: String,
    pub header_picture_profile: Option<avid_reader::mxf::PictureProfile>,
    pub picture_descriptor: Vec<(&'static str, Vec<u8>)>,
    pub picture_essence_coding: Option<[u8; 16]>,
    pub codec: String,
    pub ticks: i64,
    pub packets: i64,
    pub raster: (i64, i64),
    pub pix: String,
    pub channels: i64,
    pub sample_rate: i64,
    pub bits: i64,
}

#[cfg(test)]
mod tests;

pub use validation::{normalize_source_clocks, validate_bin_audio};
pub(crate) use planning::{
    preview_native_recipes, preview_picture_recipes, referenced_input_paths, NativeMediaPreparation,
    PrepareIssue,
};
pub(crate) use mono::gain_paths;
pub(crate) use source::{essence_spans, pcm_essence_bytes};

pub(crate) struct LtRangePicture {
    pub(crate) path: PathBuf,
    stamp: Stamp,
    ffprobe: PathBuf,
    package: String,
    stream: usize,
    terminal: terminal::Terminal,
}

pub(crate) type LtRangePictures = std::collections::HashMap<PathBuf, LtRangePicture>;

impl LtRangePicture {
    pub(crate) fn contains_frames(&self, start: i64, frames: i64) -> bool {
        start >= 0 && frames > 0 && start.checked_add(frames).is_some_and(|end| end <= self.terminal.frames)
    }

    pub(crate) fn verify_span(&self, start: i64, frames: i64, offset: i64, bytes: i64) -> R<()> {
        if self.terminal.matches_span(start, frames, offset, bytes) {
            Ok(())
        } else {
            Err("the saved Color Adapter byte range does not match the indexed picture edit".into())
        }
    }
}

pub(crate) fn verify_lt_range_picture(
    path: &std::path::Path,
    tool: &std::path::Path,
    frames: i64,
    package: &str,
) -> R<LtRangePicture> {
    let before = source::stamp(path)?;
    let package = package.to_ascii_uppercase();
    let identity = avid_reader::mxf::read_identity(path).map_err(|err| err.to_string())?;
    let probe = backend::probe(tool, path)?;
    backend::verify_stream_package(&probe, &identity.file_packages)?;
    let [picture] = probe.streams.as_slice() else {
        return Err("range expansion requires one managed ProRes picture".into());
    };
    // The MOV media plan also verifies the full clock and matching output color metadata.
    if probe.op == backend::AVID_PRORES_LT_ATOM
        && identity.file_packages == [package.as_str()]
        && identity.picture_essence_coding == Some(avid_reader::mxf::PRORES_422_LT_CODING)
        && identity.file_tracks == [(24, 1, frames)]
        && Some(picture.ticks) == frames.checked_mul(prproj::convert::model::ticks_per_unit(Fraction::new(24, 1)))
        && picture.profile == "LT"
        && picture.color.get(1).and_then(Option::as_deref) == Some("tv")
        && picture.color.get(2).and_then(Option::as_deref) == Some("bt709")
    {
        let terminal = terminal::Terminal::qualify_lt(path, &probe, tool)?;
        if source::stamp(path)? != before {
            return Err("input changed during Color Adapter picture verification".into());
        }
        Ok(LtRangePicture {
            path: path.into(),
            stamp: before,
            ffprobe: tool.into(),
            package,
            stream: probe.stream_indices[0],
            terminal,
        })
    } else {
        Err("range expansion requires matching Avid ProRes LT with explicit Rec.709 video levels".into())
    }
}

pub(crate) fn verify_dnxhd_range_picture(
    path: &std::path::Path, tool: &std::path::Path, frames: i64, package: &str,
) -> R<()> {
    let package = package.strip_prefix("urn:smpte:umid:").ok_or("missing source package")?;
    let package = backend::umid(&package.replace('.', ""))?;
    let identity = avid_reader::mxf::read_identity(path).map_err(|err| err.to_string())?;
    let probe = backend::probe(tool, path)?;
    backend::verify_stream_package(&probe, &identity.file_packages)?;
    let [picture] = probe.streams.as_slice() else {
        return Err("nested range expansion requires one managed picture stream".into());
    };
    let expected_color = ["1:1", "tv", "bt709", "bt709", "bt709", "progressive"];
    if probe.op != backend::OP_ATOM
        || identity.file_packages != [package]
        || identity.file_tracks != [(24, 1, frames)]
        || identity.file_start_timecode() != Some((0, 24, false))
        || picture.codec != "dnxhd" || picture.profile != "DNXHD" || picture.pix != "yuv422p"
        || (picture.width, picture.height) != (1920, 1080) || picture.rate != "24/1"
        || !(picture.packets == frames || frames.checked_add(1) == Some(picture.packets))
        || !picture.color.iter().map(Option::as_deref).eq(expected_color.map(Some))
    {
        return Err("nested range expansion requires verified 1080p24 DNxHD and explicit Rec.709 video levels".into());
    }
    Ok(())
}

use backend::Probe;
use graph::json::Value;
use prproj::convert::model::Fraction;
use source::Stamp;
use std::path::PathBuf;

type R<T> = Result<T, String>;

struct Input {
    path: PathBuf,
    aliases: Vec<String>,
    stamp: Stamp,
    probe: Probe,
    identity: Value,
    terminal: Option<terminal::Terminal>,
}
struct Job {
    inputs: Vec<usize>,
    clips: Vec<Value>,
    tc: Option<String>,
    rate: Fraction,
    mov: bool,
    pixel_aspect: Option<mov::PixelAspect>,
    separate_mono: bool,
    wide_audio: Vec<mono::MonoPcm>,
}
impl Job {
    fn packaged_inputs(&self) -> &[usize] {
        if self.wide_audio.is_empty() { &self.inputs } else { &self.inputs[..1] }
    }
}
pub struct Plan {
    destination: PathBuf,
    inputs: Vec<Input>,
    jobs: Vec<Job>,
    mono: Vec<mono::MonoPcm>,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    versions: Vec<String>,
    estimate: u64,
    free: u64,
}
