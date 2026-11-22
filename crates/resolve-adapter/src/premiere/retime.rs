//! Read the clip-owned Premiere time map. Its trim is on the map's record axis.
use crate::{
    Result,
    retime::{LinearMap, MAX_LINEAR_KEYS},
};
use graph::{Rational, Sidecar, Value};
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{enums::TimeInterpolationType, Project, TrackItemId},
    xml::NodeId,
};

pub(super) fn frame_sampling(project: &Project, clip: TrackItemId) -> Result<bool> {
    Ok(project.time_interpolation_type(clip)? == TimeInterpolationType::FrameSampling)
}

pub(super) fn parameter(project: &Project, clip: TrackItemId) -> Result<Option<NodeId>> {
    let tree = &project.doc.tree;
    let native = project.track_item(clip);
    let Some(core) = tree.child_by_tag(native.clip, "Clip") else {
        return Ok(None);
    };
    let mut references = tree
        .children(core)
        .iter()
        .filter(|&&node| tree.tag(node) == "TimeRemapping");
    let Some(_) = references.next() else {
        return Ok(None);
    };
    if references.next().is_some() {
        return Err("ambiguous Premiere Time Remapping reference".into());
    }
    project
        .time_remapping(clip)?
        .map(Some)
        .ok_or_else(|| "incomplete Premiere Time Remapping reference".into())
}

pub(super) fn capture(
    project: &Project,
    param: NodeId,
    owner: &str,
    trim: (i64, i64),
    available: i64,
    clocks: (i64, Rational),
    budget: &mut super::components::Budget,
    sampled_frame_count: &mut usize,
) -> Result<(Rational, Vec<i64>, [Sidecar; 2])> {
    let (frame_ticks, source_rate) = clocks;
    let record_rate = Rational::new(TICKS_PER_SECOND.into(), frame_ticks.into())
        .ok_or("invalid Premiere retime record clock")?;
    let mixed = source_rate != record_rate;
    let fps = source_rate.num() as f64 / source_rate.den() as f64;
    let source_ticks = Rational::new(source_rate.den().into(), source_rate.num().into())
        .and_then(|time| time.to_units(TICKS_PER_SECOND))
        .ok_or("invalid Premiere retime source clock")?;
    let (in_ticks, out_ticks) = trim;
    let span = out_ticks
        .checked_sub(in_ticks)
        .ok_or("Premiere remap trim overflows")?;
    if in_ticks < 0 || in_ticks % frame_ticks != 0 || span % frame_ticks != 0 {
        return Err("Premiere Time Remapping trim is outside the whole record frame grid".into());
    }
    let length = span / frame_ticks;
    let count = crate::retime::hold_count(length).ok_or("invalid Linear retime edit length")?;
    *sampled_frame_count = sampled_frame_count
        .checked_add(count)
        .filter(|count| *count <= graph::MAX_GRAPH_BYTES / size_of::<i64>())
        .ok_or("Premiere sampled frame map budget exceeded")?;
    let tree = &project.doc.tree;
    let read = |tag| tree.findtext(param, tag);
    if tree.tag(param) != "TimeComponentParam"
        || tree.get(param, "ClassID") != Some("278ae1f9-ab7b-4dff-a53c-21029a399a9d")
        || read("ParameterID")?.as_deref() != Some("-1")
        || !matches!(
            read("DiscontinuousInterpolate")?.as_deref(),
            None | Some("false")
        )
        || !matches!(read("IsTimeVarying")?.as_deref(), None | Some("true"))
    {
        return Err("unqualified Premiere Time Remapping parameter".into());
    }
    let base = read("StartKeyframe")?.ok_or("missing Premiere Time Remapping start key")?;
    let fields: Vec<_> = base.split(',').collect();
    let ["-91445760000000000", value, "0", "0", "0", "0", "0", "0"] = fields.as_slice() else {
        return Err("unqualified Premiere Time Remapping start key".into());
    };
    if value.parse::<f64>()? != 0.0 {
        return Err("Premiere Time Remapping start key must begin at source zero".into());
    }
    let saved = read("Keyframes")?.ok_or("Premiere Time Remapping has no keys")?;
    if saved.len() > 64 * 1024 {
        return Err("Premiere Time Remapping keys exceed the size limit".into());
    }
    let mut keys: Vec<[i64; 2]> = Vec::new();
    let mut entries = saved.trim_end_matches(';').split(';').peekable();
    while let Some(entry) = entries.next() {
        let fields: Vec<_> = entry.split(',').collect();
        let [record, source, "6", "0", "0", "0", "0", "0"] = fields.as_slice() else {
            return Err("Premiere Time Remapping requires Linear keys without handles".into());
        };
        let record: i64 = record.parse()?;
        if record < 0 {
            return Err("Premiere Time Remapping keys exceed the count or record grid".into());
        }
        let saved_seconds: f64 = source.parse()?;
        let frame = source_frame(saved_seconds, fps)?;
        // A mixed-rate native save can append the exclusive file end
        // one source frame after the last playable key, with up to one tick
        // of rounding in the observed saves.
        // The last playable key covers all sampled record positions.
        if mixed && entries.peek().is_none() && frame == available
            && keys.last().is_some_and(|[previous, source]| {
                let delta = record - previous * frame_ticks;
                *source == available - 1
                    && (source_ticks..=source_ticks + 1).contains(&delta)
            })
        {
            break;
        }
        if record % frame_ticks != 0 || keys.len() == MAX_LINEAR_KEYS {
            return Err("Premiere Time Remapping keys exceed the count or record grid".into());
        }
        if mixed && (in_ticks..out_ticks).contains(&record) {
            // At a sampled knot, Premiere can request the previous file frame
            // if conversion of the saved ordinate lands one tick early.
            let exact_ticks = frame.checked_mul(source_ticks)
                .ok_or("invalid Premiere retime source key clock")?;
            if (saved_seconds * TICKS_PER_SECOND as f64) < exact_ticks as f64 {
                return Err("mixed-rate Premiere Linear key lands before its source frame tick".into());
            }
        }
        keys.push([record / frame_ticks, frame]);
    }
    if keys.len() < 3
        || source_frame(read("UpperBound")?.ok_or("missing Premiere remap extent")?.parse()?, fps)? != available
    {
        return Err("Premiere Time Remapping contradicts the verified original extent".into());
    }
    budget.charge(saved.len(), keys.len())?;
    let last = keys.len() - 1;
    // Premiere stores the end of the file as an exclusive terminal key.
    // Resolve uses the last source frame. Keep the terminal segment's slope.
    if keys[last][1] == available {
        let dx = keys[last][0] - keys[last - 1][0];
        let dy = keys[last][1] - keys[last - 1][1];
        if dx < 2 || dx != dy {
            return Err(
                "Premiere Time Remapping needs a unity tail to the verified file end".into(),
            );
        }
        keys[last][0] -= 1;
        keys[last][1] -= 1;
    }
    let map = LinearMap::new(in_ticks / frame_ticks, keys, available, source_rate, record_rate)?;
    let receipt = Sidecar {
        adapter: "premiere.time_remapping".into(),
        object: owner.into(),
        data: Value::Obj(vec![
            ("in_ticks".into(), Value::Int(in_ticks)),
            ("out_ticks".into(), Value::Int(out_ticks)),
            ("keys".into(), Value::Str(saved)),
        ]),
    };
    let samples = map.samples(length)?;
    Ok((map.source_time(samples[0])?, samples, [map.receipt(owner), receipt]))
}

fn source_frame(seconds: f64, fps: f64) -> Result<i64> {
    let frame = seconds * fps;
    if !(0.0..=10_000_000.0).contains(&frame) || (frame - frame.round()).abs() > 1e-7 {
        return Err("Premiere Time Remapping is outside the whole source frame grid".into());
    }
    Ok(frame.round() as i64)
}
