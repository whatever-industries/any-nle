//! `Caption` and `CaptionTrack` (port of the classes in `models/caption.py`), as handles into a
//! [`CaptionStore`] owned by the [`Project`].
//!
//! Python's property reads become `caption_*` methods (`&mut self` because reading a payload
//! fills the document's hash cache); setters are `set_caption_*`.

use std::collections::HashMap;

use super::flat::{self, Color};
use super::{PyError, PyResult};
use crate::build::fmt::fmt_f;
use crate::build::time::Time;
use crate::convert::model::py_str_repr;
use crate::model::project::py_int;
use crate::model::{Project, SequenceId};
use crate::xml::NodeId;

pub use flat::Color as CaptionColor;

/// Handle to a caption track (Python `CaptionTrack`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CaptionTrackId(pub(crate) usize);

/// Handle to a caption (Python `Caption`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CaptionId(pub(crate) usize);

/// Python `Caption` attributes.
#[derive(Clone, Debug)]
pub struct CaptionData {
    /// The `CaptionDataClipTrackItem` object.
    pub element: NodeId,
    /// `_text`.
    pub text: String,
    pub track: CaptionTrackId,
    /// `_source_start`.
    pub source_start: Time,
    /// `_source_end`.
    pub source_end: Time,
}

/// Python `CaptionTrack` attributes.
#[derive(Clone, Debug)]
pub struct CaptionTrackData {
    /// The `CaptionDataClipTrack` object.
    pub element: NodeId,
    pub sequence: SequenceId,
    /// `_captions`.
    pub captions: Vec<CaptionId>,
}

/// Caption records of a project (all handles ever made, plus each sequence's current tracks).
#[derive(Clone, Debug, Default)]
pub struct CaptionStore {
    pub(crate) tracks: Vec<CaptionTrackData>,
    pub(crate) captions: Vec<CaptionData>,
    /// `Sequence._caption_tracks`.
    pub(crate) by_sequence: HashMap<SequenceId, Vec<CaptionTrackId>>,
}

/// Alias used by the module docs.
pub type Caption = CaptionData;

/// Python `CaptionFormat` (an `IntEnum` of ExtendScript's `CAPTION_FORMAT_*` constants).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptionFormat {
    Subtitle = 0,
    Cea608 = 1,
    Cea708 = 2,
    Teletext = 3,
    EbuSubtitle = 0x10003,
    Op42 = 0x20003,
    Op47 = 0x30003,
}

impl CaptionFormat {
    /// `CaptionFormat(value)`; `ValueError` text as Python's enum.
    pub fn from_value(value: i64) -> PyResult<CaptionFormat> {
        Ok(match value {
            0 => CaptionFormat::Subtitle,
            1 => CaptionFormat::Cea608,
            2 => CaptionFormat::Cea708,
            3 => CaptionFormat::Teletext,
            0x10003 => CaptionFormat::EbuSubtitle,
            0x20003 => CaptionFormat::Op42,
            0x30003 => CaptionFormat::Op47,
            other => return super::value_err(format!("{other} is not a valid CaptionFormat")),
        })
    }

    /// `validate_enum(CaptionFormat)` for a plain int.
    pub fn validate_int(value: i64) -> PyResult<CaptionFormat> {
        Self::from_value(value).map_err(|_| PyError::Value(format!("{value} is not a valid CaptionFormat value")))
    }

    /// `int(format)`.
    pub fn value(self) -> i64 {
        self as i64
    }

    /// Python's member name.
    pub fn name(self) -> &'static str {
        match self {
            CaptionFormat::Subtitle => "SUBTITLE",
            CaptionFormat::Cea608 => "CEA_608",
            CaptionFormat::Cea708 => "CEA_708",
            CaptionFormat::Teletext => "TELETEXT",
            CaptionFormat::EbuSubtitle => "EBU_SUBTITLE",
            CaptionFormat::Op42 => "OP42",
            CaptionFormat::Op47 => "OP47",
        }
    }
}

const TRACK_ITEM: &str = "DataClipTrackItem/ClipTrackItem/TrackItem/";

/// `int(text or 0)`.
fn int_or_zero(text: Option<String>) -> PyResult<i64> {
    match text.filter(|t| !t.is_empty()) {
        None => Ok(0),
        Some(t) => Ok(py_int(&t)?),
    }
}

impl Project {
    // ---- Sequence / track level ---------------------------------------------------------------

    /// `Sequence.caption_tracks`.
    pub fn caption_tracks(&self, sequence: SequenceId) -> Vec<CaptionTrackId> {
        self.captions.by_sequence.get(&sequence).cloned().unwrap_or_default()
    }

    /// A caption track's record.
    pub fn caption_track(&self, track: CaptionTrackId) -> &CaptionTrackData {
        &self.captions.tracks[track.0]
    }

    /// A caption's record.
    pub fn caption(&self, caption: CaptionId) -> &CaptionData {
        &self.captions.captions[caption.0]
    }

    /// `CaptionTrack.sequence`.
    pub fn caption_track_sequence(&self, track: CaptionTrackId) -> SequenceId {
        self.caption_track(track).sequence
    }

    /// `CaptionTrack.id`.
    pub fn caption_track_id(&self, track: CaptionTrackId) -> PyResult<i64> {
        let tree = &self.doc.tree;
        int_or_zero(tree.findtext(self.caption_track(track).element, "DataClipTrack/ClipTrack/Track/ID")?)
    }

    /// `CaptionTrack.index`.
    pub fn caption_track_index(&self, track: CaptionTrackId) -> PyResult<i64> {
        let tree = &self.doc.tree;
        int_or_zero(tree.findtext(self.caption_track(track).element, "DataClipTrack/ClipTrack/Track/Index")?)
    }

    /// `CaptionTrack.captions`.
    pub fn captions(&self, track: CaptionTrackId) -> Vec<CaptionId> {
        self.caption_track(track).captions.clone()
    }

    /// `CaptionTrack.format`.
    pub fn caption_track_format(&self, track: CaptionTrackId) -> PyResult<CaptionFormat> {
        let element = self.caption_track(track).element;
        let low = int_or_zero(self.doc.tree.findtext(element, "Format")?)?;
        let high = int_or_zero(self.doc.tree.findtext(element, "SubFormat")?)?;
        CaptionFormat::from_value((high << 16) | low)
    }

    /// `CaptionTrack.format = value`.
    pub fn set_caption_track_format(&mut self, track: CaptionTrackId, value: CaptionFormat) -> PyResult<()> {
        let element = self.caption_track(track).element;
        super::builder::write_track_format(&mut self.doc.tree, element, value.value())
    }

    /// `repr(CaptionTrack)`.
    pub fn caption_track_repr(&self, track: CaptionTrackId) -> PyResult<String> {
        Ok(format!(
            "CaptionTrack(index={}, captions={})",
            self.caption_track_index(track)?,
            self.caption_track(track).captions.len()
        ))
    }

    // ---- Caption level --------------------------------------------------------------------------

    /// `Caption.track`.
    pub fn caption_owner(&self, caption: CaptionId) -> CaptionTrackId {
        self.caption(caption).track
    }

    /// `Caption.text`.
    pub fn caption_text(&self, caption: CaptionId) -> &str {
        &self.caption(caption).text
    }

    /// `Caption.text = value`.
    pub fn set_caption_text(&mut self, caption: CaptionId, value: &str) -> PyResult<()> {
        crate::model::item::validate_name(value)?;
        let payload = self.caption_payload(caption)?;
        let replaced = flat::replace_payload_text(&payload, value)?;
        self.caption_write_payload(caption, &replaced)?;
        self.captions.captions[caption.0].text = value.to_string();
        Ok(())
    }

    /// `Caption.start`.
    pub fn caption_start(&self, caption: CaptionId) -> PyResult<Time> {
        let element = self.caption(caption).element;
        Ok(Time::new(int_or_zero(self.doc.tree.findtext(element, &format!("{TRACK_ITEM}Start"))?)?))
    }

    /// `Caption.end`.
    pub fn caption_end(&self, caption: CaptionId) -> PyResult<Time> {
        let element = self.caption(caption).element;
        Ok(Time::new(int_or_zero(self.doc.tree.findtext(element, &format!("{TRACK_ITEM}End"))?)?))
    }

    /// `Caption.source_start`.
    pub fn caption_source_start(&self, caption: CaptionId) -> Time {
        self.caption(caption).source_start
    }

    /// `Caption.source_end`.
    pub fn caption_source_end(&self, caption: CaptionId) -> Time {
        self.caption(caption).source_end
    }

    /// `Caption._text_data`: the timeline block's FormattedTextData.
    fn caption_text_data(&self, caption: CaptionId) -> PyResult<Option<NodeId>> {
        let tree = &self.doc.tree;
        let Some(reference) = tree.find1(self.caption(caption).element, "BlockVector/BlockVectorItem") else {
            return Ok(None);
        };
        let block = self.doc.resolve(reference)?;
        Ok(tree.child_by_tag(block, "FormattedTextData"))
    }

    /// `Caption._payload`.
    pub fn caption_payload(&mut self, caption: CaptionId) -> PyResult<Vec<u8>> {
        let data = self.caption_text_data(caption)?;
        let payload = match data {
            None => None,
            Some(d) => self.doc.payload(d)?,
        };
        payload.ok_or_else(|| PyError::Value("caption has no styled-text payload".into()))
    }

    /// `Caption._write_payload`.
    pub fn caption_write_payload(&mut self, caption: CaptionId, payload: &[u8]) -> PyResult<()> {
        let Some(data) = self.caption_text_data(caption)? else {
            return super::value_err("caption has no styled-text payload");
        };
        flat::write_payload_element(&mut self.doc, data, payload)
    }

    /// `Caption.font_size`.
    pub fn caption_font_size(&mut self, caption: CaptionId) -> PyResult<f64> {
        flat::read_font_size(&self.caption_payload(caption)?)
    }

    /// `Caption.font_size = value`.
    pub fn set_caption_font_size(&mut self, caption: CaptionId, value: f64) -> PyResult<()> {
        flat::validate_positive_number(value)?;
        let payload = self.caption_payload(caption)?;
        let new_payload = flat::write_font_size(&payload, value)?;
        if new_payload != payload {
            self.caption_write_payload(caption, &new_payload)?;
        }
        Ok(())
    }

    /// `Caption.font_family`.
    pub fn caption_font_family(&mut self, caption: CaptionId) -> PyResult<String> {
        flat::read_font_family(&self.caption_payload(caption)?)
    }

    /// `Caption.font_family = value`.
    pub fn set_caption_font_family(&mut self, caption: CaptionId, value: &str) -> PyResult<()> {
        crate::model::item::validate_name(value)?;
        let payload = self.caption_payload(caption)?;
        let written = flat::write_font_family(&payload, value)?;
        self.caption_write_payload(caption, &written)
    }

    fn style_float(&mut self, caption: CaptionId, slot: usize) -> PyResult<Option<f64>> {
        flat::read_style_float(&self.caption_payload(caption)?, slot)
    }

    fn set_style_float(&mut self, caption: CaptionId, slot: usize, value: f64, enable: Option<usize>) -> PyResult<()> {
        flat::validate_number(value)?;
        let mut payload = flat::write_style_float(&self.caption_payload(caption)?, slot, value)?;
        if let Some(flag) = enable {
            payload = flat::write_style_flag(&payload, flag)?;
        }
        self.caption_write_payload(caption, &payload)
    }

    fn style_color(&mut self, caption: CaptionId, slot: usize) -> PyResult<Option<Color>> {
        flat::read_style_color(&self.caption_payload(caption)?, slot)
    }

    fn set_style_color(&mut self, caption: CaptionId, slot: usize, value: Color, enable: Option<usize>) -> PyResult<()> {
        value.validate()?;
        let mut payload = flat::write_style_color(&self.caption_payload(caption)?, slot, value)?;
        if let Some(flag) = enable {
            payload = flat::write_style_flag(&payload, flag)?;
        }
        self.caption_write_payload(caption, &payload)
    }

    fn block_float(&mut self, caption: CaptionId, slot: usize) -> PyResult<Option<f64>> {
        flat::read_block_float(&self.caption_payload(caption)?, slot)
    }

    fn set_block_float(&mut self, caption: CaptionId, slot: usize, value: f64, enable: Option<usize>) -> PyResult<()> {
        flat::validate_number(value)?;
        let mut payload = flat::write_block_float(&self.caption_payload(caption)?, slot, value)?;
        if let Some(flag) = enable {
            payload = flat::write_block_flag(&payload, flag)?;
        }
        self.caption_write_payload(caption, &payload)
    }

    fn block_color(&mut self, caption: CaptionId, slot: usize) -> PyResult<Option<Color>> {
        flat::read_block_color(&self.caption_payload(caption)?, slot)
    }

    fn set_block_color(&mut self, caption: CaptionId, slot: usize, value: Color, enable: Option<usize>) -> PyResult<()> {
        value.validate()?;
        let mut payload = flat::write_block_color(&self.caption_payload(caption)?, slot, value)?;
        if let Some(flag) = enable {
            payload = flat::write_block_flag(&payload, flag)?;
        }
        self.caption_write_payload(caption, &payload)
    }

    /// `repr(Caption)`.
    pub fn caption_repr(&self, caption: CaptionId) -> PyResult<String> {
        Ok(format!(
            "Caption(text={}, start={}s)",
            py_str_repr(&self.caption(caption).text),
            fmt_f(self.caption_start(caption)?.seconds(), 3)
        ))
    }
}

macro_rules! style_props {
    ($( $kind:ident, $get:ident, $set:ident, $slot:expr, $enable:expr, $ty:ty, $doc:literal; )*) => {
        impl Project {
            $( style_props!(@one $kind, $get, $set, $slot, $enable, $ty, $doc); )*
        }
    };
    (@one float_style, $get:ident, $set:ident, $slot:expr, $enable:expr, $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $get(&mut self, caption: CaptionId) -> PyResult<Option<f64>> {
            self.style_float(caption, $slot)
        }
        #[doc = $doc]
        pub fn $set(&mut self, caption: CaptionId, value: f64) -> PyResult<()> {
            self.set_style_float(caption, $slot, value, $enable)
        }
    };
    (@one color_style, $get:ident, $set:ident, $slot:expr, $enable:expr, $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $get(&mut self, caption: CaptionId) -> PyResult<Option<Color>> {
            self.style_color(caption, $slot)
        }
        #[doc = $doc]
        pub fn $set(&mut self, caption: CaptionId, value: Color) -> PyResult<()> {
            self.set_style_color(caption, $slot, value, $enable)
        }
    };
    (@one float_block, $get:ident, $set:ident, $slot:expr, $enable:expr, $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $get(&mut self, caption: CaptionId) -> PyResult<Option<f64>> {
            self.block_float(caption, $slot)
        }
        #[doc = $doc]
        pub fn $set(&mut self, caption: CaptionId, value: f64) -> PyResult<()> {
            self.set_block_float(caption, $slot, value, $enable)
        }
    };
    (@one color_block, $get:ident, $set:ident, $slot:expr, $enable:expr, $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $get(&mut self, caption: CaptionId) -> PyResult<Option<Color>> {
            self.block_color(caption, $slot)
        }
        #[doc = $doc]
        pub fn $set(&mut self, caption: CaptionId, value: Color) -> PyResult<()> {
            self.set_block_color(caption, $slot, value, $enable)
        }
    };
}

use flat::*;

style_props! {
    color_style, caption_fill_color, set_caption_fill_color, STYLE_FILL_SLOT, None, Color, "`Caption.fill_color` (read/write).";
    color_style, caption_stroke_color, set_caption_stroke_color, STYLE_STROKE_COLOR_SLOT, Some(STYLE_STROKE_ENABLED_SLOT), Color, "`Caption.stroke_color` (setting raises the Stroke enable flag).";
    float_style, caption_stroke_width, set_caption_stroke_width, STYLE_STROKE_WIDTH_SLOT, Some(STYLE_STROKE_ENABLED_SLOT), f64, "`Caption.stroke_width` (setting raises the Stroke enable flag).";
    float_style, caption_tracking, set_caption_tracking, STYLE_TRACKING_SLOT, None, f64, "`Caption.tracking`.";
    float_block, caption_leading, set_caption_leading, BLOCK_LEADING_SLOT, None, f64, "`Caption.leading`.";
    color_block, caption_shadow_color, set_caption_shadow_color, BLOCK_SHADOW_COLOR_SLOT, None, Color, "`Caption.shadow_color`.";
    float_block, caption_shadow_opacity, set_caption_shadow_opacity, BLOCK_SHADOW_OPACITY_SLOT, None, f64, "`Caption.shadow_opacity`.";
    float_block, caption_shadow_angle, set_caption_shadow_angle, BLOCK_SHADOW_ANGLE_SLOT, None, f64, "`Caption.shadow_angle`.";
    float_block, caption_shadow_distance, set_caption_shadow_distance, BLOCK_SHADOW_DISTANCE_SLOT, None, f64, "`Caption.shadow_distance`.";
    float_block, caption_shadow_size, set_caption_shadow_size, BLOCK_SHADOW_SIZE_SLOT, None, f64, "`Caption.shadow_size`.";
    float_block, caption_shadow_blur, set_caption_shadow_blur, BLOCK_SHADOW_BLUR_SLOT, None, f64, "`Caption.shadow_blur`.";
    color_block, caption_background_color, set_caption_background_color, BLOCK_BACKGROUND_COLOR_SLOT, Some(BLOCK_BACKGROUND_ENABLED_SLOT), Color, "`Caption.background_color` (setting raises the Background enable flag).";
    float_block, caption_background_opacity, set_caption_background_opacity, BLOCK_BACKGROUND_OPACITY_SLOT, Some(BLOCK_BACKGROUND_ENABLED_SLOT), f64, "`Caption.background_opacity`.";
    float_block, caption_background_size, set_caption_background_size, BLOCK_BACKGROUND_SIZE_SLOT, Some(BLOCK_BACKGROUND_ENABLED_SLOT), f64, "`Caption.background_size`.";
    float_block, caption_background_corner_radius, set_caption_background_corner_radius, BLOCK_BACKGROUND_CORNER_SLOT, Some(BLOCK_BACKGROUND_ENABLED_SLOT), f64, "`Caption.background_corner_radius`.";
}
