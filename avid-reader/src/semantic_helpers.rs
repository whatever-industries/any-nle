//! Typed semantic queries over the decoded AVB graph.
//!
//! Source behavior: pinned py-avb `components.py` (`Component.media_kind`,
//! `SourceClip.mob`/`track`), `trackgroups.py` (`Track.media_kind`,
//! `Composition.mob_type`/`usage`) at ff64e06b60077ecabe5db3fef239898b08363898.
//! These queries do not infer media availability or evaluate timecode tracks.

use crate::bin_nav::{BinNavigator, BinQueryError};
use crate::file::AvbFile;
use crate::objects::{Node, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticError {
    WrongClass([u8; 4]),
    MissingField(&'static str),
    InvalidMobType(i64),
    InvalidTimecodeFps(i64),
    InvalidTimecodePosition(i64),
    Bin(BinQueryError),
}

impl From<BinQueryError> for SemanticError {
    fn from(value: BinQueryError) -> Self {
        Self::Bin(value)
    }
}

fn int_field(node: &Node, field: &'static str) -> Result<i64, SemanticError> {
    node.get(field)
        .and_then(Value::as_int)
        .ok_or(SemanticError::MissingField(field))
}

/// py-avb's `Component.media_kind` labels, including its `unknownN` fallback.
pub fn media_kind(node: &Node) -> Result<Option<String>, SemanticError> {
    let id = int_field(node, "media_kind_id")?;
    Ok(match id {
        0 => None,
        1 => Some("picture".into()),
        2 => Some("sound".into()),
        3 => Some("timecode".into()),
        4 => Some("edgecode".into()),
        5 => Some("attribute".into()),
        6 => Some("effectdata".into()),
        7 => Some("DescriptiveMetadata".into()),
        16 => Some("DataEssenceTrack".into()),
        id => Some(format!("unknown{id}")),
    })
}

/// py-avb's `Track.media_kind`: absent component means `None`.
pub fn track_media_kind(file: &AvbFile, track: &Node) -> Result<Option<String>, SemanticError> {
    let Some(component_id) = track.get("component").and_then(Value::as_index) else {
        return Ok(None);
    };
    let component = file
        .node(component_id)
        .ok_or(SemanticError::MissingField("component"))?;
    media_kind(component)
}

/// py-avb's `Composition.mob_type` raises for unknown values.
pub fn mob_type(node: &Node) -> Result<&'static str, SemanticError> {
    if node.class != *b"CMPO" {
        return Err(SemanticError::WrongClass(node.class));
    }
    match int_field(node, "mob_type_id")? {
        1 => Ok("CompositionMob"),
        2 => Ok("MasterMob"),
        3 => Ok("SourceMob"),
        n => Err(SemanticError::InvalidMobType(n)),
    }
}

/// py-avb's `Composition.usage` (`unknown` for all other nonzero codes).
pub fn usage(node: &Node) -> Result<Option<&'static str>, SemanticError> {
    if node.class != *b"CMPO" {
        return Err(SemanticError::WrongClass(node.class));
    }
    Ok(match int_field(node, "usage_code")? {
        0 => None,
        1 => Some("precompute"),
        2 => Some("subclip"),
        3 => Some("effect"),
        4 => Some("group"),
        5 => Some("groupoofter"),
        6 => Some("motion"),
        7 => Some("mastermob"),
        9 => Some("precompute_file"),
        _ => Some("unknown"),
    })
}

/// One py-avb-style source-mob lookup: only mobs actually listed in this bin
/// participate; duplicate MobIDs select the last bin item.
pub fn source_mob<'a>(
    file: &'a AvbFile,
    source: &Node,
) -> Result<Option<(u32, &'a Node)>, SemanticError> {
    if source.class != *b"SCLP" {
        return Err(SemanticError::WrongClass(source.class));
    }
    let Some(Value::MobId(mob_id)) = source.get("mob_id") else {
        return Ok(None);
    };
    Ok(BinNavigator::new(file)?
        .find_by_mob_id(mob_id)?
        .map(|found| (found.object_index, found.mob)))
}

/// A source clip's first matching track, following py-avb's index + component
/// media-kind comparison. `track_position` is zero-based in the target mob.
pub struct SourceTrack<'a> {
    pub mob_index: u32,
    pub mob: &'a Node,
    pub track_position: usize,
    pub track: &'a Node,
    pub component_index: u32,
    pub component: &'a Node,
}

pub fn source_track<'a>(
    file: &'a AvbFile,
    source: &Node,
) -> Result<Option<SourceTrack<'a>>, SemanticError> {
    let Some((mob_index, mob)) = source_mob(file, source)? else {
        return Ok(None);
    };
    let source_track_id = int_field(source, "track_id")?;
    let source_kind = media_kind(source)?;
    let Some(Value::List(tracks)) = mob.get("tracks") else {
        return Err(SemanticError::MissingField("tracks"));
    };
    for (track_position, value) in tracks.iter().enumerate() {
        let Value::Node(track) = value else {
            continue;
        };
        if track.get("index").and_then(Value::as_int) != Some(source_track_id) {
            continue;
        }
        let Some(component_index) = track.get("component").and_then(Value::as_index) else {
            continue;
        };
        let Some(component) = file.node(component_index) else {
            continue;
        };
        if media_kind(component)? == source_kind {
            return Ok(Some(SourceTrack {
                mob_index,
                mob,
                track_position,
                track,
                component_index,
                component,
            }));
        }
    }
    Ok(None)
}

/// Stored TCCP fields. `flags` is intentionally uninterpreted: py-avb labels
/// its drop-frame meaning unknown, and its Timecode class has no arithmetic API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimecodeFields {
    pub start: i64,
    pub length: i64,
    pub fps: i64,
    pub flags: i64,
}

pub fn timecode_fields(node: &Node) -> Result<TimecodeFields, SemanticError> {
    if node.class != *b"TCCP" {
        return Err(SemanticError::WrongClass(node.class));
    }
    Ok(TimecodeFields {
        start: int_field(node, "start")?,
        length: int_field(node, "length")?,
        fps: int_field(node, "fps")?,
        flags: int_field(node, "flags")?,
    })
}

impl TimecodeFields {
    /// Convenience for a known non-drop segment. Position is relative to this
    /// TCCP and must fall inside it. This is not an Aux TC offset evaluator.
    pub fn non_drop_frame_at(self, position: i64) -> Result<i64, SemanticError> {
        if self.fps <= 0 {
            return Err(SemanticError::InvalidTimecodeFps(self.fps));
        }
        if position < 0 || position >= self.length {
            return Err(SemanticError::InvalidTimecodePosition(position));
        }
        self.start
            .checked_add(position)
            .ok_or(SemanticError::InvalidTimecodePosition(position))
    }
}
