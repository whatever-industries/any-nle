//! CLI file protection with a disposable synthetic bin and empty media folder.
use avid_reader::document::AvbDocument;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "anynle-output-{}-{time}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let path = path.canonicalize().unwrap();
        std::fs::create_dir(path.join("media")).unwrap();
        AvbDocument::new_empty_bin(19, "Synthetic output fixture", 100, false)
            .write_new(&path.join("source.avb"))
            .unwrap();
        Self(path)
    }

    fn convert(&self, project: &Path, graph: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_any-nle"));
        command
            .current_dir(&self.0)
            .args([
                "convert",
                "--from",
                "avid",
                "source.avb",
                "--media",
                "media",
                "--to",
                "premiere",
            ])
            .arg(project)
            .args(["--seed", "output-protection"]);
        if let Some(path) = graph {
            command.arg("--graph").arg(path);
        }
        command.output().unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Fresh, test-owned files only; retain failed cases for inspection.
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[test]
fn existing_graph_file_is_not_overwritten() {
    let scratch = Scratch::new();
    let graph = scratch.0.join("protected.json");
    let project = scratch.0.join("project.prproj");
    std::fs::write(&graph, b"existing graph bytes").unwrap();
    let result = scratch.convert(&project, Some(&graph));
    assert_eq!(std::fs::read(&graph).unwrap(), b"existing graph bytes");
    assert!(
        !result.status.success(),
        "an existing snapshot must be refused"
    );
    assert!(!project.exists());
}

#[test]
fn graph_output_cannot_replace_the_input_bin() {
    let scratch = Scratch::new();
    let source = scratch.0.join("source.avb");
    let original = std::fs::read(&source).unwrap();
    let project = scratch.0.join("project.prproj");
    let result = scratch.convert(&project, Some(&source));
    assert_eq!(
        std::fs::read(&source).unwrap(),
        original,
        "source bin was replaced"
    );
    assert!(!result.status.success());
    assert!(!project.exists());
}

#[test]
fn colliding_project_and_graph_paths_are_refused_without_creating_a_file() {
    let scratch = Scratch::new();
    let project = scratch.0.join("project.prproj");
    std::fs::create_dir(scratch.0.join("child")).unwrap();
    let mut aliases = vec![
        project.clone(),
        scratch.0.join("child/../project.prproj"),
        PathBuf::from("./project.prproj"),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&scratch.0, scratch.0.join("alias")).unwrap();
        aliases.push(scratch.0.join("alias/project.prproj"));
    }
    for graph in aliases {
        let result = scratch.convert(&project, Some(&graph));
        assert!(!result.status.success());
        assert!(
            !project.exists(),
            "a graph was left at the project output path"
        );
    }
}

#[test]
fn refused_primary_output_does_not_leave_a_graph_snapshot() {
    let scratch = Scratch::new();
    let project = scratch.0.join("project.prproj");
    let graph = scratch.0.join("graph.json");
    std::fs::write(&project, b"existing project bytes").unwrap();
    let result = scratch.convert(&project, Some(&graph));
    assert!(!result.status.success());
    assert_eq!(std::fs::read(&project).unwrap(), b"existing project bytes");
    assert!(
        !graph.exists(),
        "snapshot was created for a preflight refusal"
    );
}

#[cfg(unix)]
#[test]
fn graph_symlinks_preserve_both_existing_and_absent_targets() {
    for dangling in [false, true] {
        let scratch = Scratch::new();
        let project = scratch.0.join("project.prproj");
        let graph = scratch.0.join("graph.json");
        let target = scratch.0.join("protected.txt");
        if !dangling {
            std::fs::write(&target, b"protected target").unwrap();
        }
        std::os::unix::fs::symlink(&target, &graph).unwrap();
        let result = scratch.convert(&project, Some(&graph));
        assert!(!result.status.success());
        assert_eq!(std::fs::read_link(&graph).unwrap(), target);
        if dangling {
            assert!(!target.exists());
        } else {
            assert_eq!(std::fs::read(&target).unwrap(), b"protected target");
        }
        assert!(!project.exists());
    }
}

#[test]
fn missing_output_parent_does_not_leave_a_graph_snapshot() {
    let scratch = Scratch::new();
    let project = scratch.0.join("missing/project.prproj");
    let graph = scratch.0.join("graph.json");
    let result = scratch.convert(&project, Some(&graph));
    assert!(!result.status.success());
    assert!(
        !graph.exists(),
        "snapshot was created despite a missing project folder"
    );
    assert!(!project.exists());
}

#[test]
fn case_only_output_names_are_refused_portably() {
    let scratch = Scratch::new();
    for name in ["project.prproj", "Édit.prproj"] {
        let project = scratch.0.join(name);
        let graph = scratch.0.join(name.to_uppercase());
        let result = scratch.convert(&project, Some(&graph));
        assert!(!result.status.success());
        assert!(!project.exists());
        assert!(!graph.exists());
    }
}

#[cfg(unix)]
#[test]
fn dangling_primary_symlink_is_refused_even_without_a_snapshot() {
    let scratch = Scratch::new();
    let project = scratch.0.join("project.prproj");
    let target = scratch.0.join("absent-target.prproj");
    std::os::unix::fs::symlink(&target, &project).unwrap();
    let result = scratch.convert(&project, None);
    assert!(!result.status.success());
    assert_eq!(std::fs::read_link(&project).unwrap(), target);
    assert!(!target.exists());
}

#[test]
fn fresh_snapshot_keeps_project_bytes_and_contains_the_emitted_graph() {
    let scratch = Scratch::new();
    let project = scratch.0.join("project.prproj");
    let graph = scratch.0.join("graph.json");
    let result = scratch.convert(&project, None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let baseline = std::fs::read(&project).unwrap();
    // Preserve the inspected test output, then reuse its path for an exact byte comparison.
    std::fs::rename(&project, scratch.0.join("baseline.prproj")).unwrap();
    let result = scratch.convert(&project, Some(&graph));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(&project).unwrap(), baseline);
    let emitted = Command::new(env!("CARGO_BIN_EXE_any-nle"))
        .current_dir(&scratch.0)
        .args(["graph", "--from", "avid", "source.avb", "--media", "media"])
        .output()
        .unwrap();
    assert!(emitted.status.success());
    assert_eq!(std::fs::read(&graph).unwrap(), emitted.stdout);
    graph::source::SourceGraph::from_json_text(&String::from_utf8(emitted.stdout).unwrap())
        .unwrap();
}

#[test]
fn graph_snapshot_at_the_old_staging_name_survives_project_save() {
    let scratch = Scratch::new();
    let project = scratch.0.join("project.prproj");
    let graph = scratch.0.join("project.prproj.tmp");
    let result = scratch.convert(&project, Some(&graph));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(graph.is_file(), "project save consumed the graph snapshot");
    graph::source::SourceGraph::from_json_text(&std::fs::read_to_string(&graph).unwrap()).unwrap();
    assert!(project.is_file());
    assert!(!scratch.0.join("project.prproj.tmp.2").exists());
}

#[test]
fn rewrap_refuses_media_scan_problems_before_backend_or_any_output() {
    let scratch = Scratch::new();
    let input = scratch.0.join("media/broken.mxf");
    std::fs::write(&input, b"not an MXF").unwrap();
    for dry in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_any-nle"));
        cmd.current_dir(&scratch.0).args([
            "convert",
            "--from",
            "avid",
            "source.avb",
            "--media",
            "media",
            "--to",
            "premiere",
            "out.prproj",
            "--graph",
            "out.json",
            "--rewrap-op1a",
            "new-media",
            "--ffmpeg",
            "missing-backend",
        ]);
        if dry {
            cmd.arg("--rewrap-dry-run");
        }
        let result = cmd.output().unwrap();
        assert!(!result.status.success());
        let err = String::from_utf8_lossy(&result.stderr);
        assert!(err.contains("reader/media scan without problems"), "{err}");
        assert!(!scratch.0.join("new-media").exists());
        assert!(!scratch.0.join("out.prproj").exists());
        assert!(!scratch.0.join("out.json").exists());
        assert_eq!(std::fs::read(&input).unwrap(), b"not an MXF");
    }
}

#[test]
fn color_preview_reads_both_versions_without_preparing_a_timeline() {
    use graph::source::{
        SourceBinding, SourceGraph, SourceMarker, SourceSequence, SourceTrack, SourceTransition,
    };
    use graph::{
        Graph, Label, Marker, MarkerOwner, Project, Rational as Q, Sequence, StreamKind,
    };
    let scratch = Scratch::new();
    let label = Some(Label {
        palette: "avid".into(),
        name: "Red".into(),
    });
    let old = Graph {
        project: Project {
            name: "Color facts".into(),
        },
        media: vec![],
        items: vec![],
        bins: vec![],
        losses: vec![],
        sidecars: vec![],
        sequences: vec![Sequence {
            id: "sequence:color".into(),
            name: "Color facts".into(),
            rate: Q::integer(24),
            frame_size: (1920, 1080),
            start: Q::ZERO,
            tracks: vec![],
            placements: vec![],
            transitions: vec![],
            markers: vec![Marker {
                name: "Palette".into(),
                comment: "".into(),
                user: "".into(),
                color: label.clone(),
                start: Q::ZERO,
                duration: Q::ZERO,
                track: None,
                owner: MarkerOwner::Sequence,
                placement: None,
                source: None,
                inherited: false,
            }],
        }],
    };
    let unresolved = SourceBinding {
        placements: vec![],
        reason: Some("native relationship unresolved".into()),
    };
    let source = SourceGraph {
        multicams: Vec::new(),
        multicam_uses: Vec::new(),
        project: old.project.clone(),
        media: vec![],
        items: vec![],
        bins: vec![],
        losses: vec![],
        sidecars: vec![],
        sequences: vec![SourceSequence {
            disabled_placements: Vec::new(),
            texts: Vec::new(),
            id: "sequence:color".into(),
            name: "Color facts".into(),
            rate: Q::integer(24),
            frame_size: None,
            start: None,
            clock: None,
            tracks: vec![SourceTrack::new(StreamKind::Video, 1, None)],
            placements: vec![],
            transitions: vec![SourceTransition {
                id: "transition:unknown".into(),
                kind: StreamKind::Video,
                track: 1,
                cut: Q::ZERO,
                duration: Q::integer(1),
                cut_offset: Q::ZERO,
                style: "unresolved".into(),
                outgoing: unresolved.clone(),
                incoming: unresolved.clone(),
            }],
            markers: vec![SourceMarker {
                name: "Palette".into(),
                comment: "".into(),
                user: "".into(),
                color: label,
                start: None,
                duration: Q::ZERO,
                track: None,
                owner: MarkerOwner::Sequence,
                owner_occurrence: "native:palette-owner".into(),
                owner_local: None,
                owner_rate: None,
                binding: unresolved,
                source: None,
                inherited: false,
            }],
        }],
    };
    source.validate().unwrap();
    assert!(premiere_adapter::prepare(
        &source,
        &Default::default(),
        premiere_adapter::ClockProfile::LegacyImport
    )
    .is_err());
    let mut results = Vec::new();
    for (name, text) in [
        ("legacy.json", old.to_json_text().unwrap()),
        ("source.json", source.to_json_text().unwrap()),
    ] {
        let path = scratch.0.join(name);
        std::fs::write(&path, text).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_any-nle"))
            .arg("colours")
            .arg(path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("Red"));
        results.push(out.stdout);
    }
    assert_eq!(results[0], results[1]);
    assert!(Graph::from_json_text(&source.to_json_text().unwrap()).is_err());
}

#[test]
fn held_fcpxml_keeps_its_explicit_legacy_snapshot_profile() {
    let scratch = Scratch::new();
    let out = Command::new(env!("CARGO_BIN_EXE_any-nle"))
        .current_dir(&scratch.0)
        .args([
            "convert",
            "--from",
            "avid",
            "source.avb",
            "--media",
            "media",
            "--to",
            "fcpxml",
            "held.fcpxml",
            "--graph",
            "legacy.json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(scratch.0.join("legacy.json")).unwrap();
    graph::Graph::from_json_text(&text).unwrap();
    assert!(graph::source::SourceGraph::from_json_text(&text).is_err());
    assert!(std::fs::read_to_string(scratch.0.join("held.fcpxml"))
        .unwrap()
        .contains("<fcpxml"));
}
