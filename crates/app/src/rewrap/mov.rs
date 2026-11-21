//! Bounded ProRes MOV preparation, with discrete interleaved PCM for LT masters.
//! PCM samples are byte-interleaved, never mixed/resampled. Only generated trailing
//! moov metadata is rewritten: media/chunk offsets and compressed picture bytes stay put.
use super::{
    backend::{integer, run, Stream},
    Job, Plan, R,
};
use graph::json::{self, Value};
use graph::{source::SourceGraph, StreamKind};
use prproj::convert::model::{VideoFile, TICKS_PER_SECOND as TPS};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Command,
};

fn string<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    if let Some(Value::Str(s)) = v.get(k) {
        Some(s)
    } else {
        None
    }
}
pub(super) type PixelAspect = (u32, u32);

pub(super) fn validate_source(s: &Stream, raw: &Value, v: &VideoFile) -> R<Option<PixelAspect>> {
    let Some(Value::Arr(streams)) = raw.get("streams") else {
        return Err("missing ProRes streams".into());
    };
    let p = streams
        .iter()
        .find(|s| string(s, "codec_type") == Some("video"))
        .ok_or("missing ProRes picture")?;
    let profile = avid_reader::mxf::ProResProfile::from_fourcc(&v.codec)
        .ok_or("unrecognized ProRes picture profile")?;
    use avid_reader::mxf::ProResProfile;
    let pixel_format_matches = match profile {
        ProResProfile::FourFourFourFour | ProResProfile::FourFourFourFourXq =>
            matches!(s.pix.as_str(), "yuv444p10le" | "yuv444p12le" | "yuva444p10le" | "yuva444p12le"),
        _ => s.pix == "yuv422p10le",
    };
    if s.kind != "video"
        || s.codec != "prores"
        || s.profile != profile.probe_name()
        || !pixel_format_matches
        || s.width != v.width
        || s.height != v.height
        || s.rate != format!("{}/{}", v.rate.num, v.rate.den)
        || string(p, "field_order") != Some("progressive")
    {
        return Err("ProRes MOV requires matching profile, raster and rate with progressive picture".into());
    }
    fn parse_pair(ratio: &str) -> Option<PixelAspect> {
        let (n, d) = ratio.split_once(':')?;
        let (n, d) = (n.parse::<u32>().ok()?, d.parse::<u32>().ok()?);
        (n > 0 && d > 0).then_some((n, d))
    }
    let (n, d) = string(p, "sample_aspect_ratio").and_then(parse_pair)
        .ok_or("ProRes MOV source pixel aspect could not be verified")?;
    Ok((n != d).then_some((n, d)))
}

/// Index saved descriptors by exact graph media locator. Shared locators must
/// agree even when the same file appears under different bin item identities.
pub(super) fn saved_picture_descriptors(graph: Option<&SourceGraph>) -> HashMap<&str, Vec<&Value>> {
    let Some(graph) = graph else { return HashMap::new() };
    let paths: HashMap<_, _> = graph.media.iter()
        .filter(|m| m.kind == StreamKind::Video && m.original.is_none())
        .filter_map(|m| match m.locations.as_slice() {
            [path] => Some((m.id.as_str(), path.as_str())),
            _ => None,
        })
        .collect();
    let mut saved = HashMap::<_, Vec<_>>::new();
    for side in graph.sidecars.iter().filter(|s| s.adapter == "avid.source") {
        if let Some(&path) = paths.get(side.object.as_str()) {
            saved.entry(path).or_default().push(&side.data);
        }
    }
    saved
}

pub(super) fn verify_pixel_aspect(
    video: &VideoFile,
    pixel: PixelAspect,
    identity: &avid_reader::mxf::MxfIdentity,
    saved: &[&Value],
) -> R<()> {
    let fail = || format!("{}: ProRes pixel aspect {}:{} requires matching saved Avid and physical MXF picture geometry",
        video.path, pixel.0, pixel.1);
    let [package] = identity.file_packages.as_slice() else { return Err(fail()) };
    let field = |name| identity.picture_descriptor.iter()
        .find(|(key, _)| *key == name).map(|(_, value)| value.as_slice());
    let dimension = |name| field(name)
        .and_then(|b| b.try_into().ok()).map(u32::from_be_bytes).map(i64::from);
    let Some(bytes) = field("display_aspect").filter(|b| b.len() == 8) else {
        return Err(fail());
    };
    let physical = (
        i64::from(u32::from_be_bytes(bytes[..4].try_into().unwrap())),
        i64::from(u32::from_be_bytes(bytes[4..].try_into().unwrap())),
    );
    let matches_pixel = |(n, d): (i64, i64)| n > 0 && d > 0
        && i128::from(n) * i128::from(video.height) * i128::from(pixel.1)
            == i128::from(d) * i128::from(video.width) * i128::from(pixel.0);
    if dimension("display_width") != Some(video.width)
        || dimension("display_height") != Some(video.height)
        || !matches_pixel(physical) || saved.is_empty()
        || saved.iter().any(|side| {
            let Some(desc) = side.get("descriptor") else { return true };
            let Some(Value::Arr(ratio)) = desc.get("display_aspect") else { return true };
            let [Value::Int(n), Value::Int(d)] = ratio.as_slice() else { return true };
            !string(side, "file_package").is_some_and(|id| id.eq_ignore_ascii_case(package))
                || desc.get("width") != Some(&Value::Int(video.width))
                || desc.get("height") != Some(&Value::Int(video.height))
                || !matches_pixel((*n, *d))
        })
    {
        return Err(fail());
    }
    Ok(())
}
fn atoms(b: &[u8]) -> R<Vec<(usize, usize, [u8; 4])>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        if b.len() - at < 8 {
            return Err("truncated MOV atom".into());
        }
        let n = u32::from_be_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        if n < 8 || n > b.len() - at {
            return Err("unsupported MOV atom bounds".into());
        }
        if out.len() >= 100_000 {
            return Err("MOV atom budget exceeded".into());
        }
        out.push((at, n, b[at + 4..at + 8].try_into().unwrap()));
        at += n;
    }
    Ok(out)
}
fn atom(kind: &[u8; 4], b: &[u8]) -> R<Vec<u8>> {
    let n = u32::try_from(b.len().checked_add(8).ok_or("MOV size overflow")?)
        .map_err(|_| "MOV atom too large")?;
    let mut out = n.to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(b);
    Ok(out)
}
#[derive(Clone, Copy)]
enum Description {
    PackedPcm { channels: u32, bits: u32 },
    ProResAlpha([u8; 4]),
}

fn rewrite(b: &[u8], description: Description, nesting: usize, changed: &mut usize) -> R<Vec<u8>> {
    if nesting > 8 {
        return Err("MOV metadata nesting exceeded".into());
    }
    let mut out = Vec::new();
    for (at, n, kind) in atoms(b)? {
        let body = &b[at + 8..at + n];
        let replacement = if [
            &b"moov"[..],
            &b"trak"[..],
            &b"mdia"[..],
            &b"minf"[..],
            &b"stbl"[..],
        ]
        .contains(&kind.as_slice())
        {
            atom(&kind, &rewrite(body, description, nesting + 1, changed)?)?
        } else if &kind == b"stsd" {
            if body.len() < 8 || u32::from_be_bytes(body[4..8].try_into().unwrap()) != 1 {
                return Err("MOV needs one sample description per track".into());
            }
            let entries = atoms(&body[8..])?;
            if entries.len() != 1 {
                return Err("invalid MOV sample description count".into());
            }
            let (_, _, codec) = entries[0];
            if matches!(description, Description::ProResAlpha(expected) if codec == expected) {
                // QuickTime ImageDescription depth includes the alpha component.
                let mut entry = body[8..].to_vec();
                if entry.len() < 86
                    || !matches!(&entry[82..84], [0, 24 | 32])
                    || entry[84..86] != [0xff, 0xff]
                {
                    return Err("unqualified generated ProRes alpha description".into());
                }
                entry[82..84].copy_from_slice(&32u16.to_be_bytes());
                let mut payload = body[..8].to_vec();
                payload.extend(entry);
                *changed += 1;
                atom(&kind, &payload)?
            } else if let (Description::PackedPcm { channels, bits }, b"sowt" | b"in24" | b"lpcm") =
                (description, &codec)
            {
                let e = &body[8..];
                if e.len() < 36 {
                    return Err("short PCM sample description".into());
                }
                let version = u16::from_be_bytes(e[16..18].try_into().unwrap());
                let expected = if bits == 16 { b"sowt" } else { b"in24" };
                if &codec != expected
                    || version != (if bits == 24 { 1 } else { 0 })
                    || u16::from_be_bytes(e[24..26].try_into().unwrap()) as u32 != channels
                    || u16::from_be_bytes(e[26..28].try_into().unwrap()) != 16
                    || u32::from_be_bytes(e[32..36].try_into().unwrap()) != 48000 << 16
                {
                    return Err("unqualified generated PCM description".into());
                }
                if bits == 24 {
                    if e.len() < 52
                        || e[36..40] != 1u32.to_be_bytes()
                        || e[40..44] != 3u32.to_be_bytes()
                        || e[44..48] != (channels * 3).to_be_bytes()
                        || e[48..52] != 2u32.to_be_bytes()
                    {
                        return Err("unqualified PCM24 packet description".into());
                    }
                    let wave = atoms(&e[52..])?
                        .into_iter()
                        .find(|(_, _, k)| k == b"wave")
                        .ok_or("missing PCM24 wave description")?;
                    let children = &e[52 + wave.0 + 8..52 + wave.0 + wave.1];
                    let endian = atoms(children)?
                        .into_iter()
                        .find(|(_, _, k)| k == b"enda")
                        .ok_or("missing PCM24 endian declaration")?;
                    if endian.1 != 10 || children[endian.0 + 8..endian.0 + 10] != [0, 1] {
                        return Err("PCM24 is not little endian".into());
                    }
                }
                // QuickTime version 2 LPCM SoundDescription, 72-byte struct including size/type.
                let mut v = e[8..16].to_vec();
                v.extend(2u16.to_be_bytes());
                v.extend([0u8; 6]);
                v.extend(3u16.to_be_bytes());
                v.extend(16u16.to_be_bytes());
                v.extend(0xfffeu16.to_be_bytes());
                v.extend(0u16.to_be_bytes());
                v.extend(0x10000u32.to_be_bytes());
                v.extend(72u32.to_be_bytes());
                v.extend(48000f64.to_be_bytes());
                v.extend(channels.to_be_bytes());
                v.extend(0x7f000000u32.to_be_bytes());
                v.extend(bits.to_be_bytes());
                v.extend(12u32.to_be_bytes());
                v.extend((channels * (bits / 8)).to_be_bytes());
                v.extend(1u32.to_be_bytes());
                let mut chan = vec![0u8; 4];
                chan.extend(((147u32 << 16) | channels).to_be_bytes());
                chan.extend([0u8; 8]);
                v.extend(atom(b"chan", &chan)?);
                let mut payload = body[..8].to_vec();
                payload.extend(atom(b"lpcm", &v)?);
                *changed += 1;
                atom(&kind, &payload)?
            } else {
                b[at..at + n].to_vec()
            }
        } else {
            b[at..at + n].to_vec()
        };
        out.extend(replacement);
        if out.len() > 32 << 20 {
            return Err("MOV metadata budget exceeded".into());
        }
    }
    Ok(out)
}
fn fix_descriptions(path: &Path, descriptions: &[Description]) -> R<()> {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut at = 0;
    let mut moov = None;
    let mut mdat = false;
    let mut entries = 0;
    while at < len {
        entries += 1;
        if entries > 32 {
            return Err("unexpected generated MOV top-level atoms".into());
        }
        f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        let mut h = [0; 8];
        f.read_exact(&mut h).map_err(|e| e.to_string())?;
        let mut n = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        if n == 1 {
            let mut x = [0; 8];
            f.read_exact(&mut x).map_err(|e| e.to_string())?;
            n = u64::from_be_bytes(x);
            if n < 16 {
                return Err("invalid extended MOV atom".into());
            }
        }
        if n < 8 || at.checked_add(n).filter(|x| *x <= len).is_none() {
            return Err("invalid generated MOV atom".into());
        }
        if &h[4..] == b"mdat" {
            mdat = true;
        }
        if &h[4..] == b"moov" {
            if !mdat || at + n != len || n > 32 << 20 || moov.is_some() {
                return Err("MOV rewrite requires one bounded moov after all media".into());
            }
            f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
            let mut b = vec![0; n as usize];
            f.read_exact(&mut b).map_err(|e| e.to_string())?;
            moov = Some((at, b));
        }
        at += n;
    }
    let (offset, mut rewritten) = moov.ok_or("missing MOV moov")?;
    for &description in descriptions {
        let mut changed = 0;
        rewritten = rewrite(&rewritten, description, 0, &mut changed)?;
        if changed != 1 {
            return Err(match description {
                Description::PackedPcm { .. } => "MOV needs exactly one interleaved PCM description",
                Description::ProResAlpha(_) => "MOV needs exactly one ProRes alpha description",
            }.into());
        }
    }
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    f.write_all(&rewritten).map_err(|e| e.to_string())?;
    f.set_len(offset + rewritten.len() as u64)
        .map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}
fn pack(plan: &Plan, job: &Job, path: &Path) -> R<(u32, u32)> {
    let count = job.inputs.len() - 1;
    let first = &plan.inputs[job.inputs[1]];
    let bits = first.probe.streams[0].bits as usize;
    let mut lanes = Vec::new();
    let mut samples = None;
    for i in job.inputs.iter().skip(1) {
        let spans = super::source::essence_spans(&plan.inputs[*i].path)?;
        if spans.len() != 1 || spans[0].kind != 0x16 || spans[0].bytes % (bits as u64 / 8) != 0 {
            return Err(
                "MOV interleaving requires one complete clip-wrapped PCM essence per source".into(),
            );
        }
        let frames = spans[0].bytes / (bits as u64 / 8);
        if samples.replace(frames).is_some_and(|n| n != frames) {
            return Err("unequal physical PCM spans".into());
        }
        let mut f = File::open(&plan.inputs[*i].path).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(spans[0].offset))
            .map_err(|e| e.to_string())?;
        lanes.push(f.take(spans[0].bytes));
    }
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let bytes = bits / 8;
    let mut remaining = samples.unwrap();
    let mut blocks = vec![vec![0; 4096 * bytes]; count];
    let mut interleaved = vec![0; 4096 * bytes * count];
    while remaining > 0 {
        let n = remaining.min(4096) as usize;
        for (f, b) in lanes.iter_mut().zip(&mut blocks) {
            f.read_exact(&mut b[..n * bytes])
                .map_err(|e| e.to_string())?;
        }
        for frame in 0..n {
            for lane in 0..count {
                let pos = (frame * count + lane) * bytes;
                interleaved[pos..pos + bytes]
                    .copy_from_slice(&blocks[lane][frame * bytes..(frame + 1) * bytes]);
            }
        }
        out.write_all(&interleaved[..n * bytes * count])
            .map_err(|e| e.to_string())?;
        remaining -= n as u64;
    }
    out.sync_all().map_err(|e| e.to_string())?;
    Ok((count as u32, bits as u32))
}
fn exact_ticks(s: &Value) -> R<i64> {
    let tb = string(s, "time_base").ok_or("missing MOV timebase")?;
    let (n, d) = tb.split_once('/').ok_or("invalid timebase")?;
    let n = n.parse::<i128>().map_err(|_| "invalid timebase")?;
    let d = d.parse::<i128>().map_err(|_| "invalid timebase")?;
    if n <= 0 || d <= 0 {
        return Err("nonpositive MOV timebase".into());
    }
    let t = (integer(s, "duration_ts")? as i128)
        .checked_mul(n)
        .and_then(|v| v.checked_mul(TPS as i128))
        .ok_or("MOV duration overflow")?;
    if t <= 0 || t % d != 0 {
        return Err("inexact MOV duration".into());
    }
    i64::try_from(t / d).map_err(|_| "MOV duration overflow".into())
}
pub(super) fn execute(plan: &Plan, job: &Job, partial: &Path, hashes: &[String]) -> R<Value> {
    let channels = job.inputs.len() - 1;
    let scratch = partial.with_extension("pcm");
    let packed = if channels > 0 && !job.separate_mono {
        Some(pack(plan, job, &scratch)?)
    } else {
        None
    };
    let mut c = Command::new(&plan.ffmpeg);
    c.args([
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "mxf",
        "-i",
    ])
    .arg(&plan.inputs[job.inputs[0]].path);
    if let Some((count, bits)) = packed {
        c.args([
            "-f",
            &format!("s{bits}le"),
            "-ar",
            "48000",
            "-ac",
            &count.to_string(),
            "-i",
        ])
        .arg(&scratch);
    }
    if job.separate_mono {
        for index in job.inputs.iter().skip(1) {
            c.args(["-protocol_whitelist", "file,pipe", "-f", "mxf", "-i"])
                .arg(&plan.inputs[*index].path);
        }
    }
    c.args([
        "-map",
        &format!("0:{}", plan.inputs[job.inputs[0]].probe.stream_indices[0]),
    ]);
    if packed.is_some() {
        c.args(["-map", "1:a:0"]);
    }
    if job.separate_mono {
        for (file, index) in job.inputs.iter().enumerate().skip(1) {
            c.args(["-map", &format!("{file}:{}", plan.inputs[*index].probe.stream_indices[0])]);
        }
    }
    let picture = &plan.inputs[job.inputs[0]];
    if let Some(tail) = &picture.terminal {
        c.args(["-bsf:v:0", &tail.filter()]);
    }
    c.args(["-c", "copy", "-r:v", &format!("{}/{}", job.rate.num, job.rate.den), "-map_metadata", "0"]);
    if let Some(tc) = &job.tc {
        c.args(["-timecode", tc, "-write_tmcd", "1"]);
    } else {
        c.args(["-write_tmcd", "0"]);
    }
    c.args(["-f", "mov"]).arg(partial);
    run(&mut c)?;
    let source = &picture.probe.streams[0];
    let mut descriptions = Vec::new();
    if let Some((count, bits)) = packed {
        descriptions.push(Description::PackedPcm { channels: count, bits });
    }
    if matches!(source.pix.as_str(), "yuva444p10le" | "yuva444p12le") {
        use avid_reader::mxf::ProResProfile;
        let codec = if source.profile == ProResProfile::FourFourFourFour.probe_name() {
            *b"ap4h"
        } else if source.profile == ProResProfile::FourFourFourFourXq.probe_name() {
            *b"ap4x"
        } else {
            return Err("unqualified ProRes alpha profile".into());
        };
        descriptions.push(Description::ProResAlpha(codec));
    }
    if !descriptions.is_empty() {
        fix_descriptions(partial, &descriptions)?;
    }
    let raw = json::parse(&run(Command::new(&plan.ffprobe)
        .args([
            "-v",
            "error",
            "-count_packets",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(partial))?)?;
    let Some(Value::Arr(streams)) = raw.get("streams") else {
        return Err("missing MOV output streams".into());
    };
    let video = streams
        .iter()
        .filter(|s| string(s, "codec_type") == Some("video"))
        .collect::<Vec<_>>();
    let audio = streams
        .iter()
        .filter(|s| string(s, "codec_type") == Some("audio"))
        .collect::<Vec<_>>();
    let data = streams
        .iter()
        .filter(|s| string(s, "codec_type") == Some("data"))
        .collect::<Vec<_>>();
    let packets = picture.terminal.as_ref().map_or(source.packets, |tail| tail.frames);
    if video.len() != 1
        || audio.len() != if job.separate_mono { channels } else { usize::from(channels > 0) }
        || data.len() != usize::from(job.tc.is_some())
        || streams.len() != video.len() + audio.len() + data.len()
    {
        return Err("unexpected MOV stream topology".into());
    }
    let v = video[0];
    if exact_ticks(v)? != source.ticks
        || integer(v, "nb_read_packets")? != packets
        || string(v, "codec_name") != Some("prores")
        || string(v, "profile") != Some(source.profile.as_str())
        || string(v, "pix_fmt") != Some(source.pix.as_str())
        || integer(v, "width")? != source.width
        || integer(v, "height")? != source.height
        || string(v, "r_frame_rate") != Some(source.rate.as_str())
        || v.get("tags").and_then(|t| string(t, "timecode")) != job.tc.as_deref()
        || integer(v, "start_pts")? != 0
    {
        return Err("MOV picture descriptor/clock changed".into());
    }
    for (key, expected) in [
        "sample_aspect_ratio",
        "color_range",
        "color_space",
        "color_transfer",
        "color_primaries",
        "field_order",
    ]
    .iter()
    .zip(&source.color)
    {
        if let Some(expected) = expected {
            if string(v, key) != Some(expected) {
                return Err("MOV color metadata changed".into());
            }
        }
    }
    if let Some(track) = data.first() {
        if string(track, "codec_tag_string") != Some("tmcd")
            || exact_ticks(track)? != source.ticks
            || track.get("tags").and_then(|t| string(t, "timecode")) != job.tc.as_deref()
        {
            return Err("MOV timecode track changed".into());
        }
    }
    let fingerprint = run(Command::new(&plan.ffmpeg)
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(partial)
        .args([
            "-map", "0:v:0", "-c", "copy", "-f", "hash", "-hash", "sha256", "-",
        ]))?;
    if fingerprint.trim() != format!("SHA256={}", hashes[0]) {
        return Err("MOV compressed picture payload changed".into());
    }
    if job.separate_mono {
        for (ordinal, a) in audio.iter().enumerate() {
            let input = &plan.inputs[job.inputs[ordinal + 1]].probe.streams[0];
            let encoding = format!("pcm_s{}le", input.bits);
            if integer(a, "channels")? != 1 || integer(a, "sample_rate")? != input.samples
                || string(a, "codec_name") != Some(encoding.as_str())
                || exact_ticks(a)? != input.ticks || integer(a, "start_pts")? != 0
            {
                return Err(format!("MOV mono stream {ordinal} descriptor or clock changed"));
            }
            let hash = run(Command::new(&plan.ffmpeg).args(["-v", "error", "-nostdin", "-i"])
                .arg(partial).args(["-map", &format!("0:a:{ordinal}"), "-c", "copy", "-f", "hash", "-hash", "sha256", "-"]))?;
            if hash.trim() != format!("SHA256={}", hashes[ordinal + 1]) {
                return Err(format!("MOV mono stream {ordinal} samples changed"));
            }
        }
    }
    if let Some((count, bits)) = packed {
        let a = audio[0];
        if integer(a, "channels")? != count as i64
            || integer(a, "sample_rate")? != 48000
            || string(a, "codec_name") != Some(format!("pcm_s{bits}le").as_str())
            || exact_ticks(a)? != source.ticks
            || integer(a, "start_pts")? != 0
        {
            return Err("MOV PCM descriptor/clock changed".into());
        }
        for (ch, expected) in hashes.iter().skip(1).enumerate() {
            // Pure channel selection; PCM16/24 to the same integer sample format.
            let hash = run(Command::new(&plan.ffmpeg)
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(partial)
                .args([
                    "-map",
                    "0:a:0",
                    "-af",
                    &format!("pan=mono|c0=c{ch}"),
                    "-c:a",
                    &format!("pcm_s{bits}le"),
                    "-f",
                    "hash",
                    "-hash",
                    "sha256",
                    "-",
                ]))?;
            if hash.trim() != format!("SHA256={expected}") {
                return Err(format!("MOV PCM channel {ch} bytes/order changed"));
            }
        }
        // Scratch is generated media, retained if cleanup fails; never turn verification success into a false failure.
        if let Err(e) = fs::remove_file(&scratch) {
            eprintln!(
                "verified MOV; generated PCM scratch retained at {}: {e}",
                scratch.display()
            );
        }
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anamorphic_picture_requires_matching_saved_and_physical_geometry() {
        use super::super::receipts::obj;
        let video = VideoFile {
            path: "/media/picture.mxf".into(),
            codec: "apcn".into(),
            width: 1440,
            height: 1080,
            rate: prproj::convert::model::Fraction::new(24, 1),
            duration: TPS,
            start: 0,
        };
        let identity = avid_reader::mxf::MxfIdentity {
            file_packages: vec!["A1".repeat(32)],
            picture_descriptor: vec![
                ("display_width", 1440u32.to_be_bytes().to_vec()),
                ("display_height", 1080u32.to_be_bytes().to_vec()),
                ("display_aspect", [16u32.to_be_bytes(), 9u32.to_be_bytes()].concat()),
            ],
            ..Default::default()
        };
        let descriptor = |package, width, n, d| obj(vec![
            ("file_package", Value::Str(package)),
            ("descriptor", obj(vec![
                ("width", Value::Int(width)),
                ("height", Value::Int(1080)),
                ("display_aspect", Value::Arr(vec![Value::Int(n), Value::Int(d)])),
            ])),
        ]);
        let saved = descriptor(identity.file_packages[0].clone(), 1440, 32, 18);
        assert!(verify_pixel_aspect(&video, (4, 3), &identity, &[&saved]).is_ok());
        assert!(verify_pixel_aspect(&video, (16, 15), &identity, &[&saved]).is_err());
        assert!(verify_pixel_aspect(&video, (4, 3), &identity, &[]).is_err());
        for bad in [
            descriptor("B2".repeat(32), 1440, 16, 9),
            descriptor(identity.file_packages[0].clone(), 1920, 16, 9),
            descriptor(identity.file_packages[0].clone(), 1440, 4, 3),
        ] {
            assert!(verify_pixel_aspect(&video, (4, 3), &identity, &[&saved, &bad]).is_err());
        }
    }

    #[test]
    fn prores_alpha_rewrite_changes_only_qualified_sample_depth() {
        for codec in [*b"ap4h", *b"ap4x"] {
            let stsd = |depth: u16| {
                let mut entry = vec![0; 78];
                entry[74..76].copy_from_slice(&depth.to_be_bytes());
                entry[76..78].copy_from_slice(&[0xff, 0xff]);
                let mut body = vec![0, 0, 0, 0, 0, 0, 0, 1];
                body.extend(atom(&codec, &entry).unwrap());
                atom(b"stsd", &body).unwrap()
            };
            let mut changed = 0;
            let description = Description::ProResAlpha(codec);
            assert_eq!(rewrite(&stsd(24), description, 0, &mut changed).unwrap(), stsd(32));
            assert_eq!(changed, 1);
            assert!(rewrite(&stsd(16), description, 0, &mut 0).is_err());
            let other_entry = Description::ProResAlpha(*b"apch");
            let mut changed = 0;
            assert_eq!(rewrite(&stsd(24), other_entry, 0, &mut changed).unwrap(), stsd(24));
            assert_eq!(changed, 0);
        }
    }

    #[test]
    fn malformed_metadata_and_unbounded_nesting_refuse_without_panicking() {
        for bad in [
            vec![0; 7],
            vec![0; 8],
            vec![0xff; 16],
            vec![0, 0, 0, 9, b'f', b'r', b'e', b'e'],
        ] {
            assert!(atoms(&bad).is_err());
        }
        let mut nested = atom(b"free", b"").unwrap();
        for _ in 0..10 {
            nested = atom(b"moov", &nested).unwrap();
        }
        assert!(rewrite(&nested, Description::PackedPcm { channels: 8, bits: 24 }, 0, &mut 0).is_err());
    }
    #[test]
    fn metadata_before_media_is_not_relocated_or_modified() {
        let path = std::env::temp_dir().join(format!(
            "anynle-mov-refusal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bytes = [
            atom(b"ftyp", b"qt  ").unwrap(),
            atom(b"moov", b"").unwrap(),
            atom(b"mdat", b"preserve").unwrap(),
        ]
        .concat();
        std::fs::write(&path, &bytes).unwrap();
        assert!(fix_descriptions(&path, &[Description::PackedPcm { channels: 8, bits: 24 }]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }
}
