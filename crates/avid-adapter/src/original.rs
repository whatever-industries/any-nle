//! Exact saved-locator/root-map lookup and bounded read-only physical-stream probes.
use crate::SourceReadIssue;
use avid_reader::source as av;
use graph::{source::OriginalStream, Rational, StreamKind, Value};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
#[derive(Clone, Debug)]
pub struct RootMapping {
    pub saved_root: PathBuf,
    pub current_root: PathBuf,
}
#[derive(Clone, Debug)]
pub struct OriginalOptions {
    pub root_mappings: Vec<RootMapping>,
    pub ffprobe: PathBuf,
}
impl Default for OriginalOptions {
    fn default() -> Self {
        Self {
            root_mappings: vec![],
            ffprobe: PathBuf::from("ffprobe"),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Selected {
    pub path: PathBuf,
    pub stream: OriginalStream,
    pub kind: StreamKind,
    pub codec: String,
    pub raster: Option<(i64, i64)>,
    pub rate: Option<Rational>,
    pub sample_rate: Option<i64>,
    pub bits: Option<i64>,
    pub duration: Rational,
    /// Source-frame origin for the qualified automatic adapter only.
    pub motion_audio_origin: Option<i64>,
}
#[derive(Default)]
pub struct OriginalIndex {
    pub tracks: HashMap<(String, usize), Selected>,
    pub original_inputs: Vec<av::SourceInput>,
    /// Explicit destination defaults, never observations of missing source tags.
    pub notes: Vec<String>,
}
fn text(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}
fn number(v: &Value, k: &str) -> Option<i64> {
    match v.get(k)? {
        Value::Str(s) => s.parse().ok(),
        Value::Int(n) => Some(*n),
        _ => None,
    }
}
fn ratio(v: &Value, k: &str) -> Option<Rational> {
    text(v, k).and_then(|s| {
        let (n, d) = s.split_once('/')?;
        Rational::new(n.parse().ok()?, d.parse().ok()?)
    })
}
const ORIGINAL_PROBE_ENTRIES:&str="stream=index,codec_type,codec_name,codec_tag_string,profile,level,pix_fmt,width,height,avg_frame_rate,r_frame_rate,sample_aspect_ratio,field_order,color_range,color_space,color_transfer,color_primaries,sample_rate,channels,channel_layout,bits_per_sample,bits_per_raw_sample,time_base,start_pts,duration_ts,nb_frames:stream_side_data=side_data_type:stream_tags=rotate,timecode:format=format_name:format_tags=major_brand:packet=stream_index,pts,duration:packet_side_data=side_data_type,skip_samples,discard_padding";
const UNTAGGED_COLOR_NOTE:&str="Original picture has incomplete or unspecified color/range tags. The writer uses its BT.709 destination profile as a default; this is not observed source color or range.";

/// A suffix chooses a supported route only after the physical demuxer family
/// corroborates it. In particular, a zero PTS does not turn PCM in another
/// container into a WAV source.
fn qualify_container(path: &Path, probe: &Value) -> Result<&'static str, String> {
    let format = probe.get("format").and_then(|f| f.get("format_name"));
    let Some(Value::Str(format)) = format else {
        return Err("original physical container format is absent or unresolved".into());
    };
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let movie_family = !format.is_empty()
        && format
            .split(',')
            .all(|name| matches!(name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"));
    match extension.as_str(){
        "wav" if format=="wav"=>Ok("wav"),
        "mov"|"mp4" if movie_family=>Ok("mov"),
        "wav"=>Err("original physical container mismatch: .wav requires the WAV demuxer family".into()),
        "mov"|"mp4"=>Err("original physical container mismatch: .mov/.mp4 requires the FFmpeg MOV demuxer family".into()),
        _=>Err("original suffix has no qualified physical container route".into()),
    }
}

/// Shared geometry, color and orientation checks for the admitted movie profiles.
/// A missing color declaration is a destination default, not a source BT.709 fact.
fn qualify_movie_picture(video: &Value, allow_adapter_rate: bool) -> Result<bool, String> {
    let rate = ratio(video, "avg_frame_rate");
    let qualified_rate = rate == Some(Rational::integer(24))
        || (allow_adapter_rate && rate == Rational::new(24_000, 1001));
    if number(video, "width") != Some(1920)
        || number(video, "height") != Some(1080)
        || !qualified_rate
        || ratio(video, "r_frame_rate") != rate
        || text(video, "field_order").as_deref() != Some("progressive")
        || text(video, "sample_aspect_ratio").as_deref() != Some("1:1")
    {
        return Err("original picture geometry/rate is unqualified: requires square-pixel progressive 1920x1080 at 24 fps, or 23.976 fps with a qualified automatic adapter".into());
    }
    let mut defaulted = false;
    for field in ["color_space", "color_transfer", "color_primaries"] {
        match video.get(field) {
            Some(Value::Str(value)) if value=="bt709"=>{},
            None=>defaulted=true,
            Some(Value::Str(value)) if matches!(value.as_str(),"unknown"|"unspecified")=>defaulted=true,
            _=>return Err(format!("original picture {field} is unqualified: requires BT.709 or an untagged/unspecified value; HDR and other color profiles are not admitted")),
        }
    }
    match video.get("color_range") {
        Some(Value::Str(value)) if value=="tv"=>{},
        None=>defaulted=true,
        Some(Value::Str(value)) if matches!(value.as_str(),"unknown"|"unspecified")=>defaulted=true,
        _=>return Err("original picture color range is unqualified: full-range and other tagged ranges are not admitted".into()),
    }
    match video.get("side_data_list") {
        None=>{},Some(Value::Arr(rows)) if rows.is_empty()=>{},
        _=>return Err("original picture side data is unqualified: HDR, rotation and other stream side data require separate interpretation".into()),
    }
    match video.get("tags") {
        None => {}
        Some(Value::Obj(tags)) => {
            let mut rotation = tags.iter().filter(|(name, _)| name == "rotate");
            if let Some((_, value)) = rotation.next() {
                let zero = match value {
                    Value::Str(s) => s.parse::<f64>().is_ok_and(|n| n.is_finite() && n == 0.0),
                    Value::Int(n) => *n == 0,
                    Value::Float(n) => n.is_finite() && *n == 0.0,
                    _ => false,
                };
                if !zero || rotation.next().is_some() {
                    return Err("original picture rotation is unqualified: nonzero or unresolved tags.rotate cannot use the identity-orientation writer profile".into());
                }
            }
        }
        _ => return Err("original picture rotation metadata is unresolved".into()),
    }
    Ok(defaulted)
}

fn qualify_movie(video: &Value, audio: Option<&Value>, allow_adapter_rate: bool) -> Result<bool, String> {
    if text(video, "codec_name").as_deref() != Some("h264")
        || text(video, "codec_tag_string").as_deref() != Some("avc1")
        || text(video, "profile").as_deref() != Some("High")
        || number(video, "level") != Some(40)
        || text(video, "pix_fmt").as_deref() != Some("yuv420p")
    {
        return Err("original picture profile is unqualified: requires H.264 High level 4.0 avc1/yuv420p; Hi10 and wider profiles are not admitted".into());
    }
    let defaulted = qualify_movie_picture(video, allow_adapter_rate && audio.is_some())?;
    if let Some(audio) = audio {
        if text(audio, "codec_name").as_deref() != Some("aac")
            || text(audio, "codec_tag_string").as_deref() != Some("mp4a")
            || text(audio, "profile").as_deref() != Some("LC")
            || number(audio, "sample_rate") != Some(48000)
            || number(audio, "channels") != Some(2)
            || text(audio, "channel_layout").as_deref() != Some("stereo")
        {
            return Err(
                "original movie audio profile is unqualified: requires AAC LC/mp4a 48000 Hz stereo"
                    .into(),
            );
        }
    }
    Ok(defaulted)
}

fn qualify_prores_hq(path: &Path, probe: &Value, streams: &[Value]) -> Result<bool, String> {
    let [video, timecode] = streams else {
        return Err("linked ProRes HQ requires one picture stream and one timecode track".into());
    };
    let brand = probe.get("format").and_then(|f| f.get("tags"))
        .and_then(|tags| text(tags, "major_brand"));
    if !path.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("mov"))
        || brand.as_deref() != Some("qt  ")
        || number(video, "index") != Some(0)
        || text(video, "codec_type").as_deref() != Some("video")
        || text(video, "codec_name").as_deref() != Some("prores")
        || text(video, "codec_tag_string").as_deref() != Some("apch")
        || text(video, "profile").as_deref() != Some("HQ")
        || text(video, "pix_fmt").as_deref() != Some("yuv422p10le")
    {
        return Err("linked ProRes picture requires QuickTime ProRes 422 HQ/apch, 10-bit 4:2:2".into());
    }
    let zero_timecode = |stream: &Value| {
        stream.get("tags").and_then(|tags| text(tags, "timecode")).as_deref() == Some("00:00:00:00")
    };
    let picture_base = ratio(video, "time_base").filter(|base| base.num() > 0);
    let picture_duration = number(video, "duration_ts").filter(|&duration| duration > 0);
    if number(timecode, "index") != Some(1)
        || text(timecode, "codec_type").as_deref() != Some("data")
        || text(timecode, "codec_tag_string").as_deref() != Some("tmcd")
        || number(timecode, "nb_frames") != Some(1)
        || ratio(timecode, "avg_frame_rate") != Some(Rational::integer(24))
        || picture_base.is_none()
        || ratio(timecode, "time_base") != picture_base
        || picture_duration.is_none()
        || number(timecode, "duration_ts") != picture_duration
        || number(timecode, "start_pts") != Some(0)
        || !zero_timecode(video)
        || !zero_timecode(timecode)
    {
        return Err("linked ProRes HQ requires one matching 24 fps non-drop timecode track starting at zero".into());
    }
    qualify_movie_picture(video, false)
}

/// Saved path syntax is parsed independently of the current OS. Only an explicit
/// prefix mapping may turn a foreign path into a local path; no basename search.
fn saved_parts(raw: &str) -> Result<(String, Vec<String>), String> {
    if raw.len() > 32768 || raw.chars().any(char::is_control) {
        return Err("original locator is too long or contains control characters".into());
    }
    let windows = raw.as_bytes().get(1) == Some(&b':')
        && raw.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && raw
            .as_bytes()
            .get(2)
            .is_some_and(|b| matches!(b, b'/' | b'\\'));
    let unc = raw.starts_with("\\\\");
    let normalized = if windows || unc {
        raw.replace('\\', "/")
    } else {
        raw.to_owned()
    };
    let (root, body) = if windows {
        (normalized[..2].to_ascii_uppercase(), &normalized[3..])
    } else if unc {
        let mut parts = normalized[2..].split('/');
        let server = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or("invalid UNC server")?;
        let share = parts
            .next()
            .filter(|s| !s.is_empty())
            .ok_or("invalid UNC share")?;
        if [server, share].iter().any(|p| matches!(*p, "." | "..")) {
            return Err("invalid UNC root".into());
        }
        let used = 2 + server.len() + 1 + share.len();
        (
            format!(
                "//{}/{}",
                server.to_ascii_lowercase(),
                share.to_ascii_lowercase()
            ),
            normalized.get(used + 1..).unwrap_or(""),
        )
    } else if normalized.starts_with('/') {
        ("/".into(), &normalized[1..])
    } else {
        return Err("original locator is not a normalized absolute path".into());
    };
    let mut parts = Vec::new();
    for part in body.split('/') {
        if part == ".." {
            return Err("original locator contains a parent escape".into());
        }
        if !part.is_empty() && part != "." {
            parts.push(part.to_owned());
        }
    }
    Ok((root, parts))
}
pub fn resolve(locators: &[String], mappings: &[RootMapping]) -> Result<PathBuf, String> {
    let mappings = mappings
        .iter()
        .map(|m| {
            if !m.current_root.is_absolute()
                || m.current_root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(
                    "current original root mappings must be absolute without parent escapes".into(),
                );
            }
            let saved = m
                .saved_root
                .to_str()
                .ok_or("saved root mapping is not UTF-8")?;
            Ok((saved_parts(saved)?, &m.current_root))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut paths = HashSet::new();
    for raw in locators {
        let (root, parts) = saved_parts(raw)?;
        let path = Path::new(raw);
        let mut candidates = Vec::new();
        if path.is_absolute() {
            candidates.push(path.to_owned());
        }
        for ((saved_root, saved), current) in &mappings {
            if root == *saved_root && parts.starts_with(saved) {
                let mut mapped = (*current).clone();
                for part in &parts[saved.len()..] {
                    mapped.push(part);
                }
                candidates.push(mapped);
            }
        }
        for p in candidates {
            if p.is_file() {
                paths.insert(std::fs::canonicalize(&p).map_err(|e| e.to_string())?);
            }
        }
    }
    if paths.len() != 1 {
        return Err(format!("original locator resolution has {} exact candidates; an explicit unambiguous root mapping is required",paths.len()));
    }
    Ok(paths.into_iter().next().unwrap())
}

fn bounded_probe(mut command: Command, timeout: std::time::Duration) -> Result<Vec<u8>, String> {
    use std::{sync::mpsc, time::Instant};
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("stdout pipe unavailable".into());
    };
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take((1 << 20) + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + timeout;
    let mut output = None;
    let mut status = None;
    let failure = loop {
        if output.is_none() {
            match rx.try_recv() {
                Ok(Ok(bytes)) if bytes.len() <= 1 << 20 => output = Some(bytes),
                Ok(Ok(_)) => break "stdout exceeds 1 MiB".to_string(),
                Ok(Err(e)) => break format!("stdout read failed: {e}"),
                Err(mpsc::TryRecvError::Disconnected) => break "stdout reader disconnected".into(),
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(e) => break format!("process status failed: {e}"),
            }
        }
        if let Some(status) = status {
            if let Some(bytes) = output.take() {
                if status.success() {
                    return Ok(bytes);
                }
                break format!("process refused the file ({status})");
            }
        }
        if Instant::now() >= deadline {
            break format!("timed out after {} ms", timeout.as_millis());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    // Kill and reap only this owned child. A bounded receive drains the pipe without
    // an unbounded job-thread read/join, even if a broken descendant keeps a pipe open.
    let _ = child.kill();
    let _ = child.wait();
    if output.is_none() {
        let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
    }
    Err(failure)
}
/// Verify picture geometry and return the probed rate and duration for supported Timewarp clocks.
pub(super) fn square_1080p_mxf(path: &Path, tool: &Path) -> Option<Option<(Rational, Rational)>> {
    let mut command = Command::new(tool);
    command.args([
        "-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mxf", "-select_streams", "v",
        "-show_entries",
        "stream=width,height,sample_aspect_ratio,field_order,r_frame_rate,time_base,duration_ts",
        "-of", "json",
    ]).arg(path);
    bounded_probe(command, std::time::Duration::from_secs(10)).ok()
        .and_then(|raw| graph::json::parse(std::str::from_utf8(&raw).ok()?).ok())
        .and_then(|value| {
            let Some(Value::Arr(streams)) = value.get("streams") else { return None; };
            let [video] = streams.as_slice() else { return None; };
            if number(video, "width") != Some(1920) || number(video, "height") != Some(1080)
                || text(video, "sample_aspect_ratio").as_deref() != Some("1:1")
                || text(video, "field_order").as_deref() != Some("progressive")
            { return None; }
            let clock = match (text(video, "r_frame_rate").as_deref(), text(video, "time_base").as_deref()) {
                (Some("24/1"), Some("1/24")) => Some((24, 1)),
                (Some("24000/1001"), Some("1001/24000")) => Some((24_000, 1001)),
                _ => None,
            };
            Some(clock.and_then(|(num, den)| {
                let count = i128::from(number(video, "duration_ts").filter(|&n| n > 0)?);
                Some((Rational::new(num, den)?, Rational::new(count * den, num)?))
            }))
        })
}

const MAX_PRORES_PICTURE_PACKETS: i64 = 16_384;

fn verify_prores_packets(packets: &str, count: i64, ticks_per_frame: i128) -> Result<(), String> {
    let mut seen = 0;
    for row in packets.lines() {
        let expected = i128::from(seen) * ticks_per_frame;
        let mut fields = row.split(',').map(|field| field.parse::<i128>().ok());
        if seen >= count
            || fields.next() != Some(Some(expected))
            || fields.next() != Some(Some(expected))
            || fields.next() != Some(Some(ticks_per_frame))
            || fields.next().is_some()
        {
            return Err(format!(
                "linked ProRes HQ packet {seen} does not match the exact 24 fps picture clock"
            ));
        }
        seen += 1;
    }
    if seen != count {
        return Err(
            "linked ProRes HQ picture packet count disagrees with the declared extent".into(),
        );
    }
    Ok(())
}

fn prove_prores_cadence(path: &Path, tool: &Path, video: &Value) -> Result<(), String> {
    let count = number(video, "nb_frames")
        .filter(|count| (1..=MAX_PRORES_PICTURE_PACKETS).contains(count))
        .ok_or("linked ProRes HQ requires a declared picture count within the 16,384-packet proof limit")?;
    let base = ratio(video, "time_base")
        .filter(|base| base.num() > 0)
        .ok_or("linked ProRes HQ picture time base is missing")?;
    let denominator = i128::from(base.num()) * 24;
    let ticks_per_frame = i128::from(base.den()) / denominator;
    if ticks_per_frame == 0
        || i128::from(base.den()) % denominator != 0
        || number(video, "duration_ts").map(i128::from) != Some(i128::from(count) * ticks_per_frame)
    {
        return Err("linked ProRes HQ picture extent does not fit its exact 24 fps clock".into());
    }
    let mut command = Command::new(tool);
    command
        .args([
            "-v", "error", "-protocol_whitelist", "file,pipe", "-f", "mov",
            "-select_streams", "v:0", "-show_packets", "-show_entries",
            "packet=pts,dts,duration:packet_side_data=", "-of", "csv=p=0",
        ])
        .arg(path);
    // Read through EOF so an extra or uneven packet after the metadata probe's
    // initial sixteen packets cannot enter the fixed-rate destination route.
    let bytes = bounded_probe(command, std::time::Duration::from_secs(30))
        .map_err(|why| format!("linked ProRes HQ complete picture packet probe failed: {why}"))?;
    let packets = std::str::from_utf8(&bytes)
        .map_err(|_| "linked ProRes HQ packet probe output is not UTF-8")?;
    verify_prores_packets(packets, count, ticks_per_frame)
}

struct OriginalProbe {
    metadata: Value,
    /// Present only after the whole physical picture packet clock was checked.
    prores_color_defaulted: Option<bool>,
}

fn probe(path: &Path, tool: &Path) -> Result<OriginalProbe, String> {
    let mut command = Command::new(tool);
    command
        .args([
            "-v",
            "error",
            "-read_intervals",
            "%+#16",
            "-show_packets",
            "-show_entries",
            ORIGINAL_PROBE_ENTRIES,
            "-of",
            "json",
        ])
        .arg(path);
    let context = |why: String| {
        format!(
            "original stream probe {} using {}: {why}",
            path.display(),
            tool.display()
        )
    };
    let bytes = bounded_probe(command, std::time::Duration::from_secs(30)).map_err(context)?;
    let metadata = graph::json::parse(
        std::str::from_utf8(&bytes).map_err(|_| context("stdout is not UTF-8".into()))?,
    )
    .map_err(&context)?;
    let prores_color_defaulted = match metadata.get("streams") {
        Some(Value::Arr(streams))
            if streams
                .iter()
                .any(|stream| text(stream, "codec_name").as_deref() == Some("prores")) =>
        {
            qualify_container(path, &metadata).map_err(&context)?;
            let defaulted = qualify_prores_hq(path, &metadata, streams).map_err(&context)?;
            prove_prores_cadence(path, tool, &streams[0]).map_err(&context)?;
            Some(defaulted)
        }
        _ => None,
    };
    Ok(OriginalProbe {
        metadata,
        prores_color_defaulted,
    })
}
fn has_aac_priming_packet(probe: &Value, stream: i64) -> bool {
    let Some(Value::Arr(packets)) = probe.get("packets") else { return false; };
    let mut audio = packets.iter().filter(|p| number(p, "stream_index") == Some(stream));
    let (Some(first), Some(second)) = (audio.next(), audio.next()) else { return false; };
    number(first, "pts") == Some(-1024) && number(first, "duration") == Some(1024)
        && number(second, "pts") == Some(0)
        && matches!(first.get("side_data_list"), Some(Value::Arr(side)) if side.as_slice().first().is_some_and(|s| {
            side.len() == 1 && text(s, "side_data_type").as_deref() == Some("Skip Samples")
                && number(s, "skip_samples") == Some(1024) && number(s, "discard_padding") == Some(0)
        }))
}
impl OriginalIndex {
    pub fn scan(facts: &av::SourceProject, options: &OriginalOptions) -> Result<Self, String> {
        Self::scan_for_review(facts, options, &mut || Ok(())).map_err(SourceReadIssue::into_message)
    }
    #[cfg(test)]
    pub(crate) fn scan_with(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        mut inspect: impl FnMut(&Path) -> Result<Value, String>,
    ) -> Result<Self, String> {
        Self::scan_with_control(
            facts,
            options,
            |path| {
                inspect(path).map(|metadata| OriginalProbe {
                    metadata,
                    prores_color_defaulted: None,
                })
            },
            &mut || Ok(()),
        )
        .map_err(SourceReadIssue::into_message)
    }
    pub fn scan_controlled(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::scan_for_review(facts, options, check).map_err(SourceReadIssue::into_message)
    }
    pub(crate) fn scan_for_review(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, SourceReadIssue> {
        Self::scan_with_control(facts, options, |path| probe(path, &options.ffprobe), check)
    }
    fn scan_with_control(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        mut inspect: impl FnMut(&Path) -> Result<OriginalProbe, String>,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, SourceReadIssue> {
        let mut out = Self::default();
        let mut last_probe: Option<(PathBuf, OriginalProbe)> = None;
        let mut used = 0usize;
        let mut pinned = HashMap::<PathBuf, String>::new();
        let adapter_masters: std::collections::HashSet<_> = facts.sequences.iter()
            .flat_map(|s| &s.events)
            .filter(|e| e.motion_adapter_phase.is_some())
            .map(|e| e.event.master)
            .collect();
        for m in &facts.masters {
            check()?;
            if m.originals.iter().all(Option::is_none) {
                continue;
            }
            if m.originals.len() != m.master.tracks.len() || m.originals.iter().any(Option::is_none)
            {
                return Err("mixed or unresolved original descriptor routes in one master".into());
            }
            for (i, raw) in m.originals.iter().enumerate() {
                check()?;
                let raw = raw.as_ref().unwrap();
                let t = &m.master.tracks[i];
                let path = resolve(&raw.locators, &options.root_mappings).map_err(|why| {
                    let saved = raw.locators.first().map_or("no saved path", String::as_str);
                    SourceReadIssue::OriginalLocator(format!("{:?} · {saved}: {why}", m.master.name))
                })?;
                let describe = |why: String| format!("{:?} · {}: {why}", m.master.name, path.display());
                let fresh_probe = last_probe.as_ref().is_none_or(|(p, _)| p != &path);
                if fresh_probe {
                    // Pin bytes on both sides of the probe. Neither mtime nor a post-probe
                    // hash alone establishes which bytes supplied the stream facts.
                    let before = crate::revision::file_revision_checked(&path, check)
                        .map_err(|e| format!("original revision read failed: {e}"))?;
                    if pinned.get(&path).is_some_and(|old| old != &before) {
                        return Err("original content changed between source selections".into());
                    }
                    let observed = inspect(&path).map_err(describe)?;
                    let after = crate::revision::file_revision_checked(&path, check)
                        .map_err(|e| format!("original revision recheck failed: {e}"))?;
                    if before != after {
                        return Err("original content changed during stream probe".into());
                    }
                    if !pinned.contains_key(&path) {
                        used = used
                            .checked_add(
                                2 * (std::mem::size_of::<av::SourceInput>()
                                    + path.as_os_str().len()
                                    + 64),
                            )
                            .ok_or("original revision budget overflow")?;
                        if used > av::MAX_SOURCE_FACT_BYTES {
                            return Err("original revisions exceed 64 MiB".into());
                        }
                        pinned.insert(path.clone(), before.clone());
                        out.original_inputs.push(av::SourceInput {
                            path: path.clone(),
                            revision: before,
                        });
                    }
                    last_probe = Some((path.clone(), observed));
                }
                let physical = &last_probe.as_ref().unwrap().1;
                let v = &physical.metadata;
                let physical_container = qualify_container(&path, v)?;
                let Value::Arr(streams) = v.get("streams").ok_or("probe lacks streams")? else {
                    return Err("probe streams malformed".into());
                };
                if streams.len() > 3 {
                    return Err("original has unqualified additional streams".into());
                }
                let videos: Vec<_> = streams
                    .iter()
                    .filter(|s| text(s, "codec_type").as_deref() == Some("video"))
                    .collect();
                let audios: Vec<_> = streams
                    .iter()
                    .filter(|s| text(s, "codec_type").as_deref() == Some("audio"))
                    .collect();
                if videos.len() > 1
                    || audios.len() > 1
                    || streams.iter().any(|s| {
                        !matches!(text(s, "codec_type").as_deref(), Some("video" | "audio"))
                            && text(s, "codec_tag_string").as_deref() != Some("tmcd")
                    })
                {
                    return Err("original stream selection is ambiguous/unqualified".into());
                }
                let mut picture_codec = "avc1";
                if physical_container == "mov" {
                    let video = videos
                        .first()
                        .ok_or("original movie picture stream absent")?;
                    let defaulted = if text(video, "codec_name").as_deref() == Some("prores") {
                        picture_codec = "apch";
                        if m.master.tracks.len() != 1
                            || raw.descriptor_class != "CDCI"
                            || raw.descriptor_length != t.length
                            || raw.descriptor_rate != 24.0
                            || raw.file_track != Some(1)
                            || !matches!(&t.essence, Some(avid_reader::extract::Essence::Picture {
                                width: 1920, height: 1080, ..
                            }))
                            || m.master.start.as_ref().is_some_and(|tc| tc.frames != 0 || tc.fps != 24 || tc.drop)
                        {
                            return Err(describe("linked ProRes HQ requires a matching saved 1080p24 picture-only descriptor and zero timecode origin".into()).into());
                        }
                        physical.prores_color_defaulted
                            .ok_or_else(|| "linked ProRes HQ has no complete verified picture packet clock".into())
                    } else {
                        qualify_movie(video, audios.first().copied(), adapter_masters.contains(&m.master.mob_id))
                    }.map_err(describe)?;
                    if defaulted && fresh_probe {
                        used = used
                            .checked_add(std::mem::size_of::<String>() + UNTAGGED_COLOR_NOTE.len())
                            .ok_or("original note budget overflow")?;
                        if used > av::MAX_SOURCE_FACT_BYTES {
                            return Err("original notes exceed 64 MiB".into());
                        }
                        out.notes.push(UNTAGGED_COLOR_NOTE.into());
                    }
                }
                let (stream, kind) = if t.kind == avid_reader::extract::KIND_PICTURE {
                    (
                        *videos.first().ok_or("original picture stream absent")?,
                        StreamKind::Video,
                    )
                } else {
                    (
                        *audios.first().ok_or("original audio stream absent")?,
                        StreamKind::Audio,
                    )
                };
                let index = number(stream, "index").ok_or("original stream index absent")?;
                let tb = ratio(stream, "time_base").ok_or("original stream clock absent")?;
                let len = number(stream, "duration_ts")
                    .filter(|n| *n > 0)
                    .ok_or("original stream extent absent")?;
                let start = match number(stream, "start_pts") {
                    Some(n) => n,
                    None if physical_container == "wav" => 0,
                    None => return Err("original stream origin absent".into()),
                };
                if start != 0 {
                    return Err(
                        "nonzero original stream origins require separate qualification".into(),
                    );
                }
                let codec = text(stream, "codec_name").ok_or("original codec absent")?;
                let (container, representation, bits) =
                    match (physical_container, codec.as_str(), kind) {
                        ("mov", "h264", StreamKind::Video) => ("mov", None, None),
                        ("mov", "prores", StreamKind::Video) => ("mov", None, None),
                        ("mov", "aac", StreamKind::Audio) if videos.len() == 1 => {
                            ("mov", Some("aac"), None)
                        }
                        ("wav", "pcm_s16le", StreamKind::Audio) if videos.is_empty() => {
                            ("wav", Some("pcm16"), Some(16))
                        }
                        ("wav", "pcm_s24le", StreamKind::Audio) if videos.is_empty() => {
                            ("wav", Some("pcm24"), Some(24))
                        }
                        _ => return Err("original container/codec route is not qualified".into()),
                    };
                used = used
                    .checked_add(
                        std::mem::size_of::<Selected>()
                            + path.as_os_str().len()
                            + raw.locators.iter().map(String::len).sum::<usize>()
                            + m.id.len()
                            + 256,
                    )
                    .ok_or("original index budget overflow")?;
                if used > av::MAX_SOURCE_FACT_BYTES {
                    return Err("original index exceeds 64 MiB".into());
                }
                let mut selected = Selected {
                    path: path.clone(),
                    stream: OriginalStream {
                        saved_locators: raw.locators.clone(),
                        container: container.into(),
                        stream_index: index,
                        channel_index: None,
                        channels: None,
                        time_base: tb,
                        start_pts: start,
                        duration_ts: len,
                        sample_representation: representation.map(str::to_owned),
                    },
                    kind,
                    codec: if kind == StreamKind::Video {
                        picture_codec.into()
                    } else {
                        codec
                    },
                    raster: None,
                    rate: None,
                    sample_rate: None,
                    bits,
                    duration: Rational::new(len as i128 * tb.num() as i128, tb.den() as i128)
                        .ok_or("original duration overflow")?,
                    motion_audio_origin: None,
                };
                if kind == StreamKind::Video {
                    let rate =
                        ratio(stream, "avg_frame_rate").ok_or("original frame rate absent")?;
                    if text(stream, "codec_tag_string").as_deref() != Some(picture_codec)
                        || Some(rate) != ratio(stream, "r_frame_rate")
                        || index != 0
                        || text(stream, "field_order").as_deref() != Some("progressive")
                        || text(stream, "sample_aspect_ratio").as_deref() != Some("1:1")
                        || number(stream, "nb_frames") != Some(t.length)
                        || av::Rate::from_avid(t.edit_rate).map(|r| (r.num, r.den))
                            != Some((rate.num(), rate.den()))
                    {
                        return Err(
                            "original picture rate/extent/scan qualification mismatch".into()
                        );
                    }
                    if selected.duration
                        != Rational::new(t.length as i128 * rate.den() as i128, rate.num() as i128)
                            .ok_or("original frame duration overflow")?
                    {
                        return Err(
                            "original picture duration disagrees with frame extent/rate".into()
                        );
                    }
                    selected.rate = Some(rate);
                    selected.raster = Some((
                        number(stream, "width").ok_or("original width absent")?,
                        number(stream, "height").ok_or("original height absent")?,
                    ));
                } else {
                    let sr = number(stream, "sample_rate").ok_or("original sample clock absent")?;
                    let channels = number(stream, "channels").ok_or("original channels absent")?;
                    let qualified_priming = representation == Some("aac")
                        && adapter_masters.contains(&m.master.mob_id)
                        && raw.motion_audio_track_start == Some(1)
                        && raw.descriptor_rate == 46.875 && sr == 48000 && channels == 2
                        && len.checked_add(1024) == Some(raw.descriptor_length)
                        && has_aac_priming_packet(v, index);
                    if !matches!(channels, 1 | 2)
                        || raw.channels != Some(channels)
                        || raw.sample_rate != Some(sr as f64)
                        || tb != Rational::new(1, sr as i128).ok_or("invalid sample rate")?
                        || (raw.descriptor_length != len && !qualified_priming)
                        || index != if videos.is_empty() { 0 } else { 1 }
                    {
                        return Err(
                            "original audio descriptor/physical sample-clock mismatch".into()
                        );
                    }
                    if representation == Some("aac") && raw.descriptor_class != "MPGA"
                        || representation != Some("aac")
                            && (raw.descriptor_class != "PCMA"
                                || raw.stored_quantization_bits != bits)
                    {
                        return Err("original audio descriptor representation mismatch".into());
                    }
                    let ch = raw
                        .file_track
                        .ok_or("original source channel route missing")?
                        - 1;
                    if !(0..channels).contains(&ch) {
                        return Err("original source channel route outside physical stream".into());
                    }
                    selected.sample_rate = Some(sr);
                    selected.stream.channels = Some(channels);
                    selected.stream.channel_index = Some(ch);
                    if qualified_priming {
                        selected.motion_audio_origin = raw.motion_audio_track_start;
                    }
                }
                out.tracks.insert((m.id.clone(), i), selected);
            }
        }
        // A reused cached probe still needs a final revision check after all
        // selected tracks/masters have consumed it. Publication rechecks these pins.
        for input in &out.original_inputs {
            if crate::revision::file_revision_checked(&input.path, check)
                .map_err(|e| format!("original final revision read failed: {e}"))?
                != input.revision
            {
                return Err("original content changed before source read completed".into());
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_adapter_priming_needs_observed_skip_packet_and_playable_zero() {
        let raw = r#"{"packets":[{"stream_index":0,"pts":0,"duration":512},
            {"stream_index":1,"pts":-1024,"duration":1024,"side_data_list":[
              {"side_data_type":"Skip Samples","skip_samples":1024,"discard_padding":0}]},
            {"stream_index":1,"pts":0,"duration":1024}]}"#;
        let parse = |s: &str| graph::json::parse(s).unwrap();
        assert!(has_aac_priming_packet(&parse(raw), 1));
        assert!(!has_aac_priming_packet(&parse(raw), 0));
        assert!(!has_aac_priming_packet(&parse(&raw.replace("\"skip_samples\":1024", "\"skip_samples\":0")), 1));
        assert!(!has_aac_priming_packet(&parse(&raw.replace("\"stream_index\":1,\"pts\":0", "\"stream_index\":1,\"pts\":1024")), 1));
    }
    fn picture_profile() -> Value {
        graph::json::parse(r#"{"codec_name":"h264","codec_tag_string":"avc1","profile":"High","level":40,"pix_fmt":"yuv420p","width":1920,"height":1080,"avg_frame_rate":"24/1","r_frame_rate":"24/1","field_order":"progressive","sample_aspect_ratio":"1:1"}"#).unwrap()
    }
    fn audio_profile() -> Value {
        graph::json::parse(r#"{"codec_name":"aac","codec_tag_string":"mp4a","profile":"LC","sample_rate":"48000","channels":2,"channel_layout":"stereo"}"#).unwrap()
    }
    fn set(v: &mut Value, key: &str, value: Value) {
        let Value::Obj(rows) = v else { panic!() };
        if let Some((_, old)) = rows.iter_mut().find(|(k, _)| k == key) {
            *old = value;
        } else {
            rows.push((key.into(), value));
        }
    }
    #[test]
    fn physical_container_family_requires_probe_evidence_matching_the_route() {
        fn probe(name: Value) -> Value {
            Value::Obj(vec![(
                "format".into(),
                Value::Obj(vec![("format_name".into(), name)]),
            )])
        }
        for (suffix, name, expected) in [
            ("wav", "wav", "wav"),
            ("WAV", "wav", "wav"),
            ("mov", "mov", "mov"),
            ("mp4", "mov,mp4,m4a,3gp,3g2,mj2", "mov"),
            ("MOV", "mov,mp4,m4a,3gp,3g2,mj2", "mov"),
        ] {
            assert_eq!(
                qualify_container(
                    Path::new(&format!("/Synthetic/source.{suffix}")),
                    &probe(Value::Str(name.into()))
                )
                .unwrap(),
                expected
            );
        }
        for suffix in ["wav", "mov", "mp4"] {
            let path = PathBuf::from(format!("/Synthetic/source.{suffix}"));
            for name in [
                "matroska,webm",
                "unknown",
                "",
                "mov,matroska",
                "mov,,mp4",
                "wav,mov",
            ] {
                assert!(
                    qualify_container(&path, &probe(Value::Str(name.into()))).is_err(),
                    "accepted {suffix}/{name}"
                );
            }
            for value in [
                Value::Null,
                probe(Value::Null),
                Value::Obj(vec![("format".into(), Value::Obj(vec![]))]),
                Value::Obj(vec![]),
            ] {
                assert!(qualify_container(&path, &value).is_err());
            }
        }
        assert!(qualify_container(
            Path::new("/Synthetic/source.wav"),
            &probe(Value::Str("mov,mp4,m4a,3gp,3g2,mj2".into()))
        )
        .is_err());
        for suffix in ["mov", "mp4"] {
            assert!(qualify_container(
                Path::new(&format!("/Synthetic/source.{suffix}")),
                &probe(Value::Str("wav".into()))
            )
            .is_err());
        }
    }
    #[test]
    fn initial_movie_profile_distinguishes_explicit_color_from_destination_defaults() {
        let base = picture_profile();
        let audio = audio_profile();
        assert_eq!(qualify_movie(&base, Some(&audio), false).unwrap(), true);
        assert_eq!(qualify_movie(&base, None, false).unwrap(), true);
        let mut tagged = base.clone();
        for field in ["color_space", "color_transfer", "color_primaries"] {
            set(&mut tagged, field, Value::Str("bt709".into()));
        }
        set(&mut tagged, "color_range", Value::Str("tv".into()));
        assert_eq!(qualify_movie(&tagged, Some(&audio), false).unwrap(), false);
        for unspecified in ["unknown", "unspecified"] {
            let mut v = tagged.clone();
            set(&mut v, "color_transfer", Value::Str(unspecified.into()));
            assert_eq!(qualify_movie(&v, Some(&audio), false).unwrap(), true);
        }
        let mut v = tagged;
        set(&mut v, "side_data_list", Value::Arr(vec![]));
        set(
            &mut v,
            "tags",
            Value::Obj(vec![("rotate".into(), Value::Str("0".into()))]),
        );
        assert_eq!(qualify_movie(&v, Some(&audio), false).unwrap(), false);
        // The actual command requests every field used by the admission/rotation checks.
        for key in [
            "profile",
            "level",
            "pix_fmt",
            "color_range",
            "color_space",
            "color_transfer",
            "color_primaries",
            "channel_layout",
            "stream_side_data=side_data_type",
            "stream_tags=rotate",
        ] {
            assert!(ORIGINAL_PROBE_ENTRIES.contains(key));
        }
    }
    #[test]
    fn initial_movie_profile_refuses_unqualified_picture_audio_hdr_and_rotation() {
        let base = picture_profile();
        let audio = audio_profile();
        for (field, value) in [
            ("codec_name", Value::Str("hevc".into())),
            ("codec_tag_string", Value::Str("avc3".into())),
            ("profile", Value::Str("High 10".into())),
            ("profile", Value::Str("Main".into())),
            ("profile", Value::Null),
            ("level", Value::Int(41)),
            ("level", Value::Int(-99)),
            ("level", Value::Null),
            ("pix_fmt", Value::Str("yuv420p10le".into())),
            ("pix_fmt", Value::Str("yuv422p".into())),
            ("pix_fmt", Value::Null),
            ("width", Value::Int(3840)),
            ("height", Value::Int(1088)),
            ("avg_frame_rate", Value::Str("24000/1001".into())),
            ("r_frame_rate", Value::Str("25/1".into())),
            ("field_order", Value::Str("tt".into())),
            ("sample_aspect_ratio", Value::Str("4:3".into())),
            ("color_space", Value::Str("bt2020nc".into())),
            ("color_primaries", Value::Str("bt2020".into())),
            ("color_transfer", Value::Str("smpte2084".into())),
            ("color_transfer", Value::Str("arib-std-b67".into())),
            ("color_range", Value::Str("pc".into())),
            ("color_transfer", Value::Null),
            (
                "side_data_list",
                graph::json::parse(r#"[{"side_data_type":"Mastering display metadata"}]"#).unwrap(),
            ),
            (
                "side_data_list",
                graph::json::parse(r#"[{"side_data_type":"Display Matrix"}]"#).unwrap(),
            ),
            ("tags", graph::json::parse(r#"{"rotate":"90"}"#).unwrap()),
            (
                "tags",
                graph::json::parse(r#"{"rotate":"unresolved"}"#).unwrap(),
            ),
            ("tags", graph::json::parse(r#"{"rotate":null}"#).unwrap()),
        ] {
            let mut v = base.clone();
            set(&mut v, field, value);
            assert!(
                qualify_movie(&v, Some(&audio), false).is_err(),
                "accepted {field}: {v:?}"
            );
        }
        for (field, value) in [
            ("codec_name", Value::Str("mp3".into())),
            ("codec_tag_string", Value::Str("unknown".into())),
            ("codec_tag_string", Value::Null),
            ("profile", Value::Str("HE-AAC".into())),
            ("profile", Value::Str("HE-AACv2".into())),
            ("profile", Value::Null),
            ("sample_rate", Value::Str("44100".into())),
            ("sample_rate", Value::Str("96000".into())),
            ("channels", Value::Int(1)),
            ("channels", Value::Int(6)),
            ("channel_layout", Value::Str("unknown".into())),
        ] {
            let mut v = audio.clone();
            set(&mut v, field, value);
            assert!(
                qualify_movie(&base, Some(&v), false).is_err(),
                "accepted audio {field}: {v:?}"
            );
        }
    }
    #[test]
    fn linked_prores_hq_refuses_conflicting_timecode_and_additional_streams() {
        let mov = Path::new("/Synthetic/source.mov");
        let probe = graph::json::parse(r#"{"format":{"tags":{"major_brand":"qt  "}}}"#).unwrap();
        let streams = graph::json::parse(r#"[
            {"index":0,"codec_type":"video","codec_name":"prores","codec_tag_string":"apch","profile":"HQ",
             "pix_fmt":"yuv422p10le","width":1920,"height":1080,"avg_frame_rate":"24/1","r_frame_rate":"24/1",
             "field_order":"progressive","sample_aspect_ratio":"1:1","time_base":"1/12288","duration_ts":122880,
             "tags":{"timecode":"00:00:00:00"}},
            {"index":1,"codec_type":"data","codec_tag_string":"tmcd","nb_frames":"1","avg_frame_rate":"24/1",
             "time_base":"1/12288","duration_ts":122880,"start_pts":0,"tags":{"timecode":"00:00:00:00"}}
        ]"#).unwrap();
        let Value::Arr(streams) = streams else { panic!() };
        assert_eq!(qualify_prores_hq(mov, &probe, &streams).unwrap(), true);
        assert!(qualify_prores_hq(Path::new("/Synthetic/source.mp4"), &probe, &streams).is_err());
        assert!(qualify_prores_hq(mov, &probe, &streams[..1]).is_err());
        for (index, key, value) in [
            (0, "profile", Value::Str("LT".into())),
            (0, "avg_frame_rate", Value::Str("24000/1001".into())),
            (0, "color_transfer", Value::Str("smpte2084".into())),
            (1, "codec_type", Value::Str("audio".into())),
            (1, "start_pts", Value::Int(1)),
            (1, "duration_ts", Value::Int(122879)),
            (1, "tags", graph::json::parse(r#"{"timecode":"00:00:01:00"}"#).unwrap()),
        ] {
            let mut changed = streams.clone();
            set(&mut changed[index], key, value);
            assert!(qualify_prores_hq(mov, &probe, &changed).is_err(), "{key}");
        }
        let mut with_audio = streams.clone();
        with_audio.push(audio_profile());
        assert!(qualify_prores_hq(mov, &probe, &with_audio).is_err());
    }
    #[test]
    fn linked_prores_hq_checks_every_picture_packet() {
        let count = 240;
        let ticks_per_frame = 512;
        let mut rows: Vec<_> = (0..count)
            .map(|i| format!("{0},{0},{ticks_per_frame}", i128::from(i) * ticks_per_frame))
            .collect();
        assert!(verify_prores_packets(&rows.join("\n"), count, ticks_per_frame).is_ok());
        assert!(verify_prores_packets(&rows[..239].join("\n"), count, ticks_per_frame).is_err());
        rows.push("122880,122880,512".into());
        assert!(verify_prores_packets(&rows.join("\n"), count, ticks_per_frame).is_err());
        rows.pop();
        // Two late packets keep the exact overall duration and average rate.
        rows[200] = "102400,102400,528".into();
        rows[201] = "102928,102928,496".into();
        assert!(verify_prores_packets(&rows.join("\n"), count, ticks_per_frame)
            .unwrap_err()
            .contains("packet 200"));
    }
    #[cfg(unix)]
    #[test]
    fn probe_child_timeout_and_stdout_budget_are_bounded_and_reaped() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf partial; exec sleep 5"]);
        let error = bounded_probe(command, Duration::from_millis(40)).unwrap_err();
        assert!(error.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec yes synthetic"]);
        let start = Instant::now();
        let error = bounded_probe(command, Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("exceeds 1 MiB"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf '{\"streams\":[]}'"]);
        assert_eq!(
            bounded_probe(command, Duration::from_secs(1)).unwrap(),
            br#"{"streams":[]}"#
        );
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf refused; exit 3"]);
        assert!(bounded_probe(command, Duration::from_secs(1))
            .unwrap_err()
            .contains("refused"));
    }
    #[test]
    fn foreign_saved_paths_require_explicit_component_mappings() {
        let root = std::env::temp_dir().join(format!("anynle-foreign-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let file = root.join("nested/control.wav");
        std::fs::write(&file, b"synthetic").unwrap();
        for (saved, prefix) in [
            (r"Q:\Recordings\nested\control.wav", r"Q:\Recordings"),
            ("/Volumes/Recorded/nested/control.wav", "/Volumes/Recorded"),
            (
                r"\\synthetic-host\Recordings\nested\control.wav",
                r"\\synthetic-host\Recordings",
            ),
        ] {
            let mappings = [RootMapping {
                saved_root: prefix.into(),
                current_root: root.clone(),
            }];
            assert_eq!(
                resolve(&[saved.into()], &mappings).unwrap(),
                std::fs::canonicalize(&file).unwrap()
            );
            let bad = saved.replace("nested", "../nested");
            assert!(resolve(&[bad], &mappings).is_err());
        }
        assert_ne!(
            saved_parts("/Recorded").unwrap().1,
            saved_parts("/RecordedOther").unwrap().1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exact_locator_mapping_refuses_basename_ambiguity_and_parent_escape() {
        let root =
            std::env::temp_dir().join(format!("anynle-original-path-{}", std::process::id()));
        let a = root.join("saved");
        let b = root.join("mapped");
        std::fs::create_dir_all(a.join("nested")).unwrap();
        std::fs::create_dir_all(b.join("nested")).unwrap();
        let mapped = b.join("nested/source.mp4");
        std::fs::write(&mapped, b"synthetic").unwrap();
        let saved = a.join("nested/source.mp4");
        let mappings = vec![RootMapping {
            saved_root: a.clone(),
            current_root: b.clone(),
        }];
        assert!(resolve(&[saved.to_string_lossy().into_owned()], &[]).is_err());
        assert_eq!(
            resolve(&[saved.to_string_lossy().into_owned()], &mappings).unwrap(),
            std::fs::canonicalize(&mapped).unwrap()
        );
        std::fs::write(&saved, b"other synthetic").unwrap();
        assert!(resolve(&[saved.to_string_lossy().into_owned()], &mappings).is_err());
        assert!(resolve(
            &[a.join("../mapped/nested/source.mp4")
                .to_string_lossy()
                .into_owned()],
            &mappings
        )
        .is_err());
        assert!(resolve(&["source.mp4".into()], &mappings).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
