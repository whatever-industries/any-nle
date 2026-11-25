//! Optional stdio MCP host. The client can supply filters and finding IDs only.
use crate::{
    backend_bundle,
    diagnostics::{self, Diagnostics},
    job, offline,
};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::{async_rw::AsyncRwTransport, stdio},
    ErrorData, RoleServer, ServerHandler, ServiceExt,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
struct Relink {
    report: PathBuf,
    moves: PathBuf,
    checksums: Option<PathBuf>,
}

struct Support {
    report: Diagnostics,
    job: Option<job::JobSpec>,
    relink: Option<Relink>,
    scan: Arc<tokio::sync::Semaphore>,
    last_review: Arc<tokio::sync::Mutex<Option<Diagnostics>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Explain {
    id: usize,
}

fn args<T: serde::de::DeserializeOwned>(
    args: Option<rmcp::model::JsonObject>,
) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(args.unwrap_or_default()))
        .map_err(|_| ErrorData::invalid_params("Arguments do not match the tool schema.", None))
}

fn page(report: &Diagnostics, options: Page) -> Value {
    let findings: Vec<_> = report
        .findings
        .iter()
        .skip(options.offset)
        .take(32)
        .map(diagnostics::Finding::tool_value)
        .collect();
    let next = options.offset.saturating_add(findings.len());
    json!({
        "summary": report.summary(),
        "findings": findings,
        "next_offset": (next < report.findings.len()).then_some(next),
    })
}

fn tool(name: &'static str, description: &'static str, input: Value) -> Tool {
    Tool::new(
        name,
        description,
        Arc::new(input.as_object().expect("object tool schema").clone()),
    )
    .with_annotations(ToolAnnotations::from_raw(
        None,
        Some(true),
        None,
        None,
        Some(false),
    ))
}

fn review_job(mut spec: job::JobSpec) -> Diagnostics {
    // Bundled tools take precedence. Source builds can use the launcher's PATH.
    let Ok(tools) = backend_bundle::resolve(None, None, false) else {
        return Diagnostics::blocked(&job::ReviewBlocker::new(job::BlockerKind::MediaTools, ""));
    };
    spec.ffmpeg = Some(tools.ffmpeg);
    spec.ffprobe = Some(tools.ffprobe);
    match job::prepare_for_review(spec, &job::CancelToken::default(), &mut |_| {}) {
        Ok(prepared) => Diagnostics::ready(prepared.review()),
        Err(blocker) => Diagnostics::blocked(&blocker),
    }
}

impl Support {
    async fn check<F>(&self, check: F) -> Result<Diagnostics, ErrorData>
    where
        F: FnOnce() -> Diagnostics + Send + 'static,
    {
        // Avoid running a second full scan while one is hashing media.
        let permit =
            self.scan.clone().try_acquire_owned().map_err(|_| {
                ErrorData::internal_error("A local check is already running.", None)
            })?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            check()
        })
        .await
        .map_err(|_| ErrorData::internal_error("The local check stopped unexpectedly.", None))
    }
}

impl ServerHandler for Support {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("any-nle", diagnostics::app_version()))
            .with_instructions("Inspect the selected redacted Any NLE diagnostics. Finding IDs refer to one result. Use exact saved positions where present. Unknown positions require the user's local Review.")
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let pagination = json!({"type":"object","additionalProperties":false,
            "properties":{"offset":{"type":"integer","minimum":0}}});
        let mut tools = vec![
            tool(
                "summarize_diagnostics",
                "Summarize the selected diagnostics report and list up to 32 findings.",
                pagination.clone(),
            ),
            tool(
                "explain_finding",
                "Explain a finding ID from the selected diagnostics report.",
                json!({"type":"object","additionalProperties":false,"required":["id"],
                    "properties":{"id":{"type":"integer","minimum":1}}}),
            ),
        ];
        if self.job.is_some() {
            tools.push(tool("review_job", "Run read-only Review on the Avid to Premiere job supplied at launch and return up to 32 findings.", pagination));
        }
        if self.relink.is_some() {
            tools.push(tool(
                "verify_relink",
                "Verify the proposed moves supplied at launch before using Link Media.",
                json!({"type":"object","additionalProperties":false}),
            ));
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name;
        let unknown_tool =
            || ErrorData::invalid_params("Tool is unavailable in this session.", None);
        let value = match name.as_ref() {
            "summarize_diagnostics" => page(&self.report, args(request.arguments)?),
            "explain_finding" => {
                let Explain { id } = args(request.arguments)?;
                id.checked_sub(1)
                    .and_then(|index| self.report.findings.get(index))
                    .ok_or_else(|| {
                        ErrorData::invalid_params(
                            "Finding ID is outside the selected report.",
                            None,
                        )
                    })?
                    .tool_value()
            }
            "review_job" => {
                let job = self.job.clone().ok_or_else(unknown_tool)?;
                let options: Page = args(request.arguments)?;
                if options.offset == 0 {
                    let result = self.check(move || review_job(job)).await?;
                    let output = page(&result, options);
                    *self.last_review.lock().await = Some(result);
                    output
                } else {
                    let last = self.last_review.lock().await;
                    let result = last.as_ref().ok_or_else(|| {
                        ErrorData::invalid_params("Start a review at offset zero.", None)
                    })?;
                    page(result, options)
                }
            }
            "verify_relink" => {
                let relink = self.relink.clone().ok_or_else(unknown_tool)?;
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Empty {}
                let _: Empty = args(request.arguments)?;
                let result = self
                    .check(move || match backend_bundle::resolve(None, None, false) {
                        Ok(tools) => offline::verify_diagnostics(
                            &relink.report,
                            &relink.moves,
                            relink.checksums.as_deref(),
                            &tools.ffprobe,
                        ),
                        Err(_) => Diagnostics::relink(Err(diagnostics::Reason::MediaTools)),
                    })
                    .await?;
                page(&result, Page { offset: 0 })
            }
            _ => return Err(unknown_tool()),
        };
        Ok(CallToolResult::structured(value).into())
    }
}

fn selected_file(path: &str) -> Result<PathBuf, &'static str> {
    let path = std::fs::canonicalize(Path::new(path))
        .map_err(|_| "Could not resolve a selected input file.")?;
    if !path.is_file() {
        return Err("Selected inputs must be regular files.");
    }
    Ok(path)
}

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let mut report = None;
    let mut job_path = None;
    let mut relink_report = None;
    let mut moves = None;
    let mut checksums = None;
    let mut pairs = args.chunks_exact(2);
    for pair in &mut pairs {
        let selection = match pair[0].as_str() {
            "--diagnostics" => &mut report,
            "--job" => &mut job_path,
            "--relink-report" => &mut relink_report,
            "--moves" => &mut moves,
            "--checksums" => &mut checksums,
            _ => return Err("Unknown MCP input option.".into()),
        };
        if selection.is_some() {
            return Err("Duplicate MCP input option.".into());
        }
        *selection = Some(selected_file(&pair[1])?);
    }
    if !pairs.remainder().is_empty() {
        return Err("Each MCP input option needs a file path.".into());
    }
    let report = Diagnostics::load(
        report
            .as_deref()
            .ok_or("MCP needs --diagnostics REPORT.json.")?,
    )?;
    let job = job_path
        .as_deref()
        .map(|path| {
            let job =
                job::load(path).map_err(|_| "Could not load the selected Avid to Premiere job.")?;
            if job.schema != "anynle.job/1" || job.ffmpeg.is_some() || job.ffprobe.is_some() {
                return Err(
                    "MCP Review requires an Avid to Premiere job using the installed media tools.",
                );
            }
            Ok(job)
        })
        .transpose()?;
    let relink = match (relink_report, moves, checksums) {
        (Some(report), Some(moves), checksums) => Some(Relink {
            report,
            moves,
            checksums,
        }),
        (None, None, None) => None,
        _ => return Err("MCP relink verification needs both --relink-report and --moves.".into()),
    };
    let support = Support {
        report,
        job,
        relink,
        scan: Arc::new(tokio::sync::Semaphore::new(1)),
        last_review: Arc::new(tokio::sync::Mutex::new(None)),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start the MCP runtime.")?;
    runtime.block_on(async move {
        let (read, write) = stdio();
        let service = support
            .serve(AsyncRwTransport::new(read, write).with_max_line_length(64 << 10))
            .await
            .map_err(|_| "Could not initialize the MCP connection.")?;
        service
            .waiting()
            .await
            .map_err(|_| "The MCP connection stopped unexpectedly.")?;
        Ok(())
    })
}
