//! Bounded backend processes and strict FFmpeg/ffprobe stream inspection.
use super::R;
use graph::json::{self, Value};
use prproj::convert::model::TICKS_PER_SECOND as TPS;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const LIMIT: usize = 4 << 20;
fn strv<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    match v.get(k) {
        Some(Value::Str(s)) => Some(s),
        _ => None,
    }
}
pub(super) fn integer(v: &Value, k: &str) -> R<i64> {
    match v.get(k) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Str(s)) => s.parse().map_err(|_| format!("bad {k}")),
        _ => Err(format!("missing {k}")),
    }
}
fn fraction(s: &str) -> R<(i128, i128)> {
    let (a, b) = s.split_once('/').ok_or("missing rational separator")?;
    let n: i128 = a.parse().map_err(|_| "invalid rational")?;
    let d: i128 = b.parse().map_err(|_| "invalid rational")?;
    if n <= 0 || d <= 0 {
        return Err("nonpositive rational".into());
    }
    Ok((n, d))
}
pub(super) fn bounded(mut r: impl Read, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut buf = [0; 8192];
    let mut overflow = false;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let take = n.min(max - out.len());
        out.extend_from_slice(&buf[..take]);
        overflow |= take < n;
    }
    Ok((out, overflow))
}
pub(super) fn run(cmd: &mut Command) -> R<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start backend: {e}"))?;
    let err = child.stderr.take().unwrap();
    let thread = std::thread::spawn(move || bounded(err, 65536));
    let out = bounded(child.stdout.take().unwrap(), LIMIT);
    let status = child.wait().map_err(|e| e.to_string())?;
    let (errors, _) = thread
        .join()
        .map_err(|_| "backend reader panicked")?
        .map_err(|e| e.to_string())?;
    let (bytes, large) = out.map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "backend failed ({status}): {}",
            String::from_utf8_lossy(&errors)
        ));
    }
    if large {
        return Err("backend output exceeds 4 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "backend output is not UTF-8".into())
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stream {
    pub(super) kind: String,
    pub(super) codec: String,
    pub(super) ticks: i64,
    pub(super) packets: i64,
    pub(super) width: i64,
    pub(super) height: i64,
    pub(super) rate: String,
    pub(super) samples: i64,
    pub(super) bits: i64,
    pub(super) channels: i64,
    pub(super) pix: String,
    pub(super) profile: String,
    pub(super) color: Vec<Option<String>>,
}
#[derive(Clone, Debug)]
pub(super) struct Probe {
    pub(super) raw: Value,
    pub(super) streams: Vec<Stream>,
    pub(super) op: String,
    pub(super) tc: Option<String>,
    // Demuxer indexes are distinct from the ordered output essence streams.
    pub(super) stream_indices: Vec<usize>,
    pub(super) references: Vec<(String, String)>,
}
pub(super) const OP_ATOM: &str = "060e2b34040101020d01020110030000";
pub(super) fn umid(s: &str) -> R<String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("missing/malformed MXF file-package UMID".into());
    }
    Ok(s.to_ascii_uppercase())
}
pub(super) fn probe(tool: &Path, path: &Path) -> R<Probe> {
    let raw = json::parse(&run(Command::new(tool)
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mxf",
            "-count_packets",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path))?)?;
    let p = interpret_probe(raw)?;
    if !p.references.is_empty() {
        // FFmpeg creates DATA placeholders for unresolved material-package tracks.
        // Inspect their actual packets as well as tags: real data must never be dropped.
        let packets = run(Command::new(tool)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-f",
                "mxf",
                "-select_streams",
                "d",
                "-show_packets",
                "-show_entries",
                "packet=stream_index",
                "-of",
                "csv=p=0",
            ])
            .arg(path))?;
        if !packets.trim().is_empty() {
            return Err(
                "ancillary data has packets; cannot discard it as a package reference".into(),
            );
        }
        let spans = super::source::essence_spans(path)?;
        let kind = if p.streams[0].kind == "video" {
            0x15
        } else {
            0x16
        };
        if spans.len() != 1 || spans[0].kind != kind {
            return Err(
                "reference-only OP-Atom requires one matching physical essence element".into(),
            );
        }
        let id = avid_reader::mxf::read_identity(path).map_err(|e| e.to_string())?;
        let Some(Value::Arr(raw_streams)) = p.raw.get("streams") else {
            unreachable!()
        };
        let selected = &raw_streams[p.stream_indices[0]];
        let selected_id = selected
            .get("tags")
            .and_then(|t| strv(t, "file_package_umid"))
            .ok_or("selected stream lacks file-package identity")?;
        if id.file_packages != [umid(selected_id)?] {
            return Err("selected stream disagrees with physical MXF file-package identity".into());
        }
    }
    Ok(p)
}

fn interpret_probe(raw: Value) -> R<Probe> {
    let format = raw.get("format").ok_or("probe has no format")?;
    if strv(format, "format_name") != Some("mxf") {
        return Err("source/output is not MXF".into());
    }
    let tags = format.get("tags").ok_or("MXF has no format tags")?;
    let op = strv(tags, "operational_pattern_ul")
        .ok_or("MXF has no operational pattern")?
        .replace('.', "")
        .to_lowercase();
    let mut tc = strv(tags, "timecode").map(str::to_string);
    let Some(Value::Arr(ss)) = raw.get("streams") else {
        return Err("probe has no streams".into());
    };
    if ss.is_empty() || ss.len() > 33 {
        return Err("only one picture and up to 32 mono sounds are supported".into());
    }
    let mut streams = Vec::new();
    let mut stream_indices = Vec::new();
    let mut references = Vec::new();
    let mut package_ids = std::collections::HashSet::new();
    for (index, s) in ss.iter().enumerate() {
        if integer(s, "index")? != index as i64 {
            return Err("noncanonical stream order".into());
        }
        let kind = strv(s, "codec_type")
            .ok_or("stream lacks kind")?
            .to_string();
        if !matches!(kind.as_str(), "video" | "audio") {
            let st = s
                .get("tags")
                .ok_or("unsupported ancillary stream without reference tags")?;
            let reference_kind = strv(st, "data_type").unwrap_or("");
            if op != OP_ATOM
                || kind != "data"
                || strv(s, "codec_name").is_some()
                || !matches!(reference_kind, "video" | "audio")
                || (s.get("nb_read_packets").is_some() && integer(s, "nb_read_packets")? != 0)
            {
                return Err(
                    "unsupported ancillary stream; no stream may be silently discarded".into(),
                );
            }
            let id =
                umid(strv(st, "file_package_umid").ok_or("reference lacks file-package UMID")?)?;
            if !package_ids.insert(id.clone()) {
                return Err("duplicate reference package identity".into());
            }
            references.push((reference_kind.into(), id));
            continue;
        }
        stream_indices.push(index);
        if s.get("start_pts").is_some() && integer(s, "start_pts")? != 0 {
            return Err("nonzero packet origin is not qualified".into());
        }
        let (n, d) = fraction(strv(s, "time_base").ok_or("missing time_base")?)?;
        let units = integer(s, "duration_ts")? as i128;
        let numerator = units
            .checked_mul(n)
            .and_then(|v| v.checked_mul(TPS as i128))
            .ok_or("duration overflow")?;
        if units <= 0 || numerator % d != 0 {
            return Err("duration is not positive exact model ticks".into());
        }
        let ticks = i64::try_from(numerator / d).map_err(|_| "duration overflow")?;
        let packets = integer(s, "nb_read_packets")?;
        if packets <= 0 {
            return Err("empty media stream".into());
        }
        streams.push(Stream {
            kind: kind.clone(),
            codec: strv(s, "codec_name").ok_or("unknown codec")?.into(),
            ticks,
            packets,
            width: integer(s, "width").unwrap_or(0),
            height: integer(s, "height").unwrap_or(0),
            rate: if kind == "video" {
                format!("{d}/{n}")
            } else {
                strv(s, "r_frame_rate").unwrap_or("").into()
            },
            samples: integer(s, "sample_rate").unwrap_or(0),
            bits: integer(s, "bits_per_sample").unwrap_or(0),
            channels: integer(s, "channels").unwrap_or(0),
            pix: strv(s, "pix_fmt").unwrap_or("").into(),
            profile: strv(s, "profile").unwrap_or("").into(),
            color: [
                "sample_aspect_ratio",
                "color_range",
                "color_space",
                "color_transfer",
                "color_primaries",
                "field_order",
            ]
            .iter()
            .map(|k| strv(s, k).map(str::to_string))
            .collect(),
        });
    }
    if !references.is_empty() {
        if streams.len() != 1 {
            return Err("reference placeholders require one OP-Atom essence stream".into());
        }
        let s = &ss[stream_indices[0]];
        let st = s.get("tags").ok_or("selected stream lacks tags")?;
        let selected =
            umid(strv(st, "file_package_umid").ok_or("selected stream lacks file-package UMID")?)?;
        if package_ids.contains(&selected) {
            return Err("reference points to selected essence package".into());
        }
    }
    if op == OP_ATOM && streams.len() == 1 {
        if let Some(stream_tc) = ss[stream_indices[0]]
            .get("tags")
            .and_then(|t| strv(t, "timecode"))
        {
            if tc.as_deref().is_some_and(|t| t != stream_tc) {
                return Err("format and essence-stream timecodes disagree".into());
            }
            tc = Some(stream_tc.into());
        }
    }
    Ok(Probe {
        raw,
        streams,
        op,
        tc,
        stream_indices,
        references,
    })
}
pub(super) fn version(tool: &Path, name: &str) -> R<String> {
    let v = run(Command::new(tool).arg("-version"))?;
    let line = v
        .lines()
        .next()
        .ok_or("missing backend version")?
        .to_string();
    if !line.starts_with(&format!("{name} version 8.1 ")) {
        return Err(format!(
            "this rewrap path requires {name} 8.1; found {line}"
        ));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_probe() -> String {
        format!(
            r#"{{"format":{{"format_name":"mxf","tags":{{"operational_pattern_ul":"{OP_ATOM}"}}}},
          "streams":[
          {{"index":0,"codec_type":"data","tags":{{"data_type":"video","file_package_umid":"0x{}"}}}},
          {{"index":1,"codec_type":"audio","codec_name":"pcm_s24le","time_base":"1/48000",
            "duration_ts":9600,"nb_read_packets":"5","start_pts":0,"channels":1,"bits_per_sample":24,"sample_rate":"48000",
            "tags":{{"file_package_umid":"0x{}","timecode":"01:00:00:00"}}}},
          {{"index":2,"codec_type":"data","tags":{{"data_type":"audio","file_package_umid":"0x{}"}}}}
        ]}}"#,
            "11".repeat(32),
            "22".repeat(32),
            "33".repeat(32)
        )
    }

    #[test]
    fn reference_placeholders_keep_demux_index_and_essence_timecode() {
        let p = interpret_probe(json::parse(&native_probe()).unwrap()).unwrap();
        assert_eq!(p.stream_indices, [1]);
        assert_eq!(p.streams.len(), 1);
        assert_eq!(p.streams[0].kind, "audio");
        assert_eq!(p.tc.as_deref(), Some("01:00:00:00"));
        assert_eq!(
            p.references,
            [
                ("video".into(), "11".repeat(32)),
                ("audio".into(), "33".repeat(32))
            ]
        );
    }

    #[test]
    fn ancillary_and_ambiguous_references_are_refused() {
        let original = native_probe();
        for bad in [
            original.replace(
                "\"codec_type\":\"data\"",
                "\"codec_type\":\"data\",\"nb_read_packets\":1",
            ),
            original.replace(
                "\"codec_type\":\"data\"",
                "\"codec_type\":\"data\",\"codec_name\":\"bin_data\"",
            ),
            original.replace(OP_ATOM, "060e2b34040101010d01020101010900"),
            original.replace("\"data_type\":\"video\"", "\"data_type\":\"data\""),
            original.replace(&"11".repeat(32), &"22".repeat(32)),
            original.replace(&"33".repeat(32), &"11".repeat(32)),
            original.replace(&"33".repeat(32), "bad-umid"),
            original.replace("\"index\":2", "\"index\":3"),
            original.replace("\"start_pts\":0", "\"start_pts\":1"),
            original.replace(
                "\"format_name\":\"mxf\",\"tags\":{",
                "\"format_name\":\"mxf\",\"tags\":{\"timecode\":\"02:00:00:00\",",
            ),
        ] {
            assert!(
                interpret_probe(json::parse(&bad).unwrap()).is_err(),
                "{bad}"
            );
        }
    }
}
