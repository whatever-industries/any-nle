//! Read-only preparation followed by one explicit execution of the prepared document.
use super::*;
use crate::{backend_bundle, output, rewrap, source_input};
use avid_reader::source::SourceInput;
use mapping::{AvidToPremiere, BinMode};
use prproj::convert::{prepared::PreparedClip, writer::PreparedProject};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::SystemTime,
};

const MAX_JOB_BYTES: usize = 1 << 20;
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
#[derive(Clone, Debug)]
struct OriginalStamp {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}
fn original_stamp(path: &Path) -> Result<OriginalStamp, String> {
    let m = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_file() {
        return Err(format!("{}: original is not a file", path.display()));
    }
    Ok(OriginalStamp {
        path: path.into(),
        len: m.len(),
        modified: m.modified().map_err(|e| e.to_string())?,
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
fn check_paths(spec: &JobSpec, receipt: &Path, replay: &Path) -> Result<(), String> {
    output::preflight(&spec.output, Some(receipt))?;
    output::preflight(&spec.output, Some(replay))?;
    if path_key(receipt)? == path_key(replay)? {
        return Err("Receipt and replay paths overlap".into());
    }
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
    receipt: PathBuf,
    replay: PathBuf,
    aux_outputs: Vec<PathBuf>,
}
impl PreparedJob {
    pub fn review(&self) -> &Review {
        &self.review
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
    cancel.check()?;
    if spec.schema != "anynle.job/1" {
        return Err("Unsupported job schema".into());
    }
    if serde_json::to_vec(&spec).map_err(|e| e.to_string())?.len() > MAX_JOB_BYTES {
        return Err("Job settings exceed 1 MiB".into());
    }
    if spec
        .output
        .extension()
        .is_none_or(|x| !x.eq_ignore_ascii_case("prproj"))
    {
        return Err("Choose a .prproj project output".into());
    }
    let receipt = sidecar(&spec.output, ".anynle-report.json")?;
    let replay = sidecar(&spec.output, ".anynle-job.json")?;
    check_paths(&spec, &receipt, &replay)?;
    let mut inv = super::inventory(&spec, cancel, cb)?;
    let bins = super::inventory::selected_paths(&spec, &inv)?;
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
        .collect::<Result<Vec<_>, String>>()?;
    source_input::recheck_checked(&inputs, &mut || cancel.check())
        .map_err(|e| format!("Bin inventory changed; run pre-flight again: {e}"))?;
    for choice in &spec.effects {
        let capability = inv
            .effects
            .iter()
            .find(|e| e.key == choice.key)
            .ok_or_else(|| format!("Unknown effect choice {}", choice.key))?;
        if !capability.modes.contains(&choice.mode) {
            return Err(format!(
                "Effect {} does not support that conversion mode",
                choice.key
            ));
        }
    }
    let mut policy = if spec.policy_json.trim().is_empty() {
        AvidToPremiere::default()
    } else {
        AvidToPremiere::from_json_text(&spec.policy_json).map_err(|e| e.to_string())?
    };
    policy.bin_mode = match spec.mode {
        ClipMode::RewrapNative | ClipMode::Separate => BinMode::Separate,
        ClipMode::Grouped => BinMode::Grouped,
        ClipMode::Merged => BinMode::Merged,
    };
    // The packaged wizard requires verified bundled tools. Explicit developer paths are deliberate overrides.
    let tools = backend_bundle::resolve(spec.ffmpeg.as_deref(), spec.ffprobe.as_deref(), true)?;
    progress(
        cb,
        "source",
        "Reading qualified Avid source facts and resolving media",
    );
    cancel.check()?;
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
        avid_adapter::read_with_originals_controlled(
            &inputs,
            &spec.media_roots,
            &project,
            &names,
            &avid_adapter::original::OriginalOptions {
                root_mappings: spec
                    .path_mappings
                    .iter()
                    .map(|m| avid_adapter::original::RootMapping {
                        saved_root: m.saved_prefix.clone(),
                        current_root: m.actual_root.clone(),
                    })
                    .collect(),
                ffprobe: tools.ffprobe.clone(),
            },
            &mut |n, path| {
                cancel.check()?;
                cb(Progress {
                    stage: "source".into(),
                    current_bin: Some(path.into()),
                    completed: n,
                    total: bins.len(),
                    message: format!("Reading bin {}/{}: {}", n + 1, bins.len(), path.display()),
                });
                Ok(())
            },
            &mut || cancel.check(),
        )
    };
    let mut read = read_source(cb)?;
    // Direct native links need the same source-stability protection as new-media inputs.
    // Capture content revisions, then re-interpret before accepting those revisions.
    let mut native_paths = read
        .graph
        .media
        .iter()
        .filter(|m| m.original.is_none())
        .flat_map(|m| m.locations.iter().map(PathBuf::from))
        .collect::<Vec<_>>();
    native_paths.sort();
    native_paths.dedup();
    progress(
        cb,
        "source verification",
        "Hashing referenced native media in place; no footage is copied",
    );
    let mut media_inputs = source_input::pin_checked(&native_paths, &mut || cancel.check())?;
    if !media_inputs.is_empty() {
        cancel.check()?;
        let again = read_source(cb)?;
        if again.graph != read.graph {
            return Err(
                "Source interpretation changed while native media was being verified".into(),
            );
        }
        source_input::recheck_checked(&media_inputs, &mut || cancel.check())?;
        read = again;
    }
    media_inputs.extend(read.original_inputs.iter().cloned());
    cancel.check()?;
    source_input::recheck_checked(&inputs, &mut || cancel.check())?;
    read.graph.encoded_len().map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    for text in read
        .diagnostics
        .iter()
        .chain(&read.scan_issues)
        .chain(&read.audio_group_issues)
    {
        notes.push(note(
            "warning",
            "source",
            text,
            "Review the affected object before using the result.",
        ));
    }
    // An unresolved master must not vanish from a successful wizard conversion.
    if read
        .graph
        .losses
        .iter()
        .any(|l| l.aspect == "source_master")
    {
        return Err("Selected content contains unresolved masters; exclude those bins or resolve their media before conversion".into());
    }
    if !read.scan_issues.is_empty() || !read.audio_group_issues.is_empty() {
        return Err(format!(
            "Media scan or channel routing needs resolution: {}",
            read.scan_issues
                .iter()
                .chain(&read.audio_group_issues)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    for l in read
        .graph
        .losses
        .iter()
        .chain(policy.losses_source(&read.graph).iter())
    {
        notes.push(note("loss", &l.object, &l.reason, &l.aspect));
    }
    let native = spec.mode == ClipMode::RewrapNative;
    let prepared = premiere_adapter::prepare_originals(
        &read.graph,
        &policy.to_options(),
        if native {
            premiere_adapter::ClockProfile::VerifiedRewrap
        } else {
            premiere_adapter::ClockProfile::LegacyImport
        },
    )
    .map_err(|e| format!("Premiere preparation failed: {e}"))?;
    for d in prepared.diagnostics {
        notes.push(note(
            "info",
            "preparation",
            d,
            "Qualified source facts are retained separately from destination behavior.",
        ));
    }
    for l in prepared.losses {
        notes.push(note("loss", l.object, l.reason, l.aspect));
    }
    drop(prepared.ownership);
    let input = prepared.input;
    let mut originals = Vec::new();
    let mut seen = HashSet::new();
    let mut native_count = 0;
    for clip in &input.clips {
        match clip {
            PreparedClip::Original(c) => {
                if seen.insert(c.path.clone()) {
                    originals.push(original_stamp(Path::new(&c.path))?);
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
    inv.ume_originals = paths(Some(true), false);
    inv.total_media = paths(None, false);
    inv.found_media = paths(None, true);
    notes.push(note("info","source verification","Referenced media content is hashed in place during pre-flight and rechecked before publication.","This can take time for large sources; it prevents using stale pre-flight facts after a file changes."));
    inv.notes
        .retain(|n| n != super::inventory::PRESENCE_PENDING);
    inv.notes.push(
        "Pre-flight resolved referenced source media; application acceptance is a separate check."
            .into(),
    );
    if !spec.original_roots.is_empty() {
        notes.push(note("info","original roots","Original roots are hints for your saved-prefix mappings; files are selected only by exact saved paths or explicit mappings.","No basename search or automatic original-file substitution is performed."));
    }
    progress(
        cb,
        "preparation",
        "Preparing the destination document and any new-media plan",
    );
    cancel.check()?;
    let media = if native && native_count > 0 {
        let dir = spec
            .new_media
            .as_deref()
            .ok_or("Native OP-Atom clips need a NEW media output folder. Creates new media.")?;
        let model = input
            .build_native_media_model()
            .map_err(|e| e.to_string())?;
        prproj::convert::native_av::preflight(&model).map_err(|e| e.to_string())?;
        let roots = spec
            .media_roots
            .iter()
            .chain(&spec.original_roots)
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let plan = rewrap::Plan::prepare_media(
            &model,
            dir,
            &roots,
            &spec.output,
            None,
            &tools.ffmpeg,
            &tools.ffprobe,
            true,
        )?;
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
        return Err("A new-media folder applies only to the rewrap mode".into());
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
        let root = std::fs::canonicalize(folder).map_err(|e| e.to_string())?;
        if input.bins.len() != bins.len() {
            return Err("Prepared bins do not match the selected source folders".into());
        }
        let parents = bins
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
            .collect::<Result<Vec<_>, _>>()?;
        let seconds = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        Some(prproj::convert::writer::DestinationBinLayout {
            root_name: import_name(seconds)?,
            parents,
        })
    } else {
        None
    };
    let document = match &layout {
        Some(layout) => prproj::convert::writer::prepare_typed_project_with_layout(
            &input,
            &spec.output,
            &mut ids,
            &bindings,
            native,
            layout,
        ),
        None => prproj::convert::writer::prepare_typed_project(
            &input,
            &spec.output,
            &mut ids,
            &bindings,
            native,
        ),
    }
    .map_err(|e| format!("Premiere document preparation failed: {e}"))?;
    document
        .check_reserved_output(&receipt)
        .map_err(|e| e.to_string())?;
    document
        .check_reserved_output(&replay)
        .map_err(|e| e.to_string())?;
    if let Some(p) = &media {
        document
            .check_reserved_output(p.directory())
            .map_err(|e| e.to_string())?;
    }
    for d in &document.report().notes {
        notes.push(note("info", "writer", d, "Writer preparation report."));
    }
    for d in &document.report().unverified {
        notes.push(note(
            "warning",
            "native qualification",
            d,
            "Structural preparation is not application qualification.",
        ));
    }
    cancel.check()?;
    let review = Review {
        inventory: inv,
        estimated_bytes: media.as_ref().map(|p| p.estimated_bytes()),
        available_bytes: media.as_ref().map(|p| p.available_bytes()),
        notes,
        cli_equivalent: cli_command(&spec)?,
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
        receipt,
        replay,
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
fn recheck_originals(originals: &[OriginalStamp]) -> Result<(), String> {
    for old in originals {
        let now = original_stamp(&old.path)?;
        if old.len != now.len || old.modified != now.modified {
            return Err(format!(
                "Original media changed after pre-flight: {}",
                old.path.display()
            ));
        }
    }
    Ok(())
}
fn write_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
    owned: &mut Vec<PathBuf>,
) -> Result<(), String> {
    use std::io::Write;
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    owned.push(path.to_owned());
    file.write_all(text.as_bytes())
        .map_err(|e| format!("Partial receipt retained at {}: {e}", path.display()))
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
        job.document.recheck_outputs().map_err(|e| e.to_string())?;
        source_input::recheck_checked(&job.inputs, &mut || cancel.check())?;
        source_input::recheck_checked(&job.media_inputs, &mut || cancel.check())?;
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
        cancel.check()?;
        source_input::recheck_checked(&job.inputs, &mut || cancel.check())?;
        source_input::recheck_checked(&job.media_inputs, &mut || cancel.check())?;
        recheck_originals(&job.originals)?;
        phase = "asset publication";
        progress(cb, "publication", "Publishing prepared assets and project");
        job.document.publish_assets().map_err(|e| e.to_string())?;
        cancel.check()?;
        phase = "project publication";
        let report = job.document.publish_project().map_err(|e| e.to_string())?;
        for d in report.notes {
            job.review.notes.push(note(
                "info",
                "publication",
                d,
                "Successful project publication may retain a staging file after cleanup failure.",
            ));
        }
        if let Some(p) = &job.media {
            phase = "media completion receipt";
            p.record_project_with_mode(
                &job.spec.output,
                "native prepared-media AV master (DNx OP1a / ProRes MOV)",
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
        report: job.review.notes.clone(),
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
            result.status = CompletionStatus::Failed;
            result.message = format!("Project published; replay settings could not be saved: {e}");
        }
        result.retained_paths = retain(&job);
    }
    {
        // The on-disk receipt reports its own intended path; a failed write never becomes success.
        result.receipt = Some(job.receipt.clone());
        if let Err(e) = write_json(&job.receipt, &result, &mut job.aux_outputs) {
            result.receipt = None;
            result.status = CompletionStatus::Failed;
            result.message = if published {
                format!(
                    "Project published; completion receipt failed: {e}. Retained project: {}",
                    job.spec.output.display()
                )
            } else {
                format!(
                    "{}; failure/cancellation receipt could not be written: {e}",
                    result.message
                )
            };
        }
        result.retained_paths = retain(&job);
    }
    progress(cb, "finished", &result.message);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
                notes: vec![],
                cli_equivalent: String::new(),
            },
            document,
            media: None,
            media_created: false,
            inputs: vec![],
            media_inputs: vec![],
            originals: vec![],
            aux_outputs: vec![],
        }
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
        let done = execute(job, &CancelToken::default(), &mut |_| {});
        assert_eq!(done.status, CompletionStatus::Failed);
        assert!(done.message.contains("changed"));
        assert!(!t.0.join("new.prproj").exists());
        assert_eq!(std::fs::read(media).unwrap(), b"other");
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
        let error = prepare(spec, &CancelToken::default(), &mut |p| {
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
        assert!(error.contains("Bin inventory changed"));
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
    }
}
