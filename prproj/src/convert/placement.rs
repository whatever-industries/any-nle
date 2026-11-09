//! Premiere field order for placed clips, shared by the writer and the combined-clip builders
//! (port of `converter/src/avid2prproj/placement.py`).

use crate::error::{value, Result};
use crate::model::{Project, TrackItemId};
use crate::xml::{NodeId, Tree};

/// `normalize_placement(item, multicam_rect)`: match Premiere's field order on a placed clip
/// core (ClipID, [SelectedTrackIndex], InPoint, OutPoint, [IsMulticam]); a placed multicam
/// video clip also carries the multicam flags and the track item a FrameRect (PRPROJ-0011
/// placements).
pub fn normalize_placement(project: &mut Project, item: TrackItemId, multicam_rect: Option<&str>) -> Result<()> {
    let data = project.track_item(item).clone();
    let tree = &mut project.doc.tree;
    let Some(core) = tree.child_by_tag(data.clip, "Clip") else { return value("'NoneType' object has no attribute 'find'") };
    let Some(clip_id) = tree.child_by_tag(core, "ClipID") else { return value("'NoneType' object has no attribute 'tail'") };
    let tail = tree.tail(clip_id).map(str::to_string);
    let mut moved: Vec<NodeId> = ["InPoint", "OutPoint"].iter().filter_map(|t| tree.child_by_tag(core, t)).collect();
    for &e in &moved {
        tree.remove(core, e);
    }
    if multicam_rect.is_some() {
        let selected = tree.new_element("SelectedTrackIndex");
        tree.set_text(selected, Some("0"));
        moved.insert(0, selected);
        let flag = tree.new_element("IsMulticam");
        tree.set_text(flag, Some("true"));
        moved.push(flag);
    }
    let at = position(tree, core, clip_id) + 1;
    for (offset, &e) in moved.iter().enumerate() {
        tree.set_tail(e, tail.clone());
        tree.insert(core, at + offset, e);
    }
    let element = data.element;
    let is_video = tree.tag(element) == "VideoClipTrackItem";
    if is_video {
        order_video_item_fields(tree, element);
    }
    if let Some(rect_text) = multicam_rect {
        let rect = tree.new_element("FrameRect");
        tree.set_text(rect, Some(rect_text));
        let tone = tree.child_by_tag(element, "ToneMapSettings");
        let par = tree.child_by_tag(element, "PixelAspectRatio");
        if let Some(par) = par {
            // Premiere's order: ToneMapSettings, FrameRect, PixelAspectRatio
            tree.remove(element, par);
        }
        let at = match tone {
            Some(tone) => position(tree, element, tone) + 1,
            None => tree.len(element),
        };
        let rect_tail = tone.and_then(|t| tree.tail(t).map(str::to_string));
        tree.set_tail(rect, rect_tail);
        tree.insert(element, at, rect);
        if let Some(par) = par {
            tree.insert(element, at + 1, par);
        }
        order_video_item_fields(tree, element);
    }
    Ok(())
}

fn position(tree: &Tree, parent: NodeId, child: NodeId) -> usize {
    tree.children(parent).iter().position(|&c| c == child).expect("child of parent")
}

const VIDEO_ITEM_ORDER: [&str; 3] = ["ToneMapSettings", "FrameRect", "PixelAspectRatio"];

/// `order_video_item_fields(item)`: Premiere writes ToneMapSettings, FrameRect,
/// PixelAspectRatio in that order on a video track item (PRPROJ-0011 placements); py_premiere
/// writes PixelAspectRatio first. `element` is the track item.
pub fn order_video_item_fields(tree: &mut Tree, element: NodeId) {
    let present: Vec<NodeId> = VIDEO_ITEM_ORDER.iter().filter_map(|t| tree.child_by_tag(element, t)).collect();
    if present.len() < 2 {
        return;
    }
    let first = present.iter().map(|&e| position(tree, element, e)).min().expect("non-empty");
    let mut by_position = present.clone();
    by_position.sort_by_key(|&e| position(tree, element, e));
    let tails: Vec<Option<String>> = by_position.iter().map(|&e| tree.tail(e).map(str::to_string)).collect();
    for &e in &present {
        tree.remove(element, e);
    }
    for (offset, (&e, tail)) in present.iter().zip(tails).enumerate() {
        tree.set_tail(e, tail);
        tree.insert(element, first + offset, e);
    }
}
