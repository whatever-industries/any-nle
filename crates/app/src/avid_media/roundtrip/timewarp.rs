use super::*;
use avid_reader::{
    file::AvbFile,
    source::{extract_source_files, SourceInput, SourceLimits},
};

const SPEED: &str = "8d56827c-847e-11d5-935a-50f857c10000";
const POSITION: &str = "8d56827d-847e-11d5-935a-50f857c10000";
const AUXILIARY: &str = "8d568283-847e-11d5-935a-50f857c10000";

pub(super) struct Check {
    mob: MobId,
    record: i64,
    track: i64,
    length: i64,
    frame_map: Vec<i64>,
    controls: Json,
}

pub(super) fn prepare(
    graph: &SourceGraph,
    donor: &Donor<'_>,
    owner: &str,
    changes: &mut Changes,
) -> Option<Result<()>> {
    let mut matches = graph
        .sidecars
        .iter()
        .filter(|sidecar| sidecar.adapter == "avid.timewarp")
        .flat_map(|sidecar| {
            list(&sidecar.data, "effects")
                .unwrap_or(&[])
                .iter()
                .map(move |effect| (sidecar, effect))
        })
        .filter(|(_, effect)| text(effect, "owner").ok() == Some(owner));
    let (sidecar, saved) = matches.next()?;
    Some(if matches.next().is_some() {
        Err("ambiguous saved Timewarp owner".into())
    } else {
        prepare_saved(graph, donor, &sidecar.object, &sidecar.data, saved, changes)
    })
}

fn prepare_saved(
    graph: &SourceGraph,
    donor: &Donor<'_>,
    sequence_id: &str,
    container: &Json,
    saved: &Json,
    changes: &mut Changes,
) -> Result<()> {
    let sequence = only(
        graph
            .sequences
            .iter()
            .filter(|sequence| sequence.id == sequence_id),
        "Timewarp sequence",
    )?;
    if sequence.rate != Rational::integer(24) {
        return Err("the initial editable Timewarp profile requires exact 24 fps".into());
    }
    let mob: MobId = text(container, "mob")?.parse()?;
    let record = number(saved, "record")?;
    let track = number(saved, "track")?;
    let length = number(saved, "length")?;
    if record < 0 || track != 1 || !(3..=4096).contains(&length) {
        return Err("unsupported native Timewarp interval or track".into());
    }
    let placement = only(
        sequence.placements.iter().filter(|placement| {
            placement.video_track == Some(track)
                && placement.record_start.to_units(24) == Some(record)
                && placement.duration.to_units(24) == Some(length)
        }),
        "qualified Timewarp placement",
    )?;
    let source = graph
        .sidecar("avid.source", &placement.id)
        .ok_or("missing Timewarp placement source facts")?;
    if text(source, "owner")? != text(saved, "owner")? {
        return Err("Timewarp occurrence and placement ownership disagree".into());
    }
    let frame_map = placement
        .frame_map
        .as_ref()
        .filter(|map| map.len() as i64 == length)
        .ok_or("Timewarp owner does not have a qualified frame map")?;
    let native = donor.mob(&mob.urn())?;
    let lane = donor.track(native, 1, track)?;
    donor.unshared(lane)?;
    let effect = donor.segment(lane, record, length, *b"SPED")?;
    donor.unshared(effect)?;
    let effect = donor.node(effect)?;
    let controls = saved
        .get("controls")
        .ok_or("missing saved Timewarp controls")?;
    if effect.get("effect_id").and_then(Value::as_str) != Some("EFF_ADV_MOTION_CTL")
        || rate(effect)? != sequence.rate
        || !matches!(effect.get("precomputed"), None | Some(Value::Null))
        || controls.get("precompute_present") != Some(&Json::Bool(false))
    {
        return Err("unsupported donor Timewarp or stale render reference".into());
    }
    let input = reference(only(tracks(effect)?, "Timewarp input")?, "component")?;
    let clip = plain_clip(donor, input)?;
    let expected = controls
        .get("input")
        .ok_or("missing saved Timewarp input")?;
    if clip.get("mob_id") != Some(&Value::MobId(text(expected, "mob")?.parse()?))
        || integer(clip, "track_id")? != number(expected, "track")?
        || integer(clip, "start_time")? != number(expected, "start")?
        || integer(clip, "length")? != number(expected, "length")?
        || rate(clip)?.to_string() != text(expected, "rate")?
    {
        return Err("donor Timewarp source identity or trim disagrees".into());
    }
    let actual = only(
        graph.items.iter().filter(|item| item.id == placement.item),
        "Timewarp backing item",
    )?;
    if actual
        .umid
        .as_deref()
        .map(str::parse::<MobId>)
        .transpose()?
        != Some(text(expected, "mob")?.parse()?)
    {
        return Err("Timewarp placement and native control use different masters".into());
    }
    let parameter_list = reference(effect, "param_list")?;
    donor.unshared(parameter_list)?;
    let entries = refs(donor.node(parameter_list)?, "items")?;
    let curves = list(controls, "curves")?;
    if entries.len() != 3 || curves.len() != 3 {
        return Err("expected the complete three-curve Linear Timewarp profile".into());
    }
    for name in [SPEED, POSITION, AUXILIARY] {
        let saved = only(
            curves
                .iter()
                .filter(|curve| text(curve, "parameter").ok() == Some(name)),
            "saved Timewarp parameter",
        )?;
        let parameter = only(entries.iter().filter_map(|entry| entry.as_index()).filter(|id|
            matches!(donor.doc.node(*id).and_then(|n| n.get("uuid")), Some(Value::Uuid(uuid)) if uuid.to_string() == name)),
            "donor Timewarp parameter")?;
        donor.unshared(parameter)?;
        let parameter = donor.node(parameter)?;
        let curve = reference(parameter, "control_track")?;
        let node = donor.node(curve)?;
        let interpolation = if name == AUXILIARY { 2 } else { 3 };
        if node.class != *b"PRCL"
            || number(saved, "interpolation")? != interpolation
            || integer(node, "interp_kind")? != interpolation
        {
            return Err("unsupported Timewarp interpolation".into());
        }
        let Some(Value::List(points)) = node.get("control_points") else {
            return Err("missing donor Timewarp points".into());
        };
        let mut points = points.clone();
        if name == POSITION {
            let saved = saved.get("points").ok_or("missing saved Position map")?;
            let values = list(saved, "values")?;
            let first = number(saved, "first_half_frame")?;
            if values.len() != points.len() || values.len() > 8193 {
                return Err("donor Position extent disagrees".into());
            }
            for (index, (point, value)) in points.iter_mut().zip(values).enumerate() {
                let point = point_node(point)?;
                let (num, den) = point_offset(point)?;
                if den <= 0
                    || i128::from(num) * 2 != i128::from(den) * (i128::from(first) + index as i128)
                    || integer(point, "timescale")? != 1
                {
                    return Err("donor Position map is not on the saved half-frame grid".into());
                }
                replace_float(point, value)?;
            }
        } else {
            let saved = list(saved, "points")?;
            let count = if name == SPEED { 2 } else { 1 };
            if saved.len() != count || points.len() != count {
                return Err("donor Speed or auxiliary curve has the wrong key count".into());
            }
            for (point, key) in points.iter_mut().zip(saved) {
                let point = point_node(point)?;
                let (num, den) = point_offset(point)?;
                let offset = list(key, "time")?;
                if den <= 0
                    || offset != [Json::Int(num), Json::Int(den)]
                    || integer(point, "timescale")? != number(key, "timescale")?
                {
                    return Err("donor Timewarp key coordinates disagree".into());
                }
                let value = key.get("value").ok_or("missing saved Timewarp key value")?;
                if name == SPEED {
                    if !matches!(value, Json::Float(v) if v.is_finite() && *v > 0.0) {
                        return Err("the initial Timewarp profile requires positive speed".into());
                    }
                    replace_float(point, value)?;
                }
            }
        }
        if name != AUXILIARY {
            changes.set(donor, curve, "control_points", Value::List(points))?;
        }
    }
    changes.items.insert(actual.id.clone());
    changes.timewarps.push(Check {
        mob,
        record,
        track,
        length,
        frame_map: frame_map.clone(),
        controls: controls.clone(),
    });
    Ok(())
}

fn point_node(value: &mut Value) -> Result<&mut Node> {
    match value {
        Value::Node(node) if node.type_name == "ParamControlPoint" => Ok(node),
        _ => Err("unsupported donor Timewarp point shape".into()),
    }
}

fn point_offset(point: &Node) -> Result<(i64, i64)> {
    if let Some(Value::List(offset)) = point.get("offset") {
        if let [Value::Int(num), Value::Int(den)] = offset.as_slice() {
            return Ok((*num, *den));
        }
    }
    Err("malformed donor Timewarp point offset".into())
}

fn replace_float(point: &mut Node, value: &Json) -> Result<()> {
    let Json::Float(value) = value else {
        return Err("Timewarp point lacks its saved floating-point value".into());
    };
    let (_, field) = point
        .fields
        .iter_mut()
        .find(|(name, _)| *name == "value")
        .ok_or("missing donor key value")?;
    if !value.is_finite() || !matches!(field, Value::Float(_)) {
        return Err("invalid Timewarp floating-point key".into());
    }
    *field = Value::Float(*value);
    Ok(())
}

pub(super) fn verify(bytes: &[u8], checks: &[Check]) -> Result<()> {
    if checks.is_empty() {
        return Ok(());
    }
    let file = AvbFile::parse(bytes)?;
    let input = SourceInput {
        path: PathBuf::from("staged.avb"),
        revision: "0".repeat(64),
    };
    let facts = extract_source_files(&[input], &[&file], SourceLimits::default())
        .map_err(|error| error.message)?;
    for check in checks {
        let sequence = only(
            facts
                .sequences
                .iter()
                .filter(|sequence| sequence.info.mob_id == Some(check.mob)),
            "staged native Timewarp sequence",
        )?;
        let saved = only(
            sequence.saved_timewarps.iter().filter(|saved| {
                saved.record == check.record
                    && saved.track == check.track
                    && saved.length == check.length
            }),
            "staged qualified Timewarp controls",
        )?;
        let event = only(
            sequence.events.iter().filter(|event| {
                event.event.kind == 1
                    && event.event.track == check.track
                    && event.range.as_ref().is_some_and(|range| {
                        range.record_start == check.record && range.length == check.length
                    })
            }),
            "staged qualified Timewarp event",
        )?;
        let range = event
            .range
            .as_ref()
            .ok_or("staged Timewarp range is unresolved")?;
        let samples = event
            .saved_timewarp
            .as_ref()
            .ok_or("staged Timewarp lost its picture samples")?;
        if avid_adapter::timewarp_controls(&saved.controls) != check.controls
            || !samples
                .iter()
                .map(|frame| range.source_in.checked_add(*frame))
                .eq(check.frame_map.iter().copied().map(Some))
        {
            return Err(format!(
                "{}: staged native Timewarp controls or frame selections disagree at V{} frame {}",
                check.mob.urn(),
                check.track,
                check.record
            )
            .into());
        }
    }
    Ok(())
}
