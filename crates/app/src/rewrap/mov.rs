//! Bounded ProRes MOV preparation, with discrete interleaved PCM for LT masters.
//! PCM samples are byte-interleaved, never mixed/resampled. Only generated trailing
//! moov metadata is rewritten: media/chunk offsets and compressed picture bytes stay put.
use super::{
    backend::{integer, run, Stream},
    Job, Plan, R,
};
use graph::json::{self, Value};
use prproj::convert::model::{VideoFile, TICKS_PER_SECOND as TPS};
use std::{
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
pub(super) fn validate_source(s: &Stream, raw: &Value, v: &VideoFile) -> R<()> {
    let Some(Value::Arr(streams)) = raw.get("streams") else {
        return Err("missing ProRes streams".into());
    };
    let p = streams
        .iter()
        .find(|s| string(s, "codec_type") == Some("video"))
        .ok_or("missing ProRes picture")?;
    if s.kind != "video"
        || s.codec != "prores"
        || s.profile != if v.codec == "apch" { "HQ" } else { "LT" }
        || s.pix != "yuv422p10le"
        || s.width != 1920
        || s.height != 1080
        || v.width != 1920
        || v.height != 1080
        || v.rate.num != 24
        || v.rate.den != 1
        || s.rate != "24/1"
        || string(p, "sample_aspect_ratio") != Some("1:1")
        || string(p, "field_order") != Some("progressive")
    {
        return Err("ProRes MOV requires progressive square-pixel 1920x1080 at 24/1 with the qualified picture profile".into());
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
fn rewrite(b: &[u8], channels: u32, bits: u32, depth: usize, changed: &mut usize) -> R<Vec<u8>> {
    if depth > 8 {
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
            atom(&kind, &rewrite(body, channels, bits, depth + 1, changed)?)?
        } else if &kind == b"stsd" {
            if body.len() < 8 || u32::from_be_bytes(body[4..8].try_into().unwrap()) != 1 {
                return Err("MOV needs one sample description per track".into());
            }
            let entries = atoms(&body[8..])?;
            if entries.len() != 1 {
                return Err("invalid MOV sample description count".into());
            }
            let (_, _, codec) = entries[0];
            if [&b"sowt"[..], &b"in24"[..], &b"lpcm"[..]].contains(&codec.as_slice()) {
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
fn fix_description(path: &Path, channels: u32, bits: u32) -> R<()> {
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
    let (offset, b) = moov.ok_or("missing MOV moov")?;
    let mut changed = 0;
    let new = rewrite(&b, channels, bits, 0, &mut changed)?;
    if changed != 1 {
        return Err("MOV needs exactly one interleaved PCM description".into());
    }
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    f.write_all(&new).map_err(|e| e.to_string())?;
    f.set_len(offset + new.len() as u64)
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
    let packed = if channels > 0 {
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
    c.args([
        "-map",
        &format!("0:{}", plan.inputs[job.inputs[0]].probe.stream_indices[0]),
    ]);
    if packed.is_some() {
        c.args(["-map", "1:a:0"]);
    }
    let picture = &plan.inputs[job.inputs[0]];
    if let Some(tail) = &picture.terminal {
        c.args(["-bsf:v:0", &tail.filter()]);
    }
    c.args(["-c", "copy", "-r:v", "24", "-map_metadata", "0"]);
    if let Some(tc) = &job.tc {
        c.args(["-timecode", tc, "-write_tmcd", "1"]);
    } else {
        c.args(["-write_tmcd", "0"]);
    }
    c.args(["-f", "mov"]).arg(partial);
    run(&mut c)?;
    if let Some((count, bits)) = packed {
        fix_description(partial, count, bits)?;
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
    let source = &picture.probe.streams[0];
    let packets = picture.terminal.as_ref().map_or(source.packets, |tail| tail.frames);
    if video.len() != 1
        || audio.len() != usize::from(channels > 0)
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
        || string(v, "pix_fmt") != Some("yuv422p10le")
        || integer(v, "width")? != 1920
        || integer(v, "height")? != 1080
        || string(v, "sample_aspect_ratio") != Some("1:1")
        || string(v, "r_frame_rate") != Some("24/1")
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
        assert!(rewrite(&nested, 8, 24, 0, &mut 0).is_err());
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
        assert!(fix_description(&path, 8, 24).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }
}
