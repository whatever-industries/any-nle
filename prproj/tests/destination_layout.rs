//! Synthetic panel hierarchy, independent Python parity, and no-layout compatibility.
use prproj::convert::{
    model::{self, Bin, Model},
    prepared::PreparedProjectInput,
    writer::{
        prepare_project, prepare_typed_project, prepare_typed_project_with_layout,
        DestinationBinLayout,
    },
};
use prproj::{
    model::{ItemId, Project},
    SeededUuids,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "anynle-layout-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(empty: bool) -> Model {
    let mut m =
        model::parse_model_str(include_str!("../../examples/opatom_clip.model.json")).unwrap();
    if empty {
        m.clips = Default::default();
        m.sequences.clear();
    }
    m.bins = vec![
        Bin {
            name: "Selects".into(),
            clips: if empty { vec![] } else { vec!["clip-1".into()] },
        },
        Bin {
            name: "Selects".into(),
            clips: vec![],
        },
        Bin {
            name: "Other".into(),
            clips: vec![],
        },
    ];
    m
}
fn layout() -> DestinationBinLayout {
    DestinationBinLayout {
        root_name: "AVID_IMPORT_20300102T030405Z".into(),
        parents: vec![
            vec!["Scene A".into()],
            vec!["Scene B".into()],
            vec!["Scene A".into()],
        ],
    }
}
fn xml(p: &Path) -> String {
    let b = std::fs::read(p).unwrap();
    let mut s = String::new();
    flate2::read::GzDecoder::new(b.as_slice())
        .read_to_string(&mut s)
        .unwrap();
    s
}
fn load(p: &Path) -> Project {
    Project::from_document(prproj::xml::parse_prproj(p).unwrap(), p).unwrap()
}
fn names(p: &Project, id: ItemId) -> Vec<String> {
    p.children(id).iter().map(|&id| p.item_name(id)).collect()
}
#[test]
fn mirrored_parents_dedupe_without_collapsing_same_named_bins_and_sequences_use_import_root() {
    let scratch = Scratch::new();
    let out = scratch.0.join("out.prproj");
    let input = PreparedProjectInput::from_model(&fixture(false)).unwrap();
    let p = prepare_typed_project_with_layout(
        &input,
        &out,
        &mut SeededUuids::new("layout"),
        &[],
        false,
        &layout(),
    )
    .unwrap();
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
    p.publish().unwrap();
    let project = load(&out);
    let root = project.root_item().unwrap();
    assert_eq!(names(&project, root), vec![layout().root_name.clone()]);
    let import = project.children(root)[0];
    assert_eq!(
        names(&project, import),
        vec!["Scene A", "Scene B", "Example sequence"]
    );
    let a = project.children(import)[0];
    let b = project.children(import)[1];
    assert_eq!(names(&project, a), vec!["Selects", "Other"]);
    assert_eq!(names(&project, b), vec!["Selects"]);
    assert_ne!(project.children(a)[0], project.children(b)[0]);
    assert!(!project.children(project.children(a)[0]).is_empty());
}
#[test]
fn linked_original_and_sequence_only_sources_remain_under_the_import_root() {
    use prproj::convert::prepared::*;
    let scratch = Scratch::new();
    let out = scratch.0.join("mixed.prproj");
    let mut input = PreparedProjectInput::from_model(&fixture(false)).unwrap();
    input.bins[0].clips.clear(); // Legacy clip is referenced only by the sequence.
    input.bins[1].clips = vec!["original".into()];
    input.clips.push(PreparedClip::Original(OriginalClip {
        metadata: ClipMetadata {
            id: "original".into(),
            name: "Linked original".into(),
            material_umid: None,
            tape_name: None,
            avid: None,
            time_display: None,
            rate: Some(model::Fraction::new(24, 1)),
            markers: vec![],
        },
        path: scratch.0.join("source.wav").to_string_lossy().into_owned(),
        container: "wav".into(),
        video: None,
        audio: Some(OriginalAudio {
            sample_rate: 48000,
            channels: 2,
            duration: 2 * model::TICKS_PER_SECOND,
            stream_index: 0,
            representation: SampleRepresentation::Pcm16,
        }),
        channels: vec![
            OriginalChannel {
                avid_track: 1,
                channel_index: 0,
                gain_db: 0.0,
            },
            OriginalChannel {
                avid_track: 2,
                channel_index: 1,
                gain_db: 0.0,
            },
        ],
        groups: vec![vec![1, 2]],
    }));
    prepare_typed_project_with_layout(
        &input,
        &out,
        &mut SeededUuids::new("mixed-layout"),
        &[],
        false,
        &layout(),
    )
    .unwrap()
    .publish()
    .unwrap();
    let project = load(&out);
    let root = project.root_item().unwrap();
    assert_eq!(project.children(root).len(), 1);
    let import = project.children(root)[0];
    let b = project.child_named(import, "Scene B").unwrap();
    let selects = project.child_named(b, "Selects").unwrap();
    assert_eq!(names(&project, selects), vec!["Linked original"]);
    assert!(project
        .children(import)
        .iter()
        .any(|&i| project.is_sequence_item(i)));
    assert!(
        project
            .children(import)
            .iter()
            .filter(|&&i| project.item_type(i) == prproj::model::ItemType::Clip.as_enum())
            .count()
            > 1
    );
}

#[test]
fn omitted_layout_preserves_legacy_bytes_and_uuid_count() {
    let scratch = Scratch::new();
    let out = scratch.0.join("out.prproj");
    let m = fixture(false);
    let mut ids = SeededUuids::new("unchanged");
    prepare_project(&m, &out, &mut ids, &[], false)
        .unwrap()
        .publish()
        .unwrap();
    let before = std::fs::read(&out).unwrap();
    let count = ids.count();
    std::fs::remove_file(&out).unwrap();
    let mut ids = SeededUuids::new("unchanged");
    prepare_typed_project(
        &PreparedProjectInput::from_model(&m).unwrap(),
        &out,
        &mut ids,
        &[],
        false,
    )
    .unwrap()
    .publish()
    .unwrap();
    assert_eq!(ids.count(), count);
    assert_eq!(std::fs::read(&out).unwrap(), before);
}
#[test]
fn invalid_layouts_refuse_before_uuid_allocation_or_outputs() {
    let scratch = Scratch::new();
    let out = scratch.0.join("out.prproj");
    let input = PreparedProjectInput::from_model(&fixture(true)).unwrap();
    let mut cases = vec![];
    let mut bad = layout();
    bad.parents.pop();
    cases.push(bad);
    for name in [
        "",
        ".",
        "..",
        "bad/name",
        "bad\\name",
        "bad\u{7f}",
        "bad\u{85}",
        "bad\n",
        "bad\u{ffff}",
    ] {
        let mut bad = layout();
        bad.root_name = name.into();
        cases.push(bad);
        let mut bad = layout();
        bad.parents[0] = vec![name.into()];
        cases.push(bad);
    }
    let mut bad = layout();
    bad.parents[0] = vec!["x".into(); 63];
    cases.push(bad);
    let mut bad = layout();
    bad.root_name = "é".repeat(2049);
    cases.push(bad);
    for bad in cases {
        let mut ids = SeededUuids::new("refuse");
        assert!(
            prepare_typed_project_with_layout(&input, &out, &mut ids, &[], false, &bad).is_err()
        );
        assert_eq!(ids.count(), 0);
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
    }
    let mut input = input;
    input.bins[0].name = "bad/name".into();
    assert!(prepare_typed_project_with_layout(
        &input,
        &out,
        &mut SeededUuids::new("bad-bin"),
        &[],
        false,
        &layout()
    )
    .is_err());
}
#[test]
fn maximum_depth_is_admitted_but_reference_only_budget_excess_refuses() {
    let scratch = Scratch::new();
    let out = scratch.0.join("out.prproj");
    let mut input = PreparedProjectInput::from_model(&fixture(true)).unwrap();
    input.bins.truncate(1);
    let max = DestinationBinLayout {
        root_name: "Import".into(),
        parents: vec![vec!["x".into(); 62]],
    };
    prepare_typed_project_with_layout(
        &input,
        &out,
        &mut SeededUuids::new("depth"),
        &[],
        false,
        &max,
    )
    .unwrap();
    assert!(!out.exists());
    input.bins = vec![
        Bin {
            name: "Selects".into(),
            clips: vec![]
        };
        10_000
    ];
    let many = DestinationBinLayout {
        root_name: "Import".into(),
        parents: vec![vec!["x".into(); 62]; 10_000],
    };
    let mut ids = SeededUuids::new("budget");
    let error = prepare_typed_project_with_layout(&input, &out, &mut ids, &[], false, &many)
        .err()
        .unwrap();
    assert!(error.to_string().contains("64 MiB"));
    assert_eq!(ids.count(), 0);
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 0);
}
#[test]
fn rust_and_independent_python_layouts_match_xml_for_empty_and_legacy_sources() {
    let scratch = Scratch::new();
    let out = scratch.0.join("out.prproj");
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let code = r#"import json,sys
from pathlib import Path
from avid2prproj.model import parse_model
from avid2prproj.writer import write_project
from avid2prproj.determinism import seeded_ids
m=json.loads((Path(sys.argv[2])/'examples/opatom_clip.model.json').read_text())
if sys.argv[3]=='markers':
 import copy
 m['sequences'][0]['markers'].append({'name':'Filler','comment':'','user':'','color':'','start_ticks':2116800000000,'duration_ticks':0,'owner':'filler','track':'V2'})
 second=copy.deepcopy(m['sequences'][0]);second['name']='Second sequence';m['sequences'].append(second)
empty=sys.argv[3]=='empty'
if empty:m['clips']=[];m['sequences']=[]
m['bins']=[{'name':'Selects','clips':[] if empty else ['clip-1']},{'name':'Selects','clips':[]},{'name':'Other','clips':[]}]
with seeded_ids('layout-parity'):
 write_project(parse_model(m),Path(sys.argv[1]),destination_layout={'root_name':'AVID_IMPORT_20300102T030405Z','parents':[['Scene A'],['Scene B'],['Scene A']]})
"#;
    for shape in ["empty", "legacy", "markers"] {
        let empty = shape == "empty";
        let mut input_model = fixture(empty);
        if shape == "markers" {
            input_model.sequences[0].markers.push(model::Marker {
                name: "Filler".into(),
                comment: String::new(),
                user: String::new(),
                color: String::new(),
                start: 2_116_800_000_000,
                duration: 0,
                owner: "filler".into(),
                track: Some("V2".into()),
                event: None,
                source: None,
                inherited: false,
            });
            let mut second = input_model.sequences[0].clone();
            second.name = "Second sequence".into();
            input_model.sequences.push(second);
        }
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                "python".into()
            } else {
                "python3".into()
            }
        });
        let output = Command::new(python)
            .args([
                "-c",
                code,
                out.to_str().unwrap(),
                repo.to_str().unwrap(),
                shape,
            ])
            .env(
                "PYTHONPATH",
                std::env::join_paths([repo.join("src"), repo.join("vendor")]).unwrap(),
            )
            .env("PYTHONHASHSEED", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = xml(&out);
        std::fs::remove_file(&out).unwrap();
        prepare_typed_project_with_layout(
            &PreparedProjectInput::from_model(&input_model).unwrap(),
            &out,
            &mut SeededUuids::new("layout-parity"),
            &[],
            false,
            &layout(),
        )
        .unwrap()
        .publish()
        .unwrap();
        assert_eq!(xml(&out), expected, "shape={shape}");
        let project = load(&out);
        let root = project.root_item().unwrap();
        assert_eq!(project.children(root).len(), 1);
        if shape == "markers" {
            let import = project.children(root)[0];
            let processed = project.child_named(import, "Processed Clips").unwrap();
            assert_eq!(project.children(processed).len(), 2);
        }
        std::fs::remove_file(&out).unwrap();
    }
}
