//! macOS Avid-to-Resolve job orchestration shared by the wizard and CLI.
use crate::{
    job::{self, CancelToken, JobSpec},
    rewrap,
};
use anynle_resolve_adapter::turnover::{
    ManagedRewrap, Templates, Turnover, TurnoverJob, TurnoverSource,
};
use avid_reader::source::{extract_source_paths, SourceInput, SourceLimits};
use prproj::convert::model::{self, AudioFile, Bin, Clip, Fraction, Model, VideoFile};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_JOB_BYTES: u64 = 1 << 20;
type Result<T> = std::result::Result<T, String>;

pub struct SequenceRow {
    pub id: String,
    pub name: String,
    pub bin: PathBuf,
}

pub fn discover(bins: &[PathBuf], root: Option<&Path>) -> Result<(Vec<PathBuf>, Vec<SequenceRow>)> {
    let request = JobSpec {
        bins: bins.to_vec(),
        project_folder: root.map(Path::to_owned),
        entire_project: root.is_some() && bins.is_empty(),
        ..Default::default()
    };
    let inventory = job::inventory(&request, &CancelToken::default(), &mut |_| {})?;
    let bins = job::selected_paths(&request, &inventory)?;
    if !(1..=256).contains(&bins.len()) {
        return Err("Select between 1 and 256 Avid bins.".into());
    }
    let inputs = bins
        .iter()
        .map(|path| {
            let (_, revision) = inventory
                .bin_revisions
                .iter()
                .find(|(p, _)| p == path)
                .ok_or("missing bin revision")?;
            Ok::<_, String>(SourceInput {
                path: path.clone(),
                revision: revision.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let facts = extract_source_paths(&inputs, SourceLimits::default()).map_err(message)?;
    let rows = facts
        .sequences
        .into_iter()
        .map(|row| {
            Ok::<_, String>(SequenceRow {
                id: row.id,
                name: row.info.name,
                bin: bins
                    .get(row.info.bin)
                    .ok_or("missing timeline bin")?
                    .clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((bins, rows))
}

fn rec(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_JOB_BYTES + 1).read_to_end(&mut bytes))
        .map_err(message)?;
    if bytes.len() as u64 > MAX_JOB_BYTES {
        return Err("Resolve job settings exceed 1 MiB.".into());
    }
    Ok(bytes)
}

/// Local development uses a private manifest. Packaging has no implicit donor.
pub fn local_templates() -> Result<Option<Templates>> {
    let Some(path) = std::env::var_os("ANYNLE_RESOLVE_TEMPLATES").map(PathBuf::from) else {
        return Ok(None);
    };
    let path = fs::canonicalize(path).map_err(message)?;
    let mut templates: Templates = serde_json::from_slice(&rec(&path)?).map_err(message)?;
    let base = path
        .parent()
        .ok_or("Resolve template manifest lacks a parent directory")?;
    for donor in std::iter::once(&mut templates.layout)
        .chain(std::iter::once(&mut templates.profile))
        .chain(templates.atom.iter_mut())
        .chain(templates.group.iter_mut())
        .chain(templates.fairlight.iter_mut())
        .chain(templates.nested.iter_mut())
    {
        *donor = fs::canonicalize(base.join(&*donor)).map_err(message)?;
    }
    Ok(Some(templates))
}

pub fn load_if_avid_resolve(path: &Path) -> Result<Option<TurnoverJob>> {
    let json: serde_json::Value = serde_json::from_slice(&rec(path)?).map_err(message)?;
    if json["schema"] != "anynle.job/2" || json["source"]["route"] != "avid_to_resolve" {
        return Ok(None);
    }
    serde_json::from_value(json).map(Some).map_err(message)
}

pub(crate) fn run_cli(mut job: TurnoverJob, flags: &[String]) -> Result<()> {
    let mut accept = false;
    let mut flags = flags.iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--accept-losses" => accept = true,
            "--output" => {
                job.output = PathBuf::from(flags.next().ok_or("Missing .drp destination")?)
            }
            "--new-media" => {
                let TurnoverSource::AvidToResolve {
                    op1a_rewrap_dir, ..
                } = &mut job.source
                else {
                    return Err("Expected an Avid to Resolve job".into());
                };
                *op1a_rewrap_dir = Some(PathBuf::from(
                    flags.next().ok_or("Missing OP1a output folder")?,
                ));
            }
            _ => return Err(format!("Unknown Avid to Resolve job option {flag}")),
        }
    }
    let source = job.plan_avid().map_err(message)?;
    let output = job.output.clone();
    let prepared = Prepared::prepare(job, source)?;
    for loss in prepared.turnover().reviewable_losses() {
        eprintln!("{} · {}", loss.object, loss.reason);
    }
    let cleanup_warning = prepared.publish(accept)?;
    println!(
        "{}",
        serde_json::json!({"output": output, "cleanup_warning": cleanup_warning})
    );
    Ok(())
}

pub(crate) fn review_cli(job: TurnoverJob) -> (serde_json::Value, bool) {
    use crate::job::{BlockerKind, ReviewBlocker};
    use serde_json::json;

    let mut report = json!({
        "schema": job::PREFLIGHT_REPORT_SCHEMA,
        "status": "blocked",
        "project_output": job.output,
    });
    if let Err(error) = anynle_resolve_adapter::check_output(&job.output) {
        report["blocker"] = json!(ReviewBlocker::new(BlockerKind::ProjectOutput, message(error)));
        return (report, false);
    }
    let source = match job.plan_avid() {
        Ok(source) => source,
        Err(error) => {
            report["blocker"] = json!(ReviewBlocker::new(BlockerKind::SourceContent, message(error)));
            return (report, false);
        }
    };
    let rewrap_managed = matches!(&job.source, TurnoverSource::AvidToResolve {
        op1a_rewrap_dir: Some(_), ..
    });
    let media = source.review_links(&job.ffprobe, rewrap_managed);
    if let Some(first) = media.blockers.first() {
        let mut blocker = ReviewBlocker::new(BlockerKind::OriginalMedia, first);
        blocker.affected = media.blockers;
        report["blocker"] = json!(blocker);
        return (report, false);
    }
    report["media_linking"] = json!({
        "direct_files": media.direct,
        "resolve_files": media.assist,
    });
    let rewrap = source.rewrap_sources(rewrap_managed);
    if !rewrap.is_empty() {
        match crate::rewrap_size::calculate(&rewrap) {
            Ok(estimate) => report["rewrap_estimate"] = json!({
                "bytes": estimate.bytes,
                "outputs": estimate.outputs,
            }),
            Err(error) => report["rewrap_estimate_error"] = json!(error),
        }
    }
    if !media.assist.is_empty() {
        report["status"] = json!("requires_editor");
        return (report, false);
    }
    let prepared = Prepared::prepare_with(job, |ffprobe| {
        source.prepare_standalone(ffprobe).map_err(message)
    });
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            report["blocker"] = json!(ReviewBlocker::new(BlockerKind::SourceContent, error));
            return (report, false);
        }
    };
    let link = prepared.turnover();
    let losses = link.reviewable_losses().map(|loss| json!({
        "object": loss.object,
        "aspect": loss.aspect,
        "reason": loss.reason,
    })).collect::<Vec<_>>();
    report["status"] = json!("ready");
    report["approval_required"] = json!(!losses.is_empty() || !link.omitted_markers().is_empty());
    report["losses"] = json!(losses);
    report["notices"] = json!(link.notices);
    report["omitted_markers"] = json!(link.omitted_markers());
    (report, true)
}

pub struct Prepared {
    job: TurnoverJob,
    link: Turnover,
    primary: Option<rewrap::Plan>,
    replacements: HashMap<String, PathBuf>,
}

impl Prepared {
    pub fn turnover(&self) -> &Turnover {
        &self.link
    }

    pub fn prepare(
        job: TurnoverJob,
        source: anynle_resolve_adapter::turnover::AvidTurnoverPlan,
    ) -> Result<Self> {
        Self::prepare_with(job, |ffprobe| source.prepare(ffprobe).map_err(message))
    }

    fn prepare_with(
        job: TurnoverJob,
        prepare_link: impl FnOnce(&Path) -> Result<Turnover>,
    ) -> Result<Self> {
        let TurnoverSource::AvidToResolve {
            op1a_rewrap_dir,
            proxy_rewrap_dir,
            media_roots,
            ffmpeg,
            ..
        } = &job.source
        else {
            return Err("Expected an Avid to Resolve job".into());
        };
        let link = prepare_link(&job.ffprobe)?;
        link.review(&job.templates, &job.output).map_err(message)?;
        let mut replacements = HashMap::new();
        let primary = if let Some(dir) = op1a_rewrap_dir {
            if proxy_rewrap_dir
                .as_ref()
                .is_some_and(|proxy| proxy.starts_with(dir))
            {
                return Err("The proxy folder must be outside the new OP1a folder.".into());
            }
            let rows = link.managed_rewraps();
            if rows.is_empty() {
                None
            } else {
                Turnover::review_op1a_template(&job.templates).map_err(message)?;
                let model = pack(&rows)?;
                let roots = media_roots
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                let ffmpeg = ffmpeg.as_deref().unwrap_or(Path::new("ffmpeg"));
                let plan = rewrap::Plan::prepare(
                    &model,
                    dir,
                    &roots,
                    &job.output,
                    None,
                    ffmpeg,
                    &job.ffprobe,
                )?;
                let outputs: HashMap<_, _> = plan.op1a_targets().collect();
                for row in rows {
                    let paths = Vec::from(row.sources);
                    let target = outputs.get(&paths).ok_or("missing prepared OP1a output")?;
                    replacements.insert(row.item_id, target.clone());
                }
                Some(plan)
            }
        } else {
            None
        };
        Ok(Self {
            job,
            link,
            primary,
            replacements,
        })
    }

    pub fn publish(mut self, accept_losses: bool) -> Result<Option<String>> {
        if self.link.reviewable_losses().next().is_some() && !accept_losses {
            return Err("Review and approve the listed changes for this run.".into());
        }
        self.link.recheck_projects().map_err(message)?;
        self.link
            .review(&self.job.templates, &self.job.output)
            .map_err(message)?;
        if let Some(plan) = &self.primary {
            Turnover::review_op1a_template(&self.job.templates).map_err(message)?;
            let mut created = false;
            let published = plan
                .execute_observed(&mut |_, _| Ok(()), &mut created)
                .and_then(|_| {
                    self.link
                        .use_verified_rewraps(&self.replacements, &self.job.ffprobe)
                        .map_err(message)
                })
                .and_then(|_| {
                    self.link
                        .publish(&self.job.templates, &self.job.output, accept_losses)
                        .map(|result| result.cleanup_warning)
                        .map_err(message)
                });
            return published.map_err(|error| {
                if created {
                    format!(
                        "{error}\nPrepared media retained at {}",
                        plan.directory().display()
                    )
                } else {
                    error
                }
            });
        }
        self.link
            .publish(&self.job.templates, &self.job.output, accept_losses)
            .map(|result| result.cleanup_warning)
            .map_err(message)
    }
}

// The stream-copy backend consumes file recipes. Keep the Resolve edit graph
// independent of this packaging-only model.
fn pack(rows: &[ManagedRewrap]) -> Result<Model> {
    let rate = Fraction::new(24000, 1001);
    let frame = model::ticks_per_unit(rate);
    let mut clips = Vec::new();
    for row in rows {
        let start = row
            .start_frame
            .checked_mul(frame)
            .ok_or("MXF start overflows")?;
        let duration = row
            .frames
            .checked_mul(frame)
            .ok_or("MXF duration overflows")?;
        let paths = row
            .sources
            .each_ref()
            .map(|path| path.to_string_lossy().into_owned());
        clips.push(Clip {
            id: row.item_id.clone(),
            name: row.name.clone(),
            material_umid: None,
            tape_name: None,
            avid: None,
            time_display: None,
            video: Some(VideoFile {
                path: paths[0].clone(),
                codec: "AVdn".into(),
                width: 1920,
                height: 1080,
                rate,
                duration,
                start,
            }),
            audio: paths[1..]
                .iter()
                .enumerate()
                .map(|(i, path)| AudioFile {
                    path: path.clone(),
                    sample_rate: 48000,
                    duration,
                    bits: 24,
                    start,
                    avid_track: Some(i as i64 + 1),
                    gain_db: 0.,
                })
                .collect(),
            stereo_pairs: vec![],
            multichannel_groups: vec![],
            rate: Some(rate),
            markers: vec![],
        });
    }
    Ok(Model {
        project_name: "Resolve media preparation".into(),
        options: Default::default(),
        bins: vec![Bin {
            name: "Managed media".into(),
            clips: clips.iter().map(|c| c.id.clone()).collect(),
        }],
        clips: clips.into_iter().collect(),
        sequences: vec![],
    })
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
