//! Preparation for the bounded, linked-media project qualification harness.
pub(crate) mod mixer;

use crate::{
    atom::{verify_atom_binding, AtomBindingSpec},
    audio::{self, Envelope},
    clip_label::ClipLabel,
    dissolve,
    group::prepare_bound_group,
    media::{media_digest, probe_file, StreamSelector},
    prepare::{
        check_binding, check_linked_placement, checked_name, frames_on, prepare_markers, rate,
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
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    pub(crate) root_bin_id: Option<String>,
    pub(crate) timelines: Vec<TimelineSpec>,
    pub(crate) media: Vec<MediaSpec>,
    #[serde(default)]
    pub(crate) atom_media: Vec<AtomSpec>,
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

pub(crate) enum Profile {
    Mov {
        rate: Rational,
        frames: i64,
        tagged_rec709: bool,
        quicktime_timecode: bool,
        h264: bool,
        aac: bool,
    },
    Wav {
        samples: i64,
        bits: i64,
        channels: i64,
    },
    Atom(Arc<VerifiedBinding>),
}

pub(crate) struct Media {
    pub(crate) name: String,
    pub(crate) bin: usize,
    pub(crate) path: PathBuf,
    pub(crate) profile: Profile,
    pub(crate) modified_micros: u64,
    digest: [u8; 32],
}

impl Media {
    pub(crate) fn recheck(&self) -> Result<()> {
        if let Profile::Atom(binding) = &self.profile {
            return binding.recheck();
        }
        if media_digest(&self.path)? != self.digest {
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
    pub(crate) envelope: Option<Envelope>,
    pub(crate) label: Option<ClipLabel>,
    pub(crate) speed: Option<crate::retime::PictureSpeed>,
    pub(crate) grade: Option<(f32, String)>,
}

pub(crate) struct Timeline {
    pub(crate) bin: usize,
    pub(crate) name: String,
    pub(crate) origin: i64,
    pub(crate) length: i64,
    // V1, A1, A2, V2, A3. The original three slots retain their archive ordering.
    pub(crate) lanes: [Vec<Edit>; 5],
    pub(crate) markers: Vec<PreparedMarker>,
    pub(crate) marks: Option<crate::timeline_marks::TimelineMarks>,
    pub(crate) track_state: Option<crate::track_state::TrackState>,
    pub(crate) group: Option<GroupTimeline>,
    pub(crate) mixer: Option<mixer::DirectMixer>,
    pub(crate) dissolve: Option<dissolve::Dissolve>,
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
    pub(crate) bins: Vec<Bin>,
    pub(crate) media: Vec<Media>,
    pub(crate) timelines: Vec<Timeline>,
    pub(crate) active_timeline_index: usize,
    omitted_active_timeline: Option<String>,
    pub(crate) omitted_markers: Vec<String>,
}

impl PreparedProject {
    pub(crate) fn writes_caption(&self, id: &str) -> bool {
        self.timelines.iter().flat_map(|timeline| &timeline.captions).any(|cue| cue.id == id)
    }

    pub fn omitted_active_timeline(&self) -> Option<&str> {
        self.omitted_active_timeline.as_deref()
    }

    pub fn omitted_markers(&self) -> &[String] {
        &self.omitted_markers
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
    Rational::new(
        i128::from(count) * i128::from(time_base.num()),
        i128::from(time_base.den()),
    )
    .ok_or_else(|| "physical media duration overflow".into())
}

pub(crate) fn probe(
    spec: &MediaSpec,
    tool: &Path,
    name: String,
    bin: usize,
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
    let digest: [u8; 32] = hex::decode(&spec.sha256)?
        .try_into()
        .map_err(|_| "invalid linked media digest")?;
    if media_digest(&path)? != digest {
        return Err("linked media does not match its verified digest".into());
    }
    let probe = probe_file(tool, &path, format)?;
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
        let h264 = stream["codec_name"] == "h264";
        let aac = h264 && streams.len() == 2 && streams[1]["codec_name"] == "aac";
        let zero_tc = |s: &Json| s["tags"]["timecode"] == "00:00:00:00";
        let quicktime_timecode = match streams.as_slice() {
            [_] | [_, _]
                if (streams.len() == 1 || aac)
                    && stream["tags"]["timecode"].is_null()
                    && probe["format"]["tags"]["timecode"].is_null() =>
            {
                false
            }
            [_, tc]
                if !h264
                    && rate == Rational::integer(24)
                    && tc["index"] == 1
                    && tc["codec_type"] == "data"
                    && tc["codec_tag_string"] == "tmcd"
                    && tc["nb_frames"] == "1"
                    && tc["start_pts"] == 0
                    && tc["duration_ts"] == duration_ts
                    && ratio(&tc["avg_frame_rate"]).ok() == Some(rate)
                    && ratio(&tc["time_base"]).ok() == Some(time_base)
                    && zero_tc(stream)
                    && zero_tc(tc)
                    && (probe["format"]["tags"]["timecode"].is_null()
                        || zero_tc(&probe["format"])) =>
            {
                true
            }
            _ => return Err("unqualified MOV auxiliary track or file timecode".into()),
        };
        let tagged_rec709 =
            stream["color_primaries"] == "bt709" && stream["color_transfer"] == "bt709";
        let unspecified_color = ["color_primaries", "color_transfer"]
            .iter()
            .all(|key| stream[*key].is_null() || stream[*key] == "unknown");
        let suffix = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        let brand = &probe["format"]["tags"]["major_brand"];
        let is_quicktime = suffix.eq_ignore_ascii_case("mov") && brand == "qt  ";
        let is_mp4 = suffix.eq_ignore_ascii_case("mp4")
            && matches!(brand.as_str(), Some("isom" | "mp41" | "mp42"));
        let absent = |key: &str| stream[key].is_null() || stream[key] == "unknown";
        let video_profile = if h264 {
            (is_quicktime || is_mp4)
                && rate == Rational::integer(24)
                && stream["profile"] == "High"
                && stream["level"] == 40
                && stream["codec_tag_string"] == "avc1"
                && stream["pix_fmt"] == "yuv420p"
                && ((unspecified_color && absent("color_space") && absent("color_range"))
                    || (tagged_rec709
                        && stream["color_space"] == "bt709"
                        && stream["color_range"] == "tv"))
                && stream["side_data_list"].is_null()
                && stream["tags"]["rotate"].is_null()
        } else {
            is_quicktime
                && stream["codec_name"] == "prores"
                && stream["profile"] == "HQ"
                && stream["codec_tag_string"] == "apch"
                && stream["pix_fmt"] == "yuv422p10le"
                && stream["color_range"] == "tv"
                && stream["color_space"] == "bt709"
                && (tagged_rec709 || unspecified_color)
        };
        if !supported_rate(rate)
            || frames <= 0
            || frames > i64::from(i32::MAX)
            || stream["codec_type"] != "video"
            || !video_profile
            || stream["width"] != 1920
            || stream["height"] != 1080
            || stream["field_order"] != "progressive"
            || stream["sample_aspect_ratio"] != "1:1"
            || stream["start_pts"] != 0
            || stream["nb_read_packets"]
                .as_str()
                .and_then(|n| n.parse().ok())
                != Some(frames)
            || ratio(&stream["avg_frame_rate"]).ok() != Some(rate)
        {
            return Err("unqualified linked AVC or ProRes HQ picture profile".into());
        }
        if aac {
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
                || integer(audio, "duration_ts")? != frames * 2000
            {
                return Err("unqualified AVC movie AAC layout or playable sample extent".into());
            }
            crate::media::verify_aac_packet_grid(tool, &path, frames * 2000)?;
        }
        if duration_ts % frames != 0 {
            return Err("MOV timescale cannot represent one constant frame".into());
        }
        crate::media::verify_mov_packet_grid(tool, &path, frames, duration_ts / frames, h264)?;
        Profile::Mov {
            rate,
            frames,
            tagged_rec709,
            quicktime_timecode,
            h264,
            aac,
        }
    } else {
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
    let modified_micros = fs::metadata(&path)?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_micros()
        .try_into()?;
    let binding = Media {
        name,
        bin,
        path,
        profile,
        modified_micros,
        digest,
    };
    binding.recheck()?;
    Ok((binding, time_base, duration_ts))
}

fn supported_rate(rate: Rational) -> bool {
    matches!((rate.num(), rate.den()), (24, 1) | (24000, 1001))
}

pub(crate) fn check_stream(
    source: &SourceMedia,
    selector: &StreamSelector,
    media: &Media,
    time_base: Rational,
    duration_ts: i64,
) -> Result<()> {
    let (index, time_base, duration_ts) = match (&media.profile, source.kind) {
        (
            Profile::Mov {
                aac: true, frames, ..
            },
            StreamKind::Audio,
        ) => (1, Rational::new(1, 48000).unwrap(), frames * 2000),
        _ => (0, time_base, duration_ts),
    };
    let valid = match &media.profile {
        Profile::Mov { .. } if index == 1 => {
            selector
                .channel
                .is_some_and(|c| c < 2 && source.stream.is_some())
                && source.sample_rate == Some(48000)
                && source.bits.is_none()
                && source.codec.as_deref() == Some("aac")
        }
        Profile::Mov { rate, h264, .. } => {
            selector.channel.is_none()
                && source.kind == StreamKind::Video
                && source.rate == Some(*rate)
                && source.frame_size == Some((1920, 1080))
                && source.codec.as_deref().is_some_and(|c| {
                    if *h264 {
                        matches!(c, "avc1" | "h264")
                    } else {
                        matches!(c, "apch" | "prores")
                    }
                })
        }
        Profile::Wav { bits, channels, .. } => {
            selector
                .channel
                .is_some_and(|c| c < *channels as usize && source.stream == Some(c as i64 + 1))
                && source.kind == StreamKind::Audio
                && source.sample_rate == Some(48000)
                && source.bits == Some(*bits)
        }
        Profile::Atom(_) => false,
    };
    let path_agrees = source.locations.iter().any(|s| Path::new(s) == media.path);
    if selector.index != index
        || !valid
        || !path_agrees
        || source.start.is_some_and(|v| v != Rational::ZERO)
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

fn check_origin(sequence: &SourceSequence) -> Result<i64> {
    let clock = sequence
        .clock
        .as_ref()
        .ok_or("qualified sequence origin required")?;
    let witness = &clock.provenance;
    if clock.rate != sequence.rate
        || clock.display_base != 24
        || clock.drop
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
        return Err("qualified exact non-drop timeline origin required".into());
    }
    Ok(i64::try_from(clock.effective_count)?)
}

pub fn prepare(graph: &SourceGraph, spec: ProjectSpec, ffprobe: &Path) -> Result<PreparedProject> {
    prepare_project(graph, spec, ffprobe, None)
}

pub(crate) fn prepare_project(
    graph: &SourceGraph,
    spec: ProjectSpec,
    ffprobe: &Path,
    review_caption_omissions: Option<&[graph::Loss]>,
) -> Result<PreparedProject> {
    graph.validate()?;
    if !(1..=64).contains(&spec.timelines.len())
        || !(1..=256).contains(&spec.media.len().saturating_add(spec.atom_media.len()))
        || graph.bins.len() > 256
    {
        return Err("linked project exceeds timeline, media or bin limits".into());
    }
    let nests = crate::nests::plan(graph, &spec.timelines)?;
    let nested_children: HashSet<_> = nests.iter().flatten().map(|usage| usage.child).collect();
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
    let mut selected = HashMap::new();
    for binding in &spec.media {
        let item_id = binding.item_id.as_str();
        if selected.contains_key(item_id) {
            return Err(format!("duplicate linked item binding: {item_id}").into());
        }
        let size = fs::metadata(&binding.path)
            .map_err(|error| format!("{item_id}: linked media metadata unavailable: {error}"))?
            .len();
        media_bytes = media_bytes
            .checked_add(size)
            .ok_or("linked media size overflows")?;
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
        let (verified, tb, duration_ts) = probe(
            binding,
            ffprobe,
            checked_name(&item.name)?,
            bin_ids[bin.id.as_str()],
        )
        .map_err(|e| format!("{item_id}: {e}"))?;
        let mut by_label = HashMap::new();
        let mut seen = HashSet::new();
        let expected = match &verified.profile {
            Profile::Mov { aac, .. } => {
                if *aac {
                    3
                } else {
                    1
                }
            }
            Profile::Wav { channels, .. } => *channels as usize,
            Profile::Atom(_) => return Err("unexpected split MXF in MOV/WAV probe".into()),
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
            if by_label.insert(source.stream, selector.channel).is_some() {
                return Err(format!("{item_id}: duplicate logical source stream label").into());
            }
        }
        selected.insert(item_id, (media.len(), by_label));
        media.push(verified);
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
        media.push(Media {
            name: checked_name(&item.name)?,
            bin: bin_ids[bin.id.as_str()],
            path: verified.path.clone(),
            modified_micros: verified.modified_micros,
            digest: verified.digest,
            profile: Profile::Atom(Arc::new(verified)),
        });
        atom_ids.push(item_id);
    }
    let mut timelines = Vec::new();
    let mut omitted_markers = Vec::new();
    let mut seen = HashSet::new();
    let mut project_rate = None;
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
        if let Profile::Atom(binding) = &verified.profile {
            scoped_media.extend(binding.streams.iter().map(|s| s.media_id.as_str()));
        }
    }
    for (row_index, row) in spec.timelines.iter().enumerate() {
        let sequence = graph
            .sequences
            .iter()
            .find(|s| s.id == row.id)
            .ok_or("selected timeline ID absent")?;
        let mut build = || -> Result<(Timeline, Vec<String>)> {
            if !seen.insert(row.id.as_str())
                || row.canvas != (1920, 1080)
                || sequence.frame_size.is_some_and(|size| size != (1920, 1080))
                || !supported_rate(sequence.rate)
                || project_rate.is_some_and(|rate| rate != sequence.rate)
                || sequence.texts.iter().any(|text| text.kind != graph::source::TextKind::Caption)
            {
                return Err("unqualified canvas, mixed project rates, duplicate selection or timeline topology".into());
            }
            let compound = crate::nests::compound(graph, &sequence.id);
            let uses = &nests[row_index];
            let nest_member = !uses.is_empty() || nested_children.contains(&row_index);
            let shape = |kind, index| sequence.tracks.iter().any(|t|
                t.kind == kind && t.index == index && t.level_db.is_none());
            if nest_member && (!shape(StreamKind::Video, 1)
                || !(sequence.tracks.len() == 1 && !compound
                    || sequence.tracks.len() == 3 && !compound
                        && shape(StreamKind::Audio, 1) && shape(StreamKind::Audio, 2)
                    || sequence.tracks.len() == 2 && compound && shape(StreamKind::Audio, 1))
                || sequence.placements.iter().any(|p| p.video_track != Some(1)
                    || !p.audio_tracks.is_empty() || !p.grouped_audio.is_empty())
                || !sequence.texts.is_empty() || !sequence.transitions.is_empty()
                || graph.sidecar(crate::dissolve::ADAPTER, &sequence.id).is_some()
                || graph.sidecar(crate::generator::ADAPTER, &sequence.id).is_some()
                || (compound && !sequence.markers.is_empty()))
            {
                return Err("Resolve nesting requires a picture-only V1 parent and children".into());
            }
            let second_picture = sequence.tracks.iter().any(|track|
                track.kind == StreamKind::Video && track.index == 2 && track.level_db.is_none());
            let track_count = sequence.tracks.len() - usize::from(second_picture);
            let picture_only = nest_member || track_count == 1;
            let third_mono = track_count == 4 && second_picture && sequence.rate == Rational::integer(24);
            if (!picture_only && track_count != 3 && !third_mono)
                || [
                    (StreamKind::Video, 1),
                    (StreamKind::Audio, 1),
                    (StreamKind::Audio, 2),
                ]
                .into_iter()
                .take(if picture_only { 1 } else { 3 })
                .any(|(kind, index)| {
                    !sequence
                        .tracks
                        .iter()
                        .any(|t| t.kind == kind && t.index == index && t.level_db.is_none())
                })
                || (third_mono && !sequence.tracks.iter().any(|track|
                    track.kind == StreamKind::Audio && track.index == 3 && track.level_db.is_none()))
            {
                return Err("expected a qualified Resolve picture and independent mono track layout".into());
            }
            let bin = *bin_ids
                .get(row.bin_id.as_str())
                .ok_or("timeline bin assignment absent")?;
            if let Some(Value::Str(saved)) = graph
                .sidecar("resolve.source", &sequence.id)
                .and_then(|data| data.get("bin_id"))
            {
                if saved != &row.bin_id {
                    return Err("explicit timeline bin differs from saved owning bin".into());
                }
            }
            let origin = check_origin(sequence)?;
            if let Some(group) = &row.group {
                if second_picture || !sequence.texts.is_empty() || !sequence.transitions.is_empty()
                    || graph.sidecar(crate::generator::ADAPTER, &sequence.id).is_some()
                {
                    return Err("subtitle tracks and transitions on Group timelines are unqualified".into());
                }
                if group_written || sequence.rate != rate() {
                    return Err("project supports one same-rate split MXF Group timeline".into());
                }
                let group_bin = *bin_ids
                    .get(group.bin_id.as_str())
                    .ok_or("Group bin absent")?;
                let resolve_camera = |id: &str| -> Result<(usize, Arc<VerifiedBinding>)> {
                    let index = selected
                        .get(id)
                        .ok_or("no verified Group camera binding")?
                        .0;
                    let Profile::Atom(binding) = &media[index].profile else {
                        return Err(
                            "project Group cameras require verified split MXF bindings".into()
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
                        dissolve: None,
                        captions: vec![],
                        solid: None,
                        compound: false,
                        nests: vec![],
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
            let track_state = crate::track_state::TrackState::prepare_for_resolve(graph, sequence)?;
            if track_state.is_some() && (picture_only || compound || second_picture || third_mono) {
                return Err("Resolve track names and locks require a registered V1/A1/A2 timeline".into());
            }
            let dissolve = dissolve::prepare(graph, sequence)?;
            let source_dissolve = dissolve::from_source(sequence)?;
            let solid = crate::generator::prepare(graph, sequence)?;
            if solid.is_some() && !picture_only {
                return Err("Solid Color requires a picture-only linked project timeline".into());
            }
            if dissolve.is_some() && source_dissolve.is_some() {
                return Err("conflicting Resolve and cross-editor transition facts".into());
            }
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
            let mut recovered_audio = HashMap::new();
            let mut recovered_labels = HashSet::new();
            let mut lanes: [Vec<Edit>; 5] = Default::default();
            for edit in &sequence.placements {
                check_linked_placement(edit).map_err(|e| format!("{}: {e}", edit.id))?;
                let (media_index, selectors) = selected
                    .get(edit.item.as_str())
                    .ok_or_else(|| format!("no verified file binding for item {}", edit.item))?;
                let (lane, channel) = match (
                    edit.video_track,
                    edit.audio_tracks.as_slice(),
                    edit.audio_streams.as_slice(),
                    &media[*media_index].profile,
                ) {
                    (Some(1), [], [], Profile::Mov { rate, .. }) if *rate == sequence.rate => {
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
                            rate, aac: true, ..
                        },
                    ) if *rate == sequence.rate => (
                        *lane as usize,
                        selectors.get(&Some(*label)).copied().flatten(),
                    ),
                    (Some(1), [], [], Profile::Atom(_)) if sequence.rate == rate() => (0, None),
                    (None, [lane @ (1 | 2)], [label @ (1 | 2)], Profile::Atom(_))
                        if sequence.rate == rate() && lane == label =>
                    {
                        (*lane as usize, Some((*label - 1) as usize))
                    }
                    _ => {
                        return Err(
                            format!("{}: unsupported mixed rate or lane routing", edit.id).into(),
                        )
                    }
                };
                if (matches!(lane, 1 | 2 | 4) && channel.is_none()) || lanes[lane].len() == MAX_EDITS_PER_LANE {
                    return Err(format!(
                        "{}: unresolved physical channel or edit count limit",
                        edit.id
                    )
                    .into());
                }
                let mut start = frames_on(edit.record_start, sequence.rate)?;
                let mut source_in = frames_on(edit.source_in, sequence.rate)?;
                let mut length = frames_on(edit.duration, sequence.rate)?;
                if let Some(dissolve) = source_dissolve.as_ref().filter(|dissolve| dissolve.incoming == edit.id) {
                    let preroll = dissolve.prepared.before;
                    start = start.checked_add(preroll).ok_or("dissolve start overflow")?;
                    source_in = source_in.checked_add(preroll).ok_or("dissolve source in overflow")?;
                    length -= preroll;
                }
                if let Some(dissolve) = source_dissolve.as_ref().filter(|dissolve| dissolve.outgoing == edit.id) {
                    length -= crate::dissolve::DURATION - dissolve.prepared.before;
                }
                let speed = crate::retime::PictureSpeed::prepare(edit, sequence.rate, length)
                    .map_err(|e| format!("{}: {e}", edit.id))?;
                let source_length = speed.map(|speed| speed.source_length(length)).transpose()?.unwrap_or(length);
                let end = source_in
                    .checked_add(source_length)
                    .ok_or("source range overflow")?;
                let available = match &media[*media_index].profile {
                    Profile::Mov { frames, .. } => *frames,
                    Profile::Wav { samples, .. } => samples / samples_per_frame(sequence.rate),
                    Profile::Atom(binding) => binding.frames,
                };
                if length <= 0 || end > available {
                    return Err(
                        format!("{}: edit exceeds the physical playable range", edit.id).into(),
                    );
                }
                let envelope = if matches!(lane, 1 | 2 | 4) {
                    if graph.sidecar(audio::UNQUALIFIED_FADE_ADAPTER, &edit.id).is_some() {
                        return Err(format!("{}: unqualified Resolve clip fade points", edit.id).into());
                    }
                    let receipt = graph.sidecar(crate::source::ADAPTER, &edit.id);
                    if (receipt.is_some() || edit.volume_db.is_some() || !edit.volume_keys.is_empty())
                        && !matches!(&media[*media_index].profile, Profile::Wav { .. })
                    {
                        return Err(format!("{}: clip envelopes require a verified WAV source", edit.id).into());
                    }
                    if (receipt.is_some() || !edit.volume_keys.is_empty())
                        && (sequence.rate.num(), sequence.rate.den()) != (24, 1)
                    {
                        return Err(format!("{}: linked fades and volume keys require 24 fps", edit.id).into());
                    }
                    let envelope = audio::prepare(edit, source_in, length, sequence.rate, receipt)?;
                    if let Some(loss) = envelope.and_then(Envelope::recovered_loss) {
                        recovered_audio.insert(edit.id.as_str(), loss);
                    }
                    envelope.filter(|control| *control != Envelope::static_volume(0.0))
                } else {
                    None
                };
                let video = crate::video::prepare(edit, sequence.rate, source_in, length, available)
                    .map_err(|e| format!("{}: {e}", edit.id))?;
                let grade = crate::grade::prepare(graph, &edit.id)?;
                let label = ClipLabel::prepare(graph, &edit.id)?;
                if label.is_some() {
                    recovered_labels.insert(edit.id.as_str());
                }
                if grade.is_some() && lane != 0 {
                    return Err("Resolve primary gain must belong to a V1 picture edit".into());
                }
                lanes[lane].push(Edit {
                    media: *media_index,
                    channel,
                    start,
                    source_in,
                    length,
                    video,
                    envelope,
                    label,
                    speed,
                    grade,
                });
            }
            let mut duration = None;
            let base_lanes: &[usize] = if picture_only { &[0] } else if third_mono { &[0, 1, 2, 4] } else { &[0, 1, 2] };
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
                        "picture and two mono lanes must cover the same record extent".into(),
                    );
                }
                duration = Some(end);
            }
            if let Some(fade) = dissolve.as_ref().filter(|fade| fade.audio) {
                let adjacent = lanes[1].windows(2).find(|pair| pair[1].start == fade.cut);
                let Some([left, right]) = adjacent else {
                    return Err("missing prepared A1 crossfade endpoints".into());
                };
                let (Profile::Wav { samples, .. }, Profile::Wav { .. }) =
                    (&media[left.media].profile, &media[right.media].profile)
                else {
                    return Err("A1 crossfade requires two verified WAV channels".into());
                };
                if left.source_in.checked_add(left.length)
                    .and_then(|end| end.checked_add(dissolve::DURATION - fade.before))
                    .is_none_or(|end| end > samples / samples_per_frame(sequence.rate))
                    || right.source_in < fade.before
                {
                    return Err("A1 crossfade exceeds physical source handles".into());
                }
            }
            if picture_only && (!lanes[1].is_empty() || !lanes[2].is_empty()) {
                return Err("picture-only timeline contains audio placements".into());
            }
            let length = duration.ok_or("empty linked project timeline")?;
            if nest_member {
                let receipt = graph.sidecar(crate::source::ADAPTER, &sequence.id)
                    .ok_or("missing saved Resolve nesting extent")?;
                if crate::nests::int(receipt, "record_length") != Some(length)
                    || crate::nests::int(receipt, "record_origin") != Some(origin)
                {
                    return Err("prepared nesting extent disagrees with the saved Resolve sequence".into());
                }
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
            let marks = crate::timeline_marks::TimelineMarks::prepare(graph, &sequence.id, length)?;
            if compound && marks.is_some() {
                return Err("Resolve timeline marks require a registered timeline".into());
            }
            if let Some(loss) = graph.losses.iter().find(|l| {
                if track_state.is_some() && crate::track_state::TrackState::recover(l, &sequence.id) {
                    return false;
                }
                if marks.is_some() && crate::timeline_marks::TimelineMarks::recover(l, &sequence.id) {
                    return false;
                }
                if recovered_audio
                    .get(l.object.as_str())
                    .is_some_and(|(aspect, reason)| l.aspect == *aspect && l.reason == *reason)
                {
                    return false;
                }
                if recovered_labels.contains(l.object.as_str()) && ClipLabel::recover(l) {
                    return false;
                }
                if mixer.is_some() && mixer::DirectMixer::recover(l, &sequence.id) {
                    return false;
                }
                if dissolve.as_ref().is_some_and(|d| d.recover(l, &sequence.id)) {
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
            let (markers, omitted) = prepare_markers(graph, sequence, length)?;
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
                    markers,
                    marks,
                    track_state,
                    group: None,
                    mixer,
                    dissolve: dissolve.or_else(|| source_dissolve.map(|value| value.prepared)),
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
    }
    for item_id in spec
        .media
        .iter()
        .map(|binding| binding.item_id.as_str())
        .chain(atom_ids.iter().map(String::as_str))
    {
        omitted_markers.extend(
            items[item_id]
                .markers
                .iter()
                .map(|m| m.owner_occurrence.clone()),
        );
    }
    let mut selections = graph
        .bins
        .iter()
        .filter(|bin| bin.parent.is_none())
        .filter_map(|bin| graph.sidecar(crate::source::ACTIVE_TIMELINE_ADAPTER, &bin.id));
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
        rate: project_rate.ok_or("missing project rate")?,
        bins,
        media,
        timelines,
        active_timeline_index: selected.unwrap_or(0),
        omitted_active_timeline: active.filter(|_| selected.is_none()).cloned(),
        omitted_markers,
    })
}

pub(crate) fn samples_per_frame(rate: Rational) -> i64 {
    if rate.den() == 1 {
        2000
    } else {
        2002
    }
}
