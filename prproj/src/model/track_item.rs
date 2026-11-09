//! Placed clips (port of `py_premiere/models/track_item.py` timing and the `XmlField`
//! descriptors of `models/descriptors.py` it uses).
//!
//! `start`/`end` are timeline positions stored on `ClipTrackItem/TrackItem` (a zero `Start` is
//! elided); `in_point`/`out_point` are source positions stored on the timeline clip's `Clip`
//! core, scaled by `PlaybackSpeed` (raw = source x speed, read back truncated).

use super::project::py_int;
use super::{ItemId, Project, TrackItemId};
use crate::build::marker::validate_string;
use crate::build::time::Time;
use crate::error::{value, Error, Result};
use crate::xml::mutations::insert_leaf_before;
use crate::xml::NodeId;

impl Project {
    fn timing_parent(&self, item: TrackItemId) -> Result<NodeId> {
        let element = self.track_item(item).element;
        self.doc.tree.find1(element, "ClipTrackItem/TrackItem").ok_or_else(|| {
            Error::Value(format!("missing <ClipTrackItem/TrackItem> under <{}>", self.doc.tree.tag(element)))
        })
    }

    fn timing_field(&self, item: TrackItemId, leaf: &str) -> Result<Time> {
        let parent = self.timing_parent(item)?;
        let text = match self.doc.tree.child_by_tag(parent, leaf) {
            Some(e) => self.doc.tree[e].text_or_empty(),
            None => "0",
        };
        Ok(Time::new(py_int(text)?))
    }

    /// `TrackItem.start` (0 when elided).
    pub fn start(&self, item: TrackItemId) -> Result<Time> {
        self.timing_field(item, "Start")
    }

    /// `TrackItem.end`.
    pub fn end(&self, item: TrackItemId) -> Result<Time> {
        self.timing_field(item, "End")
    }

    /// `TrackItem.duration`: `end - start`.
    pub fn duration(&self, item: TrackItemId) -> Result<Time> {
        Ok(Time::new(self.end(item)?.ticks - self.start(item)?.ticks))
    }

    /// `TrackItem.start = value`: writes the text (creating `<Start>` before `End` when elided,
    /// even for 0, like the Python descriptor), then grows the sequence's source duration.
    pub fn set_start(&mut self, item: TrackItemId, value: Time) -> Result<()> {
        let parent = self.timing_parent(item)?;
        let text = value.ticks.to_string();
        match self.doc.tree.child_by_tag(parent, "Start") {
            Some(e) => self.doc.tree.set_text(e, Some(text)),
            None => {
                insert_leaf_before(&mut self.doc.tree, parent, "End", "Start", &text)?;
            }
        }
        self.carry_sequence_length(item)
    }

    /// `TrackItem.end = value`, then grows the sequence's source duration.
    pub fn set_end(&mut self, item: TrackItemId, value: Time) -> Result<()> {
        let parent = self.timing_parent(item)?;
        let Some(e) = self.doc.tree.child_by_tag(parent, "End") else {
            return value_err("cannot set end: <ClipTrackItem/TrackItem/End> is absent and its creation anchor is not defined for this field");
        };
        self.doc.tree.set_text(e, Some(value.ticks.to_string()));
        self.carry_sequence_length(item)
    }

    fn carry_sequence_length(&mut self, item: TrackItemId) -> Result<()> {
        let sequence = self.track(self.track_item(item).track).sequence;
        self.grow_source_duration(sequence)
    }

    /// `TrackItem.speed`: `PlaybackSpeed`, 1.0 when elided.
    pub fn speed(&self, item: TrackItemId) -> Result<f64> {
        let clip = self.track_item(item).clip;
        match self.doc.tree.findtext(clip, "Clip/PlaybackSpeed")? {
            Some(t) if !t.is_empty() => py_float(&t),
            _ => Ok(1.0),
        }
    }

    /// `TrackItem.is_speed_reversed`.
    pub fn is_speed_reversed(&self, item: TrackItemId) -> Result<bool> {
        Ok(self.doc.tree.findtext(self.track_item(item).clip, "Clip/PlayBackwards")?.as_deref() == Some("true"))
    }

    fn source_ticks(&self, item: TrackItemId, tag: &str) -> Result<i64> {
        let clip = self.track_item(item).clip;
        let raw = match self.doc.tree.findtext(clip, &format!("Clip/{tag}"))? {
            Some(t) if !t.is_empty() => py_int(&t)?,
            _ => 0,
        };
        py_div_trunc(raw, self.speed(item)?)
    }

    fn write_source_ticks(&mut self, item: TrackItemId, tag: &str, ticks: i64, anchors: &[&str]) -> Result<()> {
        let clip = self.track_item(item).clip;
        let Some(core) = self.doc.tree.child_by_tag(clip, "Clip") else { return value("missing <Clip> element") };
        let speed = self.speed(item)?;
        let mut raw = py_round(ticks as f64 * speed)?;
        while py_div_trunc(raw, speed)? < ticks {
            raw += 1;
        }
        while raw > 0 && py_div_trunc(raw - 1, speed)? >= ticks {
            raw -= 1;
        }
        let text = raw.to_string();
        let tree = &mut self.doc.tree;
        if let Some(e) = tree.child_by_tag(core, tag) {
            tree.set_text(e, Some(text));
            return Ok(());
        }
        for anchor in anchors {
            if tree.child_by_tag(core, anchor).is_some() {
                insert_leaf_before(tree, core, anchor, tag, &text)?;
                return Ok(());
            }
        }
        value(format!("cannot create <{tag}>: no known anchor present"))
    }

    /// `TrackItem.in_point`: source ticks (raw / speed, truncated).
    pub fn in_point(&self, item: TrackItemId) -> Result<Time> {
        Ok(Time::new(self.source_ticks(item, "InPoint")?))
    }

    /// `TrackItem.in_point = value`: stores the raw value that reads back as `value`.
    pub fn set_in_point(&mut self, item: TrackItemId, value: Time) -> Result<()> {
        self.write_source_ticks(item, "InPoint", value.ticks, &["ClipID"])
    }

    /// `TrackItem.out_point`.
    pub fn out_point(&self, item: TrackItemId) -> Result<Time> {
        Ok(Time::new(self.source_ticks(item, "OutPoint")?))
    }

    /// `TrackItem.out_point = value`.
    pub fn set_out_point(&mut self, item: TrackItemId, value: Time) -> Result<()> {
        self.write_source_ticks(item, "OutPoint", value.ticks, &["InPoint", "ClipID"])
    }

    /// `TrackItem.name` (stored on the SubClip).
    pub fn track_item_name(&self, item: TrackItemId) -> Result<String> {
        let subclip = self.track_item(item).subclip;
        match self.doc.tree.child_by_tag(subclip, "Name") {
            Some(e) => Ok(self.doc.tree[e].text_or_empty().to_string()),
            None => value("missing <Name> element"),
        }
    }

    /// `TrackItem.name = value`.
    pub fn set_track_item_name(&mut self, item: TrackItemId, name: &str) -> Result<()> {
        validate_string(name)?;
        let subclip = self.track_item(item).subclip;
        let Some(e) = self.doc.tree.child_by_tag(subclip, "Name") else {
            return value("cannot set name: <Name> is absent and its creation anchor is not defined for this field");
        };
        self.doc.tree.set_text(e, Some(name));
        Ok(())
    }

    /// `TrackItem.project_item`: the source item, through the SubClip's master-clip UID.
    pub fn track_item_source(&self, item: TrackItemId) -> Option<ItemId> {
        let subclip = self.track_item(item).subclip;
        let reference = self.doc.tree.child_by_tag(subclip, "MasterClip")?;
        let uid = self.doc.tree.get(reference, "ObjectURef")?;
        self.item_by_master_uid(uid)
    }

    /// `TrackItem.media_type`.
    pub fn track_item_media_type(&self, item: TrackItemId) -> super::MediaType {
        self.track(self.track_item(item).track).media_type
    }
}

fn value_err<T>(message: &str) -> Result<T> {
    value(message)
}

/// Python `int(raw / speed)`: float division, truncated toward zero. A zero speed raises
/// like Python's `ZeroDivisionError`, a NaN or infinite quotient like `int()` does.
pub fn py_div_trunc(raw: i64, speed: f64) -> Result<i64> {
    if speed == 0.0 {
        return value("float division by zero");
    }
    py_float_to_int(raw as f64 / speed)
}

/// Python `int(x)` for a float.
pub fn py_float_to_int(x: f64) -> Result<i64> {
    if x.is_nan() {
        return value("cannot convert float NaN to integer");
    }
    if x.is_infinite() {
        return value("cannot convert float infinity to integer");
    }
    let t = x.trunc();
    const LIMIT: f64 = 9_223_372_036_854_775_808.0; // 2^63
    if !(-LIMIT..LIMIT).contains(&t) {
        return value(format!("integer out of range: {t}"));
    }
    Ok(t as i64)
}

/// Python `round(x)` for a float (half to even).
pub fn py_round(x: f64) -> Result<i64> {
    if !x.is_finite() {
        return py_float_to_int(x);
    }
    py_float_to_int(x.round_ties_even())
}

/// Python `float(text)` for the decimal strings Premiere stores.
pub(crate) fn py_float(text: &str) -> Result<f64> {
    crate::pystr::py_strip(text)
        .parse::<f64>()
        .map_err(|_| Error::Value(format!("could not convert string to float: {}", crate::convert::model::py_str_repr(text))))
}
