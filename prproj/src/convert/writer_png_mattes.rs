//! Keep the tested nonlinear blend inside a one-frame child with both inputs.
use super::{add, other, Result};
use crate::build::time::Time;
use crate::convert::{
    model::{ticks_per_unit, Sequence},
    prepared::{PngAlpha, PreparedPngEdit, PreparedPngStill},
};
use crate::ids::UuidSource;
use crate::model::{ItemId, Project, SequenceId};
use crate::xml::mutations::append_leaf;

pub(super) fn import(
    project: &mut Project,
    stills: &[PreparedPngStill],
    bins: &[ItemId],
    ids: &mut dyn UuidSource,
) -> Result<Vec<ItemId>> {
    stills
        .iter()
        .map(|still| {
            let item = project.import_prepared_png(&still.path, bins[still.bin], ids)?;
            project.set_item_name(item, &still.name)?;
            let interpretation = project
                .footage_interpretation(item)?
                .ok_or_else(|| other("imported PNG has no footage interpretation"))?;
            let tree = &mut project.doc.tree;
            match still.alpha {
                PngAlpha::Ignore => interpretation.set_ignore_alpha(tree, true)?,
                PngAlpha::Straight { invert } => interpretation.set_invert_alpha(tree, invert)?,
            }
            Ok(item)
        })
        .collect()
}

pub(super) fn place(
    project: &mut Project,
    sequence: SequenceId,
    seq: &Sequence,
    preset: &str,
    parent: Option<ItemId>,
    stills: &[PreparedPngStill],
    items: &[ItemId],
    edits: &[&PreparedPngEdit],
    ids: &mut dyn UuidSource,
) -> Result<()> {
    let frame = ticks_per_unit(seq.rate);
    for edit in edits {
        let background = items[edit.background];
        let mut v1 = background;
        let mut v2 = None;
        if let Some(foreground) = edit.foreground {
            match stills[foreground].alpha {
                PngAlpha::Ignore => v2 = Some(items[foreground]),
                PngAlpha::Straight { .. } => {
                    let child = project.add_sequence(
                        &format!("{} — Matte Key {}", seq.name, edit.start / frame + 1),
                        preset,
                        Some(2),
                        ids,
                    )?;
                    let item = project
                        .sequence_item(child)
                        .ok_or_else(|| other("Matte Key child has no bin item"))?;
                    if let Some(bin) = parent {
                        project.move_item(item, bin)?;
                    }
                    set_nonlinear_blend(project, child)?;
                    project.set_zero_point(child, Time::new(add(seq.start, edit.start)?))?;
                    place_frame(project, child, 0, background, 0, frame, ids)?;
                    place_frame(project, child, 1, items[foreground], 0, frame, ids)?;
                    project.set_nested_source_duration(child, frame)?;
                    v1 = item;
                }
            }
        }
        place_frame(project, sequence, 0, v1, edit.start, frame, ids)?;
        if let Some(v2) = v2 {
            place_frame(project, sequence, 1, v2, edit.start, frame, ids)?;
        }
    }
    Ok(())
}

fn place_frame(
    project: &mut Project,
    sequence: SequenceId,
    lane: usize,
    source: ItemId,
    start: i64,
    frame: i64,
    ids: &mut dyn UuidSource,
) -> Result<()> {
    let track = *project
        .video_tracks(sequence)
        .get(lane)
        .ok_or_else(|| other("PNG composite needs two picture tracks"))?;
    let edit = project.add_clip(track, source, Time::new(start), ids)?;
    project.set_in_point(edit, Time::new(0))?;
    project.set_out_point(edit, Time::new(frame))?;
    project.set_end(edit, Time::new(add(start, frame)?))?;
    Ok(())
}

fn set_nonlinear_blend(project: &mut Project, sequence: SequenceId) -> Result<()> {
    let sequence = project.sequence(sequence).element;
    let mut groups = None;
    for reference in project
        .doc
        .tree
        .findall(sequence, "TrackGroups/TrackGroup/Second")?
    {
        let group = project.doc.resolve(reference)?;
        if project.doc.tree.tag(group) == "VideoTrackGroup" && groups.replace(group).is_some() {
            return Err(other("Matte Key child has multiple video track groups"));
        }
    }
    let group = groups.ok_or_else(|| other("Matte Key child has no video track group"))?;
    let tree = &mut project.doc.tree;
    if let Some(field) = tree.child_by_tag(group, "AllowLinearCompositing") {
        tree.set_text(field, Some("false"));
    } else {
        append_leaf(tree, group, "AllowLinearCompositing", "false");
    }
    Ok(())
}
