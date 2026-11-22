//! Preparation for the bounded, linked-media project qualification harness.
mod clip_envelope;
mod markers;
pub(crate) mod mixer;
pub(crate) mod timing;

use crate::{
    atom::{verify_atom_binding, AtomBindingSpec},
    audio::Envelope,
    clip_label::ClipLabel,
    dissolve,
    group::prepare_bound_group,
    media::{media_digest, probe_file, verify_group_binding, BindingSpec, StreamSelector},
    prepare::{
        check_binding, check_linked_placement, checked_name, frames_on, rate,
        utf8_path, PreparedMarker, MAX_EDITS_PER_LANE,
    },
    PreparedGroup, Result, VerifiedBinding, DAY,
};
use graph::{
    source::{SourceGraph, SourceMedia, SourceSequence},
    Rational, StreamKind, Value,
};
use serde::Deserialize;
use serde_json::Value as Json;
use timing::{PreparedTiming, Timing, TimingContext};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

type SidecarIndex<'a> = HashMap<(&'a str, &'a str), &'a Value>;

#[derive(Clone, Copy)]
pub(crate) enum SidecarLookup<'a> {
    Indexed(&'a SidecarIndex<'a>),
    Linear(&'a SourceGraph),
}

impl<'a> SidecarLookup<'a> {
    pub(crate) fn get(self, adapter: &str, object: &str) -> Option<&'a Value> {
        match self {
            Self::Indexed(index) => index.get(&(adapter, object)).copied(),
            Self::Linear(graph) => graph.sidecar(adapter, object),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    pub(crate) root_bin_id: Option<String>,
    pub(crate) timelines: Vec<TimelineSpec>,
    pub(crate) media: Vec<MediaSpec>,
    #[serde(default)]
    pub(crate) atom_media: Vec<AtomSpec>,
    #[serde(skip)]
    pub(crate) proxy_audio: HashMap<String, [avid_reader::source::SourceInput; 2]>,
}

pub(crate) fn avid_original_spec(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    inputs: &[avid_reader::source::SourceInput],
) -> Result<ProjectSpec> {
    let [bin] = graph.bins.as_slice() else {
        return Err("Avid UME probe expects one saved bin".into());
    };
    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for edit in &sequence.placements {
        if !seen.insert(edit.item.as_str()) {
            continue;
        }
        if media.len() == 256 {
            return Err("selected Avid UME items exceed the linked project media limit".into());
        }
        let item = graph
            .items
            .iter()
            .find(|item| item.id == edit.item)
            .ok_or("selected Avid UME item is absent")?;
        let sources = item.media.iter().map(|link| {
            graph.media.iter().find(|media| media.id == link.media).ok_or("selected Avid UME stream is absent")
        }).collect::<std::result::Result<Vec<_>, _>>()?;
        media.push(original_media_spec(item, &bin.id, &sources, inputs)?);
    }
    if media.is_empty() {
        return Err(format!(
            "{}: selected sequence has no resolved UME edits",
            sequence.id
        )
        .into());
    }
    Ok(ProjectSpec {
        root_bin_id: None,
        timelines: vec![TimelineSpec {
            id: sequence.id.clone(),
            bin_id: bin.id.clone(),
            canvas: (1920, 1080),
            group: None,
        }],
        media,
        atom_media: vec![],
        proxy_audio: HashMap::new(),
    })
}

pub(crate) fn original_media_spec(
    item: &graph::source::SourceItem,
    bin_id: &str,
    sources: &[&SourceMedia],
    inputs: &[avid_reader::source::SourceInput],
) -> Result<MediaSpec> {
    let mut binding: Option<MediaSpec> = None;
    for source in sources {
        let [locator] = source.locations.as_slice() else {
            return Err("selected stream requires one resolved original locator".into());
        };
        let original = source.original.as_ref().ok_or_else(|| {
            format!("{}: selected source lacks a qualified original", item.id)
        })?;
        let path = Path::new(locator);
        let input = inputs.iter().find(|input| input.path == path)
            .ok_or("original has no verified content receipt")?;
        let binding = binding.get_or_insert_with(|| MediaSpec {
            item_id: item.id.clone(),
            bin_id: bin_id.into(),
            path: path.to_owned(),
            sha256: input.revision.clone(),
            streams: vec![],
        });
        if binding.path != path {
            return Err("selected streams resolve to different originals".into());
        }
        binding.streams.push(StreamSelector {
            media_id: source.id.clone(),
            index: usize::try_from(original.stream_index)?,
            channel: original.channel_index.map(usize::try_from).transpose()?,
        });
    }
    binding.ok_or_else(|| "selected item has no resolved original streams".into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AtomSpec {
    pub(crate) bin_id: String,
    pub(crate) binding: AtomBindingSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimelineSpec {
    pub(crate) id: String,
    pub(crate) bin_id: String,
    pub(crate) canvas: (u16, u16),
    #[serde(default)]
    pub(crate) group: Option<GroupSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupSpec {
    pub(crate) bin_id: String,
    pub(crate) cameras: [String; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MediaSpec {
    pub(crate) item_id: String,
    pub(crate) bin_id: String,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) streams: Vec<StreamSelector>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MovieVideo {
    ProResHq,
    Avc,
    HevcMain10,
    Imported,
}

impl MovieVideo {
    pub(crate) fn cache_codec(self) -> Result<&'static str> {
        Ok(match self {
            Self::ProResHq => "apch",
            Self::Avc => "avc1",
            Self::HevcMain10 => "hvc1",
            Self::Imported => return Err("native movie records are imported instead of synthesized".into()),
        })
    }

    fn matches_source_codec(self, codec: &str) -> bool {
        self.cache_codec().is_ok_and(|saved| codec == saved)
            || codec == match self {
                Self::ProResHq => "prores",
                Self::Avc => "h264",
                Self::HevcMain10 => "hevc",
                Self::Imported => return false,
            }
    }
}

#[derive(Clone)]
pub(crate) enum Profile {
    Mov {
        rate: Rational,
        frames: i64,
        raster: (u16, u16),
        tagged_rec709: bool,
        quicktime_timecode: Option<i64>,
        avid_source_timecode: Option<i64>,
        video: MovieVideo,
        aac: bool,
        pcm: Option<i64>,
    },
    Wav {
        samples: i64,
        bits: i64,
        channels: i64,
    },
    MonoAtom {
        frames: i64,
        bits: i64,
        start: i64,
        package: [u8; 32],
        material: [u8; 32],
    },
    BoundMxf(Arc<VerifiedBinding>),
}

impl Profile {
    pub(crate) fn timecoded_camera_pcm(&self) -> bool {
        matches!(self, Self::Mov { video: MovieVideo::Avc, rate, raster: (4096, 2160),
            pcm: Some(16), quicktime_timecode: Some(_), .. } if *rate == Rational::integer(24))
    }

    /// Source origin written to Resolve. Untagged media and ordinary zero-start MOVs use
    /// the existing default. Explicit Avid clock receipts can also establish frame zero.
    pub(crate) fn movie_timecode(&self) -> Option<i64> {
        match self {
            Self::Mov { quicktime_timecode, avid_source_timecode, .. } =>
                avid_source_timecode.or(quicktime_timecode.filter(|&start| start != 0)),
            _ => None,
        }
    }

    pub(crate) fn pool_marker_grid(&self, project_rate: Rational) -> Result<Option<(Rational, i64)>> {
        match self {
            Self::Mov {
                rate, frames, raster: (1920, 1080), video: MovieVideo::ProResHq,
                aac: false, pcm: None, ..
            } if crate::source::supported_marker_rate(*rate) => {
                Ok(Some((*rate, *frames)))
            }
            // The WAV writer saves the project rate as the audio item's import clock.
            Self::Wav { samples, channels: 1 | 2, bits: 16 | 24 }
                if crate::source::supported_marker_rate(project_rate) =>
            {
                Ok(Some((project_rate, samples / samples_per_frame(project_rate)?)))
            }
            _ => Ok(None),
        }
    }
}

pub(crate) struct Media {
    pub(crate) name: String,
    pub(crate) bin: usize,
    pub(crate) path: PathBuf,
    pub(crate) profile: Profile,
    pub(crate) modified_micros: u64,
    pub(crate) calendar_seconds: Option<u64>,
    pub(crate) identity: MediaIdentity,
    pub(crate) proxy: Option<avid_adapter::proxy::Attachment>,
    pub(crate) native: Option<Arc<crate::native_import::Movie>>,
}

#[derive(Clone)]
pub(crate) enum MediaIdentity {
    Digest([u8; 32]),
    File(crate::media::FileStamp),
}

pub(crate) type FileProbes = HashMap<PathBuf, (Media, Rational, i64)>;

pub(crate) enum MediaPreparation<'a> {
    Automatic,
    StandaloneOnly,
    VerifiedPremiere(&'a FileProbes),
}

impl Media {
    fn for_item(&self, name: String, bin: usize) -> Self {
        Self {
            name,
            bin,
            path: self.path.clone(),
            profile: self.profile.clone(),
            modified_micros: self.modified_micros,
            calendar_seconds: self.calendar_seconds,
            identity: self.identity.clone(),
            proxy: None,
            native: self.native.clone(),
        }
    }

    pub(crate) fn digest_hex(&self) -> Result<String> {
        match &self.identity {
            MediaIdentity::Digest(digest) => Ok(hex::encode(digest)),
            MediaIdentity::File(_) => Err("linked media has no checksum binding".into()),
        }
    }

    pub(crate) fn recheck(&self) -> Result<()> {
        if let Profile::BoundMxf(binding) = &self.profile {
            return binding.recheck();
        }
        let unchanged = match &self.identity {
            MediaIdentity::Digest(digest) => media_digest(&self.path)? == *digest,
            MediaIdentity::File(stamp) => crate::media::stamp(fs::metadata(&self.path)?)? == *stamp,
        };
        if !unchanged {
            return Err(format!("linked media changed: {}", self.path.display()).into());
        }
        Ok(())
    }
}

pub(crate) struct Bin {
    pub(crate) parent: Option<usize>,
    pub(crate) name: String,
}

pub(crate) struct Edit {
    pub(crate) media: usize,
    pub(crate) channel: Option<usize>,
    pub(crate) start: i64,
    pub(crate) source_in: i64,
    pub(crate) length: i64,
    pub(crate) video: Option<crate::video::PreparedVideo>,
    pub(crate) input_sizing: Option<crate::input_scaling::Sizing>,
    pub(crate) envelope: Option<Envelope>,
    pub(crate) label: Option<ClipLabel>,
    pub(crate) markers: Vec<PreparedMarker>,
    pub(crate) timing: Timing,
    pub(crate) grade: Option<(crate::grade::Operation, String)>,
    pub(crate) fusion: Option<crate::clip_fusion::Operation>,
}

pub(crate) struct Timeline {
    pub(crate) bin: usize,
    pub(crate) name: String,
    pub(crate) origin: i64,
    pub(crate) length: i64,
    // V1, A1, A2, V2, A3. The original three slots retain their archive ordering.
    pub(crate) lanes: [Vec<Edit>; 5],
    pub(crate) stereo: bool,
    pub(crate) markers: Vec<PreparedMarker>,
    pub(crate) marks: Option<crate::timeline_marks::TimelineMarks>,
    pub(crate) track_state: Option<crate::track_state::TrackState>,
    pub(crate) group: Option<GroupTimeline>,
    pub(crate) mixer: Option<mixer::DirectMixer>,
    pub(crate) faders: mixer::StaticFaders,
    pub(crate) dissolves: Vec<dissolve::Dissolve>,
    pub(crate) captions: Vec<crate::captions::Caption>,
    pub(crate) solid: Option<crate::generator::Solid>,
    pub(crate) compound: bool,
    pub(crate) nests: Vec<crate::nests::Use>,
}

pub(crate) struct GroupTimeline {
    pub(crate) bin: usize,
    pub(crate) cameras: [usize; 2],
    pub(crate) prepared: PreparedGroup,
}

pub struct PreparedProject {
    pub(crate) name: String,
    pub(crate) rate: Rational,
    pub(crate) canvas: (u16, u16),
    pub(crate) bins: Vec<Bin>,
    pub(crate) media: Vec<Media>,
    pub(crate) pool_markers: HashMap<usize, Vec<PreparedMarker>>,
    pub(crate) timelines: Vec<Timeline>,
    pub(crate) active_timeline_index: usize,
    omitted_active_timeline: Option<String>,
    pub(crate) omitted_markers: Vec<String>,
    pub(crate) destination_losses: Vec<graph::Loss>,
    recovered_premiere_marks: HashSet<String>,
    recovered_premiere_adjusts: HashSet<String>,
}

impl PreparedProject {
    pub fn media_counts(&self) -> (usize, usize) {
        let direct: HashSet<_> = self.media.iter().filter(|media| media.native.is_none())
            .flat_map(|media| match &media.profile {
                Profile::BoundMxf(binding) if binding.atom.is_some() => {
                    binding.atom.iter().flat_map(|atom| atom.files.iter().map(|file| &file.path)).collect::<Vec<_>>()
                }
                _ => vec![&media.path],
            }).collect();
        let assisted: HashSet<_> = self.media.iter().filter(|media| media.native.is_some())
            .map(|media| &media.path).collect();
        (direct.len(), assisted.len())
    }

    pub(crate) fn recovers_premiere_mark(&self, loss: &graph::Loss) -> bool {
        loss.aspect == crate::premiere::TIMELINE_MARK_LOSS
            && self.recovered_premiere_marks.contains(&loss.object)
    }

    pub(crate) fn recovers_premiere_adjust(&self, loss: &graph::Loss) -> bool {
        loss.aspect == "premiere.appearance"
            && loss.reason == crate::premiere::color_mode::ADJUST_LOSS
            && self.recovered_premiere_adjusts.contains(&loss.object)
    }

    pub(crate) fn writes_caption(&self, id: &str) -> bool {
        self.timelines.iter().flat_map(|timeline| &timeline.captions).any(|cue| cue.id == id)
    }

    pub fn omitted_active_timeline(&self) -> Option<&str> {
        self.omitted_active_timeline.as_deref()
    }

    pub fn omitted_markers(&self) -> &[String] {
        &self.omitted_markers
    }

    pub fn marker_ownership_losses(&self) -> impl Iterator<Item = &graph::Loss> {
        self.destination_losses.iter().filter(|loss| loss.aspect == markers::LOSS_ASPECT)
    }

    pub fn transition_approximation_losses(&self) -> impl Iterator<Item = &graph::Loss> {
        self.destination_losses.iter().filter(|loss| loss.aspect == "transition_approximation")
    }

    pub fn color_adjust_losses(&self) -> impl Iterator<Item = &graph::Loss> {
        self.destination_losses.iter().filter(|loss| loss.aspect == crate::clip_fusion::APPROXIMATION)
    }

    pub fn empty_nest_audio_losses(&self) -> impl Iterator<Item = &graph::Loss> {
        self.destination_losses.iter().filter(|loss| loss.aspect == crate::nests::EMPTY_NEST_AUDIO_LOSS)
    }

    pub fn stereo_nest_edge_losses(&self) -> impl Iterator<Item = &graph::Loss> {
        self.destination_losses.iter().filter(|loss| loss.aspect == crate::nests::STEREO_NEST_EDGE_LOSS)
    }
}

fn integer(s: &Json, key: &str) -> Result<i64> {
    s[key]
        .as_i64()
        .ok_or_else(|| format!("missing physical media {key}").into())
}

fn ratio(s: &Json) -> Result<Rational> {
    let (n, d) = s
        .as_str()
        .and_then(|v| v.split_once('/'))
        .ok_or("missing physical ratio")?;
    Rational::new(n.parse()?, d.parse()?).ok_or_else(|| "invalid physical ratio".into())
}

pub(crate) fn extent(count: i64, time_base: Rational) -> Result<Rational> {
    anynle_media_verification::stream_duration(count, time_base)
        .ok_or_else(|| "physical media duration overflow".into())
}

pub(crate) fn non_drop_24(frames: i64) -> String {
    format!("{:02}:{:02}:{:02}:{:02}", frames / 86400, frames / 1440 % 60,
        frames / 24 % 60, frames % 24)
}

#[derive(Debug)]
pub(crate) struct UnavailableCache(pub(crate) &'static str);

impl std::fmt::Display for UnavailableCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for UnavailableCache {}

pub(crate) fn probe(
    spec: &MediaSpec,
    tool: &Path,
    name: String,
    bin: usize,
    canvas: (u16, u16),
) -> Result<(Media, Rational, i64)> {
    probe_linked(spec, tool, name, bin, None, None, Some(canvas), true)
}

pub(crate) fn review_original(
    path: &Path,
    ffprobe: &Path,
    canvas: Option<(i64, i64)>,
) -> Result<bool> {
    let spec = MediaSpec {
        item_id: String::new(),
        bin_id: String::new(),
        path: path.to_owned(),
        sha256: String::new(),
        streams: vec![],
    };
    let eligible = canvas == Some((1920, 1080)) && crate::native_import::eligible(path);
    match probe_original(&spec, ffprobe, String::new(), 0, None, None, ProbeMode::Review) {
        Ok((media, _, _)) => {
            Ok(eligible && matches!(media.profile, Profile::Mov { raster: (width, height), .. }
                if canvas != Some((i64::from(width), i64::from(height)))))
        }
        Err(error) if eligible && error.is::<UnavailableCache>() => Ok(true),
        Err(error) => Err(error),
    }
}

fn probe_linked(
    spec: &MediaSpec,
    tool: &Path,
    name: String,
    bin: usize,
    avid_start: Option<i64>,
    proxy: Option<avid_adapter::proxy::Attachment>,
    canvas: Option<(u16, u16)>,
    allow_native_import: bool,
) -> Result<(Media, Rational, i64)> {
    let eligible = proxy.is_none() && canvas == Some((1920, 1080))
        && crate::native_import::eligible(&spec.path);
    let standalone = || {
        let mode = if eligible && spec.sha256.is_empty() {
            ProbeMode::Review
        } else {
            ProbeMode::Linked
        };
        let result = probe_original(spec, tool, name.clone(), bin, avid_start, proxy, mode)?;
        if eligible && matches!(&result.0.profile, Profile::Mov { raster, .. } if Some(*raster) != canvas) {
            return Err(UnavailableCache("original raster differs from the Resolve canvas").into());
        }
        Ok(result)
    };
    let assisted = || probe_native(spec, tool, name.clone(), bin, avid_start);
    if !allow_native_import || !eligible || !crate::native_import::available() {
        return standalone();
    }
    if std::env::var_os("ANYNLE_RESOLVE_IMPORT_ORDER").as_deref() == Some("api-first".as_ref()) {
        return match assisted() {
            Ok(result) => Ok(result),
            Err(error) if error.is::<UnavailableCache>() => standalone()
                .map_err(|second| format!("{second}; Resolve import: {error}").into()),
            Err(error) => Err(error),
        };
    }
    match standalone() {
        Err(error) if error.is::<UnavailableCache>() => assisted()
            .map_err(|native| format!("{error}; Resolve import fallback: {native}").into()),
        result => result,
    }
}

fn probe_native(
    spec: &MediaSpec,
    tool: &Path,
    name: String,
    bin: usize,
    avid_start: Option<i64>,
) -> Result<(Media, Rational, i64)> {
    if !spec.path.is_absolute() {
        return Err("native import needs an absolute original locator".into());
    }
    let path = fs::canonicalize(&spec.path)?;
    utf8_path(&path)?;
    let metadata = fs::metadata(&path)?;
    let modified_micros = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_micros().try_into()?;
    let stamp = crate::media::stamp(metadata)?;
    let identity = if spec.sha256.is_empty() {
        MediaIdentity::File(stamp.clone())
    } else {
        let digest = media_digest(&path)?;
        if hex::encode(digest) != spec.sha256 {
            return Err("native original does not match its verified digest".into());
        }
        MediaIdentity::Digest(digest)
    };
    let native = crate::native_import::capture(&path, tool)?;
    let start = native.start()?;
    if avid_start.is_some_and(|expected| expected != start) {
        return Err("native import source timecode disagrees with the Avid original".into());
    }
    if crate::media::stamp(fs::metadata(&path)?)? != stamp || fs::canonicalize(&spec.path)? != path {
        return Err("native original changed during import".into());
    }
    let (tb, duration) = native.picture_clock();
    let profile = Profile::Mov {
        rate: native.rate(),
        frames: native.frames()?,
        raster: native.raster(),
        tagged_rec709: false,
        quicktime_timecode: Some(start),
        avid_source_timecode: avid_start,
        video: MovieVideo::Imported,
        aac: native.aac(),
        pcm: native.bits(),
    };
    Ok((Media {
        name, bin, path, profile, modified_micros, calendar_seconds: None,
        identity, proxy: None, native: Some(Arc::new(native)),
    }, tb, duration))
}

pub(crate) fn probe_resolve_original(spec: &MediaSpec, tool: &Path, camera: bool) -> Result<(Media, Rational, i64)> {
    let mode = if camera { ProbeMode::Camera } else { ProbeMode::Resolve };
    probe_original(spec, tool, String::new(), 0, None, None, mode)
}

pub(crate) fn probe_resolve_original_by_path(spec: &MediaSpec, tool: &Path) -> Result<(Media, Rational, i64)> {
    probe_original(spec, tool, String::new(), 0, None, None, ProbeMode::Review)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeMode { Linked, Resolve, Camera, Review }

fn probe_original(
    spec: &MediaSpec,
    tool: &Path,
    name: String,
    bin: usize,
    avid_source_timecode: Option<i64>,
    proxy: Option<avid_adapter::proxy::Attachment>,
    mode: ProbeMode,
) -> Result<(Media, Rational, i64)> {
    if !spec.path.is_absolute() {
        return Err("linked media binding needs an absolute path".into());
    }
    let path = fs::canonicalize(&spec.path)?;
    utf8_path(&path)?;
    let format = match path.extension().and_then(|x| x.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("mov") || ext.eq_ignore_ascii_case("mp4") => "mov",
        Some(ext) if ext.eq_ignore_ascii_case("wav") => "wav",
        _ => return Err("expected an original MOV, MP4 or WAV file".into()),
    };
    let destination_link = matches!(mode, ProbeMode::Resolve | ProbeMode::Camera);
    let identity = if destination_link || mode == ProbeMode::Review {
        MediaIdentity::File(crate::media::stamp(fs::metadata(&path)?)?)
    } else {
        let digest: [u8; 32] = hex::decode(&spec.sha256)?
            .try_into()
            .map_err(|_| "invalid linked media digest")?;
        if media_digest(&path)? != digest {
            return Err("linked media does not match its verified digest".into());
        }
        MediaIdentity::Digest(digest)
    };
    // Destination links check every picture packet below. Read the stream
    // headers first so unsupported originals can fail without an essence scan.
    let probe = if destination_link {
        crate::media::probe_json(tool, &path, &["-f", format, "-show_streams", "-show_format"])?
    } else {
        probe_file(tool, &path, format)?
    };
    let streams = probe["streams"]
        .as_array()
        .ok_or("missing physical media streams")?;
    let stream = streams.first().ok_or("missing physical essence stream")?;
    if stream["index"] != 0 {
        return Err("expected physical stream zero".into());
    }
    let duration_ts = integer(stream, "duration_ts")?;
    let time_base = ratio(&stream["time_base"])?;
    if duration_ts <= 0 || time_base <= Rational::ZERO {
        return Err("invalid physical media extent".into());
    }
    let profile = if format == "mov" {
        let rate = ratio(&stream["r_frame_rate"])?;
        let duration = extent(duration_ts, time_base)?;
        let frames = frames_on(duration, rate)?;
        let video = match stream["codec_name"].as_str() {
            Some("h264") => MovieVideo::Avc,
            Some("hevc") => MovieVideo::HevcMain10,
            Some("prores") => MovieVideo::ProResHq,
            _ => return Err(UnavailableCache("unqualified linked AVC, HEVC or ProRes HQ picture profile").into()),
        };
        let aac = video != MovieVideo::ProResHq && streams.len() == 2 && streams[1]["codec_name"] == "aac";
        let suffix = path.extension().and_then(|v| v.to_str()).unwrap_or_default();
        let camera = mode == ProbeMode::Camera;
        if camera && !(video == MovieVideo::Avc && rate == Rational::integer(24)
            && suffix.eq_ignore_ascii_case("mov") && streams.len() == 3
            && streams[1]["codec_name"] == "pcm_s16be" && streams[1]["codec_tag_string"] == "twos")
        {
            return Err("unqualified timecoded camera AVC and PCM stream layout".into());
        }
        let direct_pcm = if camera {
            Some(16)
        } else if video == MovieVideo::Avc && streams.len() == 2 && embedded_movie_pcm_rate(rate)
            && avid_source_timecode.is_none() && proxy.is_none()
        {
            match (
                suffix.to_ascii_lowercase().as_str(),
                streams[1]["codec_name"].as_str(),
                streams[1]["codec_tag_string"].as_str(),
            ) {
                ("mov", Some("pcm_s16le"), Some("sowt"))
                | ("mp4", Some("pcm_s16le"), Some("ipcm")) => Some(16),
                ("mov", Some("pcm_s24le"), Some("in24"))
                | ("mp4", Some("pcm_s24le"), Some("ipcm")) => Some(24),
                _ => None,
            }
        } else {
            None
        };
        let pcm = direct_pcm.or_else(|| proxy.as_ref().and_then(|p| p.bits));
        let embedded_start = if camera {
            Some(crate::media::tc_frames(stream["tags"]["timecode"].as_str()
                .ok_or("camera picture has no embedded source timecode")?)?)
        } else { match (video, streams.as_slice()) {
            (MovieVideo::ProResHq, [picture, _])
                if supported_rate(rate) && avid_source_timecode.is_none() =>
            {
                picture["tags"]["timecode"].as_str().map(crate::media::tc_frames).transpose()?
            }
            _ => None,
        }};
        let start = avid_source_timecode.or(embedded_start).unwrap_or(0);
        if !(0..24 * 60 * 60 * 24).contains(&start) {
            return Err("camera original timecode outside a 24-hour day".into());
        }
        let tc_text = non_drop_24(start);
        let matching_tc = |s: &Json| s["tags"]["timecode"].as_str() == Some(tc_text.as_str());
        let timing_streams: Vec<_> = if pcm.is_some() && streams.len() == 3 {
            vec![&streams[0], &streams[2]]
        } else { streams.iter().collect() };
        let quicktime_timecode = match timing_streams.as_slice() {
            [_, tc] if camera && tc["index"] == 2 && tc["codec_type"] == "data"
                && tc["codec_tag_string"] == "tmcd" && tc["start_pts"] == 0
                && tc["duration_ts"] == duration_ts
                && ratio(&tc["time_base"]).ok() == Some(time_base)
                && ratio(&tc["avg_frame_rate"]).ok() == Some(rate)
                && tc["nb_frames"].as_str().and_then(|n| n.parse().ok()) == Some(frames)
                && matching_tc(stream) && matching_tc(&streams[1]) && matching_tc(tc)
                && (probe["format"]["tags"]["timecode"].is_null() || matching_tc(&probe["format"])) =>
            {
                Some(start)
            }
            [_] | [_, _]
                if (streams.len() == 1 || aac || direct_pcm.is_some())
                    && avid_source_timecode.is_none()
                    && stream["tags"]["timecode"].is_null()
                    && probe["format"]["tags"]["timecode"].is_null() =>
            {
                None
            }
            [_, tc]
                if video == MovieVideo::ProResHq
                    && supported_rate(rate)
                    && tc["index"] == if pcm.is_some() {2} else {1}
                    && tc["codec_type"] == "data"
                    && tc["codec_tag_string"] == "tmcd"
                    && tc["nb_frames"] == "1"
                    && tc["start_pts"] == 0
                    && tc["duration_ts"] == duration_ts
                    && ratio(&tc["avg_frame_rate"]).ok() == Some(rate)
                    && ratio(&tc["time_base"]).ok() == Some(time_base)
                    && matching_tc(stream)
                    && matching_tc(tc)
                    && (probe["format"]["tags"]["timecode"].is_null()
                        || matching_tc(&probe["format"])) =>
            {
                Some(start)
            }
            _ => return Err(UnavailableCache("unqualified MOV auxiliary track or file timecode").into()),
        };
        let tagged_rec709 =
            stream["color_primaries"] == "bt709" && stream["color_transfer"] == "bt709";
        let unspecified_color = ["color_primaries", "color_transfer"]
            .iter()
            .all(|key| stream[*key].is_null() || stream[*key] == "unknown");
        let brand = &probe["format"]["tags"]["major_brand"];
        let is_quicktime = suffix.eq_ignore_ascii_case("mov") && brand == "qt  ";
        let is_mp4 = suffix.eq_ignore_ascii_case("mp4")
            && matches!(brand.as_str(), Some("isom" | "mp41" | "mp42"));
        let absent = |key: &str| stream[key].is_null() || stream[key] == "unknown";
        // Resolve fills the same AVC media cache on direct import when the
        // matrix and range are known but the primaries and transfer are unset.
        let avc_default_color = unspecified_color
            && ((absent("color_space") && absent("color_range"))
                || (stream["color_space"] == "bt709" && stream["color_range"] == "tv"));
        let raster = (
            u16::try_from(integer(stream, "width")?)?,
            u16::try_from(integer(stream, "height")?)?,
        );
        let avc_geometry = match raster {
            (1920, 1080) => stream["level"] == 40,
            (3840, 2160) => is_mp4 && stream["level"] == 51 && uhd_rate(rate) && unspecified_color,
            _ => false,
        };
        let video_profile = match video {
            MovieVideo::Avc if camera => is_quicktime && raster == (4096, 2160)
                && stream["level"] == 51 && stream["codec_tag_string"] == "avc1"
                && match (stream["profile"].as_str(), stream["pix_fmt"].as_str()) {
                    (Some("High"), Some("yuv420p")) | (Some("High 4:2:2"), Some("yuv422p10le")) => true,
                    _ => false,
                }
                && tagged_rec709 && stream["color_space"] == "bt709"
                && matches!(stream["color_range"].as_str(), Some("tv" | "pc"))
                && stream["side_data_list"].is_null() && stream["tags"]["rotate"].is_null(),
            MovieVideo::Avc => (is_quicktime || is_mp4)
                && avc_rate(rate)
                && avc_geometry
                && stream["profile"] == "High"
                && stream["codec_tag_string"] == "avc1"
                && stream["pix_fmt"] == "yuv420p"
                && (avc_default_color
                    || (tagged_rec709
                        && stream["color_space"] == "bt709"
                        && stream["color_range"] == "tv"))
                && stream["side_data_list"].is_null()
                && stream["tags"]["rotate"].is_null(),
            MovieVideo::HevcMain10 => is_mp4
                && rate == Rational::integer(24)
                && raster == (1920, 1080)
                && aac
                && stream["profile"] == "Main 10"
                && stream["codec_tag_string"] == "hvc1"
                && stream["level"] == 120
                && stream["pix_fmt"] == "yuv420p10le"
                && unspecified_color
                && stream["color_space"] == "bt709"
                && stream["color_range"] == "tv"
                && stream["side_data_list"].is_null()
                && stream["tags"]["rotate"].is_null(),
            MovieVideo::ProResHq => is_quicktime
                && raster == (1920, 1080)
                && stream["profile"] == "HQ"
                && stream["codec_tag_string"] == "apch"
                && stream["pix_fmt"] == "yuv422p10le"
                && stream["color_range"] == "tv"
                && stream["color_space"] == "bt709"
                && (tagged_rec709 || unspecified_color),
            MovieVideo::Imported => false,
        };
        let supported_clock = supported_rate(rate)
            || avc_rate(rate) && (video == MovieVideo::Avc
                || video == MovieVideo::ProResHq && streams.len() == 1);
        if !supported_clock
            || frames <= 0
            || frames > i64::from(i32::MAX)
            || stream["codec_type"] != "video"
            || !video_profile
            || stream["field_order"] != "progressive"
            || stream["sample_aspect_ratio"] != "1:1"
            || stream["start_pts"] != 0
            || !destination_link && stream["nb_read_packets"]
                .as_str()
                .and_then(|n| n.parse().ok())
                != Some(frames)
            || ratio(&stream["avg_frame_rate"]).ok() != Some(rate)
        {
            let diagnostic = if video == MovieVideo::HevcMain10 {
                "unqualified linked HEVC Main 10 picture profile"
            } else {
                "unqualified linked AVC or ProRes HQ picture profile"
            };
            if !supported_clock || !video_profile {
                return Err(UnavailableCache(diagnostic).into());
            }
            return Err(diagnostic.into());
        }
        if aac {
            let samples = movie_audio_sample_extent(frames, rate)?;
            let audio = &streams[1];
            if audio["index"] != 1
                || audio["codec_type"] != "audio"
                || audio["profile"] != "LC"
                || audio["codec_tag_string"] != "mp4a"
                || audio["channels"] != 2
                || audio["channel_layout"] != "stereo"
                || audio["sample_rate"] != "48000"
                || audio["start_pts"] != 0
                || ratio(&audio["time_base"])? != Rational::new(1, 48000).unwrap()
            {
                return Err(if video == MovieVideo::Avc {
                    "unqualified AVC movie AAC layout or playable sample extent"
                } else {
                    "unqualified HEVC movie AAC layout or playable sample extent"
                }.into());
            }
            if integer(audio, "duration_ts")? != samples {
                return Err(UnavailableCache("standalone movie cache requires audio ending at the picture extent").into());
            }
            crate::media::verify_aac_packet_grid(tool, &path, samples)?;
        }
        if let Some(p) = &proxy {
            if video != MovieVideo::ProResHq || quicktime_timecode.is_none() || p.path == path || p.frames != frames || p.start != start || p.rate != rate {
                return Err("Resolve proxy disagrees with its original clock, raster or extent".into());
            }
        }
        if let Some(bits) = pcm {
            let audio = &streams[1];
            let expected = if camera {"pcm_s16be"} else if bits == 16 {"pcm_s16le"} else {"pcm_s24le"};
            let samples = if direct_pcm.is_some() {
                movie_audio_sample_extent(frames, rate)?
            } else {
                frames * samples_per_frame(rate)?
            };
            if audio["index"] != 1 || audio["codec_type"] != "audio" || audio["codec_name"] != expected
                || audio["channels"] != 2 || audio["sample_rate"] != "48000" || audio["start_pts"] != 0
                || audio["bits_per_sample"] != bits
                || ratio(&audio["time_base"])? != Rational::new(1,48000).unwrap() {
                return Err("unqualified movie stereo PCM clock or layout".into());
            }
            if integer(audio,"duration_ts")? != samples {
                if !camera && proxy.is_none() {
                    return Err(UnavailableCache("standalone movie cache requires audio ending at the picture extent").into());
                }
                return Err("unqualified movie stereo PCM clock or layout".into());
            }
            if direct_pcm.is_some() {
                if audio["channel_layout"] != "stereo" && !(camera && audio["channel_layout"].is_null()) {
                    return Err("movie PCM does not have a saved stereo channel layout".into());
                }
                crate::media::verify_pcm_packet_grid(tool, &path, "mov", 1, samples, bits / 8 * 2)?;
            }
        }
        if duration_ts % frames != 0 {
            return Err("MOV timescale cannot represent one constant frame".into());
        }
        crate::media::verify_mov_packet_grid(
            tool, &path, frames, duration_ts / frames, video != MovieVideo::ProResHq,
        )?;
        Profile::Mov {
            rate,
            frames,
            raster,
            tagged_rec709,
            quicktime_timecode,
            avid_source_timecode,
            video,
            aac,
            pcm,
        }
    } else {
        if avid_source_timecode.is_some() {
            return Err("Avid camera-original timecode requires a MOV binding".into());
        }
        let channels = integer(stream, "channels")?;
        let bits = match stream["codec_name"].as_str() {
            Some("pcm_s16le") => 16,
            Some("pcm_s24le") => 24,
            _ => return Err("unqualified linked WAV PCM depth".into()),
        };
        if streams.len() != 1
            || !matches!(channels, 1 | 2)
            || stream["codec_type"] != "audio"
            || stream["sample_rate"] != "48000"
            || stream["bits_per_sample"] != bits
            || time_base != Rational::new(1, 48000).unwrap()
            || stream["start_pts"].as_i64().unwrap_or(0) != 0
            || !probe["format"]["tags"]["time_reference"].is_null()
                && probe["format"]["tags"]["time_reference"] != "0"
        {
            return Err("unqualified linked WAV layout, origin or sample clock".into());
        }
        Profile::Wav {
            samples: duration_ts,
            bits,
            channels,
        }
    };
    let timestamp = fs::metadata(&path)?;
    let modified = match &profile {
        Profile::Mov { pcm: Some(_), .. } => timestamp.created().or_else(|_| timestamp.modified())?,
        _ => timestamp.modified()?,
    };
    let modified_micros = modified.duration_since(UNIX_EPOCH)?
        .as_micros()
        .try_into()?;
    let calendar_seconds = match &profile {
        Profile::Mov { video: MovieVideo::Avc, pcm: Some(_), .. }
        | Profile::Mov { video: MovieVideo::HevcMain10, .. } =>
            Some(timestamp.modified()?.duration_since(UNIX_EPOCH)?.as_secs()),
        _ => None,
    };
    let binding = Media {
        name,
        bin,
        path,
        profile,
        modified_micros,
        calendar_seconds,
        identity,
        proxy,
        native: None,
    };
    binding.recheck()?;
    Ok((binding, time_base, duration_ts))
}

fn supported_rate(rate: Rational) -> bool {
    matches!((rate.num(), rate.den()), (24, 1) | (24000, 1001))
}

pub(crate) fn avc_rate(rate: Rational) -> bool {
    matches!((rate.num(), rate.den()), (24 | 25, 1) | (24000 | 30000, 1001))
}

pub(crate) fn is_2997(rate: Rational) -> bool {
    (rate.num(), rate.den()) == (30000, 1001)
}

pub(crate) fn uhd_rate(rate: Rational) -> bool {
    matches!((rate.num(), rate.den()), (25, 1) | (24000 | 30000, 1001))
}

pub(crate) fn embedded_movie_pcm_rate(rate: Rational) -> bool {
    matches!((rate.num(), rate.den()), (25, 1) | (30000, 1001))
}

// The Avid adapter already checked the physical PCM layout, clock and decoded
// channel content. Bind exactly those pinned files to their saved graph package.
fn prepare_proxy_audio(
    input: &avid_reader::source::SourceInput,
    expected_package: &str,
    bin: usize,
    frames: i64,
    bits: i64,
    start: i64,
) -> Result<(Media, u64)> {
    let path = fs::canonicalize(&input.path)?;
    let name = checked_name(path.file_name().and_then(|name| name.to_str())
        .ok_or("invalid Avid audio sibling filename")?)?;
    if path.extension().and_then(|ext| ext.to_str()) != Some("mxf") {
        return Err("expected a verified Avid mono MXF".into());
    }
    let id = avid_reader::mxf::read_identity(&path)?;
    let [package] = id.file_packages.as_slice() else {
        return Err("expected one MXF file package".into());
    };
    let [material] = id.material_packages.as_slice() else {
        return Err("expected one MXF material package".into());
    };
    if package != expected_package || package == material {
        return Err("verified MXF disagrees with the saved audio sibling package".into());
    }
    let umid = |value: &str| -> Result<[u8; 32]> {
        let bytes = hex::decode(value)?.try_into().map_err(|_| "invalid MXF package")?;
        if bytes == [0; 32] {
            return Err("empty MXF package identity".into());
        }
        Ok(bytes)
    };
    let metadata = fs::metadata(&path)?;
    let modified_micros = metadata.created().or_else(|_| metadata.modified())?
        .duration_since(UNIX_EPOCH)?.as_micros().try_into()?;
    let media = Media {
        name,
        bin,
        path,
        profile: Profile::MonoAtom {
            frames,
            bits,
            start,
            package: umid(package)?,
            material: umid(material)?,
        },
        modified_micros,
        calendar_seconds: None,
        identity: MediaIdentity::Digest(hex::decode(&input.revision)?.try_into().map_err(|_| "invalid MXF digest")?),
        proxy: None,
        native: None,
    };
    media.recheck()?;
    Ok((media, metadata.len()))
}

pub(crate) fn check_stream(
    source: &SourceMedia,
    selector: &StreamSelector,
    media: &Media,
    time_base: Rational,
    duration_ts: i64,
) -> Result<()> {
    if let Some(native) = &media.native {
        return native.check_stream(source, selector);
    }
    let (index, time_base, duration_ts) = match (&media.profile, source.kind) {
        (
            Profile::Mov { aac, pcm, rate, frames, video, .. },
            StreamKind::Audio,
        ) if *aac || pcm.is_some() => (
            1,
            Rational::new(1, 48000).unwrap(),
            if *video != MovieVideo::ProResHq {
                movie_audio_sample_extent(*frames, *rate)?
            } else {
                frames * samples_per_frame(*rate)?
            },
        ),
        _ => (0, time_base, duration_ts),
    };
    let valid = match &media.profile {
        Profile::Mov { pcm, .. } if index == 1 => {
            selector
                .channel
                .is_some_and(|c| c < 2 && source.stream.is_some())
                && source.sample_rate == Some(48000)
                && source.bits == *pcm
                && (media.profile.timecoded_camera_pcm() && source.codec.as_deref() == Some("pcm")
                    || source.codec.as_deref() == Some(match pcm {
                    Some(16) => "pcm_s16le", Some(24) => "pcm_s24le", _ => "aac",
                }))
        }
        Profile::Mov { rate, video, raster: (width, height), .. } => {
            selector.channel.is_none()
                && source.kind == StreamKind::Video
                && source.rate == Some(*rate)
                && source.frame_size == Some((i64::from(*width), i64::from(*height)))
                && source.codec.as_deref().is_some_and(|codec| video.matches_source_codec(codec))
        }
        Profile::Wav { bits, channels, .. } => {
            selector
                .channel
                .is_some_and(|c| c < *channels as usize && source.stream == Some(c as i64 + 1))
                && source.kind == StreamKind::Audio
                && source.sample_rate == Some(48000)
                && source.bits == Some(*bits)
        }
        Profile::BoundMxf(_) | Profile::MonoAtom { .. } => false,
    };
    let path_agrees = source.locations.iter().any(|s| Path::new(s) == media.path);
    let origin_agrees = match (&media.profile, source.kind, media.profile.movie_timecode()) {
        (Profile::Mov { rate, .. }, StreamKind::Video, Some(start)) => {
            source.start == Rational::new(i128::from(start) * i128::from(rate.den()), rate.num().into())
        }
        _ => source.start.is_none_or(|v| v == Rational::ZERO),
    };
    if selector.index != index
        || !valid
        || !path_agrees
        || !origin_agrees
        || source.duration != extent(duration_ts, time_base)?
    {
        return Err("graph media facts disagree with verified linked file".into());
    }
    if let Some(original) = &source.original {
        let expected_channels = match (&media.profile, index) {
            (Profile::Mov { .. }, 1) => Some(2),
            (Profile::Wav { channels, .. }, _) => Some(*channels),
            _ => None,
        };
        let expected_container = if matches!(media.profile, Profile::Wav { .. }) {
            "wav"
        } else {
            "mov"
        };
        if original.stream_index != index as i64
            || original.container != expected_container
            || original.channels != expected_channels
            || original.channel_index != selector.channel.map(|c| c as i64)
            || original.time_base != time_base
            || original.start_pts != 0
            || original.duration_ts != duration_ts
        {
            return Err(
                "saved original selector or clock disagrees with verified linked file".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn check_origin(sequence: &SourceSequence) -> Result<i64> {
    let clock = sequence
        .clock
        .as_ref()
        .ok_or("qualified sequence origin required")?;
    let witness = &clock.provenance;
    if clock.rate != sequence.rate
        || clock.display_base != display_base(sequence.rate)
        || clock.drop != is_2997(sequence.rate)
        || clock.drop && clock.effective_count >= 2_589_408
        || clock.exact_start() != sequence.start
        || !matches!(
            witness.route.as_str(),
            "Resolve archive clock" | "direct sequence TC1" | "Premiere sequence ticks"
        )
        || witness.source_rate != sequence.rate
        || witness.origin_count != clock.effective_count
        || witness.source_offset != 0
        || (witness.route == "Resolve archive clock"
            && (witness.timecode_track != "Sm2Sequence.MediaExtents"
                || witness.source_package != sequence.id))
        || (witness.route == "Premiere sequence ticks"
            && (witness.timecode_track != "MZ.ZeroPoint"
                || witness.source_package != sequence.id))
    {
        return Err("qualified exact Resolve timeline origin required".into());
    }
    Ok(i64::try_from(clock.effective_count)?)
}

pub fn prepare(graph: &SourceGraph, spec: ProjectSpec, ffprobe: &Path) -> Result<PreparedProject> {
    prepare_project(graph, spec, ffprobe, None, MediaPreparation::Automatic)
}

pub(crate) fn prepare_project(
    graph: &SourceGraph,
    spec: ProjectSpec,
    ffprobe: &Path,
    review_caption_omissions: Option<&[graph::Loss]>,
    media_preparation: MediaPreparation<'_>,
) -> Result<PreparedProject> {
    let (premiere_files, allow_native_import) = match media_preparation {
        MediaPreparation::VerifiedPremiere(files) => (Some(files), false),
        MediaPreparation::Automatic => (None, true),
        MediaPreparation::StandaloneOnly => (None, false),
    };
    graph.validate()?;
    if !(1..=64).contains(&spec.timelines.len())
        || !(1..=256).contains(&spec.media.len().saturating_add(spec.atom_media.len()))
        || graph.bins.len() > 256
    {
        return Err("linked project exceeds timeline, media or bin limits".into());
    }
    let sidecar_index = graph.sidecars.iter()
        .map(|sidecar| ((sidecar.adapter.as_str(), sidecar.object.as_str()), &sidecar.data))
        .collect();
    let sidecars = SidecarLookup::Indexed(&sidecar_index);
    let nests = crate::nests::plan(graph, &spec.timelines)?;
    let nested_children: HashSet<_> = nests.iter().flatten().map(|usage| usage.child).collect();
    let stereo_children: HashSet<_> = nests.iter().flatten()
        .filter(|usage| usage.stereo_audio.is_some()).map(|usage| usage.child).collect();
    if !stereo_children.is_empty() && spec.timelines.len() != 2 {
        return Err("a Resolve stereo audio nest requires one selected parent and child".into());
    }
    let mut bins = vec![Bin {
        parent: None,
        name: "Master".into(),
    }];
    let mut bin_ids = HashMap::new();
    if let Some(root) = spec.root_bin_id.as_deref() {
        if !graph
            .bins
            .iter()
            .any(|bin| bin.id == root && bin.parent.is_none() && bin.name == "Master")
        {
            return Err("explicit root must identify a top-level Master bin".into());
        }
        bin_ids.insert(root, 0);
    }
    for bin in &graph.bins {
        if Some(bin.id.as_str()) != spec.root_bin_id.as_deref() {
            bin_ids.insert(&bin.id, bins.len());
            bins.push(Bin {
                parent: Some(0),
                name: checked_name(&bin.name)?,
            });
        }
    }
    for bin in &graph.bins {
        if let Some(parent) = &bin.parent {
            bins[bin_ids[bin.id.as_str()]].parent = Some(bin_ids[parent.as_str()]);
        }
    }
    let items: HashMap<_, _> = graph.items.iter().map(|x| (x.id.as_str(), x)).collect();
    let streams: HashMap<_, _> = graph.media.iter().map(|x| (x.id.as_str(), x)).collect();
    let mut media = Vec::new();
    let mut media_bytes = 0u64;
    let mut counted_files = HashSet::new();
    let mut replay_files: HashMap<_, (Media, Rational, i64)> = HashMap::new();
    let mut selected = HashMap::new();
    let mut independent_audio = HashMap::new();
    let group_cameras: HashSet<_> = spec.timelines.iter()
        .filter_map(|row| row.group.as_ref())
        .flat_map(|group| group.cameras.iter().map(String::as_str))
        .collect();
    for binding in &spec.media {
        let item_id = binding.item_id.as_str();
        if selected.contains_key(item_id) {
            return Err(format!("duplicate linked item binding: {item_id}").into());
        }
        let size = fs::metadata(&binding.path)
            .map_err(|error| format!("{item_id}: linked media metadata unavailable: {error}"))?
            .len();
        let file_key = fs::canonicalize(&binding.path)?;
        if counted_files.insert(file_key.clone()) {
            media_bytes = media_bytes.checked_add(size).ok_or("linked media size overflows")?;
        }
        if media_bytes > 8u64 << 30 {
            return Err("linked project media exceeds the 8 GiB research budget".into());
        }
        let item = items.get(item_id).ok_or("linked binding item absent")?;
        let bin = graph
            .bins
            .iter()
            .find(|b| b.id == binding.bin_id && b.items.contains(&binding.item_id))
            .ok_or("linked media bin assignment contradicts graph membership")?;
        if (!item.stream_groups.is_empty() && item.stream_groups.as_slice() != [vec![1, 2]])
            || binding.streams.len() != item.media.len()
            || !(1..=3).contains(&binding.streams.len())
        {
            return Err(format!("unqualified routing or stream count for {item_id}").into());
        }
        let mut avid_source_timecode = None;
        for link in &item.media {
            let source = streams[link.media.as_str()];
            if let Some(clock) = source.clock.as_ref().filter(|clock|
                clock.provenance.route == graph::source::AVID_SOURCE_COLUMNS_TIMECODE)
            {
                if avid_source_timecode.is_some() || source.kind != StreamKind::Video
                    || !supported_rate(clock.rate) || clock.drop || clock.display_base != 24
                {
                    return Err(format!("{item_id}: unsupported camera-original timecode").into());
                }
                avid_source_timecode = Some(i64::try_from(clock.effective_count)?);
            }
        }
        let name = checked_name(&item.name)?;
        let bin_index = bin_ids[bin.id.as_str()];
        if group_cameras.contains(item_id)
            && sidecars.get(crate::premiere::MULTICAM_CAMERA, item_id).is_some()
        {
            let selectors = binding.streams.iter().map(|stream| StreamSelector {
                media_id: stream.media_id.clone(),
                index: stream.index,
                channel: stream.channel,
            }).collect::<Vec<_>>().try_into()
                .map_err(|_| format!("{item_id}: expected three multicam essence selectors"))?;
            let verified = verify_group_binding(BindingSpec {
                item_id: binding.item_id.clone(),
                path: binding.path.clone(),
                sha256: binding.sha256.clone(),
                streams: selectors,
            }, ffprobe).map_err(|error| format!("{item_id}: {error}"))?;
            check_binding(graph, &verified)?;
            selected.insert(item_id, (media.len(), HashMap::new()));
            media.push(Media {
                name,
                bin: bin_index,
                path: verified.path.clone(),
                modified_micros: verified.modified_micros,
                calendar_seconds: None,
                identity: MediaIdentity::Digest(verified.digest),
                profile: Profile::BoundMxf(Arc::new(verified)),
                proxy: None,
                native: None,
            });
            continue;
        }
        let proxy = avid_adapter::proxy::Attachment::from_graph(graph,item_id)?;
        let canvas = spec.timelines.first().map(|timeline| timeline.canvas);
        let (verified, tb, duration_ts) = if let Some(files) = premiere_files {
            let (file, time_base, count) = files.get(&binding.path)
                .ok_or_else(|| format!("{item_id}: missing verified Premiere original"))?;
            if avid_source_timecode.is_some() || proxy.is_some() || file.digest_hex()? != binding.sha256 {
                return Err(format!("{item_id}: incompatible verified Premiere media binding").into());
            }
            (file.for_item(name, bin_index), *time_base, *count)
        } else if proxy.is_none() && crate::native_import::eligible(&binding.path) {
            let key = (file_key, binding.sha256.clone(), avid_source_timecode);
            let (file, time_base, count) = match replay_files.entry(key) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => entry.insert(
                    probe_linked(binding, ffprobe, name.clone(), bin_index, avid_source_timecode, None, canvas, allow_native_import)
                        .map_err(|error| format!("{item_id}: {error}"))?,
                ),
            };
            (file.for_item(name, bin_index), *time_base, *count)
        } else {
            probe_linked(binding, ffprobe, name, bin_index, avid_source_timecode, proxy, canvas, allow_native_import)
                .map_err(|error| format!("{item_id}: {error}"))?
        };
        let mut by_label = HashMap::new();
        let mut seen = HashSet::new();
        let expected = match &verified.profile {
            Profile::Mov { aac, pcm, .. } => {
                if *aac || pcm.is_some() {
                    3
                } else {
                    1
                }
            }
            Profile::Wav { channels, .. } => *channels as usize,
            Profile::BoundMxf(_) | Profile::MonoAtom { .. } =>
                return Err("unexpected split MXF in MOV/WAV probe".into()),
        };
        if binding.streams.len() != expected {
            return Err(format!("{item_id}: incomplete physical selector set").into());
        }
        for selector in &binding.streams {
            let link = item
                .media
                .iter()
                .find(|l| l.media == selector.media_id)
                .ok_or("linked selector has the wrong item owner")?;
            if link.source_offset != Some(Rational::ZERO)
                || link.gain_db != 0.0
                || !seen.insert(selector.channel)
            {
                return Err(format!(
                    "{item_id}: unresolved offset, gain or duplicate physical selector"
                )
                .into());
            }
            let source = streams[link.media.as_str()];
            check_stream(source, selector, &verified, tb, duration_ts)
                .map_err(|e| format!("{item_id}: {e}"))?;
            let label = if verified.native.is_some() && source.kind == StreamKind::Video {
                None
            } else {
                source.stream
            };
            if by_label.insert(label, selector.channel).is_some() {
                return Err(format!("{item_id}: duplicate logical source stream label").into());
            }
        }
        if media.len() == 256 {
            return Err("selected media exceed the Resolve project limit".into());
        }
        selected.insert(item_id, (media.len(), by_label));
        media.push(verified);
        if let Some(siblings) = spec.proxy_audio.get(item_id) {
            let Profile::Mov { frames, pcm: Some(bits), avid_source_timecode: Some(start), .. } =
                media.last().ok_or("missing verified proxy picture")?.profile
            else {
                return Err(format!("{item_id}: independent proxy audio requires a verified stereo proxy").into());
            };
            if siblings[0].path == siblings[1].path || media.len() + 2 > 256 {
                return Err(format!("{item_id}: duplicate audio sibling or Resolve media item limit").into());
            }
            for (channel, sibling) in siblings.iter().enumerate() {
                let source = binding.streams.iter().find(|selector| selector.channel == Some(channel))
                    .ok_or("missing original channel for verified Avid sibling")?;
                let package = sidecars.get("avid.source", &source.media_id)
                    .and_then(|data| match data.get("file_package") {
                        Some(Value::Str(id)) => Some(id.as_str()),
                        _ => None,
                    })
                    .ok_or("missing Avid audio sibling file package")?;
                let (verified, size) = prepare_proxy_audio(
                    sibling, package, bin_index, frames, bits, start,
                ).map_err(|e| format!("{item_id} A{}: {e}", channel+1))?;
                media_bytes = media_bytes.checked_add(size).ok_or("linked media size overflows")?;
                if media_bytes > 8u64 << 30 {
                    return Err("linked project media exceeds the 8 GiB research budget".into());
                }
                independent_audio.insert((item_id, channel), media.len());
                media.push(verified);
            }
        }
    }
    let mut atom_ids = Vec::new();
    for row in spec.atom_media {
        let item_id = row.binding.item_id.clone();
        if selected.contains_key(item_id.as_str()) {
            return Err(format!("duplicate media item binding: {item_id}").into());
        }
        let bin = graph
            .bins
            .iter()
            .find(|b| b.id == row.bin_id && b.items.contains(&item_id))
            .ok_or_else(|| format!("{item_id}: split MXF bin contradicts graph membership"))?;
        for sibling in &row.binding.siblings {
            if !sibling.path.is_absolute() {
                return Err(format!("{item_id}: split MXF binding requires absolute paths").into());
            }
            let size = fs::metadata(&sibling.path)
                .map_err(|error| format!("{item_id}: split MXF metadata unavailable: {error}"))?
                .len();
            media_bytes = media_bytes
                .checked_add(size)
                .ok_or("linked media size overflows")?;
        }
        if media_bytes > 8u64 << 30 {
            return Err("linked project media exceeds the 8 GiB research budget".into());
        }
        let verified = verify_atom_binding(row.binding, ffprobe)
            .map_err(|error| format!("{item_id}: {error}"))?;
        let item =
            check_binding(graph, &verified).map_err(|error| format!("{item_id}: {error}"))?;
        selected.insert(item.id.as_str(), (media.len(), HashMap::new()));
        if media.len() == 256 {
            return Err("selected media exceed the Resolve project limit".into());
        }
        media.push(Media {
            name: checked_name(&item.name)?,
            bin: bin_ids[bin.id.as_str()],
            path: verified.path.clone(),
            modified_micros: verified.modified_micros,
            calendar_seconds: None,
            identity: MediaIdentity::Digest(verified.digest),
            profile: Profile::BoundMxf(Arc::new(verified)),
            proxy: None,
            native: None,
        });
        atom_ids.push(item_id);
    }
    let mut timelines = Vec::new();
    let mut omitted_markers = Vec::new();
    let mut destination_losses = Vec::new();
    let mut recovered_premiere_adjusts = HashSet::new();
    let mut recovered_premiere_marks = HashSet::new();
    let mut seen = HashSet::new();
    let mut project_rate = None;
    let mut project_canvas = None;
    let mut group_written = false;
    let reviewable_captions: HashSet<_> = review_caption_omissions
        .unwrap_or_default()
        .iter()
        .map(|loss| loss.object.as_str())
        .collect();
    let mut scoped_media: HashSet<_> = spec
        .media
        .iter()
        .flat_map(|m| {
            std::iter::once(m.item_id.as_str()).chain(m.streams.iter().map(|s| s.media_id.as_str()))
        })
        .collect();
    scoped_media.extend(atom_ids.iter().map(String::as_str));
    for verified in &media {
        if let Profile::BoundMxf(binding) = &verified.profile {
            scoped_media.extend(binding.streams.iter().map(|s| s.media_id.as_str()));
        }
    }
    for (row_index, row) in spec.timelines.iter().enumerate() {
        let sequence = graph
            .sequences
            .iter()
            .find(|s| s.id == row.id)
            .ok_or("selected timeline ID absent")?;
        if let Some(edit) = sequence.disabled_placements.first() {
            return Err(format!("{}: disabled source edit has no qualified Resolve mapping", edit.id).into());
        }
        let mut build = || -> Result<(Timeline, Vec<String>)> {
            let avid_ranges = crate::grade::AvidRanges::new(sidecars, sequence)?;
            let premiere_avc_rate = avc_rate(sequence.rate)
                && sidecars.get("premiere.source", &sequence.id).is_some()
                && row.group.is_none();
            let saved_range = sequence.placements.iter()
                .any(|edit| sidecars.get(crate::grade::RANGE_ADAPTER, &edit.id).is_some());
            let range_avc_rate = avc_rate(sequence.rate) && row.group.is_none()
                && (avid_ranges.has_ranges() || saved_range) && sequence.placements.iter().all(|edit|
                    selected.get(edit.item.as_str()).is_some_and(|(index, _)| {
                        matches!(media[*index].profile, Profile::Mov {
                            rate, video: MovieVideo::Avc, raster: (1920, 1080), aac: false, pcm: None, ..
                        } if rate == sequence.rate)
                    }));
            let assisted_rate = row.group.is_none() && crate::native_import::supported_rate(sequence.rate)
                && sequence.placements.iter().any(|edit| selected.get(edit.item.as_str())
                    .is_some_and(|(index, _)| media[*index].native.is_some()));
            let canvas = row.canvas;
            let uhd = canvas == (3840, 2160) && premiere_avc_rate && uhd_rate(sequence.rate);
            let saved_canvas = (i64::from(canvas.0), i64::from(canvas.1));
            if !seen.insert(row.id.as_str())
                || !(canvas == (1920, 1080) || uhd)
                || sequence.frame_size.is_some_and(|size| size != saved_canvas)
                || (uhd && sequence.frame_size.is_none())
                || !(supported_rate(sequence.rate) || premiere_avc_rate || range_avc_rate || assisted_rate)
                || project_rate.is_some_and(|rate| rate != sequence.rate)
                || project_canvas.is_some_and(|size| size != canvas)
                || sequence.texts.iter().any(|text| text.kind != graph::source::TextKind::Caption)
            {
                return Err("unqualified canvas, mixed project rates, duplicate selection or timeline topology".into());
            }
            let compound = crate::nests::compound(graph, &sequence.id);
            let faders = mixer::StaticFaders::prepare(graph, sequence)?;
            let uses = &nests[row_index];
            destination_losses.extend(uses.iter().filter_map(|usage| {
                Some(graph::Loss {
                    object: usage.empty_audio_companion.clone()?,
                    aspect: crate::nests::EMPTY_NEST_AUDIO_LOSS.into(),
                    reason: "The matched child has no audio edits. Resolve keeps the picture nest and omits its empty audio companion.".into(),
                })
            }));
            if sidecars.get(crate::nests::PREMIERE_ADAPTER, &sequence.id).is_some() {
                destination_losses.extend(uses.iter().filter_map(|usage| {
                    Some(graph::Loss {
                        object: usage.stereo_audio.clone()?,
                        aspect: crate::nests::STEREO_NEST_EDGE_LOSS.into(),
                        reason: "Resolve applies short audio ramps at the nested edit edges. The clip remains editable with the original stereo channel order and trim.".into(),
                    })
                }));
            }
            let nest_member = !uses.is_empty() || nested_children.contains(&row_index);
            let stereo_parent = uses.iter().any(|usage| usage.stereo_audio.is_some());
            let stereo_child = stereo_children.contains(&row_index);
            let stereo_nest = stereo_parent || stereo_child;
            let shape = |kind, index| sequence.tracks.iter().any(|t|
                t.kind == kind && t.index == index && t.level_db.is_none());
            if stereo_nest && (canvas != (1920, 1080) || sequence.tracks.len() != 2
                || !shape(StreamKind::Audio, 1)
                || (stereo_child && !sequence.placements.iter().any(crate::stereo::placement)))
            {
                return Err("Resolve stereo audio nesting requires neutral V1 and stereo WAV A1".into());
            }
            if nest_member && (!shape(StreamKind::Video, 1)
                || !(stereo_nest || sequence.tracks.len() == 1 && !compound
                    || sequence.tracks.len() == 3 && !compound
                        && shape(StreamKind::Audio, 1) && shape(StreamKind::Audio, 2)
                    || sequence.tracks.len() == 2 && compound && shape(StreamKind::Audio, 1))
                || sequence.placements.iter().any(|p| {
                    let picture = p.video_track == Some(1)
                        && p.audio_tracks.is_empty() && p.grouped_audio.is_empty();
                    !(picture || stereo_child && crate::stereo::placement(p))
                })
                || !sequence.texts.is_empty() || !sequence.transitions.is_empty()
                || sidecars.get(crate::dissolve::ADAPTER, &sequence.id).is_some()
                || sidecars.get(crate::generator::ADAPTER, &sequence.id).is_some()
                || (compound && !sequence.markers.is_empty()))
            {
                return Err("Resolve nesting requires a picture-only V1 parent and children".into());
            }
            let second_picture = sequence.tracks.iter().any(|track|
                track.kind == StreamKind::Video && track.index == 2 && track.level_db.is_none());
            let track_count = sequence.tracks.len() - usize::from(second_picture);
            let stereo_wav_24 = track_count == 2
                && sequence.rate == Rational::integer(24)
                && canvas == (1920, 1080);
            let stereo = stereo_nest || !nest_member && !second_picture && matches!(track_count, 2 | 3)
                && spec.timelines.len() == 1
                && (stereo_wav_24
                    || crate::stereo::clock(sequence.rate, (i64::from(canvas.0), i64::from(canvas.1))))
                && sequence.placements.iter().any(crate::stereo::placement);
            let mixed_stereo = stereo && track_count == 3;
            // A saved linked project can declare empty mono lanes even when
            // only the picture lane has edits. The writer already emits
            // these lanes for a picture-only timeline.
            let empty_linked_mono = track_count == 3 && !second_picture
                && sidecars.get("resolve.source", &sequence.id).is_some()
                && shape(StreamKind::Video, 1) && shape(StreamKind::Audio, 1)
                && shape(StreamKind::Audio, 2) && faders == Default::default()
                && sequence.placements.iter().all(|edit| edit.video_track == Some(1)
                    && edit.audio_tracks.is_empty() && edit.grouped_audio.is_empty());
            let picture_only = nest_member && !stereo_nest || track_count == 1 || empty_linked_mono;
            let third_mono = track_count == 4 && second_picture && sequence.rate == Rational::integer(24);
            if (!picture_only && track_count != 3 && !third_mono && !stereo)
                || [
                    (StreamKind::Video, 1),
                    (StreamKind::Audio, 1),
                    (StreamKind::Audio, 2),
                ]
                .into_iter()
                .take(if picture_only { 1 } else if stereo && !mixed_stereo { 2 } else { 3 })
                .any(|(kind, index)| {
                    !sequence
                        .tracks
                        .iter()
                        .any(|t| t.kind == kind && t.index == index)
                })
                || (third_mono && !sequence.tracks.iter().any(|track|
                    track.kind == StreamKind::Audio && track.index == 3 && track.level_db.is_none()))
            {
                return Err("expected a qualified Resolve picture and independent mono track layout".into());
            }
            let bin = *bin_ids
                .get(row.bin_id.as_str())
                .ok_or("timeline bin assignment absent")?;
            if let Some(Value::Str(saved)) = sidecars
                .get("resolve.source", &sequence.id)
                .and_then(|data| data.get("bin_id"))
            {
                if saved != &row.bin_id {
                    return Err("explicit timeline bin differs from saved owning bin".into());
                }
            }
            let origin = check_origin(sequence)?;
            if let Some(group) = &row.group {
                if stereo {
                    return Err("Group timelines require independent mono tracks".into());
                }
                if faders != mixer::StaticFaders::default() {
                    return Err("Group timelines require neutral track and bus faders".into());
                }
                if second_picture || !sequence.texts.is_empty() || !sequence.transitions.is_empty()
                    || sidecars.get(crate::generator::ADAPTER, &sequence.id).is_some()
                {
                    return Err("subtitle tracks and transitions on Group timelines are unqualified".into());
                }
                if group_written || sequence.rate != rate() {
                    return Err("project supports one same-rate MXF Group timeline".into());
                }
                let group_bin = *bin_ids
                    .get(group.bin_id.as_str())
                    .ok_or("Group bin absent")?;
                let resolve_camera = |id: &str| -> Result<(usize, Arc<VerifiedBinding>)> {
                    let index = selected
                        .get(id)
                        .ok_or("no verified Group camera binding")?
                        .0;
                    let Profile::BoundMxf(binding) = &media[index].profile else {
                        return Err(
                            "project Group cameras require verified MXF bindings".into()
                        );
                    };
                    Ok((index, Arc::clone(binding)))
                };
                let (a, a_binding) = resolve_camera(&group.cameras[0])?;
                let (b, b_binding) = resolve_camera(&group.cameras[1])?;
                let prepared =
                    prepare_bound_group(graph, &row.id, row.canvas, None, [a_binding, b_binding])?;
                let omitted = sequence
                    .markers
                    .iter()
                    .map(|marker| marker.owner_occurrence.clone())
                    .collect();
                group_written = true;
                return Ok((
                    Timeline {
                        bin,
                        name: checked_name(&sequence.name)?,
                        origin,
                        length: prepared.end - prepared.origin,
                        lanes: Default::default(),
                        markers: vec![],
                        marks: None,
                        track_state: None,
                        mixer: None,
                        faders: Default::default(),
                        dissolves: vec![],
                        captions: vec![],
                        solid: None,
                        compound: false,
                        nests: vec![],
                        stereo: false,
                        group: Some(GroupTimeline {
                            bin: group_bin,
                            cameras: [a, b],
                            prepared,
                        }),
                    },
                    omitted,
                ));
            }
            if graph
                .multicam_uses
                .iter()
                .any(|m| m.sequence == sequence.id)
            {
                return Err(
                    "Group timeline requires explicit verified camera and bin bindings".into(),
                );
            }
            let mixer = mixer::DirectMixer::prepare(graph, sequence)?;
            if faders != mixer::StaticFaders::default()
                && (picture_only || second_picture || third_mono || mixer.is_some() || nest_member)
            {
                return Err("static faders require a linked V1/A1/A2 timeline with a neutral mixer".into());
            }
            let track_state = crate::track_state::TrackState::prepare_for_resolve(graph, sequence)?;
            if faders.pans != [0; 2] && track_state.is_some() {
                return Err("linked track pan with track names, locks or output state is unqualified".into());
            }
            if track_state.is_some() && (picture_only || compound || second_picture || third_mono) {
                return Err("Resolve track names and locks require a registered V1/A1/A2 timeline".into());
            }
            let native_dissolves = dissolve::prepare_all_native(graph, sequence)?;
            let source_dissolves = dissolve::from_source(sequence)?;
            if stereo && (faders != Default::default() || mixer.is_some() || track_state.is_some()
                || !native_dissolves.is_empty() || !source_dissolves.is_empty()
                || !sequence.texts.is_empty())
            {
                return Err("linked stereo A1 requires a neutral timeline with straight cuts".into());
            }
            let solid = crate::generator::prepare(graph, sequence)?;
            if solid.is_some() && !picture_only {
                return Err("Solid Color requires a picture-only linked project timeline".into());
            }
            if !native_dissolves.is_empty() && !source_dissolves.is_empty() {
                return Err("conflicting Resolve and cross-editor transition facts".into());
            }
            for fade in source_dissolves.iter().filter(|fade| fade.prepared.audio()) {
                if graph.losses.iter().any(|loss| {
                    (loss.object == fade.outgoing || loss.object == fade.incoming)
                        && loss.aspect == "premiere.appearance"
                }) {
                    return Err(format!(
                        "{}: A1 crossfade endpoints have unsupported clip processing",
                        fade.prepared.id
                    ).into());
                }
                if fade.prepared.style == dissolve::Style::Power {
                    destination_losses.push(graph::Loss {
                        object: fade.prepared.id.clone(),
                        aspect: "transition_approximation".into(),
                        reason: "Premiere Constant Power maps to an editable Resolve Cross Fade +3 dB. The audio envelope differs.".into(),
                    });
                }
            }
            let transition_heads: HashMap<_, _> = source_dissolves.iter()
                .map(|transition| (transition.incoming, transition.prepared.before))
                .collect();
            let transition_tails: HashMap<_, _> = source_dissolves.iter()
                .map(|transition| (transition.outgoing, transition.prepared.after()))
                .collect();
            if mixer.is_some() && (picture_only || third_mono) {
                return Err(
                    "direct Fairlight mixer requires audio on the selected timeline".into(),
                );
            }
            let placement_ids: HashSet<_> =
                sequence.placements.iter().map(|p| p.id.as_str()).collect();
            let linear_resizes: HashSet<_> = sequence
                .placements
                .iter()
                .filter(|p| matches!(p.effects.as_slice(), [effect] if effect.kind == "avid.resize_linear"))
                .map(|p| p.id.as_str())
                .collect();
            let mut recovered_labels = HashSet::new();
            let mut recovered_avid_ranges = HashSet::new();
            let mut verified_color_media = HashSet::new();
            let mut marker_preparation = markers::TimelineMarkerPreparation::new(graph, sequence, sidecars);
            let has_a1_crossfade = source_dissolves.iter().chain(&native_dissolves)
                .any(|fade| fade.prepared.audio());
            let mut clip_envelopes =
                clip_envelope::ClipEnvelopePreparation::new(graph, sequence, has_a1_crossfade, sidecars);
            let mut lanes: [Vec<Edit>; 5] = Default::default();
            for edit in &sequence.placements {
                let stereo_edit = stereo && crate::stereo::placement(edit);
                check_linked_placement(edit, stereo_edit).map_err(|e| format!("{}: {e}", edit.id))?;
                let (media_index, selectors) = selected
                    .get(edit.item.as_str())
                    .ok_or_else(|| format!("no verified file binding for item {}", edit.item))?;
                let linear = sidecars.get(crate::retime::LINEAR_ADAPTER, &edit.id);
                let (mixed_rate, mixed_linear) = match &media[*media_index].profile {
                    Profile::Mov { rate, video, aac: false, pcm: None,
                        quicktime_timecode, avid_source_timecode: None, .. }
                        if *video == MovieVideo::Avc
                            || *video == MovieVideo::ProResHq && canvas == (1920, 1080) =>
                    {
                        if linear.is_some() {
                            (None, true)
                        } else if !stereo && *video == MovieVideo::ProResHq
                            && quicktime_timecode.is_none_or(|start| start == 0)
                            && native_dissolves.is_empty() && source_dissolves.is_empty()
                        {
                            (crate::mixed_rate::Cadence::between(*rate, sequence.rate), false)
                        } else {
                            (None, false)
                        }
                    }
                    Profile::Mov { video: MovieVideo::Imported, aac: false, pcm: None, .. }
                        if linear.is_some() => (None, true),
                    _ => (None, false),
                };
                let raster_mismatch =
                    matches!(&media[*media_index].profile, Profile::Mov { raster, .. } if *raster != canvas);
                if raster_mismatch && media[*media_index].native.is_none() {
                    return Err(format!("{}: original raster differs from the Resolve canvas", edit.id).into());
                }
                let (lane, channel) = match (
                    edit.video_track,
                    edit.audio_tracks.as_slice(),
                    edit.audio_streams.as_slice(),
                    &media[*media_index].profile,
                ) {
                    (None, [], [], profile) if stereo_edit => {
                        let supported = match profile {
                            Profile::Mov { rate, video: MovieVideo::Avc, pcm: Some(_), .. }
                                if !mixed_stereo && !stereo_wav_24 =>
                                *rate == sequence.rate && crate::stereo::mov(&media[*media_index].path),
                            Profile::Wav { channels: 2, .. } => true,
                            _ => false,
                        };
                        if !supported || selectors.get(&Some(1)) != Some(&Some(0))
                            || selectors.get(&Some(2)) != Some(&Some(1))
                        {
                            if stereo_wav_24 {
                                return Err(format!("{}: 24 fps stereo A1 requires verified left/right PCM channels from WAV", edit.id).into());
                            }
                            return Err(format!("{}: stereo A1 requires verified left/right PCM channels from WAV or AVC MOV", edit.id).into());
                        }
                        (1, None)
                    }
                    (Some(1), [], [], Profile::Mov { rate, .. }) if *rate == sequence.rate || mixed_rate.is_some() || mixed_linear => {
                        (0, None)
                    }
                    (Some(2), [], [], Profile::Mov { rate, .. })
                        if second_picture && *rate == sequence.rate => (3, None),
                    (None, [lane @ (1 | 2)], [label], Profile::Wav { .. }) => (
                        *lane as usize,
                        selectors.get(&Some(*label)).copied().flatten(),
                    ),
                    (None, [3], [label], Profile::Wav { .. }) if third_mono => (
                        4, selectors.get(&Some(*label)).copied().flatten(),
                    ),
                    (
                        None,
                        [lane @ (1 | 2)],
                        [label],
                        Profile::Mov {
                            rate, aac, pcm, ..
                        },
                    ) if *rate == sequence.rate && (*aac || pcm.is_some()) => (
                        *lane as usize,
                        selectors.get(&Some(*label)).copied().flatten(),
                    ),
                    (Some(1), [], [], Profile::BoundMxf(binding))
                        if sequence.rate == rate() && binding.atom.is_some() => (0, None),
                    (None, [lane @ (1 | 2)], [label @ (1 | 2)], Profile::BoundMxf(binding))
                        if sequence.rate == rate() && lane == label && binding.atom.is_some() =>
                    {
                        (*lane as usize, Some((*label - 1) as usize))
                    }
                    _ => {
                        return Err(
                            format!("{}: unsupported mixed rate or lane routing", edit.id).into(),
                        )
                    }
                };
                if stereo && !(lane == 0 || stereo_edit
                    || mixed_stereo && lane == 2
                        && matches!(media[*media_index].profile, Profile::Wav { channels: 1, .. }))
                {
                    return Err(format!("{}: stereo A1 allows only an independent mono WAV on A2", edit.id).into());
                }
                let (media_index, channel) = match channel.and_then(|ch| {
                    independent_audio.get(&(edit.item.as_str(), ch))
                }) {
                    Some(index) => (*index, Some(0)),
                    None => (*media_index, channel),
                };
                if let Some(native) = &media[media_index].native {
                    let linear_picture = lane == 0 && linear.is_some();
                    // TimingContext checks the exact clocks, frame samples, and framing below.
                    if canvas != (1920, 1080)
                        || !crate::native_import::supported_rate(sequence.rate) || !matches!(lane, 0 | 1 | 2)
                        || stereo_edit || edit.speed.is_some()
                        || edit.frame_map.is_some() && !linear_picture
                        || edit.transform.is_some() && !(linear_picture && native.audio_clock().is_none())
                        || !edit.effects.is_empty()
                        || !source_dissolves.is_empty() || !native_dissolves.is_empty()
                    {
                        return Err(format!("{}: native import requires a same-rate 1080p straight cut", edit.id).into());
                    }
                    if lane != 0 {
                        native.check_audio_end(edit).map_err(|error| format!("{}: {error}", edit.id))?;
                    }
                }
                if (matches!(lane, 1 | 2 | 4) && channel.is_none() && !stereo_edit) || lanes[lane].len() == MAX_EDITS_PER_LANE {
                    return Err(format!(
                        "{}: unresolved physical channel or edit count limit",
                        edit.id
                    )
                    .into());
                }
                let premiere_fades = clip_envelopes.check_premiere_fades(
                    edit, lane, &media[media_index].profile,
                )?;
                let mut start = frames_on(edit.record_start, sequence.rate)?;
                let trim_rate = match (&media[media_index].profile, linear) {
                    (Profile::Mov { rate, .. }, Some(_)) => *rate,
                    _ => sequence.rate,
                };
                let mut source_in = frames_on(edit.source_in, trim_rate)?;
                let mut length = frames_on(edit.duration, sequence.rate)?;
                if let Some(&preroll) = transition_heads.get(edit.id.as_str()) {
                    start = start.checked_add(preroll).ok_or("dissolve start overflow")?;
                    source_in = source_in.checked_add(preroll).ok_or("dissolve source in overflow")?;
                    length -= preroll;
                }
                if let Some(&postroll) = transition_tails.get(edit.id.as_str()) {
                    length -= postroll;
                }
                let PreparedTiming { timing, source_in, source_end, loss } = TimingContext {
                    media: &media[media_index],
                    rate: sequence.rate,
                    canvas,
                    lane,
                    picture_only,
                    raster_mismatch,
                    transition_trimmed: transition_heads.contains_key(edit.id.as_str())
                        || transition_tails.contains_key(edit.id.as_str()),
                    mixed_rate,
                    mixed_linear,
                    can_review_losses: review_caption_omissions.is_some(),
                }.prepare(linear, edit, source_in, length)?;
                destination_losses.extend(loss);
                if is_2997(sequence.rate) && matches!(lane, 1 | 2) {
                    if matches!(media[media_index].profile, Profile::Wav { .. }) {
                        check_premiere_audio_record_grid(origin, start, sequence.rate)
                            .map_err(|error| format!("{} A{lane} ({}): {error}", sequence.name, edit.id))?;
                    }
                    let aligned_record = whole_sample_count(start, sequence.rate).is_ok();
                    let exact = aligned_record && [source_in, length]
                        .into_iter().all(|frame| whole_sample_count(frame, sequence.rate).is_ok());
                    let resolve_source = sidecars.get(crate::source::ADAPTER, &sequence.id)
                        .and_then(|record| record.get("kind"))
                        .is_some_and(|kind| matches!(kind, Value::Str(kind) if kind == "registered_timeline"));
                    let verified_movie = media[media_index].native.as_ref()
                        .is_some_and(|movie| movie.verified_fractional_frame_audio());
                    if !exact && !(resolve_source && verified_movie) {
                        if !verified_movie || sidecars.get("premiere.source", &sequence.id).is_none() {
                            return Err(format!("{}: 29.97 audio edit must align to whole 48 kHz samples", edit.id).into());
                        }
                        check_premiere_audio_record_grid(origin, start, sequence.rate)
                            .map_err(|error| format!("{} A{lane} ({}): {error}", sequence.name, edit.id))?;
                    }
                }
                let available = match &media[media_index].profile {
                    Profile::Mov { frames, .. } | Profile::MonoAtom { frames, .. } => *frames,
                    Profile::Wav { samples, .. } => wav_playable_frames(*samples, sequence.rate)?,
                    Profile::BoundMxf(binding) => binding.frames,
                };
                if length <= 0 || source_end > available {
                    return Err(
                        format!("{}: edit exceeds the physical playable range", edit.id).into(),
                    );
                }
                let envelope = clip_envelopes.prepare(
                    edit, lane, &media[media_index].profile, source_in, length, premiere_fades,
                )?;
                let prepared_pin = match (edit.transform.as_ref(), edit.effects.as_slice()) {
                    (None, [effect]) => crate::clip_fusion::prepare_pin(effect)
                        .map_err(|error| format!("{}: {error}", edit.id))?,
                    _ => None,
                };
                let video = if prepared_pin.is_some() {
                    None
                } else {
                    crate::video::prepare(
                        edit, sequence.rate, source_in, length, available, matches!(timing, Timing::Linear(_)),
                    )
                        .map_err(|e| format!("{}: {e}", edit.id))?
                };
                let mut grade = crate::grade::prepare_with_sidecars(sidecars, &edit.id)?;
                if lane == 0 && canvas == (1920, 1080) && matches!(timing, Timing::Straight)
                    && media[media_index].native.is_none() && media[media_index].proxy.is_none()
                {
                    let source = &items[edit.item.as_str()].media;
                    if let [binding] = source.as_slice() {
                        let source_media = streams[binding.media.as_str()];
                        if source_media.online == Some(true) && source_media.rate == Some(sequence.rate)
                            && binding.source_offset == Some(Rational::ZERO)
                            && source_media.original.as_ref().is_some_and(|original|
                                original.container == "mov" && original.stream_index == 0)
                        {
                            if let Some(range) = avid_ranges.prepare(sidecars, edit, source_media)? {
                                if grade.is_some() || video.is_some() {
                                    return Err(format!("{}: conflicting picture and Avid range operations", edit.id).into());
                                }
                                grade = Some(range);
                                recovered_avid_ranges.insert(edit.id.as_str());
                            }
                        }
                    }
                }
                if matches!(grade.as_ref(), Some((crate::grade::Operation::RangeCdl(_), _))) {
                    let physical = &media[media_index];
                    let refuse = || format!("{}: range CDL needs a plain same-rate 1080p Rec.709 picture-only MOV", edit.id);
                    let Profile::Mov {
                        rate, frames, video: codec, raster: (1920, 1080), aac: false, pcm: None, ..
                    } = physical.profile else {
                        return Err(refuse().into());
                    };
                    if lane != 0 || canvas != (1920, 1080) || !matches!(timing, Timing::Straight)
                        || rate != sequence.rate || video.is_some() || physical.native.is_some()
                        || physical.proxy.is_some() || raster_mismatch
                    {
                        return Err(refuse().into());
                    }
                    if verified_color_media.insert(media_index) {
                        match codec {
                            MovieVideo::Avc => avid_adapter::original::verify_avc_range_scale_picture(
                                &physical.path, ffprobe, rate, frames)?,
                            MovieVideo::ProResHq if rate == Rational::integer(24) =>
                                avid_adapter::original::verify_range_scale_picture(&physical.path, ffprobe, frames)?,
                            _ => return Err(refuse().into()),
                        }
                    }
                }
                if matches!(timing, Timing::Linear(_)) && grade.is_some() {
                    return Err(format!("{}: Linear retime with a saved grade is unqualified", edit.id).into());
                }
                if grade.as_ref().is_some_and(|(operation, _)| matches!(operation, crate::grade::Operation::ExposureCdl(_)))
                    && (lane != 0 || sequence.rate != Rational::integer(24) || row.canvas != (1920, 1080)
                        || matches!(timing, Timing::Hold | Timing::Constant(_)) || video.is_some()
                        || !matches!(&media[media_index].profile, Profile::Mov {
                            video: MovieVideo::ProResHq, raster: (1920,1080), aac: false, pcm: None, ..
                        }))
                {
                    if sidecars.get(crate::premiere::color_mode::ADAPTER, &edit.id)
                        .and_then(crate::premiere::color_mode::qualified_exposure).is_some()
                    {
                        destination_losses.push(graph::Loss {
                            object: edit.id.clone(),
                            aspect: "premiere.appearance".into(),
                            reason: "Resolve Color Mode Exposure needs plain 1080p24 ProRes HQ picture".into(),
                        });
                        grade = None;
                    } else {
                        return Err(format!("{}: Resolve exposure CDL is outside the qualified picture profile", edit.id).into());
                    }
                }
                if grade.is_some() {
                    if matches!(timing, Timing::Hold) {
                        return Err(format!("{}: Resolve frame hold with a saved grade is unqualified", edit.id).into());
                    }
                    if matches!(timing, Timing::MixedRate(_)) {
                        return Err(format!("{}: Resolve mixed-rate picture with a saved grade is unqualified", edit.id).into());
                    }
                }
                if uhd && (video.is_some() || grade.is_some() || matches!(timing, Timing::Constant(_))) {
                    return Err(format!("{}: editable UHD picture effects require a separate profile", edit.id).into());
                }
                let input_sizing = crate::input_scaling::prepare(
                    sidecars, &sequence.id, &edit.id, edit.video_track.is_some() && raster_mismatch,
                ).map_err(|error| format!("{}: {error}", edit.id))?;
                let adjust = sidecars.get(crate::premiere::color_mode::ADAPTER, &edit.id)
                    .and_then(crate::premiere::color_mode::qualified_adjust);
                let fusion_profile = lane == 0 && picture_only && canvas == (1920, 1080)
                    && sequence.rate == Rational::integer(24) && matches!(timing, Timing::Straight)
                    && grade.is_none() && video.is_none() && input_sizing.is_none()
                    && source_dissolves.is_empty() && native_dissolves.is_empty()
                    && crate::clip_fusion::supports_media(&media[media_index]);
                let fusion = if let Some(operation) = prepared_pin {
                    if !fusion_profile || adjust.is_some() {
                        return Err(format!("{}: Resolve Corner Pin requires plain 1080p24 Rec.709 ProRes HQ picture", edit.id).into());
                    }
                    Some(operation)
                } else if let Some(adjust) = adjust.filter(|_| fusion_profile) {
                    recovered_premiere_adjusts.insert(edit.id.clone());
                    destination_losses.push(graph::Loss {
                        object: edit.id.clone(),
                        aspect: crate::clip_fusion::APPROXIMATION.into(),
                        reason: "Premiere Color Mode Adjust maps to editable Resolve Fusion controls using a measured Rec.709 approximation.".into(),
                    });
                    Some(crate::clip_fusion::Operation::Adjust(adjust))
                } else {
                    None
                };
                if input_sizing == Some(crate::input_scaling::Sizing::CenterCrop)
                    && (lane != 0 || media[media_index].native.is_none()
                    || video.is_some() || grade.is_some() || matches!(timing, Timing::Constant(_) | Timing::Hold | Timing::MixedRate(_)))
                {
                    return Err(format!("{}: center crop requires a plain assisted picture edit", edit.id).into());
                }
                let label = ClipLabel::prepare(graph, &edit.id)?;
                if stereo && (video.is_some() || matches!(timing, Timing::Constant(_)) || grade.is_some() || envelope.is_some()) {
                    return Err(format!("{}: linked stereo A1 requires neutral picture and audio edits", edit.id).into());
                }
                if label.is_some() {
                    recovered_labels.insert(edit.id.as_str());
                }
                if grade.is_some() && lane != 0 {
                    return Err("Resolve primary gain must belong to a V1 picture edit".into());
                }
                let markers = marker_preparation.prepare_edit(
                    edit,
                    &media[media_index].profile,
                    lane == 0 && matches!(timing, Timing::Straight),
                )?;
                lanes[lane].push(Edit {
                    media: media_index,
                    channel,
                    start,
                    source_in,
                    length,
                    video,
                    input_sizing,
                    envelope,
                    label,
                    markers,
                    timing,
                    grade,
                    fusion,
                });
            }
            let mut duration = None;
            let base_lanes: &[usize] = if picture_only || stereo_parent { &[0] }
                else if stereo && !mixed_stereo { &[0, 1] }
                else if third_mono { &[0, 1, 2, 4] }
                else { &[0, 1, 2] };
            for &index in base_lanes {
                let lane = &mut lanes[index];
                lane.sort_by_key(|e| e.start);
                // The retained picture-only trims can begin with Avid filler.
                // Keep that gap at record time without shifting clip-local keys.
                let mut ranges: Vec<_> = lane.iter().map(|edit| (edit.start, edit.length)).collect();
                if index == 0 {
                    ranges.extend(uses.iter().map(|usage| (usage.start, usage.length)));
                    ranges.sort_unstable_by_key(|range| range.0);
                    if ranges.len() > MAX_EDITS_PER_LANE {
                        return Err("linked V1 exceeds the edit count limit".into());
                    }
                }
                let mut end = if picture_only {
                    ranges.first().map_or(0, |range| range.0)
                } else {
                    0
                };
                for (start, length) in ranges {
                    if start != end {
                        return Err("linked project requires contiguous nonoverlapping cuts and nests".into());
                    }
                    end = end.checked_add(length).ok_or("record range overflow")?;
                }
                if let Some(solid) = &solid {
                    if solid.start != end || end == 0 {
                        return Err("Solid Color must follow the linked picture span".into());
                    }
                    end = end.checked_add(solid.duration).ok_or("Solid Color range overflow")?;
                }
                if end == 0 || duration.is_some_and(|v| v != end) {
                    return Err(
                        if mixed_stereo { "picture, stereo A1 and mono A2 must cover the same record extent" }
                        else if stereo { "picture and stereo A1 must cover the same record extent" }
                        else { "picture and two mono lanes must cover the same record extent" }.into(),
                    );
                }
                duration = Some(end);
            }
            for fade in native_dissolves.iter().chain(&source_dissolves)
                .map(|value| &value.prepared).filter(|fade| fade.audio())
            {
                let adjacent = lanes[1].windows(2).find(|pair| pair[1].start == fade.cut);
                let Some([left, right]) = adjacent else {
                    return Err(format!("{}: missing prepared A1 crossfade endpoints", fade.id).into());
                };
                let (Profile::Wav { samples, .. }, Profile::Wav { .. }) =
                    (&media[left.media].profile, &media[right.media].profile)
                else {
                    return Err(format!("{}: A1 crossfade requires two verified WAV channels", fade.id).into());
                };
                if left.envelope.is_some() || right.envelope.is_some() {
                    return Err(format!("{}: A1 crossfade with a clip envelope requires a combined mapping", fade.id).into());
                }
                let playable_frames = samples / samples_per_frame(sequence.rate)?;
                if left.source_in.checked_add(left.length)
                    .and_then(|end| end.checked_add(fade.after()))
                    .is_none_or(|end| end > playable_frames)
                    || right.source_in < fade.before
                {
                    return Err(format!("{}: A1 crossfade exceeds physical source handles", fade.id).into());
                }
            }
            if picture_only && (!lanes[1].is_empty() || !lanes[2].is_empty()) {
                return Err("picture-only timeline contains audio placements".into());
            }
            let length = duration.ok_or("empty linked project timeline")?;
            if nest_member && !crate::nests::matches_extent(graph, sequence, origin, length) {
                return Err("prepared nesting extent disagrees with the saved sequence".into());
            }
            if second_picture {
                if lanes[3].is_empty() {
                    return Err("the linked V2 profile requires at least one picture edit".into());
                }
                lanes[3].sort_by_key(|edit| edit.start);
                let mut end = 0;
                for edit in &lanes[3] {
                    if edit.start < end {
                        return Err("linked V2 edits overlap on their own track".into());
                    }
                    end = edit.start.checked_add(edit.length)
                        .filter(|end| *end <= length).ok_or("linked V2 exceeds the V1 extent")?;
                }
            }
            if origin.checked_add(length).is_none_or(|end| end >= DAY) {
                return Err("timeline extends beyond the supported day".into());
            }
            use crate::timeline_marks::TimelineMarks;
            let native_marks = TimelineMarks::prepare(graph, &sequence.id, length)?;
            let premiere_marks = TimelineMarks::from_premiere(graph, sequence, length)?;
            if native_marks.is_some() && premiere_marks.is_some() {
                return Err("both Resolve and Premiere timeline ranges claim this sequence".into());
            }
            let marks = native_marks.or(premiere_marks);
            if compound && marks.is_some() {
                return Err("Resolve timeline marks require a registered timeline".into());
            }
            if premiere_marks.is_some() {
                recovered_premiere_marks.insert(sequence.id.clone());
            }
            if let Some(loss) = graph.losses.iter().find(|l| {
                if track_state.is_some() && crate::track_state::TrackState::recover(l, &sequence.id) {
                    return false;
                }
                if marks.is_some() && crate::timeline_marks::TimelineMarks::recover(l, &sequence.id) {
                    return false;
                }
                if premiere_marks.is_some() && l.object == sequence.id
                    && l.aspect == crate::premiere::TIMELINE_MARK_LOSS
                {
                    return false;
                }
                if clip_envelopes.recovers(l) {
                    return false;
                }
                if recovered_labels.contains(l.object.as_str()) && ClipLabel::recover(l) {
                    return false;
                }
                if l.aspect == crate::grade::AVID_RANGE_LOSS && recovered_avid_ranges.contains(l.object.as_str()) {
                    return false;
                }
                if l.aspect == "premiere.appearance" && l.reason == crate::premiere::color_mode::ADJUST_LOSS
                    && recovered_premiere_adjusts.contains(&l.object)
                {
                    return false;
                }
                if mixer.is_some() && mixer::DirectMixer::recover(l, &sequence.id) {
                    return false;
                }
                if faders.recover(l, &sequence.id) {
                    return false;
                }
                if native_dissolves.iter().any(|d| d.prepared.recover(l, &sequence.id)) {
                    return false;
                }
                if crate::nests::recover(l, &sequence.id, uses)
                    || crate::nests::recover_empty_stereo(graph, l, &sequence.id)
                {
                    return false;
                }
                if review_caption_omissions.is_some() && crate::turnover::is_reviewable_loss(l) {
                    return false;
                }
                if linear_resizes.contains(l.object.as_str())
                    && crate::video::motion::is_neutral_resize_background(l)
                {
                    return false;
                }
                l.object == "project"
                    || l.object == sequence.id
                    || scoped_media.contains(l.object.as_str())
                    || placement_ids.contains(l.object.as_str())
                    || l.object.match_indices('/').any(|(index, _)| {
                        scoped_media.contains(&l.object[..index])
                            || placement_ids.contains(&l.object[..index])
                    })
            }) {
                return Err(format!(
                    "source loss on {} ({}): {}",
                    loss.object, loss.aspect, loss.reason
                )
                .into());
            }
            let (markers, omitted) = marker_preparation.finish(length, &mut destination_losses)?;
            let mut captions = Vec::new();
            let mut caption_track = None;
            for text in &sequence.texts {
                match crate::captions::prepare(text, sequence.rate, length) {
                    Ok(cue) => {
                        // Avid reports the V-track index of the SubCap effect.
                        // Resolve owns subtitles on an independent track.
                        if caption_track.replace(text.track).is_some_and(|track| track != text.track) {
                            return Err(format!("{}: Resolve supports one source caption track", text.id).into());
                        }
                        captions.push(cue);
                    }
                    Err(_) if reviewable_captions.contains(text.id.as_str()) => {}
                    Err(error) => return Err(error),
                }
            }
            captions.sort_by_key(|cue| cue.start);
            if let Some(pair) = captions.windows(2).find(|pair| pair[0].start + pair[0].length > pair[1].start) {
                return Err(format!("overlapping Resolve captions {} and {}", pair[0].id, pair[1].id).into());
            }
            Ok((
                Timeline {
                    bin,
                    name: checked_name(&sequence.name)?,
                    origin,
                    length,
                    lanes,
                    stereo,
                    markers,
                    marks,
                    track_state,
                    group: None,
                    mixer,
                    faders,
                    dissolves: native_dissolves.into_iter().chain(source_dissolves)
                        .map(|transition| transition.prepared).collect(),
                    captions,
                    solid,
                    compound,
                    nests: uses.clone(),
                },
                omitted,
            ))
        };
        let (timeline, omitted) = build().map_err(|e| format!("timeline {}: {e}", row.id))?;
        timelines.push(timeline);
        omitted_markers.extend(omitted);
        project_rate = Some(sequence.rate);
        project_canvas = Some(row.canvas);
    }
    let project_rate = project_rate.ok_or("missing project rate")?;
    let mut pool_markers = HashMap::new();
    for item_id in spec
        .media
        .iter()
        .map(|binding| binding.item_id.as_str())
        .chain(atom_ids.iter().map(String::as_str))
    {
        let item = items[item_id];
        let index = selected[item_id].0;
        let profile = &media[index].profile;
        let (prepared, omitted) = match profile.pool_marker_grid(project_rate)? {
            Some((rate, frames)) if media[index].proxy.is_none()
                && (item.rate == Some(rate) || item.rate.is_none() && matches!(profile, Profile::Wav { .. })) =>
            {
                let (prepared, omitted) = crate::prepare::prepare_pool_markers(graph, item, rate, frames)?;
                for marker in &item.markers {
                    let Some((color, target)) = marker.color.as_ref().filter(|c| c.palette == "premiere")
                        .and_then(|c| crate::marker_palette::premiere_substitute(&c.name).map(|target| (c, target)))
                    else { continue };
                    let Some(frame) = marker.start.and_then(|start| frames_on(start, rate).ok()) else { continue };
                    if prepared.binary_search_by_key(&(frame as u64), |m| m.frame).is_ok() {
                        destination_losses.push(graph::Loss {
                            object: marker.owner_occurrence.clone(),
                            aspect: "premiere.marker".into(),
                            reason: format!("Premiere source marker color {} maps to Resolve {target}", color.name),
                        });
                    }
                }
                (prepared, omitted)
            }
            _ => (Vec::new(), item.markers.iter().map(|m| m.owner_occurrence.clone()).collect()),
        };
        if !prepared.is_empty() {
            pool_markers.insert(index, prepared);
        }
        omitted_markers.extend(omitted);
    }
    let mut selections = graph
        .bins
        .iter()
        .filter(|bin| bin.parent.is_none())
        .filter_map(|bin| sidecars.get(crate::source::ACTIVE_TIMELINE_ADAPTER, &bin.id));
    let active = match (selections.next(), selections.next()) {
        (Some(receipt), None) => match receipt.get("sequence_id") {
            Some(Value::Str(id))
                if receipt.get("unavailable_reason") == Some(&Value::Null)
                    && graph.sequences.iter().any(|sequence| sequence.id == *id) =>
            {
                Some(id)
            }
            _ => None,
        },
        _ => None,
    };
    for usage in timelines.iter().flat_map(|row| &row.nests) {
        let child = &timelines[usage.child];
        if child.origin != usage.origin || child.length != usage.child_length
            || child.compound != (usage.kind == crate::nests::Kind::Compound)
        {
            return Err(format!("Resolve nest {}: prepared child extent disagrees with the saved trim", usage.id).into());
        }
    }
    let selected = active.and_then(|id| spec.timelines.iter().zip(&timelines)
        .filter(|(_, timeline)| !timeline.compound)
        .position(|(row, _)| row.id == *id));
    Ok(PreparedProject {
        name: checked_name(&graph.project.name)?,
        rate: project_rate,
        canvas: project_canvas.ok_or("missing project canvas")?,
        bins,
        media,
        pool_markers,
        timelines,
        active_timeline_index: selected.unwrap_or(0),
        omitted_active_timeline: active.filter(|_| selected.is_none()).cloned(),
        omitted_markers,
        destination_losses,
        recovered_premiere_marks,
        recovered_premiere_adjusts,
    })
}

pub(crate) fn display_base(rate: Rational) -> u16 {
    match (rate.num(), rate.den()) {
        (25, 1) => 25,
        (30, 1) | (30000, 1001) => 30,
        _ => 24,
    }
}

pub(crate) fn samples_per_frame(rate: Rational) -> Result<i64> {
    whole_sample_count(1, rate)
}

fn wav_playable_frames(samples: i64, rate: Rational) -> Result<i64> {
    if !is_2997(rate) {
        return Ok(samples / samples_per_frame(rate)?);
    }
    // Use the WAV's sample extent because one 29.97 frame spans 1601.6 samples.
    let frames = i128::from(samples) * i128::from(rate.num()) / (48_000 * i128::from(rate.den()));
    Ok(frames.try_into()?)
}

pub(crate) fn wav_timemap_endpoint(samples: i64, rate: Rational) -> Result<f64> {
    if is_2997(rate) {
        return Ok(samples as f64 / 48_000.0 - rate.den() as f64 / rate.num() as f64);
    }
    Ok((samples - samples_per_frame(rate)?) as f64 / 48_000.0)
}

fn whole_sample_count(frame_count: i64, rate: Rational) -> Result<i64> {
    let samples = sample_count(frame_count, rate)?;
    if samples.den() != 1 {
        return Err("Resolve profile requires a whole 48 kHz sample count per frame".into());
    }
    Ok(samples.num())
}

pub(crate) fn movie_audio_sample_extent(frames: i64, rate: Rational) -> Result<i64> {
    // The picture duration can end between audio samples. Only complete
    // samples belong to its playable audio extent.
    let samples = sample_count(frames, rate)?;
    Ok(samples.num() / samples.den())
}

fn sample_count(frames: i64, rate: Rational) -> Result<Rational> {
    let numerator = (48_000 * i128::from(rate.den())).checked_mul(i128::from(frames))
        .ok_or("Resolve audio sample count overflow")?;
    Rational::new(numerator, rate.num().into())
        .filter(|samples| samples.num() >= 0 && rate.num() > 0)
        .ok_or_else(|| "invalid Resolve audio sample count".into())
}

fn check_premiere_audio_record_grid(origin: i64, start: i64, rate: Rational) -> Result<()> {
    // The two editors agreed across all source trim phases with a
    // whole-sample origin and whole-sample record starts.
    for (label, frame, relative_to) in [
        ("Timeline origin", origin, "timecode zero"),
        ("Audio record start", start, "the timeline origin"),
    ] {
        let samples = sample_count(frame, rate)?;
        if samples.den() != 1 {
            let whole = samples.num() / samples.den();
            let remainder = samples.num() % samples.den();
            return Err(format!(
                "{label} at frame {frame} falls between 48 kHz samples ({whole} + {remainder}/{} samples from {relative_to}). Premiere to Resolve needs both positions to align to whole samples for this edit.",
                samples.den(),
            ).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_audio_frame_grid_requires_a_whole_sample_count() {
        for (rate, count) in [
            (Rational::new(24000, 1001).unwrap(), 2002),
            (Rational::integer(24), 2000),
            (Rational::integer(25), 1920),
        ] {
            assert_eq!(samples_per_frame(rate).unwrap(), count);
        }
        let rate = Rational::new(30000, 1001).unwrap();
        assert!(samples_per_frame(rate).is_err());
        assert_eq!(whole_sample_count(5, rate).unwrap(), 8008);
        assert_eq!(whole_sample_count(40, rate).unwrap(), 64064);
        assert_eq!(movie_audio_sample_extent(48, rate).unwrap(), 76876);
    }

    #[test]
    fn ntsc_mono_wav_uses_its_sample_extent() {
        let rate = Rational::new(30000, 1001).unwrap();
        assert_eq!(wav_playable_frames(8_007, rate).unwrap(), 4);
        assert_eq!(wav_playable_frames(8_008, rate).unwrap(), 5);
        assert_eq!(wav_timemap_endpoint(2_304_000, rate).unwrap(), 47.966633333333334);
    }

    #[test]
    fn premiere_fractional_audio_reports_the_off_sample_record_position() {
        let rate = Rational::new(30000, 1001).unwrap();
        assert!(check_premiere_audio_record_grid(1790, 20, rate).is_ok());
        let origin = check_premiere_audio_record_grid(1791, 5, rate).unwrap_err().to_string();
        assert!(origin.starts_with("Timeline origin at frame 1791"));
        assert!(origin.contains("2868465 + 3/5 samples from timecode zero"));
        let cut = check_premiere_audio_record_grid(1790, 13, rate).unwrap_err().to_string();
        assert!(cut.starts_with("Audio record start at frame 13"));
        assert!(cut.contains("20820 + 4/5 samples from the timeline origin"));
    }

    #[test]
    fn stereo_wav_markers_use_complete_frames_on_the_import_clock() {
        let rate = Rational::new(24000, 1001).unwrap();
        for bits in [16, 24] {
            let wav = Profile::Wav { samples: 96_000, bits, channels: 2 };
            assert_eq!(wav.pool_marker_grid(rate).unwrap(), Some((rate, 47)));
            let integer_rate = Rational::integer(24);
            assert_eq!(wav.pool_marker_grid(integer_rate).unwrap(), Some((integer_rate, 48)));
        }
    }
}
