//! Single-ended Constant Gain transitions saved on a clip edge.
use super::components::field;
use crate::{audio::PREMIERE_FADE_ADAPTER, Result};
use graph::{Sidecar, Value};
use prproj::{
    model::{Project, TrackData},
    xml::NodeId,
};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Head,
    Tail,
}

pub(super) fn capture(
    project: &Project,
    track: &TrackData,
    frame_ticks: i64,
    sidecars: &mut Vec<Sidecar>,
) -> Result<bool> {
    let tree = &project.doc.tree;
    let mut edges = HashMap::new();
    let mut has_crossfade = false;
    for &transition in &track.transitions {
        let [inner] = tree.findall(transition, "TransitionTrackItem")?.try_into()
            .map_err(|_| "invalid Premiere audio transition body")?;
        let flag = |name| -> Result<bool> {
            match field(tree, inner, name)? {
                Some("true") => Ok(true),
                Some("false") => Ok(false),
                _ => Err("missing or invalid Premiere audio transition endpoint flag".into()),
            }
        };
        let endpoints = (flag("HasIncomingClip")?, flag("HasOutgoingClip")?);
        let edge = match endpoints {
            (true, true) => {
                has_crossfade = true;
                continue;
            }
            (true, false) => Edge::Head,
            (false, true) => Edge::Tail,
            (false, false) => return Err("Premiere audio transition has no clip endpoint".into()),
        };
        if edges.insert(transition, edge).is_some() {
            return Err("duplicate Premiere clip-edge transition reference on a lane".into());
        }
    }
    if has_crossfade {
        if !edges.is_empty() {
            return Err("Premiere clip-edge fades and crossfades on the same lane are unqualified".into());
        }
        return Ok(false);
    }
    let has_fades = !edges.is_empty();
    for &clip in &track.clips {
        let element = project.track_item(clip).element;
        let clip_start = project.start(clip)?.ticks;
        let clip_end = project.end(clip)?.ticks;
        let mut fades = Vec::new();
        for (edge, name, path) in [
            (Edge::Head, "head", "ClipTrackItem/HeadTransition"),
            (Edge::Tail, "tail", "ClipTrackItem/TailTransition"),
        ] {
            let references = tree.findall(element, path)?;
            let reference = match references.as_slice() {
                [] => continue,
                [reference] => *reference,
                _ => return Err("duplicate Premiere clip-edge reference on a clip".into()),
            };
            let transition = project.doc.resolve(reference)?;
            let id = tree.get(transition, "ObjectID").ok_or("Premiere clip-edge fade lacks ObjectID")?;
            if edges.remove(&transition) != Some(edge) {
                return Err(format!("Premiere transition {id}: clip-edge owner or lane reference disagrees").into());
            }
            if !tree.findall(element, "Panner")?.is_empty() {
                return Err(format!("Premiere transition {id}: combined clip pan and fade is unqualified").into());
            }
            let span = duration(project, transition, edge, clip_start, clip_end)
                .map_err(|error| format!("Premiere transition {id}: {error}"))?;
            let (unit, length) = if span % frame_ticks == 0 {
                ("frames", span / frame_ticks)
            } else {
                ("ticks", span)
            };
            fades.push((name.into(), Value::Obj(vec![
                ("transition".into(), Value::Str(format!("premiere:transition:{id}"))),
                (unit.into(), Value::Int(length)),
            ])));
        }
        if !fades.is_empty() {
            let id = tree.get(element, "ObjectID").ok_or("Premiere clip-edge owner lacks ObjectID")?;
            sidecars.push(Sidecar {
                adapter: PREMIERE_FADE_ADAPTER.into(),
                object: format!("premiere:edit:{id}"),
                data: Value::Obj(fades),
            });
        }
    }
    if !edges.is_empty() {
        return Err("Premiere lane lists a clip-edge fade without an owning clip".into());
    }
    Ok(has_fades)
}

fn duration(
    project: &Project,
    transition: NodeId,
    edge: Edge,
    clip_start: i64,
    clip_end: i64,
) -> Result<i64> {
    let tree = &project.doc.tree;
    let invalid = || "expected a Constant Gain fade at the owning clip edge";
    let [inner] = tree.findall(transition, "TransitionTrackItem")?.try_into().map_err(|_| invalid())?;
    let [span] = tree.findall(inner, "TrackItem")?.try_into().map_err(|_| invalid())?;
    let tick = |owner, name| -> Result<Option<i64>> {
        field(tree, owner, name)?.map(str::parse::<i64>).transpose().map_err(Into::into)
    };
    let start = tick(span, "Start")?.unwrap_or(0);
    let end = tick(span, "End")?.ok_or_else(invalid)?;
    let offset = tick(inner, "Alignment")?.ok_or_else(invalid)?;
    let duration = end.checked_sub(start).filter(|&n| n > 0).ok_or_else(invalid)?;
    if tree.tag(transition) != "AudioTransitionTrackItem"
        || tree.get(transition, "ClassID") != Some("23f687b6-9c5a-42f4-bea1-e7b3e28e7082")
        || tree.get(transition, "Version") != Some("4")
        || field(tree, inner, "MatchName")? != Some("Constant Gain")
        || field(tree, transition, "FadeShapeType")? != Some("0")
        || field(tree, transition, "FadeShapeValue")? != Some("0")
        || !matches!(field(tree, transition, "AudioChannelLayout")?,
            Some(r#"[{"channellabel":100},{"channellabel":101}]"# | r#"[{"channellabel":0}]"#))
        || !matches!(field(tree, transition, "Reverse")?, None | Some("false"))
        || !matches!(field(tree, transition, "BorderWidth")?, None | Some("0"))
        || start < clip_start || end > clip_end
        || match edge {
            Edge::Head => offset != 0 || start != clip_start,
            Edge::Tail => offset != duration || end != clip_end,
        }
    {
        return Err(invalid().into());
    }
    Ok(duration)
}
