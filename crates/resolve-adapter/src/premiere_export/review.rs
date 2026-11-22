//! Project-wide dry run of the same per-timeline writer used for publication.
use super::{originals::OriginalFailure, prepare_timeline, Originals};
use crate::Result;
use graph::source::SourceGraph;
use serde::Serialize;
use std::{io::Write, path::Path};

#[derive(Default, Serialize)]
struct Totals {
    ready: usize,
    ready_with_losses: usize,
    blocked: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ready,
    ReadyWithLosses,
    Blocked,
}

#[derive(Serialize)]
struct Timeline<'a> {
    id: &'a str,
    name: &'a str,
    placements: usize,
    status: Status,
    losses: Vec<Loss>,
    blocker: Option<Blocker>,
}

#[derive(Serialize)]
struct Loss {
    object: String,
    aspect: String,
    reason: String,
}

#[derive(Serialize)]
struct Blocker {
    stage: &'static str,
    diagnostic: String,
    original: Option<String>,
    owner: Option<String>,
}

/// Dry-run each timeline independently. Admit reportable losses so the
/// report distinguishes an approval requirement from a hard refusal.
pub fn write(graph: &SourceGraph, ffprobe: &Path, mut report: impl Write) -> Result<()> {
    graph.validate()?;
    let scratch = tempfile::tempdir()?;
    let destination = scratch.path().join("review.prproj");
    let mut originals = Originals::new(ffprobe, true);
    let mut totals = Totals::default();
    write!(
        report,
        "{{\n\"schema\":\"anynle.resolve-premiere-review/1\",\n\"timelines\":[\n"
    )?;
    for (index, sequence) in graph.sequences.iter().enumerate() {
        let (status, losses, blocker) =
            match prepare_timeline(graph.clone(), index, &mut originals, &destination, true) {
                Ok((_, losses)) if losses.is_empty() => {
                    totals.ready += 1;
                    (Status::Ready, losses, None)
                }
                Ok((_, losses)) => {
                    totals.ready_with_losses += 1;
                    (Status::ReadyWithLosses, losses, None)
                }
                Err(error) => {
                    totals.blocked += 1;
                    let media = error.downcast_ref::<OriginalFailure>();
                    let blocker = Blocker {
                        stage: if media.is_some() { "media" } else { "timeline" },
                        diagnostic: error.to_string(),
                        original: media.map(|failure| failure.locator.clone()),
                        owner: media.map(|failure| failure.owner.clone()),
                    };
                    (Status::Blocked, Vec::new(), Some(blocker))
                }
            };
        if index > 0 {
            report.write_all(b",\n")?;
        }
        serde_json::to_writer(
            &mut report,
            &Timeline {
                id: &sequence.id,
                name: &sequence.name,
                placements: sequence.placements.len(),
                status,
                losses: losses
                    .into_iter()
                    .map(|loss| Loss {
                        object: loss.object,
                        aspect: loss.aspect,
                        reason: loss.reason,
                    })
                    .collect(),
                blocker,
            },
        )?;
    }
    report.write_all(b"\n],\n\"totals\":")?;
    serde_json::to_writer(&mut report, &totals)?;
    report.write_all(b",\n\"originals\":[\n")?;
    for (index, original) in originals.reviews().enumerate() {
        if index > 0 {
            report.write_all(b",\n")?;
        }
        serde_json::to_writer(&mut report, &original)?;
    }
    report.write_all(b"\n]\n}\n")?;
    Ok(())
}
