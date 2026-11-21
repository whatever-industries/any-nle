//! FCPXML export and the four CLIs against Python (`tools/expected_export_cli.py`).
//!
//! Every row of `tests/data/export_cli/index.tsv` is replayed: the FCPXML export of each
//! sequence of each corpus project (file hash, or the exception as `Class: message`), and every
//! CLI command in-process (`prproj::cli::*::main`) with the same arguments. Stdout (hash and
//! length), the exit code, stderr and any file the command writes must equal Python's.
//! Rows whose argv names an argparse usage error compare only the exit code (argparse's usage
//! wording is not reproduced). A sample of rows is also run through the compiled binaries.
//!
//! Regenerate with
//! `PYTHONHASHSEED=0 PYTHONPATH=src:vendor python3 prproj/tools/expected_export_cli.py`.

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{base, data, Scratch};
use prproj::convert::model::{parse_json, Json};
use prproj::export::export_fcpxml;
use prproj::hash::{hex, sha256};
use prproj::model::application;

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Row {
    id: String,
    kind: String,
    argv: Vec<String>,
    exit: i32,
    stdout_sha: String,
    stdout_len: usize,
    stderr: String,
    file_sha: String,
}

fn parse_rows() -> Vec<Row> {
    let text = std::fs::read_to_string(data("export_cli/index.tsv")).unwrap();
    text.lines()
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            let argv = match parse_json(f[2]).unwrap() {
                Json::Arr(items) => items.into_iter().map(|j| if let Json::Str(s) = j { s } else { panic!("argv") }).collect(),
                _ => panic!("argv"),
            };
            let stderr = match parse_json(f[6]).unwrap() {
                Json::Str(s) => s,
                _ => panic!("stderr"),
            };
            Row {
                id: f[0].into(),
                kind: f[1].into(),
                argv,
                exit: f[3].parse().unwrap(),
                stdout_sha: f[4].into(),
                stdout_len: f[5].parse().unwrap(),
                stderr,
                file_sha: f[7].into(),
            }
        })
        .collect()
}

fn sha(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

fn fcpxml_digest(path: &Path) -> String {
    // The oracle recorded relative media as file URLs rooted at its original
    // working directory. Normalize only that prefix in media references.
    const MEDIA_URL: &str = "<media-rep kind=\"original-media\" src=\"file://";
    const ORACLE_PREFIX: &str = "/private/tmp/prproj-export-cli-cwd/";
    let cwd = std::env::current_dir().unwrap();
    let mut prefix = MEDIA_URL.to_owned();
    for &byte in cwd.to_str().unwrap().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            prefix.push(char::from(byte));
        } else {
            write!(prefix, "%{byte:02X}").unwrap();
        }
    }
    prefix.push('/');
    let xml = std::fs::read_to_string(path).unwrap();
    sha(xml.replace(&prefix, &format!("{MEDIA_URL}{ORACLE_PREFIX}")).as_bytes())
}

/// Whether every project the case names exists (the corpus lives outside the repository).
/// Paths are relative to the test workspace, whose links point at `common::base()`.
fn available(row: &Row) -> bool {
    row.argv.iter().filter(|a| a.ends_with(".prproj") && !a.ends_with("does-not-exist.prproj")).all(|a| Path::new(a).exists())
}

/// The file `out/<id>.*` a case wrote.
fn output_file(id: &str) -> Option<PathBuf> {
    let prefix = format!("{id}.");
    std::fs::read_dir("out")
        .unwrap()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
}

struct Outcome {
    exit: i32,
    stdout: Vec<u8>,
    stderr: String,
    file: Option<PathBuf>,
}

fn run_in_process(row: &Row) -> Outcome {
    if let Some(index) = row.kind.strip_prefix("fcpxml:") {
        let index: usize = index.parse().unwrap();
        let target = format!("out/{}.fcpxml", row.id);
        let mut stderr = String::new();
        let mut file = None;
        match application::parse(&row.argv[0], None) {
            Err(e) => stderr = format!("{}: {e}\n", e.py_class()),
            Ok(app) => {
                let project = app.project();
                let sequence = project.sequences().get(index).copied();
                match export_fcpxml(project, &target, sequence) {
                    Ok(_) => file = Some(PathBuf::from(target)),
                    Err(e) => stderr = format!("{}: {e}\n", e.py_class()),
                }
            }
        }
        return Outcome { exit: 0, stdout: Vec::new(), stderr, file };
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let kind = row.kind.trim_end_matches('U');
    let exit = match kind {
        "inspect" => prproj::cli::inspect::main(&row.argv, &mut out, &mut err),
        "compare" => prproj::cli::compare::main(&row.argv, &mut out, &mut err),
        "validate" => prproj::cli::validate::main(&row.argv, &mut out, &mut err),
        "visualize" => prproj::cli::visualize::main(&row.argv, &mut out, &mut err),
        other => panic!("unknown kind {other}"),
    };
    Outcome {
        exit,
        stdout: out,
        stderr: String::from_utf8_lossy(&err).into_owned(),
        file: output_file(&row.id),
    }
}

fn compare(row: &Row, got: &Outcome, failures: &mut Vec<String>) {
    let usage = row.kind.ends_with('U');
    let mut problems = Vec::new();
    if got.exit != row.exit {
        problems.push(format!("exit {} vs python {}", got.exit, row.exit));
    }
    if !usage {
        if sha(&got.stdout) != row.stdout_sha || got.stdout.len() != row.stdout_len {
            problems.push(format!("stdout differs ({} bytes vs python {})", got.stdout.len(), row.stdout_len));
        }
        if got.stderr != row.stderr {
            problems.push(format!("stderr {:?} vs python {:?}", got.stderr, row.stderr));
        }
        let file_sha = match got.file.as_deref() {
            Some(file) if row.kind.starts_with("fcpxml:") => fcpxml_digest(file),
            Some(file) => sha(&std::fs::read(file).unwrap()),
            None => "-".into(),
        };
        if file_sha != row.file_sha {
            problems.push("written file differs".into());
        }
    }
    if problems.is_empty() {
        if let Some(file) = &got.file {
            std::fs::remove_file(file).unwrap();
        }
    } else {
        let cwd = std::env::current_dir().unwrap();
        let dir = cwd.join("diagnostics");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{}.out", row.id)), &got.stdout).unwrap();
        let retained_file = got.file.as_ref().map_or_else(String::new, |file| {
            format!(", rust file: {}", cwd.join(file).display())
        });
        failures.push(format!(
            "{} {} {:?}: {} (rust stdout: {}/{}.out, python: /tmp/prproj-export-cli/py/{}.out{retained_file})",
            row.id,
            row.kind,
            row.argv,
            problems.join("; "),
            dir.display(),
            row.id,
            row.id
        ));
    }
}

struct Workspace {
    scratch: Scratch,
    previous_dir: PathBuf,
}

impl Workspace {
    /// Enter an isolated workspace with the same relative paths as the Python oracle.
    /// The caller holds LOCK until this guard restores the process's working directory.
    fn enter() -> Self {
        let previous_dir = std::env::current_dir().unwrap();
        let corpus = previous_dir.join(base());
        let scratch = Scratch::new("prproj-export-cli");
        std::fs::create_dir(scratch.path().join("out")).unwrap();
        for name in ["premiere-format-research", "converter"] {
            std::os::unix::fs::symlink(corpus.join(name), scratch.path().join(name)).unwrap();
        }
        std::env::set_current_dir(scratch.path()).unwrap();
        Self { scratch, previous_dir }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let restored = std::env::set_current_dir(&self.previous_dir);
        if !std::thread::panicking() {
            restored.expect("restore working directory before removing the test workspace");
        }
    }
}

#[test]
fn export_and_cli_match_python() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let rows = parse_rows();
    let _workspace = Workspace::enter();
    let (mut compared, mut skipped) = (0, 0);
    let mut failures = Vec::new();
    let mut by_kind = std::collections::BTreeMap::<String, usize>::new();
    for row in &rows {
        if !available(row) {
            skipped += 1;
            continue;
        }
        let got = run_in_process(row);
        compare(row, &got, &mut failures);
        compared += 1;
        *by_kind.entry(row.kind.split(':').next().unwrap().to_string()).or_default() += 1;
    }
    eprintln!("export_cli: {compared} cases compared ({by_kind:?}), {skipped} skipped (corpus file absent), {} failed", failures.len());
    assert!(failures.is_empty(), "{} mismatches, first {}:\n{}", failures.len(), failures.len().min(8), failures[..failures.len().min(8)].join("\n"));
    if skipped == 0 {
        assert!(compared >= 2000, "only {compared} cases");
    }
}

fn bin(kind: &str) -> PathBuf {
    PathBuf::from(match kind {
        "inspect" => env!("CARGO_BIN_EXE_prproj-inspect"),
        "compare" => env!("CARGO_BIN_EXE_prproj-compare"),
        "validate" => env!("CARGO_BIN_EXE_prproj-validate"),
        "visualize" => env!("CARGO_BIN_EXE_prproj-visualize"),
        other => panic!("{other}"),
    })
}

/// The compiled binaries give Python's stdout, stderr and exit status (a sample of the rows).
#[test]
fn binaries_match_python() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let rows = parse_rows();
    let workspace = Workspace::enter();
    let mut failures = Vec::new();
    let mut ran = 0;
    for (n, row) in rows.iter().enumerate() {
        let kind = row.kind.trim_end_matches('U');
        if row.kind.starts_with("fcpxml") || !available(row) || (n % 23 != 0 && !row.kind.ends_with('U')) {
            continue;
        }
        let output = Command::new(bin(kind))
            .args(&row.argv)
            .current_dir(workspace.scratch.path())
            .output()
            .unwrap();
        ran += 1;
        let got = Outcome {
            exit: output.status.code().unwrap(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            file: output_file(&row.id),
        };
        compare(row, &got, &mut failures);
    }
    eprintln!("binaries: {ran} cases run");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures[..failures.len().min(8)].join("\n"));
}
