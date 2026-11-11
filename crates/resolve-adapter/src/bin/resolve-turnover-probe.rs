//! Internal CLI for preparing and publishing a link-in-place turnover.
use anynle_resolve_adapter::turnover::{JobSchema, Templates, TurnoverJob, TurnoverSource};
use serde::Deserialize;
use std::path::{Path, PathBuf};

mod support;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProbeJob {
    #[serde(default)]
    bins: Vec<PathBuf>,
    premiere: Option<PathBuf>,
    avid_project_root: Option<PathBuf>,
    #[serde(default)]
    media_roots: Vec<PathBuf>,
    sequences: Vec<String>,
    templates: Templates,
    ffprobe: PathBuf,
    output: PathBuf,
    #[serde(default)]
    mappings: Vec<[PathBuf; 2]>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum JobDocument {
    Versioned(TurnoverJob),
    Legacy(LegacyProbeJob),
}

impl TryFrom<LegacyProbeJob> for TurnoverJob {
    type Error = &'static str;

    fn try_from(job: LegacyProbeJob) -> Result<Self, Self::Error> {
        let source = match job.premiere {
            Some(premiere) => {
                let [sequence] = job.sequences.as_slice() else {
                    return Err("select exactly one Premiere sequence");
                };
                if !job.bins.is_empty()
                    || !job.media_roots.is_empty()
                    || !job.mappings.is_empty()
                    || job.avid_project_root.is_some()
                {
                    return Err("Avid bin and media options do not apply to Premiere");
                }
                TurnoverSource::PremiereToResolve {
                    premiere,
                    sequence: sequence.clone(),
                }
            }
            None => TurnoverSource::AvidToResolve {
                bins: job.bins,
                sequences: job.sequences,
                avid_project_root: job.avid_project_root,
                media_roots: job.media_roots,
                mappings: job.mappings,
            },
        };
        Ok(Self {
            schema: JobSchema::V2,
            source,
            templates: job.templates,
            ffprobe: job.ffprobe,
            output: job.output,
        })
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    const USAGE: &str = "usage: resolve-turnover-probe JOB.json [--review | --graph | --accept-losses]\n       resolve-turnover-probe --list-premiere-sequences INPUT.prproj";
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let (job, flags) = args.split_first().ok_or(USAGE)?;
    if job == "--list-premiere-sequences" {
        let [path] = flags else {
            return Err(USAGE.into());
        };
        let sequences = anynle_resolve_adapter::list_premiere_sequences(Path::new(path))?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &sequences)?;
        println!();
        return Ok(());
    }
    let (review, graph, accept) = match flags {
        [] => (false, false, false),
        [flag] if flag == "--review" => (true, false, false),
        [flag] if flag == "--graph" => (false, true, false),
        [flag] if flag == "--accept-losses" => (false, false, true),
        _ => return Err("invalid turnover probe options".into()),
    };
    let job = match serde_json::from_str(&support::read(Path::new(job), 2 << 20)?)? {
        JobDocument::Versioned(job) => job,
        JobDocument::Legacy(job) => TurnoverJob::try_from(job)?,
    };
    let result = job.prepare()?;
    for notice in &result.notices {
        eprintln!("{notice}");
    }
    for marker in result.omitted_markers() {
        eprintln!("omitted marker occurrence {marker}");
    }
    for loss in result.creative_losses() {
        eprintln!("{} ({}): {}", loss.object, loss.aspect, loss.reason);
    }
    if graph {
        result
            .source_graph()
            .write_json(&mut std::io::stdout().lock())?;
    } else if review {
        result.review(&job.templates, &job.output)?;
        println!("Resolve review complete. Link to originals.");
    } else {
        let published = result.publish(&job.templates, &job.output, accept)?;
        if let Some(warning) = published.cleanup_warning {
            eprintln!("{warning}");
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Resolve turnover probe: {error}");
        std::process::exit(1);
    }
}
