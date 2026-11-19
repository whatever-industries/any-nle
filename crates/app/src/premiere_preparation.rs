//! Shared graph and typed document preparation for Avid to Premiere.
use crate::png_mattes;
use avid_reader::source::SourceInput;
use graph::source::SourceGraph;
use premiere_adapter::{ClockProfile, PreparedPremiereInput};
use prproj::convert::{
    model::Options,
    prepared::PreparedProjectInput,
    rewrap::MediaRelink,
    writer::{self, DestinationBinLayout, PreparedProject, Report, TypedMedia},
};
use prproj::UuidSource;
use std::{collections::HashSet, path::Path};

pub(crate) fn clock_profile(rewrap: bool) -> ClockProfile {
    if rewrap {
        ClockProfile::VerifiedRewrap
    } else {
        ClockProfile::LegacyImport
    }
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
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(PreparedPremiereInput, crate::source_luts::Applied), String> {
    let mut prepared = if clock == ClockProfile::LegacyImport {
        premiere_adapter::prepare_linked_originals(graph, options, clock, spooled)
    } else {
        premiere_adapter::prepare_originals(graph, options, clock)
    }.map_err(|e| format!("Premiere preparation failed: {e}"))?;
    matte.apply(&mut prepared.input, graph)?;
    prepared.losses.retain(|loss| matte.keeps_loss(loss));
    let mut luts = if clock == ClockProfile::LegacyImport || native_rewrap {
        let color_media = if native_rewrap {
            crate::source_luts::ColorMedia::Rewrap
        } else {
            crate::source_luts::ColorMedia::Linked
        };
        crate::source_luts::apply(&mut prepared, graph, bins, ffprobe, color_media, check)?
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
) -> writer::Result<PreparedProject> {
    let media = if offline.is_empty() {
        TypedMedia::Bound { bindings, native_av: native }
    } else {
        TypedMedia::Offline { paths: offline }
    };
    writer::prepare_typed_project_for_premiere_27(input, output, ids, media, layout)
}

pub(crate) fn timeline_subject(
    sequence: &graph::source::SourceSequence,
    kind: graph::StreamKind,
    track: i64,
    record_start: Option<graph::Rational>,
) -> String {
    use prproj::build::time::{Time, TICKS_PER_SECOND};

    let lane = match kind {
        graph::StreamKind::Audio => "A",
        graph::StreamKind::Video => "V",
    };
    let subject = format!("{} · {lane}{track}", sequence.name);
    let position = (|| {
        let offset = record_start?;
        let frame = graph::Rational::new(sequence.rate.den().into(), sequence.rate.num().into())?;
        let timebase = frame.to_units(TICKS_PER_SECOND)?;
        let offset_ticks = offset.to_units(TICKS_PER_SECOND)?;
        if timebase <= 0 || offset_ticks < 0 || offset_ticks % timebase != 0 {
            return None;
        }
        if let Some(clock) = &sequence.clock {
            let nominal = (TICKS_PER_SECOND as f64 / timebase as f64).round_ties_even();
            let valid_drop = !clock.drop
                || matches!((clock.rate.num(), clock.rate.den()), (30_000, 1001) | (60_000, 1001));
            if clock.rate == sequence.rate
                && f64::from(clock.display_base) == nominal
                && valid_drop
                && clock.exact_start() == sequence.start
            {
                if let Some(ticks) = sequence.start.and_then(|start| start.to_units(TICKS_PER_SECOND))
                    .and_then(|start| start.checked_add(offset_ticks))
                {
                    return Time::new(ticks).to_timecode(timebase, clock.drop).ok();
                }
            }
        }
        Some(format!("frame {} from sequence start", offset_ticks / timebase))
    })();
    match position {
        Some(position) => format!("{subject} · {position}"),
        None => subject,
    }
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
