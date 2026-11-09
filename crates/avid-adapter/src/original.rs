//! Exact saved-locator/root-map lookup and bounded read-only physical-stream probes.
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
const ORIGINAL_PROBE_ENTRIES:&str="stream=index,codec_type,codec_name,codec_tag_string,profile,level,pix_fmt,width,height,avg_frame_rate,r_frame_rate,sample_aspect_ratio,field_order,color_range,color_space,color_transfer,color_primaries,sample_rate,channels,channel_layout,bits_per_sample,bits_per_raw_sample,time_base,start_pts,duration_ts,nb_frames:stream_side_data=side_data_type:stream_tags=rotate:format=format_name";
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

/// Initial movie qualification is intentionally the observed picture/codec slice.
/// A missing color declaration is a destination default, not a source BT.709 fact.
fn qualify_movie(video: &Value, audio: Option<&Value>) -> Result<bool, String> {
    if text(video, "codec_name").as_deref() != Some("h264")
        || text(video, "codec_tag_string").as_deref() != Some("avc1")
        || text(video, "profile").as_deref() != Some("High")
        || number(video, "level") != Some(40)
        || text(video, "pix_fmt").as_deref() != Some("yuv420p")
    {
        return Err("original picture profile is unqualified: requires H.264 High level 4.0 avc1/yuv420p; Hi10 and wider profiles are not admitted".into());
    }
    let rate = Some(Rational::integer(24));
    if number(video, "width") != Some(1920)
        || number(video, "height") != Some(1080)
        || ratio(video, "avg_frame_rate") != rate
        || ratio(video, "r_frame_rate") != rate
        || text(video, "field_order").as_deref() != Some("progressive")
        || text(video, "sample_aspect_ratio").as_deref() != Some("1:1")
    {
        return Err("original picture geometry/rate is unqualified: requires square-pixel progressive 1920x1080 at 24 fps".into());
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
fn probe(path: &Path, tool: &Path) -> Result<Value, String> {
    let mut command = Command::new(tool);
    command
        .args([
            "-v",
            "error",
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
    graph::json::parse(
        std::str::from_utf8(&bytes).map_err(|_| context("stdout is not UTF-8".into()))?,
    )
    .map_err(context)
}
impl OriginalIndex {
    pub fn scan(facts: &av::SourceProject, options: &OriginalOptions) -> Result<Self, String> {
        Self::scan_with(facts, options, |path| probe(path, &options.ffprobe))
    }
    pub(crate) fn scan_with(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        inspect: impl FnMut(&Path) -> Result<Value, String>,
    ) -> Result<Self, String> {
        Self::scan_with_control(facts, options, inspect, &mut || Ok(()))
    }
    pub fn scan_controlled(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::scan_with_control(facts, options, |path| probe(path, &options.ffprobe), check)
    }
    fn scan_with_control(
        facts: &av::SourceProject,
        options: &OriginalOptions,
        mut inspect: impl FnMut(&Path) -> Result<Value, String>,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        let mut out = Self::default();
        let mut last_probe: Option<(PathBuf, Value)> = None;
        let mut used = 0usize;
        let mut pinned = HashMap::<PathBuf, String>::new();
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
                let path = resolve(&raw.locators, &options.root_mappings)?;
                let fresh_probe = last_probe.as_ref().is_none_or(|(p, _)| p != &path);
                if fresh_probe {
                    // Pin bytes on both sides of the probe. Neither mtime nor a post-probe
                    // hash alone establishes which bytes supplied the stream facts.
                    let before = crate::revision::file_revision_checked(&path, check)
                        .map_err(|e| format!("original revision read failed: {e}"))?;
                    if pinned.get(&path).is_some_and(|old| old != &before) {
                        return Err("original content changed between source selections".into());
                    }
                    let observed = inspect(&path)?;
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
                let v = &last_probe.as_ref().unwrap().1;
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
                if physical_container == "mov" {
                    let video = videos
                        .first()
                        .ok_or("original movie picture stream absent")?;
                    let defaulted = qualify_movie(video, audios.first().copied())?;
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
                        "avc1".into()
                    } else {
                        codec
                    },
                    raster: None,
                    rate: None,
                    sample_rate: None,
                    bits,
                    duration: Rational::new(len as i128 * tb.num() as i128, tb.den() as i128)
                        .ok_or("original duration overflow")?,
                };
                if kind == StreamKind::Video {
                    let rate =
                        ratio(stream, "avg_frame_rate").ok_or("original frame rate absent")?;
                    if text(stream, "codec_tag_string").as_deref() != Some("avc1")
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
                    if !matches!(channels, 1 | 2)
                        || raw.channels != Some(channels)
                        || raw.sample_rate != Some(sr as f64)
                        || tb != Rational::new(1, sr as i128).ok_or("invalid sample rate")?
                        || raw.descriptor_length != len
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
        assert_eq!(qualify_movie(&base, Some(&audio)).unwrap(), true);
        assert_eq!(qualify_movie(&base, None).unwrap(), true);
        let mut tagged = base.clone();
        for field in ["color_space", "color_transfer", "color_primaries"] {
            set(&mut tagged, field, Value::Str("bt709".into()));
        }
        set(&mut tagged, "color_range", Value::Str("tv".into()));
        assert_eq!(qualify_movie(&tagged, Some(&audio)).unwrap(), false);
        for unspecified in ["unknown", "unspecified"] {
            let mut v = tagged.clone();
            set(&mut v, "color_transfer", Value::Str(unspecified.into()));
            assert_eq!(qualify_movie(&v, Some(&audio)).unwrap(), true);
        }
        let mut v = tagged;
        set(&mut v, "side_data_list", Value::Arr(vec![]));
        set(
            &mut v,
            "tags",
            Value::Obj(vec![("rotate".into(), Value::Str("0".into()))]),
        );
        assert_eq!(qualify_movie(&v, Some(&audio)).unwrap(), false);
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
                qualify_movie(&v, Some(&audio)).is_err(),
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
                qualify_movie(&base, Some(&v)).is_err(),
                "accepted audio {field}: {v:?}"
            );
        }
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
