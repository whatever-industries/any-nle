//! Plain raster-mismatch controls on Resolve video edits.
use crate::{linked::SidecarLookup, Result};
use graph::{Sidecar, Value, source::SourceGraph};

pub(crate) const PREMIERE: &str = "premiere.input_scaling";
pub(crate) const RESOLVE: &str = "resolve.input_scaling";
pub(crate) const CENTER_CROP: &str = "center_crop";
pub(crate) const FIT: &str = "scale_to_fit";
pub(crate) const PROJECT_SETTING: &str = "project_input_scaling";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sizing {
    Fit,
    CenterCrop,
}

// Same-build native change from Scaling = Project to Scaling = Crop.
pub(crate) const CROP_FILTERS: &str =
    "0000000200000019800a16082c38124a004a004a08085c1a040a0220014a004a00";
pub(crate) const PROJECT_FILTERS: &str = "0000000200000011800a0e082c38124a004a004a004a004a00";
const CROP_PROPERTY: &[u8] = &[0x1a, 0x07, 0x08, 0x01, 0x10, 0xc4, 0x83, 0x98, 0x06];

pub(crate) fn receipt(adapter: &str, edit: &str, value: &str) -> Sidecar {
    Sidecar {
        adapter: adapter.into(),
        object: edit.into(),
        data: Value::Str(value.into()),
    }
}

fn observed(value: Option<&Value>) -> Result<Option<Sizing>> {
    match value {
        None => Ok(None),
        Some(Value::Str(mode)) if mode == FIT => Ok(Some(Sizing::Fit)),
        Some(Value::Str(mode)) if mode == CENTER_CROP => Ok(Some(Sizing::CenterCrop)),
        _ => Err("unqualified input sizing control".into()),
    }
}

fn explicit(sidecars: SidecarLookup<'_>, edit: &str) -> Result<Option<Sizing>> {
    let premiere = sidecars.get(PREMIERE, edit);
    let resolve = sidecars.get(RESOLVE, edit);
    if premiere.is_some() && resolve.is_some() {
        return Err("ambiguous input sizing owners".into());
    }
    observed(premiere.or(resolve))
}

pub(crate) fn prepare(
    sidecars: SidecarLookup<'_>, sequence: &str, edit: &str, raster_mismatch: bool,
) -> Result<Option<Sizing>> {
    if let Some(sizing) = explicit(sidecars, edit)? {
        return Ok((raster_mismatch || sizing == Sizing::CenterCrop).then_some(sizing));
    }
    if let Some(source) = sidecars.get(crate::source::ADAPTER, sequence).filter(|_| raster_mismatch) {
        let sizing = observed(source.get(PROJECT_SETTING))?
            .ok_or("unresolved inherited Resolve input sizing")?;
        return Ok(Some(sizing));
    }
    Ok(None)
}

pub(crate) fn matches_readback(
    graph: &SourceGraph, sequence: &str, edit: &str, expected: Option<Sizing>,
) -> Result<bool> {
    let clip = explicit(SidecarLookup::Linear(graph), edit)?;
    Ok(match expected {
        Some(Sizing::Fit) if clip.is_none() => {
            let project = graph.sidecar(crate::source::ADAPTER, sequence);
            observed(project.and_then(|source| source.get(PROJECT_SETTING)))? == expected
        }
        Some(_) => clip == expected,
        None => clip != Some(Sizing::CenterCrop),
    })
}

pub(crate) fn crop_fields(body: &[u8]) -> Result<Vec<u8>> {
    crate::binary::replace_bytes(body, 1, |state| {
        crate::binary::rewrite_bytes(state, 1, |saved| {
            if saved.is_some() {
                return Err("pinned video edit already has a sizing control".into());
            }
            Ok(Some(CROP_PROPERTY.to_vec()))
        })
    })
}
