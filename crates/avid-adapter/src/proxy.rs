//! Verified Avid proxy inputs. Graph facts describe a binding; preparation never writes media.
use super::{av, index::MediaIndex, original, Observed};
use graph::{Rational, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub const ADAPTER: &str = "avid.proxy";
const FRAME_BYTES: i64 = 188_416;
const MAX_FRAMES: i64 = 8192;
// SHA-256 of the observed 16 KiB all-zero DNxHD terminal item.
const ZERO_TAIL: &str = "SHA256:4fe7b59af6de3b665b67788cc2f99892ab827efae3a467342b3bb4e3bc8e5bfe";

#[derive(Clone)]
pub struct Binding {
    picture: PathBuf,
    pub audio: Option<[PathBuf; 2]>,
    pub path: PathBuf,
    bits: Option<i64>,
    frames: i64,
    start: i64,
    ffprobe: PathBuf,
    ffmpeg: PathBuf,
}

fn cmd(tool: &Path, kind: &str, path: &Path) -> Command {
    let mut c = Command::new(tool);
    if kind == "ffmpeg" {
        c.args([
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path);
    } else {
        c.args(["-v", "error", "-protocol_whitelist", "file,pipe"])
            .arg(path);
    }
    c
}
fn probe_packets(path: &Path, tool: &Path, frames: i64, mov: bool) -> Result<Vec<String>, String> {
    let mut c = cmd(tool, "ffprobe", path);
    c.args([
        "-select_streams",
        "v:0",
        "-show_packets",
        "-show_entries",
        "packet=pts,size,data_hash:packet_side_data=",
        "-show_data_hash",
        "sha256",
        "-of",
        "csv=p=0",
    ]);
    let out = original::bounded_probe(c, Duration::from_secs(30))?;
    let out = std::str::from_utf8(&out).map_err(|e| e.to_string())?;
    let mut hashes = vec![];
    let mut tail = false;
    for (i, row) in out.lines().enumerate() {
        let mut f = row.split(',');
        let pts = f.next().and_then(|v| v.parse::<i64>().ok());
        let size = f.next().and_then(|v| v.parse::<i64>().ok());
        let hash = f.next().ok_or("missing proxy packet digest")?;
        if f.next().is_some() || tail {
            return Err("unexpected proxy packet fields or terminal data".into());
        }
        let expected = i as i64 * if mov { 1001 } else { 1 };
        if pts != Some(expected) {
            return Err("proxy picture packet clock is not exact".into());
        }
        if (i as i64) < frames
            && size == Some(FRAME_BYTES)
            && hash.starts_with("SHA256:")
            && hash.len() == 71
        {
            hashes.push(hash.to_owned());
        } else if !mov && i as i64 == frames && size == Some(16384) && hash == ZERO_TAIL {
            tail = true;
        } else {
            return Err("proxy picture has an unqualified essence or terminal packet".into());
        }
    }
    if hashes.len() as i64 != frames {
        return Err("proxy picture frame count is incomplete".into());
    }
    Ok(hashes)
}
fn verify_rewrapped_metadata(
    path: &Path,
    tool: &Path,
    frames: i64,
    start: i64,
    bits: i64,
) -> Result<(), String> {
    let mut command = cmd(tool, "ffprobe", path);
    command.args(["-show_entries",
        "stream=index,codec_name,codec_type,codec_tag_string,channels,channel_layout,bits_per_sample,sample_rate,time_base,start_pts,duration_ts,width,height,field_order,sample_aspect_ratio:stream_tags=timecode",
        "-of","json"]);
    let bytes = original::bounded_probe(command, Duration::from_secs(30))?;
    let json = graph::json::parse(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)?;
    let Some(Value::Arr(streams)) = json.get("streams") else {
        return Err("proxy MOV lacks streams".into());
    };
    let [video, audio, tc] = streams.as_slice() else {
        return Err("proxy MOV must contain picture, one stereo stream, and timecode".into());
    };
    let num = original::number;
    let text = original::text;
    let expected_tc = super::tc(start, 24);
    if text(video, "codec_tag_string").as_deref() != Some("AVdn")
        || num(video, "width") != Some(1920)
        || num(video, "height") != Some(1080)
        || text(video, "sample_aspect_ratio").as_deref() != Some("1:1")
        || text(video, "field_order")
            .is_some_and(|v| !matches!(v.as_str(), "unknown" | "progressive"))
        || num(video, "start_pts") != Some(0)
        || num(video, "duration_ts") != Some(frames * 1001)
        || text(video, "time_base").as_deref() != Some("1/24000")
        || text(tc, "codec_tag_string").as_deref() != Some("tmcd")
        || num(tc, "start_pts") != Some(0)
        || num(tc, "duration_ts") != Some(frames * 1001)
        || text(tc, "time_base").as_deref() != Some("1/24000")
        || [video, tc].iter().any(|s| {
            s.get("tags").and_then(|t| text(t, "timecode")).as_deref() != Some(&expected_tc)
        })
        || num(audio, "channels") != Some(2)
        || num(audio, "bits_per_sample") != Some(bits)
        || text(audio, "channel_layout").as_deref() != Some("stereo")
        || text(audio, "codec_name").as_deref()
            != Some(if bits == 16 { "pcm_s16le" } else { "pcm_s24le" })
        || num(audio, "sample_rate") != Some(48000)
        || num(audio, "start_pts") != Some(0)
        || num(audio, "duration_ts") != Some(frames * 2002)
        || text(audio, "time_base").as_deref() != Some("1/48000")
    {
        return Err(
            "verified proxy MOV does not preserve picture and PCM clocks, raster or channel layout"
                .into(),
        );
    }
    Ok(())
}

fn pcm_digest(path: &Path, tool: &Path, pan: Option<i64>, bits: i64) -> Result<Vec<u8>, String> {
    let mut c = cmd(tool, "ffmpeg", path);
    c.args(["-map", "0:a:0"]);
    if let Some(channel) = pan {
        c.args(["-af", &format!("pan=mono|c0=c{channel}")]);
    }
    c.args([
        "-c:a",
        if bits == 16 { "pcm_s16le" } else { "pcm_s24le" },
        "-f",
        "hash",
        "-hash",
        "sha256",
        "-",
    ]);
    original::bounded_probe(c, Duration::from_secs(60))
}

pub struct Attachment {
    pub path: PathBuf,
    pub bits: Option<i64>,
    pub rate: Rational,
    pub frames: i64,
    pub start: i64,
}

impl Attachment {
    pub fn from_graph(g: &graph::source::SourceGraph, item: &str) -> Result<Option<Self>, String> {
        let Some(data) = g
            .sidecars
            .iter()
            .find(|s| s.adapter == ADAPTER && s.object == item)
            .map(|s| &s.data)
        else {
            return Ok(None);
        };
        let str_value = |name| match data.get(name) {
            Some(Value::Str(s)) => Ok(s.clone()),
            _ => Err(format!("invalid proxy {name}")),
        };
        let num = |name| match data.get(name) {
            Some(Value::Int(n)) => Ok(*n),
            _ => Err(format!("invalid proxy {name}")),
        };
        // Graph documents may reference prepared media, but never carry executable paths.
        let path = PathBuf::from(str_value("path")?);
        let frames = num("frames")?;
        let start = num("start")?;
        let bits = match data.get("bits") {
            Some(Value::Null) => None,
            Some(Value::Int(16)) => Some(16),
            Some(Value::Int(24)) => Some(24),
            _ => return Err("invalid proxy sample depth".into()),
        };
        if !path.is_absolute()
            || !(1..=MAX_FRAMES).contains(&frames)
            || !(0..2_073_600).contains(&start)
            || path.extension().and_then(|s| s.to_str())
                != Some(if bits.is_some() { "mov" } else { "mxf" })
        {
            return Err("invalid proxy path, extent, or origin".into());
        }
        Ok(Some(Self {
            path,
            bits,
            rate: Rational::new(24000, 1001).unwrap(),
            frames,
            start,
        }))
    }
}

impl Binding {
    pub fn rewrap_sources(&self) -> Option<Vec<PathBuf>> {
        self.audio.as_ref().map(|audio| {
            std::iter::once(self.picture.clone()).chain(audio.iter().cloned()).collect()
        })
    }

    pub(crate) fn fact(&self) -> Value {
        super::obj(vec![
            ("path", super::s(self.path.to_str().unwrap())),
            ("frames", Value::Int(self.frames)),
            ("start", Value::Int(self.start)),
            ("bits", self.bits.map(Value::Int).unwrap_or(Value::Null)),
        ])
    }

    pub(crate) fn verify(
        master: &av::SourceMaster,
        original: &original::Selected,
        bits: Option<i64>,
        index: &MediaIndex,
        options: &original::OriginalOptions,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(Self, Vec<av::SourceInput>), String> {
        let tracks = &master.master.tracks;
        let clock = original
            .column_timecode
            .as_ref()
            .ok_or("proxy requires a qualified original clock")?;
        let frames = tracks[0].length;
        if clock.rate
            != (av::Rate {
                num: 24000,
                den: 1001,
            })
            || !(1..=MAX_FRAMES).contains(&frames)
        {
            return Err("direct proxy profile requires 23.976 and at most 8192 frames".into());
        }
        let mut pins = vec![];
        for (n, t) in tracks.iter().enumerate() {
            let id = t.file_mob.umid_hex();
            let path = index
                .files
                .get(&id)
                .ok_or("missing or ambiguous Avid proxy file package")?;
            let revision =
                super::revision::file_revision_checked(path, check).map_err(|e| e.to_string())?;
            let physical = avid_reader::mxf::read_identity(path).map_err(|e| e.to_string())?;
            if physical.file_packages.as_slice() != [id] || physical.file_essence_tracks != 1 {
                return Err(
                    "Avid proxy file package or essence origin is not unique and zero-based".into(),
                );
            }
            let probe = original::probe_offline_atom(path, &options.ffprobe)?;
            let Some(Value::Arr(streams)) = probe.get("streams") else {
                return Err("missing Avid proxy streams".into());
            };
            let mut essence = streams
                .iter()
                .filter(|s| original::text(s, "codec_type").as_deref() != Some("data"));
            let s = essence
                .next()
                .filter(|_| essence.next().is_none())
                .ok_or("Avid proxy must have one physical essence stream per MXF")?;
            let text = |key| original::text(s, key);
            let num = |key| original::number(s, key);
            if probe
                .get("format")
                .and_then(|f| f.get("tags"))
                .and_then(|t| original::text(t, "operational_pattern_ul"))
                .as_deref()
                != Some("060e2b34.04010102.0d010201.10030000")
                || num("start_pts") != Some(0)
            {
                return Err("Avid proxy is not the qualified zero-origin OP-Atom profile".into());
            }
            let exact_clock = physical.qualified_file_clock().is_some_and(|c| {
                c.rate == (24000, 1001)
                    && c.rounded_base == 24
                    && !c.drop
                    && c.source_offset == 0
                    && c.effective_count == clock.count as u64
            });
            if !exact_clock {
                return Err(format!(
                    "Avid proxy sibling {n} disagrees with the original's exact file clock"
                ));
            }
            if n == 0 {
                if !matches!(
                    &t.essence,
                    Some(avid_reader::extract::Essence::Picture {
                        resolution_id: 1253,
                        ..
                    })
                ) || text("codec_name").as_deref() != Some("dnxhd")
                    || text("profile").as_deref() != Some("DNXHD")
                    || physical.picture_essence_coding
                        != Some([6, 14, 43, 52, 4, 1, 1, 10, 4, 1, 2, 2, 113, 19, 0, 0])
                    || num("duration_ts") != Some(frames)
                    || num("width") != Some(1920)
                    || num("height") != Some(1080)
                    || text("time_base").as_deref() != Some("1001/24000")
                    || text("sample_aspect_ratio").as_deref() != Some("1:1")
                    || text("field_order").as_deref() != Some("progressive")
                {
                    return Err("Avid picture is outside the verified 1080p DNxHD LB proxy profile or its clock disagrees".into());
                }
                probe_packets(path, &options.ffprobe, frames, false)?;
            } else {
                let bits = bits.ok_or("unexpected Avid proxy audio")?;
                if !matches!(
                    t.essence,
                    Some(avid_reader::extract::Essence::Sound { bits: saved, .. }) if saved == bits
                ) {
                    return Err(format!("Avid proxy A{n} saved PCM depth differs from the original"));
                }
                if master.offsets.get(n) != Some(&Observed::Known(0))
                    || num("channels") != Some(1)
                    || num("sample_rate") != Some(48000)
                    || num("bits_per_sample") != Some(bits)
                    || num("duration_ts") != Some(frames * 2002)
                    || text("time_base").as_deref() != Some("1/48000")
                    || text("codec_name").as_deref()
                        != Some(if bits == 16 { "pcm_s16le" } else { "pcm_s24le" })
                {
                    return Err(format!("Avid proxy A{n} does not match the corresponding original PCM channel and clock"));
                }
                if pcm_digest(path, &options.ffmpeg, None, bits)?
                    != pcm_digest(&original.path, &options.ffmpeg, Some(n as i64 - 1), bits)?
                {
                    return Err(format!("Avid proxy A{n} does not match the corresponding original PCM channel"));
                }
            }
            if super::revision::file_revision_checked(path, check).map_err(|e| e.to_string())?
                != revision
            {
                return Err("Avid proxy input changed during verification".into());
            }
            pins.push(av::SourceInput {
                path: path.clone(),
                revision,
            });
        }
        let picture = pins[0].path.clone();
        let (path, audio) = if bits.is_some() {
            let dir = options
                .proxy_rewrap_dir
                .as_ref()
                .ok_or("stereo requires opt-in proxy rewrap")?;
            let dir =
                std::fs::canonicalize(dir).map_err(|e| format!("proxy output folder: {e}"))?;
            if !dir.is_dir() {
                return Err("proxy output folder is not a directory".into());
            }
            let path = dir.join(format!("{}-proxy.mov", tracks[0].file_mob.umid_hex()));
            (path, Some([pins[1].path.clone(), pins[2].path.clone()]))
        } else {
            (picture.clone(), None)
        };
        if path.to_str().is_none() {
            return Err("proxy path is not UTF-8".into());
        }
        if path == original.path {
            return Err("Avid proxy output occupies the camera original locator".into());
        }
        Ok((
            Self {
                picture,
                audio,
                path,
                bits,
                frames,
                start: clock.count,
                ffprobe: options.ffprobe.clone(),
                ffmpeg: options.ffmpeg.clone(),
            },
            pins,
        ))
    }

    pub fn estimated_bytes(&self) -> u64 {
        self.audio.as_ref().map_or(0, |_| {
            self.frames as u64 * (FRAME_BYTES as u64 + 2002 * 6) + (1 << 20)
        })
    }

    /// Execute only after review. Stage beside the destination and publish exclusively.
    fn execute(&self) -> Result<(), String> {
        let Some(audio) = &self.audio else {
            return Ok(());
        };
        let bits = self.bits.ok_or("missing proxy PCM depth")?;
        check_vacant(&self.path)?;
        let expected = probe_packets(&self.picture, &self.ffprobe, self.frames, false)?;
        let root = self.path.parent().ok_or("missing proxy parent")?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let staging = root.join(format!(".anynle-proxy-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&staging).map_err(|e| e.to_string())?;
        let staged = staging.join("staged.mov");
        let result = (|| {
            let mut c = cmd(&self.ffmpeg, "ffmpeg", &self.picture);
            c.arg("-i")
                .arg(&audio[0])
                .arg("-i")
                .arg(&audio[1])
                .args([
                    "-map",
                    "0:v:0",
                    "-filter_complex",
                    "[1:a:0][2:a:0]join=inputs=2:channel_layout=stereo:map=0.0-FL|1.0-FR[a]",
                    "-map",
                    "[a]",
                    "-map_metadata",
                    "-1",
                    "-c:v",
                    "copy",
                    "-r:v",
                    "24000/1001",
                    "-tag:v",
                    "AVdn",
                    "-bsf:v",
                    &format!("noise=drop=gte(n\\,{})", self.frames),
                    "-c:a",
                    if bits == 16 { "pcm_s16le" } else { "pcm_s24le" },
                    "-video_track_timescale",
                    "24000",
                    "-timecode",
                    &super::tc(self.start, 24),
                    "-write_tmcd",
                    "1",
                    "-f",
                    "mov",
                    "-n",
                ])
                .arg(&staged);
            // Drop only a terminal item that the independently qualified packet scan allows.
            original::bounded_probe(c, Duration::from_secs(120))?;
            if probe_packets(&staged, &self.ffprobe, self.frames, true)? != expected {
                return Err("proxy MOV changed compressed picture payloads".into());
            }
            verify_rewrapped_metadata(&staged, &self.ffprobe, self.frames, self.start, bits)?;
            for (channel, source) in audio.iter().enumerate() {
                if pcm_digest(&staged, &self.ffmpeg, Some(channel as i64), bits)?
                    != pcm_digest(source, &self.ffmpeg, None, bits)?
                {
                    return Err("proxy MOV changed PCM samples or channel order".into());
                }
            }
            std::fs::hard_link(&staged, &self.path)
                .map_err(|e| format!("exclusive proxy publication: {e}"))?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_dir(&staging);
        result
    }
}

fn check_vacant(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("proxy output {}: {e}", path.display())),
        Ok(_) => Err(format!(
            "proxy output already occupied: {}; choose another folder",
            path.display()
        )),
    }
}

fn distinct(bindings: &[Binding]) -> Result<std::collections::BTreeMap<&Path, &Binding>, String> {
    let mut paths = std::collections::BTreeMap::<&Path, &Binding>::new();
    for b in bindings.iter().filter(|b| b.audio.is_some()) {
        if let Some(previous) = paths.insert(&b.path, b) {
            if (&b.picture, &b.audio, b.frames, b.start, b.bits)
                != (
                    &previous.picture,
                    &previous.audio,
                    previous.frames,
                    previous.start,
                    previous.bits,
                )
            {
                return Err(format!(
                    "different Avid sources claim proxy destination {}",
                    b.path.display()
                ));
            }
        }
    }
    Ok(paths)
}

/// All output paths and the required space are checked before the first write.
pub fn review(bindings: &[Binding]) -> Result<u64, String> {
    let mut per_dir = std::collections::BTreeMap::<&Path, u64>::new();
    let mut total = 0u64;
    for (path, b) in distinct(bindings)? {
        check_vacant(path)?;
        let dir = path.parent().ok_or("missing proxy output folder")?;
        let bytes = b.estimated_bytes();
        total = total
            .checked_add(bytes)
            .ok_or("proxy media estimate overflows")?;
        *per_dir.entry(dir).or_default() += bytes;
    }
    for (dir, needed) in per_dir {
        if fs2::available_space(dir).map_err(|e| format!("proxy folder {}: {e}", dir.display()))?
            < needed
        {
            return Err(format!(
                "proxy folder {} requires at least {needed} available bytes",
                dir.display()
            ));
        }
    }
    Ok(total)
}

/// Retain already verified media if a later path fails.
pub fn execute_all(bindings: &[Binding]) -> Result<(), String> {
    execute_observed(bindings, &mut |_| {})
}

pub fn execute_observed(
    bindings: &[Binding],
    published_path: &mut dyn FnMut(&Path),
) -> Result<(), String> {
    review(bindings)?;
    let mut published = vec![];
    for (path, b) in distinct(bindings)? {
        b.execute().map_err(|e| {
            format!(
                "proxy {}: {e}; retained proxy outputs: {published:?}",
                path.display()
            )
        })?;
        published.push(path);
        published_path(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occupied_proxy_destination_cannot_publish_or_reuse_an_incumbent() {
        let root = std::env::temp_dir().join(format!(
            "anynle-proxy-review-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("existing.mov");
        std::fs::write(&path, b"existing").unwrap();
        let binding = Binding {
            picture: root.join("absent.mxf"),
            audio: Some([root.join("a.mxf"), root.join("b.mxf")]),
            path: path.clone(),
            bits: Some(16),
            frames: 96,
            start: 0,
            ffprobe: PathBuf::from("fake"),
            ffmpeg: PathBuf::from("fake"),
        };
        let err = execute_all(&[binding]).unwrap_err();
        assert!(err.contains("already occupied"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
