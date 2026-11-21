//! Write a native Premiere project from the neutral edit model (port of
//! `converter/src/avid2prproj/writer.py`).
//!
//! [`write_project`] is the whole conversion: bins and clips (separate, merged, grouped and
//! stereo builds), sequences with linked placements at exact ticks, speed, audio gain / volume
//! / pan / track levels / crossfades, Avid colour correction as an embedded LUT (plus a
//! `.cube` in `"<stem> LUTs"` next to the project), clip / sequence / edit / filler markers
//! (edit copies, marker-conflict rules, adjustment layers), and the save.
//!
//! Every UUID comes from the one `&mut dyn UuidSource` passed in, in exactly Python's order:
//! with `SeededUuids::new(seed)` the output is byte-identical to the Python writer run under
//! `seeded_ids(seed)`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::audio::{add_crossfade, set_clip_gain, set_clip_volume, set_mix_level, set_mono_track_pan, set_track_level, split_overflow};
use super::avid_color::{self, ExtendedSdrGrade};
use super::color_mode::ClipLut;
use super::colors;
use super::combined::{
    combined_order, create_grouped_with_media as create_grouped, create_merged, create_stereo_nest, declare_stereo_chain, display_format,
    group_for_channel, move_to_processed_clips, narrow_to_group, parts_bin, set_clip_pan, set_clip_pan_keys, tag_merged_audio, tag_merged_picture, Pan,
};
use super::lumetri::{apply_lumetri_lut, cube_text, cube_text_domain, itx3_lut_domain};
use super::markers as mk;
use super::matte::apply_matte;
use super::motion::{apply_corner_pin, apply_corner_pin_with_flop, apply_flip, apply_motion_keys};
use super::multicam::PreparedGroupAudio;
use super::time_remap::set_time_remap;
use super::metadata;
use super::prepared::{PictureEffect, PreparedLayeredSubmaster, PreparedPictureEffect, PreparedPictureNest, PreparedText, PreparedTrackLane, PreparedVideoDissolve};
use super::model::{event_audio_files, py_str_repr, ticks_per_unit, Clip, ColorCorrection, Event, EventAudio, Fraction, Marker, Model, ModelError, Sequence};
use super::opatom::{add_audio_item, add_video_item, source_item_names, DirectDnxhrFile, MediaTable, OpatomError, UnverifiedShape};
use super::placement::normalize_placement;
use crate::build::fmt::{fmt_f, py_repr};
use crate::build::sequence::{extra_formats, formats, linked_original_formats};
use crate::build::skeleton::ProjectVersion;
use crate::build::time::{floor_div, Time};
use crate::error::Error;
use crate::ids::UuidSource;
use crate::model::project::{py_int, py_path};
use crate::model::{ItemId, Project, SequenceId, TrackId, TrackItemId};

#[path = "writer_destination_layout.rs"]
mod destination_layout;
pub use destination_layout::DestinationBinLayout;
#[path = "writer_png_mattes.rs"]
mod png_mattes;

/// Lumetri LUT edge length.
pub const LUT_SIZE: i64 = 33;

/// Presets of py_premiere's `FORMATS` the converter uses as they are.
const VERIFIED_PRESETS: [&str; 8] = [
    "1080p23976", "1080p25", "1080p2997", "1080p50", "1080p5994", "2160p23976", "2160p25", "2160p2997",
];

/// Why a write failed, by the Python exception class (the CLI exits 2 for the first four,
/// like `avid2prproj`; anything else is a traceback there).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// `ModelError`.
    Model(ModelError),
    /// `UnverifiedShape`: a Premiere shape not observed yet.
    Unverified(UnverifiedShape),
    /// `FileExistsError`: the output exists (the converter never overwrites).
    Exists(String),
    /// `FileNotFoundError`: the output folder does not exist.
    NotFound(String),
    /// Anything else (Python `ValueError`, `KeyError`, ...).
    Other(Error),
}

impl WriteError {
    /// The errors `python -m avid2prproj` reports as `avid2prproj: <message>`, exit status 2.
    pub fn is_reported(&self) -> bool {
        !matches!(self, WriteError::Other(_))
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Model(e) => e.fmt(f),
            WriteError::Unverified(e) => e.fmt(f),
            WriteError::Exists(m) | WriteError::NotFound(m) => f.write_str(m),
            WriteError::Other(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<ModelError> for WriteError {
    fn from(e: ModelError) -> Self {
        WriteError::Model(e)
    }
}

impl From<UnverifiedShape> for WriteError {
    fn from(e: UnverifiedShape) -> Self {
        WriteError::Unverified(e)
    }
}

impl From<OpatomError> for WriteError {
    fn from(e: OpatomError) -> Self {
        match e {
            OpatomError::Unverified(u) => WriteError::Unverified(u),
            OpatomError::Other(e) => WriteError::Other(e),
        }
    }
}

impl From<Error> for WriteError {
    fn from(e: Error) -> Self {
        match e {
            Error::FileExists(message) => WriteError::Exists(message),
            Error::FileNotFound(message) => WriteError::NotFound(message),
            other => WriteError::Other(other),
        }
    }
}

impl From<std::io::Error> for WriteError {
    fn from(e: std::io::Error) -> Self {
        WriteError::Other(e.into())
    }
}

/// Result of the converter's Premiere logic.
pub type Result<T> = std::result::Result<T, WriteError>;

fn other(message: impl Into<String>) -> WriteError {
    WriteError::Other(Error::Value(message.into()))
}

/// What the write reports (Python `Report`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub notes: Vec<String>,
    pub unverified: Vec<String>,
    pub grouped_picture_marker_moves: Vec<GroupedPictureMarkerMove>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupedPictureMarkerMove {
    pub sequence: usize,
    pub record_ticks: i64,
    pub marker_name: String,
}

/// `_preset(seq, report)` over a sequence's (or a stereo build's `_Format`) name, rate and
/// frame size: a verified py_premiere preset, else a converter format (noted), else
/// [`UnverifiedShape`].
pub fn preset(name: &str, rate: Fraction, width: i64, height: i64, report: &mut Report) -> Result<String> {
    let unit = ticks_per_unit(rate);
    let matches = |frame_rate: u64, w: u32, h: u32| {
        u64::try_from(unit).ok() == Some(frame_rate) && i64::from(w) == width && i64::from(h) == height
    };
    for (preset, f) in formats() {
        if matches(f.frame_rate, f.width, f.height) && VERIFIED_PRESETS.contains(&preset) {
            return Ok(preset.to_string());
        }
    }
    for (preset, f) in extra_formats().into_iter().chain(linked_original_formats()) {
        if matches(f.frame_rate, f.width, f.height) {
            let provenance = if preset == "avid2prproj-1080p24" {
                " (User-reported in Premiere 26.5.2, PRPROJ-0010)"
            } else {
                ""
            };
            report.notes.push(format!("sequence {}: converter format {preset}{provenance}", py_str_repr(name)));
            return Ok(preset.to_string());
        }
    }
    Err(UnverifiedShape(format!("sequence {}: no sequence format for {rate} fps {width}x{height}", py_str_repr(name))).into())
}

fn preset_bound(
    name: &str,
    rate: Fraction,
    width: i64,
    height: i64,
    report: &mut Report,
    rewrapped: bool,
) -> Result<String> {
    match preset(name, rate, width, height, report) {
        Ok(p) => Ok(p),
        Err(e) if !rewrapped => Err(e),
        Err(_) => {
            let p = super::rewrap::sequence_preset(rate, width, height)
                .ok_or_else(|| other("unsupported rewrap sequence format"))?;
            report.unverified.push(format!("sequence {}: opt-in OP1a format {rate} fps {width}x{height}; native acceptance pending",py_str_repr(name)));
            Ok(p)
        }
    }
}

/// `set_playback_speed(placed, ratio)`: constant speed on a placed clip: `<PlaybackSpeed>`
/// (source frames per timeline frame) on the clip core, `<PlayBackwards>true` for reverse; the
/// in/out points written afterwards are stored scaled by the speed. Element position after
/// OutPoint is UNVERIFIED (awaiting User read-back).
pub fn set_playback_speed(project: &mut Project, placed: TrackItemId, ratio: (i64, i64)) -> Result<()> {
    let (timeline, source) = ratio;
    let clip = project.track_item(placed).clip;
    let tree = &mut project.doc.tree;
    let Some(core) = tree.child_by_tag(clip, "Clip") else { return Err(other("'NoneType' object has no attribute 'find'")) };
    if timeline == 0 {
        return Err(other("division by zero"));
    }
    let speed = source as f64 / (timeline as f64).abs();
    let backwards = (timeline < 0).then_some("true".to_string());
    for (tag, text) in [("PlaybackSpeed", Some(py_repr(speed))), ("PlayBackwards", backwards)] {
        let Some(text) = text else { continue };
        let leaf = match tree.child_by_tag(core, tag) {
            Some(leaf) => leaf,
            None => {
                let leaf = tree.sub_element(core, tag, &[]);
                let children = tree.children(core).to_vec();
                let previous = children[children.len() - 2];
                let previous_tail = tree.tail(previous).map(str::to_string);
                let first_tail = tree.tail(children[0]).map(str::to_string);
                tree.set_tail(leaf, previous_tail);
                tree.set_tail(previous, first_tail);
                leaf
            }
        };
        tree.set_text(leaf, Some(text));
    }
    Ok(())
}

/// `_report_stereo_pans`: Avid can pan each channel of a stereo edit; a Premiere stereo clip
/// has one Balance. Default (unset or 100L/100R) converts exactly; anything else is reported.
fn report_stereo_pans(report: &mut Report, seq: &Sequence, i: usize, track: i64, pans: &[Option<f64>]) {
    let is = |p: Option<f64>, v: f64| p.is_none() || p == Some(v);
    if pans.iter().any(Option::is_some) && !(is(pans[0], -100.0) && is(pans[pans.len() - 1], 100.0)) {
        let shown: Vec<String> = pans
            .iter()
            .map(|p| match p {
                None => "default".to_string(),
                Some(p) => {
                    let s = fmt_f(*p, 0);
                    if s.starts_with('-') {
                        s
                    } else {
                        format!("+{s}")
                    }
                }
            })
            .collect();
        report.unverified.push(format!(
            "sequence {} event {i}: stereo edit on A{track} has per-channel pan {} in Avid; placed as plain stereo (decision pending)",
            py_str_repr(&seq.name),
            shown.join("/")
        ));
    }
}

/// Python's unbounded `int` result as tick count: the exact value when it fits `i64`, else a
/// clean error (Python would write the big number; Premiere ticks never get there).
fn fit(value: i128) -> Result<i64> {
    i64::try_from(value).map_err(|_| other(format!("integer out of range: {value} ticks")))
}

/// `a + b` on tick counts, without overflow.
fn add(a: i64, b: i64) -> Result<i64> {
    fit(i128::from(a) + i128::from(b))
}

/// A Python `OSError` raised for `path`: `[Errno N] <strerror>: '<path>'`, as the class the CLI
/// reports (`FileExistsError`, `FileNotFoundError`) or, for any other errno, a crash.
fn os_error(error: &std::io::Error, path: &Path) -> WriteError {
    let text = error.to_string();
    let strerror = match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text.clone(),
    };
    let name = py_str_repr(&path.to_string_lossy());
    let message = match error.raw_os_error() {
        Some(code) => format!("[Errno {code}] {strerror}: {name}"),
        None => format!("{strerror}: {name}"),
    };
    match error.kind() {
        std::io::ErrorKind::AlreadyExists => WriteError::Exists(message),
        std::io::ErrorKind::NotFound => WriteError::NotFound(message),
        _ => WriteError::Other(Error::Io(message)),
    }
}

/// Allocate the same suffixes as the former exclusive writer, accounting for
/// both existing paths and other assets planned in this document. No path is created.
fn plan_cube_file(path: &Path, planned: &mut HashSet<String>) -> Result<PathBuf> {
    let stem = path.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default();
    let extension = path.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let mut number = 1u64;
    loop {
        let candidate = if number == 1 { path.to_path_buf() } else { path.with_file_name(format!("{stem} ({number}){extension}")) };
        let key = candidate.to_string_lossy().to_lowercase();
        match std::fs::symlink_metadata(&candidate) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !planned.contains(&key) => {
                planned.insert(key); return Ok(candidate);
            },
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(os_error(&e,&candidate)),
            _ => {},
        }
        number=number.checked_add(1).ok_or_else(|| other("no unused LUT filename available"))?;
    }
}

fn check_lut_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(WriteError::Exists(format!("{} exists; LUT output folder must be a real directory",path.display()))),
        // Preserve the former mkdir/Python FileExistsError contract without
        // creating anything during preparation.
        Ok(m) if !m.is_dir() => Err(WriteError::Exists(format!("[Errno 17] File exists: {}",py_str_repr(&path.to_string_lossy())))),
        Ok(_) => Ok(()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(os_error(&e,path)),
    }
}

/// Small deterministic recipe rather than a duplicate full LUT payload. The
/// validated payload hash is checked again on publication; UUIDs are not consumed.
struct PreparedLut {
    path: PathBuf,
    recipe: LutRecipe,
    title: String,
    sha256: String,
}

enum LutRecipe {
    Legacy(ColorCorrection),
    Extended(ExtendedSdrGrade),
}

impl PreparedLut {
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = match &self.recipe {
            LutRecipe::Legacy(color) => {
                let curves = color.curves.iter().map(|(k,v)|
                    (k.clone(),v.iter().map(|&(x,y)|(x*255.0,y*255.0)).collect())).collect::<Vec<_>>();
                let transform = avid_color::rgb_transform(&color.tone, color.saturation, &curves)?;
                let points = avid_color::cube_lut(&*transform, LUT_SIZE)?;
                cube_text(&points, LUT_SIZE, &self.title)
            }
            LutRecipe::Extended(grade) => cube_text_domain(&grade.points(),
                i64::from(ExtendedSdrGrade::EDGE), &self.title, ExtendedSdrGrade::DOMAIN),
        }.into_bytes();
        if crate::hash::hex(&crate::hash::sha256(&bytes))!=self.sha256 {
            return Err(other("prepared LUT payload changed before publication"));
        }
        Ok(bytes)
    }
}

/// One validated, encoded Premiere document. Its DOM is released after preparation;
/// publishing these bytes never rebuilds the document or draws another UUID.
pub struct PreparedProject {
    saved: Option<crate::model::project::PreparedSave>,
    out_path: PathBuf,
    report: Report,
    assets: Vec<PreparedLut>,
    assets_attempted: bool,
    assisted_media_pending: bool,
    assisted_media: super::media_donor::AssistedMedia,
    assets_published: bool,
    published: bool,
    retained: Vec<PathBuf>,
    saved_marker_data: HashSet<MarkerDataKey>,
    saved_marker_colors: HashSet<MarkerDataKey>,
}
impl PreparedProject {
    pub fn report(&self) -> &Report { &self.report }

    /// The internal import probe verifies the encoded media records before publication.
    #[doc(hidden)]
    pub fn verify_linked_media(
        &mut self,
        expected: &super::media_donor::ExpectedMedia,
        donor: Option<&super::media_donor::MediaRecords>,
    ) -> Result<()> {
        let saved = self.saved.as_mut().ok_or_else(|| other("project already published"))?;
        let mut document = saved.inspect_document()?;
        if let Some(records) = donor {
            records.apply(&mut document, expected)?;
            saved.replace_document(&document)?;
            document = saved.inspect_document()?;
        }
        super::media_donor::verify(&document, expected)?;
        Ok(())
    }
    /// Verified original locators whose media records still require Premiere.
    #[doc(hidden)]
    pub fn assisted_originals(&self) -> impl ExactSizeIterator<Item = &String> {
        self.assisted_media.paths()
    }
    /// Replace converter-owned drafts with verified native media encoding.
    #[doc(hidden)]
    pub fn verify_assisted_media(&mut self, donor: Option<&Path>) -> Result<()> {
        let saved = self.saved.as_mut().ok_or_else(|| other("project already published"))?;
        let mut document = saved.inspect_document()?;
        if let Some(path) = donor {
            let encoding = self.assisted_media.apply(&mut document, path)?;
            saved.replace_document(&document)?;
            self.assisted_media.verify_encoding(&saved.inspect_document()?, &encoding)?;
            self.assisted_media_pending = false;
        } else {
            self.assisted_media.verify_draft(&document)?;
        }
        Ok(())
    }
    pub fn output_path(&self) -> &Path { &self.out_path }
    pub fn retained_paths(&self) -> &[PathBuf] { &self.retained }
    pub fn is_published(&self) -> bool { self.published }
    pub fn has_marker_custom_data(&self, key: &MarkerDataKey) -> bool {
        self.saved_marker_data.contains(key)
    }
    pub fn has_marker_original_color(&self, key: &MarkerDataKey) -> bool {
        self.saved_marker_colors.contains(key)
    }
    /// Reserve another conversion output against both LUT files and their directory.
    /// Existing parents are resolved, so aliases cannot evade the preparation check.
    pub fn check_reserved_output(&self, path: &Path) -> Result<()> {
        fn key(path: &Path) -> std::io::Result<String> {
            let mut missing=Vec::new(); let mut root=path;
            loop {
                match std::fs::symlink_metadata(root) {
                    Ok(_)=>break,
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                    Err(e)=>return Err(e),
                }
                if let Some(name)=root.file_name() { missing.push(name.to_owned()); }
                root=root.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            }
            let mut resolved=std::fs::canonicalize(root)?;
            for name in missing.into_iter().rev() { resolved.push(name); }
            Ok(resolved.to_string_lossy().to_lowercase())
        }
        let reserved=key(path).map_err(|e|os_error(&e,path))?;
        for asset in &self.assets {
            for allocated in [&asset.path, &py_parent(&asset.path)] {
                if key(allocated).map_err(|e|os_error(&e,allocated))?==reserved {
                    return Err(other(format!("{} conflicts with a prepared LUT output {}",path.display(),allocated.display())));
                }
            }
        }
        Ok(())
    }
    pub fn recheck_outputs(&self) -> Result<()> {
        match std::fs::symlink_metadata(&self.out_path) {
            Ok(_) => return Err(WriteError::Exists(format!("{} exists; the converter never overwrites",self.out_path.display()))),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(os_error(&e,&self.out_path)),
        }
        if !self.assets_published {
            for asset in &self.assets {
                check_lut_directory(&py_parent(&asset.path))?;
                match std::fs::symlink_metadata(&asset.path) {
                    Ok(_) => return Err(WriteError::Exists(format!("{} appeared after preparation; refusing to rename a prepared LUT",asset.path.display()))),
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
                    Err(e) => return Err(os_error(&e,&asset.path)),
                }
            }
        }
        Ok(())
    }
    pub fn publish_assets(&mut self) -> Result<()> {
        if self.assisted_media_pending { return Err(other("Premiere-assisted media has not been verified")); }
        if self.assets_published { return Ok(()); }
        if self.assets_attempted { return Err(other("asset publication already attempted; retained partial outputs require inspection")); }
        self.recheck_outputs()?;
        self.assets_attempted=true;
        for asset in &self.assets {
            let bytes=asset.bytes()?;
            let parent=py_parent(&asset.path);
            match std::fs::create_dir(&parent) {
                Ok(()) => self.retained.push(parent.clone()),
                Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists => check_lut_directory(&parent)?,
                Err(e) => return Err(os_error(&e,&parent)),
            }
            let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&asset.path)
                .map_err(|e|os_error(&e,&asset.path))?;
            self.retained.push(asset.path.clone());
            file.write_all(&bytes).map_err(|e|os_error(&e,&asset.path))?;
        }
        self.assets_published=true;
        Ok(())
    }
    pub fn publish_project(&mut self) -> Result<Report> {
        if !self.assets_published { return Err(other("publish validated assets before the project")); }
        self.recheck_outputs()?;
        let saved=self.saved.take().ok_or_else(||other("project publication already attempted"))?;
        let warnings=saved.publish()?;
        self.published=true;
        self.retained.push(self.out_path.clone());
        self.report.notes.extend(warnings);
        Ok(std::mem::take(&mut self.report))
    }
    pub fn publish(mut self) -> Result<Report> {
        self.publish_assets()?;
        self.publish_project()
    }
}

/// Python `round(Fraction)`: half to even.
fn round_fraction(rate: Fraction) -> i64 {
    let floor = rate.num.div_euclid(rate.den);
    let twice = 2 * rate.num.rem_euclid(rate.den);
    if twice > rate.den || (twice == rate.den && floor % 2 != 0) {
        floor + 1
    } else {
        floor
    }
}

/// `_timecode(ticks, rate, start)`: `HH:MM:SS:FF` (non-drop, hours wrap at 24).
pub fn timecode(ticks: i64, rate: Fraction, start: i64) -> Result<String> {
    let unit = ticks_per_unit(rate);
    let fps = round_fraction(rate);
    let frames = floor_div(add(ticks, start)?, unit)?;
    let hours = floor_div(frames, fit(3600 * i128::from(fps))?)?.rem_euclid(24);
    let minutes = floor_div(frames, fit(60 * i128::from(fps))?)?.rem_euclid(60);
    let seconds = floor_div(frames, fps)?.rem_euclid(60);
    Ok(format!("{hours:02}:{minutes:02}:{seconds:02}:{:02}", frames.rem_euclid(fps)))
}

/// `_scoped(markers, tracks)`: the clip markers on the given Avid master tracks ("V1", "A2");
/// `None` = all.
fn scoped<'a>(markers: &'a [Marker], tracks: Option<&[String]>) -> Vec<&'a Marker> {
    markers
        .iter()
        .filter(|m| match tracks {
            None => true,
            Some(tracks) => m.track.as_ref().is_some_and(|t| tracks.contains(t)),
        })
        .collect()
}

/// `_order_track_items`: Premiere lists a track's clips in time order; out of order (filler
/// adjustment layers added after the edits), Premiere 26.5 played a V1 clip but did not draw it
/// (User, 2026-10-15).
fn order_track_items(project: &mut Project, sequence: SequenceId) -> Result<()> {
    let tracks: Vec<TrackId> = project.video_tracks(sequence).iter().chain(project.audio_tracks(sequence)).copied().collect();
    for track in tracks {
        project.sort_track_clips_by_start(track)?;
    }
    Ok(())
}

/// `_label_items`: the Avid clip colour as the Premiere label of each item.
fn label_items(project: &mut Project, clip: &Clip, items: &[ItemId], options: &super::model::Options) -> Result<()> {
    let Some(rgb16) = clip.avid.as_ref().and_then(|a| a.color_rgb16) else { return Ok(()) };
    let index = colors::nearest_label(rgb16, &options.clip_labels);
    for &item in items {
        mk::set_label(project, item, index)?;
    }
    Ok(())
}

/// Python `Path.parent`: `.` for a bare name.
fn py_parent(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from(if path.has_root() && path.parent().is_none() { "/" } else { "." }),
    }
}

/// Python `parent / name` (no `./` prefix for `.`).
fn py_join(parent: &Path, name: &str) -> PathBuf {
    let parent = parent.to_string_lossy();
    if parent == "." {
        py_path(name)
    } else if parent.ends_with('/') {
        py_path(&format!("{parent}{name}"))
    } else {
        py_path(&format!("{parent}/{name}"))
    }
}

/// Python `str.isalnum()` for one character (Unicode letters and numbers). Rust's
/// `is_alphanumeric` also accepts `Other_Alphabetic` marks; those are rare in sequence names.
fn is_alnum(c: char) -> bool {
    c.is_alphanumeric()
}

/// How a clip is placed (Python `placement[clip_id]` payloads).
#[derive(Clone, Debug)]
enum Payload {
    /// `(video_item, audio_items, stereo_builds)`: audio in model order (separate) or combined
    /// A01-first order (merged); stereo builds keyed by their Avid track pair.
    Parts { video: Option<ItemId>, audio: Vec<ItemId>, stereo: Vec<(Vec<i64>, ItemId)> },
    /// A grouped (multicam) item.
    Grouped(ItemId),
    Native(ItemId),
    Original(ItemId),
}

fn stereo_get(stereo: &[(Vec<i64>, ItemId)], key: &[i64]) -> Option<ItemId> {
    stereo.iter().rev().find(|(k, _)| k == key).map(|(_, v)| *v)
}

fn stereo_set(stereo: &mut Vec<(Vec<i64>, ItemId)>, key: Vec<i64>, item: ItemId) {
    match stereo.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = item,
        None => stereo.push((key, item)),
    }
}

/// A marker placed on the sequence instead: (marker, sequence ticks, note).
type Extra = (Marker, i64, String);

struct LegacyClips<'a> {order:Vec<&'a Clip>, index:HashMap<&'a str,usize>}
impl<'a> FromIterator<(&'a str,&'a Clip)> for LegacyClips<'a> {
    fn from_iter<T:IntoIterator<Item=(&'a str,&'a Clip)>>(values:T)->Self{let mut out=Self{order:vec![],index:HashMap::new()};for(id,c)in values{out.index.insert(id,out.order.len());out.order.push(c);}out}
}
impl LegacyClips<'_> {fn values(&self)->impl Iterator<Item=&Clip>{self.order.iter().copied()} fn get(&self,id:&str)->Option<&Clip>{self.index.get(id).map(|i|self.order[*i])}}
impl std::ops::Index<&str> for LegacyClips<'_>{type Output=Clip;fn index(&self,id:&str)->&Clip{self.get(id).expect("validated legacy clip reference")}}
struct WriterInput<'a> {
    options:&'a super::model::Options,
    project_version: ProjectVersion,
    marker_provenance: Option<MarkerProvenance<'a>>,
    clips:LegacyClips<'a>,
    originals:HashMap<&'a str,&'a super::prepared::OriginalClip>,
    bins:&'a [super::model::Bin], sequences:&'a [Sequence],
    texts:&'a [super::prepared::PreparedText],
    video_dissolves: &'a [PreparedVideoDissolve],
    audio_edge_fades: &'a [super::prepared::PreparedAudioEdgeFade],
    audio_crossfades: &'a [super::prepared::PreparedAudioCrossfade],
    audio_pans: &'a [super::prepared::PreparedAudioPan],
    audio_mixers: &'a BTreeMap<usize, super::prepared::PreparedAudioMixer>,
    track_states: &'a BTreeMap<usize, Vec<super::prepared::PreparedTrackState>>,
    timeline_marks: &'a BTreeMap<usize, super::prepared::PreparedTimelineMarks>,
    non_drop_sequences: &'a HashSet<usize>,
    picture_effects: &'a [PreparedPictureEffect],
    source_luts: &'a [super::prepared::PreparedSourceLut],
    extended_sdr_grades: &'a HashSet<(usize, usize)>,
    edit_labels: &'a [super::prepared::PreparedEditLabel],
    frame_holds: &'a HashMap<(usize, usize), i64>,
    disabled_edits: &'a HashSet<(usize, usize)>,
    direct_dnxhr_origins: &'a HashMap<String, i64>,
    direct_dnxhd_pcm24: Option<&'a HashSet<String>>,
    constant_speeds: &'a [super::prepared::PreparedConstantSpeed],
    multicams: &'a [super::multicam::PreparedMulticam],
    multicam_uses: &'a [super::multicam::PreparedMulticamUse],
    multicam_tracks: &'a [super::multicam_tracks::PreparedTrackMulticam],
    autosync_bins: &'a BTreeMap<usize, usize>,
    picture_subclips: &'a [super::prepared::PreparedPictureSubclip],
    av_subclips: &'a [super::prepared::PreparedAvSubclip],
    av_subclip_uses: &'a [super::prepared::PreparedAvSubclipUse],
    picture_nests: &'a [PreparedPictureNest],
    layered_submasters: &'a [PreparedLayeredSubmaster],
    png_mattes: &'a super::prepared::PreparedPngMattes,
}
impl WriterInput<'_> {
    fn picture_clock(&self, clip: &str) -> Option<(super::model::Fraction, i64)> {
        self.clips.get(clip).and_then(|c| c.video.as_ref()).map(|v| (v.rate, v.duration))
            .or_else(|| self.originals.get(clip).and_then(|c| c.video.as_ref()).map(|v| (v.rate, v.duration)))
    }

    fn native_sequence_channels(&self,seq:&Sequence)->Result<usize> {
        let widths:HashSet<_>=seq.events.iter().flat_map(|e|e.stereo.iter().filter(|s|s.channels.len()>2).map(|s|s.channels.len())).collect();
        if widths.is_empty(){return Ok(0);}
        if widths.len()!=1{return Err(other("mixed 5.1/7.1 timeline output needs explicit destination routing"));}
        let width=*widths.iter().next().unwrap();
        for e in &seq.events {
            let mono=if self.originals.contains_key(e.clip.as_str()){!e.audio_tracks.is_empty()}else{!event_audio_files(&self.clips[e.clip.as_str()],e)?.is_empty()};
            if mono||e.stereo.iter().any(|s|s.channels.len()!=width){return Err(other("mixed mono/stereo/surround timeline output needs explicit destination routing"));}
        }
        Ok(width)
    }
}

struct Writer<'a> {
    model: &'a WriterInput<'a>,
    rewrapped: bool,
    native_av: bool,
    linked_op1a: &'a HashSet<String>,
    project: Project,
    media: MediaTable,
    ids: &'a mut dyn UuidSource,
    report: Report,
    saved_marker_data: HashSet<MarkerDataKey>,
    saved_marker_colors: HashSet<MarkerDataKey>,
    root: ItemId,
    sequence_parent: Option<ItemId>,
    out_path: PathBuf,
    assets: Vec<PreparedLut>,
    asset_names: HashSet<String>,
    extended_luts: HashMap<ExtendedSdrGrade, (String, ClipLut)>,
    /// clip id -> (video item, audio items in model order)
    items: HashMap<String, (Option<ItemId>, Vec<ItemId>)>,
    /// clip id -> ("separate" | "merged" | "grouped", payload)
    placement: HashMap<String, (&'static str, Payload)>,
    /// clip id -> the bin it was imported into (`None` = the root)
    clip_bins: HashMap<String, Option<ItemId>>,
    multicam_items: Vec<ItemId>,
    multicam_uses: HashMap<(usize, usize), &'a super::multicam::PreparedMulticamUse>,
    track_multicam_items: HashMap<usize, Vec<(ItemId, &'a super::multicam_tracks::PreparedTrackMulticam)>>,
    av_subclip_items: Vec<ItemId>,
    av_subclip_uses: HashMap<(usize, usize, Option<i64>), usize>,
    picture_nests: HashSet<(usize, usize)>,
    png_items: Vec<ItemId>,
    png_edits: HashMap<usize, Vec<&'a super::prepared::PreparedPngEdit>>,
    constant_speeds: HashMap<(usize, usize), super::prepared::PreparedConstantSpeed>,
    video_dissolves: HashMap<usize, Vec<&'a PreparedVideoDissolve>>,
    audio_edge_fades: HashMap<(usize, usize, i64), &'a super::prepared::PreparedAudioEdgeFade>,
    audio_crossfades: HashMap<usize, Vec<&'a super::prepared::PreparedAudioCrossfade>>,
    audio_pans: HashMap<(usize, usize, i64), &'a super::prepared::PreparedAudioPan>,
    picture_effects: HashMap<(usize, usize), (PictureEffect, [bool; 2])>,
    source_luts: HashMap<(usize, usize), &'a super::prepared::PreparedSourceLut>,
    edit_labels: HashMap<(usize, usize), Vec<super::prepared::PreparedEditLabel>>,
    disabled_edits: &'a HashSet<(usize, usize)>,
}

/// Index of `file` (a reference into `clip.audio`) by identity (Python `id(a)`).
fn identity(clip: &Clip, file: &super::model::AudioFile) -> usize {
    clip.audio.iter().position(|f| std::ptr::eq(f, file)).expect("file of this clip")
}

/// Python `list[index]` with negative indices.
fn track_at(tracks: &[TrackId], number: i64) -> Result<TrackId> {
    let index = number - 1;
    let len = tracks.len() as i64;
    let at = if index < 0 { index + len } else { index };
    if at < 0 || at >= len {
        return Err(other("list index out of range"));
    }
    Ok(tracks[at as usize])
}

impl Writer<'_> {
    fn apply_extended_sdr_grade(
        &mut self,
        grade: ExtendedSdrGrade,
        picture: TrackItemId,
    ) -> Result<&str> {
        use std::collections::hash_map::Entry;
        let (filename, lut) = match self.extended_luts.entry(grade) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let stem = self.out_path.file_stem().unwrap_or_default().to_string_lossy();
                let folder = py_join(&py_parent(&self.out_path), &format!("{stem} LUTs"));
                check_lut_directory(&folder)?;
                let title = grade.title().to_owned();
                let requested = py_join(&folder, &format!("{title}.cube"));
                let path = plan_cube_file(&requested, &mut self.asset_names)?;
                let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let name = filename.strip_suffix(".cube")
                    .ok_or_else(|| other("extended SDR LUT filename has no .cube suffix"))?;
                let points = grade.points();
                let edge = ExtendedSdrGrade::EDGE;
                let domain = ExtendedSdrGrade::DOMAIN;
                let sha256 = crate::hash::hex(&crate::hash::sha256(
                    cube_text_domain(&points, i64::from(edge), &title, domain).as_bytes()));
                let lut = ClipLut::new(name, itx3_lut_domain(&points, edge, domain));
                self.assets.push(PreparedLut { path, recipe: LutRecipe::Extended(grade), title, sha256 });
                e.insert((filename, lut))
            }
        };
        lut.apply(&mut self.project, picture)?;
        Ok(filename)
    }

    fn add_clip_marker(&mut self, item: ItemId, marker: &Marker, start: i64) -> Result<crate::xml::NodeId> {
        Ok(mk::add_clip_marker_node(&mut self.project, item, marker, start, &self.model.options, &mut self.report.notes, self.ids)?)
    }

    fn original_markers(&mut self, item: ItemId, metadata: &super::prepared::ClipMetadata) -> Result<()> {
        let mut custom_data = Vec::new();
        let mut source_colors = Vec::new();
        let mut budget = mk::MAX_MARKER_CUSTOM_DATA;
        for (index, marker) in metadata.markers.iter().enumerate() {
            let node = self.add_clip_marker(item, marker, marker.start)?;
            let Some(rows) = self.model.marker_provenance else { continue };
            let key = MarkerDataKey::Source(metadata.id.clone(), index);
            let custom = rows.custom_data.get(&key)
                .filter(|data| !data.is_empty() && data.len() <= budget && custom_data.len() < 128);
            let source_color = rows.original_colors.get(&key).filter(|_| source_colors.len() < 128);
            if custom.is_none() && source_color.is_none() { continue; }
            let tree = &self.project.doc.tree;
            let guid = crate::model::marker::marker_guid(tree, node)?;
            if let Some(data) = custom {
                custom_data.push(format!("{guid}:{}", crate::hash::hex(data.as_bytes())));
                budget -= data.len();
                self.saved_marker_data.insert(key.clone());
            }
            if let Some(bit) = source_color {
                self.save_marker_color(node, &guid, *bit, key, &mut source_colors)?;
            }
        }
        if !custom_data.is_empty() || !source_colors.is_empty() {
            let tree = &self.project.doc.tree;
            let element = self.project.item(item).element;
            let uid = tree.get(element, "ObjectUID")
                .ok_or_else(|| other("source marker custom data lacks an item UID"))?
                .to_owned();
            let props = tree.find1(element, "ProjectItem/Node/Properties")
                .ok_or_else(|| other("source marker custom data lacks a property bag"))?;
            for (tag, entries) in [(mk::SOURCE_MARKER_DATA, custom_data), (mk::SOURCE_MARKER_COLORS, source_colors)] {
                if !entries.is_empty() {
                    let saved = format!("1 {uid} {}", entries.join(","));
                    crate::xml::mutations::append_leaf(&mut self.project.doc.tree, props, tag, &saved);
                }
            }
        }
        Ok(())
    }

    fn save_marker_color(
        &mut self, node: crate::xml::NodeId, guid: &str, bit: u64,
        key: MarkerDataKey, entries: &mut Vec<String>,
    ) -> Result<()> {
        let preview = crate::model::marker::marker_color_index(&self.project.doc.tree, node)?
            .ok_or_else(|| other("marker fallback color is outside the Premiere palette"))?;
        entries.push(format!("{guid}:{bit}:{preview}"));
        self.saved_marker_colors.insert(key);
        Ok(())
    }

    /// `part_markers`: master-clip markers on a source item: those on its Avid track, counted
    /// from the clip's first frame (at media start timecode + offset a marker on an MXF starting
    /// at 01:00:00:00 did not appear in Premiere 26.5; User, 2026-10-15).
    fn part_markers(&mut self, clip: &Clip, item: ItemId, track: String) -> Result<()> {
        for m in scoped(&clip.markers, Some(&[track])) {
            self.add_clip_marker(item, m, m.start)?;
        }
        Ok(())
    }

    /// `build_markers`: master-clip markers on a build (its own time starts at 0).
    fn build_markers(&mut self, clip: &Clip, build: ItemId, tracks: Option<&[String]>) -> Result<()> {
        for m in scoped(&clip.markers, tracks) {
            self.add_clip_marker(build, m, m.start)?;
        }
        Ok(())
    }

    fn merged_audio_marker_clock(&self, clip: &Clip) -> Option<Fraction> {
        let video = clip.video.as_ref()?;
        self.media.direct_combined_display_start(&video.path)?;
        match self.media.picture_format_override(&video.path) {
            Some("102") => Some(video.rate),
            Some("110") if video.rate == Fraction::new(24_000, 1001)
                && clip.audio.iter().all(|audio| audio.bits == 16) => Some(video.rate),
            _ => None,
        }
    }

    fn share_grouped_picture_markers(
        &mut self,
        clip: &Clip,
        item: ItemId,
        edit_markers: &[&Marker],
    ) -> Result<()> {
        let Some(video) = &clip.video else { return Ok(()) };
        // This collection is also visible on the audio templates. Only
        // expose it on picture when all of its markers belong to V1.
        if self.media.direct_combined_display_start(&video.path).is_none()
            || self.project.item_markers(item).is_empty()
            || clip.markers.iter().chain(edit_markers.iter().copied())
                .any(|marker| marker.track.as_deref() != Some("V1"))
        {
            return Ok(());
        }
        let data = self.project.item(item);
        let tree = &self.project.doc.tree;
        let audio = self.project.own_clip_core(item)?.ok_or_else(|| other("grouped audio clip is missing"))?;
        let reference = tree.find1(audio, "MarkerOwner/Markers").ok_or_else(|| other("grouped marker reference is missing"))?;
        let collection = tree.get(reference, "ObjectRef").ok_or_else(|| other("grouped marker collection has no ObjectRef"))?.to_owned();
        let mut pictures = data.clip_elements.iter().copied()
            .filter(|&node| tree.tag(node) == "VideoClip");
        let picture = pictures.next().ok_or_else(|| other("grouped picture clip is missing"))?;
        if pictures.next().is_some() {
            return Err(other("grouped markers have multiple picture clips"));
        }
        let core = crate::model::item::clip_core(tree, picture).ok_or_else(|| other("grouped picture clip core is missing"))?;
        if tree.child_by_tag(core, "MarkerOwner").is_some() {
            return Err(other("grouped picture clip already has a marker owner"));
        }
        let tree = &mut self.project.doc.tree;
        let owner = tree.new_element_with("MarkerOwner", &[("Version", "1")]);
        tree.set_text(owner, Some("\n\t\t\t\t"));
        let reference = tree.sub_element(owner, "Markers", &[("ObjectRef", &collection)]);
        tree.set_tail(reference, Some("\n\t\t\t"));
        crate::xml::mutations::insert_before(tree, core, "Source", owner)?;
        Ok(())
    }

    fn apply_metadata(&mut self, item: ItemId, clip: &Clip) -> Result<()> {
        metadata::apply(&mut self.project, item, clip.avid.as_ref())?;
        Ok(())
    }

    fn import_clip(&mut self, clip_id: &str, target: Option<ItemId>) -> Result<()> {
        let model = self.model;
        if let Some(c)=model.originals.get(clip_id) {
            let previous=std::mem::replace(&mut self.project.import_target,target);
            let imported=super::original::add_item(&mut self.project,c,&c.metadata.name,self.ids);
            self.project.import_target=previous;
            let item=imported?;
            metadata::apply(&mut self.project,item,c.metadata.avid.as_ref())?;
            if let Some(rgb)=c.metadata.avid.as_ref().and_then(|a|a.color_rgb16){let label=colors::nearest_label(rgb,&model.options.clip_labels);mk::set_label(&mut self.project,item,label)?;}
            self.original_markers(item, &c.metadata)?;
            self.items.insert(clip_id.into(),(c.video.as_ref().map(|_|item),vec![]));
            self.clip_bins.insert(clip_id.into(),target);
            self.placement.insert(clip_id.into(),("original",Payload::Original(item)));
            return Ok(());
        }
        let clip = &model.clips[clip_id];
        if self.native_av || self.linked_op1a.contains(clip_id) {
            let previous = std::mem::replace(&mut self.project.import_target, target);
            let imported = super::native_av::add_item(&mut self.project, &mut self.media, clip, &clip.name, self.ids);
            self.project.import_target = previous;
            let item = imported?;
            self.apply_metadata(item, clip)?;
            label_items(&mut self.project, clip, &[item], model.options)?;
            self.build_markers(clip, item, None)?;
            self.items.insert(clip_id.to_string(), (Some(item), Vec::new()));
            self.clip_bins.insert(clip_id.to_string(), target);
            self.placement.insert(clip_id.to_string(), ("native", Payload::Native(item)));
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.project.import_target, target);
        let imported = self.import_items(clip);
        self.project.import_target = previous;
        let (video, audio) = imported?;
        self.items.insert(clip_id.to_string(), (video, audio.clone()));
        self.clip_bins.insert(clip_id.to_string(), target);
        self.placement.insert(clip_id.to_string(), ("separate", Payload::Parts { video, audio: audio.clone(), stereo: Vec::new() }));
        let mut mode: &'static str = match model.options.bin_mode.as_str() {
            "merged" => "merged",
            "grouped" => "grouped",
            _ => "separate",
        };
        let name = py_str_repr(&clip.name);
        if mode == "merged" && !clip.stereo_pairs.is_empty() && clip.video.is_some() && target.is_some() {
            // Premiere cannot merge into stereo (Adobe); build the stereo pair as a group (User).
            mode = "grouped";
            self.report.notes.push(format!("clip {name}: Avid stereo pair; built as a group instead of merged"));
        }
        let combinable = match (target, video) {
            (Some(target), Some(video)) if mode != "separate" && clip.video.is_some() && !clip.audio.is_empty() => Some((target, video)),
            _ => None,
        };
        if let Some((target, video)) = combinable {
            let combined: Vec<ItemId> = combined_order(clip)?.into_iter().map(|i| audio[i]).collect();
            if mode == "merged" {
                let (merged_item, video_copy, audio_copies) =
                    create_merged(&mut self.project, &mut self.media, clip, video, &combined, target, &clip.name, self.ids)?;
                self.apply_metadata(merged_item, clip)?;
                let mut labelled = vec![merged_item, video_copy];
                labelled.extend(&audio_copies);
                label_items(&mut self.project, clip, &labelled, model.options)?;
                if !clip.markers.is_empty() {
                    self.build_markers(clip, merged_item, None)?;
                    if clip.video.as_ref().is_some_and(|v| self.media.direct_combined_display_start(&v.path).is_some()) {
                        self.part_markers(clip, video_copy, "V1".into())?;
                    }
                    let audio_clock = self.merged_audio_marker_clock(clip);
                    if audio_clock.is_some() {
                        for (index, &copy) in combined_order(clip)?.iter().zip(&audio_copies) {
                            if let Some(track) = clip.audio[*index].avid_track {
                                self.part_markers(clip, copy, format!("A{track}"))?;
                            }
                        }
                    }
                    let verified_source_markers = clip.video.as_ref().is_some_and(|video| {
                        self.media.direct_combined_display_start(&video.path).is_some()
                            && self.media.picture_format_override(&video.path) == Some("102")
                    }) && clip.markers.iter().all(|marker| marker.track.as_deref() == Some("V1"));
                    let verified_audio_ranges = audio_clock == Some(Fraction::new(24_000, 1001))
                        && clip.markers.iter().all(|marker|
                            marker.duration > 0 && matches!(marker.track.as_deref(), Some("A1" | "A2")));
                    if verified_audio_ranges {
                        self.report.notes.push(format!(
                            "clip {name}: Premiere can list each A1/A2 source range twice in the Merged Source Monitor. Timeline markers stay on their audio components."
                        ));
                    } else if !verified_source_markers {
                        self.report.unverified.push(format!("clip {name}: markers on a merged clip (time base unverified)"));
                    }
                }
                self.placement.insert(
                    clip_id.to_string(),
                    ("merged", Payload::Parts { video: Some(video_copy), audio: audio_copies, stereo: Vec::new() }),
                );
            } else {
                let grouped_item = create_grouped(&mut self.project, clip, Some(video), &combined, target, &clip.name, Some(&self.media), self.ids)?;
                self.apply_metadata(grouped_item, clip)?;
                label_items(&mut self.project, clip, &[grouped_item], model.options)?;
                self.build_markers(clip, grouped_item, None)?;
                self.share_grouped_picture_markers(clip, grouped_item, &[])?;
                self.placement.insert(clip_id.to_string(), ("grouped", Payload::Grouped(grouped_item)));
            }
        } else if let (Some(target), false) = (target, clip.stereo_pairs.is_empty()) {
            // No combined clip (Separate mode, or audio-only): each Avid stereo pair becomes an
            // audio-only stereo build; its two mono files become its parts.
            let mut stereo = Vec::new();
            let only_pair = clip.video.is_none() && clip.audio.len() == 2;
            for &(left, right) in &clip.stereo_pairs {
                let mut pair_items = Vec::new();
                for t in [left, right] {
                    let Some(at) = clip.audio.iter().position(|f| f.avid_track == Some(t)) else {
                        return Err(other("StopIteration"));
                    };
                    pair_items.push(audio[at]);
                }
                let build_name = if only_pair { clip.name.clone() } else { format!("{}_A{left:02}A{right:02}", clip.name) };
                let rate = clip.rate.unwrap_or(Fraction::new(24, 1));
                let preset = preset_bound(&build_name, rate, 1920, 1080, &mut self.report, self.rewrapped)?;
                let build = create_stereo_nest(&mut self.project, &pair_items, target, &build_name, &preset, self.ids)?;
                self.apply_metadata(build, clip)?;
                label_items(&mut self.project, clip, &[build], model.options)?;
                self.build_markers(clip, build, Some(&[format!("A{left}"), format!("A{right}")]))?;
                stereo_set(&mut stereo, vec![left, right], build);
            }
            self.placement.insert(clip_id.to_string(), ("separate", Payload::Parts { video, audio, stereo }));
            if mode != "separate" {
                self.report.notes.push(format!("clip {name}: no picture to {mode}; stereo pair built as a stereo audio clip"));
            }
        } else if mode != "separate" {
            self.report
                .notes
                .push(format!("clip {name}: not {mode} (needs video and audio, and a bin); placed as separate clips"));
        }
        if clip.material_umid.is_none() {
            self.report.notes.push(format!("clip {name}: no material UMID; ClipID left empty"));
        }
        Ok(())
    }

    /// The import part of `import_clip` (inside Python's `try`/`finally`).
    fn import_items(&mut self, clip: &Clip) -> Result<(Option<ItemId>, Vec<ItemId>)> {
        let (video_name, audio_names) = source_item_names(clip);
        let video = match &clip.video {
            Some(v) => Some(add_video_item(&mut self.project, &mut self.media, v, clip, Some(&video_name), self.ids)?),
            None => None,
        };
        let mut audio = Vec::new();
        for (a, n) in clip.audio.iter().zip(&audio_names) {
            audio.push(add_audio_item(&mut self.project, &mut self.media, a, clip, Some(n), self.ids)?);
        }
        for &item in video.iter().chain(&audio) {
            self.apply_metadata(item, clip)?;
        }
        let labelled: Vec<ItemId> = video.iter().chain(&audio).copied().collect();
        label_items(&mut self.project, clip, &labelled, self.model.options)?;
        if let Some(video) = video {
            self.part_markers(clip, video, "V1".to_string())?;
        }
        for (&item, f) in audio.iter().zip(&clip.audio) {
            let track = match f.avid_track {
                Some(t) => t.to_string(),
                None => "None".to_string(),
            };
            self.part_markers(clip, item, format!("A{track}"))?;
        }
        if clip.audio.iter().any(|a| a.avid_track.is_none()) {
            self.report.notes.push(format!("clip {}: audio without an Avid track was numbered by model position", py_str_repr(&clip.name)));
        }
        Ok((video, audio))
    }

    /// `settle`: Avid has both a source marker (copied in at cut-in) and the edit's own marker
    /// on one frame: the winner stays on the clip; the other is dropped or goes to the sequence
    /// (options).
    fn settle<'m>(
        &mut self,
        e: &Event,
        source_ms: &[&'m Marker],
        own_ms: &[&'m Marker],
        extra: &mut Vec<Extra>,
        where_: &str,
    ) -> Result<(Vec<&'m Marker>, Vec<&'m Marker>)> {
        let options = &self.model.options;
        let timeline = |src: i64| -> Result<i64> {
            let mut along = i128::from(src) - i128::from(e.source_in);
            if let Some((t, s)) = e.speed_ratio {
                if s == 0 {
                    return Err(other("integer division or modulo by zero"));
                }
                along = (along * i128::from(t).abs()).div_euclid(i128::from(s));
            }
            fit(i128::from(e.record_start) + along)
        };
        let original = self.model.originals.get(e.clip.as_str()).copied();
        let grouped = self.model.clips.get(&e.clip).filter(|clip| {
            matches!(self.placement.get(&e.clip), Some(("grouped", _)))
                && clip.video.as_ref().is_some_and(|video|
                    self.media.direct_combined_display_start(&video.path).is_some())
        });
        let grouped_audio = grouped.map(|clip| event_audio_files(clip, e)).transpose()?;
        let conflicts = |source: &Marker, edit: &Marker| {
            if edit.source != Some(source.start) {
                return false;
            }
            if let Some(clip) = original {
                return mk::lanes_overlap(
                    e, source, edit, clip.video.is_some(),
                    |index| clip.channels.iter().any(|channel| channel.avid_track == index),
                    |slot| super::original::mono_source_channel(clip, e, slot),
                );
            }
            if let (Some(clip), Some(selected)) = (grouped, grouped_audio.as_ref()) {
                return mk::lanes_overlap(
                    e, source, edit, true,
                    |index| clip.audio.iter().any(|file| file.avid_track == Some(index)),
                    |slot| selected.get(slot).and_then(|file| file.avid_track),
                );
            }
            true
        };
        let edit_wins = options.marker_conflict_winner == "edit";
        let (mut keep_source, mut keep_own) = (Vec::new(), Vec::new());
        for &m in source_ms {
            if edit_wins && own_ms.iter().any(|edit| conflicts(m, edit)) {
                if options.marker_conflict_loser_to_sequence {
                    extra.push((m.clone(), timeline(m.start)?, "clip marker replaced on this edit".to_string()));
                }
            } else {
                keep_source.push(m);
            }
        }
        for &o in own_ms {
            if !edit_wins && source_ms.iter().any(|source| conflicts(source, o)) {
                if options.marker_conflict_loser_to_sequence {
                    extra.push((o.clone(), o.start, "edit marker under a clip marker".to_string()));
                }
            } else {
                keep_own.push(o);
            }
        }
        if keep_source.len() + keep_own.len() < source_ms.len() + own_ms.len() {
            let tail = if options.marker_conflict_loser_to_sequence { ", the other placed on the sequence" } else { "" };
            self.report.notes.push(format!(
                "{where_}: an edit marker and a clip marker share a frame; the {} marker is kept{tail}",
                options.marker_conflict_winner
            ));
        }
        Ok((keep_source, keep_own))
    }

    /// `marked`: the settled source and own markers on an edit copy.
    fn marked(&mut self, item: ItemId, source_ms: &[&Marker], own_ms: &[&Marker], base: i64, e: &Event, extra: &mut Vec<Extra>) -> Result<usize> {
        let where_ = py_str_repr(&self.project.item_name(item));
        let (keep_source, keep_own) = self.settle(e, source_ms, own_ms, extra, &where_)?;
        let source_count = keep_source.len();
        for m in keep_source {
            self.add_clip_marker(item, m, add(base, m.start)?)?;
        }
        for o in keep_own {
            let Some(source) = o.source else {
                return Err(other("unsupported operand type(s) for +: 'int' and 'NoneType'"));
            };
            self.add_clip_marker(item, o, add(base, source)?)?;
        }
        Ok(source_count)
    }

    fn save_edit_marker_provenance(
        &mut self,
        copy: ItemId,
        parent: ItemId,
        sequence: SequenceId,
        position: (usize, usize),
        edit: &Event,
        source_count: usize,
    ) -> Result<()> {
        let tree = &self.project.doc.tree;
        let marks = &self.project.item_markers(copy)[source_count..];
        if marks.is_empty() || marks.len() > 128 {
            return Ok(());
        }
        let parent = tree.get(self.project.item(parent).element, "ObjectUID")
            .ok_or_else(|| other("edit marker original lacks ObjectUID"))?;
        let sequence = self.project.sequence_id(sequence)
            .ok_or_else(|| other("edit marker sequence lacks ObjectUID"))?;
        let mut ids = Vec::with_capacity(marks.len());
        let mut budget = mk::MAX_MARKER_CUSTOM_DATA;
        let mut version = 1;
        let (sequence_index, event) = position;
        for &mark in marks {
            let mut guid = crate::model::marker::marker_guid(tree, mark)?;
            let key = MarkerDataKey::Edit(sequence_index, event, crate::model::marker::marker_start(tree, mark)?.ticks);
            if let Some(rows) = self.model.marker_provenance {
                let custom = rows.custom_data.get(&key)
                    .filter(|data| !data.is_empty() && data.len() <= budget);
                let color = rows.original_colors.get(&key);
                if custom.is_some() || color.is_some() {
                    guid.push(':');
                    if let Some(data) = custom {
                        guid.push_str(&crate::hash::hex(data.as_bytes()));
                        budget -= data.len();
                        self.saved_marker_data.insert(key.clone());
                        version = version.max(2);
                    }
                    if let Some(bit) = color {
                        let preview = crate::model::marker::marker_color_index(tree, mark)?
                            .ok_or_else(|| other("marker fallback color is outside the Premiere palette"))?;
                        guid.push_str(&format!(":{bit}:{preview}"));
                        self.saved_marker_colors.insert(key);
                        version = 3;
                    }
                }
            }
            ids.push(guid);
        }
        let data = format!("{version} {parent} {sequence} {} {} {} {}", edit.record_start,
            edit.source_in, edit.duration, ids.join(","));
        let props = tree.find1(self.project.item(copy).element, "ProjectItem/Node/Properties")
            .ok_or_else(|| other("edit marker item lacks a property bag"))?;
        crate::xml::mutations::append_leaf(&mut self.project.doc.tree, props, mk::EDIT_MARKER_PROVENANCE, &data);
        Ok(())
    }

    /// `edit_copy`: Premiere keeps markers on the master clip, so an edit with markers of its own
    /// plays a copy of the clip carrying the source markers plus its own (User decision
    /// 2026-10-15). Returns the payload this edit is placed from.
    #[allow(clippy::too_many_arguments)]
    fn edit_copy(
        &mut self,
        seq: &Sequence,
        sequence: SequenceId,
        position: (usize, usize),
        e: &Event,
        kind: &str,
        payload: Payload,
        own: &[&Marker],
        extra: &mut Vec<Extra>,
    ) -> Result<Payload> {
        let (sequence_index, i) = position;
        let model = self.model;
        if let Some(c)=model.originals.get(e.clip.as_str()) {
            let target=self.clip_bins.get(&e.clip).copied().flatten().unwrap_or(self.root);
            let name=format!("{} [edit {}]",c.metadata.name,timecode(e.record_start,seq.rate,seq.start)?);
            let previous=self.project.import_target.replace(target);
            let copy=super::original::add_item(&mut self.project,c,&name,self.ids);
            self.project.import_target=previous;let copy=copy?;
            metadata::apply(&mut self.project,copy,c.metadata.avid.as_ref())?;
            let source_count = self.marked(copy, &c.metadata.markers.iter().collect::<Vec<_>>(), own, 0, e, extra)?;
            if model.marker_provenance.is_some() && e.video_track == Some(1) && e.audio_tracks.is_empty()
                && e.speed_ratio.is_none() && e.time_remap.is_none() && c.audio.is_none()
                && matches!((seq.rate.num, seq.rate.den), (24, 1) | (24000, 1001))
                && c.video.as_ref().is_some_and(|v| v.rate == seq.rate
                    && v.codec == "apch" && (v.width, v.height) == (1920, 1080))
            {
                let Payload::Original(parent) = payload else {
                    return Err(other("edit marker copy has no original item"));
                };
                self.save_edit_marker_provenance(copy, parent, sequence, position, e, source_count)?;
            }
            move_to_processed_clips(&mut self.project,&[copy],Some(target),self.ids)?;
            return Ok(Payload::Original(copy));
        }
        let clip = &model.clips[e.clip.as_str()];
        let target = self.clip_bins.get(&e.clip).copied().flatten().unwrap_or(self.root);
        let name = format!("{} [edit {}]", clip.name, timecode(e.record_start, seq.rate, seq.start)?);
        let (video, audio) = self.items[&e.clip].clone();
        if kind == "native" {
            let previous = self.project.import_target.replace(target);
            let copy = super::native_av::add_item(&mut self.project, &mut self.media, clip, &name, self.ids);
            self.project.import_target = previous;
            let copy = copy?;
            self.apply_metadata(copy, clip)?;
            label_items(&mut self.project, clip, &[copy], model.options)?;
            self.marked(copy, &clip.markers.iter().collect::<Vec<_>>(), own, 0, e, extra)?;
            move_to_processed_clips(&mut self.project, &[copy], Some(target), self.ids)?;
            self.report.notes.push(format!("sequence {} event {i}: edit markers; placed from its own copy {}", py_str_repr(&seq.name), py_str_repr(&name)));
            return Ok(Payload::Native(copy));
        }
        if kind == "merged" {
            let mut payload = payload;
            let mut own = own.to_vec();
            if self.merged_audio_marker_clock(clip) == Some(seq.rate)
                && e.speed_ratio.is_none() && e.time_remap.is_none()
                && own.iter().any(|m| m.track.as_deref().is_some_and(|t| t.starts_with('A')))
            {
                let Payload::Parts { audio: copies, .. } = &mut payload else {
                    return Err(other("merged edit is missing its audio components"));
                };
                let files = event_audio_files(clip, e)?;
                let order = combined_order(clip)?;
                for (&track, &file) in e.audio_tracks.iter().zip(&files) {
                    let Some(label) = file.avid_track else { continue };
                    // A component copy is shared by every use of that file within
                    // this event. Preserve lane ownership only for a unique use.
                    if files.iter().filter(|f| f.avid_track == Some(label)).count() != 1 {
                        continue;
                    }
                    let lane = format!("A{track}");
                    let markers: Vec<_> = own.iter().copied().filter(|m|
                        m.track.as_deref() == Some(lane.as_str()) && m.source.is_some()).collect();
                    if markers.is_empty() { continue; }
                    let index = order.iter().position(|&i| clip.audio[i].avid_track == Some(label))
                        .ok_or_else(|| other("merged edit audio has no matching component"))?;
                    let parts = parts_bin(&mut self.project, target, self.ids)?;
                    let previous = self.project.import_target.replace(parts);
                    let copy = add_audio_item(&mut self.project, &mut self.media, file, clip, Some(&format!("{name}_A{label:02}")), self.ids);
                    self.project.import_target = previous;
                    let copy = copy?;
                    let source_name = self.project.item_name(audio[order[index]]);
                    tag_merged_audio(&mut self.project, copy, &clip.name, &source_name, index)?;
                    self.apply_metadata(copy, clip)?;
                    label_items(&mut self.project, clip, &[copy], model.options)?;
                    self.marked(copy, &scoped(&clip.markers, Some(&[format!("A{label}")])), &markers, 0, e, extra)?;
                    copies[index] = copy;
                    own.retain(|m| m.track.as_deref() != Some(lane.as_str()) || m.source.is_none());
                    self.report.notes.push(format!(
                        "sequence {} event {i}: {lane} edit markers kept on a dedicated merged audio component",
                        py_str_repr(&seq.name)
                    ));
                }
            }
            if let Some(v) = &clip.video {
                if self.media.direct_combined_display_start(&v.path).is_some()
                    && e.video_track == Some(1) && seq.rate == v.rate
                    && e.speed_ratio.is_none() && e.time_remap.is_none()
                {
                    let is_picture = |m: &Marker| m.track.as_deref() == Some("V1") && m.source.is_some();
                    let markers: Vec<_> = own.iter().copied().filter(|m| is_picture(m)).collect();
                    if !markers.is_empty() {
                        let Payload::Parts { video: video @ Some(_), .. } = &mut payload else {
                            return Err(other("merged edit is missing its picture component"));
                        };
                        let parts = parts_bin(&mut self.project, target, self.ids)?;
                        let previous = self.project.import_target.replace(parts);
                        let copy = add_video_item(&mut self.project, &mut self.media, v, clip, Some(&format!("{name}_V")), self.ids);
                        self.project.import_target = previous;
                        let copy = copy?;
                        let (original_name, _) = source_item_names(clip);
                        tag_merged_picture(&mut self.project, copy, &clip.name, &original_name)?;
                        self.apply_metadata(copy, clip)?;
                        label_items(&mut self.project, clip, &[copy], model.options)?;
                        self.marked(copy, &scoped(&clip.markers, Some(&["V1".into()])), &markers, 0, e, extra)?;
                        *video = Some(copy);
                        own.retain(|m| !is_picture(m));
                        self.report.notes.push(format!(
                            "sequence {} event {i}: V1 edit markers kept on a dedicated merged picture component",
                            py_str_repr(&seq.name)
                        ));
                    }
                }
            }
            for o in own {
                let track = o.track.clone().unwrap_or_else(|| "None".to_string());
                extra.push(((*o).clone(), o.start, format!("on this {track} edit (merged clips cannot carry per-edit markers yet)")));
            }
            return Ok(payload);
        }
        if kind == "grouped" {
            let combined: Vec<ItemId> = combined_order(clip)?.into_iter().map(|j| audio[j]).collect();
            let copy = create_grouped(&mut self.project, clip, video, &combined, target, &name, Some(&self.media), self.ids)?;
            self.apply_metadata(copy, clip)?;
            label_items(&mut self.project, clip, &[copy], model.options)?;
            let all: Vec<&Marker> = clip.markers.iter().collect();
            let is_picture = |m: &Marker| m.track.as_deref() == Some("V1");
            let known_lanes = clip.markers.iter().chain(own.iter().copied()).all(|m| {
                is_picture(m) || m.track.as_deref().is_some_and(|track| track.starts_with('A'))
            });
            let mixed_lanes = own.iter().any(|m| is_picture(m))
                && clip.markers.iter().chain(own.iter().copied()).any(|m| !is_picture(m));
            let direct_picture = clip.video.as_ref().is_some_and(|v| {
                self.media.direct_combined_display_start(&v.path).is_some() && seq.rate == v.rate
            }) && e.video_track == Some(1) && e.speed_ratio.is_none() && e.time_remap.is_none();
            if direct_picture && known_lanes && mixed_lanes {
                // The shared Grouped collection is only qualified on V1 when
                // it contains picture markers alone. Settle picture conflicts
                // independently and keep the surviving edit ranges at record time.
                let (source_picture, source_audio): (Vec<_>, Vec<_>) =
                    all.into_iter().partition(|m| is_picture(m));
                let (edit_picture, edit_audio): (Vec<_>, Vec<_>) =
                    own.iter().copied().partition(|m| is_picture(m));
                let (keep_source, keep_edit) = self.settle(e, &source_picture, &edit_picture, extra, &name)?;
                self.marked(copy, &source_audio, &edit_audio, 0, e, extra)?;
                for m in keep_source {
                    self.add_clip_marker(copy, m, m.start)?;
                }
                for m in keep_edit {
                    extra.push((m.clone(), m.start, "on this V1 edit (Grouped clip also carries audio markers)".into()));
                    self.report.grouped_picture_marker_moves.push(GroupedPictureMarkerMove {
                        sequence: sequence_index,
                        record_ticks: m.start,
                        marker_name: m.name.clone(),
                    });
                }
            } else {
                self.marked(copy, &all, own, 0, e, extra)?;
                self.share_grouped_picture_markers(clip, copy, own)?;
            }
            move_to_processed_clips(&mut self.project, &[copy], Some(target), self.ids)?;
            self.report.notes.push(format!(
                "sequence {} event {i}: edit markers; placed from its own copy {}",
                py_str_repr(&seq.name),
                py_str_repr(&name)
            ));
            return Ok(Payload::Grouped(copy));
        }
        let Payload::Parts { video: mut video_item, audio: mut audio_items, stereo: mut stereo_builds } = payload else {
            return Err(other("cannot unpack non-iterable ProjectItem object"));
        };
        let parts = parts_bin(&mut self.project, target, self.ids)?;
        let files = event_audio_files(clip, e)?;
        let mut stereo_tracks: Vec<(i64, Vec<i64>)> = Vec::new();
        for st in &e.stereo {
            match stereo_tracks.iter_mut().find(|(t, _)| *t == st.audio_track) {
                Some(slot) => slot.1 = st.channels.clone(),
                None => stereo_tracks.push((st.audio_track, st.channels.clone())),
            }
        }
        let previous = self.project.import_target.replace(parts);
        let result = (|| -> Result<()> {
            let mut tracks: Vec<&String> = Vec::new();
            for o in own {
                let Some(t) = &o.track else { return Err(other("'<' not supported between instances of 'NoneType' and 'str'")) };
                if !tracks.contains(&t) {
                    tracks.push(t);
                }
            }
            tracks.sort();
            for track in tracks {
                let mine: Vec<&Marker> = own.iter().copied().filter(|o| o.track.as_ref() == Some(track)).collect();
                let number = py_int(track.get(1..).unwrap_or(""))?;
                let stereo_pair = stereo_tracks.iter().rev().find(|(t, _)| *t == number).map(|(_, c)| c.clone());
                if track.starts_with('V') && Some(number) == e.video_track && video_item.is_some() {
                    let Some(v) = &clip.video else { return Err(other("'NoneType' object has no attribute 'rate'")) };
                    let copy = add_video_item(&mut self.project, &mut self.media, v, clip, Some(&format!("{name}_V")), self.ids)?;
                    self.apply_metadata(copy, clip)?;
                    label_items(&mut self.project, clip, &[copy], model.options)?;
                    self.marked(copy, &scoped(&clip.markers, Some(&["V1".to_string()])), &mine, 0, e, extra)?;
                    video_item = Some(copy);
                } else if track.starts_with('A') && e.audio_tracks.contains(&number) {
                    let at = e.audio_tracks.iter().position(|&t| t == number).expect("contained");
                    let f = *files.get(at).ok_or_else(|| other("list index out of range"))?;
                    let index = clip.audio.iter().position(|g| g == f).expect("file of this clip");
                    let avid_track = f.avid_track.unwrap_or(index as i64 + 1);
                    let copy = add_audio_item(
                        &mut self.project,
                        &mut self.media,
                        f,
                        clip,
                        Some(&format!("{name}_A{avid_track:02}")),
                        self.ids,
                    )?;
                    self.apply_metadata(copy, clip)?;
                    label_items(&mut self.project, clip, &[copy], model.options)?;
                    self.marked(copy, &scoped(&clip.markers, Some(&[format!("A{avid_track}")])), &mine, 0, e, extra)?;
                    *audio_items.get_mut(index).ok_or_else(|| other("list assignment index out of range"))? = copy;
                } else if track.starts_with('A') && stereo_pair.as_ref().is_some_and(|p| stereo_get(&stereo_builds, p).is_some()) {
                    let pair = stereo_pair.expect("checked");
                    let original_audio = &self.items[&e.clip].1;
                    let mut pair_items = Vec::new();
                    for &t in &pair {
                        let Some(at) = clip.audio.iter().position(|f| f.avid_track == Some(t)) else {
                            return Err(other("StopIteration"));
                        };
                        pair_items.push(original_audio[at]);
                    }
                    let rate = clip.rate.unwrap_or(Fraction::new(24, 1));
                    let preset = preset_bound(&name, rate, 1920, 1080, &mut self.report, self.rewrapped)?;
                    let (Some(first), Some(second)) = (pair.first(), pair.get(1)) else {
                        return Err(other("tuple index out of range"));
                    };
                    let copy = create_stereo_nest(
                        &mut self.project,
                        &pair_items,
                        target,
                        &format!("{name}_A{first:02}A{second:02}"),
                        &preset,
                        self.ids,
                    )?;
                    self.apply_metadata(copy, clip)?;
                    label_items(&mut self.project, clip, &[copy], model.options)?;
                    let pair_tracks: Vec<String> = pair.iter().map(|t| format!("A{t}")).collect();
                    self.marked(copy, &scoped(&clip.markers, Some(&pair_tracks)), &mine, 0, e, extra)?;
                    move_to_processed_clips(&mut self.project, &[copy], Some(target), self.ids)?;
                    stereo_set(&mut stereo_builds, pair, copy);
                } else {
                    for o in &mine {
                        extra.push(((*o).clone(), o.start, format!("on {track}")));
                    }
                    continue;
                }
                self.report.notes.push(format!(
                    "sequence {} event {i}: {track} edit markers; placed from its own copy {}",
                    py_str_repr(&seq.name),
                    py_str_repr(&name)
                ));
            }
            Ok(())
        })();
        self.project.import_target = previous;
        result?;
        Ok(Payload::Parts { video: video_item, audio: audio_items, stereo: stereo_builds })
    }

    fn nest_picture(&mut self, seq: &Sequence, edit: &Event, event: usize, source: ItemId, preset: &str) -> Result<ItemId> {
        // Collapse stores a trimmed inline input, so create a child for just
        // the saved interval. The child clock labels its parent position.
        let name = format!("{} — Submaster {}", seq.name, event + 1);
        let child = self.picture_child(&name, preset, seq, edit.record_start)?;
        let track = track_at(self.project.video_tracks(child), 1)?;
        self.place_picture_slice(track, source, 0, edit.source_in, edit.duration)?;
        // add_clip starts with the whole file's extent. Reset the child
        // source documents to the actual saved interval before nesting it.
        self.project.set_nested_source_duration(child, edit.duration)?;
        self.project.sequence_item(child).ok_or_else(|| other("nest has no project-panel item"))
    }

    fn picture_child(&mut self, name: &str, preset: &str, seq: &Sequence, start: i64) -> Result<SequenceId> {
        let child = self.project.add_sequence(name, preset, None, self.ids)?;
        if let Some(parent) = self.sequence_parent {
            let item = self.project.sequence_item(child).ok_or_else(|| other("nest has no project-panel item"))?;
            self.project.move_item(item, parent)?;
        }
        self.project.set_zero_point(child, Time::new(add(seq.start, start)?))?;
        Ok(child)
    }

    fn place_picture_slice(&mut self, track: TrackId, source: ItemId, start: i64, source_in: i64, duration: i64) -> Result<()> {
        let item = self.project.add_clip(track, source, Time::new(start), self.ids)?;
        self.project.set_in_point(item, Time::new(source_in))?;
        self.project.set_out_point(item, Time::new(add(source_in, duration)?))?;
        self.project.set_end(item, Time::new(add(start, duration)?))?;
        normalize_placement(&mut self.project, item, None)?;
        Ok(())
    }

    fn write_layered_submaster(&mut self, seq: &Sequence, nest: &PreparedLayeredSubmaster, preset: &str, target: TrackId) -> Result<()> {
        let name = format!("{} — Layered Submaster V{} at frame {}", seq.name, nest.track,
            nest.start / ticks_per_unit(seq.rate));
        let child = self.picture_child(&name, preset, seq, nest.start)?;
        for (lane, layer) in nest.layers.iter().enumerate() {
            let Some((_, Payload::Parts { video: Some(source), .. })) = self.placement.get(&layer.clip) else {
                return Err(other("Submaster picture source is unavailable"));
            };
            let track = track_at(self.project.video_tracks(child), lane as i64 + 1)?;
            self.place_picture_slice(track, *source, layer.start, layer.source_in, layer.duration)?;
        }
        self.project.set_nested_source_duration(child, nest.duration)?;
        let source = self.project.sequence_item(child).ok_or_else(|| other("nest has no project-panel item"))?;
        self.place_picture_slice(target, source, nest.start, 0, nest.duration)
    }

    fn write_sequence(&mut self, seq: &Sequence, sequence_index: usize) -> Result<()> {
        let model = self.model;
        let options = &model.options;
        let seq_name = py_str_repr(&seq.name);
        let non_drop = model.non_drop_sequences.contains(&sequence_index);
        let expanded_groups = || model.multicam_tracks.iter().filter(|group| group.sequence == sequence_index);
        let mut preset = preset_bound(&seq.name, seq.rate, seq.width, seq.height, &mut self.report, self.rewrapped)
            .or_else(|error| {
                let mut picture = seq.events.iter().filter(|event| event.video_track.is_some()).peekable();
                if self.rewrapped || (expanded_groups().next().is_none() && picture.peek().is_none()) || !picture.all(|event| {
                    model.originals.get(event.clip.as_str()).is_some_and(|clip|
                        clip.matches_explicit_linked_raster(seq))
                }) {
                    return Err(error);
                }
                let drop = seq.rate == Fraction::new(30_000, 1001) && !non_drop;
                let display = super::rewrap::display_format(seq.rate, drop).ok_or(error)?;
                self.report.notes.push(format!(
                    "sequence {seq_name}: explicit linked raster {}x{} at {} fps",
                    seq.width, seq.height, seq.rate));
                Ok(format!("anynle-linked/{}/{}/{}/{display}", ticks_per_unit(seq.rate), seq.width, seq.height))
            })?;
        if self.native_av {
            let width=model.native_sequence_channels(seq)?;
            if width>0 {
                // Wrap the selected destination preset. Audio grouping must not
                // re-derive its display clock (e.g. change existing DF to NDF).
                preset = format!("anynle-grouped/{width}/{preset}");
            }
        }
        let named_empty_tracks = model.track_states.get(&sequence_index)
            .into_iter().flatten().filter(|state| state.create_empty);
        let needed = seq.events.iter().flat_map(|e| e.audio_tracks.iter().copied().chain(e.stereo.iter().map(|s| s.audio_track)))
            .chain(expanded_groups().flat_map(|group| &group.uses)
                .filter(|edit| edit.angle.is_none()).map(|edit| edit.track))
            .chain(seq.track_levels.iter().map(|t| t.track))
            .chain(seq.audio_crossfades.iter().map(|f| f.track))
            .chain(named_empty_tracks.clone().filter_map(|state| match state.lane {
                PreparedTrackLane::Audio(track) => Some(track),
                _ => None,
            }))
            .max().unwrap_or(0) as usize;
        let base = crate::build::sequence::format_by_name(&preset).ok_or_else(|| UnverifiedShape("unknown sequence preset".into()))?;
        if needed > base.audio_tracks {
            if base.mono_discrete { return Err(UnverifiedShape("expanding a mono-discrete output bus is not qualified".into()).into()); }
            preset = format!("anynle-tracks/{needed}/{preset}");
            self.report.notes.push(format!("sequence {seq_name}: expanded audio track capacity to {needed}; output bus unchanged"));
            self.report.unverified.push(format!("sequence {seq_name}: expanded audio-track count requires fresh native Premiere read-back; structural checks do not qualify playback"));
        }
        let picture_tracks = seq.events.iter().filter_map(|event| event.video_track)
            .chain(expanded_groups().flat_map(|group| &group.uses)
                .filter(|edit| edit.angle.is_some()).map(|edit| edit.track))
            .chain(model.layered_submasters.iter().filter(|nest| nest.sequence == sequence_index).map(|nest| nest.track))
            .chain(model.texts.iter()
                .filter(|text| text.sequence == sequence_index && text.kind == super::prepared::PreparedTextKind::Graphic)
                .map(|text| text.track))
            .chain(seq.markers.iter()
                .filter(|marker| marker.owner == "filler")
                .filter_map(|marker| marker.track.as_deref()?.strip_prefix('V')?.parse::<i64>().ok()))
            .chain(named_empty_tracks.filter_map(|state| match state.lane {
                PreparedTrackLane::Picture(track) => Some(track),
                _ => None,
            }))
            .max().unwrap_or(0) as usize;
        if picture_tracks > base.video_tracks {
            self.report.notes.push(format!("sequence {seq_name}: expanded video track capacity to {picture_tracks}"));
        }
        let mixer = model.audio_mixers.get(&sequence_index);
        let track_pans = mixer.and_then(|mixer| mixer.mono_track_pans);
        let sequence = if track_pans.is_some() || non_drop {
            let mut format = crate::build::sequence::format_by_name(&preset)
                .ok_or_else(|| UnverifiedShape("unknown sequence preset".into()))?;
            if let Some(pans) = track_pans {
                if format.mono_discrete || format.grouped_channels != 0 || format.audio_tracks < 2 {
                    return Err(UnverifiedShape("track pan requires two mono inputs and a stereo mix".into()).into());
                }
                format.mono_stereo_inputs = pans.map(|pan| pan.is_some());
            }
            if non_drop { format.time_display = 103; }
            format.video_tracks = format.video_tracks.max(picture_tracks);
            self.project.add_sequence_with_format(&seq.name, &format, self.ids)?
        } else {
            self.project.add_sequence(&seq.name, &preset, (picture_tracks > base.video_tracks).then_some(picture_tracks), self.ids)?
        };
        self.project.defer_sequence_extent(sequence);
        if let Some(parent) = self.sequence_parent {
            let item = self.project.sequence_item(sequence).ok_or_else(|| other("sequence has no project-panel item"))?;
            self.project.move_item(item, parent)?;
        }
        let mut extra_markers: Vec<Extra> = Vec::new();
        if seq.start != 0 {
            self.project.set_zero_point(sequence, Time::new(seq.start))?;
        }
        let vtracks = self.project.video_tracks(sequence).to_vec();
        let atracks = self.project.audio_tracks(sequence).to_vec();
        for nest in model.layered_submasters.iter().filter(|nest| nest.sequence == sequence_index) {
            self.write_layered_submaster(seq, nest, &preset, track_at(&vtracks, nest.track)?)?;
        }
        if let Some(edits) = self.png_edits.get(&sequence_index) {
            png_mattes::place(
                &mut self.project, sequence, seq, &preset, self.sequence_parent,
                &self.model.png_mattes.stills, &self.png_items, edits, self.ids,
            )?;
        }
        let mut captions = Vec::new();
        for t in model.texts.iter().filter(|t| t.sequence == sequence_index) {
            match t.kind {
                super::prepared::PreparedTextKind::Graphic => {
                    let destination_text = t.content.replace('\n', "\r");
                    let added = self.project.add_graphic(vtracks[t.track as usize - 1], &destination_text, Some(Time::new(t.start)), Some(Time::new(t.duration)), "Translated Avid Text", (0.1,0.35), self.ids)?;
                    // The compatibility primitive's retained template has a 1.8 pt
                    // LucidaConsole override. Use a readable, explicit destination
                    // default here; this is not a claim to preserve Avid styling.
                    let param = self.project.doc.by_object_id[&added.param_ids[0]];
                    let value = self.project.doc.tree.child_by_tag(param, "StartKeyframeValue").ok_or_else(|| other("graphic text payload missing"))?;
                    let payload = self.project.doc.payload(value)?.ok_or_else(|| other("graphic text payload empty"))?;
                    let payload = crate::caption::flat::write_font_family(&payload, "ArialMT").map_err(|e| other(e.to_string()))?;
                    let payload = crate::caption::flat::write_font_size(&payload, 48.0).map_err(|e| other(e.to_string()))?;
                    crate::payload::set_payload(&mut self.project.doc, value, &payload);
                },
                super::prepared::PreparedTextKind::Caption => captions.push(crate::caption::builder::SrtCue {start:t.start,end:t.start+t.duration,text:t.content.clone()}),
            }
        }
        if !captions.is_empty() {
            captions.sort_by_key(|c| c.start);
            self.project.add_caption_track(sequence, &captions, self.ids).map_err(|e| other(e.to_string()))?;
        }
        if model.texts.iter().any(|t| t.sequence == sequence_index) {
            self.report.notes.push(format!("sequence {seq_name}: Avid text content and record spans translated; destination text appearance defaults used"));
            self.report.unverified.push(format!("sequence {seq_name}: caption/graphic translation requires native read-back; Avid styling and title geometry are not reproduced"));
        }
        let beyond = |i: usize| WriteError::from(UnverifiedShape(format!("sequence {seq_name} event {i}: audio track beyond {} tracks", atracks.len())));
        let mut master_gain: HashMap<TrackItemId, f64> = HashMap::new();
        let mut edit_pan: HashMap<TrackItemId, (Option<f64>, i64)> = HashMap::new();
        let mut default_panned = 0usize;
        let dissolve_events: HashSet<_> = self.video_dissolves.get(&sequence_index)
            .into_iter().flatten().flat_map(|d| [d.outgoing, d.incoming]).collect();
        let mut dissolve_items = HashMap::new();
        let crossfade_events: HashSet<_> = self.audio_crossfades.get(&sequence_index).into_iter().flatten()
            .flat_map(|fade| [(fade.outgoing, fade.track), (fade.incoming, fade.track)]).collect();
        let mut crossfade_items = HashMap::new();
        let mut edit_markers: HashMap<i64, Vec<&Marker>> = HashMap::new();
        for marker in seq.markers.iter().filter(|m| m.owner == "edit" && !m.inherited) {
            if let Some(event) = marker.event {
                edit_markers.entry(event).or_default().push(marker);
            }
        }
        for (i, e) in seq.events.iter().enumerate() {
            let plain_nest = self.picture_nests.contains(&(sequence_index, i));
            let group_use = self.multicam_uses.get(&(sequence_index, i)).copied();
            let prepared_picture = self.picture_effects.get(&(sequence_index, i)).copied();
            let picture_effect = prepared_picture.map(|(effect, _)| effect);
            let (kind, mut payload) = self.placement[&e.clip].clone();
            if let Some(own) = edit_markers.get(&(i as i64)) {
                payload = self.edit_copy(seq, sequence, (sequence_index, i), e, kind, payload, own, &mut extra_markers)?;
            }
            let start = Time::new(e.record_start);
            if let Some(v) = e.video_track {
                if v > vtracks.len() as i64 {
                    return Err(UnverifiedShape(format!("sequence {seq_name} event {i}: video track {v} > {} tracks", vtracks.len())).into());
                }
            }
            if e.audio_tracks.iter().any(|&t| t > atracks.len() as i64) {
                return Err(beyond(i));
            }
            let clip = model.clips.get(e.clip.as_str());
            let original=model.originals.get(e.clip.as_str()).copied();
            let mut placed: Vec<TrackItemId> = Vec::new();
            let mut linked_group_audio = Vec::new();
            if let Some(u) = group_use {
                let source = self.multicam_items[u.group];
                if let Some(v) = e.video_track {
                    placed.push(self.project.add_clip(track_at(&vtracks,v)?, source, start, self.ids)?);
                }
                let clip = clip.ok_or_else(|| other("multicam leaf registry mismatch"))?;
                let files = event_audio_files(clip, e)?;
                for (j, (&audio, &track)) in u.audio.iter().zip(&e.audio_tracks).enumerate() {
                    let target = track_at(&atracks, track)?;
                    let gain = edit_value(&e.audio_gains, j)?;
                    let original_gain = files[j].gain_db;
                    let (item, applied_gain) = match audio {
                        PreparedGroupAudio::Group(channel) => {
                            let item = self.project.add_clip(target, source, start, self.ids)?;
                            self.project.narrow_multicam_audio(item, source, channel)?;
                            // The Group source already applies the leaf gain.
                            (item, gain.map_or(0.0, |g| g - original_gain))
                        }
                        PreparedGroupAudio::LinkedAlternate(_) => {
                            let sibling = self.items[&e.clip].1.get(identity(clip, files[j]))
                                .copied().ok_or_else(|| other("linked alternate Group audio sibling is missing"))?;
                            let item = self.project.add_clip(target, sibling, start, self.ids)?;
                            linked_group_audio.push(item);
                            (item, gain.unwrap_or(original_gain))
                        }
                    };
                    master_gain.insert(item, applied_gain);
                    edit_pan.insert(item, (edit_value(&e.audio_pans,j)?,track));
                    placed.push(item);
                }
            } else { match &payload {
                Payload::Original(source)=>{
                    let c=original.ok_or_else(||other("original clip registry mismatch"))?;
                    if let Some(v)=e.video_track {placed.push(self.project.add_clip(track_at(&vtracks,v)?,*source,start,self.ids)?);}
                    for (j,&track) in e.audio_tracks.iter().enumerate(){
                        let lane=super::original::mono_source_channel(c,e,j).ok_or_else(||other("original mono source channel missing"))?;
                        let group=c.groups.iter().position(|g|g==&vec![lane]).ok_or_else(||other("original mono group missing"))?;
                        let item=super::native_av::place_audio(&mut self.project,track_at(&atracks,track)?,*source,group,start,self.ids)?;
                        let channel=c.channels.iter().find(|a|a.avid_track==lane).unwrap();
                        master_gain.insert(item,edit_value(&e.audio_gains,j)?.unwrap_or(channel.gain_db));edit_pan.insert(item,(edit_value(&e.audio_pans,j)?,track));placed.push(item);
                    }
                    for st in &e.stereo {
                        let group=c.groups.iter().position(|g|g==&st.channels).ok_or_else(||other("original stereo group missing"))?;
                        let item=super::native_av::place_audio(&mut self.project,track_at(&atracks,st.audio_track)?,*source,group,start,self.ids)?;
                        master_gain.insert(item,st.gain_db.unwrap_or(c.channels.iter().find(|a|a.avid_track==st.channels[0]).unwrap().gain_db));placed.push(item);
                    }
                }
                Payload::Native(source) => {
                    let clip=clip.ok_or_else(||other("native clip registry mismatch"))?;
                    let source = *source;
                    let groups = super::native_av::groups(clip)?;
                    if let Some(v) = e.video_track {
                        placed.push(self.project.add_clip(track_at(&vtracks,v)?,source,start,self.ids)?);
                    }
                    for (j,(file,&track)) in event_audio_files(clip,e)?.iter().zip(&e.audio_tracks).enumerate() {
                        let at = identity(clip,file);
                        let group = groups.iter().position(|g| *g == vec![at]).ok_or_else(||other("native mono group missing"))?;
                        let item = super::native_av::place_audio(&mut self.project,track_at(&atracks,track)?,source,group,start,self.ids)?;
                        master_gain.insert(item,edit_value(&e.audio_gains,j)?.unwrap_or(file.gain_db));
                        edit_pan.insert(item,(edit_value(&e.audio_pans,j)?,track));
                        placed.push(item);
                    }
                    for st in &e.stereo {
                        let wanted: Vec<_> = st.channels.iter().map(|t|clip.audio.iter().position(|a|a.avid_track==Some(*t)).unwrap()).collect();
                        let group = groups.iter().position(|g| *g == wanted).ok_or_else(||other("native stereo group missing"))?;
                        let item = super::native_av::place_audio(&mut self.project,track_at(&atracks,st.audio_track)?,source,group,start,self.ids)?;
                        master_gain.insert(item,st.gain_db.unwrap_or(clip.audio[wanted[0]].gain_db));
                        placed.push(item);
                    }
                }
                Payload::Grouped(grouped) => {
                    let clip=clip.ok_or_else(||other("grouped clip registry mismatch"))?;
                    // A multicam clip: video from the multicam sequence; the event's n-th audio
                    // track plays the clip's n-th model audio file, i.e. its channel in the
                    // combined (A01-first) order.
                    let grouped = *grouped;
                    if let Some(v) = e.video_track {
                        placed.push(self.project.add_clip(track_at(&vtracks, v)?, grouped, start, self.ids)?);
                    }
                    let combined = combined_order(clip)?;
                    let groups = self
                        .project
                        .item_extra
                        .get(&grouped)
                        .and_then(|x| x.channel_groups.clone())
                        .unwrap_or_else(|| (0..combined.len()).map(|c| vec![c]).collect());
                    let files = event_audio_files(clip, e)?;
                    let channels: Vec<usize> = files
                        .iter()
                        .map(|f| {
                            let k = identity(clip, f);
                            combined.iter().position(|&c| c == k).expect("in combined order")
                        })
                        .collect();
                    for (j, (&channel, &track)) in channels.iter().zip(&e.audio_tracks).enumerate() {
                        let item = self.project.add_clip(track_at(&atracks, track)?, grouped, start, self.ids)?;
                        let group = group_for_channel(&groups, channel)?;
                        narrow_to_group(&mut self.project, item, &groups, group)?;
                        // The multicam's inner clips already carry the master gain; an edit gain
                        // replaces it, so only the difference goes on the edit.
                        let own_gain = edit_value(&e.audio_gains, j)?;
                        master_gain.insert(item, own_gain.map_or(0.0, |g| g - files[j].gain_db));
                        edit_pan.insert(item, (edit_value(&e.audio_pans, j)?, track));
                        placed.push(item);
                    }
                    // An Avid stereo edit: one item on its track playing the clip's stereo group.
                    let by_track: Vec<(Option<i64>, usize)> =
                        combined.iter().enumerate().map(|(pos, &k)| (clip.audio[k].avid_track, pos)).collect();
                    for st in &e.stereo {
                        if st.audio_track > atracks.len() as i64 {
                            return Err(beyond(i));
                        }
                        report_stereo_pans(&mut self.report, seq, i, st.audio_track, &st.pans);
                        let mut wanted = Vec::new();
                        for &c in &st.channels {
                            let Some(&(_, pos)) = by_track.iter().rev().find(|(t, _)| *t == Some(c)) else {
                                return Err(other(format!("KeyError: {c}")));
                            };
                            wanted.push(pos);
                        }
                        let Some(group) = groups.iter().position(|g| *g == wanted) else {
                            self.report.unverified.push(format!(
                                "sequence {seq_name} event {i}: stereo edit whose clip is not grouped as that pair; not placed"
                            ));
                            continue;
                        };
                        let item = self.project.add_clip(track_at(&atracks, st.audio_track)?, grouped, start, self.ids)?;
                        narrow_to_group(&mut self.project, item, &groups, group)?;
                        let left = stereo_left(clip, &st.channels)?;
                        master_gain.insert(item, st.gain_db.map_or(0.0, |g| g - left));
                        placed.push(item);
                    }
                }
                Payload::Parts { video: video_item, audio: audio_items, stereo: stereo_builds } => {
                    let clip=clip.ok_or_else(||other("legacy clip registry mismatch"))?;
                    // Items are in model order (separate) or combined A01-first order (merged).
                    let files: Vec<usize> = if kind == "merged" { combined_order(clip)? } else { (0..clip.audio.len()).collect() };
                    let by_file: HashMap<usize, ItemId> = files.iter().copied().zip(audio_items.iter().copied()).collect();
                    let event_files = event_audio_files(clip, e)?;
                    let mut event_items = Vec::new();
                    for f in &event_files {
                        let k = identity(clip, f);
                        event_items.push(*by_file.get(&k).ok_or_else(|| other("KeyError"))?);
                    }
                    if let Some(v) = e.video_track {
                        let subclip = self.av_subclip_uses.get(&(sequence_index, i, None))
                            .map(|&index| self.av_subclip_items[index]);
                        let Some(video_item) = subclip.or(*video_item) else {
                            return Err(other("'NoneType' object has no attribute '_sequence_uid'"));
                        };
                        let video_item = if plain_nest {
                            self.nest_picture(seq, e, i, video_item, &preset)?
                        } else {
                            video_item
                        };
                        placed.push(self.project.add_clip(track_at(&vtracks, v)?, video_item, start, self.ids)?);
                    }
                    for (j, ((&audio_item, &track), f)) in event_items.iter().zip(&e.audio_tracks).zip(&event_files).enumerate() {
                        let target = track_at(&atracks, track)?;
                        let item = if let Some(&subclip) = self.av_subclip_uses.get(&(sequence_index, i, Some(track))) {
                            let source = self.av_subclip_items[subclip];
                            let group = match f.avid_track {
                                Some(1) => 0,
                                Some(2) => 1,
                                _ => return Err(other("A/V subclip requires an exact mono source label")),
                            };
                            super::native_av::place_audio(&mut self.project, target, source, group, start, self.ids)?
                        } else {
                            self.project.add_clip(target, audio_item, start, self.ids)?
                        };
                        // An edit's own Avid gain is its effective gain (Avid copies the master's
                        // at cut-in); without one, the master clip's gain applies, as Premiere
                        // copies it.
                        let own_gain = edit_value(&e.audio_gains, j)?;
                        master_gain.insert(item, own_gain.unwrap_or(f.gain_db));
                        edit_pan.insert(item, (edit_value(&e.audio_pans, j)?, track));
                        placed.push(item);
                    }
                    // An Avid stereo edit: one stereo clip on its track, played from the pair's build.
                    for st in &e.stereo {
                        report_stereo_pans(&mut self.report, seq, i, st.audio_track, &st.pans);
                        let Some(build) = stereo_get(stereo_builds, &st.channels) else {
                            self.report.unverified.push(format!(
                                "sequence {seq_name} event {i}: stereo edit for a pair the clip has no stereo build for ({kind} mode); not placed"
                            ));
                            continue;
                        };
                        if st.audio_track > atracks.len() as i64 {
                            return Err(beyond(i));
                        }
                        let item = self.project.add_clip(track_at(&atracks, st.audio_track)?, build, start, self.ids)?;
                        let extra = self.project.item_extra.get(&build).cloned().unwrap_or_default();
                        if extra.stereo_nest {
                            declare_stereo_chain(&mut self.project, item)?;
                        } else {
                            let Some(groups) = extra.channel_groups else {
                                return Err(other("'ProjectItem' object has no attribute '_channel_groups'"));
                            };
                            narrow_to_group(&mut self.project, item, &groups, 0)?;
                        }
                        // The build's inner clips carry each part's master gain (User-confirmed
                        // -6 dB inside the stereo build); an edit gain replaces it, so add only
                        // the difference.
                        let left = stereo_left(clip, &st.channels)?;
                        master_gain.insert(item, st.gain_db.map_or(0.0, |g| g - left));
                        placed.push(item);
                    }
                }
            }}
            if self.disabled_edits.contains(&(sequence_index, i)) {
                let [item] = placed.as_slice() else {
                    return Err(other("disabled occurrence did not produce one Premiere clip"));
                };
                self.project.set_disabled(*item, true)?;
            }
            let rect = if group_use.is_some() { Some("0,0,1920,1080".into()) } else { match (kind, clip.and_then(|c|c.video.as_ref())) {
                ("grouped", Some(v)) => Some(format!("0,0,{},{}", v.width, v.height)),
                _ => None,
            }};
            if let Some(labels) = self.edit_labels.get(&(sequence_index, i)) {
                for label in labels {
                    let track = match label.audio_track {
                        Some(track) => track_at(&atracks, track)?,
                        None => track_at(&vtracks, e.video_track.ok_or_else(|| other("label edit has no picture"))?)?,
                    };
                    let mut copies = placed.iter().copied()
                        .filter(|&item| self.project.track_item(item).track == track);
                    let copy = copies.next().filter(|_| copies.next().is_none())
                        .ok_or_else(|| other("label owner did not produce one Premiere clip"))?;
                    mk::set_edit_label(&mut self.project, copy, label.index)?;
                }
            }
            for &t in &placed {
                let mut span = e.duration;
                let mut raw_speed = None;
                let mut raw_start = None;
                if let Some(ratio) = e.speed_ratio {
                    span = e
                        .source_duration
                        .ok_or_else(|| other("unsupported operand type(s) for +: 'int' and 'NoneType'"))?;
                    if let Some(&speed) = self.constant_speeds.get(&(sequence_index, i)) {
                        let (start, end) = match original {
                            Some(original) => speed.raw_original_trim(seq, e, original),
                            None => clip.and_then(|clip| speed.raw_trim(seq, e, clip)),
                        }
                            .ok_or_else(|| other("qualified constant-speed trim is out of range"))?;
                        span = end - start;
                        raw_start = Some(start);
                        raw_speed = Some((speed.ratio.1 * speed.ratio.0.signum(), speed.ratio.0.abs()));
                    } else if let Some(original) = original {
                        // Premiere 27 plays original-file cuts from the raw In.
                        // In reverse the raw trim is measured from media end.
                        raw_speed = Some(ratio);
                        if ratio.0 < 0 {
                            let available = original.video.as_ref()
                                .ok_or_else(|| other("retimed original lacks picture"))?.duration;
                            raw_start = Some(available.checked_sub(e.source_in)
                                .and_then(|v| v.checked_sub(span)).filter(|&v| v >= 0)
                                .ok_or_else(|| other("reverse original trim exceeds source"))?);
                        }
                    } else {
                        set_playback_speed(&mut self.project, t, ratio)?;
                    }
                }
                // A remapped clip's In/Out are positions on its remap curve (see writer.py).
                let is_video = self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem";
                let start = if plain_nest && is_video {
                    0
                } else if let Some(u) = group_use.filter(|_| !linked_group_audio.contains(&t)) {
                    u.source_in
                } else if e.time_remap.is_some() {
                    ticks_per_unit(seq.rate)
                } else {
                    raw_start.unwrap_or(e.source_in)
                };
                self.project.set_in_point(t, Time::new(start))?;
                self.project.set_out_point(t, Time::new(add(start, span)?))?;
                // Preserve raw file coordinates; the legacy setter scales by speed.
                if let Some(ratio) = raw_speed {
                    set_playback_speed(&mut self.project, t, ratio)?;
                }
                self.project.set_end(t, Time::new(add(e.record_start, e.duration)?))?;
                let r = if is_video { rect.as_deref() } else { None };
                normalize_placement(&mut self.project, t, r)?;
                if is_video {
                    if let Some(held_ticks) = self.model.frame_holds.get(&(sequence_index, i)) {
                        let clip = self.project.track_item(t).clip;
                        let tree = &mut self.project.doc.tree;
                        crate::xml::mutations::append_leaf(tree, clip, "FrameHoldStart", &held_ticks.to_string());
                        crate::xml::mutations::append_leaf(tree, clip, "FrameHold", "5");
                    }
                    if dissolve_events.contains(&i) && dissolve_items.insert(i, t).is_some() {
                        return Err(other("dissolve endpoint produced more than one picture item"));
                    }
                    if let Some(angle) = group_use.and_then(|u| u.video) {
                        super::multicam::select_picture(&mut self.project, t, angle)?;
                    }
                }
            }
            if let Some(remap) = &e.time_remap {
                let clip_rate = clip.and_then(|c|c.video.as_ref().map(|v|v.rate)).or_else(||original.and_then(|c|c.video.as_ref().map(|v|v.rate))).ok_or_else(|| other("time_remap on a clip without video"))?;
                let unit = ticks_per_unit(seq.rate);
                let frames = floor_div(e.duration, unit)?;
                if remap.len() as i64 != frames {
                    return Err(UnverifiedShape(format!(
                        "sequence {seq_name} event {i}: time_remap has {} frames, the edit {frames}",
                        remap.len()
                    ))
                    .into());
                }
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        // Media time, a quarter frame into each source frame (see writer.py).
                        let keys: Vec<(Time, f64)> = remap
                            .iter()
                            .enumerate()
                            .map(|(k, &f)| {
                                let n = (4 * f as i128 + 1) * clip_rate.den as i128;
                                let d = 4 * clip_rate.num as i128;
                                (Time::new((k as i64 + 1) * unit), n as f64 / d as f64)
                            })
                            .collect();
                        set_time_remap(&mut self.project, t, &keys)?;
                    }
                }
                if original.and_then(|clip| clip.video.as_ref()).is_some_and(|video| video.codec == "apch") {
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: linked ProRes conform as Premiere Time Remapping from verified source frames"
                    ));
                } else if original.is_some() {
                    let direction = if seq.rate == Fraction::new(24, 1) {
                        "23.976 to 24"
                    } else {
                        "24 to 23.976"
                    };
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: automatic {direction} Avid Motion Adapter picture cadence as Premiere Time Remapping"
                    ));
                } else {
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: Avid Timewarp as Time Remapping, one key per frame from Avid's own sampled positions (provisional)"
                    ));
                }
            }
            if let Some(matte) = &e.matte {
                if matte.inverted && matte.shapes.len() > 1 {
                    // See writer.py: per-mask inversion is not Avid's Key Out of several shapes.
                    self.report.notes.push(format!("sequence {seq_name} event {i}: Avid AniMatte Key Out with several shapes not converted yet"));
                } else {
                    let rate = clip.and_then(|c|c.video.as_ref().map(|v|v.rate)).or_else(||original.and_then(|c|c.video.as_ref().map(|v|v.rate))).ok_or_else(|| other("matte on a clip without video"))?;
                    let unit = ticks_per_unit(rate);
                    for &t in &placed {
                        if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                            for n in apply_matte(&mut self.project, t, matte, e.source_in, unit)? {
                                self.report.notes.push(format!("sequence {seq_name} event {i}: {n}"));
                            }
                        }
                    }
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: Avid AniMatte as Opacity mask(s), per-frame path keys for tracking (provisional)"
                    ));
                }
                if matte.feathers.iter().any(|f| *f != (0.0, 0.0)) {
                    self.report.notes.push(format!("sequence {seq_name} event {i}: AniMatte feather not converted yet (units not calibrated)"));
                }
                for n in &matte.not_converted {
                    self.report.notes.push(format!("sequence {seq_name} event {i}: AniMatte {n} not converted"));
                }
            }
            if let Some(tf) = &e.transform {
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        for n in apply_motion_keys(&mut self.project, t, tf, picture_effect, self.ids)? {
                            self.report.notes.push(format!("sequence {seq_name} event {i}: {n}"));
                        }
                    }
                }
                if !tf.not_converted.is_empty() {
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: Avid picture effect not converted yet: {}",
                        tf.not_converted.join(", ")
                    ));
                }
            }
            if let Some(PictureEffect::WarpRoll { keys: [first, ..], .. }) = picture_effect {
                let transform = super::model::Transform {
                    position: (first.x, 0.0), scale: (first.width, 100.0),
                    rotation: first.rotation, anchor: (0.0, 0.0), crop: [0.0; 4],
                    not_converted: Vec::new(),
                };
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        apply_motion_keys(&mut self.project, t, &transform, picture_effect, self.ids)?;
                    }
                }
            }
            let [horizontal, vertical] = match prepared_picture {
                Some((PictureEffect::Flip { horizontal, vertical }, _)) => [horizontal, vertical],
                Some((_, axes)) => axes,
                None => [false; 2],
            };
            if horizontal || vertical {
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        apply_flip(&mut self.project, t, horizontal, vertical)?;
                    }
                }
            }
            if let Some(PictureEffect::CornerPin(pin)) = picture_effect {
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        apply_corner_pin(&mut self.project, t, pin, self.ids)?;
                    }
                }
            }
            if let Some(PictureEffect::CornerPinWithFlop { pin, flop_before_pin }) = picture_effect {
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        apply_corner_pin_with_flop(&mut self.project, t, pin, flop_before_pin, self.ids)?;
                    }
                }
            }
            if let Some(PictureEffect::UltraKey { rgb }) = picture_effect {
                let mut picture = placed.iter().copied().filter(|&item|
                    self.project.doc.tree.tag(self.project.track_item(item).element) == "VideoClipTrackItem");
                let item = picture.next().filter(|_| picture.next().is_none())
                    .ok_or_else(|| other("Ultra Key did not produce one Premiere picture edit"))?;
                super::ultra_key::apply(&mut self.project, item, rgb)?;
            }
            if let Some(effect @ (PictureEffect::VideoToFullRange | PictureEffect::FullToVideoRange)) = picture_effect {
                let mut picture = placed.iter().copied().filter(|&item|
                    self.project.doc.tree.tag(self.project.track_item(item).element) == "VideoClipTrackItem");
                let item = picture.next().filter(|_| picture.next().is_none())
                    .ok_or_else(|| other("range scaling did not produce one Premiere picture edit"))?;
                super::asc_cdl::apply_range_scale(&mut self.project, item, effect == PictureEffect::VideoToFullRange)?;
            }
            if let Some(lut) = self.source_luts.get(&(sequence_index, i)) {
                let mut picture = placed.iter().copied().filter(|&item|
                    self.project.doc.tree.tag(self.project.track_item(item).element) == "VideoClipTrackItem");
                let item = picture.next().filter(|_| picture.next().is_none())
                    .ok_or_else(|| other("source LUT did not produce one Premiere picture edit"))?;
                if lut.edge != super::prepared::PreparedSourceLut::FULL_EDGE {
                    super::color_mode::apply_lut(&mut self.project, item, lut)?;
                } else {
                    apply_lumetri_lut(&mut self.project, item, &lut.points, lut.edge, Path::new(&lut.filename), self.ids)?;
                }
            }
            if self.model.extended_sdr_grades.contains(&(sequence_index, i)) {
                let grade = e.color_correction.as_ref().and_then(ExtendedSdrGrade::from_correction)
                    .ok_or_else(|| other("extended SDR LUT lost its prepared grade"))?;
                let mut picture = placed.iter().copied().filter(|&item|
                    self.project.doc.tree.tag(self.project.track_item(item).element) == "VideoClipTrackItem");
                let item = picture.next().filter(|_| picture.next().is_none())
                    .ok_or_else(|| other("extended SDR LUT did not produce one Premiere picture edit"))?;
                let filename = self.apply_extended_sdr_grade(grade, item)?.to_owned();
                self.report.notes.push(format!(
                    "sequence {seq_name} event {i}: embedded extended Rec.709 SDR Color Mode LUT ({filename}); visible SDR approximation, extreme YCbCr levels may differ"
                ));
            } else if let Some(cc) = &e.color_correction {
                let curves_ui8: Vec<(String, Vec<(f64, f64)>)> =
                    cc.curves.iter().map(|(k, v)| (k.clone(), v.iter().map(|&(x, y)| (x * 255.0, y * 255.0)).collect())).collect();
                if !avid_color::is_identity(&cc.tone, cc.saturation, &curves_ui8) {
                    // The whole modelled Avid correction as one LUT embedded in Lumetri
                    // (PRPROJ-0027), also saved as a .cube next to the project.
                    let transform = avid_color::rgb_transform(&cc.tone, cc.saturation, &curves_ui8)?;
                    let points = avid_color::cube_lut(&*transform, LUT_SIZE)?;
                    let stem = self.out_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let lut_dir = py_join(&py_parent(&self.out_path), &format!("{stem} LUTs"));
                    check_lut_directory(&lut_dir)?;
                    let safe: String =
                        seq.name.chars().map(|ch| if is_alnum(ch) || " -_".contains(ch) { ch } else { '_' }).take(60).collect();
                    let tc = timecode(e.record_start, seq.rate, seq.start)?;
                    let requested_cube = py_join(&lut_dir, &format!("{safe} {} avid cc.cube", tc.replace(':', "-")));
                    let cube = plan_cube_file(&requested_cube, &mut self.asset_names)?;
                    let title=format!("Avid Color Correction, {} {tc}",seq.name);
                    let digest=crate::hash::hex(&crate::hash::sha256(cube_text(&points,LUT_SIZE,&title).as_bytes()));
                    self.assets.push(PreparedLut {path:cube.clone(),recipe:LutRecipe::Legacy(cc.clone()),title,sha256:digest});
                    for &t in &placed {
                        if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                            apply_lumetri_lut(&mut self.project, t, &points, LUT_SIZE as u32, &cube, self.ids)?;
                        }
                    }
                    let cube_name = cube.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if cube != requested_cube {
                        self.report.notes.push(format!("sequence {seq_name} event {i}: LUT export filename already used; saved as {cube_name}; existing files kept"));
                    }
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: Avid color correction as an embedded Lumetri LUT ({cube_name}); modelled from one measured Avid fixture, provisional"
                    ));
                }
                if !cc.not_converted.is_empty() {
                    self.report.notes.push(format!(
                        "sequence {seq_name} event {i}: Avid color correction not converted yet: {}",
                        cc.not_converted.join(", ")
                    ));
                }
            }
            if placed.len() > 1 {
                self.project.link(sequence, &placed)?;
            }
            let audio_placed: Vec<TrackItemId> = placed
                .iter()
                .copied()
                .filter(|&t| self.project.doc.tree.tag(self.project.track_item(t).element) == "AudioClipTrackItem")
                .collect();
            let (gain_db, volume, shift) = split_overflow(&e.audio);
            if shift != 0.0 {
                self.report.notes.push(format!(
                    "sequence {seq_name} event {i}: volume above Premiere's +6 dB clip maximum; {} dB moved into audio gain (same loudness)",
                    fmt_f(shift, 3)
                ));
            }
            for t in audio_placed {
                // A new placement carries its master clip's gain (Premiere copies it,
                // PRPROJ-0020) on top of the edit's own gain.
                let total = gain_db + master_gain.get(&t).copied().unwrap_or(0.0);
                set_clip_gain(&mut self.project, t, total)?;
                if e.audio != EventAudio::default() {
                    set_clip_volume(&mut self.project, t, &volume)?;
                }
                let (mut pan, track) = match edit_pan.get(&t) {
                    Some(&(pan, track)) => (pan, Some(track)),
                    None => (None, None),
                };
                if let Some(track) = track {
                    if crossfade_events.contains(&(i, track)) && crossfade_items.insert((i, track), t).is_some() {
                        return Err(other("audio crossfade endpoint produced more than one item"));
                    }
                }
                let pan_keys = track.and_then(|track| self.audio_pans.get(&(sequence_index, i, track)));
                if let Some(curve) = pan_keys {
                    if self.project.sequence(sequence).audio_channel_count != Some(2) {
                        return Err(other("clip pan keys require a stereo Premiere output"));
                    }
                    set_clip_pan_keys(&mut self.project, t, &curve.keys)?;
                } else {
                    if let (None, Some(track)) = (pan, track) {
                        if options.match_avid_default_pan {
                            pan = Some(if track.rem_euclid(2) != 0 { -100.0 } else { 100.0 }); // Avid default: odd tracks left, even right
                        } else {
                            default_panned += 1;
                        }
                    }
                    if let Some(p) = pan.filter(|&p| p != 0.0) {
                        set_clip_pan(&mut self.project, t, Pan::Percent(p))?;
                    }
                }
                let fade_track = track.or_else(|| e.stereo.iter().find(|st| {
                    track_at(&atracks, st.audio_track).ok() == Some(self.project.track_item(t).track)
                }).map(|st| st.audio_track));
                if let Some((track, fade)) = fade_track.and_then(|track| {
                    self.audio_edge_fades.get(&(sequence_index, i, track)).map(|fade| (track, fade))
                }) {
                    let track = track_at(&atracks, track)?;
                    for (at_start, duration) in [(true, fade.head), (false, fade.tail)] {
                        if duration > 0 {
                            self.project.add_prepared_audio_fade(track, t, at_start, duration)?;
                        }
                    }
                }
            }
        }
        if default_panned != 0 {
            self.report.notes.push(format!(
                "sequence {seq_name}: {default_panned} mono edit(s) had no explicit Avid pan and were centred (Avid would play them on its default odd-left/even-right panning; option: match Avid's default panning)"
            ));
        }
        for level in &seq.track_levels {
            if level.track > atracks.len() as i64 {
                return Err(UnverifiedShape(format!("sequence {seq_name}: track level for missing audio track {}", level.track)).into());
            }
            set_track_level(&mut self.project, track_at(&atracks, level.track)?, level.db)?;
        }
        if let Some(mixer) = mixer {
            if let Some(db) = mixer.mix_level_db {
                set_mix_level(&mut self.project, sequence, db)?;
            }
            if let Some(pans) = track_pans {
                for (index, pan) in pans.into_iter().enumerate() {
                    if let Some(pan) = pan {
                        set_mono_track_pan(&mut self.project, atracks[index], pan)?;
                    }
                }
            }
            for &track in &mixer.muted_tracks {
                self.project.set_muted(track_at(&atracks, track)?, true)?;
            }
        }
        for fade in &seq.audio_crossfades {
            if fade.track > atracks.len() as i64 {
                return Err(UnverifiedShape(format!("sequence {seq_name}: crossfade on missing audio track {}", fade.track)).into());
            }
            let track = track_at(&atracks, fade.track)?;
            match add_crossfade(&mut self.project, track, fade, &fade.style) {
                Ok(()) => {}
                Err(Error::Value(message)) => return Err(UnverifiedShape(format!("sequence {seq_name}: {message}")).into()),
                Err(error) => return Err(error.into()),
            }
        }
        for fade in self.audio_crossfades.get(&sequence_index).into_iter().flatten() {
            let out = *crossfade_items.get(&(fade.outgoing, fade.track))
                .ok_or_else(|| other("missing outgoing audio crossfade item"))?;
            let inc = *crossfade_items.get(&(fade.incoming, fade.track))
                .ok_or_else(|| other("missing incoming audio crossfade item"))?;
            let cut = seq.events[fade.incoming].record_start;
            self.project.add_prepared_audio_crossfade(track_at(&atracks, fade.track)?, out, inc,
                cut - fade.before_cut, add(cut, fade.duration - fade.before_cut)?, fade.before_cut, true)?;
        }
        for d in self.video_dissolves.get(&sequence_index).into_iter().flatten() {
            let out = *dissolve_items.get(&d.outgoing).ok_or_else(|| other("missing outgoing dissolve item"))?;
            let inc = *dissolve_items.get(&d.incoming).ok_or_else(|| other("missing incoming dissolve item"))?;
            let cut = seq.events[d.incoming].record_start;
            let track = self.project.track_item(inc).track;
            self.project.add_prepared_dissolve(track, out, inc,
                cut - d.before_cut, add(cut, d.duration - d.before_cut)?, d.before_cut)?;
        }
        let unit = ticks_per_unit(seq.rate);
        let mut filler_video: BTreeMap<i64, Vec<&Marker>> = BTreeMap::new();
        let mut custom_data = Vec::new();
        let mut original_colors = Vec::new();
        let mut custom_budget = mk::MAX_MARKER_CUSTOM_DATA;
        for (index, m) in seq.markers.iter().enumerate() {
            let track = m.track.as_deref().unwrap_or("");
            if m.owner == "sequence" {
                let note = if track.is_empty() { String::new() } else { format!("on {track}") };
                let marker = mk::add_sequence_marker(&mut self.project, sequence, m, m.start, options, &mut self.report.notes, &note, self.ids)?;
                let key = MarkerDataKey::Sequence(sequence_index, index);
                let Some(rows) = model.marker_provenance else { continue };
                let custom = rows.custom_data.get(&key)
                    .filter(|data| !data.is_empty() && data.len() <= custom_budget && custom_data.len() < 128);
                let color = rows.original_colors.get(&key).filter(|_| original_colors.len() < 128);
                if custom.is_some() || color.is_some() {
                    let guid = crate::model::marker::marker_guid(&self.project.doc.tree, marker)?;
                    if let Some(data) = custom {
                        custom_data.push(format!("{guid}:{}", crate::hash::hex(data.as_bytes())));
                        custom_budget -= data.len();
                        self.saved_marker_data.insert(key.clone());
                    }
                    if let Some(bit) = color {
                        self.save_marker_color(marker, &guid, *bit, key, &mut original_colors)?;
                    }
                }
            } else if m.owner == "filler" && track.starts_with('V') {
                filler_video.entry(py_int(&track[1..])?).or_default().push(m);
            } else if m.owner == "filler" {
                // Adjustment layers are video-only.
                let note = format!("on empty {}", m.track.as_deref().unwrap_or("None"));
                mk::add_sequence_marker(&mut self.project, sequence, m, m.start, options, &mut self.report.notes, &note, self.ids)?;
            }
        }
        if !custom_data.is_empty() || !original_colors.is_empty() {
            let uid = self.project.sequence_id(sequence)
                .ok_or_else(|| other("sequence marker custom data lacks a sequence UID"))?.to_owned();
            let props = self.project.doc.tree.find1(
                self.project.sequence(sequence).element, "Node/Properties",
            ).ok_or_else(|| other("sequence marker custom data lacks a property bag"))?;
            for (tag, entries) in [(mk::SEQUENCE_MARKER_DATA, custom_data), (mk::SEQUENCE_MARKER_COLORS, original_colors)] {
                if !entries.is_empty() {
                    let saved = format!("1 {uid} {}", entries.join(","));
                    crate::xml::mutations::append_leaf(&mut self.project.doc.tree, props, tag, &saved);
                }
            }
        }
        for (m, start, note) in &extra_markers {
            mk::add_sequence_marker(&mut self.project, sequence, m, *start, options, &mut self.report.notes, note, self.ids)?;
        }
        let mut picture_by_lane = Vec::new();
        if let Some((&highest, _)) = filler_video.last_key_value() {
            for &lane in vtracks.iter().take(highest.max(0) as usize) {
                let spans = self.project.track_clips(lane).iter().map(|&clip|
                    Ok((self.project.start(clip)?.ticks, self.project.end(clip)?.ticks))
                ).collect::<Result<Vec<_>>>()?;
                picture_by_lane.push(spans);
            }
        }
        for (&track_no, ms) in &filler_video {
            if track_no > vtracks.len() as i64 {
                return Err(UnverifiedShape(format!("sequence {seq_name}: filler marker on missing video track {track_no}")).into());
            }
            let track = track_at(&vtracks, track_no)?;
            // Premiere can render a carrier as black over picture on a lower lane.
            let mut picture: Vec<_> = picture_by_lane[..track_no as usize]
                .iter().flatten().copied().collect();
            picture.sort_unstable();
            let mut furthest = i64::MIN;
            for (_, end) in &mut picture {
                furthest = furthest.max(*end);
                *end = furthest;
            }
            let mut carried = Vec::new();
            let mut spans: BTreeMap<i64, i64> = BTreeMap::new();
            for &m in ms {
                let end = add(m.start, m.duration.max(unit))?;
                let at = picture.partition_point(|&(start, _)| start < end);
                if at > 0 && picture[at - 1].1 > m.start {
                    let note = format!("on empty V{track_no}; marker carrier would overlap picture");
                    mk::add_sequence_marker(&mut self.project, sequence, m, m.start, options, &mut self.report.notes, &note, self.ids)?;
                    self.report.notes.push(format!("sequence {seq_name}: marker {} kept on sequence; V{track_no} carrier would overlap picture", py_str_repr(&m.name)));
                } else {
                    carried.push(m);
                    spans.entry(m.start).and_modify(|old| *old = (*old).max(end)).or_insert(end);
                }
            }
            let Some(length) = spans.values().copied().max() else { continue };
            let layer = mk::create_adjustment_layer(
                &mut self.project,
                self.root,
                &format!("{} V{track_no} markers", seq.name),
                unit,
                seq.width,
                seq.height,
                if self.rewrapped {i64::from(super::rewrap::display_format(seq.rate,false).ok_or_else(||other("unsupported rewrap marker clock"))?)} else {display_format(unit).unwrap_or(100)},
                length,
                self.ids,
            )?;
            move_to_processed_clips(&mut self.project, &[layer], self.sequence_parent, self.ids)?;
            for m in &carried {
                self.add_clip_marker(layer, m, m.start)?;
            }
            // Merge only overlapping spans. Extending the carrier preserves
            // every marker's end without duplicating any marker at a cut.
            let mut merged: Vec<(i64, i64)> = Vec::new();
            for (start, end) in spans {
                if let Some((_, previous_end)) = merged.last_mut().filter(|(_, e)| start < *e) {
                    *previous_end = (*previous_end).max(end);
                } else {
                    merged.push((start, end));
                }
            }
            for (start, end) in merged {
                let placed = self.project.add_clip(track, layer, Time::new(start), self.ids)?;
                self.project.set_in_point(placed, Time::new(start))?;
                self.project.set_out_point(placed, Time::new(end))?;
                self.project.set_end(placed, Time::new(end))?;
                normalize_placement(&mut self.project, placed, None)?;
            }
            self.report
                .notes
                .push(format!("sequence {seq_name}: {} marker(s) on empty V{track_no} carried by an adjustment layer", carried.len()));
        }
        if let Some(groups) = self.track_multicam_items.get(&sequence_index) {
            let rect = format!("0,0,{},{}", seq.width, seq.height);
            for (item, group) in groups {
                for edit in &group.uses {
                    let tracks = if edit.angle.is_some() { &vtracks } else { &atracks };
                    super::multicam_tracks::place(&mut self.project, *item, track_at(tracks, edit.track)?, edit, &rect, self.ids)?;
                }
            }
        }
        order_track_items(&mut self.project, sequence)?;
        self.project.finish_sequence_extent()?;
        if let Some(marks) = model.timeline_marks.get(&sequence_index) {
            let end = self.project.sequence_end(sequence)?.ticks;
            if marks.in_ticks.is_some_and(|ticks| ticks >= end)
                || marks.out_ticks.is_some_and(|ticks| ticks > end)
            {
                return Err(other("prepared timeline marks exceed the destination extent"));
            }
            self.project.set_sequence_in_point(sequence, marks.in_ticks.map(Time::new))?;
            self.project.set_sequence_out_point(sequence, marks.out_ticks.map(Time::new))?;
        }
        if let Some(states) = model.track_states.get(&sequence_index) {
            for state in states {
                let track = match state.lane {
                    PreparedTrackLane::Audio(lane) => track_at(&atracks, lane)?,
                    PreparedTrackLane::Picture(lane) => track_at(&vtracks, lane)?,
                };
                if let Some(name) = &state.name {
                    self.project.set_custom_track_name(track, name)?;
                }
                if state.locked {
                    self.project.set_locked(track, true)?;
                }
                if state.picture_disabled {
                    self.project.set_muted(track, true)?;
                }
            }
        }
        Ok(())
    }
}

/// `e.audio_gains[j] if e.audio_gains else None` (and the same for pans).
fn edit_value(values: &[Option<f64>], j: usize) -> Result<Option<f64>> {
    if values.is_empty() {
        return Ok(None);
    }
    values.get(j).copied().ok_or_else(|| other("list index out of range"))
}

/// The master gain of the stereo edit's left channel file
/// (`next(f for f in clip.audio if f.avid_track == avid_channels[0]).gain_db`).
fn stereo_left(clip: &Clip, channels: &[i64]) -> Result<f64> {
    let first = *channels.first().ok_or_else(|| other("list index out of range"))?;
    clip.audio.iter().find(|f| f.avid_track == Some(first)).map(|f| f.gain_db).ok_or_else(|| other("StopIteration"))
}

/// `write_project(model, out_path)`: write `model` as a new Premiere project at `out_path`
/// (never overwrites; the folder must exist), drawing every UUID from `ids`. Relative media
/// paths and the colour-correction LUT folder are computed against `out_path`.
pub fn write_project(model: &Model, out_path: impl AsRef<Path>, ids: &mut dyn UuidSource) -> Result<Report> {
    write_project_with_media(model, out_path, ids, &[])
}

/// Explicit OP1a media bindings; default write_project output remains byte-identical.
pub fn write_project_with_media(model: &Model, out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink]) -> Result<Report> {
    write_bound_project(model,out_path,ids,media,false)
}

/// Explicit experimental native file-backed AV masters, preserving the bin groups.
/// Retained native checks have bounded scope; wider profiles and User acceptance remain pending.
pub fn write_native_op1a_project(model: &Model, out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink]) -> Result<Report> {
    write_bound_project(model,out_path,ids,media,true)
}

fn write_bound_project(model: &Model, out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool) -> Result<Report> {
    prepare_project(model,out_path,ids,media,native_av)?.publish()
}

/// Side-effect-free construction for app preflight, using planned future media bindings.
pub fn prepare_project(model: &Model, out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool) -> Result<PreparedProject> {
    super::model::validate_model(model)?;
    prepare_project_with_texts(model, &[], out_path, ids, media, native_av)
}
/// Source-text extension; the strict model/1 API and unchanged outputs are retained.
pub fn prepare_project_with_texts(model: &Model, texts: &[super::prepared::PreparedText], out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool) -> Result<PreparedProject> {
    prepare_project_with_editorial(model, texts, &[], out_path, ids, media, native_av)
}
pub fn prepare_project_with_editorial(model: &Model, texts: &[super::prepared::PreparedText],
    video_dissolves: &[PreparedVideoDissolve], out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool) -> Result<PreparedProject> {
    prepare_project_with_effects(model, Editorial { texts, video_dissolves, picture_effects: &[], direct_dnxhr_origins: &HashMap::new(), constant_speeds: &[] },
        out_path, ids, media, native_av)
}

pub struct Editorial<'a> {
    pub texts: &'a [PreparedText],
    pub video_dissolves: &'a [PreparedVideoDissolve],
    pub picture_effects: &'a [PreparedPictureEffect],
    pub direct_dnxhr_origins: &'a HashMap<String, i64>,
    pub constant_speeds: &'a [super::prepared::PreparedConstantSpeed],
}

pub fn prepare_project_with_effects(model: &Model, editorial: Editorial<'_>,
    out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool) -> Result<PreparedProject> {
    prepare_project_with_effects_impl(model, editorial, out_path, ids, media, native_av, ProjectVersion::V45, &HashSet::new())
}

/// Prepare graph-derived legacy media for the current Premiere application.
pub fn prepare_project_with_effects_for_premiere_27(
    model: &Model, editorial: Editorial<'_>,
    out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool,
) -> Result<PreparedProject> {
    prepare_project_with_effects_for_premiere_27_with_rewrap_colors(
        model, editorial, out_path, ids, media, native_av, &HashSet::new(),
    )
}

/// Prepare legacy rewrap with source color profiles verified during media planning.
#[doc(hidden)]
pub fn prepare_project_with_effects_for_premiere_27_with_rewrap_colors(
    model: &Model, editorial: Editorial<'_>,
    out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool,
    dnxhr_444_rgb_sources: &HashSet<String>,
) -> Result<PreparedProject> {
    prepare_project_with_effects_impl(model, editorial, out_path, ids, media, native_av, ProjectVersion::V46, dnxhr_444_rgb_sources)
}

fn prepare_project_with_effects_impl(
    model: &Model, editorial: Editorial<'_>,
    out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool, project_version: ProjectVersion,
    dnxhr_444_rgb_sources: &HashSet<String>,
) -> Result<PreparedProject> {
    super::model::validate_model(model)?;
    let Editorial { texts, video_dissolves, picture_effects, direct_dnxhr_origins, constant_speeds } = editorial;
    super::prepared::validate_texts(texts, &model.sequences)?;
    super::prepared::validate_video_dissolves(video_dissolves, &model.sequences)?;
    super::prepared::validate_picture_effects(picture_effects, &model.sequences, constant_speeds,
        |_, _| false, |event| {
            model.clips.get(&event.clip)?.video.as_ref().map(|v| (v.rate, (v.width, v.height)))
        })?;
    super::prepared::validate_constant_speeds(constant_speeds, &model.options, &model.sequences,
        |id, seq, event, speed| model.clips.get(id).is_some_and(|clip| speed.raw_trim(seq, event, clip).is_some()))?;
    let view=WriterInput{options:&model.options,project_version,marker_provenance:None,clips:model.clips.iter().collect(),originals:HashMap::new(),bins:&model.bins,sequences:&model.sequences,texts,video_dissolves,audio_edge_fades:&[],audio_crossfades:&[],audio_pans:&[],audio_mixers:&BTreeMap::new(),track_states:&BTreeMap::new(),timeline_marks:&BTreeMap::new(),non_drop_sequences:&HashSet::new(),picture_effects,source_luts:&[],extended_sdr_grades:&HashSet::new(),edit_labels:&[],frame_holds:&HashMap::new(),disabled_edits:&HashSet::new(),direct_dnxhr_origins,direct_dnxhd_pcm24:None,constant_speeds,multicams:&[],multicam_uses:&[],multicam_tracks:&[],autosync_bins:&BTreeMap::new(),picture_subclips:&[],av_subclips:&[],av_subclip_uses:&[],picture_nests:&[],layered_submasters:&[],png_mattes:&super::prepared::PreparedPngMattes::default()};
    prepare_view(&view,model,out_path,ids,media,native_av,None,&HashSet::new(), &HashSet::new(), false, dnxhr_444_rgb_sources)
}

/// Validated typed input for mixed original-file and legacy media; model/1 remains strict.
pub fn prepare_typed_project(input:&super::prepared::PreparedProjectInput,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool)->Result<PreparedProject> {
    prepare_typed_project_impl(input,out_path,ids,media,native_av,None,&HashSet::new(), &HashSet::new(), None, ProjectVersion::V45, false, &HashSet::new())
}

pub enum TypedMedia<'a> {
    Bound { bindings: &'a [super::rewrap::MediaRelink], native_av: bool },
    Offline { paths: &'a HashSet<&'a str> },
    VerifiedOp1a {
        bindings: &'a [super::rewrap::MediaRelink],
        clips: &'a HashSet<String>,
        offline: &'a HashSet<&'a str>,
    },
    SelectedRewrap {
        bindings: &'a [super::rewrap::MediaRelink],
        clips: &'a HashSet<String>,
        offline: &'a HashSet<&'a str>,
    },
}

/// Prepare the selected media construction with Premiere 27's Project object version.
pub fn prepare_typed_project_for_premiere_27(
    input: &super::prepared::PreparedProjectInput,
    out_path: impl AsRef<Path>,
    ids: &mut dyn UuidSource,
    media: TypedMedia<'_>,
    layout: Option<&DestinationBinLayout>,
) -> Result<PreparedProject> {
    prepare_typed_project_for_premiere_27_with_rewrap_colors(
        input, out_path, ids, media, layout, &HashSet::new(),
    )
}

/// Prepare typed rewrap with source color profiles verified during media planning.
#[doc(hidden)]
pub fn prepare_typed_project_for_premiere_27_with_rewrap_colors(
    input: &super::prepared::PreparedProjectInput,
    out_path: impl AsRef<Path>,
    ids: &mut dyn UuidSource,
    media: TypedMedia<'_>,
    layout: Option<&DestinationBinLayout>,
    dnxhr_444_rgb_sources: &HashSet<String>,
) -> Result<PreparedProject> {
    let no_offline = HashSet::new();
    let no_op1a = HashSet::new();
    let (bindings, native_av, offline, op1a, selective) = match media {
        TypedMedia::Bound { bindings, native_av } => (bindings, native_av, &no_offline, &no_op1a, false),
        TypedMedia::Offline { paths } => (&[][..], false, paths, &no_op1a, false),
        TypedMedia::VerifiedOp1a { bindings, clips, offline } => (bindings, false, offline, clips, false),
        TypedMedia::SelectedRewrap { bindings, clips, offline } => (bindings, false, offline, clips, true),
    };
    prepare_typed_project_impl(input, out_path, ids, bindings, native_av, layout,
        offline, op1a, None, ProjectVersion::V46, selective, dnxhr_444_rgb_sources)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum MarkerDataKey {
    /// Destination sequence and marker indices.
    Sequence(usize, usize),
    /// Destination original clip ID and its marker index.
    Source(String, usize),
    /// Destination sequence index, event index and marker source ticks.
    Edit(usize, usize, i64),
}

#[derive(Clone, Copy)]
pub struct MarkerProvenance<'a> {
    pub custom_data: &'a HashMap<MarkerDataKey, String>,
    pub original_colors: &'a HashMap<MarkerDataKey, u64>,
}

/// Prepare the internal Resolve-linked Premiere 27 route with marker provenance.
pub fn prepare_typed_project_with_marker_provenance(
    input: &super::prepared::PreparedProjectInput,
    out_path: impl AsRef<Path>,
    ids: &mut dyn UuidSource,
    provenance: MarkerProvenance<'_>,
) -> Result<PreparedProject> {
    prepare_typed_project_impl(input, out_path, ids, &[], false, None, &HashSet::new(), &HashSet::new(), Some(provenance), ProjectVersion::V46, false, &HashSet::new())
}

/// Prepare selected source bins under one import root and their relative folder hierarchy.
/// Layout validation completes before UUID allocation or project construction.
pub fn prepare_typed_project_with_layout(input:&super::prepared::PreparedProjectInput,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool,layout:&DestinationBinLayout)->Result<PreparedProject> {
    prepare_typed_project_impl(input,out_path,ids,media,native_av,Some(layout),&HashSet::new(), &HashSet::new(), None, ProjectVersion::V45, false, &HashSet::new())
}
pub fn prepare_offline_project(
    input: &super::prepared::PreparedProjectInput, out_path: impl AsRef<Path>,
    ids: &mut dyn UuidSource, layout: Option<&DestinationBinLayout>, offline: &HashSet<&str>,
) -> Result<PreparedProject> {
    prepare_typed_project_impl(input,out_path,ids,&[],false,layout,offline, &HashSet::new(), None, ProjectVersion::V45, false, &HashSet::new())
}

/// Use the validated OP1a bin master construction for selected in-place MXFs.
/// Other clips retain the ordinary linked route.
pub fn prepare_linked_op1a_project(
    input: &super::prepared::PreparedProjectInput, out_path: impl AsRef<Path>,
    ids: &mut dyn UuidSource, media: &[super::rewrap::MediaRelink],
    clips: &HashSet<String>, layout: Option<&DestinationBinLayout>, offline: &HashSet<&str>,
) -> Result<PreparedProject> {
    prepare_typed_project_impl(input, out_path, ids, media, false, layout, offline, clips, None, ProjectVersion::V45, false, &HashSet::new())
}

fn prepare_typed_project_impl(input:&super::prepared::PreparedProjectInput,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool,layout:Option<&DestinationBinLayout>,offline:&HashSet<&str>,linked_op1a:&HashSet<String>,marker_provenance:Option<MarkerProvenance<'_>>,project_version:ProjectVersion,selective:bool,dnxhr_444_rgb_sources:&HashSet<String>)->Result<PreparedProject> {
    if let Some(layout) = layout { layout.validate(&input.bins)?; }
    if !native_av && layout.is_some() {
        input.validate_whole_project()?;
    } else {
        input.validate()?;
    }
    let mut legacy = Model {
        project_name: input.project_name.clone(),
        options: input.options.clone(),
        clips: input.clips.iter().filter_map(|clip| match clip {
            super::prepared::PreparedClip::Legacy(clip) => Some(clip.clone()),
            _ => None,
        }).collect(),
        bins: Vec::new(),
        sequences: Vec::new(),
    };
    if native_av {
        legacy.bins = input.bins.iter().map(|bin| super::model::Bin {
            name: bin.name.clone(),
            clips: bin.clips.iter().filter(|id| legacy.clips.contains_key(*id)).cloned().collect(),
        }).collect();
        legacy.sequences = input.sequences.iter().map(|sequence| {
            let mut sequence = sequence.clone();
            sequence.events.retain(|event| legacy.clips.contains_key(&event.clip));
            sequence.markers.clear();
            sequence
        }).collect();
    }
    let view=WriterInput{options:&input.options,project_version,marker_provenance,clips:legacy.clips.iter().collect(),originals:input.clips.iter().filter_map(|c|match c{super::prepared::PreparedClip::Original(c)=>Some((c.metadata.id.as_str(),c)),_=>None}).collect(),bins:&input.bins,sequences:&input.sequences,texts:&input.texts,video_dissolves:&input.video_dissolves,audio_edge_fades:&input.audio_edge_fades,audio_crossfades:&input.audio_crossfades,audio_pans:&input.audio_pans,audio_mixers:&input.audio_mixers,track_states:&input.track_states,timeline_marks:&input.timeline_marks,non_drop_sequences:&input.non_drop_sequences,picture_effects:&input.picture_effects,source_luts:&input.source_luts,extended_sdr_grades:&input.extended_sdr_grades,edit_labels:&input.edit_labels,frame_holds:&input.frame_holds,disabled_edits:&input.disabled_edits,direct_dnxhr_origins:&input.direct_dnxhr_origins,direct_dnxhd_pcm24:Some(&input.direct_dnxhd_pcm24),constant_speeds:&input.constant_speeds,multicams:&input.multicams,multicam_uses:&input.multicam_uses,multicam_tracks:&input.multicam_tracks,autosync_bins:&input.autosync_bins,picture_subclips:&input.picture_subclips,av_subclips:&input.av_subclips,av_subclip_uses:&input.av_subclip_uses,picture_nests:&input.picture_nests,layered_submasters:&input.layered_submasters,png_mattes:&input.png_mattes};
    prepare_view(&view,&legacy,out_path,ids,media,native_av,layout,offline,linked_op1a,selective,dnxhr_444_rgb_sources)
}
fn dissolve_handles(model: &WriterInput<'_>) -> Result<()> {
    if model.video_dissolves.is_empty() { return Ok(()); }
    let groups: HashSet<_> = model.multicam_uses.iter().map(|u| (u.sequence, u.event)).collect();
    for d in model.video_dissolves {
        let s = &model.sequences[d.sequence];
        let out = &s.events[d.outgoing];
        let inc = &s.events[d.incoming];
        let after = d.duration - d.before_cut;
        let qualified = model.picture_clock(&out.clip).is_some_and(|(rate, duration)| rate == s.rate
            && i128::from(out.source_in) + i128::from(out.duration) + i128::from(after) <= i128::from(duration))
            && model.picture_clock(&inc.clip).is_some_and(|(rate, _)| rate == s.rate && inc.source_in >= d.before_cut)
            && !groups.contains(&(d.sequence, d.outgoing)) && !groups.contains(&(d.sequence, d.incoming));
        if !qualified {
            return Err(other(format!("transition_handles_unavailable: {} video dissolve needs same-rate picture with both source handles", s.name)));
        }
    }
    Ok(())
}
fn direct_dnxhr_files(
    model: &WriterInput<'_>,
    media: &[super::rewrap::MediaRelink],
    native_av: bool,
) -> Result<HashMap<String, DirectDnxhrFile>> {
    if let Some(ids) = model.direct_dnxhd_pcm24.filter(|ids| !ids.is_empty()) {
        let cameras: HashSet<_> = model.multicams.iter().flat_map(|group| &group.cameras)
            .map(|camera| camera.clip.as_str())
            .chain(model.layered_submasters.iter().flat_map(|nest| &nest.layers).map(|layer| layer.clip.as_str()))
            .collect();
        // Copies of the same master can share the verified package. All
        // marked copies still go through the media and clock checks below.
        let package = |id: &str| {
            let clip = model.clips.get(id)?;
            let mob = clip.material_umid.as_deref().filter(|mob| !mob.is_empty())?;
            let [a, b] = clip.audio.as_slice() else { return None };
            let video = clip.video.as_ref()?;
            let (first, second) = if a.avid_track == Some(1) { (a, b) } else { (b, a) };
            Some((mob, video.path.as_str(), first.path.as_str(), second.path.as_str()))
        };
        let qualified_packages: HashSet<_> = cameras.iter().filter(|id| ids.contains(**id))
            .filter_map(|id| package(id))
            .collect();
        if ids.iter().any(|id| {
            !model.direct_dnxhr_origins.contains_key(id)
                || (!cameras.contains(id.as_str())
                    && !model.clips.get(id.as_str()).is_some_and(|clip| clip.audio.is_empty())
                    && !package(id).is_some_and(|key| qualified_packages.contains(&key)))
        }) {
            return Err(other("direct DNxHD LB needs a verified picture-only item, Group or Submaster source"));
        }
    }
    if model.direct_dnxhr_origins.is_empty() {
        return Ok(HashMap::new());
    }
    if native_av || !media.is_empty() {
        return Err(other("direct DNxHR OP-Atom cannot use rewrap bindings"));
    }
    let three_mono_cameras: HashSet<_> = model.multicams.iter()
        .filter(|group| group.audio_cameras.len() == 3)
        .flat_map(|group| group.cameras.iter().map(|camera| camera.clip.as_str()))
        .collect();
    let one_mono_cameras: HashSet<_> = model.multicams.iter()
        .filter(|group| group.audio_cameras.len() == 1)
        .flat_map(|group| group.cameras.iter().map(|camera| camera.clip.as_str()))
        .collect();
    let mut files = HashMap::new();
    for (id, &origin) in model.direct_dnxhr_origins {
        let c = model.clips.get(id).ok_or_else(|| other("direct DNxHR OP-Atom clip ID is unavailable"))?;
        let picture = c.video.as_ref();
        let three_mono = three_mono_cameras.contains(id.as_str());
        let channel_count = if three_mono { 3 }
            else if c.audio.len() == 8 && one_mono_cameras.contains(id.as_str()) { 8 }
            else if c.audio.is_empty() { 0 }
            else if c.audio.len() == 1 { 1 }
            else { 2 };
        if c.audio.len() != channel_count {
            return Err(other(if three_mono {
                "three-mono direct Group camera requires three independent mono siblings"
            } else {
                "direct DNxHR OP-Atom requires two independent mono siblings"
            }));
        }
        let bits = c.audio.first().map_or(24, |a| a.bits);
        let dnxhd_group_camera = model.direct_dnxhd_pcm24.is_some_and(|ids| ids.contains(id));
        let (rate, start, duration) = if let Some(v) = picture {
            (v.rate, v.start, v.duration)
        } else {
            let ([first] | [first, _]) = c.audio.as_slice() else {
                return Err(other("direct DNxHR OP-Atom without picture requires one or two mono siblings"));
            };
            if dnxhd_group_camera || model.multicams.iter().any(|group|
                group.cameras.iter().any(|camera| camera.clip == *id))
            {
                return Err(other("direct DNxHR OP-Atom without picture requires a standalone clip"));
            }
            (c.rate.ok_or_else(|| other("direct DNxHR OP-Atom audio clock is unavailable"))?,
                first.start, first.duration)
        };
        let profile = (rate.num, rate.den, bits);
        if picture.is_none() && (profile != (24_000, 1001, 24) || model.options.bin_mode != "separate") {
            return Err(other("direct DNxHR OP-Atom without picture requires 23.976 Separate files with PCM24"));
        }
        if matches!(channel_count, 0 | 1 | 3 | 8)
            && (profile != (24_000, 1001, 24) || model.options.bin_mode != "separate")
        {
            return Err(other("picture-only and wider direct Group profiles require 23.976 Separate files with any audio at PCM24"));
        }
        if channel_count == 1 && (dnxhd_group_camera || one_mono_cameras.contains(id.as_str())) {
            return Err(other("one available mono sibling requires a standalone DNxHR clip"));
        }
        if dnxhd_group_camera && (profile != (24_000, 1001, 24) || model.options.bin_mode != "separate") {
            return Err(other("direct DNxHD LB/PCM24 requires 23.976 Separate files"));
        }
        let (display, dnxhr) = match profile {
            (24, 1, 16) if origin == 0 && matches!(model.options.bin_mode.as_str(), "grouped" | "merged") =>
                ("100", false),
            (24_000, 1001, 24) => ("110", !dnxhd_group_camera),
            (24_000, 1001, 16) => ("110", false),
            (30_000, 1001, 24) if c.time_display == Some(102) => ("102", false),
            _ => return Err(other("unsupported direct DNx OP-Atom rate or bin mode")),
        };
        let unit = ticks_per_unit(rate);
        let labels: HashSet<_> = c.audio.iter().filter_map(|a| a.avid_track).collect();
        let valid_labels = if channel_count == 1 {
            labels.len() == 1 && (labels.contains(&1) || labels.contains(&2))
        } else {
            (1..=channel_count as i64).all(|label| labels.contains(&label))
        };
        let mut paths: HashSet<_> = picture.into_iter().map(|v| v.path.as_str()).collect();
        if picture.is_some_and(|v| v.codec != "AVdn" || (v.width, v.height) != (1920, 1080))
            || origin < 0 || origin % unit != 0
            || start < 0 || start % unit != 0 || duration <= 0 || duration % unit != 0
            || display == "102" && (origin != start || origin / unit >= 2_589_408
                || duration % (super::model::TICKS_PER_SECOND / 48_000) != 0)
            || bits == 16 && (origin != start || origin / unit >= 24 * 60 * 60 * 24)
            || !valid_labels
            || !c.stereo_pairs.is_empty() || !c.multichannel_groups.is_empty()
            || c.audio.iter().any(|s| s.sample_rate != 48000 || s.bits != bits || s.start != start
                || s.duration != duration || !paths.insert(s.path.as_str()))
        {
            return Err(other("invalid direct DNxHR OP-Atom clip profile"));
        }
        for (path, is_picture) in picture.into_iter().map(|v| (&v.path, true))
            .chain(c.audio.iter().map(|audio| (&audio.path, false)))
        {
            let profile = (DirectDnxhrFile { origin, picture: is_picture, display, dnxhr }, start, duration);
            if files.insert(path, profile).is_some_and(|prior| prior != profile) {
                return Err(other("shared direct DNxHR OP-Atom media has conflicting file facts"));
            }
        }
    }
    for c in model.clips.values().filter(|c| !model.direct_dnxhr_origins.contains_key(&c.id)) {
        if c.video.as_ref().is_some_and(|v| files.contains_key(&v.path))
            || c.audio.iter().any(|a| files.contains_key(&a.path))
        {
            return Err(other("direct DNxHR OP-Atom media is shared with an unqualified clip"));
        }
    }
    if model.sequences.iter().any(|s| s.events.iter().any(|e| {
        model.direct_dnxhr_origins.contains_key(&e.clip)
            && model.clips[&e.clip].video.as_ref().map(|v| v.rate).or(model.clips[&e.clip].rate)
                .is_some_and(|rate| s.rate != rate)
    })) {
        return Err(other("direct DNx OP-Atom needs a same-rate sequence"));
    }
    Ok(files.into_iter().map(|(path, (file, _, _))| (path.clone(), file)).collect())
}

fn prepare_view(model:&WriterInput,legacy:&Model,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool,layout:Option<&DestinationBinLayout>,offline:&HashSet<&str>,linked_op1a:&HashSet<String>,selective:bool,dnxhr_444_rgb_sources:&HashSet<String>)->Result<PreparedProject> {
    let direct_dnxhr = direct_dnxhr_files(model, if linked_op1a.is_empty() { media } else { &[] }, native_av)?;
    let sources: HashSet<_> = media.iter().map(|binding| binding.source_path.as_str()).collect();
    dissolve_handles(model)?;
    for p in model.picture_effects {
        if matches!(p.effect, PictureEffect::CornerPin(_) | PictureEffect::CornerPinWithFlop { .. }) {
            let seq = &model.sequences[p.sequence];
            let edit = &seq.events[p.event];
            let unit = ticks_per_unit(seq.rate);
            if !model.picture_clock(&edit.clip).is_some_and(|(rate, duration)| rate == seq.rate
                && edit.source_in >= 0 && edit.source_in % unit == 0
                && edit.duration > 0 && edit.duration % unit == 0
                && edit.source_in.checked_add(edit.duration).is_some_and(|end| end <= duration))
            {
                return Err(other("Corner Pin needs same-rate picture containing the edit"));
            }
        }
        if let PictureEffect::TrimmedWarp(trim) | PictureEffect::TrimmedResize(trim) = p.effect {
            let seq = &model.sequences[p.sequence];
            let clip = &seq.events[p.event].clip;
            if !model.picture_clock(clip).is_some_and(|(rate, end)| rate == seq.rate && trim.keys_fit_media(end)) {
                return Err(other("trimmed motion needs same-rate picture containing every saved key"));
            }
        }
    }
    if !model.multicams.is_empty() {
        let picture_bindings: HashMap<_, _> = media.iter().filter(|m|m.audio_stream.is_none())
            .map(|m|(m.source_path.as_str(), m.target_path.as_str())).collect();
        for g in model.multicams {
            for c in &g.cameras {
                let video = model.clips[&c.clip].video.as_ref().ok_or_else(|| other("multicam camera has no picture"))?;
                if native_av {
                    if !picture_bindings.get(video.path.as_str()).is_some_and(|p|p.to_ascii_lowercase().ends_with(".mxf")) {
                        return Err(other("multicam_native_route_required: camera picture needs an OP1a MXF binding"));
                    }
                } else if model.options.bin_mode != "separate"
                    || !model.direct_dnxhr_origins.contains_key(&c.clip)
                    || video.rate != super::multicam::RATE
                {
                    return Err(other("multicam_native_route_required: editable Groups need qualified direct DNx LB mono siblings in Separate files mode or native OP1a bindings"));
                }
            }
        }
    }
    if !native_av && model.clips.values().any(|c| !linked_op1a.contains(&c.id) && !c.multichannel_groups.is_empty()) { return Err(other("multichannel groups require the explicit native media route")); }
    if !native_av && model.clips.values().any(|c| !linked_op1a.contains(&c.id) && c.video.as_ref().is_some_and(|v| v.codec == "apch")) {
        return Err(other("generated Freeze Frame requires the native media route with a prepared MOV"));
    }
    if native_av { super::native_av::validate_bindings(legacy,media)?; }
    if !linked_op1a.is_empty() {
        if !selective && (native_av || model.options.bin_mode != "separate" || !model.multicams.is_empty()
            || !model.av_subclips.is_empty())
        {
            return Err(other("managed OP1a linking requires Separate files and a standard timeline"));
        }
        let selected: Vec<_> = legacy.clips.values().filter(|c| linked_op1a.contains(&c.id)).collect();
        if selected.len() != linked_op1a.len() {
            return Err(other("managed OP1a writer clip ID is missing"));
        }
        if selective {
            if legacy.clips.values().filter(|clip| !linked_op1a.contains(&clip.id)).any(|clip|
                clip.video.as_ref().is_some_and(|video| sources.contains(video.path.as_str()))
                || clip.audio.iter().any(|audio| sources.contains(audio.path.as_str()))) {
                return Err(other("rewrap would change a media path shared with a directly linked clip"));
            }
        }
        let constant_speed_edits: HashSet<_> = model.constant_speeds.iter()
            .map(|speed| (speed.sequence, speed.event))
            .collect();
        for (sequence_index, sequence) in model.sequences.iter().enumerate() {
            for (event_index, event) in sequence.events.iter().enumerate() {
                if !selective && linked_op1a.contains(&event.clip)
                    && (sequence.rate != Fraction::new(24, 1)
                        || event.speed_ratio.is_some()
                        || event.time_remap.is_some()
                        || constant_speed_edits.contains(&(sequence_index, event_index)))
                {
                    return Err(other(format!(
                        "managed OP1a: sequence {:?}, edit {} requires a same-rate unretimed placement",
                        sequence.name, event_index + 1,
                    )));
                }
            }
        }
        let subset = Model {
            project_name: legacy.project_name.clone(), options: legacy.options.clone(),
            clips: selected.iter().map(|clip| (*clip).clone()).collect(),
            bins: vec![super::model::Bin {
                name: "Managed OP1a".into(),
                clips: selected.iter().map(|clip| clip.id.clone()).collect(),
            }],
            sequences: vec![],
        };
        super::native_av::validate_bindings(&subset, media)?;
    }
    super::rewrap::validate_audio_formats(legacy, media)?;
    let out_path = py_path(&out_path.as_ref().to_string_lossy());
    if out_path.exists() {
        return Err(WriteError::Exists(format!("{} exists; the converter never overwrites", out_path.display())));
    }
    let parent = py_parent(&out_path);
    if !parent.is_dir() {
        return Err(WriteError::NotFound(format!("output folder {} does not exist", parent.display())));
    }
    let mut report = Report::default();
    report.notes.push(format!("bin mode: {}", model.options.bin_mode));
    report.notes.push(
        match model.options.bin_mode.as_str() {
            "separate" => "timeline: linked source clips",
            "merged" => "timeline: merged clips (placed as their linked components)",
            "grouped" => "timeline: grouped (multicam) clips",
            other_mode => return Err(other(format!("KeyError: {}", py_str_repr(other_mode)))),
        }
        .to_string(),
    );
    if native_av && !legacy.clips.is_empty() {
        report.notes.clear();
        report.notes.push("clip construction: native OP1a-backed AV masters; explicit route replaces bin mode".into());
        report.notes.push("timeline: native source groups routed from the Avid sequence".into());
        report.unverified.push("Native OP1a AV masters: profiles beyond retained 23.976 fps checks with two PCM16/24 or three PCM24 mono streams require Premiere read-back; User acceptance remains pending".into());
    }
    if native_av && media.iter().any(|m|m.target_path.to_ascii_lowercase().ends_with(".mov")) {
        report.notes[0]="clip construction: native prepared-media AV masters (DNx OP1a / ProRes MOV); explicit route replaces bin mode".into();
        report.unverified[0]="Native ProRes MOV AV masters: bounded square-pixel 1080p24 LT/PCM16/24 research read-back; wider profiles and User acceptance remain pending".into();
    }
    if native_av && model.clips.values().any(|c| c.video.as_ref().is_some_and(|v| v.codec == "apch")) {
        report.notes.push("Traditional Freeze Frame uses its generated ProRes HQ picture as a regular clip".into());
    }
    for c in model.clips.values().filter(|_|native_av) {
        for group in &c.multichannel_groups {
            report.notes.push(format!("clip {}: {} group {:?} -> {}",py_str_repr(&c.name),group.format,group.channels,if group.format=="5.1" {"Premiere 5.1 L,R,C,LFE,Ls,Rs"}else{"8-channel Adaptive, Avid L,C,R,Lss,Rss,Lsr,Rsr,LFE order; no automatic 7.1 speaker bus"}));
        }
    }
    if !model.originals.is_empty() {report.notes.push("File-backed masters use explicit stream/channel mapping.".into());report.unverified.push("File-backed masters and mixed timelines require native Premiere open/save/reopen, channel/sample checks and User acceptance".into());}
    let mut project = Project::new_with_version(model.project_version, ids)?;
    project.set_path(&out_path); // relative media paths are computed against the output location
    let Some(project_root) = project.root_item() else { return Err(other("project has no root item")) };
    let root = if let Some(layout) = layout { project.add_bin(project_root, &layout.root_name, ids)? } else { project_root };
    let mut table = MediaTable::new();
    table.direct_dnxhr = direct_dnxhr;
    for binding in media {
        if let Some(aspect) = binding.pixel_aspect {
            binding.validate_pixel_aspect()?;
            table.rewrap_aspects.insert(binding.source_path.clone(), aspect);
        }
    }
    if native_av || selective {
        for c in model.clips.values().filter(|c| native_av || linked_op1a.contains(&c.id)) {
            if let Some(v)=&c.video {
                if media.iter().any(|m| m.source_path == v.path && (m.separate_mono
                    || (c.audio.len() > 2 && m.target_path.to_ascii_lowercase().ends_with(".mov")))) {
                    table.mono_views.insert(v.path.clone());
                }
            }
        }
    }
    let bound: HashSet<&str> = media.iter().filter(|m| m.audio_stream.is_none())
        .map(|m| m.source_path.as_str()).collect();
    let mut format_clips: Vec<_> = model.clips.values().collect();
    if native_av { format_clips.sort_by_key(|c| &c.id); }
    for clip in format_clips {
        if let Some(video) = &clip.video {
            if bound.contains(video.path.as_str()) {
                let format = super::rewrap::video_format(clip, video.rate)?;
                if format == "109" {
                    report.unverified.push(format!("clip {}: extended {} fps uses frame-count display; native high-rate timecode display remains unqualified",py_str_repr(&clip.name), video.rate));
                }
                table.rewrap_formats.insert(video.path.clone(), format);
            }
        }
    }
    let mut video_dissolves: HashMap<_, Vec<_>> = HashMap::new();
    for d in model.video_dissolves {
        video_dissolves.entry(d.sequence).or_default().push(d);
    }
    let mut audio_crossfades: HashMap<_, Vec<_>> = HashMap::new();
    for fade in model.audio_crossfades {
        audio_crossfades.entry(fade.sequence).or_default().push(fade);
    }
    let mut writer = Writer {
        model,
        rewrapped: selective || (!media.is_empty() && linked_op1a.is_empty()),
        native_av,
        linked_op1a,
        project,
        media: table,
        ids,
        report,
        saved_marker_data: HashSet::new(),
        saved_marker_colors: HashSet::new(),
        root,
        sequence_parent: layout.map(|_| root),
        out_path: out_path.clone(),
        assets: Vec::new(),
        asset_names: HashSet::new(),
        extended_luts: HashMap::new(),
        items: HashMap::new(),
        placement: HashMap::new(),
        clip_bins: HashMap::new(),
        multicam_items: Vec::new(),
        multicam_uses: model.multicam_uses.iter().map(|u| ((u.sequence,u.event),u)).collect(),
        track_multicam_items: HashMap::new(),
        av_subclip_items: Vec::new(),
        av_subclip_uses: model.av_subclip_uses.iter()
            .map(|u| ((u.sequence, u.event, u.audio_track), u.subclip)).collect(),
        picture_nests: model.picture_nests.iter().map(|n| (n.sequence, n.event)).collect(),
        png_items: Vec::new(),
        png_edits: model.png_mattes.edits.iter().fold(HashMap::new(), |mut index, edit| {
            index.entry(edit.sequence).or_insert_with(Vec::new).push(edit);
            index
        }),
        constant_speeds: model.constant_speeds.iter().map(|&s| ((s.sequence, s.event), s)).collect(),
        video_dissolves,
        audio_crossfades,
        audio_edge_fades: model.audio_edge_fades.iter()
            .map(|fade| ((fade.sequence, fade.event, fade.track), fade)).collect(),
        audio_pans: model.audio_pans.iter()
            .map(|pan| ((pan.sequence, pan.event, pan.track), pan)).collect(),
        disabled_edits: model.disabled_edits,
        edit_labels: model.edit_labels.iter().fold(HashMap::new(), |mut by_edit, &label| {
            by_edit.entry((label.sequence, label.event)).or_insert_with(Vec::new).push(label);
            by_edit
        }),
        picture_effects: model.picture_effects.iter()
            .map(|e| ((e.sequence, e.event), (e.effect, e.reflection_before_motion))).collect(),
        source_luts: model.source_luts.iter().map(|lut| ((lut.sequence, lut.event), lut)).collect(),
    };
    // Borrow component names rather than cloning every complete prefix. This
    // index is linear in the validated references, including repeated paths.
    let mut folders: HashMap<(ItemId, &str), ItemId> = HashMap::new();
    let mut compound_targets = Vec::new();
    for (index, b) in model.bins.iter().enumerate() {
        let mut parent = root;
        if let Some(layout) = layout {
            for component in &layout.parents[index] {
                parent = if let Some(&existing) = folders.get(&(parent, component.as_str())) { existing }
                    else { let child = writer.project.add_bin(parent, component, writer.ids)?;
                        folders.insert((parent, component.as_str()), child); child };
            }
        }
        let target = writer.project.add_bin(parent, &b.name, writer.ids)?;
        if !model.multicams.is_empty() || !model.multicam_tracks.is_empty() || !model.picture_subclips.is_empty() || !model.av_subclips.is_empty()
            || !model.png_mattes.stills.is_empty() || !model.autosync_bins.is_empty()
        {
            compound_targets.push(target);
        }
        for clip_id in &b.clips {
            if writer.items.contains_key(clip_id) {
                writer.report.notes.push(format!("clip {} listed in several bins; imported once into the first", py_str_repr(clip_id)));
                continue;
            }
            writer.import_clip(clip_id, Some(target))?;
        }
    }
    if !model.png_mattes.stills.is_empty() {
        writer.png_items = png_mattes::import(
            &mut writer.project, &model.png_mattes.stills, &compound_targets, writer.ids,
        )?;
        let keys = model.png_mattes.edits.iter().filter(|e| e.foreground.is_some_and(|i| {
            matches!(model.png_mattes.stills[i].alpha, super::prepared::PngAlpha::Straight { .. })
        })).count();
        writer.report.notes.push(format!(
            "Linked {} PNG still interpretations and rebuilt {keys} editable Matte Key nests",
            writer.png_items.len()
        ));
    }
    for seq in model.sequences {
        for e in &seq.events {
            if !writer.items.contains_key(&e.clip) {
                writer.import_clip(&e.clip, layout.map(|_| root))?;
                writer.report.notes.push(format!("clip {} used in a sequence but in no bin; imported at the root", py_str_repr(&e.clip)));
            }
        }
    }
    for subclip in model.picture_subclips {
        let parent = &model.clips[&subclip.parent_clip];
        let video = parent.video.as_ref().ok_or_else(|| other("picture subclip parent has no video"))?;
        writer.project.import_target = Some(compound_targets[subclip.bin]);
        let result = super::opatom::add_video_subclip(
            &mut writer.project, &mut writer.media, video, parent, subclip, writer.ids,
        );
        writer.project.import_target = None;
        result?;
    }
    if !model.picture_subclips.is_empty() {
        writer.report.notes.push(format!(
            "{} reusable linked picture subclip(s)", model.picture_subclips.len()
        ));
    }
    for subclip in model.av_subclips {
        let parent = &model.clips[&subclip.parent_clip];
        writer.project.import_target = Some(compound_targets[subclip.bin]);
        let item = super::native_av::add_linked_av_subclip(
            &mut writer.project, &mut writer.media, parent, subclip, writer.ids,
        )?;
        writer.project.import_target = None;
        if let Some(marker) = subclip.source_marker(parent).map_err(other)? {
            writer.add_clip_marker(item, marker, marker.start)?;
        }
        writer.av_subclip_items.push(item);
    }
    if !model.av_subclips.is_empty() {
        writer.report.notes.push(format!(
            "{} reusable linked A/V subclip(s), {} timeline lane reference(s)",
            model.av_subclips.len(), model.av_subclip_uses.len()
        ));
    }
    for group in model.multicams {
        for camera in &group.cameras {
            if !writer.items.contains_key(&camera.clip) {
                writer.import_clip(&camera.clip, layout.map(|_| root))?;
            }
        }
        let leaf = |c: &super::multicam::PreparedCamera| -> Result<_> {
            let (picture, siblings) = &writer.items[&c.clip];
            let picture = picture.ok_or_else(|| other("multicam camera item not imported"))?;
            Ok((picture, siblings.as_slice(), &model.clips[&c.clip]))
        };
        let leaves = [leaf(&group.cameras[0])?, leaf(&group.cameras[1])?];
        let item = super::multicam::build(&mut writer.project, group, leaves, compound_targets[group.bin], writer.ids)?;
        writer.multicam_items.push(item);
    }
    if !model.multicams.is_empty() {
        let has_one_mono = model.multicams.iter().any(|g| g.audio_cameras.len() == 1);
        let has_three_mono = model.multicams.iter().any(|g| g.audio_cameras.len() == 3);
        let audio = if has_one_mono || has_three_mono {
            "each Group's mono outputs use its saved camera selections"
        } else if model.multicams.iter().all(|g| g.audio_cameras == [0; 2]) {
            "each Group has two default mono outputs from its first camera"
        } else {
            "each Group's two default mono outputs use its saved camera selections"
        };
        writer.report.notes.push(format!(
            "{} editable Premiere multicam source item(s); {audio}",
            model.multicams.len(),
        ));
        let alternate_edits = model.multicam_uses.iter().flat_map(|usage| &usage.audio)
            .filter(|choice| matches!(choice, super::multicam::PreparedGroupAudio::LinkedAlternate(_)))
            .count();
        if alternate_edits != 0 {
            writer.report.notes.push(format!(
                "{alternate_edits} alternate Group audio timeline edit(s) link directly to the selected camera's original mono MXF"
            ));
        }
        if has_three_mono {
            writer.report.notes.push("Three-mono Group scope: two continuous 23.976 DNx LB cameras with discrete mono PCM24 siblings. Picture angles remain editable in Premiere. Independent audio switches use linked mono clips.".into());
        } else if !has_one_mono {
            writer.report.unverified.push("Editable Group scope: two continuous same-rate DNx cameras with two mono PCM24 lanes; User acceptance pending. Alternate audio on the qualified linked route is a direct mono clip. Retimed or discontinuous alternatives and wider audio are outside this profile.".into());
        }
        if has_one_mono {
            writer.report.notes.push("One-mono Group scope: two same-rate DNx cameras with one saved mono PCM24 lane. Picture and audio default camera selections remain independent.".into());
        }
    }
    let editor_sequence_parent = writer.sequence_parent;
    for group in model.multicam_tracks {
        let target = *compound_targets.get(group.bin).ok_or_else(|| other("multicam source bin was not prepared"))?;
        for piece in group.picture.iter().flatten().chain(&group.mono) {
            if !writer.placement.contains_key(&piece.clip) {
                writer.import_clip(&piece.clip, Some(target))?;
            }
        }
        let leaf = |id: &str| {
            let (_, Payload::Original(item)) = writer.placement.get(id)? else { return None };
            Some((*item, *model.originals.get(id)?))
        };
        let item = super::multicam_tracks::build(
            &mut writer.project, group, &model.sequences[group.sequence], target, writer.ids, leaf,
        )?;
        writer.track_multicam_items.entry(group.sequence).or_default().push((item, group));
    }
    for (si, seq) in model.sequences.iter().enumerate() {
        writer.sequence_parent = model.autosync_bins.get(&si)
            .map(|&bin| compound_targets[bin]).or(editor_sequence_parent);
        writer.write_sequence(seq, si)?;
    }
    if !model.autosync_bins.is_empty() {
        writer.report.notes.push(format!(
            "{} reusable AutoSync source sequence(s). To keep individual mic lanes on insertion, turn off Insert and Overwrite Sequences as Nests.",
            model.autosync_bins.len(),
        ));
    }
    if !model.picture_nests.is_empty() {
        writer.report.notes.push(format!("{} editable picture Submaster nest(s); child source trimmed to the saved Avid interval", model.picture_nests.len()));
    }
    if !model.layered_submasters.is_empty() {
        writer.report.notes.push(format!("{} editable two-layer Avid Submaster nest(s)", model.layered_submasters.len()));
        writer.report.unverified.push("Layered Submaster picture parity requires a native Avid render and Premiere reopen comparison.".into());
    }
    if !media.is_empty() || !dnxhr_444_rgb_sources.is_empty() {
        super::rewrap::apply_views(
            &mut writer.project, media, !native_av && linked_op1a.is_empty(), dnxhr_444_rgb_sources,
        )?;
        if linked_op1a.is_empty() {
            writer.report.unverified.push("OP1a media binding: explicit flattened source channels; native read-back covers retained fixtures, not every profile or clip-construction mode".into());
        } else if selective {
            writer.report.notes.push("Only selected managed clips reference newly rewrapped media.".into());
        } else {
            writer.report.notes.push("Verified managed OP1a masters link to their existing MXFs in place.".into());
        }
    }
    if !offline.is_empty() {
        let root = writer.project.doc.root;
        let tree = &mut writer.project.doc.tree;
        let mut marked = HashSet::new();
        for node in tree.children(root).to_vec() {
            if tree.tag(node) != "Media" { continue; }
            let Some(path) = tree.findtext(node, "FilePath")? else { continue; };
            if !offline.contains(path.as_str()) { continue; }
            marked.insert(path);
            let reason = tree.new_element("OfflineReason");
            tree.set_text(reason, Some("5"));
            crate::xml::mutations::append_uniform_child(tree, node, reason)?;
        }
        if marked.len() != offline.len() {
            return Err(other("offline media is missing a Premiere locator"));
        }
    }
    let saved=writer.project.prepare_new_save(&out_path)?;
    let assisted_media = super::media_donor::AssistedMedia::from_clips(model.originals.values().copied())?;
    let assisted_media_pending = assisted_media.paths().len() != 0;
    let prepared=PreparedProject {saved:Some(saved),out_path,report:writer.report,assets:writer.assets,
        assets_attempted:false,assisted_media_pending,assisted_media,assets_published:false,published:false,retained:Vec::new(),
        saved_marker_data:writer.saved_marker_data,saved_marker_colors:writer.saved_marker_colors};
    prepared.recheck_outputs()?;
    Ok(prepared)
}
