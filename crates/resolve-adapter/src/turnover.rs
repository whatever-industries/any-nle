//! Shared internal preparation and publication for link-in-place turnovers.
use crate::{
    archive::{publish_turnover_checked, review_turnover},
    linked::{self, PreparedProject},
    media::media_digest,
    Result,
};
use graph::source::SourceGraph;
use serde::Deserialize;
use std::{collections::HashSet, path::{Path, PathBuf}};

pub use crate::archive::check_output;

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
        #[serde(default)]
        prefer_source_media: bool,
        #[serde(default)]
        require_camera_originals: bool,
        #[serde(default)]
        attach_avid_proxies: bool,
        proxy_rewrap_dir: Option<PathBuf>,
        ffmpeg: Option<PathBuf>,
    },
    PremiereToResolve {
        premiere: PathBuf,
        sequence: String,
    },
}

impl TurnoverJob {
    pub fn review_media_links(&self) -> Result<crate::premiere::MediaLinkReview> {
        let TurnoverSource::PremiereToResolve { premiere, sequence } = &self.source else {
            return Err("Media linking Review requires a Premiere to Resolve job".into());
        };
        check_output(&self.output)?;
        crate::premiere::review_media_links(premiere, sequence, &self.ffprobe)
    }

    pub fn prepare_with_media_progress(
        &self, progress: &mut dyn FnMut(usize, &Path),
    ) -> Result<Turnover> {
        let TurnoverSource::PremiereToResolve { premiere, sequence } = &self.source else {
            return Err("Media linking Review requires a Premiere to Resolve job".into());
        };
        check_output(&self.output)?;
        crate::premiere::prepare_with_media_progress(premiere, sequence, &self.ffprobe, progress)
    }

    pub fn prepare(&self) -> Result<Turnover> {
        use avid_adapter::original::{OriginalOptions, RootMapping};
        match &self.source {
            TurnoverSource::AvidToResolve {
                bins,
                sequences,
                avid_project_root,
                media_roots,
                mappings,
                prefer_source_media,
                require_camera_originals,
                attach_avid_proxies,
                proxy_rewrap_dir,
                ffmpeg,
            } => {
                if proxy_rewrap_dir.is_some() && !attach_avid_proxies {
                    return Err("proxy_rewrap_dir requires attach_avid_proxies; creates new media".into());
                }
                let options = OriginalOptions {
                    link_managed_op1a: false,
                    link_dry_gain: false,
                    ffprobe: self.ffprobe.clone(),
                    prefer_source_media: *prefer_source_media
                        || *require_camera_originals
                        || *attach_avid_proxies,
                    attach_avid_proxies: *attach_avid_proxies,
                    allow_media_gaps: false,
                    collect_blocked_locators: false,
                    proxy_rewrap_dir: proxy_rewrap_dir.clone(),
                    ffmpeg: ffmpeg.clone().unwrap_or_else(|| "ffmpeg".into()),
                    root_mappings: mappings
                        .iter()
                        .map(|[saved_root, current_root]| RootMapping {
                            saved_root: saved_root.clone(),
                            current_root: current_root.clone(),
                        })
                        .collect(),
                };
                crate::avid::prepare_avid_turnover_with_policy(
                    bins,
                    media_roots,
                    sequences,
                    &options,
                    avid_project_root.as_deref(),
                    *require_camera_originals,
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
    pub(crate) proxies: Vec<avid_adapter::proxy::Binding>,
}

pub struct TurnoverReview {
    pub new_proxy_paths: Vec<PathBuf>,
    pub estimated_proxy_bytes: u64,
}

impl Turnover {
    pub fn media_counts(&self) -> (usize, usize) {
        self.prepared.media_counts()
    }

    pub(crate) fn prepare(
        graph: SourceGraph,
        spec: linked::ProjectSpec,
        ffprobe: &Path,
        inputs: Vec<(PathBuf, [u8; 32])>,
        notices: Vec<String>,
        caption_omissions: Vec<graph::Loss>,
        premiere_files: Option<&linked::FileProbes>,
    ) -> Result<Self> {
        let selected: HashSet<_> = spec.timelines.iter().map(|timeline| timeline.id.as_str()).collect();
        let selected_texts = graph.sequences.iter()
            .filter(|sequence| selected.contains(sequence.id.as_str()))
            .flat_map(|sequence| &sequence.texts)
            .map(|text| text.id.clone()).collect();
        let prepared = linked::prepare_project(&graph, spec, ffprobe, Some(&caption_omissions), premiere_files)?;
        let caption_omissions = caption_omissions.into_iter()
            .filter(|loss| !prepared.writes_caption(&loss.object)).collect();
        Ok(Self {
            graph,
            notices,
            prepared,
            inputs,
            caption_omissions,
            selected_texts,
            proxies: vec![],
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
                && !self.prepared.recovers_premiere_mark(loss)
                && (loss.aspect != "text_appearance" || self.selected_texts.contains(&loss.object)))
            .chain(&self.caption_omissions)
            .chain(&self.prepared.destination_losses)
    }

    /// Check the destination plan without staging or publishing any output.
    pub fn review(&self, templates: &Templates, output: &Path) -> Result<TurnoverReview> {
        let estimated_proxy_bytes = avid_adapter::proxy::review(&self.proxies)?;
        review_turnover(&self.prepared, templates, output)?;
        let mut new_proxy_paths: Vec<_> = self.proxies.iter()
            .filter(|binding| binding.audio.is_some())
            .map(|binding| binding.path.clone())
            .collect();
        new_proxy_paths.sort_unstable();
        new_proxy_paths.dedup();
        Ok(TurnoverReview { new_proxy_paths, estimated_proxy_bytes })
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
                "review all reported losses and explicitly accept losses before publication"
                    .into(),
            );
        }
        if self.proxies.iter().any(|p| p.audio.is_some()) {
            self.review(templates, output)?;
        }
        publish_turnover_checked(&self.prepared, templates, output, || {
            let verify = || -> Result<()> {
                for (path, revision) in &self.inputs {
                    if media_digest(path)? != *revision {
                        return Err(format!("source project changed: {}", path.display()).into());
                    }
                }
                Ok(())
            };
            verify()?;
            avid_adapter::proxy::execute_all(&self.proxies)?;
            if self.proxies.iter().any(|p|p.audio.is_some()) {
                verify().map_err(|e|format!("{e}; verified proxy files retained"))?;
            }
            Ok(())
        })
    }
}

pub(crate) const PREMIERE_CAPTION_LOSS: &str = "premiere.caption";

pub(crate) fn is_reviewable_loss(loss: &graph::Loss) -> bool {
    matches!(
        loss.aspect.as_str(),
        "premiere.appearance" | "premiere.marker" | "text_appearance"
            | crate::premiere::CLIP_METADATA_LOSS | crate::premiere::CLIP_LABEL_LOSS
            | crate::premiere::TIMELINE_MARK_LOSS
            | crate::premiere::SYNC_LOCK_LOSS
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn review_and_prepare_check_destination_before_opening_source() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("Occupied.drp");
        std::fs::create_dir(&output).unwrap();
        let job: TurnoverJob = serde_json::from_value(json!({
            "schema": "anynle.job/2",
            "source": {"route": "premiere_to_resolve", "premiere": root.path().join("absent.prproj"), "sequence": "x"},
            "templates": {"layout": "layout.drp", "profile": "profile.drp"},
            "ffprobe": "ffprobe",
            "output": output,
        })).unwrap();
        assert_eq!(
            job.review_media_links().unwrap_err().to_string(),
            "project output already exists"
        );
        let error = job.prepare_with_media_progress(&mut |_, _| panic!("unexpected media probe"))
            .err().expect("occupied output must refuse prepare");
        assert_eq!(error.to_string(), "project output already exists");
        assert!(job.output.is_dir());
    }

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
