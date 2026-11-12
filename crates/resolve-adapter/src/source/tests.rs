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
        <CurrentTimelineIndex>0</CurrentTimelineIndex>
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
        graph.sidecar(ACTIVE_TIMELINE_ADAPTER, ROOT),
        Some(&Value::Obj(vec![
            ("saved_index".into(), Value::Int(0)),
            ("sequence_id".into(), text(SEQUENCE)),
            ("unavailable_reason".into(), Value::Null),
        ]))
    );
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
fn sibling_order_follows_numeric_keys_in_the_saved_map() {
    let mut nodes = 0;
    let pool_fields = hex::encode(encode_qt(&[("RootFolderRef", Qt::Text(ROOT))]));
    let project = xml::parse(
        &format!(
            r#"<SM_Project><MediaPool><Sm2MediaPool DbId="{POOL}"><FieldsBlob>{pool_fields}</FieldsBlob></Sm2MediaPool></MediaPool></SM_Project>"#
        ),
        &mut nodes,
    )
    .unwrap();
    let mut members = BTreeMap::new();
    let ids: Vec<_> = (100..111).map(|n| uuid::Uuid::from_u128(n).to_string()).collect();
    for (ordinal, id) in ids.iter().enumerate() {
        let path = format!("MediaPool/Master/{ordinal:03}_Duplicate/MpFolder.xml");
        let node = xml::parse(
            &format!(r#"<Sm2MpFolder DbId="{id}"><MediaPool>{POOL}</MediaPool><MpFolder>{ROOT}</MpFolder></Sm2MpFolder>"#),
            &mut nodes,
        )
        .unwrap();
        members.insert(path, node);
    }
    let keys: Vec<_> = (0..ids.len()).map(|ordinal| ordinal.to_string()).collect();
    let rows: Vec<_> = keys
        .iter()
        .zip(ids.iter().rev())
        .map(|(key, id)| (key.as_str(), Qt::Text(id)))
        .collect();
    let mut body = Vec::new();
    crate::binary::pb_bytes(2, &encode_qt(&rows), &mut body);
    let fields = crate::binary::pack(&body).unwrap();
    members.insert(
        ROOT_FOLDER.into(),
        xml::parse(
            &format!(r#"<Sm2MpFolder DbId="{ROOT}"><MediaPool>{POOL}</MediaPool><FieldsBlob>{fields}</FieldsBlob></Sm2MpFolder>"#),
            &mut nodes,
        )
        .unwrap(),
    );

    let ordered = folders(&members, &project, &mut Blobs::default()).unwrap();
    assert_eq!(ordered[0].0.id().unwrap(), ROOT);
    assert!(matches!(
        ordered[0].1,
        Some(BinOrder::Saved { archive_matches: Some(false) })
    ));
    assert!(ordered.iter().skip(1).map(|(node, _)| node.id().unwrap())
        .eq(ids.iter().rev().map(String::as_str)));
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

#[test]
fn transition_observations_keep_their_owners_after_graph_serialization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.drp");
    build(&path, HANDLE, ROOT);
    let mut graph = read(&path).unwrap();
    let mut second = graph.sequences[0].clone();
    second.id = "another-sequence".into();
    second.clock.as_mut().unwrap().provenance.source_package = second.id.clone();
    graph.sequences.push(second);
    let grid = Grid {
        rate: crate::prepare::rate(),
        origin: 86_400,
        length: 60,
        size: (1920, 1080),
    };
    let make = |id: &str, alignment: &str| Node {
        name: "Sm2TiTransition".into(),
        id: Some(id.into()),
        text: String::new(),
        content: 0..0,
        children: [
            ("Name", "Cross Dissolve"),
            ("Start", "86417"),
            ("Duration", "8"),
            ("AlignmentType", alignment),
            ("Position", "2"),
        ]
        .into_iter()
        .map(|(name, value)| Node {
            name: name.into(),
            id: None,
            text: value.into(),
            content: 0..0,
            children: vec![],
        })
        .collect(),
    };
    for (sequence, track, id, alignment) in [
        (SEQUENCE, "track-a", "transition-a", "3"),
        ("another-sequence", "track-b", "transition-b", "1"),
    ] {
        observations::transition(
            &mut graph, &make(id, alignment), sequence, track, StreamKind::Video, 1, grid, [None; 2], false,
        ).unwrap();
    }
    let mut serialized = Vec::new();
    graph.write_json(&mut serialized).unwrap();
    let reopened = SourceGraph::from_json_text(std::str::from_utf8(&serialized).unwrap()).unwrap();
    for (sequence, alignment) in [(SEQUENCE, "3"), ("another-sequence", "1")] {
        let Some(Value::Arr(rows)) = reopened.sidecar("resolve.transitions", sequence) else {
            panic!("missing transition receipt");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("alignment_type"), Some(&text(alignment)));
        assert_eq!(rows[0].get("position"), Some(&text("2")));
    }
    assert!(reopened.sequences.iter().all(|s| s.transitions.is_empty()));
    assert_eq!(reopened.losses.len(), 2);
}
