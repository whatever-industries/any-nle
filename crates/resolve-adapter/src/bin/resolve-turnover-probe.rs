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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PremiereProjectManifest {
    premiere: PathBuf,
    sequences: Vec<String>,
    templates: Templates,
    ffprobe: PathBuf,
    output: PathBuf,
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
                prefer_source_media: false,
                require_camera_originals: false,
                attach_avid_proxies: false,
                proxy_rewrap_dir: None,
                ffmpeg: None,
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
    const USAGE: &str = "usage: resolve-turnover-probe JOB.json [--review | --graph | --accept-losses]\n       resolve-turnover-probe --premiere-project MANIFEST.json [--review | --graph | --accept-losses]\n       resolve-turnover-probe --list-premiere-sequences INPUT.prproj\n       resolve-turnover-probe --capture-premiere INPUT.prproj SEQUENCE_ID FFPROBE";
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
    if job == "--capture-premiere" {
        let [input, selector, ffprobe] = flags else {
            return Err(USAGE.into());
        };
        let selector = selector.to_str().ok_or("invalid Premiere sequence selector")?;
        let graph = anynle_resolve_adapter::read_premiere_media_graph(
            Path::new(input), selector, Path::new(ffprobe)
        )?;
        graph.write_json(&mut std::io::stdout().lock())?;
        return Ok(());
    }
    let premiere_project = job == "--premiere-project";
    let (file, flags) = if premiere_project {
        flags.split_first().ok_or(USAGE)?
    } else {
        (job, flags)
    };
    let (review, graph, accept) = match flags {
        [] => (false, false, false),
        [flag] if flag == "--review" => (true, false, false),
        [flag] if flag == "--graph" => (false, true, false),
        [flag] if flag == "--accept-losses" => (false, false, true),
        _ => return Err("invalid turnover probe options".into()),
    };
    let input = support::read(Path::new(file), 2 << 20)?;
    let (result, templates, output) = if premiere_project {
        let manifest: PremiereProjectManifest = serde_json::from_str(&input)?;
        let result = anynle_resolve_adapter::prepare_premiere_project(
            &manifest.premiere, &manifest.sequences, &manifest.ffprobe,
        )?;
        (result, manifest.templates, manifest.output)
    } else {
        let job = match serde_json::from_str(&input)? {
            JobDocument::Versioned(job) => job,
            JobDocument::Legacy(job) => TurnoverJob::try_from(job)?,
        };
        (job.prepare()?, job.templates, job.output)
    };
    if std::env::var_os("ANYNLE_RESOLVE_IMPORT_TRACE").is_some() {
        let (direct, assisted) = result.media_counts();
        eprintln!("Media originals: {direct} standalone, {assisted} assisted");
    }
    for notice in &result.notices {
        eprintln!("{notice}");
    }
    for marker in result.omitted_markers() {
        eprintln!("omitted marker occurrence {marker}");
    }
    for loss in result.reviewable_losses() {
        eprintln!("{} ({}): {}", loss.object, loss.aspect, loss.reason);
    }
    if graph {
        result
            .source_graph()
            .write_json(&mut std::io::stdout().lock())?;
    } else if review {
        let review = result.review(&templates, &output)?;
        println!("Resolve review complete. Link to originals.");
        if !review.new_proxy_paths.is_empty() {
            println!("Creates new media.");
            println!("Estimated proxy storage: {} bytes", review.estimated_proxy_bytes);
            for path in review.new_proxy_paths {
                println!("Proxy output: {}", path.display());
            }
        }
    } else {
        let published = result.publish(&templates, &output, accept)?;
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
