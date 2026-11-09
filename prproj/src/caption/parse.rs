//! Caption track parsing: port of `parsers/caption.py`.

use std::collections::HashMap;

use super::flat::decode_caption_text;
use super::model::{CaptionData, CaptionId, CaptionTrackData, CaptionTrackId};
use super::PyResult;
use crate::build::time::Time;
use crate::model::project::py_int;
use crate::model::{Project, SequenceId};
use crate::xml::NodeId;

fn int_or_zero(text: Option<String>) -> PyResult<i64> {
    match text.filter(|t| !t.is_empty()) {
        None => Ok(0),
        Some(t) => Ok(py_int(&t)?),
    }
}

impl Project {
    /// `_text_data`: a caption owns its text through BlockVector -> Block -> FormattedTextData.
    fn caption_owner_text_data(&self, owner: NodeId) -> PyResult<Option<NodeId>> {
        let tree = &self.doc.tree;
        let Some(item) = tree.find1(owner, "BlockVector/BlockVectorItem") else { return Ok(None) };
        let block = self.doc.resolve(item)?;
        Ok(tree.child_by_tag(block, "FormattedTextData"))
    }

    /// `_hash`.
    fn caption_owner_hash(&self, owner: NodeId) -> PyResult<Option<String>> {
        Ok(self.caption_owner_text_data(owner)?.and_then(|d| self.doc.tree.get(d, "BinaryHash").map(str::to_string)))
    }

    /// `_caption_text`.
    fn caption_owner_text(&mut self, owner: NodeId) -> PyResult<String> {
        let Some(data) = self.caption_owner_text_data(owner)? else { return Ok(String::new()) };
        match self.doc.payload(data)? {
            None => Ok(String::new()),
            Some(payload) => decode_caption_text(&payload),
        }
    }

    /// `_source_times`: the imported caption stream's own times, keyed by block hash.
    fn caption_source_times(&self) -> PyResult<HashMap<String, (Time, Time)>> {
        let mut times = HashMap::new();
        let tree = &self.doc.tree;
        for caption in tree.iter_tag(self.doc.root, "Caption") {
            let Some(hash) = self.caption_owner_hash(caption)? else { continue };
            if times.contains_key(&hash) {
                continue;
            }
            times.insert(
                hash,
                (
                    Time::new(int_or_zero(tree.findtext(caption, "TimeStart")?)?),
                    Time::new(int_or_zero(tree.findtext(caption, "TimeEnd")?)?),
                ),
            );
        }
        Ok(times)
    }

    /// `parse_caption_tracks(document, sequence, group)`: build the caption tracks of a
    /// sequence's data track group and make them the sequence's `caption_tracks`.
    pub fn parse_caption_tracks(&mut self, sequence: SequenceId, group: NodeId) -> PyResult<Vec<CaptionTrackId>> {
        let track_refs = self.doc.tree.findall(group, "TrackGroup/Tracks/Track")?;
        if track_refs.is_empty() {
            self.captions.by_sequence.insert(sequence, Vec::new());
            return Ok(Vec::new());
        }
        let sources = self.caption_source_times()?;
        let mut tracks = Vec::new();
        for track_ref in track_refs {
            let element = self.doc.resolve(track_ref)?;
            let track = CaptionTrackId(self.captions.tracks.len());
            self.captions.tracks.push(CaptionTrackData { element, sequence, captions: Vec::new() });
            for item_ref in self.doc.tree.findall(element, "DataClipTrack/ClipTrack/ClipItems/TrackItems/TrackItem")? {
                let item = self.doc.resolve(item_ref)?;
                let text = self.caption_owner_text(item)?;
                let hash = self.caption_owner_hash(item)?.unwrap_or_default();
                let (source_start, source_end) = sources.get(&hash).copied().unwrap_or((Time::new(0), Time::new(0)));
                let caption = CaptionId(self.captions.captions.len());
                self.captions.captions.push(CaptionData { element: item, text, track, source_start, source_end });
                self.captions.tracks[track.0].captions.push(caption);
            }
            tracks.push(track);
        }
        self.captions.by_sequence.insert(sequence, tracks.clone());
        Ok(tracks)
    }
}
