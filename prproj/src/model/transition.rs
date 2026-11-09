//! The `Transition` model (port of `py_premiere/models/transition.py`), read-only. A transition
//! is its top-level `VideoTransitionTrackItem` / `AudioTransitionTrackItem` object
//! ([`NodeId`], as [`Project::track_transitions`] lists them).

use super::project::py_int;
use super::Project;
use crate::build::fmt::fmt_f;
use crate::build::time::Time;
use crate::convert::model::py_str_repr;
use crate::error::Result;
use crate::xml::{NodeId, Tree};

use super::color::Color;

fn text(tree: &Tree, element: NodeId, path: &str) -> Result<String> {
    Ok(tree.findtext(element, path)?.unwrap_or_default())
}

fn int_text(tree: &Tree, element: NodeId, path: &str) -> Result<i64> {
    let t = text(tree, element, path)?;
    if t.is_empty() {
        Ok(0)
    } else {
        py_int(&t)
    }
}

/// `Transition.name`: the display name.
pub fn transition_name(tree: &Tree, transition: NodeId) -> Result<String> {
    text(tree, transition, "TransitionTrackItem/DisplayName")
}

/// `Transition.match_name`.
pub fn transition_match_name(tree: &Tree, transition: NodeId) -> Result<String> {
    text(tree, transition, "TransitionTrackItem/MatchName")
}

/// `Transition.cut_point_offset`: how much lies before the cut (`Alignment`).
pub fn transition_cut_point_offset(tree: &Tree, transition: NodeId) -> Result<Time> {
    Ok(Time::new(int_text(tree, transition, "TransitionTrackItem/Alignment")?))
}

/// `Transition.start` (0 when elided).
pub fn transition_start(tree: &Tree, transition: NodeId) -> Result<Time> {
    Ok(Time::new(int_text(tree, transition, "TransitionTrackItem/TrackItem/Start")?))
}

/// `Transition.end`.
pub fn transition_end(tree: &Tree, transition: NodeId) -> Result<Time> {
    Ok(Time::new(int_text(tree, transition, "TransitionTrackItem/TrackItem/End")?))
}

/// `Transition.cut_point`: `start + cut_point_offset`.
pub fn transition_cut_point(tree: &Tree, transition: NodeId) -> Result<Time> {
    Ok(transition_start(tree, transition)? + transition_cut_point_offset(tree, transition)?)
}

/// `Transition.duration`: `end - start`.
pub fn transition_duration(tree: &Tree, transition: NodeId) -> Result<Time> {
    Ok(transition_end(tree, transition)? - transition_start(tree, transition)?)
}

/// `Transition.border_width`.
pub fn transition_border_width(tree: &Tree, transition: NodeId) -> Result<i64> {
    int_text(tree, transition, "BorderWidth")
}

/// `Transition.border_color`: the stored `r,g,b` triple, `None` without a border.
pub fn transition_border_color(tree: &Tree, transition: NodeId) -> Result<Option<Color>> {
    let stored = text(tree, transition, "BorderColor")?;
    if stored.is_empty() {
        return Ok(None);
    }
    // Python unpacks a generator: values are converted as they are consumed (at most four).
    let parts: Vec<i64> = stored.split(',').take(4).map(py_int).collect::<Result<_>>()?;
    if parts.len() != 3 {
        let message = if parts.len() > 3 {
            "too many values to unpack (expected 3)".to_string()
        } else {
            format!("not enough values to unpack (expected 3, got {})", parts.len())
        };
        return Err(crate::Error::Value(message));
    }
    Ok(Some(Color::rgb(parts[0], parts[1], parts[2])))
}

/// `Transition.is_reversed`.
pub fn transition_is_reversed(tree: &Tree, transition: NodeId) -> Result<bool> {
    Ok(tree.findtext(transition, "Reverse")?.as_deref() == Some("true"))
}

/// `Transition.direction`: the raw stored code.
pub fn transition_direction(tree: &Tree, transition: NodeId) -> Result<i64> {
    int_text(tree, transition, "Direction")
}

/// `Transition.anti_alias_quality`.
pub fn transition_anti_alias_quality(tree: &Tree, transition: NodeId) -> Result<i64> {
    int_text(tree, transition, "AntiAliasQuality")
}

/// `Transition.has_incoming_clip`.
pub fn transition_has_incoming_clip(tree: &Tree, transition: NodeId) -> Result<bool> {
    Ok(text(tree, transition, "TransitionTrackItem/HasIncomingClip")? == "true")
}

/// `Transition.has_outgoing_clip`.
pub fn transition_has_outgoing_clip(tree: &Tree, transition: NodeId) -> Result<bool> {
    Ok(text(tree, transition, "TransitionTrackItem/HasOutgoingClip")? == "true")
}

/// `repr(transition)`.
pub fn transition_repr(tree: &Tree, transition: NodeId) -> Result<String> {
    Ok(format!(
        "Transition(name={}, start={}s, end={}s)",
        py_str_repr(&transition_name(tree, transition)?),
        fmt_f(transition_start(tree, transition)?.seconds(), 3),
        fmt_f(transition_end(tree, transition)?.seconds(), 3)
    ))
}

impl Project {
    /// `Track.transitions` as a [`super::named_list::NamedList`] keyed by `name` and `match_name`.
    pub fn transitions_named(&self, track: super::TrackId) -> super::named_list::NamedList<NodeId> {
        let items = self.track_transitions(track).to_vec();
        let keys = items
            .iter()
            .map(|&t| vec![transition_name(&self.doc.tree, t), transition_match_name(&self.doc.tree, t)])
            .collect();
        super::named_list::NamedList::new(items, keys)
    }
}
