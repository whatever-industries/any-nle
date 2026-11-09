//! The `Marker` model (port of `py_premiere/models/marker.py` reads and setters, plus
//! `Sequence.remove_marker` / `ProjectItem.remove_marker`).
//!
//! A marker is its top-level `Marker` object ([`NodeId`]); its fields live in the `DVAMarker`
//! JSON blob, re-read on every access and rewritten compact with sorted keys on every write.
//! Creating markers is [`crate::build::marker`] ([`Project::add_sequence_marker`],
//! [`Project::add_item_marker`]).

use super::validators::validate_string;
use super::{ItemId, Project, SequenceId};
use crate::build::fmt::fmt_f;
use crate::build::marker::{blob_data, color_index, detach_marker, set_color_index, write_blob, Json};
use crate::build::time::Time;
use crate::convert::model::py_str_repr;
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::remove_child;
use crate::xml::{NodeId, Tree};

fn get_str(tree: &Tree, marker: NodeId, key: &str) -> Result<String> {
    Ok(match blob_data(tree, marker)?.get(key) {
        Some(Json::Str(s)) => s.clone(),
        _ => String::new(),
    })
}

fn write(tree: &mut Tree, marker: NodeId, key: &str, v: Json) -> Result<()> {
    let mut data = blob_data(tree, marker)?;
    if let Json::Obj(pairs) = &mut data {
        match pairs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = v,
            None => pairs.push((key.to_string(), v)),
        }
    }
    write_blob(tree, marker, data)
}

/// Python `int(x)` of a JSON value.
fn json_int(v: &Json) -> Result<i64> {
    match v {
        Json::Int(i) => i64::try_from(*i).map_err(|_| Error::Value("integer out of range".into())),
        Json::Float(f) => super::track_item::py_float_to_int(*f),
        Json::Bool(b) => Ok(*b as i64),
        Json::Str(s) => super::project::py_int(s),
        Json::Null => Err(Error::Type(
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'".into(),
        )),
        Json::Arr(_) => Err(Error::Type("int() argument must be a string, a bytes-like object or a real number, not 'list'".into())),
        Json::Obj(_) => Err(Error::Type("int() argument must be a string, a bytes-like object or a real number, not 'dict'".into())),
    }
}

fn ticks_json(ticks: i64) -> Json {
    Json::obj(vec![("ticks", Json::Int(ticks as i128))])
}

/// `Marker.name`.
pub fn marker_name(tree: &Tree, marker: NodeId) -> Result<String> {
    get_str(tree, marker, "mName")
}

/// `Marker.name = value`.
pub fn set_marker_name(tree: &mut Tree, marker: NodeId, name: &str) -> Result<()> {
    validate_string(name)?;
    write(tree, marker, "mName", Json::Str(name.to_string()))
}

/// `Marker.comments`.
pub fn marker_comments(tree: &Tree, marker: NodeId) -> Result<String> {
    get_str(tree, marker, "mComment")
}

/// `Marker.comments = value`.
pub fn set_marker_comments(tree: &mut Tree, marker: NodeId, comments: &str) -> Result<()> {
    validate_string(comments)?;
    write(tree, marker, "mComment", Json::Str(comments.to_string()))
}

/// `Marker.start`.
pub fn marker_start(tree: &Tree, marker: NodeId) -> Result<Time> {
    let data = blob_data(tree, marker)?;
    let ticks = match data.get("mStartTime") {
        None => 0,
        Some(Json::Obj(_)) => match data.get("mStartTime").and_then(|s| s.get("ticks")) {
            None => 0,
            Some(t) => json_int(t)?,
        },
        Some(other) => {
            let name = match other {
                Json::Str(_) => "str",
                Json::Int(_) => "int",
                Json::Float(_) => "float",
                Json::Bool(_) => "bool",
                Json::Arr(_) => "list",
                _ => "NoneType",
            };
            return Err(Error::Attribute(format!("'{name}' object has no attribute 'get'")));
        }
    };
    Ok(Time::new(ticks))
}

/// `Marker.start = value`.
pub fn set_marker_start(tree: &mut Tree, marker: NodeId, start: Time) -> Result<()> {
    write(tree, marker, "mStartTime", ticks_json(start.ticks))
}

/// `Marker.end`: `start` plus the stored duration (`start` for a point marker).
pub fn marker_end(tree: &Tree, marker: NodeId) -> Result<Time> {
    let data = blob_data(tree, marker)?;
    if let Some(duration @ Json::Obj(_)) = data.get("mDuration") {
        if let Some(t) = duration.get("ticks") {
            let ticks = json_int(t)?;
            return Ok(marker_start(tree, marker)? + Time::new(ticks));
        }
    }
    marker_start(tree, marker)
}

/// `Marker.end = value` (refuses an end before the start).
pub fn set_marker_end(tree: &mut Tree, marker: NodeId, end: Time) -> Result<()> {
    let start = marker_start(tree, marker)?;
    if end.ticks < start.ticks {
        return value("end must not precede start");
    }
    let start = marker_start(tree, marker)?;
    write(tree, marker, "mDuration", ticks_json(end.ticks - start.ticks))
}

/// `Marker.type`.
pub fn marker_type(tree: &Tree, marker: NodeId) -> Result<String> {
    get_str(tree, marker, "mType")
}

/// `Marker.guid`.
pub fn marker_guid(tree: &Tree, marker: NodeId) -> Result<String> {
    get_str(tree, marker, "mMarkerID")
}

/// `Marker.color_index`: the palette index (type default when uncoloured; `None` for a
/// stored colour outside the palette).
pub fn marker_color_index(tree: &Tree, marker: NodeId) -> Result<Option<usize>> {
    color_index(tree, marker)
}

/// `Marker.color_index = index` (draws a UUID when the marker has no colour cue yet).
pub fn set_marker_color_index(tree: &mut Tree, marker: NodeId, index: i64, uuids: &mut dyn UuidSource) -> Result<()> {
    set_color_index(tree, marker, index, uuids)
}

/// `Marker.web_link_url`.
pub fn marker_web_link_url(tree: &Tree, marker: NodeId) -> Result<String> {
    get_str(tree, marker, "mLocation")
}

/// `Marker.web_link_frame_target`.
pub fn marker_web_link_frame_target(tree: &Tree, marker: NodeId) -> Result<String> {
    get_str(tree, marker, "mTarget")
}

/// `Marker.set_type_as_comment()` (`"Comment"`), `set_type_as_chapter()` (`"Chapter"`),
/// `set_type_as_segmentation()` (`"Segmentation"`): rewrite `mType`.
pub fn set_marker_type(tree: &mut Tree, marker: NodeId, marker_type: &str) -> Result<()> {
    write(tree, marker, "mType", Json::Str(marker_type.to_string()))
}

/// `Marker.set_type_as_web_link(url, frame_target)`.
pub fn set_marker_type_as_web_link(tree: &mut Tree, marker: NodeId, url: &str, frame_target: &str) -> Result<()> {
    validate_string(url)?;
    validate_string(frame_target)?;
    write(tree, marker, "mType", Json::Str("WebLink".into()))?;
    write(tree, marker, "mLocation", Json::Str(url.to_string()))?;
    write(tree, marker, "mTarget", Json::Str(frame_target.to_string()))
}

/// `repr(marker)`.
pub fn marker_repr(tree: &Tree, marker: NodeId) -> Result<String> {
    Ok(format!(
        "Marker(name={}, start={}s)",
        py_str_repr(&marker_name(tree, marker)?),
        fmt_f(marker_start(tree, marker)?.seconds(), 3)
    ))
}

impl Project {
    /// `Sequence.remove_marker(marker)`: removing the last one drops the whole collection and
    /// the sequence's `MarkerOwner`.
    pub fn remove_sequence_marker(&mut self, sequence: SequenceId, marker: NodeId) -> Result<()> {
        if !self.sequence(sequence).markers.contains(&marker) {
            return value("marker does not belong to this sequence");
        }
        let element = self.sequence(sequence).element;
        let Some(reference) = self.doc.tree.find1(element, "MarkerOwner/Markers") else {
            return value("sequence has no marker list");
        };
        let collection = self.doc.resolve(reference)?;
        let Some(inner) = self.doc.tree.child_by_tag(collection, "Markers") else {
            return value("marker collection has no inner list");
        };
        detach_marker(&mut self.doc, inner, marker)?;
        self.sequence_mut(sequence).markers.retain(|&m| m != marker);
        if self.sequence(sequence).markers.is_empty() {
            if let Some(owner) = self.doc.tree.child_by_tag(element, "MarkerOwner") {
                remove_child(&mut self.doc.tree, element, owner)?;
            }
            self.doc.remove_object(collection)?;
        }
        Ok(())
    }

    /// `ProjectItem.markers`: the clip-marker objects.
    pub fn item_markers(&self, item: ItemId) -> &[NodeId] {
        &self.item(item).markers
    }

    /// `ProjectItem.remove_marker(marker)`.
    pub fn remove_item_marker(&mut self, item: ItemId, marker: NodeId) -> Result<()> {
        if !self.item(item).markers.contains(&marker) {
            return value("marker does not belong to this item");
        }
        let core = self.own_clip_core(item)?;
        let Some(reference) = core.and_then(|c| self.doc.tree.find1(c, "MarkerOwner/Markers")) else {
            return value("master clip has no marker collection");
        };
        let collection = self.doc.resolve(reference)?;
        let Some(inner) = self.doc.tree.child_by_tag(collection, "Markers") else {
            return value("marker collection has no inner list");
        };
        detach_marker(&mut self.doc, inner, marker)?;
        self.item_mut(item).markers.retain(|&m| m != marker);
        Ok(())
    }
}
