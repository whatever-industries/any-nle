#![cfg(unix)]

use anynle_resolve_adapter::relink::{apply, capture, originals, Move};
use prproj::xml::{compress_prproj, decompress_prproj, GzipFraming};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn project(path: &Path, a: &Path, b: &Path) {
    let media = |id, path: &Path, reel, start| {
        format!(
            r#"<Media ObjectUID="item-{id}"><FilePath>{path}</FilePath>
            <ActualMediaFilePath>{path}</ActualMediaFilePath><RelativePath>{file}</RelativePath>
            <RelativePath>{file}</RelativePath><FileKey>key-{id}</FileKey>
            <ContentAndMetadataState>state-{id}</ContentAndMetadataState>
            <AlternateTapeName>{reel}</AlternateTapeName><UseAlternateStart>true</UseAlternateStart>
            <AlternateStart>{start}</AlternateStart><CCFileModTime>123</CCFileModTime>
            <VideoStream ObjectRef="2"/></Media>"#,
            path = path.display(),
            file = path.file_name().unwrap().to_str().unwrap(),
        )
    };
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" ?>
<PremiereData><Project ObjectRef="1"/><Project ObjectID="1" Version="46"/>
        <VideoStream ObjectID="2"><FrameRate>10584000000</FrameRate></VideoStream>
        {}{}</PremiereData>

"#,
        media("a", a, "REEL-A", 254_016_000_000_i64),
        media("b", b, "REEL-B", 508_032_000_000_i64),
    );
    fs::write(
        path,
        compress_prproj(xml.as_bytes(), Some(&GzipFraming::default())),
    )
    .unwrap();
}

fn move_file(original: &Path, destination: &Path) -> Move {
    Move {
        original: original.to_str().unwrap().into(),
        destination: destination.to_str().unwrap().into(),
    }
}

#[test]
fn selective_relink_rebases_paths_without_opening_ffprobe_and_preserves_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let a = root.join("a.mov");
    let b = root.join("b.mov");
    let new = root.join("relocated/a.mov");
    let input = root.join("source.prproj");
    let output = root.join("subfolder/relinked.prproj");
    let checksum = root.join("checksums.json");
    project(&input, &a, &b);
    let before = fs::read(&input).unwrap();
    assert_eq!(
        originals(&input).unwrap(),
        [a.to_str().unwrap(), b.to_str().unwrap()]
    );
    fs::write(&a, b"synthetic original").unwrap();
    fs::write(&b, b"unchanged original").unwrap();
    capture(&input, Some(&[a.to_str().unwrap().into()]), &checksum).unwrap();
    fs::create_dir(new.parent().unwrap()).unwrap();
    fs::create_dir(output.parent().unwrap()).unwrap();
    fs::rename(&a, &new).unwrap();
    let moves = [move_file(&a, &new)];
    let no_probe = root.join("nonexistent-ffprobe");
    apply(&input, &moves, &output, &no_probe, None).unwrap();
    let (xml, _) = decompress_prproj(&fs::read(&output).unwrap()).unwrap();
    let xml = String::from_utf8(xml).unwrap();
    assert!(xml.contains("<RelativePath>../relocated/a.mov</RelativePath>"));
    assert!(xml.contains("<RelativePath>../b.mov</RelativePath>"));
    assert_eq!(xml.matches("<RelativePath>").count(), 2);
    assert!(xml.contains("<AlternateTapeName>REEL-A</AlternateTapeName>"));
    assert!(xml.contains("<AlternateStart>508032000000</AlternateStart>"));
    assert_eq!(xml.matches("<CCFileModTime>").count(), 1);
    assert_eq!(fs::read(&input).unwrap(), before);
    assert!(!a.exists());
    let second = root.join("second.prproj");
    apply(&input, &moves, &second, &no_probe, Some(&checksum)).unwrap();
    assert!(apply(&input, &moves, &output, &no_probe, None)
        .err()
        .unwrap()
        .to_string()
        .contains("already exists"));
    fs::write(&new, b"substituted bytes").unwrap();
    let refused = root.join("refused.prproj");
    let error = apply(&input, &moves, &refused, &no_probe, Some(&checksum))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("SHA-256 mismatch"), "{error}");
    assert!(!refused.exists());
}

#[test]
fn same_basename_moves_check_embedded_identity_without_counting_packets() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let old_a = root.join("old-a/clip.mov");
    let old_b = root.join("old-b/clip.mov");
    let new_a = root.join("new-a/clip.mov");
    let new_b = root.join("new-b/clip.mov");
    let input = root.join("source.prproj");
    project(&input, &old_a, &old_b);
    for (path, data) in [(&new_a, "A"), (&new_b, "BB")] {
        fs::create_dir(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    let ffprobe = root.join("probe");
    fs::write(&ffprobe, r#"#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = -count_packets ]; then exit 4; fi
    path="$arg"
done
case "$path" in
    */new-a/*) reel=REEL-A; clock=00:00:01:00 ;;
    */new-b/*) reel=REEL-B; clock=00:00:02:00 ;;
    *) exit 5 ;;
esac
printf '{"streams":[{"codec_tag_string":"tmcd","tags":{"reel_name":"%s","timecode":"%s"}}]}\n' "$reel" "$clock"
"#).unwrap();
    fs::set_permissions(&ffprobe, fs::Permissions::from_mode(0o700)).unwrap();
    let good = [move_file(&old_a, &new_a), move_file(&old_b, &new_b)];
    apply(&input, &good, &root.join("correct.prproj"), &ffprobe, None).unwrap();
    let swapped = [move_file(&old_a, &new_b), move_file(&old_b, &new_a)];
    let refused = root.join("swapped.prproj");
    let error = apply(&input, &swapped, &refused, &ffprobe, None)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("reel or source timecode mismatch"),
        "{error}"
    );
    assert!(!refused.exists());
    let coalesced = [move_file(&old_a, &new_b), move_file(&old_b, &new_b)];
    assert!(apply(&input, &coalesced, &refused, &ffprobe, None).is_err());
    assert!(!refused.exists());
}
