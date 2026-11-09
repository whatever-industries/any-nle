//! Write a native Premiere project from the neutral edit model (port of
//! `converter/src/avid2prproj/writer.py`).
//!
//! [`write_project`] is the whole conversion: bins and clips (separate, merged, grouped and
//! stereo builds), sequences with linked placements at exact ticks, speed, audio gain / volume
//! / pan / track levels / crossfades, Avid colour correction as an embedded Lumetri LUT (plus a
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

use super::audio::{add_crossfade, set_clip_gain, set_clip_volume, set_track_level, split_overflow};
use super::avid_color;
use super::colors;
use super::combined::{
    combined_order, create_grouped_with_display as create_grouped, create_merged, create_stereo_nest, declare_stereo_chain, display_format,
    group_for_channel, move_to_processed_clips, narrow_to_group, parts_bin, set_clip_pan, Pan,
};
use super::lumetri::{apply_lumetri_lut, cube_text};
use super::markers as mk;
use super::matte::apply_matte;
use super::motion::{apply_flip, apply_motion_keys};
use super::time_remap::set_time_remap;
use super::metadata;
use super::prepared::{PictureEffect, PreparedPictureEffect, PreparedText, PreparedVideoDissolve};
use super::model::{event_audio_files, py_str_repr, ticks_per_unit, Clip, ColorCorrection, Event, EventAudio, Fraction, Marker, Model, ModelError, Sequence};
use super::opatom::{add_audio_item, add_video_item, source_item_names, MediaTable, OpatomError, UnverifiedShape};
use super::placement::normalize_placement;
use crate::build::fmt::{fmt_f, py_repr};
use crate::build::sequence::{extra_formats, formats};
use crate::build::time::{floor_div, Time};
use crate::error::Error;
use crate::ids::UuidSource;
use crate::model::project::{py_int, py_path};
use crate::model::{ItemId, Project, SequenceId, TrackId, TrackItemId};

#[path = "writer_destination_layout.rs"]
mod destination_layout;
pub use destination_layout::DestinationBinLayout;

/// Lumetri LUT edge length.
pub const LUT_SIZE: i64 = 33;

/// Presets of py_premiere's `FORMATS` the converter uses as they are.
const VERIFIED_PRESETS: [&str; 5] = ["1080p23976", "1080p25", "1080p2997", "1080p50", "1080p5994"];

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
    for (preset, f) in extra_formats() {
        if matches(f.frame_rate, f.width, f.height) {
            report.notes.push(format!(
                "sequence {}: converter format {preset} (User-reported in Premiere 26.5.2, PRPROJ-0010)",
                py_str_repr(name)
            ));
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
    color: ColorCorrection,
    title: String,
    sha256: String,
}
impl PreparedLut {
    fn bytes(&self) -> Result<Vec<u8>> {
        let curves = self.color.curves.iter().map(|(k,v)|
            (k.clone(),v.iter().map(|&(x,y)|(x*255.0,y*255.0)).collect())).collect::<Vec<_>>();
        let transform=avid_color::rgb_transform(&self.color.tone,self.color.saturation,&curves)?;
        let points=avid_color::cube_lut(&*transform,LUT_SIZE)?;
        let bytes=cube_text(&points,LUT_SIZE,&self.title).into_bytes();
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
    assets_published: bool,
    published: bool,
    retained: Vec<PathBuf>,
}
impl PreparedProject {
    pub fn report(&self) -> &Report { &self.report }
    pub fn output_path(&self) -> &Path { &self.out_path }
    pub fn retained_paths(&self) -> &[PathBuf] { &self.retained }
    pub fn is_published(&self) -> bool { self.published }
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
    clips:LegacyClips<'a>,
    originals:HashMap<&'a str,&'a super::prepared::OriginalClip>,
    bins:&'a [super::model::Bin], sequences:&'a [Sequence],
    texts:&'a [super::prepared::PreparedText],
    video_dissolves: &'a [PreparedVideoDissolve],
    picture_effects: &'a [PreparedPictureEffect],
    multicams: &'a [super::multicam::PreparedMulticam],
    multicam_uses: &'a [super::multicam::PreparedMulticamUse],
}
impl WriterInput<'_> {
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
    project: Project,
    media: MediaTable,
    ids: &'a mut dyn UuidSource,
    report: Report,
    root: ItemId,
    sequence_parent: Option<ItemId>,
    out_path: PathBuf,
    assets: Vec<PreparedLut>,
    asset_names: HashSet<String>,
    /// clip id -> (video item, audio items in model order)
    items: HashMap<String, (Option<ItemId>, Vec<ItemId>)>,
    /// clip id -> ("separate" | "merged" | "grouped", payload)
    placement: HashMap<String, (&'static str, Payload)>,
    /// clip id -> the bin it was imported into (`None` = the root)
    clip_bins: HashMap<String, Option<ItemId>>,
    multicam_items: Vec<ItemId>,
    multicam_uses: HashMap<(usize, usize), &'a super::multicam::PreparedMulticamUse>,
    video_dissolves: HashMap<usize, Vec<&'a PreparedVideoDissolve>>,
    picture_effects: HashMap<(usize, usize), PictureEffect>,
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
    fn add_clip_marker(&mut self, item: ItemId, marker: &Marker, start: i64) -> Result<()> {
        mk::add_clip_marker(&mut self.project, item, marker, start, &self.model.options, &mut self.report.notes, self.ids)?;
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
            for m in &c.metadata.markers {self.add_clip_marker(item,m,m.start)?;}
            self.items.insert(clip_id.into(),(c.video.as_ref().map(|_|item),vec![]));
            self.clip_bins.insert(clip_id.into(),target);
            self.placement.insert(clip_id.into(),("original",Payload::Original(item)));
            return Ok(());
        }
        let clip = &model.clips[clip_id];
        if self.native_av {
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
                    self.report.unverified.push(format!("clip {name}: markers on a merged clip (time base unverified)"));
                }
                self.placement.insert(
                    clip_id.to_string(),
                    ("merged", Payload::Parts { video: Some(video_copy), audio: audio_copies, stereo: Vec::new() }),
                );
            } else {
                let grouped_item = create_grouped(&mut self.project, clip, Some(video), &combined, target, &clip.name, clip.video.as_ref().and_then(|v|self.media.rewrap_formats.get(&v.path)).and_then(|s|s.parse().ok()), self.ids)?;
                self.apply_metadata(grouped_item, clip)?;
                label_items(&mut self.project, clip, &[grouped_item], model.options)?;
                self.build_markers(clip, grouped_item, None)?;
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
        let own_frames: Vec<Option<i64>> = own_ms.iter().map(|o| o.source).collect();
        let source_frames: Vec<i64> = source_ms.iter().map(|m| m.start).collect();
        let edit_wins = options.marker_conflict_winner == "edit";
        let (mut keep_source, mut keep_own) = (Vec::new(), Vec::new());
        for &m in source_ms {
            if edit_wins && own_frames.contains(&Some(m.start)) {
                if options.marker_conflict_loser_to_sequence {
                    extra.push((m.clone(), timeline(m.start)?, "clip marker replaced on this edit".to_string()));
                }
            } else {
                keep_source.push(m);
            }
        }
        for &o in own_ms {
            if !edit_wins && o.source.is_some_and(|s| source_frames.contains(&s)) {
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
    fn marked(&mut self, item: ItemId, source_ms: &[&Marker], own_ms: &[&Marker], base: i64, e: &Event, extra: &mut Vec<Extra>) -> Result<()> {
        let where_ = py_str_repr(&self.project.item_name(item));
        let (keep_source, keep_own) = self.settle(e, source_ms, own_ms, extra, &where_)?;
        for m in keep_source {
            self.add_clip_marker(item, m, add(base, m.start)?)?;
        }
        for o in keep_own {
            let Some(source) = o.source else {
                return Err(other("unsupported operand type(s) for +: 'int' and 'NoneType'"));
            };
            self.add_clip_marker(item, o, add(base, source)?)?;
        }
        Ok(())
    }

    /// `edit_copy`: Premiere keeps markers on the master clip, so an edit with markers of its own
    /// plays a copy of the clip carrying the source markers plus its own (User decision
    /// 2026-10-15). Returns the payload this edit is placed from.
    #[allow(clippy::too_many_arguments)]
    fn edit_copy(
        &mut self,
        seq: &Sequence,
        i: usize,
        e: &Event,
        kind: &str,
        payload: Payload,
        own: &[&Marker],
        extra: &mut Vec<Extra>,
    ) -> Result<Payload> {
        let model = self.model;
        if let Some(c)=model.originals.get(e.clip.as_str()) {
            let target=self.clip_bins.get(&e.clip).copied().flatten().unwrap_or(self.root);
            let name=format!("{} [edit {}]",c.metadata.name,timecode(e.record_start,seq.rate,seq.start)?);
            let previous=self.project.import_target.replace(target);
            let copy=super::original::add_item(&mut self.project,c,&name,self.ids);
            self.project.import_target=previous;let copy=copy?;
            metadata::apply(&mut self.project,copy,c.metadata.avid.as_ref())?;
            self.marked(copy,&c.metadata.markers.iter().collect::<Vec<_>>(),own,0,e,extra)?;
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
            for o in own {
                let track = o.track.clone().unwrap_or_else(|| "None".to_string());
                extra.push(((*o).clone(), o.start, format!("on this {track} edit (merged clips cannot carry per-edit markers yet)")));
            }
            return Ok(payload);
        }
        if kind == "grouped" {
            let combined: Vec<ItemId> = combined_order(clip)?.into_iter().map(|j| audio[j]).collect();
            let copy = create_grouped(&mut self.project, clip, video, &combined, target, &name, clip.video.as_ref().and_then(|v|self.media.rewrap_formats.get(&v.path)).and_then(|s|s.parse().ok()), self.ids)?;
            self.apply_metadata(copy, clip)?;
            label_items(&mut self.project, clip, &[copy], model.options)?;
            let all: Vec<&Marker> = clip.markers.iter().collect();
            self.marked(copy, &all, own, 0, e, extra)?;
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

    fn write_sequence(&mut self, seq: &Sequence, sequence_index: usize) -> Result<()> {
        let model = self.model;
        let options = &model.options;
        let seq_name = py_str_repr(&seq.name);
        let mut preset = preset_bound(&seq.name, seq.rate, seq.width, seq.height, &mut self.report, self.rewrapped)?;
        if self.native_av {
            let width=model.native_sequence_channels(seq)?;
            if width>0 {
                // Wrap the selected destination preset. Audio grouping must not
                // re-derive its display clock (e.g. change existing DF to NDF).
                preset = format!("anynle-grouped/{width}/{preset}");
            }
        }
        let needed = seq.events.iter().flat_map(|e| e.audio_tracks.iter().copied().chain(e.stereo.iter().map(|s| s.audio_track)))
            .chain(seq.track_levels.iter().map(|t| t.track))
            .chain(seq.audio_crossfades.iter().map(|f| f.track))
            .max().unwrap_or(0) as usize;
        let base = crate::build::sequence::format_by_name(&preset).ok_or_else(|| UnverifiedShape("unknown sequence preset".into()))?;
        if needed > base.audio_tracks {
            if base.mono_discrete { return Err(UnverifiedShape("expanding a mono-discrete output bus is not qualified".into()).into()); }
            preset = format!("anynle-tracks/{needed}/{preset}");
            self.report.notes.push(format!("sequence {seq_name}: expanded audio track capacity to {needed}; output bus unchanged"));
            self.report.unverified.push(format!("sequence {seq_name}: expanded audio-track count requires fresh native Premiere read-back; structural checks do not qualify playback"));
        }
        let text_tracks = model.texts.iter().filter(|t| t.sequence == sequence_index && t.kind == super::prepared::PreparedTextKind::Graphic).map(|t| t.track as usize).max().unwrap_or(0);
        let sequence = self.project.add_sequence(&seq.name, &preset, (text_tracks > base.video_tracks).then_some(text_tracks), self.ids)?;
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
        for (i, e) in seq.events.iter().enumerate() {
            let group_use = self.multicam_uses.get(&(sequence_index, i)).copied();
            let picture_effect = self.picture_effects.get(&(sequence_index, i)).copied();
            let (kind, mut payload) = self.placement[&e.clip].clone();
            let own: Vec<&Marker> =
                seq.markers.iter().filter(|m| m.owner == "edit" && m.event == Some(i as i64) && !m.inherited).collect();
            if !own.is_empty() {
                payload = self.edit_copy(seq, i, e, kind, payload, &own, &mut extra_markers)?;
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
            if let Some(u) = group_use {
                let source = self.multicam_items[u.group];
                if let Some(v) = e.video_track {
                    placed.push(self.project.add_clip(track_at(&vtracks,v)?, source, start, self.ids)?);
                }
                let clip = clip.ok_or_else(|| other("multicam leaf registry mismatch"))?;
                let files = event_audio_files(clip, e)?;
                for (j, (&channel, &track)) in u.audio.iter().zip(&e.audio_tracks).enumerate() {
                    let item = self.project.add_clip(track_at(&atracks,track)?, source, start, self.ids)?;
                    self.project.narrow_merged_channel(item, channel as i64)?;
                    // The inner clip carries the leaf gain. Only an edit override's
                    // difference belongs on this outer audio placement.
                    let original_gain = files[j].gain_db;
                    master_gain.insert(item, edit_value(&e.audio_gains,j)?.map_or(0.0, |g| g - original_gain));
                    edit_pan.insert(item, (edit_value(&e.audio_pans,j)?,track));
                    placed.push(item);
                }
            } else { match &payload {
                Payload::Original(source)=>{
                    let c=original.ok_or_else(||other("original clip registry mismatch"))?;
                    if let Some(v)=e.video_track {placed.push(self.project.add_clip(track_at(&vtracks,v)?,*source,start,self.ids)?);}
                    let labels:Vec<_>=if e.audio_channels.is_empty(){c.channels.iter().take(e.audio_tracks.len()).map(|a|a.avid_track).collect()}else{e.audio_channels.clone()};
                    for (j,(&lane,&track)) in labels.iter().zip(&e.audio_tracks).enumerate(){
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
                        let Some(video_item) = *video_item else {
                            return Err(other("'NoneType' object has no attribute '_sequence_uid'"));
                        };
                        placed.push(self.project.add_clip(track_at(&vtracks, v)?, video_item, start, self.ids)?);
                    }
                    for (j, ((&audio_item, &track), f)) in event_items.iter().zip(&e.audio_tracks).zip(&event_files).enumerate() {
                        let item = self.project.add_clip(track_at(&atracks, track)?, audio_item, start, self.ids)?;
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
            let rect = if group_use.is_some() { Some("0,0,1920,1080".into()) } else { match (kind, clip.and_then(|c|c.video.as_ref())) {
                ("grouped", Some(v)) => Some(format!("0,0,{},{}", v.width, v.height)),
                _ => None,
            }};
            for &t in &placed {
                let mut span = e.duration;
                if let Some(ratio) = e.speed_ratio {
                    set_playback_speed(&mut self.project, t, ratio)?;
                    span = e
                        .source_duration
                        .ok_or_else(|| other("unsupported operand type(s) for +: 'int' and 'NoneType'"))?;
                }
                // A remapped clip's In/Out are positions on its remap curve (see writer.py).
                let start = if let Some(u) = group_use { u.source_in } else if e.time_remap.is_some() { ticks_per_unit(seq.rate) } else { e.source_in };
                self.project.set_in_point(t, Time::new(start))?;
                self.project.set_out_point(t, Time::new(add(start, span)?))?;
                self.project.set_end(t, Time::new(add(e.record_start, e.duration)?))?;
                let is_video = self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem";
                let r = if is_video { rect.as_deref() } else { None };
                normalize_placement(&mut self.project, t, r)?;
                if is_video {
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
                self.report.notes.push(format!(
                    "sequence {seq_name} event {i}: Avid Timewarp as Time Remapping, one key per frame from Avid's own sampled positions (provisional)"
                ));
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
                let keys = match picture_effect {
                    Some(PictureEffect::Resize(keys)) => Some(keys),
                    _ => None,
                };
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        for n in apply_motion_keys(&mut self.project, t, tf, keys, self.ids)? {
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
            if let Some(PictureEffect::Flip { horizontal, vertical }) = picture_effect {
                for &t in &placed {
                    if self.project.doc.tree.tag(self.project.track_item(t).element) == "VideoClipTrackItem" {
                        apply_flip(&mut self.project, t, horizontal, vertical)?;
                    }
                }
            }
            if let Some(cc) = &e.color_correction {
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
                    self.assets.push(PreparedLut {path:cube.clone(),color:cc.clone(),title,sha256:digest});
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
        for m in &seq.markers {
            let track = m.track.as_deref().unwrap_or("");
            if m.owner == "sequence" {
                let note = if track.is_empty() { String::new() } else { format!("on {track}") };
                mk::add_sequence_marker(&mut self.project, sequence, m, m.start, options, &mut self.report.notes, &note, self.ids)?;
            } else if m.owner == "filler" && track.starts_with('V') {
                filler_video.entry(py_int(&track[1..])?).or_default().push(m);
            } else if m.owner == "filler" {
                // Adjustment layers are video-only.
                let note = format!("on empty {}", m.track.as_deref().unwrap_or("None"));
                mk::add_sequence_marker(&mut self.project, sequence, m, m.start, options, &mut self.report.notes, &note, self.ids)?;
            }
        }
        for (m, start, note) in &extra_markers {
            mk::add_sequence_marker(&mut self.project, sequence, m, *start, options, &mut self.report.notes, note, self.ids)?;
        }
        for (&track_no, ms) in &filler_video {
            // Markers on empty video track space ride one-frame (or marker-length) adjustment
            // layers on that track; the layer's own time equals sequence time.
            if track_no > vtracks.len() as i64 {
                return Err(UnverifiedShape(format!("sequence {seq_name}: filler marker on missing video track {track_no}")).into());
            }
            let mut length = i64::MIN;
            for m in ms {
                length = length.max(add(m.start, m.duration.max(unit))?);
            }
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
            let mut spans: BTreeMap<i64, i64> = BTreeMap::new();
            for m in ms {
                self.add_clip_marker(layer, m, m.start)?;
                let current = spans.get(&m.start).copied().unwrap_or(unit);
                let span = if m.duration != 0 { m.duration } else { unit };
                spans.insert(m.start, current.max(span));
            }
            let mut last_end = -1;
            for (&start, &span) in &spans {
                let end = add(start, span)?;
                if start < last_end {
                    continue; // already covered by the previous instance
                }
                let placed = self.project.add_clip(track_at(&vtracks, track_no)?, layer, Time::new(start), self.ids)?;
                self.project.set_in_point(placed, Time::new(start))?;
                self.project.set_out_point(placed, Time::new(end))?;
                self.project.set_end(placed, Time::new(end))?;
                normalize_placement(&mut self.project, placed, None)?;
                last_end = end;
            }
            self.report
                .notes
                .push(format!("sequence {seq_name}: {} marker(s) on empty V{track_no} carried by an adjustment layer", ms.len()));
        }
        order_track_items(&mut self.project, sequence)?;
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
    prepare_project_with_effects(model, Editorial { texts, video_dissolves, picture_effects: &[] },
        out_path, ids, media, native_av)
}

pub struct Editorial<'a> {
    pub texts: &'a [PreparedText],
    pub video_dissolves: &'a [PreparedVideoDissolve],
    pub picture_effects: &'a [PreparedPictureEffect],
}

pub fn prepare_project_with_effects(model: &Model, editorial: Editorial<'_>,
    out_path: impl AsRef<Path>, ids: &mut dyn UuidSource,
    media: &[super::rewrap::MediaRelink], native_av: bool) -> Result<PreparedProject> {
    super::model::validate_model(model)?;
    let Editorial { texts, video_dissolves, picture_effects } = editorial;
    super::prepared::validate_texts(texts, &model.sequences)?;
    super::prepared::validate_video_dissolves(video_dissolves, &model.sequences)?;
    super::prepared::validate_picture_effects(picture_effects, &model.sequences)?;
    let view=WriterInput{options:&model.options,clips:model.clips.iter().collect(),originals:HashMap::new(),bins:&model.bins,sequences:&model.sequences,texts,video_dissolves,picture_effects,multicams:&[],multicam_uses:&[]};
    prepare_view(&view,model,out_path,ids,media,native_av,None)
}

/// Validated typed input for mixed original-file and legacy media; model/1 remains strict.
pub fn prepare_typed_project(input:&super::prepared::PreparedProjectInput,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool)->Result<PreparedProject> {
    prepare_typed_project_impl(input,out_path,ids,media,native_av,None)
}

/// Prepare selected source bins under one import root and their relative folder hierarchy.
/// Layout validation completes before UUID allocation or project construction.
pub fn prepare_typed_project_with_layout(input:&super::prepared::PreparedProjectInput,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool,layout:&DestinationBinLayout)->Result<PreparedProject> {
    prepare_typed_project_impl(input,out_path,ids,media,native_av,Some(layout))
}
fn prepare_typed_project_impl(input:&super::prepared::PreparedProjectInput,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool,layout:Option<&DestinationBinLayout>)->Result<PreparedProject> {
    if let Some(layout) = layout { layout.validate(&input.bins)?; }
    input.validate()?;
    let legacy=Model{project_name:input.project_name.clone(),options:input.options.clone(),clips:input.clips.iter().filter_map(|c|match c{super::prepared::PreparedClip::Legacy(c)=>Some(c.clone()),_=>None}).collect(),bins:input.bins.iter().map(|b|super::model::Bin{name:b.name.clone(),clips:b.clips.iter().filter(|id|input.clips.iter().any(|c|matches!(c,super::prepared::PreparedClip::Legacy(c) if c.id==**id))).cloned().collect()}).collect(),sequences:input.sequences.iter().map(|s|{let mut s=s.clone();s.events.retain(|e|input.clips.iter().any(|c|matches!(c,super::prepared::PreparedClip::Legacy(c) if c.id==e.clip)));s.markers.clear();s}).collect()};
    let view=WriterInput{options:&input.options,clips:legacy.clips.iter().collect(),originals:input.clips.iter().filter_map(|c|match c{super::prepared::PreparedClip::Original(c)=>Some((c.metadata.id.as_str(),c)),_=>None}).collect(),bins:&input.bins,sequences:&input.sequences,texts:&input.texts,video_dissolves:&input.video_dissolves,picture_effects:&input.picture_effects,multicams:&input.multicams,multicam_uses:&input.multicam_uses};
    prepare_view(&view,&legacy,out_path,ids,media,native_av,layout)
}
fn dissolve_handles(model: &WriterInput<'_>) -> Result<()> {
    if model.video_dissolves.is_empty() { return Ok(()); }
    let groups: HashSet<_> = model.multicam_uses.iter().map(|u| (u.sequence, u.event)).collect();
    for d in model.video_dissolves {
        let s = &model.sequences[d.sequence];
        let out = &s.events[d.outgoing];
        let inc = &s.events[d.incoming];
        let media = |e: &Event| {
            model.clips.get(&e.clip).and_then(|c| c.video.as_ref()).map(|v| (v.rate, v.duration))
                .or_else(|| model.originals.get(e.clip.as_str()).and_then(|c| c.video.as_ref()).map(|v| (v.rate, v.duration)))
        };
        let after = d.duration - d.before_cut;
        let qualified = media(out).is_some_and(|(rate, duration)| rate == s.rate
            && i128::from(out.source_in) + i128::from(out.duration) + i128::from(after) <= i128::from(duration))
            && media(inc).is_some_and(|(rate, _)| rate == s.rate && inc.source_in >= d.before_cut)
            && !groups.contains(&(d.sequence, d.outgoing)) && !groups.contains(&(d.sequence, d.incoming));
        if !qualified {
            return Err(other(format!("transition_handles_unavailable: {} video dissolve needs same-rate picture with both source handles", s.name)));
        }
    }
    Ok(())
}
fn prepare_view(model:&WriterInput,legacy:&Model,out_path:impl AsRef<Path>,ids:&mut dyn UuidSource,media:&[super::rewrap::MediaRelink],native_av:bool,layout:Option<&DestinationBinLayout>)->Result<PreparedProject> {
    dissolve_handles(model)?;
    if !model.multicams.is_empty() {
        if !native_av {
            return Err(other("multicam_native_route_required: editable Groups require native OP1a bindings"));
        }
        let picture_bindings: HashMap<_, _> = media.iter().filter(|m|m.audio_stream.is_none())
            .map(|m|(m.source_path.as_str(), m.target_path.as_str())).collect();
        for g in model.multicams {
            for c in &g.cameras {
                let path = &model.clips[&c.clip].video.as_ref().ok_or_else(|| other("multicam camera has no picture"))?.path;
                if !picture_bindings.get(path.as_str()).is_some_and(|p|p.to_ascii_lowercase().ends_with(".mxf")) {
                    return Err(other("multicam_native_route_required: camera picture needs an OP1a MXF binding"));
                }
            }
        }
    }
    if !native_av && model.clips.values().any(|c| !c.multichannel_groups.is_empty()) { return Err(other("multichannel groups require the explicit native media route")); }
    if native_av { super::native_av::validate_bindings(legacy,media)?; }
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
    if native_av {
        report.notes.clear();
        report.notes.push("clip construction: native OP1a-backed AV masters; explicit route replaces bin mode".into());
        report.notes.push("timeline: native source groups routed from the Avid sequence".into());
        report.unverified.push("Native OP1a AV masters: profiles beyond retained 23.976 fps checks with two PCM16/24 or three PCM24 mono streams require Premiere read-back; User acceptance remains pending".into());
    }
    if native_av && media.iter().any(|m|m.target_path.to_ascii_lowercase().ends_with(".mov")) {
        report.notes[0]="clip construction: native prepared-media AV masters (DNx OP1a / ProRes MOV); explicit route replaces bin mode".into();
        report.unverified[0]="Native ProRes MOV AV masters: bounded square-pixel 1080p24 LT/PCM16/24 research read-back; wider profiles and User acceptance remain pending".into();
    }
    for c in model.clips.values().filter(|_|native_av) {
        for group in &c.multichannel_groups {
            report.notes.push(format!("clip {}: {} group {:?} -> {}",py_str_repr(&c.name),group.format,group.channels,if group.format=="5.1" {"Premiere 5.1 L,R,C,LFE,Ls,Rs"}else{"8-channel Adaptive, Avid L,C,R,Lss,Rss,Lsr,Rsr,LFE order; no automatic 7.1 speaker bus"}));
        }
    }
    if !model.originals.is_empty() {report.notes.push("Original files: ordinary file-backed masters; explicit stream/channel mapping; no new media".into());report.unverified.push("Original-file masters and mixed timelines require native Premiere open/save/reopen, channel/sample checks and User acceptance".into());}
    let mut project = Project::new(ids)?;
    project.set_path(&out_path); // relative media paths are computed against the output location
    let Some(project_root) = project.root_item() else { return Err(other("project has no root item")) };
    let root = if let Some(layout) = layout { project.add_bin(project_root, &layout.root_name, ids)? } else { project_root };
    let mut table = MediaTable::new();
    if native_av {
        for c in model.clips.values().filter(|c|c.audio.len()>2) {
            if let Some(v)=&c.video { if media.iter().any(|m|m.source_path==v.path && m.target_path.to_ascii_lowercase().ends_with(".mov")) {table.mono_views.insert(v.path.clone());} }
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
    let mut writer = Writer {
        model,
        rewrapped: !media.is_empty(),
        native_av,
        project,
        media: table,
        ids,
        report,
        root,
        sequence_parent: layout.map(|_| root),
        out_path: out_path.clone(),
        assets: Vec::new(),
        asset_names: HashSet::new(),
        items: HashMap::new(),
        placement: HashMap::new(),
        clip_bins: HashMap::new(),
        multicam_items: Vec::new(),
        multicam_uses: model.multicam_uses.iter().map(|u| ((u.sequence,u.event),u)).collect(),
        video_dissolves,
        picture_effects: model.picture_effects.iter().map(|e| ((e.sequence, e.event), e.effect)).collect(),
    };
    // Borrow component names rather than cloning every complete prefix. This
    // index is linear in the validated references, including repeated paths.
    let mut folders: HashMap<(ItemId, &str), ItemId> = HashMap::new();
    let mut group_targets = Vec::new();
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
        if !model.multicams.is_empty() { group_targets.push(target); }
        for clip_id in &b.clips {
            if writer.items.contains_key(clip_id) {
                writer.report.notes.push(format!("clip {} listed in several bins; imported once into the first", py_str_repr(clip_id)));
                continue;
            }
            writer.import_clip(clip_id, Some(target))?;
        }
    }
    for seq in model.sequences {
        for e in &seq.events {
            if !writer.items.contains_key(&e.clip) {
                writer.import_clip(&e.clip, layout.map(|_| root))?;
                writer.report.notes.push(format!("clip {} used in a sequence but in no bin; imported at the root", py_str_repr(&e.clip)));
            }
        }
    }
    for group in model.multicams {
        for camera in &group.cameras {
            if !writer.items.contains_key(&camera.clip) {
                writer.import_clip(&camera.clip, layout.map(|_| root))?;
            }
        }
        let leaf = |c: &super::multicam::PreparedCamera| -> Result<_> {
            let item = writer.items[&c.clip].0.ok_or_else(|| other("multicam camera item not imported"))?;
            Ok((item, &model.clips[&c.clip]))
        };
        let leaves = [leaf(&group.cameras[0])?, leaf(&group.cameras[1])?];
        let item = super::multicam::build(&mut writer.project, group, leaves, group_targets[group.bin], writer.ids)?;
        writer.multicam_items.push(item);
    }
    if !model.multicams.is_empty() {
        writer.report.notes.push(format!("{} editable Premiere multicam source item(s); two cameras, independent fixed first-camera mono output lanes", model.multicams.len()));
        writer.report.unverified.push("Editable Group scope: two continuous same-rate DNx cameras with fixed two-mono PCM24 audio; User acceptance pending. Audio-angle switching, retimed or discontinuous alternatives, and wider audio are outside this profile.".into());
    }
    for (si, seq) in model.sequences.iter().enumerate() {
        writer.write_sequence(seq, si)?;
    }
    if !media.is_empty() {
        if native_av {
            super::rewrap::apply_native(&mut writer.project, media)?;
        } else {
            super::rewrap::apply(&mut writer.project, media)?;
        }
        writer.report.unverified.push("OP1a media binding: explicit flattened source channels; native read-back covers retained fixtures, not every profile or clip-construction mode".into());
    }
    let saved=writer.project.prepare_new_save(&out_path)?;
    let prepared=PreparedProject {saved:Some(saved),out_path,report:writer.report,assets:writer.assets,
        assets_attempted:false,assets_published:false,published:false,retained:Vec::new()};
    prepared.recheck_outputs()?;
    Ok(prepared)
}
