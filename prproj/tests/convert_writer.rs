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
    let (xml, framing) = decompress_prproj(written).unwrap();
    let mut doc = PremiereDocument::from_xml_bytes(&xml, framing).unwrap();
    let from_utf16 = utf16(from);
    if !xml.windows(from.len()).any(|w| w == from.as_bytes())
        && !contains_in_payloads(&doc, from.as_bytes(), &from_utf16)
    {
        return Normalized { xml, gzip: written.to_vec(), fields: Vec::new() };
    }
    let to_utf16 = utf16(to);
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
        if let Some(next) = replace_bytes(&changed, &from_utf16, &to_utf16) {
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

fn contains_in_payloads(doc: &PremiereDocument, from: &[u8], from_utf16: &[u8]) -> bool {
    let payloads = doc.index_payloads().unwrap();
    payloads.values().any(|p| {
        p.windows(from.len()).any(|w| w == from)
            || p.windows(from_utf16.len()).any(|w| w == from_utf16)
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
    eprintln!("{:<42} result", "model");
    for (name, model, out_dir, expected_dir, notes) in cases {
        let (verdict, fields, problems) = check_one(&model, &out_dir, &expected_dir, &notes, from, to);
        eprintln!("{name:<42} {verdict}");
        for f in &fields {
            eprintln!("{:<42}   output location in: {f}", "");
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

fn writer_scratch(case: &str) -> common::Scratch {
    common::Scratch::new(&format!("prproj-writer-regress-{case}"))
}

fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/convert_writer/opatom-separate.model.json")
}

fn example() -> Model {
    load_model(&example_path()).unwrap()
}

#[test]
fn sparse_picture_edits_expand_the_sequence_without_collapsing_tracks() {
    let scratch = writer_scratch("sparse-picture");
    let dir = scratch.path();
    let mut model = example();
    let seq = &mut model.sequences[0];
    let mut upper = seq.events[0].clone();
    upper.video_track = Some(5);
    upper.audio_tracks.clear();
    let expected = (upper.record_start, upper.duration, upper.source_in);
    seq.events.push(upper);

    let out = dir.join("picture.prproj");
    let report = write_project(&model, &out, &mut SeededUuids::new(SEED)).unwrap();
    let doc = PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let sequence = project.sequences()[0];
    let video = project.video_tracks(sequence);
    assert_eq!(video.len(), 5);
    assert!(video[1..4].iter().all(|&track| project.track_clips(track).is_empty()));
    for lane in [0, 4] {
        let [item] = project.track_clips(video[lane]) else { panic!("expected one picture edit") };
        assert_eq!(project.start(*item).unwrap().ticks, expected.0);
        assert_eq!(project.end(*item).unwrap().ticks, expected.0 + expected.1);
        assert_eq!(project.in_point(*item).unwrap().ticks, expected.2);
        assert_eq!(project.out_point(*item).unwrap().ticks, expected.2 + expected.1);
    }
    let audio = project.audio_tracks(sequence);
    assert_eq!(audio.len(), 4);
    assert_eq!(project.track_clips(audio[0]).len(), 1);
    assert_eq!(project.track_clips(audio[1]).len(), 1);
    assert!(report.notes.iter().any(|n| n.contains("expanded video track capacity to 5")));
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
    let scratch = writer_scratch("lutfile");
    let dir = scratch.path();
    let mut model = example();
    model.sequences[0].events[0].color_correction = Some(ColorCorrection { saturation: Some(150.0), ..Default::default() });
    std::fs::write(dir.join("o LUTs"), b"").unwrap();
    let error = write_project(&model, dir.join("o.prproj"), &mut SeededUuids::new(SEED)).unwrap_err();
    assert_eq!(error, WriteError::Exists(format!("[Errno 17] File exists: '{}'", dir.join("o LUTs").display())));
    assert!(error.is_reported());
}

#[test]
fn video_filler_markers_keep_overlapping_ranges_without_covering_picture() {
    let scratch = writer_scratch("marker-carriers");
    let dir = scratch.path();
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
    seq.markers.push(marker("upper filler", 4 * frame, 2 * frame, "V7", "filler"));
    seq.markers.push(marker("upper over picture", 19 * frame, 3 * frame, "V7", "filler"));
    let out = dir.join("o.prproj");
    let report = write_project(&model, &out, &mut SeededUuids::new(SEED)).unwrap();
    assert!(report.notes.iter().any(|s| s.contains("crosses picture") && s.contains("kept on sequence")));
    let doc = PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let ticks = |node, path| doc.tree.findtext(node, path).unwrap()
        .map(|s| s.parse::<i64>().unwrap()).unwrap_or(0);
    let spans: Vec<_> = doc.tree.iter_tag(doc.root, "VideoClipTrackItem").map(|n| {
        (ticks(n, "ClipTrackItem/TrackItem/Start") / frame, ticks(n, "ClipTrackItem/TrackItem/End") / frame)
    }).collect();
    assert_eq!(spans, [(20, 28), (2, 10), (10, 11), (18, 20), (4, 6)]);
    let owner = doc.tree.iter_tag(doc.root, "Sequence")
        .find_map(|n| doc.tree.find(n, "MarkerOwner/Markers").unwrap()).unwrap();
    let list = doc.resolve(owner).unwrap();
    let refs = doc.tree.findall(list, "Markers/Marker/Second").unwrap();
    assert_eq!(refs.len(), 2);
    let mut names = Vec::new();
    for marker_ref in refs {
        let payload = doc.tree.findtext(doc.resolve(marker_ref).unwrap(), "DVAMarker").unwrap().unwrap();
        let json = parse_json(&payload).unwrap();
        let fallback = json.get("DVAMarker").unwrap();
        let Some(Json::Str(name)) = fallback.get("mName") else { panic!("expected marker name") };
        names.push(name.clone());
        assert_eq!(fallback.get("mStartTime").unwrap().get("ticks"), Some(&Json::Int(i128::from(19 * frame))));
        assert_eq!(fallback.get("mDuration").unwrap().get("ticks"), Some(&Json::Int(i128::from(3 * frame))));
    }
    names.sort();
    assert_eq!(names, ["crosses picture", "upper over picture"]);
    let project = prproj::model::Project::from_document(doc, &out).unwrap();
    let tracks = project.video_tracks(project.sequences()[0]);
    assert_eq!(tracks.len(), 7);
    assert!(tracks[1..6].iter().all(|&track| project.track_clips(track).is_empty()));
    let [carrier] = project.track_clips(tracks[6]) else { panic!("expected the V7 marker carrier") };
    let source = project.track_item_source(*carrier).unwrap();
    let [saved] = project.item_markers(source) else { panic!("expected one marker on the carrier") };
    let tree = &project.doc.tree;
    use prproj::model::marker::{marker_end, marker_name, marker_start};
    assert_eq!(marker_name(tree, *saved).unwrap(), "upper filler");
    assert_eq!(marker_start(tree, *saved).unwrap().ticks, 4 * frame);
    assert_eq!(marker_end(tree, *saved).unwrap().ticks, 6 * frame);
}

#[test]
fn merged_marker_clock_notice_requires_the_verified_picture_profile() {
    use prproj::convert::model::Fraction;
    use prproj::convert::prepared::PreparedProjectInput;
    use prproj::convert::writer::prepare_typed_project;

    let scratch = writer_scratch("merged-marker-clock");
    let dir = scratch.path();
    for (drop_frame, source_frame, track, expect_notice) in [
        (true, 215_784, "V1", false),
        (true, 215_784, "A1", true),
        (true, 0, "V1", true),
        (false, 172_800, "V1", true),
        (false, 172_800, "A1", false),
        (false, 0, "A1", true),
    ] {
        let mut model = example();
        model.options.bin_mode = "merged".into();
        let (rate, frame, bits) = if drop_frame {
            (Fraction::new(30_000, 1001), 8_475_667_200, 24)
        } else {
            (Fraction::new(24_000, 1001), 10_594_584_000, 16)
        };
        let origin = source_frame * frame;
        let clip = model.clips.get_mut("clip-1").unwrap();
        clip.rate = Some(rate);
        clip.time_display = Some(if drop_frame { 102 } else { 110 });
        let picture = clip.video.as_mut().unwrap();
        picture.rate = rate;
        picture.start = origin;
        picture.duration = 150 * frame;
        for (i, audio) in clip.audio.iter_mut().enumerate() {
            audio.avid_track = Some(i as i64 + 1);
            audio.start = origin;
            audio.duration = picture.duration;
            audio.bits = bits;
        }
        clip.markers = vec![marker("Source range", 12 * frame, 2 * frame, track, "sequence")];
        let sequence = &mut model.sequences[0];
        sequence.rate = rate;
        sequence.markers.clear();
        sequence.events[0].source_in = 10 * frame;
        sequence.events[0].duration = 5 * frame;
        if track == "A1" {
            // The first source sibling is used on timeline A2.
            sequence.events[0].audio_channels = vec![2, 1];
            let mut own = marker("Edit range", 2 * frame, 3 * frame, "A2", "edit");
            own.event = Some(0);
            own.source = Some(12 * frame);
            sequence.markers.push(own);
            model.options.marker_conflict_loser_to_sequence = true;
            if !drop_frame && source_frame != 0 {
                let mut picture = marker("Picture range", 2 * frame, 3 * frame, "V1", "edit");
                picture.event = Some(0);
                picture.source = Some(12 * frame);
                sequence.markers.push(picture);
            }
        }
        let mut input = PreparedProjectInput::from_model(&model).unwrap();
        input.direct_dnxhr_origins.insert("clip-1".into(), origin);
        let prepared = prepare_typed_project(
            &input, dir.join("out.prproj"), &mut SeededUuids::new(SEED), &[], false,
        ).unwrap();
        assert_eq!(
            prepared.report().unverified.iter().any(|s| s.contains("markers on a merged clip")),
            expect_notice, "{drop_frame} {source_frame} {track}",
        );
        assert_eq!(
            prepared.report().notes.iter().any(|s| s.contains("list each A1/A2 source range twice")),
            !drop_frame && source_frame != 0 && track == "A1",
        );
        if track == "A1" {
            let out = dir.join("out.prproj");
            prepared.publish().unwrap();
            let doc = PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
            let project = prproj::model::Project::from_document(doc, &out).unwrap();
            let sequence = project.sequences().into_iter()
                .find(|&s| project.sequence_name(s) == model.sequences[0].name).unwrap();
            let tree = &project.doc.tree;
            use prproj::model::marker::{marker_end, marker_name, marker_start};
            let [loser] = project.sequence_markers(sequence) else { panic!("expected one sequence marker") };
            assert_eq!(marker_start(tree, *loser).unwrap().ticks, 2 * frame);
            if source_frame == 0 {
                assert_eq!(marker_name(tree, *loser).unwrap(), "Edit range");
            } else {
                let track = project.audio_tracks(sequence)[1];
                let path = &model.clips["clip-1"].audio[0].path;
                let item = project.track(track).clips[0];
                let source = project.track_item_source(item).unwrap();
                assert_eq!(project.item_media_path(source), Some(path.as_str()));
                let [edit] = project.item_markers(source) else { panic!("expected one clip marker") };
                assert_eq!(marker_name(tree, *edit).unwrap(), "Edit range");
                assert_eq!(marker_start(tree, *edit).unwrap().ticks, 12 * frame);
                assert_eq!(marker_end(tree, *edit).unwrap().ticks, 15 * frame);
                assert_eq!(marker_name(tree, *loser).unwrap(), "Source range");
            }
            if !drop_frame && source_frame != 0 {
                let item = project.track(project.video_tracks(sequence)[0]).clips[0];
                let source = project.track_item_source(item).unwrap();
                let [picture] = project.item_markers(source) else { panic!("expected picture marker") };
                assert_eq!(marker_name(tree, *picture).unwrap(), "Picture range");
            }
            std::fs::remove_file(&out).unwrap();
        }
    }
}

#[test]
fn animatte_keys_use_the_source_trim_and_refuse_unrepresentable_ticks() {
    use prproj::convert::model::{ticks_per_unit, Matte};

    let scratch = writer_scratch("animatte-key-time");
    let dir = scratch.path();
    let mut model = example();
    let frame = ticks_per_unit(model.sequences[0].rate);
    let event = &mut model.sequences[0].events[0];
    event.source_in = 24 * frame;
    let contour = vec![
        [0.2, 0.2, 0.0, 0.0, 0.0, 0.0],
        [0.5, 0.2, 0.0, 0.0, 0.0, 0.0],
        [0.5, 0.6, 0.0, 0.0, 0.0, 0.0],
    ];
    event.matte = Some(Matte {
        shapes: vec![vec![(0, contour.clone()), (3, contour)]],
        inverted: false,
        opacities: vec![100.0],
        feathers: vec![(0.0, 0.0)],
        not_converted: vec![],
    });
    let saved = dir.join("keys.prproj");
    write_project(&model, &saved, &mut SeededUuids::new(SEED)).unwrap();
    let doc = PremiereDocument::from_bytes(&std::fs::read(saved).unwrap()).unwrap();
    let path = doc.tree.iter(doc.root).find(|&node| {
        doc.tree.findtext(node, "Name").unwrap().as_deref() == Some("Path")
            && doc.tree.child_by_tag(node, "Keyframes").is_some()
    }).unwrap();
    let keys = doc.tree.findtext(path, "Keyframes").unwrap().unwrap();
    let positions: Vec<i64> = keys.split_terminator(';')
        .map(|key| key.split_once(',').unwrap().0.parse().unwrap())
        .collect();
    assert_eq!(positions, [24 * frame, 27 * frame]);

    let invalid = dir.join("overflow.prproj");
    // The key index passes model/1 validation, but its Premiere tick value overflows i64.
    model.sequences[0].events[0].matte.as_mut().unwrap().shapes[0][1].0 = (1 << 31) - 1;
    let error = write_project(&model, &invalid, &mut SeededUuids::new(SEED)).unwrap_err();
    assert!(error.to_string().contains("AniMatte shape 1 key at frame 2147483647 exceeds the Premiere tick range"));
    assert!(!invalid.exists());
}

/// Absurd-but-valid tick values: a clean error, never an overflow panic.
#[test]
fn huge_ticks_do_not_panic() {
    let big = 1i64 << 62;
    let frame = 254_016_000_000 / 24;
    let check = |model: &Model, case: &str| {
        let scratch = writer_scratch(case);
        let dir = scratch.path();
        let error = write_project(model, dir.join("o.prproj"), &mut SeededUuids::new(SEED)).unwrap_err();
        assert!(matches!(error, WriteError::Other(_)), "{case}: {error:?}");
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
    let scratch = writer_scratch("cli-lutfile");
    let dir = scratch.path();
    let text = std::fs::read_to_string(example_path()).unwrap();
    let edited = text.replacen("\"video_track\": 1,", "\"video_track\": 1, \"color_correction\": {\"saturation\": 150.0},", 1);
    assert_ne!(text, edited);
    std::fs::write(dir.join("m.json"), edited).unwrap();
    std::fs::write(dir.join("o LUTs"), b"").unwrap();
    let run = std::process::Command::new(exe).arg(dir.join("m.json")).arg(dir.join("o.prproj")).output().unwrap();
    assert_eq!(run.status.code(), Some(2), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stderr).starts_with("prproj-write: [Errno 17] File exists: '"));
}
