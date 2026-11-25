use crate::job::ReportNote;
use graph::source::{OriginalStream, SourceGraph};
use graph::{Rational, StreamKind, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

pub(crate) const RELINK_SUFFIX: &str = ".anynle-relink.json";

pub(crate) fn paths<'a>(graph: &'a SourceGraph, omitted: &HashSet<&str>) -> HashSet<&'a str> {
    let retained = (!omitted.is_empty()).then(|| graph.items.iter()
        .filter(|item| !omitted.contains(item.id.as_str()))
        .flat_map(|item| item.media.iter().map(|binding| binding.media.as_str()))
        .collect::<HashSet<_>>());
    graph
        .media
        .iter()
        .filter(|m| m.online == Some(false) && retained.as_ref().is_none_or(|ids| ids.contains(m.id.as_str())))
        .flat_map(|m| m.locations.iter().map(String::as_str))
        .collect()
}

pub(crate) fn check_edit_routes(graph: &SourceGraph) -> Result<(), String> {
    if !graph.media.iter().any(|m| m.online == Some(false)) {
        return Ok(());
    }
    if let Some(loss) = graph
        .losses
        .iter()
        .find(|loss| loss.aspect == "source_route")
    {
        return Err(format!(
            "{}: offline output requires a resolved edit route: {}",
            loss.object, loss.reason
        ));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    item: String,
    bin: String,
    name: String,
    source_filename: Option<String>,
    path: String,
    saved_locators: Vec<String>,
    package: Option<String>,
    file_track: Option<i64>,
    reel: Option<String>,
    logging_timecode: Option<String>,
    file_clock: Option<FileClock>,
    origin: Option<String>,
    duration: String,
    kind: String,
    rate: Option<String>,
    samples_per_second: Option<i64>,
    bits: Option<i64>,
    stream: Option<i64>,
    channel: Option<i64>,
    channels: Option<i64>,
    sample_representation: Option<String>,
    frame_count: Option<i64>,
    lane: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileClock {
    count: u64,
    rate: (i64, i64),
    base: u16,
    drop: bool,
}

fn string(value: &Value, name: &str) -> Option<String> {
    match value.get(name)? {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn entries(graph: &SourceGraph, omitted: &HashSet<&str>) -> Vec<Entry> {
    let media: BTreeMap<_, _> = graph
        .media
        .iter()
        .filter(|m| m.online == Some(false))
        .map(|m| (m.id.as_str(), m))
        .collect();
    if media.is_empty() {
        return Vec::new();
    }
    let sidecars: BTreeMap<_, _> = graph.sidecars.iter()
        .filter(|s| matches!(s.adapter.as_str(), "avid.source" | "avid"))
        .map(|s| ((s.adapter.as_str(), s.object.as_str()), &s.data)).collect();
    let items: BTreeMap<_, _> = graph.items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut result = Vec::new();
    for bin in &graph.bins {
        for id in &bin.items {
            if omitted.contains(id.as_str()) {
                continue;
            }
            let Some(item) = items.get(id.as_str()) else {
                continue;
            };
            for track in &item.media {
                let Some(m) = media.get(track.media.as_str()) else {
                    continue;
                };
                let facts = sidecars.get(&("avid.source", m.id.as_str())).copied();
                let text = |name| match facts.and_then(|v| v.get(name)) {
                    Some(Value::Str(s)) => Some(s.clone()),
                    _ => None,
                };
                let int = |name| match facts.and_then(|v| v.get(name)) {
                    Some(Value::Int(n)) => Some(*n),
                    _ => None,
                };
                let original = m.original.as_ref();
                let metadata = sidecars.get(&("avid", id.as_str())).copied();
                let source_filename = original
                    .and_then(|o| o.saved_locators.first())
                    .and_then(|path| path.rsplit(['/', '\\']).next())
                    .map(str::to_owned)
                    .or_else(|| metadata.and_then(|v| string(v, "source_file")));
                result.push(Entry {
                    item: item.id.clone(),
                    bin: bin.name.clone(),
                    name: item.name.clone(),
                    source_filename,
                    path: m.locations[0].clone(),
                    saved_locators: original
                        .map(|o| o.saved_locators.clone())
                        .unwrap_or_default(),
                    package: if original.is_none() {
                        text("file_package")
                    } else {
                        None
                    },
                    file_track: int("file_track"),
                    reel: item.tape.clone(),
                    logging_timecode: metadata.and_then(|v| string(v, "start_timecode")),
                    file_clock: m.clock.as_ref().map(|c| FileClock {
                        count: c.effective_count,
                        rate: (c.rate.num(), c.rate.den()),
                        base: c.display_base,
                        drop: c.drop,
                    }),
                    origin: m
                        .start
                        .map(|t| t.to_string())
                        .or_else(|| original.map(|_| "0".into())),
                    duration: m.duration.to_string(),
                    kind: if m.kind == StreamKind::Video {
                        "picture"
                    } else {
                        "audio"
                    }
                    .into(),
                    rate: m.rate.map(|r| r.to_string()),
                    samples_per_second: m.sample_rate,
                    bits: m.bits,
                    stream: original.map(|o| o.stream_index),
                    channel: original.and_then(|o| o.channel_index).or_else(|| {
                        (original.is_none() && m.kind == StreamKind::Audio).then_some(0)
                    }),
                    channels: original.and_then(|o| o.channels).or_else(|| {
                        (original.is_none() && m.kind == StreamKind::Audio).then_some(1)
                    }),
                    sample_representation: original.and_then(|o| o.sample_representation.clone()),
                    frame_count: if m.kind == StreamKind::Video {
                        original
                            .map(|o| o.duration_ts)
                            .or_else(|| int("descriptor_length"))
                    } else {
                        None
                    },
                    lane: m.stream,
                });
            }
        }
    }
    result
}

pub(crate) fn review_notes(graph: &SourceGraph, omitted: &HashSet<&str>) -> Vec<ReportNote> {
    entries(graph, omitted)
        .into_iter()
        .map(|e| ReportNote {
            severity: "warning".into(),
            object: e.item,
            message: format!(
                "{} / {} · offline {}{} · {}",
                e.bin,
                e.name,
                e.kind,
                e.lane.map(|lane| format!(" A{lane}")).unwrap_or_default(),
                e.saved_locators
                    .first()
                    .or(e.package.as_ref())
                    .unwrap_or(&e.path)
            ),
            why: "Link the verified source in Premiere when it is available.".into(),
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema: String,
    offline: Vec<Entry>,
    online_originals: BTreeSet<String>,
    online_media: BTreeSet<String>,
}

pub(crate) fn report(
    graph: &SourceGraph,
    output: &Path,
    omitted: &HashSet<&str>,
) -> Result<Option<(PathBuf, String)>, String> {
    let offline = entries(graph, omitted);
    if offline.is_empty() {
        return Ok(None);
    }
    let mut filename = output
        .file_name()
        .ok_or("missing project filename")?
        .to_os_string();
    filename.push(RELINK_SUFFIX);
    let online_originals = graph
        .media
        .iter()
        .filter(|m| m.online == Some(true))
        .filter_map(|m| m.original.as_ref()?.saved_locators.first().cloned())
        .collect();
    let online_media = graph
        .media
        .iter()
        .filter(|m| m.online == Some(true))
        .flat_map(|m| m.locations.iter().cloned())
        .collect();
    let text = serde_json::to_string_pretty(&Report {
        schema: "anynle.offline-relink/1".into(),
        offline,
        online_originals,
        online_media,
    })
    .map_err(|e| e.to_string())?;
    if text.len() > 64 << 20 {
        return Err("offline relink report exceeds 64 MiB".into());
    }
    Ok(Some((output.with_file_name(filename), text)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Move {
    original: String,
    destination: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checksum {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checksums {
    schema: String,
    originals: Vec<Checksum>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds the input bound", path.display()));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn rational(value: &str) -> Result<Rational, String> {
    let (n, d) = value.split_once('/').unwrap_or((value, "1"));
    n.parse()
        .ok()
        .zip(d.parse().ok())
        .and_then(|(n, d)| Rational::new(n, d))
        .ok_or_else(|| format!("invalid saved rational {value}"))
}

fn stamp(path: &Path) -> Result<Metadata, String> {
    let m = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_file() || m.len() == 0 {
        return Err(format!("not a nonempty regular file: {}", path.display()));
    }
    Ok(m)
}

fn unchanged(left: &Metadata, right: &Metadata) -> Result<bool, String> {
    let basic = left.len() == right.len()
        && left.modified().map_err(|e| e.to_string())?
            == right.modified().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(basic
            && (left.dev(), left.ino()) == (right.dev(), right.ino())
            && (left.ctime(), left.ctime_nsec()) == (right.ctime(), right.ctime_nsec()))
    }
    #[cfg(not(unix))]
    Ok(basic && left.created().ok() == right.created().ok())
}

fn check_wave_extent(path: &Path, samples: i64, channels: i64, bits: i64) -> Result<(), String> {
    let fail = || "WAV has truncated PCM data or an unsupported RIFF layout".to_owned();
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let mut header = [0; 12];
    file.read_exact(&mut header).map_err(|_| fail())?;
    let little_endian = |bytes: &[u8]| u64::from(u32::from_le_bytes(bytes.try_into().unwrap()));
    if &header[..4] != b"RIFF"
        || &header[8..] != b"WAVE"
        || little_endian(&header[4..8]) + 8 != size
    {
        return Err(fail());
    }
    let expected = u64::try_from(i128::from(samples) * i128::from(channels) * i128::from(bits / 8))
        .map_err(|_| fail())?;
    let mut offset = 12;
    let mut found_data = false;
    for _ in 0..1024 {
        if offset == size && found_data {
            return Ok(());
        }
        if offset + 8 > size {
            break;
        }
        file.seek(SeekFrom::Start(offset)).map_err(|_| fail())?;
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk).map_err(|_| fail())?;
        let bytes = little_endian(&chunk[4..]);
        offset += 8 + bytes + bytes % 2;
        if &chunk[..4] == b"data" {
            if found_data || bytes != expected {
                return Err(fail());
            }
            found_data = true;
        }
    }
    Err(fail())
}

fn atom(
    entry: &Entry, path: &Path, ffprobe: &Path, reason: &mut crate::diagnostics::Reason,
) -> Result<(), String> {
    let identity = avid_reader::mxf::read_identity(path).map_err(|e| e.to_string())?;
    let package = entry
        .package
        .as_ref()
        .ok_or("saved file package is missing")?;
    if identity.file_packages.as_slice() != [package.as_str()] || identity.file_essence_tracks != 1
    {
        *reason = crate::diagnostics::Reason::MxfPackageMismatch;
        return Err(format!("expected exactly one essence track for saved file package {package}; check the sibling"));
    }
    let clock = identity
        .qualified_file_clock()
        .ok_or("physical MXF clock is unresolved")?;
    let expected = entry
        .file_clock
        .as_ref()
        .ok_or("saved file clock is unresolved")?;
    if (
        clock.effective_count,
        clock.rate,
        clock.rounded_base,
        clock.drop,
    ) != (expected.count, expected.rate, expected.base, expected.drop)
    {
        return Err("physical MXF clock differs from the saved file clock".into());
    }
    let [(n, d, count)] = identity.file_tracks.as_slice() else {
        return Err("physical MXF extent is unresolved".into());
    };
    let duration = rational(&entry.duration)?;
    if *n <= 0
        || *d <= 0
        || *count <= 0
        || Rational::new(i128::from(*d) * i128::from(*count), i128::from(*n)) != Some(duration)
    {
        return Err("physical MXF header extent differs from the saved descriptor".into());
    }
    let result = avid_adapter::original::probe_offline_atom(path, ffprobe)?;
    let op = result
        .get("format")
        .and_then(|f| f.get("tags"))
        .and_then(|tags| string(tags, "operational_pattern_ul"));
    if op.as_deref() != Some("060e2b34.04010102.0d010201.10030000") {
        return Err("candidate is outside the OP-Atom profile".into());
    }
    let Some(Value::Arr(streams)) = result.get("streams") else {
        return Err("candidate stream list is missing".into());
    };
    let mut essence = streams
        .iter()
        .filter(|s| string(s, "codec_type").as_deref() != Some("data"));
    let s = essence.next().ok_or("candidate has no essence stream")?;
    let number = |key| match s.get(key) {
        Some(Value::Int(n)) => Some(*n),
        Some(Value::Str(n)) => n.parse().ok(),
        _ => None,
    };
    let str_is = |key, expected| string(s, key).as_deref() == Some(expected);
    let profile = match entry.kind.as_str() {
        "picture" => {
            str_is("codec_type", "video")
                && str_is("codec_name", "dnxhd")
                && str_is("profile", "DNXHR LB")
                && str_is("field_order", "progressive")
                && str_is("sample_aspect_ratio", "1:1")
                && number("width") == Some(1920)
                && number("height") == Some(1080)
                && (*n, *d) == (24000, 1001)
        }
        "audio" => {
            str_is("codec_type", "audio")
                && str_is("codec_name", "pcm_s24le")
                && number("channels") == Some(1)
                && number("bits_per_sample") == Some(24)
                && number("sample_rate") == Some(48_000)
                && (*n, *d) == (48_000, 1)
        }
        _ => false,
    };
    if essence.next().is_some()
        || !profile
        || number("start_pts") != Some(0)
        || number("duration_ts") != Some(*count)
        || string(s, "time_base").map(|v| rational(&v)).transpose()?
            != Rational::new(i128::from(*d), i128::from(*n))
    {
        return Err("candidate MXF essence profile, origin or extent does not match".into());
    }
    if entry.kind == "audio" {
        let bytes = u64::try_from(*count)
            .ok()
            .and_then(|n| n.checked_mul(3))
            .ok_or("saved PCM extent overflow")?;
        if crate::rewrap::pcm_essence_bytes(path)? != bytes {
            return Err("physical MXF PCM payload differs from the saved sample extent".into());
        }
    } else if !crate::rewrap::essence_spans(path)?
        .iter()
        .any(|span| span.kind == 0x15)
    {
        return Err("physical MXF picture payload is absent".into());
    }
    Ok(())
}

fn original(rows: &[&Entry], path: &Path, ffprobe: &Path) -> Result<(), String> {
    let container = if path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("wav"))
    {
        "wav"
    } else {
        "mov"
    };
    let mut expected = Vec::new();
    for row in rows {
        let duration = rational(&row.duration)?;
        let base = if row.kind == "picture" { 24 } else { 48_000 };
        let stream = OriginalStream {
            saved_locators: Vec::new(),
            container: container.into(),
            stream_index: row.stream.ok_or("saved physical stream is unresolved")?,
            channel_index: row.channel,
            channels: row.channels,
            time_base: Rational::new(1, base).unwrap(),
            start_pts: 0,
            duration_ts: duration
                .to_units(base as i64)
                .ok_or("saved original extent is not exact")?,
            sample_representation: row.sample_representation.clone(),
        };
        let kind = match row.kind.as_str() {
            "picture" => StreamKind::Video,
            "audio" => StreamKind::Audio,
            _ => return Err("saved essence kind is unresolved".into()),
        };
        expected.push((stream, kind, duration, row.bits));
    }
    let borrowed: Vec<_> = expected
        .iter()
        .map(|(s, k, d, b)| (s, *k, *d, *b))
        .collect();
    avid_adapter::original::verify_offline_original(path, ffprobe, container, &borrowed)?;
    if container == "wav" {
        let (stream, _, _, bits) = &expected[0];
        check_wave_extent(
            path,
            stream.duration_ts,
            stream.channels.ok_or("missing channel count")?,
            bits.ok_or("missing PCM depth")?,
        )?;
    }
    Ok(())
}

pub(crate) fn verify(
    report: &Path,
    moves: &Path,
    checksums: Option<&Path>,
    ffprobe: &Path,
) -> Result<(), String> {
    verify_mappings(report, moves, checksums, ffprobe, &mut crate::diagnostics::Reason::RelinkRejected,
        |mapping| println!("Verified {} → {}", mapping.original, mapping.destination.display()))
        .map(|_| ())
}

pub(crate) fn verify_diagnostics(report: &Path, moves: &Path, checksums: Option<&Path>, ffprobe: &Path) -> crate::diagnostics::Diagnostics {
    let mut reason = crate::diagnostics::Reason::RelinkRejected;
    let result = verify_mappings(report, moves, checksums, ffprobe, &mut reason, |_| {});
    crate::diagnostics::Diagnostics::relink(result.map_err(|_| reason))
}

fn verify_mappings(
    report: &Path,
    moves: &Path,
    checksums: Option<&Path>,
    ffprobe: &Path,
    reason: &mut crate::diagnostics::Reason,
    mut verified: impl FnMut(&Move),
) -> Result<usize, String> {
    let report: Report = read_json(report, 64 << 20)?;
    let moves: Vec<Move> = read_json(moves, 1 << 20)?;
    if report.schema != "anynle.offline-relink/1" || moves.is_empty() {
        return Err(
            "expected an offline relink report and at least one explicit file mapping".into(),
        );
    }
    let mut files = BTreeMap::<&str, Vec<&Entry>>::new();
    for entry in &report.offline {
        files.entry(&entry.path).or_default().push(entry);
    }
    let mut trusted = BTreeMap::new();
    if let Some(path) = checksums {
        let manifest: Checksums = read_json(path, 1 << 20)?;
        if manifest.schema != "anynle.offline-checksums/1" {
            return Err("expected trusted anynle.offline-checksums/1 manifest".into());
        }
        for file in manifest.originals {
            if file.sha256.len() != 64
                || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || !files.contains_key(file.path.as_str())
                || trusted.insert(file.path, file.sha256).is_some()
            {
                return Err("invalid, unknown or duplicate checksum record".into());
            }
        }
    }
    let mut named = BTreeMap::<String, usize>::new();
    for file in files.values().filter(|v| v[0].package.is_none()) {
        let basename = file[0]
            .source_filename
            .as_ref()
            .ok_or("saved basename is unresolved")?;
        *named.entry(basename.to_lowercase()).or_default() += 1;
    }
    for saved in &report.online_originals {
        if let Some(basename) = saved.rsplit(['/', '\\']).next() {
            *named.entry(basename.to_lowercase()).or_default() += 1;
        }
    }
    let mut selected = BTreeSet::new();
    let mut verified_paths = BTreeMap::new();
    let mut checked = BTreeMap::<PathBuf, Metadata>::new();
    #[cfg(unix)]
    let mut inodes = BTreeSet::new();
    let online: BTreeSet<_> = report
        .online_media
        .iter()
        .filter_map(|p| fs::canonicalize(p).ok())
        .collect();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        inodes.extend(
            online
                .iter()
                .filter_map(|p| fs::metadata(p).ok())
                .map(|m| (m.dev(), m.ino())),
        );
    }
    for mapping in &moves {
        let rows = files.get(mapping.original.as_str()).ok_or_else(|| {
            format!(
                "mapping does not select a saved offline locator: {}",
                mapping.original
            )
        })?;
        let candidate = &mapping.destination;
        if !selected.insert(&mapping.original)
            || !candidate.is_absolute()
            || candidate
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(
                "mappings must be unique and target an absolute path without parent escapes".into(),
            );
        }
        let path = fs::canonicalize(candidate).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                *reason = crate::diagnostics::Reason::RelinkCandidateMissing;
            }
            format!("{}: {e}", candidate.display())
        })?;
        let before = stamp(&path)?;
        if checked.contains_key(&path) || online.contains(&path) {
            return Err(format!(
                "distinct sources map to the same file: {}",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !inodes.insert((before.dev(), before.ino())) {
                return Err(format!(
                    "distinct sources map to the same physical file: {}",
                    path.display()
                ));
            }
        }
        let mut check = || {
            if let Some(package) = &rows[0].package {
                if rows.iter().any(|e| e.package.as_ref() != Some(package)) {
                    return Err("conflicting file packages share the saved path".into());
                }
                for entry in rows {
                    atom(entry, &path, ffprobe, reason)?;
                }
            } else {
                let basename = rows[0]
                    .source_filename
                    .as_deref()
                    .ok_or("saved basename is unresolved")?;
                if candidate.file_name().and_then(|n| n.to_str()) != Some(basename) {
                    return Err("candidate filename differs from the saved original".into());
                }
                if named[&basename.to_lowercase()] > 1 && !trusted.contains_key(&mapping.original) {
                    return Err("same-name originals need trusted pre-move checksums for this offline profile".into());
                }
                original(rows, &path, ffprobe)?;
            }
            if checksums.is_some() {
                let expected = trusted
                    .get(&mapping.original)
                    .ok_or("trusted checksum does not cover this source")?;
                let actual = &crate::source_input::pin(std::slice::from_ref(&path))?[0].revision;
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err("candidate SHA-256 mismatch".into());
                }
            }
            Ok::<_, String>(())
        };
        check().map_err(|e| format!("{} → {}: {e}", mapping.original, path.display()))?;
        verified_paths.insert(&mapping.original, path.clone());
        checked.insert(path, before);
    }
    for (path, before) in &checked {
        let after = stamp(path)?;
        if !unchanged(before, &after)? {
            return Err(format!(
                "candidate changed during verification: {}",
                path.display()
            ));
        }
    }
    for mapping in &moves {
        let resolved = fs::canonicalize(&mapping.destination).map_err(|e| e.to_string())?;
        if !checked.contains_key(&resolved) || resolved != verified_paths[&mapping.original] {
            return Err(format!(
                "candidate path changed during verification: {}",
                mapping.destination.display()
            ));
        }
    }
    for mapping in &moves {
        verified(mapping);
    }
    Ok(moves.len())
}
