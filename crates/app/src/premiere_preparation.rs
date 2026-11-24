//! Shared graph and typed document preparation for Avid to Premiere.
use crate::png_mattes;
use avid_reader::source::SourceInput;
use graph::{source::SourceGraph, StreamKind};
use premiere_adapter::{ClockProfile, PreparedPremiereInput};
use prproj::convert::{
    model::Options,
    prepared::PreparedProjectInput,
    rewrap::MediaRelink,
    writer::{self, DestinationBinLayout, PreparedProject, Report, TypedMedia},
};
use prproj::UuidSource;
use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}};

pub(crate) fn clock_profile(rewrap: bool) -> ClockProfile {
    if rewrap {
        ClockProfile::VerifiedRewrap
    } else {
        ClockProfile::LegacyImport
    }
}

/// Identify managed picture profiles carried by the explicit rewrap paths.
pub(crate) fn managed_rewrap_items(graph: &SourceGraph, op1a_requested: bool) -> Vec<String> {
    managed_rewrap_candidates(graph, op1a_requested).into_iter().map(|clip| clip.detail).collect()
}

pub(crate) struct ManagedRewrapCandidate<'a> {
    pub item: &'a str,
    pub detail: String,
    pub pending: bool,
}

pub(crate) fn selective_rewrap_preview(
    graph: &SourceGraph,
    candidates: &[ManagedRewrapCandidate<'_>],
) -> Vec<Vec<PathBuf>> {
    let eligible: HashSet<_> = candidates.iter()
        .filter(|row| !row.pending)
        .map(|row| row.item)
        .collect();
    let media: HashMap<_, _> = graph.media.iter()
        .map(|stream| (stream.id.as_str(), stream))
        .collect();
    let bundles = graph.items.iter()
        .filter(|item| eligible.contains(item.id.as_str()))
        .filter_map(|item| {
            let mut inputs = item.media.iter()
                .map(|binding| {
                    let stream = *media.get(binding.media.as_str())?;
                    let [path] = stream.locations.as_slice() else { return None };
                    (stream.online == Some(true) && stream.original.is_none())
                        .then_some((stream, path))
                })
                .collect::<Option<Vec<_>>>()?;
            inputs.sort_by_key(|(stream, _)| (stream.kind == StreamKind::Audio, stream.stream));
            let ((picture, _), audio) = inputs.split_first()?;
            if picture.kind != StreamKind::Video
                || audio.iter().any(|(stream, _)| stream.kind != StreamKind::Audio)
            {
                return None;
            }
            let prores = picture.codec.as_deref()
                .and_then(avid_reader::mxf::ProResProfile::from_fourcc)
                .is_some();
            let paths = inputs.into_iter().map(|(_, path)| PathBuf::from(path)).collect();
            Some((paths, prores))
        });
    crate::rewrap::preview_picture_recipes(bundles)
}

pub(crate) fn managed_rewrap_candidates(graph: &SourceGraph, op1a_requested: bool) -> Vec<ManagedRewrapCandidate<'_>> {
    use avid_reader::mxf::{PictureProfile, ProResProfile};
    use graph::Value;
    let profiles: HashMap<_, _> = graph.sidecars.iter().filter_map(|receipt| {
        if receipt.adapter != "avid.source" { return None; }
        let Value::Str(coding) = receipt.data.get("picture_coding")? else { return None };
        Some((receipt.object.as_str(), (PictureProfile::from_hex_label(coding)?, &receipt.data)))
    }).collect();
    let pictures: HashMap<_, _> = graph
        .media
        .iter()
        .filter(|media| {
            media.kind == StreamKind::Video
                && media.original.is_none()
                && media.online == Some(true)
        })
        .filter_map(|media| {
            let (profile, receipt) = profiles.get(media.id.as_str()).copied()
                .map(|(profile, receipt)| (Some(profile), Some(receipt))).unwrap_or_default();
            let descriptor = receipt.and_then(|r| r.get("mxf_picture_descriptor"));
            // The Avid adapter saves these locators only after verifying the
            // legacy sibling packages. Rewrap rechecks their physical clocks.
            let legacy_bundle = media.clock.is_none()
                && media.rate == graph::Rational::new(24_000, 1001)
                && receipt.is_some_and(|r| r.get("saved_file_track") == Some(&Value::Int(1))
                    && matches!(r.get("saved_locators"), Some(Value::Arr(paths)) if paths.len() == 1));
            let (destination, streams) = match (media.codec.as_deref(), profile) {
                (_, Some(PictureProfile::ProRes(profile))) => (format!("ProRes {} to MOV", profile.probe_name()), 1..=usize::MAX),
                (Some("apcs"), None) => (format!("ProRes {} to MOV", ProResProfile::Lt.probe_name()), 1..=usize::MAX),
                (Some("AVdn"), Some(profile @ PictureProfile::Dnx(1253)))
                    if !op1a_requested && legacy_bundle =>
                        (format!("{} to MXF OP1a · direct file clock is unqualified", profile.probe_name()), 9..=9),
                (Some("AVdn"), Some(profile @ PictureProfile::Dnx(cid)))
                    if !op1a_requested && !matches!(cid, 1253 | 1274) =>
                        (format!("{} to MXF OP1a", profile.probe_name()), 1..=usize::MAX),
                (Some("AVdn"), _) if !op1a_requested => ("DNx to MXF OP1a".into(), 34..=usize::MAX),
                _ => return None,
            };
            let [path] = media.locations.as_slice() else { return None };
            let pending = descriptor.and_then(|d| d.get("frame_layout"))
                .is_some_and(|v| matches!(v, Value::Int(n) if *n != 0))
                || matches!((media.frame_size, descriptor.and_then(|d| d.get("display_aspect"))),
                    (Some((w,h)), Some(Value::Arr(a))) if matches!(a.as_slice(),
                    [Value::Int(n), Value::Int(d)] if *n > 0 && *d > 0 && i128::from(w) * i128::from(*d) != i128::from(h) * i128::from(*n)));
            Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mxf"))
                .then_some((media.id.as_str(), (path, destination, streams, pending)))
        })
        .collect();
    if pictures.is_empty() {
        return Vec::new();
    }
    graph
        .items
        .iter()
        .flat_map(|item| {
            item.media
                .iter()
                .filter_map(|binding| pictures.get(binding.media.as_str()))
                .filter(move |(_, _, streams, _)| streams.contains(&item.media.len()))
                .map(|(path, destination, _, pending)| ManagedRewrapCandidate {
                    item: item.id.as_str(),
                    detail: format!("{} · {} · {path} · {destination}{}",
                        item.name, item.id, if *pending { " · field order or pixel aspect needs qualification" } else { "" }),
                    pending: *pending,
                })
        })
        .collect()
}

pub(crate) fn prepare_typed(
    graph: &SourceGraph,
    options: &Options,
    clock: ClockProfile,
    native_rewrap: bool,
    matte: &mut png_mattes::Plan,
    bins: &[SourceInput],
    ffprobe: &Path,
    spooled: bool,
    assisted: bool,
    managed: Option<(&HashSet<&str>, &HashSet<&str>)>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(PreparedPremiereInput, crate::source_luts::Applied), premiere_adapter::PreparationIssue> {
    let mut prepared = premiere_adapter::prepare_avid_job(graph, options, clock, spooled, assisted, managed)
        .map_err(|mut issue| {
            issue.message = format!("Premiere preparation failed: {}", issue.message);
            issue
        })?;
    matte.apply(&mut prepared.input, graph)?;
    prepared.losses.retain(|loss| matte.keeps_loss(loss));
    let mut luts = if clock == ClockProfile::LegacyImport || native_rewrap {
        let color_media = if native_rewrap {
            crate::source_luts::ColorMedia::Rewrap
        } else {
            crate::source_luts::ColorMedia::Linked
        };
        crate::source_luts::apply(
            &mut prepared, graph, bins, ffprobe, color_media, managed.map(|(selected, _)| selected), check,
        )?
    } else {
        Default::default()
    };
    if clock == ClockProfile::LegacyImport && !native_rewrap {
        crate::source_luts::prepare_extended_sdr(&mut prepared, graph, ffprobe, &mut luts, check)?;
    }
    Ok((prepared, luts))
}

pub(crate) fn prepare_document(
    input: &PreparedProjectInput,
    output: &Path,
    ids: &mut dyn UuidSource,
    bindings: &[MediaRelink],
    native: bool,
    layout: Option<&DestinationBinLayout>,
    offline: &HashSet<&str>,
    dnxhr_444_rgb_sources: &HashSet<String>,
) -> writer::Result<PreparedProject> {
    let media = if offline.is_empty() {
        TypedMedia::Bound { bindings, native_av: native }
    } else {
        TypedMedia::Offline { paths: offline }
    };
    writer::prepare_typed_project_for_premiere_27_with_rewrap_colors(
        input, output, ids, media, layout, dnxhr_444_rgb_sources,
    )
}

pub(crate) fn timeline_subject(
    sequence: &graph::source::SourceSequence,
    kind: graph::StreamKind,
    track: i64,
    record_start: Option<graph::Rational>,
) -> String {
    timeline_subject_at(
        &sequence.name, kind, track, Some(sequence.rate), record_start, review_clock(sequence),
    )
}

pub(crate) fn timeline_subject_with_exact_offset(
    sequence: &graph::source::SourceSequence,
    kind: StreamKind,
    track: i64,
    offset: Option<graph::Rational>,
) -> String {
    let position = offset.map(|offset| {
        timeline_position(sequence.rate, offset, review_clock(sequence))
            .unwrap_or_else(|| format!("{offset} seconds from sequence start"))
    });
    format_timeline_subject(&sequence.name, kind, track, position)
}

fn review_clock(sequence: &graph::source::SourceSequence) -> Option<(graph::Rational, u16, bool)> {
    let clock = sequence.clock.as_ref()?;
    let start = sequence.start?;
    (clock.rate == sequence.rate && clock.exact_start() == Some(start))
        .then_some((start, clock.display_base, clock.drop))
}

pub(crate) fn timeline_subject_at(
    name: &str,
    kind: graph::StreamKind,
    track: i64,
    rate: Option<graph::Rational>,
    record_start: Option<graph::Rational>,
    clock: Option<(graph::Rational, u16, bool)>,
) -> String {
    let position = rate.zip(record_start).and_then(|(rate, offset)| timeline_position(rate, offset, clock));
    format_timeline_subject(name, kind, track, position)
}

fn format_timeline_subject(name: &str, kind: StreamKind, track: i64, position: Option<String>) -> String {
    let lane = match kind {
        graph::StreamKind::Audio => "A",
        graph::StreamKind::Video => "V",
    };
    let subject = format!("{name} · {lane}{track}");
    match position {
        Some(position) => format!("{subject} · {position}"),
        None => subject,
    }
}

fn timeline_position(
    rate: graph::Rational,
    offset: graph::Rational,
    clock: Option<(graph::Rational, u16, bool)>,
) -> Option<String> {
    use prproj::build::time::{Time, TICKS_PER_SECOND};

    let frame = graph::Rational::new(rate.den().into(), rate.num().into())?;
    let timebase = frame.to_units(TICKS_PER_SECOND)?;
    let offset_ticks = offset.to_units(TICKS_PER_SECOND)?;
    if timebase <= 0 || offset_ticks < 0 || offset_ticks % timebase != 0 {
        return None;
    }
    if let Some((start, display_base, drop)) = clock {
        let nominal = (TICKS_PER_SECOND as f64 / timebase as f64).round_ties_even();
        let valid_drop = !drop || matches!((rate.num(), rate.den()), (30_000, 1001) | (60_000, 1001));
        if f64::from(display_base) == nominal && valid_drop {
            if let Some(ticks) = start.to_units(TICKS_PER_SECOND)
                .and_then(|start| start.checked_add(offset_ticks))
            {
                return Time::new(ticks).to_timecode(timebase, drop).ok();
            }
        }
    }
    Some(format!("frame {} from sequence start", offset_ticks / timebase))
}

pub(crate) fn grouped_picture_marker_losses(source: &SourceGraph, report: &Report) -> Vec<graph::Loss> {
    use prproj::build::time::TICKS_PER_SECOND;

    report.grouped_picture_marker_moves.iter().map(|marker| {
        let offset = graph::Rational::new(marker.record_ticks.into(), TICKS_PER_SECOND.into());
        let (object, subject) = match source.sequences.get(marker.sequence) {
            Some(sequence) => (
                sequence.id.clone(),
                timeline_subject(sequence, graph::StreamKind::Video, 1, offset),
            ),
            None => {
                let sequence = format!("Sequence {}", marker.sequence.saturating_add(1));
                (sequence.clone(), format!("{sequence} · V1"))
            }
        };
        graph::Loss {
            object,
            aspect: "marker_ownership".into(),
            reason: format!(
                "{subject}. Marker {:?} stays at its record position on the sequence because the Grouped clip also carries audio markers.",
                marker.marker_name,
            ),
        }
    }).collect()
}
