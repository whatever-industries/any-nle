//! Read-only preparation followed by one explicit execution of the prepared document.
use super::*;
mod media_blocker;
mod review_details;
use media_blocker::unresolved_masters;
use review_details::{
    append_source_blocker_details, group_clip_mode_blocker, job_preparation_blocker,
    locate_native_review_notes, locate_source_media_gaps, media_scan_blocker, review_source_groups,
    review_subjects, review_timeline_groups, source_read_blocker,
};
#[cfg(test)]
use review_details::{append_group_preparation_details, premiere_preparation_blocker};
pub(crate) use review_details::sequence_timing_blocker;
use crate::premiere_import::avid;
use crate::source_input::{original_stamp, recheck_originals_checked, OriginalStamp};
use crate::{backend_bundle, output, premiere_preparation, rewrap, source_input};
use avid_reader::source::SourceInput;
use mapping::{AvidToPremiere, BinMode};
use prproj::convert::{prepared::PreparedClip, writer::PreparedProject};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_JOB_BYTES: usize = 1 << 20;
const RECEIPT_SUFFIX: &str = ".anynle-report.json";
const REPLAY_SUFFIX: &str = ".anynle-job.json";
const DEBUG_SUFFIX: &str = ".anynle-debug.jsonl";
use base64::Engine;
pub fn load_encoded(encoded: &str) -> Result<JobSpec, String> {
    if encoded.len() > ((MAX_JOB_BYTES + 2) / 3) * 4 {
        return Err("Encoded job settings exceed 1 MiB".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("Invalid encoded job settings: {e}"))?;
    if bytes.len() > MAX_JOB_BYTES {
        return Err("Job settings exceed 1 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid job settings: {e}"))
}
fn cli_command(spec: &JobSpec) -> Result<String, String> {
    let text = serde_json::to_vec(spec).map_err(|e| e.to_string())?;
    let command = format!(
        "any-nle run --job-base64 {}",
        base64::engine::general_purpose::STANDARD.encode(text)
    );
    Ok(if command.len() <= 6000 {
        command
    } else {
        String::new()
    })
}
fn note(
    severity: &str,
    object: impl Into<String>,
    message: impl Into<String>,
    why: impl Into<String>,
) -> ReportNote {
    ReportNote {
        severity: severity.into(),
        object: object.into(),
        message: message.into(),
        why: why.into(),
    }
}
fn progress(cb: &mut dyn FnMut(Progress), stage: &str, message: &str) {
    cb(Progress {
        stage: stage.into(),
        current_bin: None,
        completed: 0,
        total: 0,
        message: message.into(),
    })
}
fn reader_findings(count: usize) -> String {
    let label = if count == 1 { "finding" } else { "findings" };
    format!("{count} reader {label}")
}
fn sidecar(output: &Path, suffix: &str) -> Result<PathBuf, String> {
    let mut name = output
        .file_name()
        .ok_or("Project output needs a file name")?
        .to_os_string();
    name.push(suffix);
    Ok(output.with_file_name(name))
}
fn path_key(path: &Path) -> Result<String, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let root = std::fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    Ok(root
        .join(path.file_name().ok_or("Output needs a name")?)
        .to_string_lossy()
        .to_lowercase())
}
fn check_preflight_report_path<'a>(
    path: &Path,
    reserved: impl IntoIterator<Item = &'a Path>,
) -> Result<(), String> {
    if path
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("json"))
    {
        return Err("Choose a .json path for the preflight report".into());
    }
    let key = path_key(path)?;
    for output in reserved {
        // A blocked job can have missing output parents. The report's parent
        // exists, so paths with unresolved parents cannot alias its location.
        if path_key(output).is_ok_and(|output_key| key == output_key) {
            return Err(format!(
                "Choose a different report path. {} is reserved for conversion.",
                output.display()
            ));
        }
    }
    Ok(())
}

/// Save the diagnostic for the stage that stopped preflight.
pub fn save_blocked_preflight_report(
    spec: &JobSpec,
    blocker: &ReviewBlocker,
    path: &Path,
) -> Result<(), String> {
    check_blocked_report_path(spec, path)?;
    write_json(path, &blocked_preflight_report(spec, blocker), &mut Vec::new())
}

pub fn check_blocked_report_path(spec: &JobSpec, path: &Path) -> Result<(), String> {
    let sidecars: Vec<_> = [
        RECEIPT_SUFFIX,
        REPLAY_SUFFIX,
        crate::offline::RELINK_SUFFIX,
        DEBUG_SUFFIX,
    ]
    .into_iter()
    .filter_map(|suffix| sidecar(&spec.output, suffix).ok())
    .collect();
    check_preflight_report_path(
        path,
        std::iter::once(spec.output.as_path())
            .chain(sidecars.iter().map(PathBuf::as_path))
            .chain(spec.new_media.as_deref()).chain(spec.rewrap_needed.as_deref()),
    )
}

pub fn blocked_preflight_report<'a>(
    spec: &'a JobSpec,
    blocker: &'a ReviewBlocker,
) -> impl serde::Serialize + 'a {
    #[derive(serde::Serialize)]
    struct BlockedReport<'a> {
        schema: &'static str,
        status: &'static str,
        project_output: &'a Path,
        blocker: &'a ReviewBlocker,
    }
    BlockedReport {
        schema: PREFLIGHT_REPORT_SCHEMA,
        status: "blocked",
        project_output: &spec.output,
        blocker,
    }
}

fn check_project_paths(spec: &JobSpec, receipt: &Path, replay: &Path) -> Result<(), String> {
    output::preflight(&spec.output, Some(receipt))?;
    output::preflight(&spec.output, Some(replay))?;
    if path_key(receipt)? == path_key(replay)? {
        return Err("Receipt and replay paths overlap".into());
    }
    Ok(())
}
fn check_new_media_paths(spec: &JobSpec, receipt: &Path, replay: &Path) -> Result<(), String> {
    if let Some(dir) = spec.new_media.as_ref().or(spec.rewrap_needed.as_ref()) {
        let dir = path_key(dir)?;
        for p in [&spec.output, receipt, replay] {
            let p = path_key(p)?;
            if p == dir || p.starts_with(&(dir.clone() + std::path::MAIN_SEPARATOR_STR)) {
                return Err(
                    "Project, report and replay must be outside the new-media folder".into(),
                );
            }
        }
    }
    Ok(())
}
fn check_paths(spec: &JobSpec, receipt: &Path, replay: &Path) -> Result<(), String> {
    check_project_paths(spec, receipt, replay)?;
    check_new_media_paths(spec, receipt, replay)
}
/// One-shot, Send-safe value. It contains encoded project bytes, never a live editor or UUID source.
pub struct PreparedJob {
    spec: JobSpec,
    review: Review,
    document: PreparedProject,
    media: Option<rewrap::Plan>,
    media_created: bool,
    inputs: Vec<SourceInput>,
    media_inputs: Vec<SourceInput>,
    originals: Vec<OriginalStamp>,
    proxy_bindings: Vec<avid_adapter::proxy::Binding>,
    receipt: PathBuf,
    replay: PathBuf,
    offline_relink: Option<(PathBuf, String)>,
    graph_debug: Option<(PathBuf, std::fs::File)>,
    aux_outputs: Vec<PathBuf>,
}
impl PreparedJob {
    pub fn review(&self) -> &Review {
        &self.review
    }

    /// Export all findings from this preflight without consuming the prepared job.
    pub fn save_preflight_report(&self, path: &Path) -> Result<(), String> {
        self.check_report_path(path)?;
        write_json(path, &self.preflight_report(), &mut Vec::new())
    }

    pub fn check_report_path(&self, path: &Path) -> Result<(), String> {
        let reserved = [
            Some(self.spec.output.as_path()),
            Some(self.receipt.as_path()),
            Some(self.replay.as_path()),
            self.media.as_ref().map(rewrap::Plan::directory),
            self.offline_relink.as_ref().map(|(path, _)| path.as_path()),
            self.graph_debug.as_ref().map(|(path, _)| path.as_path()),
        ];
        let proxy_outputs = self
            .proxy_bindings
            .iter()
            .filter(|binding| binding.audio.is_some())
            .map(|binding| binding.path.as_path());
        check_preflight_report_path(path, reserved.into_iter().flatten().chain(proxy_outputs))?;
        self.document
            .check_reserved_output(path)
            .map_err(|error| error.to_string())
    }

    pub fn preflight_report(&self) -> impl serde::Serialize + '_ {
        #[derive(serde::Serialize)]
        struct PreflightReport<'a> {
            schema: &'static str,
            status: &'static str,
            project_output: &'a Path,
            bins_checked: usize,
            found_media: usize,
            total_media: usize,
            inventory_notes: &'a [String],
            #[serde(skip_serializing_if = "Option::is_none")]
            media_linking: Option<&'a MediaLinking>,
            subjects: std::collections::BTreeMap<&'a str, &'a str>,
            #[serde(skip_serializing_if = "<[ReviewGroup]>::is_empty")]
            timeline_groups: &'a [ReviewGroup],
            #[serde(skip_serializing_if = "<[ReviewGroup]>::is_empty")]
            source_groups: &'a [ReviewGroup],
            report: &'a [ReportNote],
        }
        let review = &self.review;
        PreflightReport {
            schema: PREFLIGHT_REPORT_SCHEMA,
            status: "ready",
            project_output: &self.spec.output,
            bins_checked: review.inventory.bins.len(),
            found_media: review.inventory.found_media,
            total_media: review.inventory.total_media,
            inventory_notes: &review.inventory.notes,
            media_linking: review.media_linking.as_ref(),
            subjects: review
                .subjects
                .iter()
                .map(|(id, name)| (id.as_str(), name.as_str()))
                .collect(),
            timeline_groups: &review.timeline_groups,
            source_groups: &review.source_groups,
            report: &review.notes,
        }
    }

    /// Move the display names to a client that will browse the completion report.
    pub fn take_review_subjects(&mut self) -> HashMap<String, String> {
        std::mem::take(&mut self.review.subjects)
    }
}

pub fn load(path: &Path) -> Result<JobSpec, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_JOB_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_JOB_BYTES {
        return Err("Job settings exceed 1 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid job settings: {e}"))
}
/// No destination files or directories are created by this function.
pub fn prepare(
    spec: JobSpec,
    cancel: &CancelToken,
    cb: &mut dyn FnMut(Progress),
) -> Result<PreparedJob, String> {
    prepare_job(spec, false, cancel, cb).map_err(ReviewBlocker::cli_summary)
}

fn blocker(kind: BlockerKind) -> impl FnOnce(String) -> ReviewBlocker {
    move |message| ReviewBlocker::new(kind, message)
}

fn camera_original_links(source: &graph::source::SourceGraph) -> Vec<CameraOriginalLink> {
    let media: HashMap<_, _> = source.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let proxies: HashMap<_, _> = source
        .sidecars
        .iter()
        .filter(|s| s.adapter == avid_adapter::proxy::ADAPTER)
        .filter_map(|s| match s.data.get("path") {
            Some(graph::Value::Str(path)) => Some((s.object.as_str(), path)),
            _ => None,
        })
        .collect();
    source
        .items
        .iter()
        .filter_map(|item| {
            let streams = || {
                item.media
                    .iter()
                    .filter_map(|binding| media.get(binding.media.as_str()).copied())
            };
            let picture = streams()
                .find(|m| {
                    m.kind == graph::StreamKind::Video
                        && m.original.is_some()
                        && m.online == Some(true)
                        && m.clock.as_ref().is_some_and(|clock| {
                            clock.provenance.route == graph::source::AVID_SOURCE_COLUMNS_TIMECODE
                        })
                })?
                .locations
                .first()?;
            let audio_channels = streams()
                .filter(|m| m.kind == graph::StreamKind::Audio && m.locations.first() == Some(picture))
                .filter_map(|m| {
                    let channel = m.original.as_ref()?.channel_index?.checked_add(1)?;
                    Some((m.stream?, channel))
                })
                .collect();
            Some(CameraOriginalLink {
                name: item.name.clone(),
                picture: picture.into(),
                audio_channels,
                proxy: proxies.get(item.id.as_str()).map(|path| PathBuf::from(path.as_str())),
            })
        })
        .collect()
}

pub fn prepare_for_review(
    spec: JobSpec,
    cancel: &CancelToken,
    cb: &mut dyn FnMut(Progress),
) -> Result<PreparedJob, ReviewBlocker> {
    prepare_job(spec, true, cancel, cb)
}

fn prepare_job(
    spec: JobSpec,
    for_review: bool,
    cancel: &CancelToken,
    cb: &mut dyn FnMut(Progress),
) -> Result<PreparedJob, ReviewBlocker> {
    use BlockerKind::*;
    let look_up_camera_originals = spec.prefer_source_media || spec.attach_avid_proxies;
    if spec.allow_offline_media && spec.mode == ClipMode::RewrapNative {
        return Err(ReviewBlocker::new(Rewrap, "Allow offline media requires Separate files, Grouped or Merged"));
    }
    let linked = spec.mode != ClipMode::RewrapNative;
    if !linked && spec.rewrap_needed.is_some() {
        return Err(ReviewBlocker::new(Rewrap, "Rewrap only where needed requires a linked clip mode"));
    }
    if linked && spec.new_media.is_some() {
        return Err(ReviewBlocker::new(Rewrap, "Use the selective rewrap folder in linked clip modes"));
    }
    let assisted = cfg!(target_os = "macos") && linked && !spec.attach_avid_proxies;
    if spec.proxy_rewrap_dir.is_some() && !spec.attach_avid_proxies {
        return Err(ReviewBlocker::new(Rewrap, "Proxy rewrap requires Attach Avid media as proxies. Creates new media."));
    }
    if look_up_camera_originals && spec.mode == ClipMode::RewrapNative {
        return Err(ReviewBlocker::new(Rewrap, "Prefer source media requires Separate files, Grouped or Merged"));
    }
    if let Some(folder) = &spec.proxy_rewrap_dir {
        if !folder.is_dir() {
            return Err(ReviewBlocker::new(ProxyMedia, format!(
                "Choose an existing folder for rewrapped proxies. {} is unavailable.",
                folder.display(),
            )));
        }
    }
    let check_cancel = || cancel.check().map_err(blocker(Retry));
    check_cancel()?;
    if spec.schema != "anynle.job/1" {
        return Err(ReviewBlocker::new(ProjectSource, "Unsupported job schema"));
    }
    if serde_json::to_vec(&spec)
        .map_err(|e| ReviewBlocker::new(ProjectSource, e.to_string()))?
        .len() > MAX_JOB_BYTES
    {
        return Err(ReviewBlocker::new(ProjectSource, "Job settings exceed 1 MiB"));
    }
    if spec
        .output
        .extension()
        .is_none_or(|x| !x.eq_ignore_ascii_case("prproj"))
    {
        return Err(ReviewBlocker::new(ProjectOutput, "Choose a .prproj project output"));
    }
    let receipt = sidecar(&spec.output, RECEIPT_SUFFIX).map_err(blocker(ProjectOutput))?;
    let replay = sidecar(&spec.output, REPLAY_SUFFIX).map_err(blocker(ProjectOutput))?;
    check_project_paths(&spec, &receipt, &replay).map_err(blocker(ProjectOutput))?;
    check_new_media_paths(&spec, &receipt, &replay).map_err(blocker(NewMedia))?;
    let mut inv = super::inventory(&spec, cancel, cb).map_err(blocker(ProjectSource))?;
    let bins = super::inventory::selected_paths(&spec, &inv).map_err(blocker(ProjectSource))?;
    let inputs = bins
        .iter()
        .map(|path| {
            let revision = inv
                .bin_revisions
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, r)| r.clone())
                .ok_or_else(|| format!("Inventory lacks a revision for {}", path.display()))?;
            Ok(SourceInput {
                path: path.clone(),
                revision,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(blocker(Retry))?;
    source_input::recheck_checked(&inputs, &mut || cancel.check())
        .map_err(|e| ReviewBlocker::new(Retry, format!("Bin inventory changed; run pre-flight again: {e}")))?;
    for choice in &spec.effects {
        let capability = inv
            .effects
            .iter()
            .find(|e| e.key == choice.key)
            .ok_or_else(|| ReviewBlocker::new(Effects, format!("Unknown effect choice {}", choice.key)))?;
        if !capability.modes.contains(&choice.mode) {
            return Err(ReviewBlocker::new(Effects, format!(
                "Effect {} does not support that conversion mode",
                choice.key
            )));
        }
    }
    let mut policy = if spec.policy_json.trim().is_empty() {
        AvidToPremiere::default()
    } else {
        AvidToPremiere::from_json_text(&spec.policy_json)
            .map_err(|e| ReviewBlocker::new(ProjectSource, e.to_string()))?
    };
    policy.bin_mode = match spec.mode {
        ClipMode::RewrapNative | ClipMode::Separate => BinMode::Separate,
        ClipMode::Grouped => BinMode::Grouped,
        ClipMode::Merged => BinMode::Merged,
    };
    // The packaged wizard requires verified bundled tools. Explicit developer paths are deliberate overrides.
    let tools = backend_bundle::resolve(spec.ffmpeg.as_deref(), spec.ffprobe.as_deref(), true)
        .map_err(blocker(MediaTools))?;
    progress(
        cb,
        "source",
        "Reading qualified Avid source facts and resolving media",
    );
    check_cancel()?;
    let names = bins
        .iter()
        .map(|p| {
            p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    let project = spec
        .project_name
        .clone()
        .unwrap_or_else(|| names[0].clone());
    let read_source = |cb: &mut dyn FnMut(Progress)| {
        avid_adapter::read_with_media_policy_spooled(
            &inputs,
            &spec.media_roots,
            &project,
            &names,
            &avid_adapter::original::OriginalOptions {
                link_managed_op1a: spec.mode == ClipMode::Separate,
                link_dry_gain: spec.mode != ClipMode::RewrapNative,
                root_mappings: spec
                    .path_mappings
                    .iter()
                    .map(|m| avid_adapter::original::RootMapping {
                        saved_root: m.saved_prefix.clone(),
                        current_root: m.actual_root.clone(),
                    })
                    .collect(),
                ffprobe: tools.ffprobe.clone(),
                prefer_source_media: look_up_camera_originals,
                attach_avid_proxies: spec.attach_avid_proxies,
                proxy_rewrap_dir: spec.proxy_rewrap_dir.clone(),
                ffmpeg: tools.ffmpeg.clone(),
                allow_media_gaps: linked,
                preserve_managed_audio_without_picture: spec.mode == ClipMode::Separate,
                collect_blocked_locators: for_review,
                verify_linked_imports: assisted,
            },
            spec.allow_offline_media,
            linked,
            linked && spec.entire_project,
            &mut |event| {
                use avid_adapter::SourceProgress;
                cancel.check()?;
                match event {
                    SourceProgress::ReadingBin { index, path, findings } => cb(Progress {
                        stage: "source".into(),
                        current_bin: Some(path.into()),
                        completed: index,
                        total: bins.len(),
                        message: format!("{} · Reading bin {}/{} · {}", reader_findings(findings),
                            index + 1, bins.len(), path.display()),
                    }),
                    SourceProgress::BinsRead { total, findings } => cb(Progress {
                        stage: "source".into(),
                        current_bin: None,
                        completed: total,
                        total,
                        message: format!("{} · Finished reading bins", reader_findings(findings)),
                    }),
                    SourceProgress::ResolvingMedia => {
                        progress(cb, "source media", "Checking saved media links and managed files");
                    }
                    SourceProgress::BuildingTimelines { completed, total } => cb(Progress {
                        stage: "timelines".into(),
                        current_bin: None,
                        completed,
                        total,
                        message: format!("Building timelines ({completed}/{total} complete)"),
                    }),
                    SourceProgress::ValidatingGraph => {
                        progress(cb, "graph", "Validating the source graph");
                    }
                }
                Ok(())
            },
            &mut || cancel.check(),
        )
        .map_err(|issue| source_read_blocker(issue, linked, &inputs))
    };
    let mut read = read_source(cb)?;
    if let Some(issue) = group_clip_mode_blocker(
        &read.graph, spec.mode, for_review.then_some((&read.sequence_bins, bins.as_slice())),
    ) {
        check_cancel()?;
        source_input::recheck_checked(&inputs, &mut || cancel.check()).map_err(blocker(Retry))?;
        return Err(issue);
    }
    // Direct native links need the same source-stability protection as new-media inputs.
    // Capture content revisions, then re-interpret before accepting those revisions.
    let verified_paths: HashSet<_> = read.original_inputs.iter()
        .map(|input| input.path.as_path())
        .collect();
    let mut native_paths = read
        .graph
        .media
        .iter()
        .filter(|m| m.original.is_none() && m.online == Some(true))
        .flat_map(|m| m.locations.iter().map(PathBuf::from))
        .filter(|path| !verified_paths.contains(path.as_path()))
        .collect::<Vec<_>>();
    native_paths.sort();
    native_paths.dedup();
    progress(
        cb,
        "source verification",
        "Hashing referenced native media in place; no footage is copied",
    );
    let media_error = |index: usize, message: String| {
        let mut issue = ReviewBlocker::new(AvidMedia, message);
        if for_review && !cancel.is_cancelled() {
            let kind = media_blocker::FileKind::Managed;
            media_blocker::append_file_uses(
                &mut issue, &read.graph, &read.sequence_bins, &bins, &native_paths[index], kind,
            );
            media_blocker::append_unavailable_files(
                &mut issue, &read.graph, &read.sequence_bins, &bins,
                native_paths[index + 1..].iter().map(PathBuf::as_path), kind, cancel,
            );
        }
        issue
    };
    let mut media_inputs = Vec::new();
    for (index, path) in native_paths.iter().enumerate() {
        let input = source_input::pin_file_checked(path, &mut || cancel.check())
            .map_err(|message| media_error(index, message))?;
        media_inputs.push(input);
    }
    if !media_inputs.is_empty() {
        check_cancel()?;
        let again = match read_source(cb) {
            Err(error) if cancel.is_cancelled() => return Err(error),
            result => result,
        };
        check_cancel()?;
        // Losing a managed binding can make the second source scan produce a
        // different graph or fail. Attribute file changes using the first graph,
        // which still has the saved edits and stream bindings.
        for (index, input) in media_inputs.iter().enumerate() {
            source_input::recheck_file_checked(input, &mut || cancel.check())
                .map_err(|message| media_error(index, message))?;
        }
        let again = again?;
        if again.graph != read.graph {
            return Err(ReviewBlocker::new(
                Retry,
                "Source interpretation changed while native media was being verified",
            ));
        }
        read = again;
    }
    let mappings = spec.path_mappings.iter().map(|m| avid_adapter::original::RootMapping {
        saved_root: m.saved_prefix.clone(),
        current_root: m.actual_root.clone(),
    }).collect::<Vec<_>>();
    if spec.mode == ClipMode::Separate {
        crate::wav_original::select(&mut read, &mappings, &tools.ffprobe, &mut || cancel.check())
            .map_err(blocker(OriginalMedia))?;
    }
    media_inputs.extend(read.original_inputs.iter().cloned());
    let mut matte = crate::png_mattes::prepare(&read, &mappings, spec.mode == ClipMode::Separate,
        &tools.ffprobe,
        &mut || cancel.check()).map_err(|error| {
            let mut issue = ReviewBlocker::new(OriginalMedia, error.message);
            if for_review && !cancel.is_cancelled() {
                if let Some(edit) = error.edit {
                    media_blocker::append_png_edit(
                        &mut issue, &read.graph, &read.sequence_bins, &bins, edit, cancel,
                    );
                }
            }
            issue
        })?;
    let extra_still_count = matte.extra_linked_originals();
    media_inputs.append(&mut matte.inputs);
    check_cancel()?;
    source_input::recheck_checked(&inputs, &mut || cancel.check()).map_err(blocker(Retry))?;
    let spool_stats = read.receipt_spool_stats();
    if spool_stats.is_none() {
        read.graph.encoded_len().map_err(|e| ReviewBlocker::new(SourceContent, e.to_string()))?;
    }
    let mut notes = Vec::new();
    if let Some((records, losses, bytes)) = spool_stats {
        let message = format!("Staged {records} native receipt records on disk ({bytes} bytes).");
        progress(cb, "graph", &message);
        notes.push(note("info", "source graph", message, "Whole-project graph working memory."));
        if losses > 0 {
            notes.push(note("loss", "source graph", format!("{losses} source omissions recorded on disk."),
                "The graph diagnostics file lists these occurrences and their reasons."));
        }
    }
    let proxy_bytes = avid_adapter::proxy::review(&read.proxy_bindings).map_err(blocker(ProxyMedia))?;
    if proxy_bytes > 0 {
        notes.push(note("warning","proxy preparation","Creates new media.",
            format!("Estimated upper size: {} MiB for verified Avid picture and stereo PCM.", proxy_bytes.div_ceil(1<<20))));
        for b in read.proxy_bindings.iter().filter(|b| b.audio.is_some()) {
            for path in [&spec.output,&receipt,&replay] {
                output::preflight(path,Some(&b.path)).map_err(blocker(ProxyMedia))?;
            }
        }
    }
    // A blocking master keeps priority, but Review also shows the scan and routing findings.
    let source_issue = if spec.mode == ClipMode::RewrapNative
        && (read.missing_native_picture || !read.missing_managed_audio.is_empty()) {
        let mut issue = ReviewBlocker::new(RewrapMissingMedia, "Rewrap requires all source media");
        issue.affected = unresolved_masters(&read.graph, &inputs, &matte);
        issue.affected.append(&mut read.missing_managed_audio);
        Some(issue)
    } else if read
        .graph
        .losses
        .iter()
        .any(|l| l.aspect == "source_master" && matte.keeps_loss(l))
    {
        let kind = if read.missing_native_picture || !read.package_conflict_details.is_empty()
            || (for_review && !read.scan_issues.is_empty()) {
            AvidMedia
        } else {
            SourceContent
        };
        let mut issue = ReviewBlocker::new(kind, "Selected content contains unresolved masters; exclude those bins or resolve their media before conversion");
        issue.affected = unresolved_masters(&read.graph, &inputs, &matte);
        if let Some(reason) = &read.unused_proof_issue {
            issue.affected.insert(0, reason.clone());
        }
        if look_up_camera_originals {
            let mut candidates = read.diagnostics.iter().filter(|note|
                note.contains(avid_adapter::original::SOURCE_MEDIA_UNAVAILABLE));
            if let Some(first) = candidates.next() {
                issue.message = format!("{}\n{first}", issue.message);
                issue.affected.extend(candidates.cloned());
            }
        }
        Some(issue)
    } else if !read.scan_issues.is_empty() || !read.audio_group_issues.is_empty() {
        Some(media_scan_blocker(&read.scan_issues, &read.audio_group_issues, for_review))
    } else {
        None
    };
    if let Some(mut issue) = source_issue {
        issue.affected.append(&mut read.package_conflict_details);
        if for_review {
            append_source_blocker_details(&mut issue, &mut read, &inputs, &bins, &matte);
        }
        return Err(issue);
    }
    for text in read.diagnostics.iter().filter(|text| matte.keeps_reader_note(text)) {
        notes.push(note(
            "warning",
            "source",
            text,
            "Review the affected object before using the result.",
        ));
    }
    if !linked {
        let review_bins = for_review.then_some((&read.sequence_bins, bins.as_slice()));
        if let Some(issue) = sequence_timing_blocker(&read.graph, review_bins) {
            return Err(issue);
        }
        crate::offline::check_edit_routes(&read.graph).map_err(blocker(SourceContent))?;
    }
    for l in policy.losses_source(&read.graph) {
        notes.push(note("loss", l.object, l.reason, l.aspect));
    }
    let native = spec.mode == ClipMode::RewrapNative;
    let candidates = if native { Vec::new() } else {
        premiere_preparation::managed_rewrap_candidates(&read.graph, false)
    };
    let candidate_ids: HashSet<_> = candidates.iter().map(|clip| clip.item).collect();
    let matte_only_ids = matte.managed_fill_only.clone();
    let approved_items: HashSet<_> = candidates.iter()
        .filter(|clip| !clip.pending && spec.rewrap_needed.is_some() && !matte_only_ids.contains(clip.item))
        .map(|clip| clip.item).collect();
    let omitted_items: HashSet<_> = candidate_ids.difference(&approved_items).copied().collect();
    let rewrap_approved = !approved_items.is_empty();
    let selected_rewrap_ids: HashSet<_> = approved_items.iter()
        .map(|id| id.strip_prefix("clip:").unwrap_or(id).to_string()).collect();
    if rewrap_approved && spec.mode != ClipMode::Separate {
        for clip in candidates.iter().filter(|clip| approved_items.contains(clip.item)) {
            notes.push(note("loss", clip.item, &clip.detail, "Prepared as a native AV master. The requested Grouped or Merged bin construction applies to the remaining clips."));
        }
    }
    let gain = rewrap::gain_paths(&read.graph);
    if !native && !gain.is_empty() {
        return Err(ReviewBlocker::new(Rewrap, "AudioSuite Gain requires the native prepared-media mode to copy the saved render to WAV"));
    }
    let (prepared, mut luts) = premiere_preparation::prepare_typed(
        &read.graph,
        &policy.to_options(),
        premiere_preparation::clock_profile(native),
        native,
        &mut matte,
        &inputs,
        &tools.ffprobe,
        spool_stats.is_some(),
        assisted,
        (!candidates.is_empty()).then_some((&approved_items, &omitted_items)),
        &mut || cancel.check(),
    )
    .map_err(|error| job_preparation_blocker(error, &read, &bins, for_review))?;
    media_inputs.append(&mut luts.inputs);
    for d in prepared.diagnostics {
        notes.push(note(
            "info",
            "preparation",
            d,
            "Qualified source facts are retained separately from destination behavior.",
        ));
    }
    let omitted_offline: HashSet<_> = prepared.losses.iter()
        .filter(|loss| loss.aspect == "omitted_offline_bin_item")
        .map(|loss| loss.object.as_str()).collect();
    let offline_relink = crate::offline::report(&read.graph, &spec.output, &omitted_offline).map_err(blocker(ProjectOutput))?;
    let offline_notes = crate::offline::review_notes(&read.graph, &omitted_offline);
    let offline = crate::offline::paths(&read.graph, &omitted_offline);
    for l in prepared.losses {
        notes.push(note("loss", l.object, l.reason, l.aspect));
    }
    drop(prepared.ownership);
    let mut input = prepared.input;
    if let Some((path, _)) = &offline_relink {
        output::preflight(&spec.output, Some(path)).map_err(blocker(ProjectOutput))?;
    }
    for clip in offline_notes {
        notes.push(clip);
    }
    let mut originals = luts.pictures;
    let mut seen = HashSet::new();
    let native_count = input.clips.iter()
        .filter(|clip| matches!(clip, PreparedClip::Legacy(_))).count();
    let mut original_paths = input.clips.iter()
        .filter_map(|clip| match clip {
            PreparedClip::Original(original) => Some(original.path.as_str()),
            PreparedClip::Legacy(_) => None,
        })
        .filter(|path| !offline.contains(*path) && seen.insert(*path))
        .map(Path::new);
    while let Some(path) = original_paths.next() {
        match original_stamp(path) {
            Ok(stamp) => originals.push(stamp),
            Err(message) => {
                let mut issue = ReviewBlocker::new(OriginalMedia, message);
                if for_review && !cancel.is_cancelled() {
                    let kind = media_blocker::FileKind::Original;
                    media_blocker::append_file_uses(
                        &mut issue, &read.graph, &read.sequence_bins, &bins, path, kind,
                    );
                    media_blocker::append_unavailable_files(
                        &mut issue, &read.graph, &read.sequence_bins, &bins, original_paths,
                        kind, cancel,
                    );
                }
                return Err(issue);
            }
        }
    }
    let selected: HashSet<_> = bins.iter().collect();
    inv.bins.retain(|b| selected.contains(&b.path));
    inv.bin_revisions.retain(|(p, _)| selected.contains(p));
    let paths = |original: Option<bool>, online: bool| {
        read.graph
            .media
            .iter()
            .filter(|m| {
                original.is_none_or(|b| b == m.original.is_some())
                    && (!online || m.online == Some(true))
            })
            .map(|m| m.locations.first().unwrap_or(&m.id))
            .collect::<HashSet<_>>()
            .len()
    };
    inv.native_media = paths(Some(false), false);
    inv.ume_originals = paths(Some(true), true);
    inv.total_media = paths(None, false) + extra_still_count;
    inv.found_media = paths(None, true) + extra_still_count;
    notes.push(note("info","source verification","Referenced media content is hashed in place during pre-flight and rechecked before publication.","This can take time for large sources; it prevents using stale pre-flight facts after a file changes."));
    inv.notes
        .retain(|n| n != super::inventory::PRESENCE_PENDING);
    inv.notes.push(if offline.is_empty() {
        "Pre-flight resolved referenced source media; application acceptance is a separate check.".into()
    } else {
        "The project will include offline media. Verify the proposed files with the relink report before using Link Media.".into()
    });
    if !spec.original_roots.is_empty() {
        let action = if look_up_camera_originals {
            "Prefer source media also checks exact paths from saved Avid source columns."
        } else {
            "Files are selected from saved original locators, without a filename search."
        };
        notes.push(note("info","original roots","Original roots are hints for your saved-prefix mappings; files are selected only by exact saved paths or explicit mappings.",action));
    }
    progress(
        cb,
        "preparation",
        "Preparing the destination document and any new-media plan",
    );
    check_cancel()?;
    let media = if (native && native_count > 0) || rewrap_approved {
        let mut model = input
            .build_native_media_model()
            .map_err(|e| ReviewBlocker::new(Rewrap, e.to_string()))?;
        if rewrap_approved {
            model.clips = model.clips.values()
                .filter(|clip| selected_rewrap_ids.contains(&clip.id)).cloned().collect();
            for bin in &mut model.bins { bin.clips.retain(|id| selected_rewrap_ids.contains(id)); }
        }
        let dir = spec.new_media.as_deref().or(spec.rewrap_needed.as_deref())
            .ok_or_else(|| {
                let mut issue = ReviewBlocker::new(NewMedia, "Native OP-Atom clips need a NEW media output folder. Creates new media.");
                issue.rewrap_sources = rewrap::preview_native_recipes(&model, &gain);
                issue
            })?;
        let roots = spec
            .media_roots
            .iter()
            .chain(&spec.original_roots)
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let plan = if rewrap_approved {
            rewrap::Plan::prepare_selected(&model, dir, &roots, &spec.output, &tools.ffmpeg, &tools.ffprobe)
        } else {
            let native_media = rewrap::NativeMediaPreparation {
                model: &model, source: &read.graph, writer_input: Some(&mut input), gain: &gain,
                managed_pictures: luts.managed_pictures,
            };
            native_media.preflight().map_err(blocker(Rewrap))?;
            native_media.prepare(dir, &roots, &spec.output, None, &tools.ffmpeg, &tools.ffprobe)
        }
            .map_err(|issue| match issue {
                rewrap::PrepareIssue::Destination(m) => ReviewBlocker::new(NewMedia, m),
                rewrap::PrepareIssue::Tools(m) => ReviewBlocker::new(MediaTools, m),
                rewrap::PrepareIssue::Source(m) => ReviewBlocker::new(Rewrap, m),
                rewrap::PrepareIssue::SourceFile { path, message } => {
                    let mut issue = ReviewBlocker::new(Rewrap, message);
                    if for_review && !cancel.is_cancelled() {
                        let kind = media_blocker::FileKind::Managed;
                        issue.affected.push(format!("Managed file {}", path.display()));
                        media_blocker::append_file_uses(
                            &mut issue, &read.graph, &read.sequence_bins, &bins, &path, kind,
                        );
                        media_blocker::append_unavailable_files(
                            &mut issue, &read.graph, &read.sequence_bins, &bins,
                            rewrap::referenced_input_paths(&model).filter(|other| *other != path.as_path()),
                            kind, cancel,
                        );
                    }
                    issue
                }
            })?;
        notes.push(note(
            "notice",
            "media",
            plan.review_text(),
            "Creates new media. Source files remain in place.",
        ));
        Some(plan)
    } else {
        None
    };
    if native_count == 0 {
        notes.push(note(
            "info",
            "media",
            "Original-file links do not create new media.",
            "Premiere will reference the resolved originals in place.",
        ));
    }
    let bindings = media.as_ref().map(|p| p.bindings()).unwrap_or_default();
    let dnxhr_444_rgb_sources = media.as_ref()
        .map(|plan| plan.dnxhr_444_rgb_sources()).unwrap_or_default();
    let mut ids = prproj::RandomUuids::new();
    let layout = if let Some(folder) = &spec.project_folder {
        let root = std::fs::canonicalize(folder)
            .map_err(|e| ReviewBlocker::new(ProjectSource, e.to_string()))?;
        if read.graph.bins.len() != bins.len() || input.bins.len() < bins.len()
            || input.bins.iter().zip(&read.graph.bins).any(|(a, b)| a.name != b.name)
        {
            return Err(ReviewBlocker::new(ProjectSource, "Prepared bins do not match the selected source folders"));
        }
        let mut parents = bins
            .iter()
            .map(|p| {
                let relative = p
                    .strip_prefix(&root)
                    .map_err(|_| "Selected bin lies outside the project folder")?;
                relative
                    .parent()
                    .unwrap_or(Path::new(""))
                    .components()
                    .map(|c| {
                        c.as_os_str()
                            .to_str()
                            .map(str::to_owned)
                            .ok_or("Project folder names must be UTF-8")
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ReviewBlocker::new(ProjectSource, e))?;
        // Destination-only bins (for example a stereo clip's timeline mono
        // views) have no source folder. Keep them under the import root.
        parents.resize(input.bins.len(), Vec::new());
        let seconds = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?
            .as_secs();
        Some(prproj::convert::writer::DestinationBinLayout {
            root_name: import_name(seconds).map_err(blocker(ProjectOutput))?,
            parents,
        })
    } else {
        None
    };
    // Count physical direct sources before the writer creates per-stream aliases for OP1a.
    let direct_files_before_binding = (!candidates.is_empty()).then(|| {
        input.clips.iter().flat_map(|clip| match clip {
            PreparedClip::Original(clip) if !clip.premiere_assisted => vec![clip.path.as_str()],
            PreparedClip::Legacy(clip) if !selected_rewrap_ids.contains(&clip.id) => {
                clip.video.iter().map(|v| v.path.as_str())
                    .chain(clip.audio.iter().map(|a| a.path.as_str())).collect()
            }
            _ => Vec::new(),
        }).chain(input.png_mattes.jpeg_fills.iter()
            .filter(|fill| fill.managed_start.is_some())
            .filter_map(|fill| fill.path.to_str()))
            .filter(|path| !offline.contains(path)).collect::<HashSet<_>>().len() + extra_still_count
    });
    let has_verified_op1a = crate::managed_op1a::has_verified_streams(&read.graph);
    let mut document = if has_verified_op1a && rewrap_approved {
        crate::managed_op1a::bind(&read.graph, &mut input, &offline).and_then(|(verified, mut all_bindings)| {
            all_bindings.extend(bindings);
            prproj::convert::writer::prepare_typed_project_for_premiere_27_with_rewrap_colors(
                &input, &spec.output, &mut ids, prproj::convert::writer::TypedMedia::MixedManaged {
                    bindings: &all_bindings, verified: &verified, rewrapped: &selected_rewrap_ids, offline: &offline,
                }, layout.as_ref(), &dnxhr_444_rgb_sources,
            )
        })
    } else if has_verified_op1a {
        crate::managed_op1a::prepare(
            &read.graph, &mut input, &spec.output, &mut ids, layout.as_ref(), &offline,
        )
    } else if !candidates.is_empty() {
        prproj::convert::writer::prepare_typed_project_for_premiere_27_with_rewrap_colors(
            &input, &spec.output, &mut ids, prproj::convert::writer::TypedMedia::SelectedRewrap {
                bindings: &bindings, clips: &selected_rewrap_ids, offline: &offline,
            }, layout.as_ref(), &dnxhr_444_rgb_sources,
        )
    } else {
        premiere_preparation::prepare_document(
            &input, &spec.output, &mut ids, &bindings, native, layout.as_ref(), &offline, &dnxhr_444_rgb_sources,
        )
    }
    .map_err(|e| {
        let message = format!("Premiere document preparation failed: {e}");
        let mut issue = ReviewBlocker::new(SourceContent, message);
        for note in notes.iter().filter(|note|
            note.message.contains(avid_adapter::original::SOURCE_MEDIA_UNAVAILABLE))
        {
            issue.kind = OriginalLocator;
            issue.affected.push(note.message.clone());
        }
        if !issue.affected.is_empty() {
            issue.message.push_str(". Locate the matching camera original or choose a supported primary-media mode.");
        }
        issue
    })?;
    if document.assisted_originals().len() > 64 {
        return Err(ReviewBlocker::new(OriginalMedia, "Premiere-assisted media linking supports at most 64 originals per job"));
    }
    if document.assisted_originals().len() != 0 {
        document.verify_assisted_media(None)
            .map_err(|error| blocker(OriginalMedia)(error.to_string()))?;
    }
    document
        .check_reserved_output(&receipt)
        .map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
    document
        .check_reserved_output(&replay)
        .map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
    if let Some((path, _)) = &offline_relink {
        document.check_reserved_output(path).map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
    }
    let direct_files = direct_files_before_binding.unwrap_or_else(|| {
        inv.found_media.saturating_sub(document.assisted_originals().len())
    });
    let media_linking = linked.then(|| MediaLinking {
        direct_files,
        premiere_files: document.assisted_originals().cloned().collect(),
        rewrap_available: candidates.iter().filter(|clip|
            !clip.pending && !rewrap_approved && !matte_only_ids.contains(clip.item))
            .map(|clip| clip.detail.clone()).collect(),
        rewrapped_clips: candidates.iter().filter(|clip| approved_items.contains(clip.item))
            .map(|clip| clip.detail.clone()).collect(),
        conversion_pending: candidates.iter().filter(|clip| clip.pending)
            .map(|clip| clip.detail.clone()).collect(),
    });
    let rewrap_sources = {
        let mut sources = media.as_ref().map(|plan| plan.estimate_sources()).unwrap_or_default();
        if !rewrap_approved && !candidates.is_empty() {
            let remaining = candidates.iter().filter(|clip| !matte_only_ids.contains(clip.item));
            sources.extend(premiere_preparation::selective_rewrap_preview(&read.graph, remaining));
        }
        let mut seen = HashSet::new();
        sources.extend(read.proxy_bindings.iter().filter_map(|binding| {
            let sources = binding.rewrap_sources()?;
            seen.insert(&binding.path).then_some(sources)
        }));
        sources
    };
    let graph_debug = if spool_stats.is_some_and(|(records, _, _)| records > 0) {
        let path = sidecar(&spec.output, DEBUG_SUFFIX).map_err(blocker(ProjectOutput))?;
        output::preflight(&spec.output, Some(&path)).map_err(blocker(ProjectOutput))?;
        document.check_reserved_output(&path).map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
        let file = read.take_receipt_spool().map_err(blocker(SourceContent))?
            .ok_or_else(|| ReviewBlocker::new(SourceContent, "Missing staged graph diagnostics"))?;
        Some((path, file))
    } else { None };
    if let Some(p) = &media {
        document
            .check_reserved_output(p.directory())
            .map_err(|e| ReviewBlocker::new(NewMedia, e.to_string()))?;
    }
    for d in &document.report().notes {
        notes.push(note("info", "writer", d, "Writer preparation report."));
    }
    for loss in premiere_preparation::grouped_picture_marker_losses(&read.graph, document.report()) {
        notes.push(note("loss", loss.object, loss.reason, loss.aspect));
    }
    for d in &document.report().unverified {
        notes.push(note(
            "warning",
            "native qualification",
            d,
            "Structural preparation is not application qualification.",
        ));
    }
    check_cancel()?;
    for b in read.proxy_bindings.iter().filter(|b| b.audio.is_some()) {
        document.check_reserved_output(&b.path).map_err(|e|ReviewBlocker::new(ProxyMedia,e.to_string()))?;
    }
    let mut diagnostic_locations = HashMap::new();
    let mut locations = for_review.then_some(&mut diagnostic_locations);
    let native_subjects = locate_native_review_notes(&read.graph, &mut notes, locations.as_deref_mut());
    let gap_subjects = locate_source_media_gaps(&read.graph, &mut notes, locations.as_deref_mut());
    let mut subjects = review_subjects(&read.graph, &notes, locations);
    subjects.extend(native_subjects);
    subjects.extend(gap_subjects);
    let review = Review {
        inventory: inv,
        rewrap_sources,
        estimated_bytes: media.as_ref().map(|p| p.estimated_bytes()).or((proxy_bytes > 0).then_some(proxy_bytes)),
        available_bytes: media.as_ref().map(|p| p.available_bytes()).or_else(||
            (proxy_bytes > 0).then(|| spec.proxy_rewrap_dir.as_deref().and_then(|p| fs2::available_space(p).ok())).flatten()),
        camera_originals: if look_up_camera_originals {
            camera_original_links(&read.graph)
        } else {
            Vec::new()
        },
        media_linking,
        subjects,
        timeline_groups: if for_review {
            review_timeline_groups(&read.graph, notes.iter().map(|note| note.object.as_str()), &read.sequence_bins, &bins)
        } else { Vec::new() },
        source_groups: if for_review {
            review_source_groups(&read.graph, &notes, &bins)
        } else { Vec::new() },
        diagnostic_locations,
        notes,
        cli_equivalent: cli_command(&spec).map_err(blocker(Retry))?,
    };
    Ok(PreparedJob {
        spec,
        review,
        document,
        media,
        media_created: false,
        inputs,
        media_inputs,
        originals,
        proxy_bindings: read.proxy_bindings,
        receipt,
        replay,
        offline_relink,
        graph_debug,
        aux_outputs: vec![],
    })
}
// Gregorian UTC civil date; no platform shell or locale is involved in a destination bin name.
fn import_name(seconds: u64) -> Result<String, String> {
    if seconds > 253402300799 {
        return Err("System clock is outside supported import timestamp range".into());
    }
    let z = (seconds / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    let day = seconds % 86400;
    Ok(format!(
        "AVID_IMPORT_{y:04}-{m:02}-{d:02}_{:02}:{:02}:{:02}",
        day / 3600,
        day / 60 % 60,
        day % 60
    ))
}

fn retain(job: &PreparedJob) -> Vec<PathBuf> {
    let mut paths = job.document.retained_paths().to_vec();
    if let Some(p) = &job.media {
        if job.media_created {
            paths.push(p.directory().into());
        }
    }
    paths.extend(job.aux_outputs.iter().cloned());
    paths.sort();
    paths.dedup();
    paths
}
fn recheck_job_sources(
    job: &PreparedJob,
    cancel: &CancelToken,
    cb: &mut dyn FnMut(Progress),
) -> Result<Vec<OriginalStamp>, String> {
    let bin_count = job.inputs.len();
    let total_media = job.media_inputs.len();
    let mut stamps = Vec::with_capacity(bin_count + total_media);
    for (index, input) in job.inputs.iter().chain(&job.media_inputs).enumerate() {
        cancel.check()?;
        if index >= bin_count {
            let completed = index - bin_count;
            let name = input.path.file_name().unwrap_or(input.path.as_os_str()).to_string_lossy();
            cb(Progress {
                stage: "source verification".into(),
                current_bin: None,
                completed,
                total: total_media,
                message: format!("Rechecking media {}/{}: {name}", completed + 1, total_media),
            });
        }
        stamps.push(source_input::recheck_file_and_stamp(input, &mut || cancel.check())?);
    }
    // A bin or media file can change while a later input is being hashed.
    cancel.check()?;
    recheck_originals_checked(&stamps, &mut || cancel.check())?;
    recheck_originals_checked(&job.originals, &mut || cancel.check())?;
    Ok(stamps)
}

pub(crate) fn write_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
    owned: &mut Vec<PathBuf>,
) -> Result<(), String> {
    use std::io::{BufWriter, Write};
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    owned.push(path.to_owned());
    let mut writer = BufWriter::with_capacity(64 * 1024, file);
    serde_json::to_writer_pretty(&mut writer, value)
        .map_err(|e| format!("Partial receipt retained at {}: {e}", path.display()))?;
    writer.flush()
        .map_err(|e| format!("Partial receipt retained at {}: {e}", path.display()))
}

fn report_sidecar_failure(result: &mut Completion, stage: &str, message: String) {
    if result.status == CompletionStatus::Complete {
        result.message = format!("Project published; {message}");
    } else {
        result.message.push('\n');
        result.message.push_str(&message);
    }
    result.status = CompletionStatus::Failed;
    result.report.push(note(
        "error",
        stage,
        message,
        "Outputs already created are listed under retained files.",
    ));
}

/// Cancellation is honored between stages. An active verified stream-copy finishes its safe boundary.
pub fn execute(
    mut job: PreparedJob,
    cancel: &CancelToken,
    cb: &mut dyn FnMut(Progress),
) -> Completion {
    let mut phase = "source/output recheck";
    let outcome = (|| -> Result<(), String> {
        cancel.check()?;
        check_paths(&job.spec, &job.receipt, &job.replay)?;
        if let Some((path, _)) = &job.offline_relink {
            output::preflight(&job.spec.output, Some(path))?;
        }
        if let Some((path, _)) = &job.graph_debug {
            output::preflight(&job.spec.output, Some(path))?;
        }
        job.document.recheck_outputs().map_err(|e| e.to_string())?;
        let mut source_stamps = recheck_job_sources(&job, cancel, cb)?;
        if job.document.assisted_originals().len() != 0 {
            phase = "Premiere media linking";
            progress(cb, "media", "Premiere is importing verified originals in a scratch project");
            avid::bind(&mut job.document)?;
            cancel.check()?;
            source_stamps = recheck_job_sources(&job, cancel, cb)?;
            for path in job.document.assisted_originals() {
                job.review.notes.push(note("info", "media",
                    format!("Used Premiere to link {path}"),
                    "Verified against the Avid graph and the installed helper's saved media record."));
            }
        }
        if let Some(plan) = &job.media {
            phase = "media preparation";
            progress(cb,"media","Creating and verifying new media; cancellation takes effect at the next safe stage");
            let actual=plan.execute_observed(&mut |completed,total|{cancel.check()?;cb(Progress{stage:"media".into(),current_bin:None,completed,total,message:format!("Verifying and preparing media {completed}/{total}; original files remain in place")});Ok(())},&mut job.media_created)?;
            if actual != plan.bindings() {
                return Err(
                    "Verified media bindings differ from the prepared project; media retained"
                        .into(),
                );
            }
        }
        let writes_proxy = job.proxy_bindings.iter().any(|b| b.audio.is_some());
        if writes_proxy {
            phase = "proxy media preparation";
            progress(cb,"media","Creating and verifying rewrapped proxies");
            avid_adapter::proxy::execute_observed(&job.proxy_bindings,
                &mut |path| job.aux_outputs.push(path.to_owned()))?;
        }
        cancel.check()?;
        // Media writes can take time. Link-only jobs already ran the content checks.
        if job.media.is_some() || writes_proxy {
            source_stamps = recheck_job_sources(&job, cancel, cb)?;
        }
        phase = "asset publication";
        progress(cb, "publication", "Publishing prepared assets and project");
        job.document.publish_assets().map_err(|e| e.to_string())?;
        cancel.check()?;
        phase = "source recheck before project publication";
        recheck_originals_checked(&source_stamps, &mut || cancel.check())?;
        recheck_originals_checked(&job.originals, &mut || cancel.check())?;
        cancel.check()?;
        phase = "project publication";
        let preparation_notes = job.document.report().notes.len();
        let report = job.document.publish_project().map_err(|e| e.to_string())?;
        // Review already contains the preparation notes. Publication only
        // appends diagnostics generated by saving the prepared project.
        for d in report.notes.into_iter().skip(preparation_notes) {
            job.review.notes.push(note(
                "warning",
                "publication",
                d,
                "Successful project publication may retain a staging file after cleanup failure.",
            ));
        }
        if let Some((path, text)) = &job.offline_relink {
            phase = "offline relink report";
            output::write_relink_report(path, text, &mut job.aux_outputs)?;
        }
        if let Some((path, file)) = &mut job.graph_debug {
            phase = "graph diagnostics publication";
            output::write_graph_debug(path, file, &mut job.aux_outputs, |written, total| {
                let percent = if total == 0 { 100 } else {
                    (u128::from(written) * 100 / u128::from(total)) as usize
                };
                cb(Progress {
                    stage: "diagnostics".into(),
                    current_bin: None,
                    completed: percent,
                    total: 100,
                    message: format!("Saving conversion diagnostics ({percent}%)"),
                });
            })?;
        }
        if let Some(p) = &job.media {
            phase = "media completion receipt";
            p.record_project_with_mode(
                &job.spec.output,
                p.mode("native prepared-media AV master (DNx OP1a / ProRes MOV)"),
            )?;
        }
        Ok(())
    })();
    if let Err(e) = &outcome {
        job.review.notes.push(note(
            "error",
            phase,
            e,
            "Previously created outputs are retained; no rollback deletes verified files.",
        ));
    }
    let published = job.document.is_published();
    let mut result = Completion {
        status: if outcome.is_ok() {
            CompletionStatus::Complete
        } else if cancel.is_cancelled() && !published {
            CompletionStatus::Cancelled
        } else {
            CompletionStatus::Failed
        },
        output: published.then(|| job.spec.output.clone()),
        retained_paths: retain(&job),
        receipt: None,
        report: std::mem::take(&mut job.review.notes),
        message: match outcome {
            Ok(()) => {
                "Project published. Application checks remain separate from conversion.".into()
            }
            Err(e) => {
                if published {
                    format!("Project published, but completion failed: {e}. Retained outputs remain available.")
                } else {
                    e
                }
            }
        },
    };
    if published {
        if let Err(e) = write_json(&job.replay, &job.spec, &mut job.aux_outputs) {
            report_sidecar_failure(
                &mut result,
                "replay settings",
                format!("replay settings could not be saved: {e}"),
            );
        }
        result.retained_paths = retain(&job);
    }
    {
        // The on-disk receipt reports its own intended path; a failed write never becomes success.
        result.receipt = Some(job.receipt.clone());
        if let Err(e) = write_json(&job.receipt, &result, &mut job.aux_outputs) {
            result.receipt = None;
            let message = if published {
                format!(
                    "completion receipt failed: {e}. Retained project: {}",
                    job.spec.output.display()
                )
            } else {
                format!("failure/cancellation receipt could not be written: {e}")
            };
            report_sidecar_failure(&mut result, "completion receipt", message);
        }
        result.retained_paths = retain(&job);
    }
    progress(cb, "finished", &result.message);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_locates_group_audio_by_exact_owner_track_and_clock() {
        use graph::source::{Multicam, MulticamUse, SourceBinding, SourceClock, SourceClockProvenance, SourceGraph, SourceSequence};
        use graph::{Rational, StreamKind};

        let rate = Rational::new(30_000, 1001).unwrap();
        let clock = SourceClock {
            effective_count: 1799,
            rate,
            drop: true,
            display_base: 30,
            provenance: SourceClockProvenance {
                origin_count: 1799,
                source_offset: 0,
                source_rate: rate,
                source_package: "Synthetic sequence".into(),
                timecode_track: "TC1".into(),
                route: "synthetic".into(),
            },
        };
        let mut source = SourceGraph {
            project: graph::Project { name: "Synthetic".into() },
            media: vec![], items: vec![], bins: vec![], losses: vec![], sidecars: vec![],
            sequences: vec![SourceSequence {
                disabled_placements: Vec::new(),
                id: "seq:1".into(), name: "Same name".into(), rate, frame_size: None,
                start: clock.exact_start(), clock: Some(clock),
                tracks: vec![], placements: vec![], transitions: vec![], markers: vec![], texts: vec![],
            }],
            multicams: vec![Multicam {
                id: "group:1".into(), name: "Two cameras".into(), rate: Some(rate),
                duration: None, lanes: vec![], reason: None,
            }],
            multicam_uses: vec![MulticamUse {
                id: "occurrence:1".into(), sequence: "seq:1".into(), candidates: vec![],
                kind: StreamKind::Audio, lane: Some(1), track: 2, selected: Some(1),
                record_start: Rational::new(1001, 30_000), duration: None, source_in: None,
                binding: SourceBinding { placements: vec![], reason: None }, reason: None,
            }],
        };
        let notes = [
            note("loss", "occurrence:1", "Alternate audio", "group_audio_editability"),
            note("loss", "group:1", "Group metadata", "group_metadata"),
        ];
        let labels = review_subjects(&source, &notes, None);
        assert_eq!(labels["occurrence:1"], "Same name · A2 · 00:01:00;02");
        assert_eq!(labels["group:1"], "Two cameras");
        let mut same_name = source.sequences[0].clone();
        same_name.id = "seq:2".into();
        same_name.clock = None;
        same_name.start = None;
        source.sequences.push(same_name);
        source.multicam_uses[0].sequence = "seq:2".into();
        assert_eq!(review_subjects(&source, &notes, None)["occurrence:1"], "Same name · A2 · frame 1 from sequence start");
        source.multicam_uses[0].record_start = Rational::new(1, 48_000);
        assert_eq!(review_subjects(&source, &notes, None)["occurrence:1"], "Same name · A2");

        let mut report = prproj::convert::writer::Report::default();
        for sequence in 0..2 {
            report.grouped_picture_marker_moves.push(prproj::convert::writer::GroupedPictureMarkerMove {
                sequence,
                record_ticks: 8_475_667_200,
                marker_name: "Picture range".into(),
            });
        }
        let changes = premiere_preparation::grouped_picture_marker_losses(&source, &report);
        assert_eq!(changes[0].object, "seq:1");
        assert_eq!(changes[1].object, "seq:2");
        assert!(changes.iter().all(|loss| loss.aspect == "marker_ownership"));
        assert!(changes[0].reason.starts_with("Same name · V1 · 00:01:00;02."));
        assert!(changes[1].reason.starts_with("Same name · V1 · frame 1 from sequence start."));
    }

    #[test]
    fn review_locates_transitions_and_edits_by_id_on_their_own_timeline() {
        use graph::source::{SourceBinding, SourceGraph, SourceSequence, SourceTransition};
        use graph::{GroupedAudio, Placement, Project, Rational, StreamKind};

        let record = Rational::new(1, 24).unwrap();
        let binding = SourceBinding { placements: vec![], reason: Some("synthetic".into()) };
        let mut sequence = SourceSequence {
            disabled_placements: Vec::new(),
            id: "seq:first".into(), name: "Same name".into(), rate: Rational::integer(24),
            frame_size: None, start: None, clock: None, tracks: vec![], markers: vec![], texts: vec![],
            transitions: vec![SourceTransition {
                id: "dissolve".into(), kind: StreamKind::Video, track: 5, cut: record,
                duration: record, cut_offset: Rational::new(1, 48).unwrap(),
                style: "Dissolve".into(), outgoing: binding.clone(), incoming: binding,
            }],
            placements: vec![Placement {
                id: "edit".into(), item: "item".into(), record_start: record,
                source_in: Rational::ZERO, duration: record, video_track: None,
                audio_tracks: vec![], audio_streams: vec![],
                grouped_audio: vec![GroupedAudio {
                    track: 7, streams: vec![0, 1], gain_db: None, pans: vec![],
                }],
                gain_db: 0.0, volume_db: None, volume_keys: vec![], track_gains_db: vec![],
                track_pans: vec![], speed: None, frame_map: None, transform: None, effects: vec![],
            }],
        };
        let mut other = sequence.clone();
        other.id = "seq:second".into();
        other.transitions.clear();
        other.placements[0].id = "picture".into();
        other.placements[0].record_start = Rational::integer(1);
        other.placements[0].video_track = Some(9);
        other.placements[0].grouped_audio.clear();
        let notes = [
            note("loss", "dissolve", "Approximated transition", "transition"),
            note("warning", "edit", "Unapplied pan", "pan"),
            note("loss", "picture", "Unapplied effect", "effect"),
            note("loss", "seq:second/component/0/event/2", "Omitted picture edit", "source_route"),
            note("info", "writer", "Unscoped information", ""),
            note("loss", "seq:seconder/component/0/event/2", "Different ID", "source_route"),
            note("loss", "edit", "Another finding about this edit", "effect"),
        ];
        let mut source = SourceGraph {
            project: Project { name: "Synthetic".into() },
            media: vec![], items: vec![], bins: vec![], losses: vec![], sidecars: vec![],
            sequences: vec![other], multicams: vec![], multicam_uses: vec![],
        };
        sequence.placements[0].record_start = Rational::new(1, 48_000).unwrap();
        source.sequences.push(sequence);
        let labels = review_subjects(&source, &notes, None);
        assert_eq!(labels["dissolve"], "Same name · V5 · frame 1 from sequence start");
        assert_eq!(labels["edit"], "Same name · A7");
        assert_eq!(labels["picture"], "Same name · V9 · frame 24 from sequence start");
        let sequence_bins = HashMap::from([("seq:first".into(), 0), ("seq:second".into(), 1)]);
        let bins = [PathBuf::from("Reel 1/Edit.avb"), PathBuf::from("Reel 2/Edit.avb")];
        let groups = review_timeline_groups(&source, notes.iter().map(|note| note.object.as_str()), &sequence_bins, &bins);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].id, "seq:second");
        assert_eq!(groups[0].bin.as_ref(), Some(&bins[1]));
        assert_eq!(groups[0].note_indices, [2, 3]);
        assert_eq!(groups[1].id, "seq:first");
        assert_eq!(groups[1].bin.as_ref(), Some(&bins[0]));
        assert_eq!(groups[1].note_indices, [0, 1, 6]);

        source.losses = [2, 3, 5, 2, 1].into_iter().map(|i| graph::Loss {
            object: notes[i].object.clone(),
            aspect: avid_adapter::UNQUALIFIED_ROUTE_TIMING.into(),
            reason: notes[i].message.clone(),
        }).collect();
        let blocked = sequence_timing_blocker(&source, Some((&sequence_bins, &bins))).unwrap();
        let direct = sequence_timing_blocker(&source, None).unwrap();
        assert_eq!(blocked.affected, direct.affected);
        assert_eq!(blocked.affected.len(), 4);
        assert_eq!(blocked.timeline_groups[0].id, "seq:second");
        assert_eq!(blocked.timeline_groups[0].note_indices, [0, 1]);
        assert_eq!(blocked.timeline_groups[1].id, "seq:first");
        assert_eq!(blocked.timeline_groups[1].note_indices, [3]);
        assert_eq!(blocked.cli_summary(), direct.cli_summary());

        use graph::source::{SourceItem, SourceItemMedia};
        let item = |id: &str| SourceItem {
            id: id.into(), name: "Same clip".into(), umid: None, tape: None, rate: None,
            color_rgb16: None, stream_groups: vec![], markers: vec![],
            media: vec![SourceItemMedia {
                media: "same-file".into(), gain_db: 0.0, source_offset: Some(Rational::ZERO),
            }],
        };
        source.items = vec![item("item"), item("another-item")];
        source.sequences[0].placements[0].item = "another-item".into();
        source.bins = ["item", "another-item"].into_iter().enumerate()
            .map(|(index, id)| graph::Bin {
                id: format!("bin:{index}"), name: "Edit".into(), parent: None, items: vec![id.into()],
            }).collect();
        let notes = notes.into_iter().chain([note("warning", "item", "Source finding", "source")])
            .collect::<Vec<_>>();
        let clips = review_source_groups(&source, &notes, &bins);
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].id, "item");
        assert_eq!(clips[0].bin.as_ref(), Some(&bins[0]));
        assert_eq!(clips[0].note_indices, [1, 6, 7]);
        assert_eq!(clips[1].id, "another-item");
        assert_eq!(clips[1].bin.as_ref(), Some(&bins[1]));
        assert_eq!(clips[1].note_indices, [2]);
        source.bins[1].items.push("item".into());
        assert!(review_source_groups(&source, &notes, &bins)[0].bin.is_none());
    }

    #[test]
    fn editable_groups_incompatible_clip_mode_has_a_specific_review_blocker() {
        use graph::source::{Multicam, MulticamUse, SourceBinding, SourceGraph, SourceSequence};
        use graph::{Rational, StreamKind};

        let mut source = SourceGraph {
            project: graph::Project { name: "Synthetic Group project".into() },
            media: vec![],
            items: vec![],
            bins: vec![],
            sequences: vec![],
            losses: vec![],
            sidecars: vec![],
            multicams: vec![],
            multicam_uses: vec![],
        };
        assert!(group_clip_mode_blocker(&source, ClipMode::Merged, None).is_none());
        source.multicams = (1..=9).map(|index| Multicam {
            id: format!("group:{index}"),
            name: format!("Group {index}"),
            rate: None,
            duration: None,
            lanes: vec![],
            reason: None,
        }).collect();
        for mode in [ClipMode::Grouped, ClipMode::Merged] {
            let issue = group_clip_mode_blocker(&source, mode, None).unwrap();
            assert_eq!(issue.kind, BlockerKind::GroupClipMode);
            assert_eq!(issue.cli_summary(), "Linked editable Groups require Separate files");
        }
        let issue = group_clip_mode_blocker(&source, ClipMode::Grouped, None).unwrap();
        assert_eq!(issue.affected.len(), 9);
        assert_eq!(issue.affected[0], "Group 1");
        assert_eq!(issue.affected[7], "Group 8");
        assert_eq!(issue.affected[8], "Group 9");
        assert!(group_clip_mode_blocker(&source, ClipMode::Separate, None).is_none());
        assert!(group_clip_mode_blocker(&source, ClipMode::RewrapNative, None).is_none());

        source.sequences = ["first", "second", "unused"].into_iter().map(|id| SourceSequence {
            id: id.into(),
            name: "Same name".into(),
            rate: Rational::integer(24),
            frame_size: None,
            start: None,
            clock: None,
            tracks: vec![],
            placements: vec![],
            disabled_placements: vec![],
            transitions: vec![],
            markers: vec![],
            texts: vec![],
        }).collect();
        source.multicam_uses = vec![MulticamUse {
            id: "picture-use".into(),
            sequence: "second".into(),
            candidates: vec!["group:2".into()],
            kind: StreamKind::Video,
            track: 2,
            lane: Some(1),
            selected: Some(1),
            record_start: Some(Rational::integer(1)),
            duration: None,
            source_in: None,
            binding: SourceBinding { placements: vec![], reason: Some("synthetic".into()) },
            reason: Some("synthetic".into()),
        }];
        let mut audio = source.multicam_uses[0].clone();
        audio.id = "audio-use".into();
        audio.sequence = "first".into();
        audio.candidates.push("group:1".into());
        audio.kind = StreamKind::Audio;
        audio.track = 3;
        audio.record_start = None;
        source.multicam_uses.push(audio);

        let bin_paths = [PathBuf::from("Reel A/Edit.avb"), PathBuf::from("Reel B/Edit.avb")];
        let sequence_bins = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let issue = group_clip_mode_blocker(
            &source, ClipMode::Grouped, Some((&sequence_bins, &bin_paths)),
        ).unwrap();
        assert_eq!(issue.affected[9], "Same name · V2 · frame 24 from sequence start · Group 2 · edit picture-use");
        assert_eq!(issue.affected[10], "Same name · A3 · Unresolved Group · edit audio-use");
        assert_eq!(issue.timeline_groups.len(), 2);
        assert_eq!(issue.timeline_groups[0].id, "first");
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&bin_paths[0]));
        assert_eq!(issue.timeline_groups[0].note_indices, [10]);
        assert_eq!(issue.timeline_groups[1].id, "second");
        assert_eq!(issue.timeline_groups[1].bin.as_ref(), Some(&bin_paths[1]));
        assert_eq!(issue.timeline_groups[1].note_indices, [9]);
        assert_eq!(issue.cli_summary(), "Linked editable Groups require Separate files");

        use premiere_adapter::{PreparationIssue, PreparationOccurrence};
        source.bins.push(graph::Bin {
            id: "bin:1".into(), name: "Edit".into(), parent: None, items: vec!["group:2".into()],
        });
        let groups = Some((&sequence_bins, bin_paths.as_slice()));
        let failure = PreparationIssue {
            message: "Group audio layout refused".into(),
            sequence: None,
            occurrence: Some(PreparationOccurrence::Group("group:2".into())),
        };
        let issue = premiere_preparation_blocker(failure, &source, groups);
        assert_eq!(issue.affected, [
            format!("\"Group 2\" · saved Group group:2 · Avid bin {}", bin_paths[1].display()),
            "Same name · V2 · frame 24 from sequence start · Group edit picture-use".into(),
        ]);
        assert_eq!(issue.timeline_groups.len(), 1);
        assert_eq!(issue.timeline_groups[0].id, "second");
        assert_eq!(issue.timeline_groups[0].note_indices, [1]);

        let failure = PreparationIssue {
            message: "Audio Group use refused".into(),
            sequence: Some("first".into()),
            occurrence: Some(PreparationOccurrence::GroupUse("audio-use".into())),
        };
        let issue = premiere_preparation_blocker(failure, &source, groups);
        assert_eq!(issue.affected, ["Same name · A3 · Group edit audio-use"]);
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&bin_paths[0]));
        assert_eq!(issue.timeline_groups[0].note_indices, [0]);
    }

    #[test]
    fn rewrap_locator_blocker_keeps_diagnostics_with_their_own_bins() {
        use avid_adapter::{OriginalLocatorIssue, OriginalLocatorKind, SourceReadIssue};

        let inputs = [
            SourceInput { path: "First.avb".into(), revision: String::new() },
            SourceInput { path: "Second.avb".into(), revision: String::new() },
        ];
        for (kind, linked, expected, cli_names_bin) in [
            (OriginalLocatorKind::MissingOriginal, false, BlockerKind::RewrapMissingOriginal, false),
            (OriginalLocatorKind::MissingOriginal, true, BlockerKind::OriginalLocator, true),
            (OriginalLocatorKind::SavedPath, false, BlockerKind::OriginalLocator, true),
            (OriginalLocatorKind::LegacyMxfPackage, false, BlockerKind::LegacyMxfPackage, true),
            (OriginalLocatorKind::LegacyMxfPackage, true, BlockerKind::LegacyMxfPackage, true),
        ] {
            let issue = source_read_blocker(
                SourceReadIssue::OriginalLocator(OriginalLocatorIssue {
                    message: "Cannot resolve the first original".into(),
                    bin: 0,
                    kind,
                    additional: vec![(1, "Second locator unavailable".into())],
                    details_truncated: true,
                    edits: Default::default(),
                }),
                linked,
                &inputs,
            );
            assert_eq!(issue.kind, expected);
            assert_eq!(
                issue.affected,
                ["Avid bin First.avb", "Second locator unavailable · Second.avb"]
            );
            assert!(issue.message.contains("diagnostic size limit"));
            assert_eq!(issue.cli_summary().contains("Avid bin First.avb"), cli_names_bin);
        }
    }

    #[test]
    fn premiere_preparation_failure_reports_the_exact_same_named_timeline() {
        use graph::{
            source::{SourceBinding, SourceGraph, SourceSequence, SourceTransition},
            Project, Rational, Sidecar, StreamKind,
        };
        use premiere_adapter::{ClockProfile, PreparationOccurrence};

        let sequence = |id: &str| SourceSequence {
            id: id.into(),
            name: "Same name".into(),
            rate: Rational::integer(24),
            frame_size: Some((1920, 1080)),
            start: None,
            clock: None,
            tracks: vec![],
            placements: vec![],
            disabled_placements: vec![],
            transitions: vec![],
            markers: vec![],
            texts: vec![],
        };
        let mut graph = SourceGraph {
            project: Project { name: "Synthetic destination check".into() },
            media: vec![], items: vec![], bins: vec![], multicams: vec![], multicam_uses: vec![],
            sequences: vec![sequence("first"), sequence("second")],
            losses: vec![],
            sidecars: vec![
                Sidecar {
                    adapter: "avid.source".into(),
                    object: "first".into(),
                    data: graph::json::parse(r#"{"compatibility_start":"3600"}"#).unwrap(),
                },
                Sidecar {
                    adapter: "avid.source".into(),
                    object: "second".into(),
                    data: graph::json::parse(r#"{"compatibility_start":"3600","native_transitions":[{"id":"second/transition/1"}]}"#).unwrap(),
                },
            ],
        };
        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let options = prproj::convert::model::Options::default();
        let prepare = |graph: &SourceGraph| premiere_preparation::prepare_typed(
            graph, &options, ClockProfile::LegacyImport, false,
            &mut crate::png_mattes::Plan::default(), &[], Path::new("unused-probe"),
            false, false, None, &mut || Ok(()),
        );
        let refusal = prepare(&graph).err().expect("unresolved transition refuses preparation");
        assert_eq!(refusal.sequence.as_deref(), Some("second"));
        assert_eq!(refusal.occurrence, Some(PreparationOccurrence::Transition("second/transition/1".into())));
        assert!(refusal.message.contains("unresolved_transition_topology second/transition/1"));
        let original = premiere_adapter::prepare_linked_originals(
            &graph, &options, ClockProfile::LegacyImport, false,
        ).err().unwrap();
        assert_eq!(refusal.message, format!("Premiere preparation failed: {original}"));
        let issue = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        assert_eq!(issue.kind, BlockerKind::SourceContent);
        assert_eq!(issue.affected, ["Same name · saved sequence second"]);
        let [group] = issue.timeline_groups.as_slice() else { panic!("expected one timeline") };
        assert_eq!(group.id, "second");
        assert_eq!(group.bin.as_ref(), Some(&bins[1]));
        assert_eq!(group.note_indices, [0]);

        graph.sidecars[1].data = graph::json::parse(r#"{
          "compatibility_start":"3600",
          "native_transitions":[{"id":"second/transition/1","kind":1,"track":2,"rate":"24",
                                  "position":{"value":4},"cutpoint":{"value":2}}]
        }"#).unwrap();
        let refusal = prepare(&graph).err().expect("native transition lacks a portable span");
        let saved_start = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        assert_eq!(saved_start.affected, [
            "Same name · V2 · frame 4 from sequence start · transition second/transition/1 (saved start)",
        ]);
        assert_eq!(saved_start.timeline_groups, [group.clone()]);

        graph.sidecars[1].data = graph::json::parse(r#"{
          "compatibility_start":"3600",
          "native_transitions":[{"id":"second/transition/1","kind":1,"track":2,"rate":null,
                                  "position":{"value":4},"cutpoint":{"value":2}}]
        }"#).unwrap();
        let refusal = prepare(&graph).err().expect("native transition clock is unknown");
        let unknown_clock = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        assert_eq!(unknown_clock.affected, ["Same name · V2 · transition second/transition/1"]);
        assert_eq!(unknown_clock.timeline_groups, [group.clone()]);

        let endpoint = SourceBinding { placements: vec![], reason: Some("ambiguous owner".into()) };
        graph.sequences[1].transitions.push(SourceTransition {
            id: "second/transition/1".into(), kind: StreamKind::Video, track: 2,
            cut: Rational::new(1, 4).unwrap(),
            cut_offset: Rational::new(1, 24).unwrap(),
            duration: Rational::new(1, 12).unwrap(), style: "Dissolve".into(),
            outgoing: endpoint.clone(), incoming: endpoint,
        });
        let refusal = prepare(&graph).err().expect("ambiguous endpoints refuse preparation");
        assert_eq!(refusal.sequence.as_deref(), Some("second"));
        assert_eq!(refusal.occurrence, Some(PreparationOccurrence::Transition("second/transition/1".into())));
        let original = premiere_adapter::prepare_linked_originals(
            &graph, &options, ClockProfile::LegacyImport, false,
        ).err().unwrap();
        assert_eq!(refusal.message, format!("Premiere preparation failed: {original}"));
        let issue = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        assert_eq!(issue.affected, ["Same name · V2 · frame 6 from sequence start · transition second/transition/1"]);
        assert_eq!(issue.timeline_groups, [group.clone()]);

        graph.sequences[1].transitions[0].style = "avid.synthetic".into();
        graph.sidecars[1].data = graph::json::parse(
            r#"{"native_transitions":[{"id":"second/transition/1"}]}"#,
        ).unwrap();
        let later = prepare(&graph).err().expect("missing start refuses after transition handling");
        assert!(later.message.contains("second: missing compatibility start provenance"));
        assert_eq!(later.sequence.as_deref(), Some("second"));
        assert!(later.occurrence.is_none());

        graph.sequences[1].id = graph.sequences[0].id.clone();
        let global = prepare(&graph).err().expect("duplicate sequence identity refuses");
        assert!(global.sequence.is_none());
        assert!(global.occurrence.is_none());
        let issue = premiere_preparation_blocker(global, &graph, Some((&owners, &bins)));
        assert!(issue.timeline_groups.is_empty() && issue.affected.is_empty());
    }

    fn premiere_edit_review_graph() -> graph::source::SourceGraph {
        graph::source::SourceGraph::from_json_text(r#"{
          "schema":"anynle.graph/2","project":{"name":"Synthetic edit review"},
          "media":[
            {"id":"pcm1","kind":"audio","locations":["/Test-A1.mxf"],"sample_rate":48000,
             "bits":24,"stream":1,"duration":"4","start":"0"},
            {"id":"pcm2","kind":"audio","locations":["/Test-A2.mxf"],"sample_rate":48000,
             "bits":24,"stream":2,"duration":"4","start":"0"}],
          "items":[{"id":"clip:master","name":"Master","rate":"24","stream_groups":[[1,2]],
                    "media":[{"media":"pcm1","source_offset":"0"},{"media":"pcm2","source_offset":"0"}]}],
          "bins":[{"id":"bin:0","name":"Bin","items":["clip:master"]}],
          "sequences":[
            {"id":"first","name":"Same name","rate":"24","frame_size":[1920,1080],"placements":[
              {"id":"first/edit","item":"clip:master","record_start":"1","source_in":"0","duration":"2",
               "grouped_audio":[{"track":7,"streams":[1,2],"pans":[null,null]}]}]},
            {"id":"second","name":"Same name","rate":"24","frame_size":[1920,1080],"placements":[
              {"id":"second/edit","item":"clip:master","record_start":"1/48000","source_in":"0","duration":"2",
               "grouped_audio":[{"track":7,"streams":[1,2],"pans":[null,null]}]}]}],
          "sidecars":[
            {"adapter":"avid.source","object":"pcm1","data":{"legacy_clock":null}},
            {"adapter":"avid.source","object":"pcm2","data":{"legacy_clock":null}},
            {"adapter":"avid.source","object":"first","data":{"compatibility_start":"3600"}},
            {"adapter":"avid.source","object":"second","data":{"compatibility_start":"3600"}}]
        }"#).unwrap()
    }

    #[test]
    fn review_uses_exact_saved_clock_for_native_picture_effects() {
        let mut graph = premiere_edit_review_graph();
        let sequence = &mut graph.sequences[0];
        sequence.rate = graph::Rational::parse("24000/1001").unwrap();
        graph.sidecars[2].data = graph::json::parse(r#"{"unresolved":[
            {"owner":"first/matte","kind":1,"track":2,
             "record_start":{"value":168},"record_rate":"24000/1001"},
            {"owner":"first/other_clock","kind":1,"track":2,
             "record_start":{"value":168},"record_rate":"24"},
            {"owner":"first/duplicate","kind":1,"track":2},
            {"owner":"first/duplicate","kind":1,"track":3},
            {"owner":"second/matte","kind":1,"track":3,
             "record_start":{"value":168},"record_rate":"24"}
        ]}"#).unwrap();
        graph.validate().unwrap();
        let notes = ["first/matte", "first/other_clock", "first/duplicate", "second/matte"]
            .map(|owner| note("loss", owner, "Saved overlay omitted", "picture_effect"));
        let mut locations = HashMap::new();
        let subjects = review_subjects(&graph, &notes, Some(&mut locations));
        assert_eq!(subjects["first/matte"], "Same name · V2 · frame 168 from sequence start · picture effect");
        assert_eq!(subjects["first/other_clock"], "Same name · V2 · picture effect");
        assert!(locations[&0].record_offset.is_some());
        assert!(locations[&1].record_offset.is_none());
        assert_eq!(subjects["first/duplicate"], "Same name");
        assert_eq!(subjects["second/matte"], "Same name");

        let mut issue = ReviewBlocker::new(BlockerKind::OriginalMedia, "PNG decode failed");
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        media_blocker::append_png_edit(&mut issue, &graph, &owners, &bins, crate::png_mattes::FailedPngEdit {
            sequence: "first".into(), id: "first/matte".into(), track: 2,
            record_start: graph::Rational::new(168 * 1001, 24_000), saved_locators: vec![],
            shared_overlay_source: false,
        }, &CancelToken::default());
        assert_eq!(issue.affected, ["Same name · V2 · frame 168 from sequence start · PNG source preparation failed · edit first/matte"]);
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&bins[0]));
    }

    #[test]
    fn review_locates_disabled_gap_notes_using_saved_owner_and_clock() {
        let mut graph = premiere_edit_review_graph();
        graph.sidecars[2].data = graph::json::parse(r#"{"unresolved":[
            {"owner":"first/native/1","disabled_clip":true,"kind":1,"track":4,
             "record_start":{"value":48},"record_rate":"24"},
            {"owner":"first/native/2","disabled_clip":true,"kind":2,"track":2,
             "record_start":{"value":48},"record_rate":"25"},
            {"owner":"first/native/3","disabled_clip":true,"kind":1,"track":4},
            {"owner":"first/native/3","disabled_clip":true,"kind":1,"track":5}
        ]}"#).unwrap();
        let mut edit = graph.sequences[1].placements[0].clone();
        edit.id = "second/portable".into();
        graph.sequences[1].disabled_placements.push(edit);
        graph.validate().unwrap();
        let mut notes = [
            note("loss", "first", "first/native/1: Disabled source edit", "disabled_clip"),
            note("loss", "first", "first/native/2: Disabled source edit", "disabled_clip"),
            note("loss", "first", "first/native/3: Disabled source edit", "disabled_clip"),
            note("loss", "second", "first/native/1: Different saved owner", "disabled_clip"),
            note("loss", "second/portable", "Disabled source edit kept as a gap", "disabled_clip"),
        ];
        let subjects = locate_native_review_notes(&graph, &mut notes, None);
        assert_eq!(subjects["first/native/1"], "Same name · V4 · frame 48 from sequence start · disabled edit");
        assert_eq!(subjects["first/native/2"], "Same name · A2 · disabled edit");
        assert_eq!(notes.each_ref().map(|n| n.object.as_str()), [
            "first/native/1", "first/native/2", "first", "second", "second/portable",
        ]);
        assert_eq!(review_subjects(&graph, &notes, None)["second/portable"],
            "Same name · A7 · 1/48000 seconds from sequence start · disabled edit");
        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let groups = review_timeline_groups(&graph, notes.iter().map(|note| note.object.as_str()), &owners, &bins);
        assert_eq!(groups[0].bin.as_ref(), Some(&bins[0]));
        assert_eq!(groups[0].note_indices, [0, 1, 2]);
        assert_eq!(groups[1].bin.as_ref(), Some(&bins[1]));
        assert_eq!(groups[1].note_indices, [3, 4]);
    }

    #[test]
    fn review_locates_only_uniquely_identified_omitted_source_events() {
        let mut graph = premiere_edit_review_graph();
        graph.sidecars[2].data = graph::json::parse(r#"{"native_events":[
            {"id":"first/route/1","placement_id":null,"kind":1,"track":4,
             "range":{"record_start":48,"record_rate":"24"}},
            {"id":"first/route/2","placement_id":null,"kind":2,"track":2,
             "range":{"record_start":48,"record_rate":"25"}},
            {"id":"first/route/3","placement_id":null,"kind":1,"track":4},
            {"id":"first/route/3","placement_id":"first/edit","kind":1,"track":5},
            {"id":"first/route/4","placement_id":"first/edit","kind":2,"track":7},
            {"id":"first/route/5","kind":2,"track":7},
            {"id":"second/route/1","placement_id":null,"kind":1,"track":8}
        ],"unresolved":[
            {"owner":"first/route/6","reason":"first/route/6: ambiguous effect inputs","kind":1,"track":5,
             "record_start":{"value":72},"record_rate":"24"}
        ]}"#).unwrap();
        graph.validate().unwrap();
        let mut notes = [
            note("loss", "first", "first/route/1: master clip was not converted", "source_route"),
            note("loss", "first", "first/route/2: master clip was not converted", "source_route"),
            note("loss", "first", "first/route/3: master clip was not converted", "source_route"),
            note("loss", "first", "first/route/4: source uses a fallback", "source_route"),
            note("loss", "first", "first/route/5: no placement evidence", "source_route"),
            note("loss", "first", "second/route/1: different owner", "source_route"),
            note("loss", "first", "first/route/6: ambiguous effect inputs", "source_route"),
            note("loss", "first", "first/route/6: other diagnostic", "source_route"),
            note("loss", "second", "first/route/1: master clip was not converted", "source_route"),
        ];
        let messages = notes.each_ref().map(|note| note.message.clone());
        let subjects = locate_native_review_notes(&graph, &mut notes, None);
        assert_eq!(subjects.len(), 3);
        assert_eq!(subjects["first/route/1"], "Same name · V4 · frame 48 from sequence start · omitted edit");
        assert_eq!(subjects["first/route/2"], "Same name · A2 · omitted edit");
        assert_eq!(subjects["first/route/6"], "Same name · V5 · frame 72 from sequence start · source route");
        assert_eq!(notes.each_ref().map(|n| n.object.as_str()), [
            "first/route/1", "first/route/2", "first", "first", "first", "first", "first/route/6", "first", "second",
        ]);
        assert_eq!(notes.each_ref().map(|note| note.message.clone()), messages);
        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let groups = review_timeline_groups(
            &graph, notes.iter().map(|note| note.object.as_str()), &owners, &bins,
        );
        assert_eq!(groups[0].bin.as_ref(), Some(&bins[0]));
        assert_eq!(groups[0].note_indices, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(groups[1].bin.as_ref(), Some(&bins[1]));
        assert_eq!(groups[1].note_indices, [8]);
    }

    #[test]
    fn review_source_media_gap_uses_the_marked_span_and_exact_owner() {
        use graph::{source::{SourceBinding, SourceMarker}, MarkerOwner, Rational};

        let mut source = premiere_edit_review_graph();
        source.sidecars[2].data = graph::json::parse(r#"{"native_events":[
          {"id":"first/gap","placement_id":null,"kind":2,"track":3,
           "range":{"record_start":44,"record_rate":"24"}},
          {"id":"first/duplicate","placement_id":null,"kind":2,"track":3},
          {"id":"first/duplicate","placement_id":null,"kind":2,"track":4},
          {"id":"first/kept","placement_id":"first/edit","kind":2,"track":3}
        ]}"#).unwrap();
        let start = Rational::new(2, 1);
        let marker = |id: &str, comment: &str| SourceMarker {
            name: "Media omitted · A3".into(),
            comment: comment.into(),
            user: String::new(),
            color: None,
            start,
            duration: Rational::new(1, 2).unwrap(),
            track: None,
            owner: MarkerOwner::Sequence,
            owner_occurrence: id.into(),
            owner_local: start,
            owner_rate: Some(source.sequences[0].rate),
            binding: SourceBinding { placements: Vec::new(), reason: None },
            source: None,
            inherited: false,
        };
        source.sequences[0].markers = vec![
            marker("first/gap", "One gap spans the safe record interval."),
            marker("first/duplicate", "Duplicate occurrence."),
            marker("first/kept", "Source has a graph placement."),
        ];
        source.validate().unwrap();
        let mut notes = [
            note("loss", "first", "One gap spans the safe record interval.", "media_gap"),
            note("loss", "second", "One gap spans the safe record interval.", "media_gap"),
            note("loss", "first", "Duplicate occurrence.", "media_gap"),
            note("loss", "first", "Source has a graph placement.", "media_gap"),
            note("loss", "first/edit", "Destination-only gap.", "media_gap"),
        ];
        let subjects = locate_source_media_gaps(&source, &mut notes, None);
        assert_eq!(notes.each_ref().map(|row| row.object.as_str()), [
            "first/gap", "second", "first", "first", "first/edit",
        ]);
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects["first/gap"], "Same name · A3 · frame 48 from sequence start · media gap");
    }

    #[test]
    fn review_locates_caption_and_title_findings_on_saved_source_tracks() {
        use graph::{source::{SourceText, TextKind}, Rational};

        let mut source = premiere_edit_review_graph();
        let caption = SourceText {
            id: "caption:1".into(),
            kind: TextKind::Caption,
            track: 3,
            content: Some("Same text".into()),
            record_start: Rational::new(1, 48_000),
            duration: Rational::new(1, 24),
            reason: None,
        };
        let mut title = caption.clone();
        title.id = "title:1".into();
        title.kind = TextKind::Graphic;
        title.track = 5;
        title.record_start = None;
        title.reason = Some("Unresolved title clock".into());
        source.sequences[0].texts.push(caption);
        source.sequences[1].texts.push(title);
        source.validate().unwrap();
        let notes = [
            note("loss", "title:1", "Missing saved time", "text"),
            note("loss", "caption:1", "Appearance not carried", "text_appearance"),
        ];
        let subjects = review_subjects(&source, &notes, None);
        assert_eq!(subjects["caption:1"], "Same name · V3 · 1/48000 seconds from sequence start · caption");
        assert_eq!(subjects["title:1"], "Same name · V5 · title");

        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let groups = review_timeline_groups(&source, notes.iter().map(|note| note.object.as_str()), &owners, &bins);
        assert_eq!(groups[0].bin.as_ref(), Some(&bins[0]));
        assert_eq!(groups[0].note_indices, [1]);
        assert_eq!(groups[1].bin.as_ref(), Some(&bins[1]));
        assert_eq!(groups[1].note_indices, [0]);
    }

    #[test]
    fn premiere_preparation_failure_reports_exact_saved_edit_and_subframe_position() {
        use graph::{source::SourceGraph, Rational};
        use premiere_adapter::{ClockProfile, PreparationOccurrence};

        let mut graph = premiere_edit_review_graph();
        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let options = prproj::convert::model::Options::default();
        let clock = ClockProfile::LegacyImport;
        let prepare = |graph: &SourceGraph| premiere_adapter::prepare_avid_job(
            graph, &options, clock, false, false, None,
        );

        let refusal = prepare(&graph).err().expect("off-grid record position refuses");
        assert_eq!(refusal.sequence.as_deref(), Some("second"));
        assert_eq!(refusal.occurrence, Some(PreparationOccurrence::Placement("second/edit".into())));
        let original = premiere_adapter::prepare_linked_originals(
            &graph, &options, clock, false,
        ).err().unwrap();
        assert_eq!(refusal.message, original.to_string());
        assert!(refusal.message.contains("destination_record_grid second/edit"));
        let issue = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        assert_eq!(issue.affected, ["Same name · A7 · 1/48000 seconds from sequence start · edit second/edit"]);
        let [group] = issue.timeline_groups.as_slice() else { panic!("expected one timeline") };
        assert_eq!(group.id, "second");
        assert_eq!(group.bin.as_ref(), Some(&bins[1]));
        assert_eq!(group.note_indices, [0]);

        graph.sequences[1].placements[0].record_start = Rational::integer(1);
        if let Err(error) = prepare(&graph) {
            panic!("the frame-aligned control must prepare: {}", error.message);
        }
        graph.sidecars.pop();
        let later = prepare(&graph).err().expect("missing start refuses after valid edit");
        assert!(later.message.contains("second: missing compatibility start provenance"));
        assert_eq!(later.sequence.as_deref(), Some("second"));
        assert!(later.occurrence.is_none());
    }

    #[test]
    fn premiere_prepared_validation_preserves_exact_saved_scope() {
        use graph::Rational;
        use premiere_adapter::{ClockProfile, PreparationOccurrence};
        use prproj::convert::prepared::PreparedValidationLocation;

        let mut graph = premiere_edit_review_graph();
        graph.sequences[1].placements[0].record_start = Rational::integer(1);
        let options = prproj::convert::model::Options::default();
        let prepare = |graph: &graph::source::SourceGraph| premiere_adapter::prepare_avid_job(
            graph, &options, ClockProfile::LegacyImport, false, false, None,
        );
        let mut input = prepare(&graph).unwrap_or_else(|issue| panic!("{}", issue.message)).input;
        input.sequences[1].events[0].source_in = 4 * prproj::build::time::TICKS_PER_SECOND;
        let unscoped = input.validate().unwrap_err();
        assert_eq!(unscoped.to_string(), "prepared event range/grid mismatch");
        for whole_project in [false, true] {
            let issue = input.validate_with_location(whole_project).unwrap_err();
            assert_eq!(issue.error, unscoped);
            assert_eq!(issue.location, Some(PreparedValidationLocation { sequence: 1, event: Some(0) }));
        }

        graph.sequences[1].placements[0].grouped_audio[0].track = 100;
        let failure = prepare(&graph).err().expect("destination track limit must refuse");
        let original = premiere_adapter::prepare_linked_originals(
            &graph, &options, ClockProfile::LegacyImport, false,
        ).err().unwrap();
        assert_eq!(failure.message, original.to_string());
        assert_eq!(failure.message, "prepared sequences[1].events[0].stereo[0].audio_track: expected 1..99");
        assert_eq!(failure.sequence.as_deref(), Some("second"));
        assert_eq!(failure.occurrence, Some(PreparationOccurrence::Placement("second/edit".into())));
        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let issue = premiere_preparation_blocker(failure, &graph, Some((&owners, &bins)));
        assert_eq!(issue.affected, ["Same name · A100 · frame 24 from sequence start · edit second/edit"]);
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&bins[1]));

        // Matching spans unite these independent lanes into one writer event.
        let mut second_lane = graph.sequences[1].placements[0].clone();
        second_lane.id = "second/other-lane".into();
        second_lane.grouped_audio[0].track = 8;
        graph.sequences[1].placements.push(second_lane);
        let failure = prepare(&graph).err().expect("same invalid lane in merged event");
        assert_eq!(failure.message, issue.message);
        assert_eq!(failure.sequence.as_deref(), Some("second"));
        assert_eq!(failure.occurrence, Some(PreparationOccurrence::CombinedPlacements {
            ids: vec!["second/edit".into(), "second/other-lane".into()],
            truncated: false,
        }));
        let issue = premiere_preparation_blocker(failure, &graph, Some((&owners, &bins)));
        assert_eq!(issue.affected, [
            "Same name · A100 · frame 24 from sequence start · combined edit member second/edit",
            "Same name · A8 · frame 24 from sequence start · combined edit member second/other-lane",
        ]);
        assert_eq!(issue.timeline_groups[0].bin.as_ref(), Some(&bins[1]));
        assert_eq!(issue.timeline_groups[0].note_indices, [0, 1]);
    }

    #[test]
    fn premiere_preparation_clip_failure_reports_direct_uses_by_id() {
        use premiere_adapter::{ClockProfile, PreparationOccurrence};

        let mut graph = premiere_edit_review_graph();
        let mut other = graph.items[0].clone();
        other.id = "clip:distinct".into();
        graph.items.push(other);
        let mut other_bin = graph.bins[0].clone();
        other_bin.id = "bin:1".into();
        other_bin.items = vec!["clip:distinct".into()];
        graph.bins.push(other_bin);
        let second = &mut graph.sequences[1];
        let disabled = second.placements[0].clone();
        second.disabled_placements.push(disabled);
        second.placements[0].id = "second/other".into();
        second.placements[0].item = "clip:distinct".into();
        second.placements[0].record_start = graph::Rational::integer(1);

        let bins = [PathBuf::from("First/Edit.avb"), PathBuf::from("Second/Edit.avb")];
        let owners = HashMap::from([("first".into(), 0), ("second".into(), 1)]);
        let options = prproj::convert::model::Options::default();
        let clock = ClockProfile::LegacyImport;
        let prepare = |graph: &graph::source::SourceGraph| premiere_adapter::prepare_avid_job(
            graph, &options, clock, false, false, None,
        );
        if let Err(error) = prepare(&graph) {
            panic!("the valid clips must prepare: {}", error.message);
        }
        graph.items[0].media[0].source_offset = None;
        let refusal = prepare(&graph).err().expect("unknown item-to-file origin refuses");
        assert!(refusal.sequence.is_none());
        assert_eq!(refusal.occurrence, Some(PreparationOccurrence::Item("clip:master".into())));
        let original = premiere_adapter::prepare_linked_originals(
            &graph, &options, clock, false,
        ).err().unwrap();
        assert_eq!(refusal.message, original.to_string());
        assert!(refusal.message.starts_with("unresolved_source_offset clip:master -> pcm1"));
        let issue = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        let clip = format!("\"Master\" · saved clip clip:master · Avid bin {}", bins[0].display());
        assert_eq!(issue.affected, [
            clip.as_str(),
            "Same name · A7 · frame 24 from sequence start · edit first/edit",
            "Same name · A7 · 1/48000 seconds from sequence start · disabled edit second/edit",
        ]);
        let [first, second] = issue.timeline_groups.as_slice() else { panic!("expected both uses") };
        assert_eq!((&*first.id, first.bin.as_ref(), first.note_indices.as_slice()), ("first", Some(&bins[0]), &[1][..]));
        assert_eq!((&*second.id, second.bin.as_ref(), second.note_indices.as_slice()), ("second", Some(&bins[1]), &[2][..]));

        graph.sequences[0].placements.clear();
        graph.sequences[1].disabled_placements.clear();
        let refusal = prepare(&graph).err().expect("unused source still needs a verified origin");
        let issue = premiere_preparation_blocker(refusal, &graph, Some((&owners, &bins)));
        assert_eq!(issue.affected, [clip]);
        assert!(issue.timeline_groups.is_empty());
    }

    #[test]
    fn rewrap_review_paths_include_only_distinct_referenced_files() {
        let mut model = prproj::convert::model::parse_model_str(include_str!(
            "../../../../examples/opatom_clip.model.json"
        )).unwrap();
        let used = model.clips.get("clip-1").unwrap().clone();
        let mut shared = used.clone();
        shared.id = "shared".into();
        model.bins[0].clips.push(shared.id.clone());
        let mut unused = used.clone();
        unused.id = "unused".into();
        unused.video.as_mut().unwrap().path = "unselected.mxf".into();
        model.clips = [used.clone(), shared, unused].into_iter().collect();

        let expected: Vec<_> = used.video.iter().map(|video| Path::new(&video.path))
            .chain(used.audio.iter().map(|audio| Path::new(&audio.path))).collect();
        assert_eq!(rewrap::referenced_input_paths(&model).collect::<Vec<_>>(), expected);
    }

    #[test]
    fn blocked_preflight_exports_all_collected_details_without_publishing() {
        let scratch = Scratch::new();
        let spec = JobSpec {
            output: scratch.0.join("New.prproj"),
            ..JobSpec::default()
        };
        let mut source = graph::source::SourceGraph {
            project: graph::Project { name: "Synthetic project".into() },
            media: vec![],
            items: vec![],
            bins: vec![],
            sequences: vec![],
            losses: (1..=10).map(|index| graph::Loss {
                object: "sequence:0".into(),
                aspect: avid_adapter::UNQUALIFIED_ROUTE_TIMING.into(),
                reason: format!("Unsupported edit {index}"),
            }).collect(),
            sidecars: vec![],
            multicams: vec![],
            multicam_uses: vec![],
        };
        source.losses.push(source.losses[0].clone());
        let issue = sequence_timing_blocker(&source, None).unwrap();
        assert_eq!(issue.affected.len(), 10);
        let summary = issue.clone().cli_summary();
        assert!(summary.ends_with(
            "Selected sequence · Unsupported edit 8; More edits are affected in the selected bins."
        ));
        assert!(!summary.contains("Unsupported edit 9"));
        assert!(std::fs::read_dir(&scratch.0).unwrap().next().is_none());

        let reserved = scratch.0.join("NEW.prproj.ANYNLE-RELINK.JSON");
        assert!(save_blocked_preflight_report(&spec, &issue, &reserved)
            .unwrap_err()
            .contains("reserved"));
        let path = scratch.0.join("Blocked.json");
        save_blocked_preflight_report(&spec, &issue, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let report: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(report["schema"], "anynle.preflight-report/1");
        assert_eq!(report["status"], "blocked");
        assert_eq!(report["blocker"]["kind"], "sequence_timing");
        assert_eq!(report["blocker"]["message"], issue.message);
        assert_eq!(report["blocker"]["affected"], serde_json::json!(issue.affected));
        assert!(save_blocked_preflight_report(&spec, &issue, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);

        let broken_output = JobSpec {
            output: scratch.0.join("missing parent").join("New.prproj"),
            ..JobSpec::default()
        };
        let Err(path_issue) = prepare_for_review(broken_output.clone(), &CancelToken::default(), &mut |_| {}) else {
            panic!("expected a missing output parent to block");
        };
        assert_eq!(path_issue.kind, BlockerKind::ProjectOutput);
        save_blocked_preflight_report(&broken_output, &path_issue, &scratch.0.join("Output Blocked.json"))
            .unwrap();
        assert!(!broken_output.output.parent().unwrap().exists());
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "anynle-job-control-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
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
    fn prepared(root: &Path) -> PreparedJob {
        use prproj::convert::model::{Model, Options};
        let spec = JobSpec {
            output: root.join("new.prproj"),
            ..JobSpec::default()
        };
        let model = Model {
            project_name: "Synthetic safety control".into(),
            options: Options::default(),
            clips: Default::default(),
            bins: vec![],
            sequences: vec![],
        };
        let document = prproj::convert::writer::prepare_project(
            &model,
            &spec.output,
            &mut prproj::SeededUuids::new("job-safety"),
            &[],
            false,
        )
        .unwrap();
        PreparedJob {
            receipt: sidecar(&spec.output, ".anynle-report.json").unwrap(),
            replay: sidecar(&spec.output, ".anynle-job.json").unwrap(),
            spec,
            review: Review {
                inventory: Inventory::default(),
                rewrap_sources: vec![],
                estimated_bytes: None,
                available_bytes: None,
                camera_originals: vec![],
                media_linking: None,
                notes: vec![],
                subjects: HashMap::new(),
                timeline_groups: vec![],
                source_groups: vec![],
                cli_equivalent: String::new(),
                diagnostic_locations: HashMap::new(),
            },
            document,
            media: None,
            media_created: false,
            inputs: vec![],
            media_inputs: vec![],
            originals: vec![],
            proxy_bindings: vec![],
            offline_relink: None,
            graph_debug: None,
            aux_outputs: vec![],
        }
    }

    #[test]
    fn preflight_report_saves_every_finding_and_leaves_job_ready() {
        let scratch = Scratch::new();
        let mut job = prepared(&scratch.0);
        job.review.inventory.found_media = 1;
        job.review.inventory.total_media = 2;
        job.review.inventory.notes.push("Inventory finding".into());
        job.review
            .subjects
            .insert("edit:1".into(), "Same display name".into());
        job.review
            .subjects
            .insert("edit:2".into(), "Same display name".into());
        for index in 0..51 {
            let (severity, object) = if index % 2 == 0 {
                ("warning", "edit:1")
            } else {
                ("loss", "edit:2")
            };
            job.review
                .notes
                .push(note(severity, object, format!("Finding {index}"), "Saved reason"));
        }

        let reserved = scratch.0.join("NEW.prproj.ANYNLE-JOB.JSON");
        assert!(job
            .save_preflight_report(&reserved)
            .unwrap_err()
            .contains("reserved"));
        assert!(!reserved.exists());

        let report_path = scratch.0.join("Preflight.json");
        job.save_preflight_report(&report_path).unwrap();
        let bytes = std::fs::read(&report_path).unwrap();
        assert!(job.save_preflight_report(&report_path).is_err());
        assert_eq!(std::fs::read(&report_path).unwrap(), bytes);
        let report: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(report["schema"], "anynle.preflight-report/1");
        assert_eq!(report["status"], "ready");
        assert_eq!(report["project_output"], job.spec.output.to_str().unwrap());
        assert_eq!(report["found_media"], 1);
        assert_eq!(report["total_media"], 2);
        assert_eq!(
            report["inventory_notes"],
            serde_json::json!(["Inventory finding"])
        );
        assert_eq!(report["subjects"]["edit:1"], "Same display name");
        assert_eq!(report["subjects"]["edit:2"], "Same display name");
        assert_eq!(
            report["report"],
            serde_json::to_value(&job.review.notes).unwrap()
        );
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);

        let completion = execute(job, &CancelToken::default(), &mut |_| {});
        assert_eq!(completion.status, CompletionStatus::Complete);
        assert!(scratch.0.join("new.prproj").is_file());
        assert_eq!(std::fs::read(&report_path).unwrap(), bytes);
    }

    #[test]
    fn missing_proxy_folder_offers_proxy_specific_recovery() {
        let scratch = Scratch::new();
        let spec = JobSpec {
            mode: ClipMode::Separate,
            attach_avid_proxies: true,
            proxy_rewrap_dir: Some(scratch.0.join("missing proxies")),
            ..JobSpec::default()
        };
        let Err(issue) = prepare_for_review(spec, &CancelToken::default(), &mut |_| {}) else {
            panic!("expected Review to block");
        };
        assert_eq!(issue.kind, BlockerKind::ProxyMedia);
        assert!(issue.message.contains("missing proxies"));
    }
    #[test]
    fn successful_job_does_not_repeat_preparation_notes_as_publication_notes() {
        let t = Scratch::new();
        let mut job = prepared(&t.0);
        let notes = job.document.report().notes.clone();
        assert!(!notes.is_empty());
        for d in &notes {
            job.review.notes.push(note("info", "writer", d.clone(), "Writer preparation report."));
        }
        job.review.subjects.insert("writer".into(), "Synthetic writer".into());
        let subjects = job.take_review_subjects();
        assert_eq!(subjects["writer"], "Synthetic writer");
        assert!(job.review().subjects.is_empty());
        let report_buffer = job.review().notes.as_ptr();
        let done = execute(job, &CancelToken::default(), &mut |_| {});
        assert_eq!(done.status, CompletionStatus::Complete);
        assert_eq!(done.report.as_ptr(), report_buffer);
        assert!(done.report.iter().all(|n| n.object != "publication"));
        assert_eq!(
            done.report.iter().filter(|n| n.object == "writer").map(|n| &n.message).collect::<Vec<_>>(),
            notes.iter().collect::<Vec<_>>(),
        );
    }
    #[test]
    fn late_project_is_preserved_and_not_claimed_as_our_output() {
        let t = Scratch::new();
        let job = prepared(&t.0);
        std::fs::write(&job.spec.output, b"incumbent").unwrap();
        let done = execute(job, &CancelToken::default(), &mut |_| {});
        assert_eq!(done.status, CompletionStatus::Failed);
        assert!(done.output.is_none());
        assert_eq!(std::fs::read(t.0.join("new.prproj")).unwrap(), b"incumbent");
        assert!(!done.retained_paths.contains(&t.0.join("new.prproj")));
        assert!(done.receipt.unwrap().is_file());
    }
    #[test]
    fn linked_media_same_size_replacement_blocks_publication() {
        let t = Scratch::new();
        let media = t.0.join("source.mxf");
        std::fs::write(&media, b"first").unwrap();
        let mut job = prepared(&t.0);
        job.media_inputs = source_input::pin(&[media.clone()]).unwrap();
        std::fs::write(&media, b"other").unwrap();
        let mut progress = Vec::new();
        let done = execute(job, &CancelToken::default(), &mut |p| progress.push(p));
        assert_eq!(done.status, CompletionStatus::Failed);
        assert!(done.message.contains("changed"));
        assert_eq!(progress[0].message, "Rechecking media 1/1: source.mxf");
        assert!(progress.iter().all(|p| p.stage != "publication"));
        assert!(!t.0.join("new.prproj").exists());
        assert_eq!(std::fs::read(media).unwrap(), b"other");
    }

    #[test]
    fn linked_media_removed_while_checking_a_later_file_blocks_publication() {
        let t = Scratch::new();
        let mut job = prepared(&t.0);
        let paths = [t.0.join("picture.mxf"), t.0.join("audio.mxf")];
        for path in &paths {
            std::fs::write(path, b"synthetic media").unwrap();
        }
        job.media_inputs = source_input::pin(&paths).unwrap();
        let done = execute(job, &CancelToken::default(), &mut |p| {
            assert_ne!(p.stage, "publication");
            if p.stage == "source verification" && p.completed == 1 {
                std::fs::remove_file(&paths[0]).unwrap();
            }
        });
        assert_eq!(done.status, CompletionStatus::Failed);
        assert!(done.message.contains(paths[0].to_str().unwrap()));
        assert!(done.output.is_none());
        assert!(!t.0.join("new.prproj").exists());
        assert_eq!(std::fs::read(&paths[1]).unwrap(), b"synthetic media");
    }

    #[test]
    fn source_bin_moved_after_hashing_blocks_publication() {
        for boundary in ["source verification", "publication"] {
            let t = Scratch::new();
            let mut job = prepared(&t.0);
            let bin = t.0.join("source.avb");
            let moved = t.0.join("moved.avb");
            let media = t.0.join("picture.mxf");
            std::fs::write(&bin, b"synthetic source bin").unwrap();
            std::fs::write(&media, b"synthetic media").unwrap();
            job.inputs = source_input::pin(std::slice::from_ref(&bin)).unwrap();
            job.media_inputs = source_input::pin(std::slice::from_ref(&media)).unwrap();
            let done = execute(job, &CancelToken::default(), &mut |p| {
                if p.stage == boundary {
                    std::fs::rename(&bin, &moved).unwrap();
                }
            });
            assert_eq!(done.status, CompletionStatus::Failed, "{boundary}");
            assert!(done.message.contains(bin.to_str().unwrap()));
            assert!(done.output.is_none());
            assert!(!t.0.join("new.prproj").exists());
            assert_eq!(std::fs::read(moved).unwrap(), b"synthetic source bin");
            assert_eq!(std::fs::read(media).unwrap(), b"synthetic media");
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_media_replaced_at_asset_publication_with_same_size_and_mtime_refuses() {
        let t = Scratch::new();
        let media = t.0.join("source.mxf");
        let replacement = t.0.join("replacement.mxf");
        std::fs::write(&media, b"first").unwrap();
        std::fs::write(&replacement, b"other").unwrap();
        let mtime = std::fs::metadata(&media).unwrap().modified().unwrap();
        std::fs::File::open(&replacement)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&replacement).unwrap().modified().unwrap(),
            mtime
        );
        let mut job = prepared(&t.0);
        job.media_inputs = source_input::pin(std::slice::from_ref(&media)).unwrap();
        let done = execute(job, &CancelToken::default(), &mut |p| {
            if p.stage == "publication" {
                std::fs::rename(&replacement, &media).unwrap();
            }
        });
        assert_eq!(done.status, CompletionStatus::Failed);
        assert!(done.message.contains(media.to_str().unwrap()));
        assert!(!t.0.join("new.prproj").exists());
        assert_eq!(std::fs::read(media).unwrap(), b"other");
        assert!(done.report.iter().any(|row| row.severity == "error"
            && row.object == "source recheck before project publication"));
    }

    #[test]
    fn linked_media_recheck_reports_each_file_and_honors_cancellation() {
        let t = Scratch::new();
        let mut job = prepared(&t.0);
        let paths = [t.0.join("picture.mxf"), t.0.join("audio.mxf")];
        for path in &paths {
            std::fs::write(path, b"synthetic media").unwrap();
        }
        job.media_inputs = source_input::pin(&paths).unwrap();
        let cancel = CancelToken::default();
        let mut seen = Vec::new();
        let done = execute(job, &cancel, &mut |p| {
            assert_ne!(p.stage, "publication");
            if p.stage == "source verification" {
                if p.completed == 1 {
                    cancel.cancel();
                }
                seen.push((p.completed, p.total, p.message));
            }
        });
        assert_eq!(done.status, CompletionStatus::Cancelled);
        assert_eq!(seen, [
            (0, 2, "Rechecking media 1/2: picture.mxf".into()),
            (1, 2, "Rechecking media 2/2: audio.mxf".into()),
        ]);
        assert!(!t.0.join("new.prproj").exists());
    }
    #[test]
    fn receipt_collision_after_publication_keeps_project_and_incumbent_receipt() {
        let t = Scratch::new();
        let job = prepared(&t.0);
        let receipt = job.receipt.clone();
        let done = execute(job, &CancelToken::default(), &mut |p| {
            if p.stage == "publication" {
                std::fs::write(&receipt, b"incumbent receipt").unwrap();
            }
        });
        assert_eq!(done.status, CompletionStatus::Failed);
        assert!(done.output.unwrap().is_file());
        assert!(done
            .message
            .contains("Project published; completion receipt failed"));
        assert_eq!(std::fs::read(&receipt).unwrap(), b"incumbent receipt");
        assert!(!done.retained_paths.contains(&receipt));
    }

    #[test]
    fn late_relink_report_collision_reports_the_published_project() {
        let scratch = Scratch::new();
        let mut job = prepared(&scratch.0);
        let project = job.spec.output.clone();
        let relink = sidecar(&project, ".anynle-relink.json").unwrap();
        let replay = job.replay.clone();
        job.offline_relink = Some((relink.clone(), "{}".into()));
        let done = execute(job, &CancelToken::default(), &mut |event| {
            if event.stage == "publication" {
                std::fs::write(&relink, b"incumbent report").unwrap();
            }
        });
        assert_eq!(done.status, CompletionStatus::Failed);
        assert_eq!(done.output.as_ref(), Some(&project));
        assert!(done.message.contains("Project published"));
        assert!(done.message.contains("Offline relink report could not be created"));
        assert!(project.is_file() && replay.is_file());
        assert_eq!(std::fs::read(&relink).unwrap(), b"incumbent report");
        let persisted: Completion =
            serde_json::from_slice(&std::fs::read(done.receipt.unwrap()).unwrap()).unwrap();
        assert!(persisted.retained_paths.contains(&project));
        assert!(persisted.retained_paths.contains(&replay));
        assert!(!persisted.retained_paths.contains(&relink));
        assert!(persisted.report.iter().any(|note| {
            note.severity == "error" && note.object == "offline relink report"
        }));
    }

    #[test]
    fn graph_diagnostics_report_progress_and_preserve_an_incumbent_file() {
        for collision in [false, true] {
            let scratch = Scratch::new();
            let mut job = prepared(&scratch.0);
            let source = scratch.0.join("staged.jsonl");
            let diagnostics = sidecar(&job.spec.output, ".anynle-debug.jsonl").unwrap();
            let bytes = b"{\"owner\":\"sequence:1\",\"data\":{}}\n";
            std::fs::write(&source, bytes).unwrap();
            job.graph_debug = Some((diagnostics.clone(), std::fs::File::open(&source).unwrap()));
            let mut progress = Vec::new();
            let done = execute(job, &CancelToken::default(), &mut |event| {
                if event.stage == "publication" && collision {
                    std::fs::write(&diagnostics, b"incumbent").unwrap();
                }
                if event.stage == "diagnostics" {
                    progress.push((event.completed, event.total));
                }
            });
            assert!(done.output.unwrap().is_file());
            if collision {
                assert!(progress.is_empty());
                assert_eq!(done.status, CompletionStatus::Failed);
                assert!(!done.retained_paths.contains(&diagnostics));
                assert_eq!(std::fs::read(&diagnostics).unwrap(), b"incumbent");
            } else {
                assert_eq!(progress, [(0, 100), (100, 100)]);
                assert_eq!(done.status, CompletionStatus::Complete);
                assert!(done.retained_paths.contains(&diagnostics));
                assert_eq!(std::fs::read(&diagnostics).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn completion_reports_replay_failure_even_if_receipt_also_fails() {
        for block_receipt in [false, true] {
            let t = Scratch::new();
            let job = prepared(&t.0);
            let replay = job.replay.clone();
            let receipt = job.receipt.clone();
            let done = execute(job, &CancelToken::default(), &mut |p| {
                if p.stage == "publication" {
                    std::fs::write(&replay, b"incumbent replay").unwrap();
                    if block_receipt {
                        std::fs::write(&receipt, b"incumbent receipt").unwrap();
                    }
                }
            });
            assert_eq!(done.status, CompletionStatus::Failed);
            let project = done.output.as_ref().unwrap();
            assert!(project.is_file());
            assert!(done.retained_paths.contains(project));
            assert_eq!(std::fs::read(&replay).unwrap(), b"incumbent replay");
            assert!(!done.retained_paths.contains(&replay));
            let replay_failure = done.message.find("replay settings could not be saved").unwrap();
            let stages: Vec<_> = done
                .report
                .iter()
                .filter(|n| n.severity == "error")
                .map(|n| n.object.as_str())
                .collect();
            if block_receipt {
                let receipt_failure = done.message.find("completion receipt failed").unwrap();
                assert!(replay_failure < receipt_failure);
                assert!(done.receipt.is_none());
                assert_eq!(stages, ["replay settings", "completion receipt"]);
                assert_eq!(std::fs::read(&receipt).unwrap(), b"incumbent receipt");
                assert!(!done.retained_paths.contains(&receipt));
            } else {
                let persisted: Completion =
                    serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
                assert_eq!(done.receipt.as_ref(), Some(&receipt));
                assert_eq!(stages, ["replay settings"]);
                assert_eq!(persisted.message, done.message);
                assert_eq!(persisted.report, done.report);
            }
        }
    }

    #[test]
    fn import_timestamp_handles_epoch_leap_day_and_range() {
        assert_eq!(import_name(0).unwrap(), "AVID_IMPORT_1970-01-01_00:00:00");
        assert_eq!(
            import_name(1709208000).unwrap(),
            "AVID_IMPORT_2024-02-29_12:00:00"
        );
        assert!(import_name(u64::MAX).is_err());
    }
    #[test]
    fn encoded_cli_roundtrips_literal_paths_and_large_jobs_use_saved_settings() {
        let mut s = JobSpec::default();
        s.project_name = Some("Literal ' \" $ backtick ` and Unicode café".into());
        let command = cli_command(&s).unwrap();
        assert_eq!(
            load_encoded(command.strip_prefix("any-nle run --job-base64 ").unwrap()).unwrap(),
            s
        );
        assert!(load_encoded("; invalid").is_err());
        s.project_name = Some("x".repeat(6000));
        assert!(cli_command(&s).unwrap().is_empty());
        assert!(load_encoded(&"x".repeat(2 << 20)).is_err());
    }
    #[test]
    fn bin_changed_after_inventory_cannot_authorize_different_source_facts() {
        let t = Scratch::new();
        let bin = t.0.join("source.avb");
        avid_reader::document::AvbDocument::new_empty_bin(123, "Synthetic", 0, false)
            .write_new(&bin)
            .unwrap();
        let spec = JobSpec {
            bins: vec![bin.clone()],
            output: t.0.join("refused.prproj"),
            ffmpeg: Some("unused-ffmpeg".into()),
            ffprobe: Some("unused-ffprobe".into()),
            ..JobSpec::default()
        };
        let mut changed = false;
        let error = prepare_for_review(spec, &CancelToken::default(), &mut |p| {
            if p.stage == "Inventory" && p.completed == p.total && !changed {
                let mut data = std::fs::read(&bin).unwrap();
                data.push(0);
                std::fs::write(&bin, data).unwrap();
                changed = true;
            }
        })
        .err()
        .unwrap();
        assert!(changed);
        assert_eq!(error.kind, BlockerKind::Retry);
        assert!(error.message.contains("Bin inventory changed"));
        assert!(!t.0.join("refused.prproj").exists());
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 1);
    }
    #[test]
    fn prepared_job_can_cross_worker_boundary() {
        fn send<T: Send + 'static>() {}
        send::<PreparedJob>();
    }
    #[test]
    fn cancelled_preparation_has_no_side_effects() {
        let c = CancelToken::default();
        c.cancel();
        let p = std::env::temp_dir().join("anynle-cancel-never-created.prproj");
        let s = JobSpec {
            output: p.clone(),
            ..JobSpec::default()
        };
        assert_eq!(prepare(s, &c, &mut |_| {}).err().unwrap(), "cancelled");
        assert!(!p.exists());
    }
    #[test]
    fn unknown_schema_and_bad_output_refuse_before_io() {
        let s = JobSpec {
            schema: "anynle.job/2".into(),
            ..JobSpec::default()
        };
        assert!(prepare(s, &CancelToken::default(), &mut |_| {})
            .err()
            .unwrap()
            .contains("schema"));

        let t = Scratch::new();
        let output = t.0.join("Existing.prproj");
        std::fs::write(&output, b"incumbent").unwrap();
        let spec = JobSpec { output: output.clone(), ..JobSpec::default() };
        let issue = prepare_for_review(spec, &CancelToken::default(), &mut |_| {
            panic!("an existing output must stop preflight before inventory")
        })
        .err()
        .unwrap();
        assert_eq!(issue.kind, BlockerKind::ProjectOutput);
        assert_eq!(std::fs::read(output).unwrap(), b"incumbent");
    }
}
