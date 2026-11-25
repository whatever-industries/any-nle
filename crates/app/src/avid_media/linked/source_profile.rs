//! Qualification of the media and native ownership for each linked Avid profile.
use super::{
    markers,
    prepare::{frames, matches_clock, timeline_clock},
    EditRate, ManagedBinding, Result, Route, RASTER,
};
use graph::{
    source::{SourceGraph, SourceItem, SourceItemMedia, SourceMedia},
    Rational, StreamKind,
};
use std::{collections::HashMap, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Profile {
    Separate,
    AvcAac,
    ManagedAtom,
}

#[derive(Debug)]
pub(super) struct SourceSet<'a> {
    pub(super) kind: SourceKind,
    pub(super) routes: [Route<'a>; 3],
    pub(super) picture_markers: Vec<markers::Marker<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceKind {
    Separate,
    AvcAac {
        split_items: bool,
        srgb_as_rec709: bool,
    },
    ManagedAtom,
}

pub(super) struct QualifiedRoute<'a> {
    pub(super) item: &'a SourceItem,
    pub(super) binding: &'a SourceItemMedia,
    pub(super) route: Route<'a>,
    pub(super) markers: Vec<markers::Marker<'a>>,
}

const AVC_OVERRIDE: &str = "premiere.avc_source_override";

pub(super) struct MediaIndex<'a> {
    pub(super) media: HashMap<&'a str, &'a SourceMedia>,
    pub(super) managed: HashMap<&'a str, &'a graph::Value>,
}

impl<'a> MediaIndex<'a> {
    pub(super) fn new(graph: &'a SourceGraph) -> Self {
        Self {
            media: graph
                .media
                .iter()
                .map(|media| (media.id.as_str(), media))
                .collect(),
            managed: graph
                .sidecars
                .iter()
                .filter(|row| row.adapter == super::MANAGED_ATOM_RECEIPT)
                .map(|row| (row.object.as_str(), &row.data))
                .collect(),
        }
    }
}

pub(super) fn is_avc_picture(media: &SourceMedia) -> bool {
    media.kind == StreamKind::Video
        && media.codec.as_deref() == Some("h264")
        && media
            .original
            .as_ref()
            .is_some_and(|stream| stream.container == "mov")
}

pub(super) fn is_avc_override(row: &graph::Sidecar) -> bool {
    row.adapter == AVC_OVERRIDE
        && matches!(&row.data, graph::Value::Str(value) if value == "srgb_as_rec709")
}

pub(super) fn is_managed_logging(row: &graph::Sidecar) -> bool {
    row.adapter == "premiere.logging"
        && matches!(&row.data, graph::Value::Obj(fields)
        if fields.iter().all(|(key, _)| matches!(key.as_str(), "TapeName" | "Description" | "LogNote")))
}

fn captured_receipt(row: &graph::Sidecar) -> bool {
    use graph::Value;
    match row.adapter.as_str() {
        // The native entry points validate the saved mixers before handing
        // the graph to this destination. Keep the qualifying receipts intact.
        "premiere.source"
        | "premiere.media_capture"
        | "premiere.media_identity"
        | "premiere.audio_groups"
        | "resolve.fairlight"
        | "resolve.active_timeline"
        | "resolve.bin_order" => true,
        "resolve.media_metadata" => matches!(&row.data, Value::Obj(fields)
            if fields.iter().all(|(field, _)| field == "UniqueMediaPoolItemId" || field == "Reel")),
        "resolve.source" => matches!(&row.data, Value::Obj(fields)
            if fields.iter().all(|(field, _)| matches!(field.as_str(),
                "kind" | "cache_frames" | "video_cache_count" | "saved_selector"
                | "saved_cache_track" | "saved_channel" | "cache_samples" | "audio_import_rate"
                | "timeline_handle" | "media_pool_clip" | "bin_id" | "record_origin" | "record_length"))),
        _ => false,
    }
}

impl Profile {
    pub(super) fn identify(graph: &SourceGraph) -> Self {
        let embedded_avc =
            matches!(graph.items.len(), 1 | 2) && graph.media.iter().any(is_avc_picture);
        let managed = !embedded_avc
            && graph
                .sidecars
                .iter()
                .any(|row| row.adapter == super::MANAGED_ATOM_RECEIPT);
        if embedded_avc {
            Self::AvcAac
        } else if managed {
            Self::ManagedAtom
        } else {
            Self::Separate
        }
    }

    pub(super) fn check_owners(self, graph: &SourceGraph, captured: bool) -> Result<()> {
        if self == Self::ManagedAtom {
            // The three Premiere source items become one native managed master.
            // Each item may keep its own Premiere bin in the captured graph.
            for item in &graph.items {
                let owners = graph
                    .bins
                    .iter()
                    .flat_map(|bin| &bin.items)
                    .filter(|id| **id == item.id)
                    .count();
                if owners != 1 {
                    return Err(
                        format!("{}: managed OP-Atom item needs one source bin", item.id).into(),
                    );
                }
            }
        } else {
            let active_bins: Vec<_> = graph.bins.iter().filter(|b| !b.items.is_empty()).collect();
            if !matches!(active_bins.as_slice(), [bin] if (captured || graph.bins.len() == 1 && bin.parent.is_none())
            && graph.items.iter().all(|item| bin.items.contains(&item.id)))
            {
                return Err("linked bin pilot requires both items in the same source bin".into());
            }
        }
        Ok(())
    }

    pub(super) fn check_receipts(self, graph: &SourceGraph, captured: bool) -> Result<usize> {
        let mut overrides = 0;
        for row in &graph.sidecars {
            if self == Self::AvcAac
                && graph.items.iter().any(|item| row.object == item.id)
                && is_avc_override(row)
            {
                overrides += 1;
                continue;
            }
            if !(captured
                && (captured_receipt(row)
                    || self == Self::AvcAac && row.adapter == "premiere.clip_markers")
                || self == Self::ManagedAtom
                    && (captured_receipt(row)
                        || row.adapter == super::MANAGED_ATOM_RECEIPT
                        || is_managed_logging(row)))
            {
                return Err(format!(
                    "{}: unsupported Avid source receipt {}",
                    row.object, row.adapter
                )
                .into());
            }
        }
        Ok(overrides)
    }

    pub(super) fn accepts_counts(self, items: usize, media: usize) -> bool {
        match self {
            Self::Separate => items == 2 && media == 3,
            Self::AvcAac => media == 3 * items,
            Self::ManagedAtom => items == 3 && media == 3,
        }
    }

    pub(super) fn layout_error(self) -> &'static str {
        match self {
            Self::Separate => {
                "linked bin pilot requires a neutral 1080p V1/A1/A2 graph with two originals"
            }
            Self::AvcAac => "linked MP4 requires a neutral 1080p V1/A1/A2 graph",
            Self::ManagedAtom => {
                "linked managed OP-Atom requires a neutral 1080p V1/A1/A2 graph with three siblings"
            }
        }
    }

    pub(super) fn check_rate_and_overrides(
        self,
        rate: EditRate,
        overrides: usize,
        items: usize,
    ) -> Result<()> {
        if !matches!(rate, EditRate::Fps23976) {
            match self {
                Self::AvcAac => {
                    return Err("the native linked AVC/AAC profile requires 23.976 fps".into())
                }
                Self::ManagedAtom => {
                    return Err("native linked managed OP-Atom requires 23.976 fps".into())
                }
                Self::Separate => {}
            }
        }
        if overrides != 0 && overrides != items {
            return Err("linked MP4 source views disagree about their color interpretation".into());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn qualify_route<'a>(
        self,
        index: &MediaIndex<'a>,
        item: &'a SourceItem,
        edit: &graph::Placement,
        kind: StreamKind,
        lane_index: usize,
        rate: EditRate,
        captured: bool,
        picture_item: Option<&str>,
    ) -> Result<QualifiedRoute<'a>> {
        let picture = kind == StreamKind::Video;
        let lane = if picture { 1 } else { lane_index as i64 };
        let members = item
            .media
            .iter()
            .filter_map(|binding| {
                let media = index.media.get(binding.media.as_str()).copied()?;
                (media.kind == kind).then_some((binding, media))
            })
            .collect::<Vec<_>>();
        let selected = if picture {
            matches!(members.as_slice(), [_]).then(|| members[0])
        } else if let [selector] = edit.audio_streams.as_slice() {
            let mut matches = members
                .iter()
                .filter(|(_, media)| media.stream == Some(*selector));
            let first = matches.next();
            first.filter(|_| matches.next().is_none()).copied()
        } else {
            None
        };
        let (binding, media) =
            selected.ok_or_else(|| format!("{}: unresolved stream selector", edit.id))?;
        if (!picture && picture_item != Some(item.id.as_str()) && !item.markers.is_empty())
            || item.color_rgb16.is_some()
            || !(item.stream_groups.is_empty()
                || (self == Self::AvcAac || captured && !picture)
                    && item.stream_groups == [vec![1, 2]]
                || self == Self::AvcAac && item.stream_groups == [vec![1], vec![2]])
            || item.rate.is_some_and(|item_rate| item_rate != rate.exact())
            || binding.gain_db != 0.0
            || item.media.len()
                != if self == Self::AvcAac {
                    3
                } else if picture || self == Self::ManagedAtom {
                    1
                } else {
                    2
                }
        {
            return Err(format!("{}: unsupported item controls or layout", edit.id).into());
        }
        let original = media
            .original
            .as_ref()
            .ok_or("missing physical stream facts")?;
        let ([path], [saved_path]) = (
            media.locations.as_slice(),
            original.saved_locators.as_slice(),
        ) else {
            return Err("linked bin pilot needs one exact original locator per stream".into());
        };
        let managed_receipt = if self == Self::ManagedAtom {
            let receipt = index
                .managed
                .get(item.id.as_str())
                .copied()
                .ok_or_else(|| format!("{}: missing managed OP-Atom receipt", edit.id))?;
            Some(ManagedBinding {
                material: item
                    .umid
                    .as_deref()
                    .ok_or("missing managed material package")?,
                receipt,
                clock: media
                    .clock
                    .as_ref()
                    .ok_or("missing managed OP-Atom clock")?,
            })
        } else {
            None
        };
        let extension = if self == Self::ManagedAtom {
            "mxf"
        } else if self == Self::AvcAac {
            "mp4"
        } else if picture {
            "mov"
        } else {
            "wav"
        };
        let container = if self == Self::AvcAac {
            "mov"
        } else {
            extension
        };
        let stream_index = if self == Self::ManagedAtom {
            lane_index as i64
        } else {
            i64::from(self == Self::AvcAac && !picture)
        };
        if path != saved_path
            || !Path::new(path).is_absolute()
            || !Path::new(path)
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case(extension))
            || media.online == Some(false)
            || original.container != container
            || original.stream_index != stream_index
            || original.start_pts != 0
            || original.duration_ts <= 0
            || original.time_base.num() <= 0
        {
            return Err(format!(
                "{}: original locator or physical stream is unqualified",
                edit.id
            )
            .into());
        }
        let source_lane = if picture {
            if media.frame_size != Some(RASTER)
                || media.rate != Some(rate.exact())
                || original.channel_index.is_some()
                || self == Self::ManagedAtom && media.codec.as_deref() != Some("dnxhd")
            {
                return Err("picture edit needs same-rate 1080p without an audio selector".into());
            }
            1
        } else {
            let representation_matches = if self == Self::AvcAac {
                media.codec.as_deref() == Some("aac")
                    && media.bits.is_none()
                    && original.sample_representation.as_deref() == Some("aac")
                    && original.time_base == Rational::new(1, 48_000).unwrap()
            } else if self == Self::ManagedAtom {
                media.bits == Some(24)
                    && media.codec.as_deref() == Some("pcm_s24le")
                    && original.sample_representation.as_deref() == Some("pcm24")
                    && original.time_base == Rational::new(1, 48_000).unwrap()
                    && original.channel_index == Some(0)
            } else {
                matches!(media.bits, Some(16 | 24))
            };
            if media.sample_rate != Some(48_000)
                || !representation_matches
                || original.channels != Some(if self == Self::ManagedAtom { 1 } else { 2 })
            {
                return Err(if self == Self::ManagedAtom {
                    "audio edit needs the verified mono 48 kHz PCM24 sibling"
                } else if self == Self::AvcAac {
                    "audio edit needs 48 kHz stereo AAC-LC"
                } else {
                    "audio edit needs two-channel 48 kHz PCM16 or PCM24"
                }
                .into());
            }
            if self == Self::ManagedAtom {
                lane
            } else {
                original
                    .channel_index
                    .filter(|n| matches!(n, 0 | 1))
                    .ok_or(if self == Self::AvcAac {
                        "missing physical AAC channel"
                    } else {
                        "missing stereo WAV channel"
                    })?
                    + 1
            }
        };
        let duration = Rational::new(
            i128::from(original.duration_ts) * i128::from(original.time_base.num()),
            i128::from(original.time_base.den()),
        )
        .ok_or("invalid physical duration")?;
        let extent = frames(rate, duration)
            .filter(|n| *n > 0)
            .ok_or("original extent needs whole edit frames")?;
        let markers = if picture {
            markers::prepare(&item.markers, binding.source_offset, extent, rate)?
        } else {
            Vec::new()
        };
        // Premiere's WAV capture leaves display timecode absent. The
        // separate physical check still requires a zero embedded origin.
        let clock = if (captured || self == Self::AvcAac) && !picture && media.start.is_none() {
            0
        } else {
            timeline_clock(rate, media.start).ok_or("missing original timecode")?
        };
        if duration != media.duration || !matches_clock(media.clock.as_ref(), clock, rate) {
            return Err(format!("{}: conflicting source extent or timecode", edit.id).into());
        }
        if !picture && self != Self::ManagedAtom && clock != 0 {
            return Err(
                format!("{}: this WAV profile requires a zero source start", edit.id).into(),
            );
        }
        Ok(QualifiedRoute {
            item,
            binding,
            markers,
            route: Route {
                path,
                name: &item.name,
                tape: item.tape.as_deref(),
                clock,
                extent,
                source_lane,
                bits: media.bits,
                stream: original,
                managed: managed_receipt,
            },
        })
    }

    pub(super) fn finish<'a>(
        self,
        index: &MediaIndex<'a>,
        items: impl ExactSizeIterator<Item = &'a SourceItem>,
        routes: [Route<'a>; 3],
        item_ids: &[&str],
        overrides: usize,
        picture_markers: Vec<markers::Marker<'a>>,
    ) -> Result<SourceSet<'a>> {
        let [video, left, right] = &routes;
        let split_items = self == Self::AvcAac && item_ids[0] != item_ids[1];
        if self == Self::ManagedAtom {
            let identity = |binding: ManagedBinding<'a>| {
                (
                    binding.material,
                    binding.receipt,
                    &binding.clock.provenance.source_package,
                )
            };
            if item_ids[0] == item_ids[1]
                || item_ids[0] == item_ids[2]
                || item_ids[1] == item_ids[2]
                || video.path == left.path
                || video.path == right.path
                || left.path == right.path
                || video.extent != left.extent
                || video.extent != right.extent
                || video.clock != left.clock
                || video.clock != right.clock
                || [video.managed, left.managed, right.managed]
                    .iter()
                    .any(|claim| claim.map(identity) != video.managed.map(identity))
            {
                return Err("managed OP-Atom requires three related, distinct file packages with a common clock and extent".into());
            }
        } else if (self != Self::AvcAac && item_ids[0] == item_ids[1])
            || self == Self::AvcAac && split_items != (items.len() == 2)
            || item_ids[1] != item_ids[2]
            || (video.path == left.path) != (self == Self::AvcAac)
            || left.path != right.path
            || left.clock != right.clock
            || left.extent != right.extent
            || left.source_lane == right.source_lane
            || left.bits != right.bits
            || left.stream.time_base != right.stream.time_base
            || left.stream.duration_ts != right.stream.duration_ts
            || self == Self::AvcAac && (video.extent != left.extent || video.clock == 0)
        {
            return Err(if self == Self::AvcAac {
            "linked MP4 needs one timecoded picture and both distinct AAC channels with matching frame extent"
        } else { "A1 and A2 must select distinct channels of the same verified WAV item" }.into());
        }
        if self == Self::AvcAac {
            let selected = [video, left, right];
            for item in items {
                let mut mask = 0u8;
                for binding in &item.media {
                    let media = index
                        .media
                        .get(binding.media.as_str())
                        .ok_or("missing linked MP4 source view")?;
                    let index = selected
                        .iter()
                        .position(|route| {
                            media.original.as_ref() == Some(route.stream)
                                && media.locations.as_slice() == [route.path]
                        })
                        .ok_or("linked MP4 source views disagree about the physical original")?;
                    if mask & (1 << index) != 0
                        || binding.gain_db != 0.0
                        || media.codec.as_deref() != Some(if index == 0 { "h264" } else { "aac" })
                    {
                        return Err(
                            "linked MP4 source view has duplicate or contradictory stream facts"
                                .into(),
                        );
                    }
                    mask |= 1 << index;
                }
            }
        }
        let kind = match self {
            Self::Separate => SourceKind::Separate,
            Self::AvcAac => SourceKind::AvcAac {
                split_items,
                srgb_as_rec709: overrides != 0,
            },
            Self::ManagedAtom => SourceKind::ManagedAtom,
        };
        Ok(SourceSet {
            kind,
            routes,
            picture_markers,
        })
    }
}

impl SourceSet<'_> {
    pub(super) fn split_avc_items(&self) -> bool {
        matches!(
            self.kind,
            SourceKind::AvcAac {
                split_items: true,
                ..
            }
        )
    }

    pub(super) fn srgb_as_rec709(&self) -> bool {
        matches!(
            self.kind,
            SourceKind::AvcAac {
                srgb_as_rec709: true,
                ..
            }
        )
    }
}
