//! Write the verified nonlinear nests and binary-alpha overlays.
use super::{add, other, Result};
use crate::build::time::Time;
use crate::convert::{
    model::{ticks_per_unit, Clip, Fraction, Matte, Sequence, VideoFile},
    opatom::{self, DirectDnxhrFile, MediaTable},
    prepared::{PngAlpha, PreparedJpegFill, PreparedPngEdit, PreparedPngStill, PreparedSplitMatte},
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
        let Some(background) = edit.background else {
            let foreground = edit.foreground.ok_or_else(|| other("PNG overlay is missing its source"))?;
            place_span(project, sequence, 1, items[foreground], edit.start, edit.duration, ids)?;
            continue;
        };
        let background = items[background];
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
                    place_span(project, child, 0, background, 0, frame, ids)?;
                    place_span(project, child, 1, items[foreground], 0, frame, ids)?;
                    project.set_nested_source_duration(child, frame)?;
                    v1 = item;
                }
            }
        }
        place_span(project, sequence, 0, v1, edit.start, frame, ids)?;
        if let Some(v2) = v2 {
            place_span(project, sequence, 1, v2, edit.start, frame, ids)?;
        }
    }
    Ok(())
}

fn place_span(
    project: &mut Project,
    sequence: SequenceId,
    lane: usize,
    source: ItemId,
    start: i64,
    duration: i64,
    ids: &mut dyn UuidSource,
) -> Result<crate::model::TrackItemId> {
    let track = *project
        .video_tracks(sequence)
        .get(lane)
        .ok_or_else(|| other("PNG composite needs two picture tracks"))?;
    let edit = project.add_clip(track, source, Time::new(start), ids)?;
    project.set_in_point(edit, Time::new(0))?;
    project.set_out_point(edit, Time::new(duration))?;
    project.set_end(edit, Time::new(add(start, duration)?))?;
    Ok(edit)
}

pub(super) fn place_split(
    project: &mut Project, sequence: SequenceId, frame: i64,
    edit: &PreparedSplitMatte, fill: ItemId, managed_hold: bool, ids: &mut dyn UuidSource,
) -> Result<()> {
    let item = place_span(project, sequence, (edit.track - 1) as usize, fill, edit.start, edit.duration, ids)?;
    if managed_hold {
        crate::convert::time_remap::set_frame_hold(project, item, 0);
    }
    let shapes = std::iter::once(edit.rectangle).chain(edit.hole)
        .map(|rectangle| vec![(0, rectangle_points(rectangle))]).collect();
    let matte = Matte {
        shapes,
        inverted: false,
        opacities: Vec::new(),
        feathers: Vec::new(),
        not_converted: Vec::new(),
    };
    crate::convert::matte::write_masks(project, item, &matte, 0, frame, edit.hole.is_some())?;
    Ok(())
}

pub(super) fn import_managed_fill(
    project: &mut Project, media: &mut MediaTable, fill: &PreparedJpegFill,
    start: i64, bin: ItemId, ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    let rate = Fraction::new(24_000, 1001);
    let video = VideoFile {
        path: fill.path.to_str().ok_or_else(|| other("non-UTF-8 Matte Key MXF path"))?.to_owned(),
        codec: "AVdn".into(),
        width: 1920, height: 1080, rate, duration: ticks_per_unit(rate), start,
    };
    let clip = Clip {
        id: String::new(), name: fill.name.clone(), material_umid: None, tape_name: None,
        video: None, audio: Vec::new(), avid: None, time_display: None, stereo_pairs: Vec::new(),
        multichannel_groups: Vec::new(), rate: Some(rate), markers: Vec::new(),
    };
    media.direct_dnxhr.insert(video.path.clone(), DirectDnxhrFile {
        origin: start, picture: true, display: "110", dnxhr: false,
    });
    let previous = project.import_target.replace(bin);
    let item = opatom::add_video_item(project, media, &video, &clip, None, ids);
    project.import_target = previous;
    Ok(item?)
}

fn rectangle_points([left, top, right, bottom]: [u32; 4]) -> Vec<[f64; 6]> {
    // Extend edges at the canvas boundary so the mask has no antialiasing
    // seam along the picture border.
    let x0 = if left == 0 { -0.01 } else { f64::from(left) / 1920.0 };
    let y0 = if top == 0 { -0.01 } else { f64::from(top) / 1080.0 };
    let x1 = if right == 1920 { 1.01 } else { f64::from(right) / 1920.0 };
    let y1 = if bottom == 1080 { 1.01 } else { f64::from(bottom) / 1080.0 };
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].into_iter()
        .map(|(x, y)| [x, y, 0.0, 0.0, 0.0, 0.0]).collect()
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
