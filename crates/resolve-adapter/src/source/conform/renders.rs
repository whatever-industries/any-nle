//! Verify decoded picture independently of the saved media locator.
use super::{Sample, Timeline};
use crate::Result;
use graph::Rational;
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const LIMIT: u64 = 8 << 20;
const DEADLINE: Duration = Duration::from_secs(60);

pub struct Renders<'a> {
    pub reference: &'a Path,
    pub candidate: &'a Path,
    pub ffmpeg: &'a Path,
    pub ffprobe: &'a Path,
}

pub(super) fn compare(
    inputs: Renders<'_>,
    left: &Timeline<'_>,
    right: &Timeline<'_>,
    samples: &[Sample<'_>],
) -> Json {
    let frames: BTreeSet<_> = samples
        .iter()
        .filter(|s| s.picture_eligible)
        .map(|s| s.record_frame)
        .collect();
    if frames.is_empty() {
        return json!({"status": "inconclusive", "reason": "no eligible picture samples"});
    }
    let run = || -> Result<Json> {
        let a = inspect(inputs.reference, &inputs, left)
            .map_err(|error| format!("reference render: {error}"))?;
        let b = inspect(inputs.candidate, &inputs, right)
            .map_err(|error| format!("candidate render: {error}"))?;
        if a.profile != b.profile {
            return Err("render picture profiles differ".into());
        }
        let mut different = false;
        let mut rows = Vec::new();
        for frame in frames {
            let index = usize::try_from(frame)?;
            let a = a
                .hashes
                .get(index)
                .ok_or("reference render is missing a sample frame")?;
            let b = b
                .hashes
                .get(index)
                .ok_or("candidate render is missing a sample frame")?;
            different |= a != b;
            rows.push(json!({
                "record_frame": frame,
                "status": if a == b { "match" } else { "different" },
                "reference_sha256": a,
                "candidate_sha256": b,
            }));
        }
        let status = if different {
            "different"
        } else if samples
            .iter()
            .any(|s| !s.picture_eligible && s.status != "excluded_dissolve")
        {
            "inconclusive"
        } else {
            "match"
        };
        Ok(json!({"status": status, "samples": rows}))
    };
    match run() {
        Ok(result) => result,
        Err(error) => json!({"status": "inconclusive", "reason": error.to_string()}),
    }
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct Stream {
    avg_frame_rate: String,
    time_base: String,
    duration_ts: i64,
    #[serde(flatten)]
    profile: Profile,
}

#[derive(Deserialize, PartialEq)]
struct Profile {
    codec_name: String,
    profile: Option<String>,
    width: u32,
    height: u32,
    pix_fmt: String,
    sample_aspect_ratio: Option<String>,
    field_order: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
}

#[derive(Deserialize)]
struct Frame {
    best_effort_timestamp: i64,
    width: u32,
    height: u32,
    pix_fmt: String,
}

struct Decoded {
    profile: Profile,
    hashes: Vec<String>,
}

fn inspect(path: &Path, inputs: &Renders<'_>, timeline: &Timeline<'_>) -> Result<Decoded> {
    let path = fs::canonicalize(path)?;
    let initial = fs::metadata(&path)?;
    if !initial.is_file() {
        return Err("expected a regular MOV render".into());
    }
    let count = timeline
        .extent
        .filter(|&n| n > 0)
        .ok_or("unknown full timeline extent")?;
    let rate = timeline.sequence.rate;
    let entries = concat!(
        "stream=avg_frame_rate,time_base,duration_ts,codec_name,profile,width,height,pix_fmt,",
        "sample_aspect_ratio,field_order,color_range,color_space,color_transfer,color_primaries",
        ":frame=best_effort_timestamp,width,height,pix_fmt",
    );
    let probe = output(
        Command::new(inputs.ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file",
                "-f",
                "mov",
                "-select_streams",
                "v",
                "-show_entries",
                entries,
                "-of",
                "json",
            ])
            .arg(&path),
    )?;
    let Probe { streams, frames } = serde_json::from_slice(&probe)?;
    let [stream]: [Stream; 1] = streams
        .try_into()
        .map_err(|_| "expected exactly one render video stream")?;
    let profile = stream.profile;
    if fraction(&stream.avg_frame_rate) != Some(rate) {
        return Err("render rate differs from the exact timeline rate".into());
    }
    if profile.width == 0
        || profile.height == 0
        || profile.pix_fmt.is_empty()
        || !profile
            .pix_fmt
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || timeline
            .sequence
            .frame_size
            .is_some_and(|size| size != (i64::from(profile.width), i64::from(profile.height)))
    {
        return Err("render raster or pixel format is unqualified".into());
    }
    let time_base = fraction(&stream.time_base).ok_or("invalid render time base")?;
    let step = Rational::new(
        i128::from(rate.den()) * i128::from(time_base.den()),
        i128::from(rate.num()) * i128::from(time_base.num()),
    )
    .filter(|step| step.num() > 0 && step.den() == 1)
    .ok_or("render timestamps cannot represent the exact frame grid")?
    .num();
    if frames.len() != usize::try_from(count)?
        || step.checked_mul(count) != Some(stream.duration_ts)
    {
        return Err("render extent does not cover the full timeline".into());
    }
    for (index, frame) in frames.iter().enumerate() {
        if step.checked_mul(i64::try_from(index)?) != Some(frame.best_effort_timestamp)
            || (frame.width, frame.height) != (profile.width, profile.height)
            || frame.pix_fmt != profile.pix_fmt
        {
            return Err(
                "render has nonzero start, irregular frame timestamps, or a changing profile"
                    .into(),
            );
        }
    }
    let inverse = format!("{}/{}", rate.den(), rate.num());
    let pixel_format = format!("+{}", profile.pix_fmt);
    let raw = output(
        Command::new(inputs.ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-xerror",
                "-err_detect",
                "explode",
                "-noautorotate",
                "-protocol_whitelist",
                "file",
                "-f",
                "mov",
                "-i",
            ])
            .arg(&path)
            .args([
                "-map",
                "0:v:0",
                "-an",
                "-sn",
                "-dn",
                "-pix_fmt",
                &pixel_format,
                "-enc_time_base",
                &inverse,
                "-fps_mode",
                "passthrough",
                "-f",
                "framehash",
                "-hash",
                "sha256",
                "-",
            ]),
    )?;
    let hashes = read_hashes(&raw, &inverse, &profile, frames.len())?;
    let final_meta = fs::metadata(path)?;
    if final_meta.len() != initial.len() || final_meta.modified()? != initial.modified()? {
        return Err("render changed while it was being checked".into());
    }
    Ok(Decoded { profile, hashes })
}

fn fraction(text: &str) -> Option<Rational> {
    let (a, b) = text.split_once('/')?;
    let (a, b) = (a.parse::<i64>().ok()?, b.parse::<i64>().ok()?);
    (a > 0 && b > 0)
        .then(|| Rational::from_units(a, b))
        .flatten()
}

fn read_hashes(raw: &[u8], inverse: &str, profile: &Profile, count: usize) -> Result<Vec<String>> {
    let text = std::str::from_utf8(raw)?;
    let dimensions = format!("#dimensions 0: {}x{}", profile.width, profile.height);
    let time_base = format!("#tb 0: {inverse}");
    if [&"#hash: SHA256".to_owned(), &dimensions, &time_base]
        .iter()
        .any(|expected| !text.lines().any(|line| line == expected.as_str()))
    {
        return Err("decoded frame hash profile is inconsistent".into());
    }
    let mut hashes = Vec::new();
    for line in text.lines().filter(|s| !s.starts_with('#')) {
        let mut fields = line.split(',').map(str::trim);
        let index = hashes.len();
        if fields.next() != Some("0")
            || fields.next().and_then(|s| s.parse::<usize>().ok()) != Some(index)
            || fields.next().and_then(|s| s.parse::<usize>().ok()) != Some(index)
            || fields.next() != Some("1")
            || fields
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .is_none_or(|size| size == 0)
        {
            return Err("decoded frame hash rows are off-grid".into());
        }
        let hash = fields
            .next()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("invalid decoded frame hash")?;
        if fields.next().is_some() || index >= count {
            return Err("decoded frame hashes exceed the full timeline extent".into());
        }
        hashes.push(hash.to_owned());
    }
    if hashes.len() != count {
        return Err("decoded frame hashes do not cover the full timeline".into());
    }
    Ok(hashes)
}

fn output(command: &mut Command) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + DEADLINE;
    let pipe = child
        .stdout
        .take()
        .expect("stdout was configured as a pipe");
    let (send, receive) = mpsc::sync_channel(1);
    let reader = thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe.take(LIMIT + 1).read_to_end(&mut bytes).map(|_| bytes);
        let _ = send.send(result);
    });
    let result = (|| -> Result<_> {
        let _reader = reader?;
        let bytes = receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "render tool did not complete its output within the deadline")??;
        if bytes.len() as u64 > LIMIT {
            return Err("render tool exceeded its output limit".into());
        }
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(bytes)
                } else {
                    Err("render tool failed".into())
                };
            }
            if Instant::now() >= deadline {
                return Err("render tool exceeded its time limit".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
