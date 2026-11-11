//! Preparation for the bounded, linked-media project qualification harness.
use crate::{
    media::{media_digest, probe_file, StreamSelector},
    prepare::{
        check_plain, checked_name, frames_on, prepare_markers, utf8_path, PreparedMarker,
        MAX_EDITS_PER_LANE,
    },
    Result, DAY,
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
    time::UNIX_EPOCH,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    root_bin_id: Option<String>,
    timelines: Vec<TimelineSpec>,
    media: Vec<MediaSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineSpec {
    id: String,
    bin_id: String,
    canvas: (u16, u16),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaSpec {
    item_id: String,
    bin_id: String,
    path: PathBuf,
    sha256: String,
    streams: Vec<StreamSelector>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
    Mov {
        rate: Rational,
        frames: i64,
        tagged_rec709: bool,
        quicktime_timecode: bool,
    },
    Wav {
        samples: i64,
        bits: i64,
        channels: i64,
    },
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
}

pub(crate) struct Timeline {
    pub(crate) bin: usize,
    pub(crate) name: String,
    pub(crate) origin: i64,
    pub(crate) length: i64,
    pub(crate) lanes: [Vec<Edit>; 3],
    pub(crate) markers: Vec<PreparedMarker>,
}

pub struct PreparedProject {
    pub(crate) name: String,
    pub(crate) rate: Rational,
    pub(crate) bins: Vec<Bin>,
    pub(crate) media: Vec<Media>,
    pub(crate) timelines: Vec<Timeline>,
    pub(crate) omitted_markers: Vec<String>,
}

impl PreparedProject {
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

fn extent(count: i64, time_base: Rational) -> Result<Rational> {
    Rational::new(
        i128::from(count) * i128::from(time_base.num()),
        i128::from(time_base.den()),
    )
    .ok_or_else(|| "physical media duration overflow".into())
}

fn probe(
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
        Some(ext) if ext.eq_ignore_ascii_case("mov") => "mov",
        Some(ext) if ext.eq_ignore_ascii_case("wav") => "wav",
        _ => return Err("expected an original MOV or WAV file".into()),
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
        let zero_tc = |s: &Json| s["tags"]["timecode"] == "00:00:00:00";
        let quicktime_timecode = match streams.as_slice() {
            [_] if stream["tags"]["timecode"].is_null()
                && probe["format"]["tags"]["timecode"].is_null() =>
            {
                false
            }
            [_, tc]
                if rate == Rational::integer(24)
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
        if !supported_rate(rate)
            || frames <= 0
            || frames > i64::from(i32::MAX)
            || stream["codec_type"] != "video"
            || stream["codec_name"] != "prores"
            || stream["profile"] != "HQ"
            || stream["codec_tag_string"] != "apch"
            || stream["width"] != 1920
            || stream["height"] != 1080
            || stream["pix_fmt"] != "yuv422p10le"
            || stream["field_order"] != "progressive"
            || stream["sample_aspect_ratio"] != "1:1"
            || stream["start_pts"] != 0
            || stream["color_range"] != "tv"
            || stream["color_space"] != "bt709"
            || !(tagged_rec709 || unspecified_color)
            || probe["format"]["tags"]["major_brand"] != "qt  "
            || stream["nb_read_packets"]
                .as_str()
                .and_then(|n| n.parse().ok())
                != Some(frames)
            || ratio(&stream["avg_frame_rate"]).ok() != Some(rate)
        {
            return Err("unqualified linked ProRes HQ MOV picture profile".into());
        }
        if duration_ts % frames != 0 {
            return Err("MOV timescale cannot represent one constant frame".into());
        }
        crate::media::verify_mov_packet_grid(tool, &path, frames, duration_ts / frames)?;
        Profile::Mov {
            rate,
            frames,
            tagged_rec709,
            quicktime_timecode,
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

fn check_stream(
    source: &SourceMedia,
    selector: &StreamSelector,
    media: &Media,
    time_base: Rational,
    duration_ts: i64,
) -> Result<()> {
    let valid = match media.profile {
        Profile::Mov { rate, .. } => {
            selector.channel.is_none()
                && source.kind == StreamKind::Video
                && source.rate == Some(rate)
                && source.frame_size == Some((1920, 1080))
                && source
                    .codec
                    .as_deref()
                    .is_some_and(|c| matches!(c, "apch" | "prores"))
        }
        Profile::Wav { bits, channels, .. } => {
            selector
                .channel
                .is_some_and(|c| c < channels as usize && source.stream == Some(c as i64 + 1))
                && source.kind == StreamKind::Audio
                && source.sample_rate == Some(48000)
                && source.bits == Some(bits)
        }
    };
    let path_agrees = source.locations.iter().any(|s| Path::new(s) == media.path);
    if selector.index != 0
        || !valid
        || !path_agrees
        || source.start.is_some_and(|v| v != Rational::ZERO)
        || source.duration != extent(duration_ts, time_base)?
    {
        return Err("graph media facts disagree with verified linked file".into());
    }
    if let Some(original) = &source.original {
        if original.stream_index != 0
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
            "Resolve archive clock" | "direct sequence TC1"
        )
        || witness.source_rate != sequence.rate
        || witness.origin_count != clock.effective_count
        || witness.source_offset != 0
        || (witness.route == "Resolve archive clock"
            && (witness.timecode_track != "Sm2Sequence.MediaExtents"
                || witness.source_package != sequence.id))
    {
        return Err("qualified exact non-drop timeline origin required".into());
    }
    Ok(i64::try_from(clock.effective_count)?)
}

pub fn prepare(graph: &SourceGraph, spec: ProjectSpec, ffprobe: &Path) -> Result<PreparedProject> {
    graph.validate()?;
    if !(1..=64).contains(&spec.timelines.len())
        || !(1..=256).contains(&spec.media.len())
        || graph.bins.len() > 256
    {
        return Err("linked project exceeds timeline, media or bin limits".into());
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
            || !(1..=2).contains(&binding.streams.len())
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
        let expected = match verified.profile {
            Profile::Mov { .. } => 1,
            Profile::Wav { channels, .. } => channels as usize,
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
            by_label.insert(source.stream, selector.channel);
        }
        selected.insert(item_id, (media.len(), by_label));
        media.push(verified);
    }
    let mut timelines = Vec::new();
    let mut omitted_markers = Vec::new();
    let mut seen = HashSet::new();
    let mut project_rate = None;
    let scoped_media: HashSet<_> = spec
        .media
        .iter()
        .flat_map(|m| {
            std::iter::once(m.item_id.as_str()).chain(m.streams.iter().map(|s| s.media_id.as_str()))
        })
        .collect();
    for row in &spec.timelines {
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
                || !sequence.transitions.is_empty()
                || !sequence.texts.is_empty()
                || graph
                    .multicam_uses
                    .iter()
                    .any(|m| m.sequence == sequence.id)
            {
                return Err("unqualified canvas, mixed project rates, duplicate selection or timeline topology".into());
            }
            if sequence.tracks.len() != 3
                || [
                    (StreamKind::Video, 1),
                    (StreamKind::Audio, 1),
                    (StreamKind::Audio, 2),
                ]
                .into_iter()
                .any(|(kind, index)| {
                    !sequence
                        .tracks
                        .iter()
                        .any(|t| t.kind == kind && t.index == index && t.level_db.is_none())
                })
            {
                return Err("expected V1 and two neutral independent mono tracks".into());
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
            let placement_ids: HashSet<_> =
                sequence.placements.iter().map(|p| p.id.as_str()).collect();
            if let Some(loss) = graph.losses.iter().find(|l| {
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
            let mut lanes: [Vec<Edit>; 3] = Default::default();
            for edit in &sequence.placements {
                check_plain(edit).map_err(|e| format!("{}: {e}", edit.id))?;
                let (media_index, selectors) = selected
                    .get(edit.item.as_str())
                    .ok_or_else(|| format!("no verified file binding for item {}", edit.item))?;
                let (lane, channel) = match (
                    edit.video_track,
                    edit.audio_tracks.as_slice(),
                    edit.audio_streams.as_slice(),
                    media[*media_index].profile,
                ) {
                    (Some(1), [], [], Profile::Mov { rate, .. }) if rate == sequence.rate => {
                        (0, None)
                    }
                    (None, [lane @ (1 | 2)], [label], Profile::Wav { .. }) => (
                        *lane as usize,
                        selectors.get(&Some(*label)).copied().flatten(),
                    ),
                    _ => {
                        return Err(
                            format!("{}: unsupported mixed rate or lane routing", edit.id).into(),
                        )
                    }
                };
                if (lane != 0 && channel.is_none()) || lanes[lane].len() == MAX_EDITS_PER_LANE {
                    return Err(format!(
                        "{}: unresolved physical channel or edit count limit",
                        edit.id
                    )
                    .into());
                }
                let start = frames_on(edit.record_start, sequence.rate)?;
                let source_in = frames_on(edit.source_in, sequence.rate)?;
                let length = frames_on(edit.duration, sequence.rate)?;
                let end = source_in
                    .checked_add(length)
                    .ok_or("source range overflow")?;
                let available = match media[*media_index].profile {
                    Profile::Mov { frames, .. } => frames,
                    Profile::Wav { samples, .. } => samples / samples_per_frame(sequence.rate),
                };
                if length <= 0 || end > available {
                    return Err(
                        format!("{}: edit exceeds the physical playable range", edit.id).into(),
                    );
                }
                lanes[lane].push(Edit {
                    media: *media_index,
                    channel,
                    start,
                    source_in,
                    length,
                });
            }
            let mut duration = None;
            for lane in &mut lanes {
                lane.sort_by_key(|e| e.start);
                let mut end = 0i64;
                for edit in lane {
                    if edit.start != end {
                        return Err("linked project requires contiguous nonoverlapping cuts".into());
                    }
                    end = end
                        .checked_add(edit.length)
                        .ok_or("record range overflow")?;
                }
                if end == 0 || duration.is_some_and(|v| v != end) {
                    return Err(
                        "picture and two mono lanes must cover the same record extent".into(),
                    );
                }
                duration = Some(end);
            }
            let length = duration.ok_or("empty linked project timeline")?;
            if origin.checked_add(length).is_none_or(|end| end >= DAY) {
                return Err("timeline extends beyond the supported day".into());
            }
            let (markers, omitted) = prepare_markers(sequence, length);
            Ok((
                Timeline {
                    bin,
                    name: checked_name(&sequence.name)?,
                    origin,
                    length,
                    lanes,
                    markers,
                },
                omitted,
            ))
        };
        let (timeline, omitted) = build().map_err(|e| format!("timeline {}: {e}", row.id))?;
        timelines.push(timeline);
        omitted_markers.extend(omitted);
        project_rate = Some(sequence.rate);
    }
    for binding in &spec.media {
        omitted_markers.extend(
            items[binding.item_id.as_str()]
                .markers
                .iter()
                .map(|m| m.owner_occurrence.clone()),
        );
    }
    Ok(PreparedProject {
        name: checked_name(&graph.project.name)?,
        rate: project_rate.ok_or("missing project rate")?,
        bins,
        media,
        timelines,
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
