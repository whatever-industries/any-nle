//! Bounded queries over stored curves and locator/descriptor fields.
//!
//! Curve arithmetic delegates to `param_eval`, a port of pinned py-avb
//! ff64e06b60077ecabe5db3fef239898b08363898. A curve's presence does not
//! establish that it drives an Avid effect, nor that the integrated samples
//! match Media Composer's rendered Timewarp.

use crate::file::AvbFile;
use crate::io::{MobId, Uuid};
use crate::objects::{Node, Value};
use crate::param_eval::{self, ParamEvalError};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub enum ExtraError {
    MissingObject(u32),
    WrongClass {
        expected: &'static str,
        actual: [u8; 4],
    },
    MissingField(&'static str),
    InvalidField(&'static str),
    DanglingReference(u32),
    ReferenceCycle(u32),
    Curve(ParamEvalError),
}

impl From<ParamEvalError> for ExtraError {
    fn from(value: ParamEvalError) -> Self {
        Self::Curve(value)
    }
}

pub fn curve_value_at(file: &AvbFile, id: u32, time: f64) -> Result<f64, ExtraError> {
    let node = file.node(id).ok_or(ExtraError::MissingObject(id))?;
    Ok(param_eval::value_at(node, time)?)
}

/// Evaluate the stored PRCL as a speed curve. The caller chooses the bounded
/// edit-unit interval. There is no implicit effect/track binding here.
pub fn speed_offset_map(
    file: &AvbFile,
    id: u32,
    start: i64,
    end: i64,
) -> Result<Vec<(f64, f64)>, ExtraError> {
    let node = file.node(id).ok_or(ExtraError::MissingObject(id))?;
    Ok(param_eval::integrate(node, start, Some(end))?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocatorKind {
    MacFile,
    WinFile,
    Url,
    Msm,
}

/// Literal stored locator strings. Multiple path variants are independent
/// fields; no precedence, normalization, existence check, or resolution is
/// implied. A URL locator has no path field in pinned py-avb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocatorPaths<'a> {
    pub kind: LocatorKind,
    pub path: Option<&'a str>,
    pub path_posix: Option<&'a str>,
    pub path_utf8: Option<&'a str>,
    pub path2_utf8: Option<&'a str>,
    /// Legacy MSML volume name. Keep it distinct from the UTF-8 extension.
    pub last_known_volume: Option<&'a str>,
    pub last_known_volume_utf8: Option<&'a str>,
}

pub fn locator_paths(node: &Node) -> Result<LocatorPaths<'_>, ExtraError> {
    let kind = match &node.class {
        b"FILE" => LocatorKind::MacFile,
        b"WINF" => LocatorKind::WinFile,
        b"URLL" => LocatorKind::Url,
        b"MSML" => LocatorKind::Msm,
        _ => {
            return Err(ExtraError::WrongClass {
                expected: "FILE/WINF/URLL/MSML",
                actual: node.class,
            })
        }
    };
    let str_field = |name| node.get(name).and_then(Value::as_str);
    if matches!(kind, LocatorKind::MacFile | LocatorKind::WinFile) && str_field("path").is_none() {
        return Err(ExtraError::MissingField("path"));
    }
    Ok(LocatorPaths {
        kind,
        path: str_field("path"),
        path_posix: str_field("path_posix"),
        path_utf8: str_field("path_utf8"),
        path2_utf8: str_field("path2_utf8"),
        last_known_volume: str_field("last_known_volume"),
        last_known_volume_utf8: str_field("last_known_volume_utf8"),
    })
}

pub struct DescriptorLocator<'a> {
    pub descriptor: &'a Node,
    pub locator_id: Option<u32>,
    pub locator: Option<&'a Node>,
}

/// Follow only the descriptor's literal `locator` reference. A null locator
/// is valid; a dangling reference is an error. This does not search media.
pub fn descriptor_locator(
    file: &AvbFile,
    descriptor_id: u32,
) -> Result<DescriptorLocator<'_>, ExtraError> {
    let descriptor = file
        .node(descriptor_id)
        .ok_or(ExtraError::MissingObject(descriptor_id))?;
    media_descriptor_record(descriptor)?;
    let value = descriptor
        .get("locator")
        .ok_or(ExtraError::MissingField("locator"))?;
    let locator_id = match value {
        Value::Null => None,
        Value::Ref(id) => Some(*id),
        _ => return Err(ExtraError::InvalidField("locator")),
    };
    let locator = locator_id
        .map(|id| file.node(id).ok_or(ExtraError::DanglingReference(id)))
        .transpose()?;
    if let Some(node) = locator {
        locator_paths(node)?;
    }
    Ok(DescriptorLocator {
        descriptor,
        locator_id,
        locator,
    })
}

fn required_int(node: &Node, name: &'static str) -> Result<i64, ExtraError> {
    match node.get(name) {
        Some(Value::Int(value)) => Ok(*value),
        None => Err(ExtraError::MissingField(name)),
        _ => Err(ExtraError::InvalidField(name)),
    }
}

fn required_float(node: &Node, name: &'static str) -> Result<f64, ExtraError> {
    match node.get(name) {
        Some(Value::Float(value)) if value.is_finite() => Ok(*value),
        None => Err(ExtraError::MissingField(name)),
        _ => Err(ExtraError::InvalidField(name)),
    }
}

fn required_bool(node: &Node, name: &'static str) -> Result<bool, ExtraError> {
    match node.get(name) {
        Some(Value::Bool(value)) => Ok(*value),
        None => Err(ExtraError::MissingField(name)),
        _ => Err(ExtraError::InvalidField(name)),
    }
}

fn optional_ref(node: &Node, name: &'static str) -> Result<Option<u32>, ExtraError> {
    match node.get(name) {
        Some(Value::Ref(id)) => Ok(Some(*id)),
        Some(Value::Null) => Ok(None),
        None => Err(ExtraError::MissingField(name)),
        _ => Err(ExtraError::InvalidField(name)),
    }
}

/// Stored SourceClip identity and position. `start_time` is a source offset;
/// this helper does not convert between edit rates or find media on disk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceClipRecord {
    pub mob_id: Option<MobId>,
    pub track_id: i64,
    pub start_time: i64,
    pub length: i64,
    pub edit_rate: f64,
    pub media_kind_id: i64,
}

pub fn source_clip_record(node: &Node) -> Result<SourceClipRecord, ExtraError> {
    if node.class != *b"SCLP" {
        return Err(ExtraError::WrongClass {
            expected: "SCLP",
            actual: node.class,
        });
    }
    let mob_id = match node.get("mob_id") {
        Some(Value::MobId(id)) => Some(*id),
        None => None, // optional SCLP extension absent in pinned py-avb
        _ => return Err(ExtraError::InvalidField("mob_id")),
    };
    Ok(SourceClipRecord {
        mob_id,
        track_id: required_int(node, "track_id")?,
        start_time: required_int(node, "start_time")?,
        length: required_int(node, "length")?,
        edit_rate: required_float(node, "edit_rate")?,
        media_kind_id: required_int(node, "media_kind_id")?,
    })
}

/// Literal TCCP values. `flags` are intentionally uninterpreted: py-avb's
/// source labels their drop-frame meaning unknown and exposes no TC evaluator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimecodeRecord {
    pub start: i64,
    pub length: i64,
    pub fps: i64,
    pub flags: i64,
    pub edit_rate: f64,
}

pub fn timecode_record(node: &Node) -> Result<TimecodeRecord, ExtraError> {
    if node.class != *b"TCCP" {
        return Err(ExtraError::WrongClass {
            expected: "TCCP",
            actual: node.class,
        });
    }
    Ok(TimecodeRecord {
        start: required_int(node, "start")?,
        length: required_int(node, "length")?,
        fps: required_int(node, "fps")?,
        flags: required_int(node, "flags")?,
        edit_rate: required_float(node, "edit_rate")?,
    })
}

const DESCRIPTOR_CLASSES: &[[u8; 4]] = &[
    *b"MDES", *b"MDTP", *b"MDFM", *b"MDNG", *b"MDFL", *b"MULD", *b"WAVE", *b"AIFC", *b"PCMA",
    *b"MPGA", *b"DIDD", *b"CDCI", *b"MPGI", *b"JPED", *b"RGBA", *b"DATD", *b"ANCD",
];

const FILE_DESCRIPTOR_CLASSES: &[[u8; 4]] = &[
    *b"MDFL", *b"MULD", *b"WAVE", *b"AIFC", *b"PCMA", *b"MPGA", *b"DIDD", *b"CDCI", *b"MPGI",
    *b"JPED", *b"RGBA", *b"DATD", *b"ANCD",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaFileFields {
    pub edit_rate: f64,
    pub length: i64,
    pub is_omfi: i64,
    pub data_offset: i64,
}

/// Common fields inherited from py-avb `MediaDescriptor`; file-specific fields
/// are present only on `MediaFileDescriptor` descendants. No codec or path
/// interpretation is inferred from these stored values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaDescriptorRecord {
    pub class: [u8; 4],
    pub mob_kind: i64,
    pub locator_id: Option<u32>,
    pub physical_media_id: Option<u32>,
    pub intermediate: bool,
    pub uuid: Option<Uuid>,
    pub file: Option<MediaFileFields>,
}

pub fn media_descriptor_record(node: &Node) -> Result<MediaDescriptorRecord, ExtraError> {
    if !DESCRIPTOR_CLASSES.contains(&node.class) {
        return Err(ExtraError::WrongClass {
            expected: "MediaDescriptor descendant",
            actual: node.class,
        });
    }
    let uuid = match node.get("uuid") {
        Some(Value::Uuid(id)) => Some(*id),
        None => None,
        _ => return Err(ExtraError::InvalidField("uuid")),
    };
    let file = if FILE_DESCRIPTOR_CLASSES.contains(&node.class) {
        Some(MediaFileFields {
            edit_rate: required_float(node, "edit_rate")?,
            length: required_int(node, "length")?,
            is_omfi: required_int(node, "is_omfi")?,
            data_offset: required_int(node, "data_offset")?,
        })
    } else {
        None
    };
    Ok(MediaDescriptorRecord {
        class: node.class,
        mob_kind: required_int(node, "mob_kind")?,
        locator_id: optional_ref(node, "locator")?,
        physical_media_id: optional_ref(node, "physical_media")?,
        intermediate: required_bool(node, "intermediate")?,
        uuid,
        file,
    })
}

/// Follow the stored `physical_media` links only. This exposes py-avb's
/// descriptor graph without inferring which media file, track, or path Avid
/// would choose. Cycles and dangling references are explicit errors.
pub fn physical_media_chain(file: &AvbFile, descriptor_id: u32) -> Result<Vec<u32>, ExtraError> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(descriptor_id);
    let mut current = descriptor_id;
    loop {
        let node = file
            .node(current)
            .ok_or(ExtraError::MissingObject(current))?;
        let record = media_descriptor_record(node)?;
        let Some(next) = record.physical_media_id else {
            return Ok(chain);
        };
        if !seen.insert(next) {
            return Err(ExtraError::ReferenceCycle(next));
        }
        if file.node(next).is_none() {
            return Err(ExtraError::DanglingReference(next));
        }
        chain.push(next);
        current = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::AvbDocument;

    #[test]
    fn locator_variants_remain_distinct() {
        let node = Node {
            class: *b"FILE",
            type_name: "MacFileLocator",
            fields: vec![
                ("path", Value::Str("mac:path".into())),
                ("path_posix", Value::Str("/Volumes/media.mov".into())),
                ("path_utf8", Value::Str("/Volumes/café.mov".into())),
            ],
        };
        let fields = locator_paths(&node).unwrap();
        assert_eq!(fields.path, Some("mac:path"));
        assert_eq!(fields.path_posix, Some("/Volumes/media.mov"));
        assert_eq!(fields.path_utf8, Some("/Volumes/café.mov"));
        assert_eq!(fields.path2_utf8, None);
    }

    #[test]
    fn invalid_locator_fails_closed() {
        let node = Node {
            class: *b"FILE",
            type_name: "MacFileLocator",
            fields: Vec::new(),
        };
        assert_eq!(locator_paths(&node), Err(ExtraError::MissingField("path")));
    }

    #[test]
    fn identity_helpers_reject_wrong_classes_and_nonfinite_rates() {
        let node = Node {
            class: *b"FILL",
            type_name: "Filler",
            fields: Vec::new(),
        };
        assert!(matches!(
            source_clip_record(&node),
            Err(ExtraError::WrongClass { .. })
        ));
        assert!(matches!(
            timecode_record(&node),
            Err(ExtraError::WrongClass { .. })
        ));
        assert!(matches!(
            media_descriptor_record(&node),
            Err(ExtraError::WrongClass { .. })
        ));
        let tc = Node {
            class: *b"TCCP",
            type_name: "Timecode",
            fields: vec![
                ("start", Value::Int(0)),
                ("length", Value::Int(1)),
                ("fps", Value::Int(24)),
                ("flags", Value::Int(0)),
                ("edit_rate", Value::Float(f64::NAN)),
            ],
        };
        assert_eq!(
            timecode_record(&tc),
            Err(ExtraError::InvalidField("edit_rate"))
        );
    }

    #[test]
    fn physical_media_cycle_is_bounded() {
        fn descriptor() -> Node {
            Node {
                class: *b"MDES",
                type_name: "MediaDescriptor",
                fields: vec![
                    ("mob_kind", Value::Int(0)),
                    ("locator", Value::Null),
                    ("intermediate", Value::Bool(false)),
                    ("physical_media", Value::Null),
                ],
            }
        }
        let mut doc = AvbDocument::new_empty_bin(1, "test", 0, false);
        let a = doc.add_object(descriptor()).unwrap();
        let b = doc.add_object(descriptor()).unwrap();
        doc.set_field(a, "physical_media", Value::Ref(b)).unwrap();
        doc.set_field(b, "physical_media", Value::Ref(a)).unwrap();
        let bytes = doc.to_bytes().unwrap();
        let file = AvbFile::parse(&bytes).unwrap();
        assert_eq!(
            physical_media_chain(&file, a),
            Err(ExtraError::ReferenceCycle(a))
        );
    }
}
