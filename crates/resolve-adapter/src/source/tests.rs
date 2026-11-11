//! Small synthetic packages exercise graph validation without proprietary fixtures.
use super::*;
use crate::binary::{qt as encode_qt, Qt};
use std::{fs::File, io::Write};
use zip::{write::SimpleFileOptions, ZipWriter};

const PROJECT: &str = "11111111-0000-0000-0000-000000000000";
const POOL: &str = "22222222-0000-0000-0000-000000000000";
const ROOT: &str = "33333333-0000-0000-0000-000000000000";
const CHILD: &str = "44444444-0000-0000-0000-000000000000";
const CLIP: &str = "55555555-0000-0000-0000-000000000000";
const HANDLE: &str = "66666666-0000-0000-0000-000000000000";
const SEQUENCE: &str = "77777777-0000-0000-0000-000000000000";

fn build(path: &Path, handle: &str, child_parent: &str) {
    build_with_child_registration(path, handle, child_parent, CHILD);
}

fn build_with_child_registration(path: &Path, handle: &str, child_parent: &str, registered: &str) {
    let fields = hex::encode(encode_qt(&[("RootFolderRef", Qt::Text(ROOT))]));
    let mut child_list = Vec::new();
    crate::binary::pb_bytes(
        2,
        &encode_qt(&[("0", Qt::Text(registered))]),
        &mut child_list,
    );
    let folder_fields = crate::binary::pack(&child_list).unwrap();
    let project = format!(
        r#"<SM_Project DbId="{PROJECT}">
        <ProjectVersion>17</ProjectVersion><ProjectName>Fixture &amp; test</ProjectName>
        <TimelineHandleVec><Element>{handle}</Element></TimelineHandleVec>
        <MediaPool><Sm2MediaPool DbId="{POOL}"><FieldsBlob>{fields}</FieldsBlob></Sm2MediaPool></MediaPool>
        </SM_Project>"#
    );
    let root = format!(
        r#"<Sm2MpFolder DbId="{ROOT}">
        <FieldsBlob>{folder_fields}</FieldsBlob><Name>Master</Name><MediaPool>{POOL}</MediaPool><MediaVec>
        <Element><Sm2MpTimelineClip DbId="{CLIP}"><MpFolder>{ROOT}</MpFolder>
        <TimelineSharedHandle><Sm2Timeline DbId="{HANDLE}"><Name>Empty test</Name>
        <MpTimelineClip>{CLIP}</MpTimelineClip><Sequence><Sm2Sequence DbId="{SEQUENCE}">
        <Parent>{HANDLE}</Parent><FrameRate>872211b5dcf937400000000000000000</FrameRate>
        <Resolution>00000000000007800000000000000438</Resolution>
        <MediaExtents>333333333327ac400000000000000000</MediaExtents>
        </Sm2Sequence></Sequence></Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element>
        </MediaVec></Sm2MpFolder>"#
    );
    let child = format!(
        r#"<Sm2MpFolder DbId="{CHILD}"><Name>Child</Name><MpFolder>{child_parent}</MpFolder>
        <MediaPool>{POOL}</MediaPool><MediaVec/></Sm2MpFolder>"#
    );
    let mut archive = ZipWriter::new(File::create(path).unwrap());
    for (name, contents) in [
        ("project.xml", project),
        (ROOT_FOLDER, root),
        ("MediaPool/Master/000_Child/MpFolder.xml", child),
    ] {
        archive
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writeln!(archive, r#"<?xml version="1.0"?>"#).unwrap();
        writeln!(archive, r#"<!--DbAppVer="21.2.0.0006" DbPrjVer="17"-->"#).unwrap();
        archive.write_all(contents.as_bytes()).unwrap();
    }
    archive.finish().unwrap();
}

#[test]
fn saved_project_identity_and_exact_clock_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.drp");
    build(&path, HANDLE, ROOT);
    let graph = read(&path).unwrap();
    assert_eq!(graph.project.name, "Fixture & test");
    assert_eq!(graph.bins.len(), 2);
    assert_eq!(graph.bins[1].parent.as_deref(), Some(ROOT));
    assert_eq!(graph.sequences.len(), 1);
    let timeline = &graph.sequences[0];
    assert_eq!(timeline.id, SEQUENCE);
    assert_eq!(
        graph
            .sidecar(ADAPTER, SEQUENCE)
            .and_then(|row| row.get("bin_id")),
        Some(&Value::Str(ROOT.into()))
    );
    assert_eq!(timeline.frame_size, Some((1920, 1080)));
    assert_eq!(timeline.clock.as_ref().unwrap().effective_count, 86_400);
    assert_eq!(timeline.start, Rational::new(18_018, 5));
    assert!(timeline.placements.is_empty());
    let mut json = Vec::new();
    graph.write_json(&mut json).unwrap();
    assert_eq!(
        SourceGraph::from_json_text(std::str::from_utf8(&json).unwrap()).unwrap(),
        graph
    );
    let mut repeated = Vec::new();
    read(&path).unwrap().write_json(&mut repeated).unwrap();
    assert_eq!(json, repeated);
}

#[test]
fn broken_registration_and_bin_parent_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("contradictory.drp");
    build(&path, CHILD, ROOT);
    assert!(read(&path)
        .unwrap_err()
        .to_string()
        .contains("registered Resolve timeline"));
    build(&path, HANDLE, CHILD);
    assert!(read(&path).unwrap_err().to_string().contains("bin parent"));
    build_with_child_registration(&path, HANDLE, ROOT, HANDLE);
    assert!(read(&path)
        .unwrap_err()
        .to_string()
        .contains("child bin registration contradicts"));
}
