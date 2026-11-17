//! Read-only preparation followed by one explicit execution of the prepared document.
use super::*;
use crate::premiere_preparation::timeline_subject;
use crate::source_input::{original_stamp, recheck_originals, OriginalStamp};
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
const PREFLIGHT_REPORT_SCHEMA: &str = "anynle.preflight-report/1";
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
            .chain(spec.new_media.as_deref()),
    )?;
    #[derive(serde::Serialize)]
    struct BlockedReport<'a> {
        schema: &'static str,
        status: &'static str,
        project_output: &'a Path,
        blocker: &'a ReviewBlocker,
    }
    let report = BlockedReport {
        schema: PREFLIGHT_REPORT_SCHEMA,
        status: "blocked",
        project_output: &spec.output,
        blocker,
    };
    write_json(path, &report, &mut Vec::new())
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
    if let Some(dir) = &spec.new_media {
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
            .map_err(|error| error.to_string())?;

        #[derive(serde::Serialize)]
        struct PreflightReport<'a> {
            schema: &'static str,
            status: &'static str,
            project_output: &'a Path,
            bins_checked: usize,
            found_media: usize,
            total_media: usize,
            inventory_notes: &'a [String],
            subjects: std::collections::BTreeMap<&'a str, &'a str>,
            report: &'a [ReportNote],
        }
        let review = &self.review;
        let report = PreflightReport {
            schema: PREFLIGHT_REPORT_SCHEMA,
            status: "ready",
            project_output: &self.spec.output,
            bins_checked: review.inventory.bins.len(),
            found_media: review.inventory.found_media,
            total_media: review.inventory.total_media,
            inventory_notes: &review.inventory.notes,
            subjects: review
                .subjects
                .iter()
                .map(|(id, name)| (id.as_str(), name.as_str()))
                .collect(),
            report: &review.notes,
        };
        write_json(path, &report, &mut Vec::new())
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

fn source_read_blocker(
    issue: avid_adapter::SourceReadIssue,
    linked: bool,
    inputs: &[SourceInput],
) -> ReviewBlocker {
    use avid_adapter::{OriginalLocatorKind, SourceReadIssue};
    use BlockerKind::*;

    let in_bin = |kind, message, bin: usize| {
        let mut blocker = ReviewBlocker::new(kind, message);
        if let Some(input) = inputs.get(bin) {
            blocker.affected.push(format!("Avid bin {}", input.path.display()));
        }
        blocker
    };
    match issue {
        SourceReadIssue::OriginalLocator(issue) => {
            let kind = match issue.kind {
                OriginalLocatorKind::LegacyMxfPackage => LegacyMxfPackage,
                OriginalLocatorKind::MissingOriginal if !linked => RewrapMissingOriginal,
                _ => OriginalLocator,
            };
            let mut blocker = in_bin(kind, issue.message, issue.bin);
            blocker.affected.extend(issue.additional.into_iter().map(|(bin, mut detail)| {
                if let Some(input) = inputs.get(bin) {
                    detail.push_str(&format!(" · {}", input.path.display()));
                }
                detail
            }));
            if issue.details_truncated {
                blocker.message.push_str("\nAdditional locator checks reached the diagnostic size limit.");
            }
            blocker
        }
        SourceReadIssue::Bin(message) => ReviewBlocker::new(ProjectSource, message),
        SourceReadIssue::OriginalMedia(message) => ReviewBlocker::new(OriginalMedia, message),
        SourceReadIssue::MediaScan(message) => ReviewBlocker::new(AvidMedia, message),
        SourceReadIssue::SourceFacts(message) => ReviewBlocker::new(SourceContent, message),
    }
}

fn unresolved_masters(
    source: &graph::source::SourceGraph,
    inputs: &[SourceInput],
    matte: &crate::png_mattes::Plan,
) -> Vec<String> {
    use graph::Value;

    let mut details = Vec::new();
    for sidecar in source.sidecars.iter().filter(|s| s.adapter == "avid.source") {
        let Some(Value::Arr(masters)) = sidecar.data.get("unresolved_masters") else {
            continue;
        };
        let bin = sidecar
            .object
            .strip_prefix("bin:")
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| inputs.get(index));
        for master in masters {
            let (Some(Value::Str(name)), Some(Value::Str(reason))) =
                (master.get("name"), master.get("reason"))
            else {
                continue;
            };
            let mut detail = format!("{name}: {reason}");
            if let Some(Value::Str(occurrence)) = master.get("occurrence") {
                if !matte.keeps_unresolved_master(&sidecar.object, occurrence, reason) {
                    continue;
                }
                detail.push_str(&format!(" · {occurrence}"));
            }
            if let Some(bin) = bin {
                detail.push_str(&format!(" · {}", bin.path.display()));
            }
            details.push(detail);
        }
    }
    details
}

fn media_scan_blocker(
    scan_issues: &[String],
    audio_group_issues: &[String],
    for_review: bool,
) -> ReviewBlocker {
    let kind = if scan_issues.is_empty() {
        BlockerKind::SourceContent
    } else {
        BlockerKind::AvidMedia
    };
    let summary = "Media scan or channel routing needs resolution";
    if for_review {
        ReviewBlocker::new(kind, summary)
    } else {
        let details = scan_issues
            .iter()
            .chain(audio_group_issues)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        ReviewBlocker::new(kind, format!("{summary}: {details}"))
    }
}

pub(crate) fn sequence_timing_blocker(source: &graph::source::SourceGraph) -> Option<ReviewBlocker> {
    let mut blocked = source
        .losses
        .iter()
        .filter(|loss| loss.aspect == avid_adapter::UNQUALIFIED_ROUTE_TIMING)
        .peekable();
    blocked.peek()?;
    let names: HashMap<&str, &str> = source
        .sequences
        .iter()
        .map(|sequence| (sequence.id.as_str(), sequence.name.as_str()))
        .collect();
    let mut issue = ReviewBlocker::new(
        BlockerKind::SequenceTiming,
        "Conversion would omit edits with unsupported Avid sequence timing",
    );
    let mut seen = HashSet::new();
    for loss in blocked {
        if seen.insert((loss.object.as_str(), loss.reason.as_str())) {
            let name = names.get(loss.object.as_str()).copied().unwrap_or("Selected sequence");
            issue.affected.push(format!("{name} · {}", loss.reason));
        }
    }
    Some(issue)
}

fn group_clip_mode_blocker(
    source: &graph::source::SourceGraph,
    mode: ClipMode,
) -> Option<ReviewBlocker> {
    if !matches!(mode, ClipMode::Grouped | ClipMode::Merged)
        || (source.multicams.is_empty() && source.multicam_uses.is_empty())
    {
        return None;
    }
    let mut issue = ReviewBlocker::new(
        BlockerKind::GroupClipMode,
        "Linked editable Groups require Separate files",
    );
    issue.affected.extend(source.multicams.iter().map(|group| {
        if group.name.is_empty() { group.id.clone() } else { group.name.clone() }
    }));
    Some(issue)
}

fn review_subjects(
    source: &graph::source::SourceGraph,
    notes: &[ReportNote],
) -> HashMap<String, String> {
    let wanted: HashSet<&str> = notes
        .iter()
        .filter(|n| matches!(n.severity.as_str(), "loss" | "warning"))
        .map(|n| n.object.as_str())
        .collect();
    let mut subjects = HashMap::new();
    let mut names: HashMap<&str, &str> = HashMap::new();
    for item in &source.items {
        names.insert(&item.id, &item.name);
    }
    for sequence in &source.sequences {
        names.insert(&sequence.id, &sequence.name);
        for transition in &sequence.transitions {
            if wanted.contains(transition.id.as_str()) {
                subjects.insert(
                    transition.id.clone(),
                    timeline_subject(sequence, transition.kind, transition.track, Some(transition.cut)),
                );
            }
        }
        for placement in &sequence.placements {
            if !wanted.contains(placement.id.as_str()) {
                continue;
            }
            // Graph/2 placements occupy one lane, including a multichannel audio group.
            let lane = if let Some(track) = placement.video_track {
                Some((graph::StreamKind::Video, track))
            } else {
                placement.audio_tracks.first().copied()
                    .or_else(|| placement.grouped_audio.first().map(|group| group.track))
                    .map(|track| (graph::StreamKind::Audio, track))
            };
            if let Some((kind, track)) = lane {
                subjects.insert(
                    placement.id.clone(),
                    timeline_subject(sequence, kind, track, Some(placement.record_start)),
                );
            }
        }
    }
    let sequences: HashMap<_, _> = source.sequences.iter()
        .map(|sequence| (sequence.id.as_str(), sequence)).collect();
    for usage in &source.multicam_uses {
        if wanted.contains(usage.id.as_str()) {
            if let Some(sequence) = sequences.get(usage.sequence.as_str()) {
                subjects.insert(
                    usage.id.clone(),
                    timeline_subject(sequence, usage.kind, usage.track, usage.record_start),
                );
            }
        }
    }
    for group in &source.multicams {
        names.insert(&group.id, &group.name);
    }
    for bin in &source.bins {
        names.insert(&bin.id, &bin.name);
    }
    for id in wanted {
        if subjects.contains_key(id) {
            continue;
        }
        // Native child occurrence IDs extend their sequence's ID with slash
        // components. Use the closest enclosing name, never a different ID.
        let mut object = id;
        loop {
            if let Some(name) = names.get(object).filter(|name| !name.is_empty()) {
                subjects.insert(id.to_owned(), (*name).to_owned());
                break;
            }
            match object.rsplit_once('/') {
                Some((parent, _)) => object = parent,
                None => break,
            }
        }
    }
    subjects
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
                collect_blocked_locators: for_review,
            },
            spec.allow_offline_media,
            linked,
            linked && spec.entire_project,
            &mut |event| {
                use avid_adapter::SourceProgress;
                cancel.check()?;
                match event {
                    SourceProgress::ReadingBin { index, path } => cb(Progress {
                        stage: "source".into(),
                        current_bin: Some(path.into()),
                        completed: index,
                        total: bins.len(),
                        message: format!("Reading bin {}/{}: {}", index + 1, bins.len(), path.display()),
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
    if let Some(issue) = group_clip_mode_blocker(&read.graph, spec.mode) {
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
    let mut media_inputs = source_input::pin_checked(&native_paths, &mut || cancel.check())
        .map_err(blocker(AvidMedia))?;
    if !media_inputs.is_empty() {
        check_cancel()?;
        let again = read_source(cb)?;
        if again.graph != read.graph {
            return Err(ReviewBlocker::new(
                Retry,
                "Source interpretation changed while native media was being verified",
            ));
        }
        source_input::recheck_checked(&media_inputs, &mut || cancel.check())
            .map_err(blocker(AvidMedia))?;
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
        &mut || cancel.check()).map_err(blocker(OriginalMedia))?;
    let png_count = matte.inputs.len();
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
            issue.affected.append(&mut read.scan_issues);
            issue.affected.append(&mut read.audio_group_issues);
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
        if let Some(issue) = sequence_timing_blocker(&read.graph) {
            return Err(issue);
        }
        crate::offline::check_edit_routes(&read.graph).map_err(blocker(SourceContent))?;
    }
    for l in policy.losses_source(&read.graph) {
        notes.push(note("loss", l.object, l.reason, l.aspect));
    }
    let native = spec.mode == ClipMode::RewrapNative;
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
        &mut || cancel.check(),
    )
    .map_err(blocker(SourceContent))?;
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
    let mut native_count = 0;
    for clip in &input.clips {
        match clip {
            PreparedClip::Original(c) => {
                if !offline.contains(c.path.as_str()) && seen.insert(c.path.clone()) {
                    originals.push(original_stamp(Path::new(&c.path)).map_err(blocker(OriginalMedia))?);
                }
            }
            PreparedClip::Legacy(_) => native_count += 1,
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
    inv.total_media = paths(None, false) + png_count;
    inv.found_media = paths(None, true) + png_count;
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
    let media = if native && native_count > 0 {
        let dir = spec
            .new_media
            .as_deref()
            .ok_or_else(|| ReviewBlocker::new(NewMedia, "Native OP-Atom clips need a NEW media output folder. Creates new media."))?;
        let model = input
            .build_native_media_model()
            .map_err(|e| ReviewBlocker::new(Rewrap, e.to_string()))?;
        let native_media = rewrap::NativeMediaPreparation {
            model: &model,
            writer_input: Some(&mut input),
            gain: &gain,
            managed_pictures: luts.managed_pictures,
        };
        native_media.preflight().map_err(blocker(Rewrap))?;
        let roots = spec
            .media_roots
            .iter()
            .chain(&spec.original_roots)
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let plan = native_media
            .prepare(dir, &roots, &spec.output, None, &tools.ffmpeg, &tools.ffprobe)
            .map_err(|issue| match issue {
                rewrap::PrepareIssue::Destination(m) => ReviewBlocker::new(NewMedia, m),
                rewrap::PrepareIssue::Tools(m) => ReviewBlocker::new(MediaTools, m),
                rewrap::PrepareIssue::Source(m) => ReviewBlocker::new(Rewrap, m),
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
    if !native && spec.new_media.is_some() {
        return Err(ReviewBlocker::new(Rewrap, "A new-media folder applies only to the rewrap mode"));
    }
    if native_count == 0 {
        notes.push(note(
            "info",
            "media",
            "Original-file links do not create new media.",
            "Premiere will reference the resolved originals in place.",
        ));
    }
    let bindings = media.as_ref().map(|p| p.bindings()).unwrap_or_default();
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
    let document = if crate::managed_op1a::has_verified_streams(&read.graph) {
        crate::managed_op1a::prepare(
            &read.graph, &mut input, &spec.output, &mut ids, layout.as_ref(), &offline,
        )
    } else {
        premiere_preparation::prepare_document(
            &input, &spec.output, &mut ids, &bindings, native, layout.as_ref(), &offline,
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
    document
        .check_reserved_output(&receipt)
        .map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
    document
        .check_reserved_output(&replay)
        .map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
    if let Some((path, _)) = &offline_relink {
        document.check_reserved_output(path).map_err(|e| ReviewBlocker::new(ProjectOutput, e.to_string()))?;
    }
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
    let review = Review {
        inventory: inv,
        estimated_bytes: media.as_ref().map(|p| p.estimated_bytes()).or((proxy_bytes > 0).then_some(proxy_bytes)),
        available_bytes: media.as_ref().map(|p| p.available_bytes()).or_else(||
            (proxy_bytes > 0).then(|| spec.proxy_rewrap_dir.as_deref().and_then(|p| fs2::available_space(p).ok())).flatten()),
        camera_originals: if look_up_camera_originals {
            camera_original_links(&read.graph)
        } else {
            Vec::new()
        },
        subjects: review_subjects(&read.graph, &notes),
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
fn recheck_media(
    inputs: &[SourceInput],
    cancel: &CancelToken,
    cb: &mut dyn FnMut(Progress),
) -> Result<(), String> {
    let total = inputs.len();
    for (completed, input) in inputs.iter().enumerate() {
        cancel.check()?;
        let name = input.path.file_name().unwrap_or(input.path.as_os_str()).to_string_lossy();
        cb(Progress {
            stage: "source verification".into(),
            current_bin: None,
            completed,
            total,
            message: format!("Rechecking media {}/{}: {name}", completed + 1, total),
        });
        source_input::recheck_checked(std::slice::from_ref(input), &mut || cancel.check())?;
    }
    Ok(())
}

fn write_json<T: serde::Serialize>(
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
        source_input::recheck_checked(&job.inputs, &mut || cancel.check())?;
        recheck_media(&job.media_inputs, cancel, cb)?;
        recheck_originals(&job.originals)?;
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
        // Media writes can take time. Link-only jobs already ran these checks
        // immediately before publication.
        if job.media.is_some() || writes_proxy {
            source_input::recheck_checked(&job.inputs, &mut || cancel.check())?;
            recheck_media(&job.media_inputs, cancel, cb)?;
            recheck_originals(&job.originals)?;
        }
        phase = "asset publication";
        progress(cb, "publication", "Publishing prepared assets and project");
        job.document.publish_assets().map_err(|e| e.to_string())?;
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
        let labels = review_subjects(&source, &notes);
        assert_eq!(labels["occurrence:1"], "Same name · A2 · 00:01:00;02");
        assert_eq!(labels["group:1"], "Two cameras");
        let mut same_name = source.sequences[0].clone();
        same_name.id = "seq:2".into();
        same_name.clock = None;
        same_name.start = None;
        source.sequences.push(same_name);
        source.multicam_uses[0].sequence = "seq:2".into();
        assert_eq!(review_subjects(&source, &notes)["occurrence:1"], "Same name · A2 · frame 1 from sequence start");
        source.multicam_uses[0].record_start = Rational::new(1, 48_000);
        assert_eq!(review_subjects(&source, &notes)["occurrence:1"], "Same name · A2");

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
        ];
        let mut source = SourceGraph {
            project: Project { name: "Synthetic".into() },
            media: vec![], items: vec![], bins: vec![], losses: vec![], sidecars: vec![],
            sequences: vec![other], multicams: vec![], multicam_uses: vec![],
        };
        sequence.placements[0].record_start = Rational::new(1, 48_000).unwrap();
        source.sequences.push(sequence);
        let labels = review_subjects(&source, &notes);
        assert_eq!(labels["dissolve"], "Same name · V5 · frame 1 from sequence start");
        assert_eq!(labels["edit"], "Same name · A7");
        assert_eq!(labels["picture"], "Same name · V9 · frame 24 from sequence start");
    }

    #[test]
    fn editable_groups_incompatible_clip_mode_has_a_specific_review_blocker() {
        use graph::source::{Multicam, SourceGraph};

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
        assert!(group_clip_mode_blocker(&source, ClipMode::Merged).is_none());
        source.multicams = (1..=9).map(|index| Multicam {
            id: format!("group:{index}"),
            name: format!("Group {index}"),
            rate: None,
            duration: None,
            lanes: vec![],
            reason: None,
        }).collect();
        for mode in [ClipMode::Grouped, ClipMode::Merged] {
            let issue = group_clip_mode_blocker(&source, mode).unwrap();
            assert_eq!(issue.kind, BlockerKind::GroupClipMode);
            assert_eq!(issue.cli_summary(), "Linked editable Groups require Separate files");
        }
        let issue = group_clip_mode_blocker(&source, ClipMode::Grouped).unwrap();
        assert_eq!(issue.affected.len(), 9);
        assert_eq!(issue.affected[0], "Group 1");
        assert_eq!(issue.affected[7], "Group 8");
        assert_eq!(issue.affected[8], "Group 9");
        assert!(group_clip_mode_blocker(&source, ClipMode::Separate).is_none());
        assert!(group_clip_mode_blocker(&source, ClipMode::RewrapNative).is_none());
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
        let issue = sequence_timing_blocker(&source).unwrap();
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
                estimated_bytes: None,
                available_bytes: None,
                camera_originals: vec![],
                notes: vec![],
                subjects: HashMap::new(),
                cli_equivalent: String::new(),
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
