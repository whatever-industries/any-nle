//! Editable plain subtitle cues on the first Resolve subtitle track.
use crate::{prepare::frames_on, Result};
use graph::{
    source::{SourceText, TextKind},
    Rational,
};

pub(crate) const TRACK_FIELDS: &str =
    "000000010000000100000012004e0075006d004c00610079006500720073000000020000000000";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Caption {
    pub(crate) id: String,
    pub(crate) start: i64,
    pub(crate) length: i64,
    pub(crate) text: String,
}

pub(crate) fn plain_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 32_768
        && !text.contains(['<', '>'])
        && !text.chars().any(|c| c.is_control() && c != '\n')
}

pub(crate) fn prepare(text: &SourceText, rate: Rational, extent: i64) -> Result<Caption> {
    let unsupported = || format!("unsupported Resolve caption {}", text.id);
    let Some((start, length)) = text.record_start.zip(text.duration) else {
        return Err(unsupported().into());
    };
    let start = frames_on(start, rate)?;
    let length = frames_on(length, rate)?;
    let Some(content) = text.content.as_deref().filter(|text| plain_text(text)) else {
        return Err(unsupported().into());
    };
    if text.kind != TextKind::Caption
        || text.track < 1
        || text.reason.is_some()
        || start < 0
        || length <= 0
        || start.checked_add(length).is_none_or(|end| end > extent)
    {
        return Err(unsupported().into());
    }
    Ok(Caption {
        id: text.id.clone(),
        start,
        length,
        text: content.into(),
    })
}
