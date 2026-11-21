//! Prepare the linked edit's picture timing after transition trims.
use super::{Media, MovieVideo, Profile, SidecarLookup};
use crate::{
    Result,
    mixed_rate::Cadence,
    retime::{self, LinearMap, PictureSpeed},
};
use graph::{Loss, Placement, Rational, source::SourceGraph};

pub(crate) enum Timing {
    Straight,
    Constant(PictureSpeed),
    Hold,
    MixedRate(Cadence),
    Linear(LinearMap),
}

pub(super) struct TimingContext<'a> {
    pub media: &'a Media,
    pub rate: Rational,
    pub canvas: (u16, u16),
    pub lane: usize,
    pub picture_only: bool,
    pub raster_mismatch: bool,
    pub transition_trimmed: bool,
    pub mixed_rate: Option<Cadence>,
    pub can_review_losses: bool,
}

pub(super) struct PreparedTiming {
    pub timing: Timing,
    pub source_in: i64,
    pub source_end: i64,
    pub loss: Option<Loss>,
}

impl TimingContext<'_> {
    pub(super) fn prepare(
        self,
        sidecars: SidecarLookup<'_>,
        edit: &Placement,
        mut source_in: i64,
        length: i64,
    ) -> Result<PreparedTiming> {
        let speed = PictureSpeed::prepare(edit, self.rate, length)
            .map_err(|error| format!("{}: {error}", edit.id))?;
        let profile = &self.media.profile;
        let plain_picture = self.picture_only
            && self.lane == 0
            && self.rate == Rational::integer(24)
            && self.canvas == (1920, 1080)
            && speed.is_none()
            && edit.transform.is_none()
            && edit.effects.is_empty();
        let mut loss = None;
        let timing = if let Some(saved) = sidecars.get(retime::LINEAR_ADAPTER, &edit.id) {
            let Profile::Mov {
                video: MovieVideo::ProResHq,
                frames,
                aac: false,
                pcm: None,
                avid_source_timecode: None,
                ..
            } = profile
            else {
                return Err(format!(
                    "{}: Linear retime requires a linked ProRes HQ picture original",
                    edit.id
                )
                .into());
            };
            if !plain_picture
                || self.mixed_rate.is_some()
                || profile.movie_timecode().is_some()
                || self.media.native.is_some()
                || self.raster_mismatch
                || self.transition_trimmed
            {
                return Err(format!(
                    "{}: Linear retime requires a plain same-rate 1080p24 V1 picture edit",
                    edit.id
                )
                .into());
            }
            let map = LinearMap::from_receipt(saved, *frames)
                .map_err(|error| format!("{}: {error}", edit.id))?;
            let samples = map
                .samples(length)
                .map_err(|error| format!("{}: {error}", edit.id))?;
            if samples[0] != source_in || edit.frame_map.as_deref() != Some(samples.as_slice()) {
                return Err(format!(
                    "{}: Linear retime contradicts the graph/2 source trim or sampled frames",
                    edit.id
                )
                .into());
            }
            Timing::Linear(map)
        } else if let Some(cadence) = self.mixed_rate {
            if self.lane != 0
                || speed.is_some()
                || edit.transform.is_some()
                || !edit.effects.is_empty()
                || !edit
                    .frame_map
                    .as_deref()
                    .is_some_and(|map| cadence.agrees(map, source_in, length))
            {
                return Err(format!("{}: mixed-rate Resolve picture requires a verified Frame Sampling map and plain V1 trim", edit.id).into());
            }
            Timing::MixedRate(cadence)
        } else if let Some(map) = &edit.frame_map {
            let frame = map.first().copied().ok_or("empty Resolve frame hold")?;
            if !plain_picture
                || profile.movie_timecode().is_some()
                || !matches!(
                    profile,
                    Profile::Mov {
                        video: MovieVideo::ProResHq,
                        aac: false,
                        pcm: None,
                        avid_source_timecode: None,
                        ..
                    }
                )
                || retime::hold_count(length) != Some(map.len())
                || map.iter().any(|&sample| sample != frame)
            {
                return Err(format!("{}: Resolve frame hold requires a constant map on linked picture-only 1080p24 ProRes HQ", edit.id).into());
            }
            if frame != source_in {
                if !self.can_review_losses {
                    return Err(format!("{}: a hold away from source In requires the Resolve turnover job with loss review", edit.id).into());
                }
                loss = Some(Loss {
                    object: edit.id.clone(),
                    aspect: "frame_hold_source_in".into(),
                    reason: format!(
                        "Resolve retains held source frame {frame} and uses it as the source In. The original source In at frame {source_in} is retained in graph/2."
                    ),
                });
                // Native Resolve exposes the held picture as the edit's source In.
                source_in = frame;
            }
            Timing::Hold
        } else if let Some(speed) = speed {
            if !matches!(
                profile,
                Profile::Mov {
                    video: MovieVideo::ProResHq,
                    ..
                }
            ) {
                if speed == PictureSpeed::Double {
                    return Err(format!(
                        "{}: 200% Resolve picture speed requires linked ProRes HQ MOV",
                        edit.id
                    )
                    .into());
                }
                if speed == PictureSpeed::Half && length % 2 != 0 {
                    return Err(format!(
                        "{}: odd-length Resolve half-speed picture requires linked ProRes HQ MOV",
                        edit.id
                    )
                    .into());
                }
            }
            Timing::Constant(speed)
        } else {
            Timing::Straight
        };
        let source_end = timing.source_end(edit, source_in, length)?;
        Ok(PreparedTiming {
            timing,
            source_in,
            source_end,
            loss,
        })
    }
}

impl Timing {
    fn source_end(&self, edit: &Placement, source_in: i64, length: i64) -> Result<i64> {
        let end = match self {
            Self::Straight => source_in.checked_add(length),
            Self::Constant(speed) => source_in.checked_add(speed.sampled_source_frames(length)?),
            Self::Hold => source_in.checked_add(1),
            Self::MixedRate(cadence) => cadence
                .last_frame(source_in, length)
                .and_then(|last| last.checked_add(1)),
            Self::Linear(_) => edit
                .frame_map
                .as_ref()
                .and_then(|samples| samples.last())
                .ok_or("missing Linear retime samples")?
                .checked_add(1),
        };
        end.ok_or_else(|| "source range overflow".into())
    }

    pub(crate) fn matches_readback(
        &self,
        graph: &SourceGraph,
        edit: &Placement,
        source_in: i64,
        length: i64,
    ) -> Result<bool> {
        let matches_map = match self {
            Self::Straight | Self::Constant(_) => edit.frame_map.is_none(),
            Self::Hold => edit.frame_map.as_ref().is_some_and(|map| {
                usize::try_from(length).ok() == Some(map.len())
                    && map.iter().all(|&frame| frame == source_in)
            }),
            Self::MixedRate(cadence) => edit
                .frame_map
                .as_ref()
                .is_some_and(|map| cadence.agrees(map, source_in, length)),
            Self::Linear(linear) => {
                let receipt = linear.receipt(&edit.id);
                graph.sidecar(&receipt.adapter, &edit.id) == Some(&receipt.data)
                    && edit.frame_map.as_ref() == Some(&linear.samples(length)?)
            }
        };
        let speed = match self {
            Self::Constant(speed) => speed.graph(length),
            _ => None,
        };
        Ok(matches_map && edit.speed == speed)
    }
}
