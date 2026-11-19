//! Bounded native Media Pool records from a dedicated Resolve scripting session.
use crate::{Result, archive, linked, media, source};
use graph::{Rational, StreamKind, source::SourceMedia};
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn supported_rate(rate: Rational) -> bool {
    matches!(
        (rate.num(), rate.den()),
        (24000, 1001) | (24 | 25, 1) | (30000, 1001)
    )
}
const WORKER: &str = "ANYNLE_RESOLVE_IMPORT_WORKER";
const SDK: &str =
    "/Library/Application Support/Blackmagic Design/DaVinci Resolve/Developer/Scripting/Modules";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerStatus {
    Disconnected,
    Free,
    UnsupportedBuild(String),
    Unconfigured,
    OpenWorker(String),
    Busy,
    Ready,
}

pub fn worker_status() -> WorkerStatus {
    let name = std::env::var(WORKER).unwrap_or_default();
    let Ok(output) = tempfile::tempfile() else {
        return WorkerStatus::Disconnected;
    };
    let Ok(stdout) = output.try_clone() else {
        return WorkerStatus::Disconnected;
    };
    let result = run_worker(
        &name,
        [Path::new("--status")],
        Stdio::from(stdout),
        Duration::from_secs(5),
    );
    if !result.is_ok_and(|status| status.success()) {
        return WorkerStatus::Disconnected;
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut output = output;
    if output.seek(SeekFrom::Start(0)).is_err() {
        return WorkerStatus::Disconnected;
    }
    let mut json = Vec::new();
    if output.take(8192).read_to_end(&mut json).is_err() {
        return WorkerStatus::Disconnected;
    }
    let Ok((state, detail)) = serde_json::from_slice::<(String, String)>(&json) else {
        return WorkerStatus::Disconnected;
    };
    match state.as_str() {
        "free" => WorkerStatus::Free,
        "unsupported_build" => WorkerStatus::UnsupportedBuild(detail),
        "worker_unconfigured" => WorkerStatus::Unconfigured,
        "open_worker" => WorkerStatus::OpenWorker(detail),
        "worker_busy" => WorkerStatus::Busy,
        "ready" => WorkerStatus::Ready,
        _ => WorkerStatus::Disconnected,
    }
}

pub(crate) struct Movie {
    pub(crate) xml: String,
    pub(crate) selectors: Option<[String; 2]>,
    streams: Vec<SourceMedia>,
    picture_time_base: Rational,
    picture_ticks: i64,
    picture_codec: String,
    audio: Option<Audio>,
    pub(crate) timecode: Option<String>,
}

struct Audio {
    codec: String,
    samples: i64,
}

impl Movie {
    pub(crate) fn readback(&self, graph: &graph::source::SourceGraph, item_id: &str) -> bool {
        let Some(item) = graph.items.iter().find(|item| item.id == item_id) else {
            return false;
        };
        item.media.len() == self.streams.len()
            && item
                .media
                .iter()
                .zip(&self.streams)
                .all(|(link, expected)| {
                    graph
                        .media
                        .iter()
                        .find(|row| row.id == link.media)
                        .is_some_and(|saved| {
                            saved.locations == expected.locations
                                && saved.kind == expected.kind
                                && saved.codec == expected.codec
                                && saved.rate == expected.rate
                                && saved.start == expected.start
                                && saved.duration == expected.duration
                                && saved.frame_size == expected.frame_size
                                && saved.sample_rate == expected.sample_rate
                                && saved.bits == expected.bits
                                && saved.stream == expected.stream
                                && saved.clock.as_ref().map(|clock| clock.effective_count)
                                    == expected.clock.as_ref().map(|clock| clock.effective_count)
                                && saved.clock.as_ref().map(|clock| clock.drop)
                                    == expected.clock.as_ref().map(|clock| clock.drop)
                        })
                })
    }

    pub(crate) fn check_stream(
        &self,
        graph: &SourceMedia,
        selector: &media::StreamSelector,
    ) -> Result<()> {
        let (index, time_base, ticks, codec) = match (graph.kind, selector.channel) {
            (StreamKind::Video, None) => (
                0,
                self.picture_time_base,
                self.picture_ticks,
                self.picture_codec.as_str(),
            ),
            (StreamKind::Audio, Some(channel @ (0 | 1))) => (
                channel + 1,
                Rational::new(1, 48000).unwrap(),
                self.audio
                    .as_ref()
                    .ok_or("native movie has no physical audio")?
                    .samples,
                self.audio
                    .as_ref()
                    .ok_or("native movie has no physical audio")?
                    .codec
                    .as_str(),
            ),
            _ => return Err("unsupported native source selector".into()),
        };
        let saved = self
            .streams
            .get(index)
            .ok_or("native movie is missing the selected stream")?;
        let expected_index = usize::from(index != 0);
        if selector.index != expected_index
            || graph.kind != saved.kind
            || graph
                .codec
                .as_deref()
                .is_some_and(|value| value != codec && Some(value) != saved.codec.as_deref())
            || graph.locations != saved.locations
            || graph.duration != saved.duration
            || graph.start.unwrap_or(Rational::ZERO) != saved.start.unwrap_or(Rational::ZERO)
            || graph.rate != saved.rate
            || graph.clock.as_ref().is_some_and(|clock| {
                let observed = self
                    .timecode
                    .as_deref()
                    .and_then(|label| {
                        media::parse_timecode(label, linked::display_base(self.rate())).ok()
                    })
                    .or_else(|| {
                        saved
                            .clock
                            .as_ref()
                            .map(|clock| (clock.effective_count, clock.drop))
                    });
                observed != Some((clock.effective_count, clock.drop))
                    || clock.rate != self.rate()
                    || clock.display_base != linked::display_base(self.rate())
            })
            || graph.frame_size != saved.frame_size
            || graph.sample_rate != saved.sample_rate
            || graph.bits != saved.bits
            || index != 0 && graph.stream != Some(index as i64)
            || graph.original.as_ref().is_some_and(|original| {
                original.container != "mov"
                    || original.stream_index != expected_index as i64
                    || original.channel_index != selector.channel.map(|channel| channel as i64)
                    || original.channels != (index != 0).then_some(2)
                    || original.start_pts != 0
                    || original.time_base != time_base
                    || original.duration_ts != ticks
            })
        {
            return Err("graph media facts disagree with the native import record".into());
        }
        Ok(())
    }

    pub(crate) fn frames(&self) -> Result<i64> {
        linked::extent(self.picture_ticks, self.picture_time_base)
            .and_then(|duration| crate::prepare::frames_on(duration, self.rate()))
    }

    pub(crate) fn rate(&self) -> Rational {
        self.streams[0].rate.expect("verified native picture rate")
    }

    pub(crate) fn start(&self) -> Result<i64> {
        crate::prepare::frames_on(self.streams[0].start.unwrap_or(Rational::ZERO), self.rate())
    }

    pub(crate) fn raster(&self) -> (u16, u16) {
        let (width, height) = self.streams[0].frame_size.expect("verified native raster");
        (width as u16, height as u16)
    }

    pub(crate) fn bits(&self) -> Option<i64> {
        self.streams.get(1).and_then(|stream| stream.bits)
    }

    pub(crate) fn picture_clock(&self) -> (Rational, i64) {
        (self.picture_time_base, self.picture_ticks)
    }

    pub(crate) fn picture_codec(&self) -> &str {
        &self.picture_codec
    }

    pub(crate) fn audio_clock(&self) -> Option<(&str, i64)> {
        self.audio
            .as_ref()
            .map(|audio| (audio.codec.as_str(), audio.samples))
    }

    pub(crate) fn aac(&self) -> bool {
        self.audio_clock().is_some_and(|(codec, _)| codec == "aac")
    }

    pub(crate) fn check_audio_end(&self, edit: &graph::Placement) -> Result<()> {
        let samples = self
            .audio
            .as_ref()
            .ok_or("native movie has no physical audio")?
            .samples;
        let end = edit
            .source_in
            .checked_add(edit.duration)
            .ok_or("audio edit end overflow")?;
        if end > linked::extent(samples, Rational::new(1, 48000).unwrap())? {
            return Err("audio edit exceeds the verified sample extent".into());
        }
        Ok(())
    }
}

pub(crate) fn eligible(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mov") || ext.eq_ignore_ascii_case("mp4"))
}

pub(crate) fn available() -> bool {
    cfg!(target_os = "macos")
        && std::env::var(WORKER).is_ok_and(|name| name.starts_with("Any NLE Import Worker "))
}

pub(crate) fn capture(path: &Path, ffprobe: &Path) -> Result<Movie> {
    let worker_name = std::env::var(WORKER)?;
    if !available() {
        return Err("no dedicated Resolve import worker is configured".into());
    }
    let temp = tempfile::tempdir()?;
    let archive = temp.path().join("native.drp");
    let status = run_worker(
        &worker_name,
        [path, &archive],
        Stdio::null(),
        Duration::from_secs(45),
    )?;
    if !status.success() {
        let message = if status.code() == Some(2) {
            "Resolve import worker is unavailable or its dedicated scratch project is not active and empty"
        } else {
            "Resolve could not export the original from the dedicated import worker"
        };
        return Err(linked::UnavailableCache(message).into());
    }
    if std::env::var_os("ANYNLE_RESOLVE_IMPORT_TRACE").is_some() {
        eprintln!("Resolve API imported {}", path.display());
    }
    read(&archive, path, ffprobe)
}

fn run_worker<const N: usize>(
    worker: &str,
    args: [&Path; N],
    stdout: Stdio,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    let mut process = Command::new("python3")
        .args(["-c", include_str!("native_import.py"), worker])
        .args(args)
        .env("PYTHONPATH", SDK)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = process.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            return Err(
                linked::UnavailableCache("dedicated Resolve import worker timed out").into(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn ratio(row: &Value) -> Result<Rational> {
    let (numerator, denominator) = row
        .as_str()
        .and_then(|s| s.split_once('/'))
        .ok_or("missing native original rate or time base")?;
    Rational::new(numerator.parse()?, denominator.parse()?)
        .ok_or_else(|| "invalid native original clock".into())
}

fn count(row: &Value) -> Option<i64> {
    row.as_i64().or_else(|| row.as_str()?.parse().ok())
}

fn read(archive_path: &Path, path: &Path, ffprobe: &Path) -> Result<Movie> {
    let graph = source::read(archive_path)?;
    let locator = crate::prepare::utf8_path(path)?;
    let [item] = graph.items.as_slice() else {
        return Err("native worker export must contain one Media Pool item".into());
    };
    if !graph.sequences.is_empty()
        || !graph.multicams.is_empty()
        || !graph.losses.is_empty()
        || !item.markers.is_empty()
        || graph.sidecar("resolve.proxy", &item.id).is_some()
        || !matches!(graph.media.len(), 1 | 3)
        || graph
            .media
            .iter()
            .any(|stream| stream.locations.as_slice() != [locator])
    {
        return Err("unqualified native import record".into());
    }
    let picture = &graph.media[0];
    let rate = picture
        .rate
        .filter(|&rate| supported_rate(rate))
        .ok_or("unsupported native picture rate")?;
    if picture.kind != StreamKind::Video {
        return Err("native import is missing picture".into());
    }
    let mut xml = archive::imported_movie(archive_path, &item.id)?;
    let selectors = match (
        graph.media.len(),
        source::imported_selectors(archive_path, &item.id)?,
    ) {
        (3, Some(selectors)) => Some(selectors),
        (3, None) => Some(archive::split_imported_movie_audio(&mut xml)?),
        (1, None) => None,
        _ => {
            return Err(
                "native Media Pool stereo PCM does not have qualified split selectors".into(),
            );
        }
    };

    let probe = media::probe_json(ffprobe, path, &["-f", "mov", "-show_streams"])?;
    let streams = probe["streams"]
        .as_array()
        .ok_or("missing native physical streams")?;
    let video = streams.first().ok_or("missing native physical picture")?;
    let (width, height) = picture.frame_size.ok_or("missing native picture raster")?;
    let frames = crate::prepare::frames_on(picture.duration, rate)?;
    let start = crate::prepare::frames_on(picture.start.unwrap_or(Rational::ZERO), rate)?;
    let time_base = ratio(&video["time_base"])?;
    let duration_ts = count(&video["duration_ts"]).ok_or("missing native media duration")?;
    let (source_timecode, drop) = match video["tags"]["timecode"].as_str() {
        Some(tc) => media::parse_timecode(tc, linked::display_base(rate))?,
        None => (0, false),
    };
    if video["codec_type"] != "video"
        || video["index"] != 0
        || ratio(&video["avg_frame_rate"])? != rate
        || ratio(&video["r_frame_rate"])? != rate
        || count(&video["start_pts"]) != Some(0)
        || count(&video["nb_frames"]) != Some(frames)
        || count(&video["width"]) != Some(width)
        || count(&video["height"]) != Some(height)
        || !matches!(
            video["sample_aspect_ratio"].as_str(),
            None | Some("1:1" | "N/A")
        )
        || !matches!(video["field_order"].as_str(), None | Some("progressive"))
        || !video["side_data_list"].is_null()
        || !video["tags"]["rotate"].is_null()
        || linked::extent(duration_ts, time_base)? != picture.duration
        || source_timecode != u64::try_from(start)?
        || picture
            .clock
            .as_ref()
            .is_some_and(|clock| clock.drop != drop)
        || frames <= 0
        || frames > i64::from(i32::MAX)
        || u16::try_from(width).is_err()
        || u16::try_from(height).is_err()
        || duration_ts % frames != 0
    {
        return Err(
            "physical picture clock, geometry or extent differs from the native import".into(),
        );
    }
    let picture_codec = video["codec_name"]
        .as_str()
        .ok_or("missing native physical codec")?
        .to_owned();
    let (audio, auxiliary) = if selectors.is_some() {
        (
            Some(streams.get(1).ok_or("missing physical stereo PCM stream")?),
            &streams[2..],
        )
    } else {
        (None, &streams[1..])
    };
    if auxiliary.len() > 1
        || auxiliary.iter().any(|row| {
            row["codec_type"] != "data"
                || row["codec_tag_string"] != "tmcd"
                || row["tags"]["timecode"] != video["tags"]["timecode"]
                || count(&row["start_pts"]) != Some(0)
                || count(&row["duration_ts"])
                    .zip(ratio(&row["time_base"]).ok())
                    .and_then(|(ticks, base)| linked::extent(ticks, base).ok())
                    != Some(picture.duration)
        })
        || (video["tags"]["timecode"].is_string() && auxiliary.is_empty())
    {
        return Err("unqualified native original auxiliary stream".into());
    }
    let audio = if let Some(audio) = audio {
        let codec = audio["codec_name"]
            .as_str()
            .ok_or("missing physical audio codec")?;
        let bits = match codec {
            "pcm_s16le" | "pcm_s16be" => Some(16),
            "pcm_s24le" | "pcm_s24be" => Some(24),
            "aac"
                if audio["profile"] == "LC"
                    && audio["codec_tag_string"] == "mp4a"
                    && audio["channel_layout"] == "stereo" =>
            {
                None
            }
            _ => {
                return Err(
                    "native importer requires embedded stereo PCM16, PCM24 or AAC-LC".into(),
                );
            }
        };
        let samples = count(&audio["duration_ts"])
            .filter(|&count| count > 0)
            .ok_or("missing playable audio sample extent")?;
        let duration = linked::extent(samples, Rational::new(1, 48000).unwrap())?;
        if audio["codec_type"] != "audio"
            || audio["index"] != 1
            || audio["channels"] != 2
            || !matches!(
                audio["channel_layout"].as_str(),
                None | Some("unknown" | "stereo")
            )
            || ratio(&audio["time_base"])? != Rational::new(1, 48000).unwrap()
            || audio["sample_rate"] != "48000"
            || count(&audio["start_pts"]) != Some(0)
            || graph.media[1..].iter().enumerate().any(|(index, stream)| {
                stream.kind != StreamKind::Audio
                    || stream.sample_rate != Some(48000)
                    || stream.bits != bits
                    || stream.duration != duration
                    || stream.stream != Some(index as i64 + 1)
            })
        {
            return Err(
                "physical stereo layout or playable extent differs from the native import".into(),
            );
        }
        if let Some(bits) = bits {
            media::verify_pcm_packet_grid(ffprobe, path, "mov", 1, samples, 2 * (bits / 8))?;
        } else {
            media::verify_aac_packet_grid(ffprobe, path, samples)?;
        }
        Some(Audio {
            codec: codec.into(),
            samples,
        })
    } else {
        None
    };
    media::verify_mov_packet_grid(
        ffprobe,
        path,
        frames,
        duration_ts / frames,
        count(&video["has_b_frames"]).is_none_or(|n| n != 0),
    )?;
    Ok(Movie {
        xml,
        selectors,
        streams: graph.media,
        picture_time_base: time_base,
        picture_ticks: duration_ts,
        picture_codec,
        audio,
        timecode: video["tags"]["timecode"].as_str().map(str::to_owned),
    })
}
