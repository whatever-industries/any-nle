//! End-to-end byte checks for `convert::writer::write_project` (Rust port Phase 4).
//!
//! 1. `golden_corpus`: every model of the golden corpus (`converter/local/golden`, made by
//!    `converter/scripts/make_golden.py` with the Python writer under
//!    `seeded_ids("any-nle-golden")`) is written with `SeededUuids::new("any-nle-golden")` to
//!    `converter/local/golden/out-rust/<name>/project.prproj` (same depth as Python's `out/`, so
//!    relative media paths match). The decompressed XML and the gzip bytes must equal Python's,
//!    and the report (notes + unverified) must equal `manifest.json`. Skips when the corpus is
//!    absent.
//! 2. `in_repo_models`: the same check for the small models of `tests/data/convert_writer/`
//!    (expected output from `tools/expected_convert_writer.py`), so the writer is covered
//!    without the corpus.
//!
//! The one allowed difference is where the writer embeds its own output location (the LUT
//! `.cube` path in the Lumetri payloads of colour-corrected edits): the Rust output directory
//! name is substituted with Python's in the XML text and inside the decoded payloads (UTF-8 and
//! UTF-16LE), the payloads re-encoded and their BinaryHashes recomputed, and then everything
//! must be identical. The fields that needed it are printed.
//! 3. Regression tests from the review: Python `repr` of every non-printable boundary, I/O
//!    errors with Python's texts and exit classes, huge tick values without overflow panics,
//!    the CLI's usage text and exit codes.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use prproj::base64::b64encode;
use prproj::convert::model::{load_model, parse_json, Json};
use prproj::convert::writer::{write_project, Report};
use prproj::payload::binary_hash;
use prproj::xml::{decompress_prproj, PremiereDocument};
use prproj::SeededUuids;

const SEED: &str = "any-nle-golden";

/// A Rust output brought to Python's output location.
struct Normalized {
    xml: Vec<u8>,
    gzip: Vec<u8>,
    /// Where the output location appeared (empty when the bytes were identical as written).
    fields: Vec<String>,
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Option<Vec<u8>> {
    if from.is_empty() || !data.windows(from.len()).any(|w| w == from) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    Some(out)
}

/// Substitute the output directory `from` with `to` everywhere in a written project.
fn normalize(written: &[u8], from: &str, to: &str) -> Normalized {
    let (xml, _) = decompress_prproj(written).unwrap();
    if !xml.windows(from.len()).any(|w| w == from.as_bytes()) && !contains_in_payloads(written, from) {
        return Normalized { xml, gzip: written.to_vec(), fields: Vec::new() };
    }
    let mut doc = PremiereDocument::from_bytes(written).unwrap();
    let mut fields = Vec::new();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let nodes: Vec<_> = doc.tree.iter(doc.root).collect();
    for &e in &nodes {
        let Some(old_hash) = doc.tree.get(e, "BinaryHash").map(str::to_string) else { continue };
        let text = doc.tree[e].text_or_empty().to_string();
        if text.trim().is_empty() {
            continue;
        }
        let payload = doc.payload(e).unwrap().unwrap();
        let mut changed = payload.clone();
        let mut encodings = Vec::new();
        if let Some(next) = replace_bytes(&changed, from.as_bytes(), to.as_bytes()) {
            changed = next;
            encodings.push("UTF-8");
        }
        if let Some(next) = replace_bytes(&changed, &utf16(from), &utf16(to)) {
            changed = next;
            encodings.push("UTF-16LE");
        }
        if encodings.is_empty() {
            continue;
        }
        let suffix = &text[text.trim_end().len()..];
        let new_hash = binary_hash(&changed);
        doc.tree[e].text = Some(b64encode(&changed) + suffix);
        let owner = doc.tree.parent(e).map(|p| object_label(&doc, p)).unwrap_or_default();
        fields.push(format!("{owner}/{} payload ({})", doc.tree.tag(e), encodings.join(", ")));
        renamed.insert(old_hash, new_hash);
    }
    for &e in &nodes {
        if let Some(new) = doc.tree.get(e, "BinaryHash").and_then(|h| renamed.get(h)).cloned() {
            doc.tree.set(e, "BinaryHash", new);
        }
        let element = &mut doc.tree[e];
        if let Some(t) = element.text.as_mut().filter(|t| t.contains(from)) {
            *t = t.replace(from, to);
            fields.push(format!("<{}> text", element.tag));
        }
        if let Some(t) = element.tail.as_mut().filter(|t| t.contains(from)) {
            *t = t.replace(from, to);
            fields.push(format!("<{}> tail", element.tag));
        }
        for (k, v) in element.attrs.iter_mut().filter(|(_, v)| v.contains(from)) {
            *v = v.replace(from, to);
            fields.push(format!("<{}> attribute {k}", element.tag));
        }
    }
    doc.invalidate_payloads();
    Normalized { xml: doc.to_xml_bytes().unwrap(), gzip: doc.to_bytes().unwrap(), fields }
}

fn contains_in_payloads(written: &[u8], from: &str) -> bool {
    let doc = PremiereDocument::from_bytes(written).unwrap();
    let payloads = doc.index_payloads().unwrap();
    payloads.values().any(|p| {
        p.windows(from.len()).any(|w| w == from.as_bytes()) || {
            let u = utf16(from);
            p.windows(u.len()).any(|w| w == u.as_slice())
        }
    })
}

/// `<Tag ObjectID="n">` of the element's top-level object, plus the path down to `e`.
fn object_label(doc: &PremiereDocument, e: prproj::NodeId) -> String {
    let mut chain = vec![doc.tree.tag(e).to_string()];
    let mut node = e;
    while let Some(parent) = doc.tree.parent(node) {
        if parent == doc.root {
            break;
        }
        node = parent;
        chain.push(doc.tree.tag(node).to_string());
    }
    chain.reverse();
    let id = doc.tree.get(node, "ObjectID").map(|i| format!(" ObjectID={i}")).unwrap_or_default();
    let name = doc.tree.findtext(node, ".//Name").ok().flatten().map(|n| format!(" ({n})")).unwrap_or_default();
    format!("{}{id}{name}", chain.join("/"))
}

fn first_difference(a: &[u8], b: &[u8]) -> String {
    let a = String::from_utf8_lossy(a);
    let b = String::from_utf8_lossy(b);
    for (n, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            let cut = |s: &str| s.chars().take(240).collect::<String>();
            return format!("line {}:\n  rust:   {}\n  python: {}", n + 1, cut(x), cut(y));
        }
    }
    format!("line counts differ: rust {} python {}", a.lines().count(), b.lines().count())
}

/// Files of `<dir>/<stem> LUTs`, name -> bytes.
fn lut_files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(" LUTs")) {
            for f in std::fs::read_dir(&path).unwrap().flatten() {
                let key = format!("{}/{}", path.file_name().unwrap().to_string_lossy(), f.file_name().to_string_lossy());
                out.insert(key, std::fs::read(f.path()).unwrap());
            }
        }
    }
    out
}

fn report_lines(report: &Report) -> Vec<String> {
    report.notes.iter().chain(&report.unverified).cloned().collect()
}

/// Write `model_path` to `out_dir/project.prproj` and compare with Python's `expected_dir`.
/// Returns (verdict, location fields, problems).
fn check_one(model_path: &Path, out_dir: &Path, expected_dir: &Path, expected_notes: &[String], from: &str, to: &str) -> (String, Vec<String>, Vec<String>) {
    let mut problems = Vec::new();
    if out_dir.exists() {
        // our own output from an earlier run: only project.prproj and its LUT folder
        for entry in std::fs::read_dir(out_dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(name == "project.prproj" || name == "project LUTs", "unexpected file in {}: {name}", out_dir.display());
        }
        std::fs::remove_dir_all(out_dir).unwrap();
    }
    std::fs::create_dir_all(out_dir).unwrap();
    let out = out_dir.join("project.prproj");
    let model = load_model(model_path).unwrap_or_else(|e| panic!("{}: {e}", model_path.display()));
    let report = match write_project(&model, &out, &mut SeededUuids::new(SEED)) {
        Ok(r) => r,
        Err(e) => return ("ERROR".into(), Vec::new(), vec![format!("write_project failed: {e}")]),
    };
    if report_lines(&report) != expected_notes {
        problems.push(format!("report differs:\n  rust:   {:?}\n  python: {:?}", report_lines(&report), expected_notes));
    }
    let written = std::fs::read(&out).unwrap();
    let expected_gzip = std::fs::read(expected_dir.join("project.prproj")).unwrap();
    let (expected_xml, _) = decompress_prproj(&expected_gzip).unwrap();
    let normalized = normalize(&written, from, to);
    if normalized.xml != expected_xml {
        problems.push(format!("XML differs at {}", first_difference(&normalized.xml, &expected_xml)));
    } else if common::GZIP_BYTES_GUARANTEED && normalized.gzip != expected_gzip {
        problems.push("gzip bytes differ (XML identical)".into());
    }
    let (rust_luts, python_luts) = (lut_files(out_dir), lut_files(expected_dir));
    if rust_luts != python_luts {
        problems.push(format!(
            "LUT files differ: rust {:?} python {:?}",
            rust_luts.keys().collect::<Vec<_>>(),
            python_luts.keys().collect::<Vec<_>>()
        ));
    }
    let verdict = if !problems.is_empty() {
        "DIFFERENT"
    } else if normalized.fields.is_empty() {
        "identical"
    } else {
        "identical after output-path substitution"
    };
    (verdict.into(), normalized.fields, problems)
}

fn json_strings(value: Option<&Json>) -> Vec<String> {
    match value {
        Some(Json::Arr(items)) => items.iter().map(|i| match i {
            Json::Str(s) => s.clone(),
            other => panic!("not a string: {other:?}"),
        }).collect(),
        _ => Vec::new(),
    }
}

fn run_cases(cases: Vec<(String, PathBuf, PathBuf, PathBuf, Vec<String>)>, from: &str, to: &str) {
    let mut failures = Vec::new();
    let mut fields_seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    eprintln!("{:<42} result", "model");
    for (name, model, out_dir, expected_dir, notes) in cases {
        let (verdict, fields, problems) = check_one(&model, &out_dir, &expected_dir, &notes, from, to);
        eprintln!("{name:<42} {verdict}");
        for f in &fields {
            eprintln!("{:<42}   output location in: {f}", "");
        }
        if !fields.is_empty() {
            fields_seen.insert(name.clone(), fields);
        }
        for p in &problems {
            eprintln!("{:<42}   {p}", "");
        }
        if !problems.is_empty() {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "models not identical to Python: {failures:?}");
}

#[test]
fn golden_corpus() {
    // The real path, as Python's make_golden.py uses: the writer embeds its output location
    // (LUT paths), so writing through the corpus-base link would differ from Python's text.
    let golden = common::base().join("converter/local/golden");
    let golden = golden.canonicalize().unwrap_or(golden);
    let Ok(manifest_text) = std::fs::read_to_string(golden.join("manifest.json")) else {
        eprintln!("SKIP: golden corpus absent: {}", golden.display());
        return;
    };
    let manifest = parse_json(&manifest_text).unwrap();
    let models = manifest.get("models").unwrap();
    let mut names: Vec<String> = std::fs::read_dir(golden.join("models"))
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".model.json").map(str::to_string))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    let mut cases = Vec::new();
    for name in names {
        let Some(entry) = models.get(&name) else { panic!("{name}: not in manifest.json (Python refused it?)") };
        let model = golden.join("models").join(format!("{name}.model.json"));
        cases.push((
            name.clone(),
            model,
            golden.join("out-rust").join(&name),
            golden.join("out").join(&name),
            json_strings(entry.get("notes")),
        ));
    }
    run_cases(cases, "/golden/out-rust/", "/golden/out/");
}

#[test]
fn in_repo_models() {
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/convert_writer");
    let notes = parse_json(&std::fs::read_to_string(data.join("notes.json")).unwrap()).unwrap();
    let Json::Obj(entries) = notes else { panic!("notes.json") };
    // Python wrote to /tmp/prproj-convert-writer/<case>/; this writes at the same depth.
    let out_base = PathBuf::from(format!("/tmp/prproj-convert-writer-rs-{}", std::process::id()));
    let mut cases = Vec::new();
    for (name, value) in &entries {
        cases.push((
            name.clone(),
            data.join(format!("{name}.model.json")),
            out_base.join(name),
            data.join(name),
            json_strings(Some(value)),
        ));
    }
    let from = format!("/{}/", out_base.file_name().unwrap().to_string_lossy());
    run_cases(cases, &from, "/prproj-convert-writer/");
    std::fs::remove_dir_all(&out_base).unwrap();
}

// --- regression tests (review of the Phase 4 port) ----------------------------------------------

use prproj::convert::model::{py_str_repr, ColorCorrection, Marker, Model};
use prproj::convert::writer::WriteError;

/// A fresh scratch folder for one case (our own; removed at the end of the test).
fn scratch(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("prproj-writer-regress-{}-{case}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/convert_writer/opatom-separate.model.json")
}

fn example() -> Model {
    load_model(&example_path()).unwrap()
}

fn marker(name: &str, start: i64, duration: i64, track: &str, owner: &str) -> Marker {
    Marker {
        name: name.into(),
        comment: String::new(),
        start,
        duration,
        user: String::new(),
        color: String::new(),
        track: Some(track.into()),
        owner: owner.into(),
        event: None,
        source: None,
        inherited: false,
    }
}

/// `repr(chr(cp))` from Python 3.14 (`tools/make_printable_table.py`): every boundary of the
/// non-printable ranges +-1 and a stride over all code points.
#[test]
fn str_repr_matches_python() {
    let text = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/convert_writer/py_repr_cases.tsv")).unwrap();
    let mut bad = Vec::new();
    let mut n = 0;
    for line in text.lines() {
        let (hex, expected) = line.split_once('\t').unwrap();
        let c = char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap();
        n += 1;
        let got = py_str_repr(&c.to_string());
        if got != expected {
            bad.push(format!("U+{hex}: rust {got} python {expected}"));
        }
    }
    assert!(n > 5000);
    assert!(bad.is_empty(), "{} differences, first: {:?}", bad.len(), &bad[..bad.len().min(10)]);
    for (s, want) in [("\u{378}", "'\\u0378'"), ("\u{110BD}", "'\\U000110bd'"), ("\u{1D173}", "'\\U0001d173'"), ("é", "'é'")] {
        assert_eq!(py_str_repr(s), want);
    }
}

/// `"<stem> LUTs"` exists as a regular file: Python's `FileExistsError` text, reported (exit 2).
#[test]
fn lut_folder_is_a_file() {
    let dir = scratch("lutfile");
    let mut model = example();
    model.sequences[0].events[0].color_correction = Some(ColorCorrection { saturation: Some(150.0), ..Default::default() });
    std::fs::write(dir.join("o LUTs"), b"").unwrap();
    let error = write_project(&model, dir.join("o.prproj"), &mut SeededUuids::new(SEED)).unwrap_err();
    assert_eq!(error, WriteError::Exists(format!("[Errno 17] File exists: '{}'", dir.join("o LUTs").display())));
    assert!(error.is_reported());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn video_filler_markers_keep_overlapping_ranges_without_covering_picture() {
    let dir = scratch("marker-carriers");
    let mut model = example();
    let frame = 254_016_000_000 / 24;
    let seq = &mut model.sequences[0];
    seq.events[0].record_start = 20 * frame;
    seq.events[0].duration = 8 * frame;
    seq.markers = [
        ("first", 2, 4),
        ("overlap", 4, 4),
        ("chain", 7, 3),
        ("touching", 10, 0),
        ("before picture", 18, 2),
        ("crosses picture", 19, 3),
    ].into_iter().map(|(name, start, duration)| marker(name, start * frame, duration * frame, "V1", "filler")).collect();
    let out = dir.join("o.prproj");
    let report = write_project(&model, &out, &mut SeededUuids::new(SEED)).unwrap();
    assert!(report.notes.iter().any(|s| s.contains("crosses picture") && s.contains("kept on sequence")));
    let doc = PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let ticks = |node, path| doc.tree.findtext(node, path).unwrap()
        .map(|s| s.parse::<i64>().unwrap()).unwrap_or(0);
    let spans: Vec<_> = doc.tree.iter_tag(doc.root, "VideoClipTrackItem").map(|n| {
        (ticks(n, "ClipTrackItem/TrackItem/Start") / frame, ticks(n, "ClipTrackItem/TrackItem/End") / frame)
    }).collect();
    assert_eq!(spans, [(20, 28), (2, 10), (10, 11), (18, 20)]);
    let owner = doc.tree.iter_tag(doc.root, "Sequence")
        .find_map(|n| doc.tree.find(n, "MarkerOwner/Markers").unwrap()).unwrap();
    let list = doc.resolve(owner).unwrap();
    let refs = doc.tree.findall(list, "Markers/Marker/Second").unwrap();
    assert_eq!(refs.len(), 1);
    let payload = doc.tree.findtext(doc.resolve(refs[0]).unwrap(), "DVAMarker").unwrap().unwrap();
    let json = parse_json(&payload).unwrap();
    let fallback = json.get("DVAMarker").unwrap();
    assert_eq!(fallback.get("mName"), Some(&Json::Str("crosses picture".into())));
    assert_eq!(fallback.get("mStartTime").unwrap().get("ticks"), Some(&Json::Int(i128::from(19 * frame))));
    assert_eq!(fallback.get("mDuration").unwrap().get("ticks"), Some(&Json::Int(i128::from(3 * frame))));
    std::fs::remove_dir_all(dir).unwrap();
}

/// Absurd-but-valid tick values: a clean error, never an overflow panic.
#[test]
fn huge_ticks_do_not_panic() {
    let big = 1i64 << 62;
    let frame = 254_016_000_000 / 24;
    let check = |model: &Model, case: &str| {
        let dir = scratch(case);
        let error = write_project(model, dir.join("o.prproj"), &mut SeededUuids::new(SEED)).unwrap_err();
        assert!(matches!(error, WriteError::Other(_)), "{case}: {error:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    };

    // a filler marker at 2^62 lasting 2^62 (the adjustment layer would end at 2^63)
    let mut model = example();
    model.sequences[0].markers.push(marker("far", big, big, "V2", "filler"));
    check(&model, "filler");

    // speed 2^62:1 with an edit and a clip marker on one frame, the loser to the sequence
    let mut model = example();
    model.options.marker_conflict_loser_to_sequence = true;
    let event = &mut model.sequences[0].events[0];
    event.speed_ratio = Some((big, 1));
    event.source_duration = Some(frame);
    event.audio_tracks.clear();
    let clip_id = event.clip.clone();
    let mut own = marker("own", 0, 0, "V1", "edit");
    own.event = Some(0);
    own.source = Some(30 * frame);
    model.sequences[0].markers.push(own);
    model.clips.get_mut(&clip_id).unwrap().markers.push(marker("clip", 30 * frame, 0, "V1", "sequence"));
    check(&model, "speedmarker");

    // speed 1:2^40 on a one-frame source
    let mut model = example();
    let event = &mut model.sequences[0].events[0];
    event.speed_ratio = Some((1, 1 << 40));
    event.source_duration = Some(frame);
    event.audio_tracks.clear();
    check(&model, "speedslow");
}

/// Missing model file: Python's `FileNotFoundError` text; the CLI exits 2 with it. Usage text in
/// argparse's wrapped form; `"<stem> LUTs"` as a file through the CLI exits 2 like Python.
#[test]
fn missing_model_and_cli() {
    let error = load_model(Path::new("./missing-model.json")).unwrap_err();
    assert_eq!(error.0, "[Errno 2] No such file or directory: 'missing-model.json'");
    let exe = env!("CARGO_BIN_EXE_prproj-write");
    let run = std::process::Command::new(exe).args(["missing-model.json", "o.prproj"]).output().unwrap();
    assert_eq!(run.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&run.stderr), "prproj-write: [Errno 2] No such file or directory: 'missing-model.json'\n");
    let run = std::process::Command::new(exe).output().unwrap();
    assert_eq!(run.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&run.stderr),
        "usage: prproj-write [-h] [--bin-mode {separate,grouped,merged}]\n                    [--match-avid-default-pan] [--seed SEED]\n                    model output\nprproj-write: error: the following arguments are required: model, output\n"
    );
    let dir = scratch("cli-lutfile");
    let text = std::fs::read_to_string(example_path()).unwrap();
    let edited = text.replacen("\"video_track\": 1,", "\"video_track\": 1, \"color_correction\": {\"saturation\": 150.0},", 1);
    assert_ne!(text, edited);
    std::fs::write(dir.join("m.json"), edited).unwrap();
    std::fs::write(dir.join("o LUTs"), b"").unwrap();
    let run = std::process::Command::new(exe).arg(dir.join("m.json")).arg(dir.join("o.prproj")).output().unwrap();
    assert_eq!(run.status.code(), Some(2), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stderr).starts_with("prproj-write: [Errno 17] File exists: '"));
    std::fs::remove_dir_all(&dir).unwrap();
}
