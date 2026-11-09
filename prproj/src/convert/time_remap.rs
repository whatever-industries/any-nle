//! Port of py_premiere's `TrackItem.set_time_remap` create path (models/track_item.py), as the
//! converter uses it for Avid Timewarp: a `TimeRemapping` object holding one Speed
//! `TimeComponentParam` whose keyframes map clip-relative timeline ticks to SOURCE seconds
//! (interp 6, zero tangents, `UpperBound` = the source duration in seconds), as py-premiere's
//! sample 25_time_remap stores them. The general read/edit API is ported separately.

use crate::build::time::{Time, TICKS_PER_SECOND};
use crate::build::{leaf, top};
use crate::error::{value, Result};
use crate::model::project::py_int;
use crate::model::{Project, TrackItemId};

const TIME_REMAP_CLASS_ID: &str = "ace5148e-9c9b-40ed-9f82-64cb67308464";
const TIME_PARAM_CLASS_ID: &str = "278ae1f9-ab7b-4dff-a53c-21029a399a9d";

/// `_format_remap_value`: source seconds at 24 fixed decimal places.
fn remap_value(v: f64) -> String {
    format!("{v:.24}")
}

/// `_format_remap_bound`: an integral bound as a bare integer, else full double precision.
fn remap_bound(v: f64) -> Result<String> {
    if v == v.trunc() {
        crate::build::fmt::int_str(v)
    } else {
        Ok(crate::build::fmt::py_repr(v))
    }
}

/// `set_time_remap` on a track item that has no curve yet. `keys` are (clip-relative timeline
/// time, source seconds).
/// The converter also writes the first key's value as the curve's start value: Premiere shows
/// it on the clip's first frame (py-premiere writes 0; frame 0 showed there, User export
/// 2026-10-20).
pub fn set_time_remap(project: &mut Project, item: TrackItemId, keys: &[(Time, f64)]) -> Result<()> {
    if keys.len() < 2 {
        return value("a time-remap curve needs at least two keyframes");
    }
    for (time, v) in keys {
        if !(v.is_finite() && *v >= 0.0) {
            return value("expected a non-negative number");
        }
        if time.ticks < 0 {
            return value("keyframe times are clip-relative, not negative");
        }
    }
    let mut ordered: Vec<(i64, f64)> = keys.iter().map(|(t, v)| (t.ticks, *v)).collect();
    ordered.sort_by_key(|k| k.0);
    if let Some(w) = ordered.windows(2).find(|w| w[0].0 == w[1].0) {
        return value(format!("duplicate keyframe time {}", w[0].0));
    }
    let clip = project.track_item(item).clip;
    let tree = &project.doc.tree;
    let Some(core) = tree.child_by_tag(clip, "Clip") else { return value("track item has no clip core") };
    if tree.child_by_tag(core, "TimeRemapping").is_some() {
        return value("track item already has a time-remap curve (edit path not ported here)");
    }
    let Some(source_ref) = tree.child_by_tag(core, "Source") else { return value("track item has no media source") };
    let source = project.doc.resolve(source_ref)?;
    let duration_text = project.doc.tree.findtext(source, "OriginalDuration")?.unwrap_or_default();
    if duration_text.is_empty() {
        return value("media source has no duration to bound the curve");
    }
    let upper_bound = py_int(&duration_text)? as f64 / TICKS_PER_SECOND as f64;
    let (last_time, last_value) = *ordered.last().unwrap();
    if last_value > upper_bound {
        return value("keyframe values cannot exceed the source duration");
    }
    if last_value < upper_bound {
        // Premiere completes an unfinished curve with a terminal key at the source end,
        // continuing at unity speed from the last key.
        let terminal = last_time + ((upper_bound - last_value) * TICKS_PER_SECOND as f64).round_ties_even() as i64;
        ordered.push((terminal, upper_bound));
    }
    let entries: String = ordered.iter().map(|(t, v)| format!("{t},{},6,0,0,0,0,0;", remap_value(*v))).collect();

    let tree = &mut project.doc.tree;
    let param = top(tree, "TimeComponentParam", TIME_PARAM_CLASS_ID, "9", None);
    leaf(tree, param, "Name", "Speed", "\n\t\t");
    leaf(tree, param, "IsTimeVarying", "true", "\n\t\t");
    leaf(tree, param, "IsLocked", "false", "\n\t\t");
    leaf(tree, param, "DiscontinuousInterpolate", "false", "\n\t\t");
    leaf(tree, param, "LowerBound", "0", "\n\t\t");
    let bound = remap_bound(upper_bound)?;
    leaf(tree, param, "UpperBound", &bound, "\n\t\t");
    let first = keys.iter().min_by_key(|k| k.0.ticks).map(|k| k.1).unwrap_or(0.0);
    leaf(tree, param, "StartKeyframe", &format!("-91445760000000000,{},0,0,0,0,0,0", remap_value(first)), "\n\t\t");
    leaf(tree, param, "Keyframes", &entries, "\n\t\t");
    leaf(tree, param, "CurrentValue", "0", "\n\t\t");
    leaf(tree, param, "ParameterID", "-1", "\n\t");
    let param_id = project.doc.add_object(param)?;
    let tree = &mut project.doc.tree;
    let remap = top(tree, "TimeRemapping", TIME_REMAP_CLASS_ID, "2", None);
    let k = tree.sub_element(remap, "Keyframes", &[("ObjectRef", &param_id)]);
    tree.set_tail(k, Some("\n\t"));
    let remap_id = project.doc.add_object(remap)?;
    // The ref slots right after `Source` in the clip core.
    let tree = &mut project.doc.tree;
    let reference = tree.new_element_with("TimeRemapping", &[("ObjectRef", &remap_id)]);
    let tail = tree.tail(source_ref).map(String::from);
    tree.set_tail(reference, tail);
    let index = tree.children(core).iter().position(|&c| c == source_ref).unwrap();
    tree.insert(core, index + 1, reference);
    Ok(())
}
