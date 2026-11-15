//! Shared graph and typed document preparation for Avid to Premiere.
use crate::png_mattes;
use graph::source::SourceGraph;
use premiere_adapter::{ClockProfile, PreparedPremiereInput};
use prproj::convert::{
    model::Options,
    prepared::PreparedProjectInput,
    rewrap::MediaRelink,
    writer::{self, DestinationBinLayout, PreparedProject},
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
    matte: &mut png_mattes::Plan,
) -> Result<PreparedPremiereInput, String> {
    let mut prepared = premiere_adapter::prepare_originals(graph, options, clock)
        .map_err(|e| format!("Premiere preparation failed: {e}"))?;
    matte.apply(&mut prepared.input, graph)?;
    prepared.losses.retain(|loss| matte.keeps_loss(loss));
    Ok(prepared)
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
    if !offline.is_empty() {
        return writer::prepare_offline_project(input, output, ids, layout, offline);
    }
    if let Some(layout) = layout {
        writer::prepare_typed_project_with_layout(input, output, ids, bindings, native, layout)
    } else {
        writer::prepare_typed_project(input, output, ids, bindings, native)
    }
}
