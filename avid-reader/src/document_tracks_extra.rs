//! Transactional edits to a composition's embedded Track list.
//!
//! Pinned py-avb exposes `TrackGroup.tracks` as an ordinary mutable list.
//! This typed Rust boundary preserves stable object IDs and checks the whole
//! document before accepting a list edit. It does not prune orphaned objects.

use crate::document::AvbDocument;
use crate::io::{Error, Result};
use crate::objects::{Node, Value};

fn fail(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}

pub trait DocumentTracksExt {
    /// Remove one track from a CMPO; its component remains in the document.
    fn remove_composition_track_at(&mut self, mob_id: u32, index: usize) -> Result<Node>;

    /// Reorder a CMPO track to its final zero-based position.
    fn move_composition_track(&mut self, mob_id: u32, from: usize, to: usize) -> Result<()>;
}

impl DocumentTracksExt for AvbDocument {
    fn remove_composition_track_at(&mut self, mob_id: u32, index: usize) -> Result<Node> {
        let original = tracks(self, mob_id)?;
        let item = original
            .get(index)
            .ok_or_else(|| fail(format!("track index {index} is out of range")))?;
        let removed = checked_track(self, item)?.clone();
        let mut edited = original.clone();
        edited.remove(index);
        commit_tracks(self, mob_id, original, edited)?;
        Ok(removed)
    }

    fn move_composition_track(&mut self, mob_id: u32, from: usize, to: usize) -> Result<()> {
        let original = tracks(self, mob_id)?;
        if from >= original.len() || to >= original.len() {
            return Err(fail(format!(
                "track move {from} -> {to} exceeds list length {}",
                original.len()
            )));
        }
        checked_track(self, &original[from])?;
        if from == to {
            return Ok(());
        }
        let mut edited = original.clone();
        let item = edited.remove(from);
        edited.insert(to, item);
        commit_tracks(self, mob_id, original, edited)
    }
}

fn tracks(doc: &AvbDocument, mob_id: u32) -> Result<Vec<Value>> {
    let mob = doc
        .node(mob_id)
        .ok_or_else(|| fail(format!("composition {mob_id} is missing or opaque")))?;
    if mob.class != *b"CMPO" {
        return Err(fail(format!("object {mob_id} is not a composition")));
    }
    match mob.get("tracks") {
        Some(Value::List(items)) => Ok(items.clone()),
        _ => Err(fail(format!(
            "composition {mob_id} has no embedded track list"
        ))),
    }
}

fn checked_track<'a>(doc: &AvbDocument, value: &'a Value) -> Result<&'a Node> {
    let Value::Node(track) = value else {
        return Err(fail("track is not an embedded node"));
    };
    if track.type_name != "Track" {
        return Err(fail("embedded track has the wrong type"));
    }
    if let Some(Value::Ref(id)) = track.get("component") {
        if doc.node(*id).is_none() {
            return Err(fail(format!("track component {id} is missing or opaque")));
        }
    }
    Ok(track)
}

fn commit_tracks(doc: &mut AvbDocument, id: u32, old: Vec<Value>, new: Vec<Value>) -> Result<()> {
    doc.set_field(id, "tracks", Value::List(new))?;
    if let Err(error) = doc.to_bytes() {
        doc.set_field(id, "tracks", Value::List(old))?;
        return Err(error);
    }
    Ok(())
}
