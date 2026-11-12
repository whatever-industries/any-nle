//! Shared internal preparation and publication for link-in-place turnovers.
use crate::{
    archive::{check_output, publish_turnover_checked, review_turnover},
    linked::{self, PreparedProject},
    media::media_digest,
    Result,
};
use graph::source::SourceGraph;
use serde::Deserialize;
use std::{collections::HashSet, path::{Path, PathBuf}};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Templates {
    pub layout: PathBuf,
    pub profile: PathBuf,
    pub atom: Option<PathBuf>,
    pub group: Option<PathBuf>,
    pub fairlight: Option<PathBuf>,
    pub nested: Option<PathBuf>,
}

/// Internal saved-job contract for the two experimental Resolve routes.
/// The public application still reads and writes its existing job/1 format.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnoverJob {
    pub schema: JobSchema,
    pub source: TurnoverSource,
    pub templates: Templates,
    pub ffprobe: PathBuf,
    pub output: PathBuf,
}

#[derive(Deserialize)]
pub enum JobSchema {
    #[serde(rename = "anynle.job/2")]
    V2,
}

#[derive(Deserialize)]
#[serde(tag = "route", rename_all = "snake_case", deny_unknown_fields)]
pub enum TurnoverSource {
    AvidToResolve {
        bins: Vec<PathBuf>,
        sequences: Vec<String>,
        avid_project_root: Option<PathBuf>,
        #[serde(default)]
        media_roots: Vec<PathBuf>,
        #[serde(default)]
        mappings: Vec<[PathBuf; 2]>,
    },
    PremiereToResolve {
        premiere: PathBuf,
        sequence: String,
    },
}

impl TurnoverJob {
    pub fn prepare(&self) -> Result<Turnover> {
        use avid_adapter::original::{OriginalOptions, RootMapping};
        match &self.source {
            TurnoverSource::AvidToResolve {
                bins,
                sequences,
                avid_project_root,
                media_roots,
                mappings,
            } => {
                let options = OriginalOptions {
                    ffprobe: self.ffprobe.clone(),
                    root_mappings: mappings
                        .iter()
                        .map(|[saved_root, current_root]| RootMapping {
                            saved_root: saved_root.clone(),
                            current_root: current_root.clone(),
                        })
                        .collect(),
                };
                crate::prepare_avid_turnover(
                    bins,
                    media_roots,
                    sequences,
                    &options,
                    avid_project_root.as_deref(),
                )
            }
            TurnoverSource::PremiereToResolve { premiere, sequence } => {
                crate::prepare_premiere_turnover(premiere, sequence, &self.ffprobe)
            }
        }
    }
}

pub struct Turnover {
    graph: SourceGraph,
    pub notices: Vec<String>,
    prepared: PreparedProject,
    inputs: Vec<(PathBuf, [u8; 32])>,
    caption_omissions: Vec<graph::Loss>,
    selected_texts: HashSet<String>,
}

impl Turnover {
    pub(crate) fn prepare(
        graph: SourceGraph,
        spec: linked::ProjectSpec,
        ffprobe: &Path,
        inputs: Vec<(PathBuf, [u8; 32])>,
        notices: Vec<String>,
        caption_omissions: Vec<graph::Loss>,
    ) -> Result<Self> {
        let selected: HashSet<_> = spec.timelines.iter().map(|timeline| timeline.id.as_str()).collect();
        let selected_texts = graph.sequences.iter()
            .filter(|sequence| selected.contains(sequence.id.as_str()))
            .flat_map(|sequence| &sequence.texts)
            .map(|text| text.id.clone()).collect();
        let prepared = linked::prepare_project(&graph, spec, ffprobe, Some(&caption_omissions))?;
        let caption_omissions = caption_omissions.into_iter()
            .filter(|loss| !prepared.writes_caption(&loss.object)).collect();
        Ok(Self {
            graph,
            notices,
            prepared,
            inputs,
            caption_omissions,
            selected_texts,
        })
    }

    pub fn omitted_markers(&self) -> &[String] {
        self.prepared.omitted_markers()
    }

    pub fn source_graph(&self) -> &SourceGraph {
        &self.graph
    }

    pub fn reviewable_losses(&self) -> impl Iterator<Item = &graph::Loss> {
        self.graph
            .losses
            .iter()
            .filter(|loss| is_reviewable_loss(loss)
                && (loss.aspect != "text_appearance" || self.selected_texts.contains(&loss.object)))
            .chain(&self.caption_omissions)
    }

    /// Check the destination plan without staging or publishing any output.
    pub fn review(&self, templates: &Templates, output: &Path) -> Result<()> {
        review_turnover(&self.prepared, templates, output)
    }

    pub fn publish(
        &self,
        templates: &Templates,
        output: &Path,
        accept_losses: bool,
    ) -> Result<crate::Published> {
        check_output(output)?;
        if !accept_losses
            && (!self.omitted_markers().is_empty() || self.reviewable_losses().next().is_some())
        {
            return Err(
                "review all omitted occurrences and explicitly accept losses before publication"
                    .into(),
            );
        }
        publish_turnover_checked(&self.prepared, templates, output, || {
            for (path, revision) in &self.inputs {
                if media_digest(path)? != *revision {
                    return Err(format!("source project changed: {}", path.display()).into());
                }
            }
            Ok(())
        })
    }
}

pub(crate) const PREMIERE_CAPTION_LOSS: &str = "premiere.caption";

pub(crate) fn is_reviewable_loss(loss: &graph::Loss) -> bool {
    matches!(
        loss.aspect.as_str(),
        "premiere.appearance" | "premiere.marker" | "text_appearance" | crate::premiere::CLIP_METADATA_LOSS
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_jobs_require_the_version_route_and_current_run_approval() {
        let premiere = json!({
            "schema": "anynle.job/2",
            "source": {"route": "premiere_to_resolve", "premiere": "in.prproj", "sequence": "x"},
            "templates": {"layout": "layout.drp", "profile": "profile.drp"},
            "ffprobe": "ffprobe",
            "output": "out.drp"
        });
        let parse = |value| serde_json::from_value::<TurnoverJob>(value);
        assert!(parse(premiere.clone()).is_ok());
        let mut avid = premiere.clone();
        avid["source"] = json!({"route":"avid_to_resolve","bins":["in.avb"],"sequences":["x"]});
        assert!(parse(avid.clone()).is_ok());
        avid["source"]["premiere"] = json!("in.prproj");
        assert!(parse(avid).is_err());
        let mut wrong_version = premiere.clone();
        wrong_version["schema"] = json!("anynle.job/1");
        assert!(parse(wrong_version).is_err());
        let mut approval = premiere;
        approval["accept_losses"] = json!(true);
        assert!(parse(approval).is_err());
    }
}
