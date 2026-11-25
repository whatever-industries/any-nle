//! Client smoke tests against the packaged CLI entry point.
use anynle_app::{
    diagnostics::Diagnostics,
    job::{BlockerKind, JobSpec, ReviewBlocker},
};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, BufReader, Cursor, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Client {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<String>,
    id: u64,
}

impl Client {
    fn start(report: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_any-nle"))
            .args(["mcp", "--diagnostics"])
            .arg(report)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            lines,
            id: 0,
        };
        let initialize = client.request(
            "initialize",
            json!({
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"headless-acceptance","version":"1.0.0"}
            }),
        );
        assert_eq!(initialize["result"]["serverInfo"]["name"], "any-nle");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }

    fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params}));
        let line = self.lines.recv_timeout(Duration::from_secs(20)).unwrap();
        let result: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(result["id"], self.id);
        result
    }

    fn tools(&mut self) -> Vec<String> {
        self.request("tools/list", json!({}))["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_json(path: &Path, value: Value) {
    std::fs::write(path, value.to_string()).unwrap();
}

#[test]
fn report_only_does_not_expose_local_checks_or_accept_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("diagnostics.json");
    let blocker = ReviewBlocker::new(
        BlockerKind::RewrapMissingOriginal,
        "Do not send private paths.",
    );
    Diagnostics::blocked(&blocker).save(&path).unwrap();
    let mut client = Client::start(&path, &[]);
    assert_eq!(client.tools(), ["summarize_diagnostics", "explain_finding"]);
    let summary = client.tool("summarize_diagnostics", json!({}));
    let result = &summary["result"]["structuredContent"];
    assert_eq!(result["summary"]["outcome"], "blocked");
    assert_eq!(result["findings"][0]["reason"], "missing_original");
    let explanation = client.tool("explain_finding", json!({"id":1}));
    assert_eq!(
        explanation["result"]["structuredContent"],
        result["findings"][0]
    );
    assert!(!summary.to_string().contains("private paths"));
    assert!(client.tool("review_job", json!({})).get("error").is_some());
    assert!(client
        .tool("summarize_diagnostics", json!({"path":"/private/path"}))
        .get("error")
        .is_some());
}

#[test]
fn scoped_mcp_review_and_wrong_package_relink_remain_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("diagnostics.json");
    let blocker = ReviewBlocker::new(BlockerKind::OriginalLocator, "Saved path unavailable.");
    Diagnostics::blocked(&blocker).save(&path).unwrap();
    let bin = root.join("private-bin.avb");
    let avb = include_bytes!("../../../avid-reader/tests/fixtures/pyavb_empty.avb");
    std::fs::write(&bin, avb).unwrap();
    let output = root.join("new.prproj");
    let job = root.join("job.json");
    write_json(
        &job,
        json!(JobSpec {
            bins: vec![bin.clone()],
            output: output.clone(),
            mode: anynle_app::job::ClipMode::Separate,
            media_roots: vec![root.clone()],
            ..Default::default()
        }),
    );
    let candidate = root.join("Original.mxf");
    let mut fixture = zip::ZipArchive::new(Cursor::new(include_bytes!(
        "../../../tests/fixtures/avid-resolve-release/split-edit.zip"
    )))
    .unwrap();
    let mut wrong_mxf = Vec::new();
    io::copy(&mut fixture.by_name("Picture.mxf").unwrap(), &mut wrong_mxf).unwrap();
    std::fs::write(&candidate, &wrong_mxf).unwrap();
    let relink = root.join("offline.json");
    let moves = root.join("moves.json");
    write_json(
        &relink,
        json!({
            "schema":"anynle.offline-relink/1", "offline":[{"item":"private-id","bin":"private-bin",
            "name":"private-clip","path":"/private/Original.mxf","saved_locators":[],
            "package":"0000000000000000000000000000000000000000000000000000000000000000",
            "duration":"2","kind":"picture"}], "online_originals":[], "online_media":[]
        }),
    );
    write_json(
        &moves,
        json!([{"original":"/private/Original.mxf","destination":candidate}]),
    );
    let mut client = Client::start(
        &path,
        &[
            "--job",
            job.to_str().unwrap(),
            "--relink-report",
            relink.to_str().unwrap(),
            "--moves",
            moves.to_str().unwrap(),
        ],
    );
    assert_eq!(
        client.tools(),
        [
            "summarize_diagnostics",
            "explain_finding",
            "review_job",
            "verify_relink"
        ]
    );
    let before = std::fs::read_dir(&root).unwrap().count();
    let reply = client.tool("review_job", json!({}));
    assert_eq!(
        reply["result"]["structuredContent"]["summary"]["outcome"], "ready",
        "{reply}"
    );
    let verified = client.tool("verify_relink", json!({}));
    let result = &verified["result"]["structuredContent"];
    assert_eq!(result["summary"]["outcome"], "blocked", "{verified}");
    assert_eq!(
        result["findings"][0]["reason"], "mxf_package_mismatch",
        "{verified}"
    );
    assert!(!verified.to_string().contains("private"));
    assert_eq!(std::fs::read_dir(root).unwrap().count(), before);
    assert_eq!(std::fs::read(candidate).unwrap(), wrong_mxf);
    assert_eq!(std::fs::read(bin).unwrap(), avb);
    assert!(!output.exists());
}
