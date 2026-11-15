//! Qualify an Avid project-local external LUT for one Premiere picture edit.
use crate::source_input::{original_stamp, recheck_originals, OriginalStamp};
use avid_reader::source::SourceInput;
use graph::{source::SourceGraph, Rational, StreamKind, Value};
use prproj::{
    convert::{
        model::{Fraction, TICKS_PER_SECOND},
        prepared::{PreparedClip, PreparedSourceLut},
    },
    xml::{NodeId, Tree},
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const ASPECT: &str = "source_color_adapter";
const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024;
const MAX_CUBE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CUBE_EDGE: usize = 65;
const MAX_DISTINCT_INPUTS: usize = 32;

#[derive(Default)]
pub(crate) struct Applied {
    pub inputs: Vec<SourceInput>,
    pub pictures: Vec<OriginalStamp>,
    recovered: HashSet<String>,
}

impl Applied {
    pub fn keeps_loss(&self, loss: &graph::Loss) -> bool {
        loss.aspect != ASPECT || !self.recovered.contains(&loss.object)
    }
}

fn text<'a>(row: &'a Value, name: &str) -> Option<&'a str> {
    match row.get(name)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

fn adapter(row: &Value) -> Option<&Value> {
    let facts = row.get("color_adapter")?;
    (text(row, "effect_id") == Some("EFF2_LUTSFX")
        && facts.get("neutral_wrapper") == Some(&Value::Bool(true))
        && text(facts, "text").is_some_and(|s| s.contains("<ExternalLut>"))
        && text(facts, "picture_file").is_some()
        && text(facts, "placement").is_some())
    .then_some(facts)
}

pub(crate) fn has_candidates(graph: &SourceGraph) -> bool {
    graph.sidecars.iter().any(|sidecar| {
        sidecar.adapter == "avid.source"
            && matches!(sidecar.data.get("unresolved"), Some(Value::Arr(rows))
            if rows.iter().any(|row| adapter(row).is_some()))
    })
}

fn project_lut_folder(id: &str, bins: &[SourceInput]) -> Result<PathBuf, String> {
    let (rev, suffix) = id.split_once("/bin/").ok_or("unknown Avid bin origin")?;
    let (index, _) = suffix
        .split_once("/item/")
        .ok_or("unknown Avid bin origin")?;
    let input = index
        .parse::<usize>()
        .ok()
        .and_then(|i| bins.get(i))
        .filter(|input| input.revision.eq_ignore_ascii_case(rev))
        .ok_or("unknown Avid bin origin")?;
    let bin = input.path.canonicalize().map_err(|e| e.to_string())?;
    let parent = bin.parent().ok_or("unknown Avid project folder")?;
    let name = parent.file_name().ok_or("unknown Avid project name")?;
    let mut project_name = name.to_os_string();
    project_name.push(".avp");
    let folder = parent.join("LUTs");
    if !parent.join(project_name).is_file()
        || !fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir())
    {
        return Err(
            "select the saved bin in its Avid project folder with its LUTs directory".into(),
        );
    }
    Ok(folder)
}

fn xml_lut(xml: &str, list: bool) -> Option<(String, String)> {
    let mut tree = Tree::new();
    let root = tree.parse_fragment(xml).ok()?;
    let container = |id: NodeId, tag| {
        let node = &tree[id];
        node.tag == tag
            && node.attrs.is_empty()
            && node.text_or_empty().trim().is_empty()
            && node.children().iter().all(|&child| {
                tree[child]
                    .tail
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            })
    };
    let single = |id, tag| {
        let [child] = tree.children(id) else {
            return None;
        };
        container(id, tag).then_some(*child)
    };
    let transform = if list {
        single(root, "ColorTransformationList")?
    } else {
        root
    };
    let lut = single(transform, "ColorTransformation")?;
    if !container(lut, "ExternalLut") {
        return None;
    }
    let [name, file, dimension, scaling] = tree.children(lut) else {
        return None;
    };
    let leaf = |id, tag| {
        let node: &prproj::xml::tree::Element = &tree[id];
        (node.tag == tag && node.attrs.is_empty() && node.children().is_empty())
            .then(|| node.text_or_empty())
    };
    let filename = leaf(*file, "LutFileName")?;
    let name = leaf(*name, "Name")?;
    (leaf(*dimension, "LutDimension")? == "3D"
        && leaf(*scaling, "InhibitScaling")?.is_empty()
        && !name.is_empty()
        && PreparedSourceLut::valid_filename(filename))
    .then(|| (name.into(), filename.into()))
}

type Cube = [(f64, f64, f64); 8];

fn rgb(mut fields: std::str::SplitWhitespace<'_>) -> Option<[f64; 3]> {
    let point = [
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ];
    (fields.next().is_none() && point.iter().all(|v| (0.0..=1.0).contains(v))).then_some(point)
}

fn cube(s: &str) -> Option<Cube> {
    let mut title = false;
    let mut edge = None;
    let mut minimum = false;
    let mut maximum = false;
    let mut points = Vec::new();
    for line in s
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let mut fields = line.split_whitespace();
        match fields.next()? {
            "TITLE" if !title && points.is_empty() => {
                let value = line
                    .strip_prefix("TITLE")?
                    .trim()
                    .strip_prefix('"')?
                    .strip_suffix('"')?;
                if value.contains('"') {
                    return None;
                }
                title = true;
            }
            "LUT_3D_SIZE" if edge.is_none() && points.is_empty() => {
                let size = fields.next()?.parse::<usize>().ok()?;
                if fields.next().is_some() || !(2..=MAX_CUBE_EDGE).contains(&size) {
                    return None;
                }
                points.reserve_exact(size.pow(3));
                edge = Some(size);
            }
            "DOMAIN_MIN"
                if !minimum && points.is_empty() && rgb(fields.clone()) == Some([0.0; 3]) =>
            {
                minimum = true;
            }
            "DOMAIN_MAX"
                if !maximum && points.is_empty() && rgb(fields.clone()) == Some([1.0; 3]) =>
            {
                maximum = true;
            }
            _ if minimum && maximum && points.len() < edge?.pow(3) => {
                points.push(rgb(line.split_whitespace())?);
            }
            _ => return None,
        }
    }
    let edge = edge?;
    if points.len() != edge.pow(3) {
        return None;
    }
    affine_corners(&points, edge)
}

fn affine_corners(points: &[[f64; 3]], edge: usize) -> Option<Cube> {
    let last = edge - 1;
    let steps = [1, edge, edge * edge];
    let corners: [[f64; 3]; 8] = std::array::from_fn(|corner| {
        let index = steps
            .iter()
            .enumerate()
            .filter(|(axis, _)| corner & (1 << axis) != 0)
            .map(|(_, stride)| stride * last)
            .sum::<usize>();
        points[index]
    });
    let origin = corners[0];
    let deltas = [corners[1], corners[2], corners[4]]
        .map(|corner| std::array::from_fn::<_, 3, _>(|channel| corner[channel] - origin[channel]));
    // Check the full red-fastest lattice, including the four dependent corners.
    // Allow decimal rounding of stored samples to six places.
    for (index, point) in points.iter().enumerate() {
        let coordinate = steps.map(|stride| (index / stride % edge) as f64 / last as f64);
        for (channel, actual) in point.iter().enumerate() {
            let expected = origin[channel]
                + (0..3)
                    .map(|axis| deltas[axis][channel] * coordinate[axis])
                    .sum::<f64>();
            if (actual - expected).abs() > 1e-6 {
                return None;
            }
        }
    }
    Some(corners.map(|[r, g, b]| (r, g, b)))
}

fn read_asset(path: PathBuf, byte_limit: u64) -> Result<(String, SourceInput), String> {
    let metadata = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.len() > byte_limit {
        return Err(format!(
            "{} is not a regular LUT asset within the byte limit",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|f| f.take(byte_limit + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > byte_limit {
        return Err(format!("{} exceeds the LUT read limit", path.display()));
    }
    let revision = prproj::hash::hex(&prproj::hash::sha256(&bytes));
    let contents =
        String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
    Ok((contents, SourceInput { path, revision }))
}

fn load(folder: &Path, name: &str, filename: &str) -> Result<(Cube, [SourceInput; 2]), String> {
    let (descriptor, descriptor_pin) = read_asset(
        folder.join(filename).with_extension("xml"),
        MAX_DESCRIPTOR_BYTES,
    )?;
    if xml_lut(&descriptor, false).is_none_or(|(n, f)| n != name || f != filename) {
        return Err("the project LUT descriptor disagrees with the edit's saved transform".into());
    }
    let (contents, cube_pin) = read_asset(folder.join(filename), MAX_CUBE_BYTES)?;
    let points = cube(&contents).ok_or(
        "use a complete affine cube with edge length 2–65 and an explicit unit RGB domain",
    )?;
    Ok((points, [descriptor_pin, cube_pin]))
}

pub(crate) fn apply(
    prepared: &mut premiere_adapter::PreparedPremiereInput,
    graph: &SourceGraph,
    bins: &[SourceInput],
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Applied, String> {
    let mut result = Applied::default();
    if prepared.input.options.bin_mode != "separate" || !has_candidates(graph) {
        return Ok(result);
    }
    let items: HashMap<_, _> = graph
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let media: HashMap<_, _> = graph
        .media
        .iter()
        .map(|media| (media.id.as_str(), media))
        .collect();
    let clips: HashMap<_, _> = prepared
        .input
        .clips
        .iter()
        .map(|clip| (clip.id(), clip))
        .collect();
    let sidecars: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.source")
        .map(|s| (s.object.as_str(), &s.data))
        .collect();
    let mut cubes = HashMap::new();
    let mut pictures = HashMap::new();
    let mut pinned = HashSet::new();
    let mut verified_pictures = HashSet::new();
    for (sequence_index, source) in graph.sequences.iter().enumerate() {
        let Some(sidecar) = sidecars.get(source.id.as_str()) else {
            continue;
        };
        let (Some(Value::Arr(issues)), Some(Value::Arr(native_events))) =
            (sidecar.get("unresolved"), sidecar.get("native_events"))
        else {
            continue;
        };
        let Some(sequence) = prepared.input.sequences.get(sequence_index) else {
            continue;
        };
        let rate = Rational::integer(24);
        if source.rate != rate
            || sequence.rate != Fraction::new(24, 1)
            || (sequence.width, sequence.height) != (1920, 1080)
            || source
                .frame_size
                .is_some_and(|canvas| canvas != (1920, 1080))
            || source
                .tracks
                .iter()
                .any(|track| track.kind == StreamKind::Video && track.index != 1)
            || source
                .transitions
                .iter()
                .any(|t| t.kind == StreamKind::Video)
        {
            continue;
        }
        let candidates: Vec<_> = issues.iter().filter_map(adapter).collect();
        let mut counts = HashMap::new();
        for facts in &candidates {
            *counts
                .entry(text(facts, "placement").unwrap())
                .or_insert(0usize) += 1;
        }
        let placements: HashMap<_, _> = source
            .placements
            .iter()
            .map(|p| (p.id.as_str(), p))
            .collect();
        let plain_color_events: HashSet<_> = native_events
            .iter()
            .filter_map(|row| {
                let Some(Value::Arr(via)) = row.get("via") else {
                    return None;
                };
                let [Value::Str(stage)] = via.as_slice() else {
                    return None;
                };
                (stage == "effect EFF2_LUTSFX not converted")
                    .then(|| text(row, "placement_id"))
                    .flatten()
            })
            .collect();
        for facts in candidates {
            check()?;
            let owner = text(facts, "placement").unwrap();
            if counts[owner] != 1 {
                continue;
            }
            let attempt = (|| -> Result<(), String> {
                if !plain_color_events.contains(owner) {
                    return Err("the Color Adapter is part of an unqualified picture stack".into());
                }
                let (name, filename) = text(facts, "text")
                    .and_then(|s| xml_lut(s, true))
                    .ok_or("unqualified external LUT descriptor")?;
                let placement = placements
                    .get(owner)
                    .filter(|p| {
                        p.video_track == Some(1)
                            && p.speed.is_none()
                            && p.frame_map.is_none()
                            && p.transform.is_none()
                            && p.effects.is_empty()
                    })
                    .ok_or("the source edit has an unresolved picture stack")?;
                let id = placement.item.as_str();
                let item = items.get(id).ok_or("unresolved source item")?;
                let [binding] = &item
                    .media
                    .iter()
                    .filter(|m| {
                        media
                            .get(m.media.as_str())
                            .is_some_and(|m| m.kind == StreamKind::Video)
                    })
                    .collect::<Vec<_>>()[..]
                else {
                    return Err("source item needs one known picture file".into());
                };
                let picture = media[binding.media.as_str()];
                let frames = picture
                    .duration
                    .to_units(24)
                    .ok_or("unknown source picture duration")?;
                let [path] = picture.locations.as_slice() else {
                    return Err("ambiguous picture path".into());
                };
                let clip = id.strip_prefix("clip:").unwrap_or(id);
                let legacy_path = match clips.get(clip) {
                    Some(PreparedClip::Legacy(c)) => c.video.as_ref().map(|v| v.path.as_str()),
                    _ => None,
                };
                if picture.online != Some(true)
                    || picture.original.is_some()
                    || picture.rate != Some(rate)
                    || picture.frame_size != Some((1920, 1080))
                    || picture.start != Some(Rational::integer(0))
                    || binding.source_offset != Some(Rational::integer(0))
                    || legacy_path != Some(path)
                {
                    return Err("the edit does not use its saved Rec.709 managed picture".into());
                }
                let package = text(facts, "picture_file").unwrap();
                let mut matching = sequence.events.iter().enumerate().filter(|(_, event)| {
                    event.clip == clip
                        && event.video_track == Some(1)
                        && event.transform.is_none()
                        && event.matte.is_none()
                        && event.color_correction.is_none()
                        && event.speed_ratio.is_none()
                        && event.time_remap.is_none()
                        && placement.record_start.to_units(TICKS_PER_SECOND)
                            == Some(event.record_start)
                        && placement.source_in.to_units(TICKS_PER_SECOND) == Some(event.source_in)
                        && placement.duration.to_units(TICKS_PER_SECOND) == Some(event.duration)
                });
                let (event, _) = matching
                    .next()
                    .filter(|_| matching.next().is_none())
                    .ok_or("the destination picture edit or trim is unresolved")?;
                if prepared
                    .input
                    .picture_effects
                    .iter()
                    .any(|e| (e.sequence, e.event) == (sequence_index, event))
                {
                    return Err("the destination edit already carries a picture effect".into());
                }
                let folder = project_lut_folder(&source.id, bins)?;
                let key = (folder, name, filename.clone());
                if !cubes.contains_key(&key) && cubes.len() == MAX_DISTINCT_INPUTS {
                    return Err(
                        "project exceeds the supported number of distinct LUT assets".into(),
                    );
                }
                let (points, inputs) = cubes
                    .entry(key)
                    .or_insert_with_key(|(folder, name, filename)| load(folder, name, filename))
                    .as_ref()
                    .map_err(Clone::clone)?;
                if !pictures.contains_key(&(path, package, frames))
                    && pictures.len() == MAX_DISTINCT_INPUTS
                {
                    return Err(
                        "project exceeds the supported number of distinct LUT picture files".into(),
                    );
                }
                let stamp = pictures
                    .entry((path, package, frames))
                    .or_insert_with(|| {
                        let linked = Path::new(path);
                        let stamp = original_stamp(linked)?;
                        let path = linked.canonicalize().map_err(|e| e.to_string())?;
                        avid_adapter::original::verify_source_lut_picture(
                            &path, ffprobe, package, frames,
                        )?;
                        recheck_originals(std::slice::from_ref(&stamp))?;
                        Ok::<_, String>(stamp)
                    })
                    .as_ref()
                    .map_err(Clone::clone)?;
                if verified_pictures.insert(stamp.path.clone()) {
                    result.pictures.push(stamp.clone());
                }
                for input in inputs {
                    if pinned.insert(input.path.clone()) {
                        result.inputs.push(input.clone());
                    }
                }
                prepared.input.source_luts.push(PreparedSourceLut {
                    sequence: sequence_index,
                    event,
                    filename: filename.clone(),
                    points: *points,
                });
                let subject = crate::premiere_preparation::timeline_subject(
                    source,
                    StreamKind::Video,
                    1,
                    Some(placement.record_start),
                );
                prepared.diagnostics.push(format!(
                    "{subject}: embedded {filename:?} as an editable clip Lumetri Look."
                ));
                result.recovered.insert(owner.into());
                Ok(())
            })();
            if let Err(reason) = attempt {
                let position = placements
                    .get(owner)
                    .map(|placement| placement.record_start);
                let subject = crate::premiere_preparation::timeline_subject(
                    source,
                    StreamKind::Video,
                    1,
                    position,
                );
                prepared
                    .diagnostics
                    .push(format!("{subject}: source LUT unavailable: {reason}."));
            }
        }
    }
    prepared.losses.retain(|loss| result.keeps_loss(loss));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_requires_the_exact_domain_and_an_affine_lattice() {
        let cube_text =
            "TITLE \"Swap red and blue\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n\
            0 0 0\n0 0 1\n0 1 0\n0 1 1\n1 0 0\n1 0 1\n1 1 0\n1 1 1\n";
        assert_eq!(cube(cube_text).unwrap()[1], (0.0, 0.0, 1.0));
        assert!(cube(&cube_text.replace("DOMAIN_MIN 0 0 0", "DOMAIN_MIN 0 0 0.1")).is_none());
        assert!(cube(&cube_text.replace("\n1 1 1\n", "\n0.9 1 1\n")).is_none());
        assert!(cube(&cube_text.replace("\n0 0 0\n", "\nNaN 0 0\n")).is_none());
        assert!(cube(&format!("{cube_text}0 0 0\n")).is_none());
    }

    #[test]
    fn larger_grid_reduces_to_the_same_corners_and_rejects_an_interior_change() {
        // Third-step samples exercise the tolerance for decimal rounding.
        let mut rows = vec![
            "LUT_3D_SIZE 4".into(),
            "DOMAIN_MIN 0 0 0".into(),
            "DOMAIN_MAX 1 1 1".into(),
        ];
        for b in 0..4 {
            for g in 0..4 {
                for r in 0..4 {
                    rows.push(format!(
                        "{:.6} {:.6} {:.6}",
                        b as f64 / 3.0,
                        g as f64 / 3.0,
                        r as f64 / 3.0
                    ));
                }
            }
        }
        let expected =
            std::array::from_fn(|i| (((i >> 2) & 1) as f64, ((i >> 1) & 1) as f64, (i & 1) as f64));
        assert_eq!(cube(&rows.join("\n")), Some(expected));
        assert!(cube(&rows[..rows.len() - 1].join("\n")).is_none());
        rows[3 + 1 + 4 + 16] = "0.334 0.333333 0.333333".into();
        assert!(cube(&rows.join("\n")).is_none());
        assert!(cube("LUT_3D_SIZE 66\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n").is_none());
    }

    #[test]
    fn descriptor_requires_the_same_external_cube_fields() {
        let xml = "<ColorTransformation><ExternalLut><Name>Test (3D)</Name>\
            <LutFileName>Test.cube</LutFileName><LutDimension>3D</LutDimension>\
            <InhibitScaling/></ExternalLut></ColorTransformation>";
        assert_eq!(
            xml_lut(xml, false),
            Some(("Test (3D)".into(), "Test.cube".into()))
        );
        assert_eq!(
            xml_lut(xml, false),
            xml_lut(
                &format!("<ColorTransformationList>{xml}</ColorTransformationList>"),
                true
            )
        );
        assert!(xml_lut(&xml.replace("Test.cube", "../Test.cube"), false).is_none());
        assert!(xml_lut(&xml.replace("<InhibitScaling/>", ""), false).is_none());
    }
}
