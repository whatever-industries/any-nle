//! Port of `converter/src/avid2prproj/markers.py`: clip labels, markers and the adjustment layers
//! that carry markers on empty Avid filler.
//!
//! Report notes go to a `&mut Vec<String>` (Python `report.notes`).

use super::colors;
use super::model::{py_str_repr, Marker, Options};
use crate::build::marker::{ensure_inner_list, set_color_index, MarkerSpec};
use crate::build::time::Time;
use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::model::item::{add_item_ref, child_items};
use crate::model::{ItemData, ItemId, ItemType, Project, SequenceId};

pub const LABEL_PREFIX: &str = "BE.Prefs.LabelColors.";

/// One adjustment layer as Premiere 26.3 saved it (py_premiere fixture 22_adjustment_layer):
/// placeholders `{name}`, `{timebase}`, `{width}`, `{height}`, `{timecode_format}`,
/// `{out_point}`, `{clip_id}`.
const ADJUSTMENT_LAYER: &str = include_str!("../../data/adjustment_layer.xml");

/// `set_label`: Premiere label `index` on a bin item: the panel stamp and the label its timeline
/// clips copy from the master's template clips (`asl.clip.label.*`).
pub fn set_label(project: &mut Project, item: ItemId, index: i64) -> Result<()> {
    project.set_color_label(item, index)?;
    let Some(master) = project.item(item).master else { return Ok(()) };
    let references = project.doc.tree.findall(master, "Clips/Clip")?;
    for reference in references {
        let clip = project.doc.resolve(reference)?;
        let Some(properties) = project.doc.tree.find(clip, "Clip/Node/Properties")? else { continue };
        let packed = colors::LABEL_PACKED[index as usize].to_string();
        let name = format!("{LABEL_PREFIX}{index}");
        for (tag, text) in [("asl.clip.label.color", packed), ("asl.clip.label.name", name)] {
            if let Some(leaf) = project.doc.tree.find(properties, tag)? {
                project.doc.tree.set_text(leaf, Some(text));
            }
        }
    }
    Ok(())
}

/// `comment_for`: the Avid comment, plus one "Avid: ..." line (user and colour when the option
/// is on, and where the marker sat in Avid when Premiere places it elsewhere).
pub fn comment_for(marker: &Marker, options: &Options, note: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !marker.comment.is_empty() {
        lines.push(marker.comment.clone());
    }
    let mut info: Vec<&str> = Vec::new();
    if options.marker_avid_info {
        info.extend([marker.user.as_str(), marker.color.as_str()].into_iter().filter(|v| !v.is_empty()));
    }
    if !note.is_empty() {
        info.push(note);
    }
    if !info.is_empty() {
        lines.push(format!("Avid: {}", info.join(" \u{b7} ")));
    }
    lines.join("\n")
}

fn color(marker: &Marker, options: &Options, notes: &mut Vec<String>) -> Option<i64> {
    let index = colors::marker_color(&marker.color, &options.marker_colors);
    if index.is_none() && !marker.color.is_empty() {
        notes.push(format!(
            "marker color {} is not an Avid palette name; Premiere's default color kept",
            py_str_repr(&marker.color)
        ));
    }
    index
}

/// `add_clip_marker`: a clip marker at `start` ticks of the item's media time.
pub fn add_clip_marker(
    project: &mut Project,
    item: ItemId,
    marker: &Marker,
    start: i64,
    options: &Options,
    notes: &mut Vec<String>,
    uuids: &mut dyn UuidSource,
) -> Result<()> {
    let core = project.own_clip_core(item)?;
    ensure_inner_list(&mut project.doc, core)?;
    let comments = comment_for(marker, options, "");
    let spec = MarkerSpec {
        name: &marker.name,
        start: Time::new(start),
        comments: &comments,
        marker_type: "Comment",
        duration: (marker.duration != 0).then(|| Time::new(marker.duration)),
    };
    let m = project.add_item_marker(item, &spec, uuids)?;
    if let Some(index) = color(marker, options, notes) {
        set_color_index(&mut project.doc.tree, m, index, uuids)?;
    }
    Ok(())
}

/// `add_sequence_marker`: a comment marker on a sequence (`note` says where it sat in Avid).
#[allow(clippy::too_many_arguments)]
pub fn add_sequence_marker(
    project: &mut Project,
    sequence: SequenceId,
    marker: &Marker,
    start: i64,
    options: &Options,
    notes: &mut Vec<String>,
    note: &str,
    uuids: &mut dyn UuidSource,
) -> Result<()> {
    let comments = comment_for(marker, options, note);
    let spec = MarkerSpec {
        name: &marker.name,
        start: Time::new(start),
        comments: &comments,
        marker_type: "Comment",
        duration: (marker.duration != 0).then(|| Time::new(marker.duration)),
    };
    let m = project.add_sequence_marker(sequence, &spec, uuids)?;
    if let Some(index) = color(marker, options, notes) {
        set_color_index(&mut project.doc.tree, m, index, uuids)?;
    }
    Ok(())
}

/// `xml.sax.saxutils.escape`.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Python `str.format` over the known placeholders, in one pass.
fn fill(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 128);
    let mut rest = template;
    'outer: while let Some(pos) = rest.find('{') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        for (key, text) in values {
            let pattern = format!("{{{key}}}");
            if tail.starts_with(&pattern) {
                out.push_str(text);
                rest = &tail[pattern.len()..];
                continue 'outer;
            }
        }
        out.push('{');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// `create_adjustment_layer`: an empty adjustment layer item in `target_bin` (it changes nothing
/// in the picture), used to carry markers that sat on filler in an Avid video track. Its media
/// time runs from 0, so markers and in points can use sequence time directly.
#[allow(clippy::too_many_arguments)]
pub fn create_adjustment_layer(
    project: &mut Project,
    target_bin: ItemId,
    name: &str,
    timebase: i64,
    width: i64,
    height: i64,
    timecode_format: i64,
    length: i64,
    uuids: &mut dyn UuidSource,
) -> Result<ItemId> {
    let out_point = length.max(timebase);
    // Python draws the ClipID while formatting, before the splice mints the object UIDs.
    let clip_id = uuids.uuid4().to_string();
    let text = fill(
        ADJUSTMENT_LAYER,
        &[
            ("name", escape(name)),
            ("timebase", timebase.to_string()),
            ("width", width.to_string()),
            ("height", height.to_string()),
            ("timecode_format", timecode_format.to_string()),
            ("out_point", out_point.to_string()),
            ("clip_id", clip_id),
        ],
    );
    let fragment = project.doc.tree.parse_fragment(&text)?;
    let spliced = project.splice(fragment, uuids)?;
    let (Some(item_element), Some(master)) = (spliced.get("ClipProjectItem"), spliced.get("MasterClip")) else {
        return value("adjustment layer template lost its item or master clip");
    };
    let bin_element = project.item(target_bin).element;
    let Some(container) = project.doc.tree.child_by_tag(bin_element, "ProjectItemContainer") else {
        return value("target bin has no ProjectItemContainer");
    };
    let items = child_items(&mut project.doc.tree, container);
    let uid = project.doc.tree.get(item_element, "ObjectUID").unwrap_or_default().to_string();
    add_item_ref(&mut project.doc.tree, items, &uid);
    let clip_references = project.doc.tree.findall(master, "Clips/Clip")?;
    let clip_elements = clip_references.into_iter().map(|r| project.doc.resolve(r)).collect::<Result<Vec<_>>>()?;
    let mut data = ItemData::new(item_element, ItemType::Clip);
    data.parent = Some(target_bin);
    data.master = Some(master);
    data.clip_elements = clip_elements;
    data.default_out_ticks = Some(out_point);
    let item = project.push_item(data);
    project.item_mut(target_bin).children.push(item);
    Ok(item)
}
