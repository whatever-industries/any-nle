//! Phase 2A: `build::skeleton` and `build::sequence` against the real Python builders
//! (`tools/expected_skeleton_sequence.py`), byte for byte, under the same seeded UUID sequence.

mod common;

use prproj::build::sequence::{build_sequence, extra_formats, formats};
use prproj::build::skeleton::{build_empty_project, EmptyProjectOptions};
use prproj::xml::{serialize_document, serialize_element, Tree};
use prproj::SeededUuids;
use std::collections::{HashMap, HashSet};

const SEED: &str = "skeleton-sequence";

fn expected(name: &str) -> Vec<u8> {
    std::fs::read(common::data("build_skeleton_sequence").join(name)).unwrap()
}

fn assert_same(name: &str, got: &[u8]) {
    let want = expected(name);
    if got != want.as_slice() {
        let at = got.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
        let lo = at.saturating_sub(80);
        panic!(
            "{name}: differs at byte {at} (got {} bytes, want {})\n got: {:?}\nwant: {:?}",
            got.len(),
            want.len(),
            String::from_utf8_lossy(&got[lo..(at + 80).min(got.len())]),
            String::from_utf8_lossy(&want[lo..(at + 80).min(want.len())]),
        );
    }
}

#[test]
fn empty_project_document() {
    let mut t = Tree::new();
    let mut ids = SeededUuids::new(SEED);
    let root = build_empty_project(&mut t, &EmptyProjectOptions::default(), &mut ids);
    assert_eq!(ids.count(), 3);
    assert_same("skeleton.xml", &serialize_document(&t, root).unwrap());
}

#[test]
fn empty_project_partial_options() {
    let mut t = Tree::new();
    let mut ids = SeededUuids::new(SEED);
    let opts = EmptyProjectOptions {
        document_id: Some("11111111-2222-4333-8444-555555555555"),
        workspace: Some("Editing"),
        build_version: Some("26.0.0x1 - 01/01/2026 00:00:00"),
        ..Default::default()
    };
    let root = build_empty_project(&mut t, &opts, &mut ids);
    assert_eq!(ids.count(), 2);
    assert_same("skeleton_partial.xml", &serialize_document(&t, root).unwrap());
}

#[test]
fn every_format_matches() {
    let cases: Vec<String> =
        String::from_utf8(expected("cases.tsv")).unwrap().lines().map(str::to_string).collect();
    let known: HashSet<&str> = formats().iter().chain(extra_formats().iter()).map(|(n, _)| *n).collect();
    let listed: HashSet<&str> = cases.iter().map(String::as_str).collect();
    assert_eq!(known, listed, "Rust presets and the Python FORMATS must be the same set");
    assert_eq!(cases.len(), 21);
    for name in &cases {
        let mut t = Tree::new();
        let mut ids = SeededUuids::new(SEED);
        let root = build_sequence(&mut t, "Seq01", name, None, None, &mut ids).unwrap();
        assert_same(&format!("seq_{name}.xml"), &serialize_element(&t, root).unwrap());
    }
}

#[test]
fn wide_linked_original_preset_writes_exact_raster_and_clock() {
    let mut t = Tree::new();
    let mut ids = SeededUuids::new(SEED);
    let root = build_sequence(&mut t, "Wide", "anynle-2048x858p24", None, None, &mut ids).unwrap();
    let xml = String::from_utf8(serialize_element(&t, root).unwrap()).unwrap();
    assert!(xml.contains("<FrameRect>0,0,2048,858</FrameRect>"));
    assert!(xml.contains("<FrameRate>10584000000</FrameRate>"));
}

#[test]
fn explicit_linked_original_format_keeps_the_raster_and_fractional_clock() {
    let mut tree = Tree::new();
    let mut ids = SeededUuids::new(SEED);
    let root = build_sequence(&mut tree, "Linked", "anynle-linked/8475667200/1600/900/103", None, None, &mut ids).unwrap();
    let xml = String::from_utf8(serialize_element(&tree, root).unwrap()).unwrap();
    assert!(xml.contains("<FrameRect>0,0,1600,900</FrameRect>"));
    assert!(xml.contains("<FrameRate>8475667200</FrameRate>"));
    assert!(xml.contains("<MZ.Sequence.VideoTimeDisplayFormat>103</MZ.Sequence.VideoTimeDisplayFormat>"));
}

#[test]
fn custom_name_pinned_uids_and_video_tracks() {
    let mut t = Tree::new();
    let mut ids = SeededUuids::new(SEED);
    let pinned: HashMap<String, String> = [
        ("item", "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
        ("sequence", "99999999-8888-4777-8666-555555555555"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let root = build_sequence(&mut t, "My Seq", "1080p25", Some(&pinned), Some(5), &mut ids).unwrap();
    assert_same("seq_custom.xml", &serialize_element(&t, root).unwrap());
}

#[test]
fn unknown_preset_is_refused() {
    let mut t = Tree::new();
    let mut ids = SeededUuids::new(SEED);
    let err = build_sequence(&mut t, "S", "nope", None, None, &mut ids).unwrap_err();
    assert!(err.to_string().starts_with("unknown preset \"nope\"; try ["), "{err}");
}
