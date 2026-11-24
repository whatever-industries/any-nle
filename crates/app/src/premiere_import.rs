//! Internal, standalone-first Premiere media import trial.
use crate::{output, premiere_preparation, source_input};
use graph::source::{SourceGraph, SourceMedia, VERIFIED_LINKED_MOV_TIMECODE};
use graph::{Rational, StreamKind, Value};
use prproj::build::time::{Time, TICKS_PER_SECOND};
use prproj::convert::media_donor::{Essence, ExpectedMedia, FileFacts, MediaRecords};
use prproj::convert::model::Options;
use prproj::RandomUuids;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) mod avid;
mod worker;
pub use worker::{status as worker_status, WorkerState};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Convert,
    Review,
    ReplayDonor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    graph: PathBuf,
    sequence: String,
    output: PathBuf,
    donor: Option<PathBuf>,
    ffprobe: PathBuf,
    #[serde(default)]
    original_sha256: BTreeMap<String, String>,
}

fn bounded_file(path: &Path, max: usize) -> Result<String> {
    let mut data = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(max as u64 + 1).read_to_end(&mut data))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() > max {
        return Err(format!("{}: input exceeds {max} bytes", path.display()));
    }
    String::from_utf8(data).map_err(|e| e.to_string())
}

fn select(graph: &mut SourceGraph, target: &str) -> Result<()> {
    let sequence = graph
        .sequences
        .iter()
        .find(|s| s.id == target)
        .ok_or_else(|| format!("unknown exact sequence ID {target}"))?;
    if !graph.multicams.is_empty()
        || !graph.multicam_uses.is_empty()
        || !matches!(sequence.rate, r if r == Rational::integer(24) || r == rate_23976())
        || !matches!(sequence.frame_size, None | Some((1920, 1080)))
        || sequence.start.is_none()
        || sequence.placements.is_empty()
        || !sequence.transitions.is_empty()
        || !sequence.disabled_placements.is_empty()
        || !sequence.texts.is_empty()
        || sequence.placements.iter().any(|edit| {
            !matches!(
                (edit.video_track, edit.audio_tracks.as_slice()),
                (Some(1), []) | (None, [1 | 2])
            ) || !edit.grouped_audio.is_empty()
                || !edit.volume_keys.is_empty()
                || edit.speed.is_some()
                || edit.frame_map.is_some()
                || edit.transform.is_some()
                || !edit.effects.is_empty()
        })
    {
        return Err("expected one 1080p24 or 23.976 straight-cut timeline on V1/A1/A2".into());
    }
    let items: HashSet<String> = sequence
        .placements
        .iter()
        .map(|edit| edit.item.clone())
        .collect();
    let mut owners = items.clone();
    owners.insert(target.into());
    owners.extend(sequence.placements.iter().map(|p| p.id.clone()));
    graph.sequences.retain(|s| s.id == target);
    graph.items.retain(|item| items.contains(&item.id));
    owners.extend(
        graph
            .items
            .iter()
            .flat_map(|item| item.media.iter().map(|link| link.media.clone())),
    );
    graph.media.retain(|media| owners.contains(&media.id));
    for bin in &mut graph.bins {
        bin.items.retain(|id| items.contains(id));
        owners.insert(bin.id.clone());
    }
    graph.sidecars.retain(|s| owners.contains(&s.object));
    graph.losses.retain(|loss| owners.contains(&loss.object));
    graph.validate().map_err(|e| e.to_string())
}

fn rate_23976() -> Rational {
    Rational::new(24_000, 1001).unwrap()
}

fn ratio(value: &str) -> Option<Rational> {
    let (num, den) = value.split_once('/')?;
    Rational::new(num.parse().ok()?, den.parse().ok()?)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value.get(key)? {
        Value::Str(value) => Some(value),
        _ => None,
    }
}

fn integer(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Str(value) => value.parse().ok(),
        Value::Int(value) => Some(*value),
        _ => None,
    }
}

fn ticks(time: Rational) -> Result<i64> {
    let n = i128::from(time.num()) * i128::from(TICKS_PER_SECOND);
    let d = i128::from(time.den());
    if n % d != 0 {
        return Err("media clock is off Premiere's tick grid".into());
    }
    i64::try_from(n / d).map_err(|_| "media clock overflows Premiere ticks".into())
}

fn period(rate: Rational) -> Result<i64> {
    if rate.num() <= 0 {
        return Err("invalid media rate".into());
    }
    ticks(Rational::new(rate.den().into(), rate.num().into()).ok_or("invalid media rate")?)
}

fn stream_duration(stream: &Value) -> Option<Rational> {
    let units = integer(stream, "duration_ts")?;
    let base = ratio(text(stream, "time_base")?)?;
    Rational::new(
        i128::from(units) * i128::from(base.num()),
        base.den().into(),
    )
}

fn pcm(media: &SourceMedia, stream: &Value) -> Result<Essence> {
    let Some(bits @ (16 | 24)) = media.bits else {
        return Err("unqualified PCM depth".into());
    };
    let expected_codec = if bits == 16 { "pcm_s16le" } else { "pcm_s24le" };
    if text(stream, "codec_type") != Some("audio")
        || text(stream, "codec_name") != Some(expected_codec)
        || integer(stream, "bits_per_sample") != Some(bits)
        || integer(stream, "channels") != Some(1)
        || integer(stream, "sample_rate") != Some(48_000)
        || media.sample_rate != Some(48_000)
    {
        return Err("source does not match the saved mono PCM layout".into());
    }
    Ok(Essence::MonoPcm {
        bits,
        sample_ticks: TICKS_PER_SECOND / 48_000,
    })
}

fn picture(
    media: &SourceMedia,
    stream: &Value,
    codec: &[u8; 4],
    rate: Rational,
) -> Result<Essence> {
    if text(stream, "codec_type") != Some("video")
        || media.rate != Some(rate)
        || media.frame_size != Some((1920, 1080))
        || integer(stream, "width") != Some(1920)
        || integer(stream, "height") != Some(1080)
        || text(stream, "sample_aspect_ratio") != Some("1:1")
        || text(stream, "field_order") != Some("progressive")
    {
        return Err("source does not match the saved progressive picture raster and clock".into());
    }
    Ok(Essence::Picture {
        codec: u32::from_be_bytes(*codec),
        frame_ticks: period(rate)?,
        width: 1920,
        height: 1080,
    })
}

fn original(media: &SourceMedia, path: &Path, tool: &Path) -> Result<FileFacts> {
    let saved = media
        .original
        .as_ref()
        .ok_or("missing saved physical selector")?;
    let label = if media.kind == StreamKind::Video {
        let start = ticks(
            media
                .start
                .ok_or("ProRes original source start is unresolved")?,
        )?;
        Some(
            Time::new(start)
                .to_timecode(period(Rational::integer(24))?, false)
                .map_err(|err| err.to_string())?,
        )
    } else {
        None
    };
    let probed =
        avid_adapter::original::inspect_premiere_donor_original(path, tool, label.as_deref())?;
    let Some(Value::Arr(streams)) = probed.get("streams") else {
        return Err("original has no streams".into());
    };
    let mut essence = streams
        .iter()
        .filter(|s| matches!(text(s, "codec_type"), Some("video" | "audio")));
    let stream = essence.next().ok_or("no original essence stream")?;
    if essence.next().is_some()
        || saved.stream_index != 0
        || integer(stream, "index") != Some(0)
        || integer(stream, "start_pts").unwrap_or(0) != 0
        || saved.start_pts != 0
        || stream_duration(stream) != Some(media.duration)
    {
        return Err("original stream selector, packet origin or playable extent differs".into());
    }
    let mut alternate_start = None;
    let essence = match (media.kind, saved.container.as_str()) {
        (StreamKind::Video, "mov") => {
            if !matches!(media.codec.as_deref(), Some("apch" | "prores"))
                || text(stream, "codec_name") != Some("prores")
                || text(stream, "codec_tag_string") != Some("apch")
                || text(stream, "avg_frame_rate").and_then(ratio) != Some(Rational::integer(24))
                || saved.channel_index.is_some()
                || saved.channels.is_some()
            {
                return Err(
                    "original picture is outside the verified ProRes HQ MOV profile".into(),
                );
            }
            let clock = media
                .clock
                .as_ref()
                .ok_or("ProRes original source clock is unresolved")?;
            let start = ticks(
                media
                    .start
                    .ok_or("ProRes original source start is unresolved")?,
            )?;
            let frame_ticks = period(Rational::integer(24))?;
            let timecode = stream
                .get("tags")
                .and_then(|tags| text(tags, "timecode"))
                .ok_or("embedded ProRes source timecode is missing")?;
            if clock.rate != Rational::integer(24)
                || clock.drop
                || clock.display_base != 24
                || Time::from_timecode(timecode, frame_ticks)
                    .map_err(|err| err.to_string())?
                    .ticks
                    != start
            {
                return Err("embedded ProRes source clock contradicts graph/2".into());
            }
            alternate_start = Some(start).filter(|&start| start != 0);
            picture(media, stream, b"apch", Rational::integer(24))?
        }
        (StreamKind::Audio, "wav") => {
            if saved.channel_index != Some(0)
                || saved.channels != Some(1)
                || media.start.is_some_and(|v| v != Rational::ZERO)
                || !matches!(
                    (media.bits, saved.sample_representation.as_deref()),
                    (Some(16), Some("pcm16")) | (Some(24), Some("pcm24"))
                )
            {
                return Err(
                    "saved WAV physical channel selector or representation is unqualified".into(),
                );
            }
            avid_adapter::original::verify_premiere_donor_payload(
                path,
                tool,
                saved.duration_ts,
                media.bits,
            )?;
            pcm(media, stream)?
        }
        _ => return Err("expected picture-only ProRes HQ MOV or standalone mono PCM WAV".into()),
    };
    // The Premiere capture uses packet units. The Avid adapter expresses a MOV
    // witness in frames. Both must resolve to the independently probed extent.
    let declared = Rational::new(
        i128::from(saved.duration_ts) * i128::from(saved.time_base.num()),
        saved.time_base.den().into(),
    );
    if declared != Some(media.duration) {
        return Err("graph/2 physical stream extent contradicts the saved media duration".into());
    }
    Ok(FileFacts {
        essence,
        duration: ticks(media.duration)?,
        physical_start: 0,
        alternate_start,
    })
}

fn atom(graph: &SourceGraph, media: &SourceMedia, path: &Path, tool: &Path) -> Result<FileFacts> {
    if media.kind == StreamKind::Video && media.codec.as_deref() != Some("AVdn") {
        return Err("direct OP-Atom requires DNxHR LB picture".into());
    }
    let clock = media
        .clock
        .as_ref()
        .ok_or("OP-Atom source clock is unresolved")?;
    let physical = avid_reader::mxf::read_identity(path).map_err(|err| err.to_string())?;
    let native = physical
        .qualified_file_clock()
        .ok_or("OP-Atom file clock is unresolved")?;
    let package = graph
        .sidecar("avid.source", &media.id)
        .and_then(|receipt| text(receipt, "file_package"))
        .ok_or("OP-Atom graph file package is unresolved")?;
    let package = package
        .strip_prefix("urn:smpte:umid:")
        .unwrap_or(package)
        .replace('.', "");
    if physical.file_packages.len() != 1
        || !physical.file_packages[0].eq_ignore_ascii_case(&package)
        || native.origin_count != clock.provenance.origin_count
        || native.effective_count != clock.effective_count
        || native.source_offset != clock.provenance.source_offset
        || ratio(&format!(
            "{}/{}",
            native.source_rate.0, native.source_rate.1
        )) != Some(clock.provenance.source_rate)
        || native.source_package != clock.provenance.source_package
        || native.timecode_track != clock.provenance.timecode_track
        || native.rate != (24_000, 1001)
        || clock.rate != rate_23976()
        || native.drop
        || clock.drop
        || native.rounded_base != clock.display_base
    {
        return Err("OP-Atom file package or saved source clock differs".into());
    }
    let inspected = avid_adapter::original::probe_offline_atom(path, tool)?;
    let Some(Value::Arr(streams)) = inspected.get("streams") else {
        return Err("OP-Atom stream list is absent".into());
    };
    let mut streams = streams
        .iter()
        .filter(|s| matches!(text(s, "codec_type"), Some("video" | "audio")));
    let stream = streams.next().ok_or("OP-Atom essence is absent")?;
    let duration = ticks(media.duration)?;
    let [(num, den, units)] = physical.file_tracks.as_slice() else {
        return Err("expected one OP-Atom file essence track".into());
    };
    if streams.next().is_some()
        || integer(stream, "start_pts").unwrap_or(0) != 0
        || stream_duration(stream) != Some(media.duration)
        || Rational::new(i128::from(*units) * i128::from(*den), i128::from(*num))
            != Some(media.duration)
        || inspected
            .get("format")
            .and_then(|v| v.get("tags"))
            .and_then(|v| text(v, "operational_pattern_ul"))
            != Some("060e2b34.04010102.0d010201.10030000")
    {
        return Err("OP-Atom physical extent or essence topology differs".into());
    }
    let essence = match media.kind {
        StreamKind::Video
            if text(stream, "codec_name") == Some("dnxhd")
                && text(stream, "profile") == Some("DNXHR LB") =>
        {
            picture(media, stream, b"AVdh", rate_23976())?
        }
        StreamKind::Audio if media.bits == Some(24) && matches!(media.stream, Some(1 | 2)) => {
            pcm(media, stream)?
        }
        _ => {
            return Err(
                "direct OP-Atom requires DNxHR LB and independent PCM24 mono siblings".into(),
            )
        }
    };
    let start = i64::try_from(clock.provenance.origin_count)
        .ok()
        .and_then(|count| count.checked_mul(period(rate_23976()).ok()?))
        .ok_or("physical OP-Atom timecode overflows")?;
    avid_adapter::original::verify_premiere_donor_payload(path, tool, *units, media.bits)?;
    Ok(FileFacts {
        essence,
        duration,
        physical_start: start,
        alternate_start: None,
    })
}

fn prepare(
    graph: &SourceGraph,
    destination: &Path,
) -> Result<prproj::convert::writer::PreparedProject> {
    let prepared = premiere_adapter::prepare_originals(
        graph,
        &Options::default(),
        premiere_adapter::ClockProfile::LegacyImport,
    )
    .map_err(|err| err.to_string())?;
    premiere_preparation::prepare_document(
        &prepared.input,
        destination,
        &mut RandomUuids::new(),
        &[],
        false,
        None,
        &HashSet::new(),
        &HashSet::new(),
    )
    .map_err(|err| err.to_string())
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub sequence: String,
    pub published: bool,
    pub files: Vec<FileLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerState>,
}

#[derive(Debug, Serialize)]
pub struct FileLink {
    pub path: String,
    pub link: Link,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Link {
    Direct,
    PremiereRequired,
    PremiereDonor,
}

pub fn run(path: &Path, mode: Mode) -> Result<Report> {
    let request: Request =
        serde_json::from_str(&bounded_file(path, 64 << 10)?).map_err(|e| e.to_string())?;
    output::preflight(&request.output, None)?;
    if request.output.extension().and_then(|s| s.to_str()) != Some("prproj") {
        return Err("output must be a new .prproj".into());
    }
    let mut graph = SourceGraph::from_json_text(&bounded_file(
        &request.graph,
        graph::source::MAX_SOURCE_GRAPH_BYTES,
    )?)
    .map_err(|err| err.to_string())?;
    select(&mut graph, &request.sequence)?;
    anynle_media_verification::linked_original::qualify(&graph.media).map_err(|e| e.to_string())?;
    if graph.media.len() > 64 {
        return Err("import trial is limited to 64 original streams".into());
    }

    let mut expected = ExpectedMedia::new();
    let mut pins = Vec::new();
    let mut missing_codec = HashSet::new();
    let mut physical_paths = HashSet::new();
    for media in &graph.media {
        let [path] = media.locations.as_slice() else {
            return Err(format!("{}: ambiguous original locator", media.id));
        };
        let file = Path::new(path);
        if !file.is_absolute() || media.online == Some(false) {
            return Err(format!(
                "{}: original must be online at an absolute path",
                media.id
            ));
        }
        if expected.contains_key(path) {
            return Err(format!("{path}: conflicting original stream bindings"));
        }
        let canonical = file
            .canonicalize()
            .map_err(|err| format!("{path}: {err}"))?;
        if !physical_paths.insert(canonical) {
            return Err(format!("{path}: ambiguous aliases of one original"));
        }
        let stamp = source_input::original_stamp(file)?;
        let facts = if media.original.is_some() {
            original(media, file, &request.ffprobe)
        } else {
            atom(&graph, media, file, &request.ffprobe)
        }
        .map_err(|err| format!("{} ({path}): {err}", media.id))?;
        if media.codec.as_deref() == Some("prores") {
            missing_codec.insert(path.clone());
        }
        pins.push(stamp);
        expected.insert(path.clone(), facts);
    }
    check_hashes(&request, &expected)?;

    let mut donor_paths = HashSet::new();
    let mut pending = false;
    let mut donor = None;
    // Resolve the verified native spelling of a known graph clock in the destination
    // view. The saved graph and its edit coordinates are retained verbatim.
    for media in &mut graph.media {
        if media
            .codec
            .as_deref()
            .is_some_and(|codec| matches!(codec, "apch" | "prores"))
        {
            if let Some(clock) = &mut media.clock {
                clock.provenance.route = VERIFIED_LINKED_MOV_TIMECODE.into();
            }
        }
    }

    let standalone = prepare(&graph, &request.output);
    let mut project = match standalone {
        Ok(project) => {
            if mode == Mode::ReplayDonor {
                donor = Some(read_donor(explicit_donor(&request)?, &expected)?);
                donor_paths.extend(expected.keys().cloned());
            }
            project
        }
        Err(error)
            if error.ends_with("unqualified original picture stream/profile")
                && !missing_codec.is_empty() =>
        {
            let required = if mode == Mode::ReplayDonor {
                expected.clone()
            } else {
                expected
                    .iter()
                    .filter(|(path, _)| missing_codec.contains(*path))
                    .map(|(path, facts)| (path.clone(), facts.clone()))
                    .collect()
            };
            for media in &mut graph.media {
                if missing_codec.contains(&media.locations[0]) {
                    // The probe already established apch from the original's
                    // physical stream. This spelling lets us validate the
                    // staged edit before asking Premiere to import anything.
                    media.codec = Some("apch".into());
                }
            }
            let mut project = prepare(&graph, &request.output)?;
            project
                .verify_linked_media(&expected, None)
                .map_err(|e| e.to_string())?;
            donor = match &request.donor {
                Some(path) => Some(read_donor(path, &required)?),
                None if mode == Mode::Review => {
                    pending = true;
                    None
                }
                None if mode == Mode::Convert => {
                    source_input::recheck_originals(&pins)?;
                    let paths: Vec<_> = required.keys().cloned().collect();
                    let captured = worker::capture(&paths).map_err(|error| {
                        format!(
                            "{} needs Premiere for media linking. {error}",
                            paths.join(", ")
                        )
                    })?;
                    Some(read_donor(&captured.path, &required)?)
                }
                None => return Err("Donor replay needs a Premiere project".into()),
            };
            donor_paths.extend(required.into_keys());
            project
        }
        Err(error) => return Err(error),
    };
    project
        .verify_linked_media(&expected, donor.as_ref())
        .map_err(|e| e.to_string())?;
    source_input::recheck_originals(&pins)?;
    check_hashes(&request, &expected)?;
    if mode != Mode::Review {
        project.publish().map_err(|e| e.to_string())?;
    }
    let files = expected
        .into_keys()
        .map(|path| {
            let link = if !donor_paths.contains(&path) {
                Link::Direct
            } else if pending {
                Link::PremiereRequired
            } else {
                Link::PremiereDonor
            };
            FileLink { path, link }
        })
        .collect();
    Ok(Report {
        sequence: request.sequence,
        published: mode != Mode::Review,
        files,
        worker: pending.then(worker_status),
    })
}

fn explicit_donor(request: &Request) -> Result<&Path> {
    request
        .donor
        .as_deref()
        .ok_or_else(|| "Donor replay needs a Premiere project".into())
}

fn read_donor(path: &Path, expected: &ExpectedMedia) -> Result<MediaRecords> {
    MediaRecords::read(path, expected).map_err(|err| format!("{}: {err}", path.display()))
}

fn check_hashes(request: &Request, expected: &ExpectedMedia) -> Result<()> {
    for (path, hash) in &request.original_sha256 {
        if !expected.contains_key(path) {
            return Err(format!("{path}: hash is for an unselected original"));
        }
        if source_input::checked_revision(Path::new(path), &mut || Ok(()))? != *hash {
            return Err(format!(
                "{path}: original media changed from its recorded SHA-256"
            ));
        }
    }
    Ok(())
}
