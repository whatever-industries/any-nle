//! Qualify saved Avid source color controls for one Premiere picture edit.
use crate::source_input::{original_stamp, recheck_originals, OriginalStamp};
use avid_reader::source::{is_canonical_external_cube, saved_color_transforms_match, SourceColorRange, SourceColorStack, SourceInput};
use graph::{source::SourceGraph, Rational, StreamKind, Value};
use prproj::{
    convert::{
        model::{ticks_per_unit, Fraction, TICKS_PER_SECOND},
        prepared::{supports_picture_motion_clock, supported_extended_sdr_edit, PictureEffect, PreparedClip, PreparedPictureEffect, PreparedSourceLut},
    },
    xml::{NodeId, Tree},
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

const ASPECT: &str = "source_color_adapter";
const MAX_DESCRIPTOR_BYTES: u64 = 16 * 1024;
const MAX_CUBE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CUBE_EDGE: usize = 65;
const MAX_DISTINCT_LUT_INPUTS: usize = 32;
// Charge the picture index, including its end boundary, and at least one
// probe's worth of work per distinct managed input. Failed probes also count.
const MANAGED_PICTURE_BUDGET: u64 = 4 * 1024 * 1024;
const MIN_MANAGED_PICTURE_CHARGE: u64 = 1024;

#[derive(Default)]
pub(crate) struct Applied {
    pub inputs: Vec<SourceInput>,
    pub pictures: Vec<OriginalStamp>,
    pub managed_pictures: crate::rewrap::LtRangePictures,
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorMedia {
    Linked,
    Rewrap,
}

struct Adapter<'a> {
    facts: &'a Value,
    owner: &'a str,
    reordered: bool,
    stack: Option<SourceColorStack<'a>>,
}

fn adapter(row: &Value, media: ColorMedia) -> Option<Adapter<'_>> {
    if text(row, "effect_id") != Some("EFF2_LUTSFX") {
        return None;
    }
    let facts = row.get("color_adapter")?;
    let xml = text(facts, "text")?;
    let neutral = facts.get("neutral_wrapper") == Some(&Value::Bool(true));
    if let Some(stack) = SourceColorStack::from_saved_xml(xml) {
        if facts.get("subclip_motion").is_some() {
            return None;
        }
        let (owner, reordered) = if neutral && text(facts, "picture_file").is_some() {
            (text(facts, "placement")?, false)
        } else {
            let proof = facts.get("reordered_picture_source")?;
            text(proof, "picture_file")?;
            (text(proof, "placement")?, true)
        };
        return Some(Adapter { facts, owner, reordered, stack: Some(stack) });
    }
    let range = SourceColorRange::from_saved_xml(xml);
    let expand = range == Some(SourceColorRange::VideoToFull);
    let managed = facts.get("managed_picture_span").is_some() && expand;
    let nested = nested_picture_file(facts).is_some()
        && (neutral
            || (media == ColorMedia::Rewrap && facts.get("managed_picture_span").is_some()));
    let qualified_shape = match media {
        ColorMedia::Linked => neutral
            && (xml.contains("<ExternalLut>") || expand
                || (!nested && range == Some(SourceColorRange::FullToVideo))),
        ColorMedia::Rewrap => managed || (nested && (expand
            || (facts.get("managed_picture_span").is_some() && is_canonical_external_cube(xml)))),
    };
    (qualified_shape && (nested || text(facts, "picture_file").is_some()))
        .then_some(Adapter { facts, owner: text(facts, "placement")?, reordered: false, stack: None })
}

fn nested_picture_file(facts: &Value) -> Option<&str> {
    text(facts.get("subclip_motion")?, "sampled_picture_file")
}

fn source_transform<'a>(source: Option<&'a Value>, package: &str) -> Option<&'a str> {
    let Some(Value::Arr(rows)) = source.and_then(|source| source.get("input_color_transforms")) else {
        return None;
    };
    let [row] = rows.as_slice() else { return None };
    if text(row, "owner_mob") != Some(package) || row.get("track") != Some(&Value::Int(1)) {
        return None;
    }
    text(row, "text")
}

pub(crate) fn has_candidates(graph: &SourceGraph, media: ColorMedia) -> bool {
    graph.sidecars.iter().any(|sidecar| {
        sidecar.adapter == "avid.source"
            && matches!(sidecar.data.get("unresolved"), Some(Value::Arr(rows))
            if rows.iter().any(|row| adapter(row, media).is_some()))
    })
}

fn project_lut_folder(id: &str, bins: &[SourceInput], nested_bin: bool) -> Result<PathBuf, String> {
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
    let parent = bin.ancestors().skip(1).take(if nested_bin { 2 } else { 1 })
        .find(|parent| parent.file_name().is_some_and(|name| {
            let mut project_name = name.to_os_string();
            project_name.push(".avp");
            parent.join(project_name).is_file()
        }))
        .ok_or("select the saved bin in its Avid project folder with its LUTs directory")?;
    let folder = parent.join("LUTs");
    if !fs::symlink_metadata(&folder).is_ok_and(|m| m.is_dir()) {
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
type EmbeddedCube = (u32, Arc<[(f64, f64, f64)]>);

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum CubeUse {
    Affine,
    Retimed,
    Ordered { cube_first: bool },
}

impl CubeUse {
    fn filename(self, filename: &str) -> String {
        match self {
            Self::Ordered { cube_first } => {
                let stem = filename.strip_suffix(".cube").unwrap_or(filename);
                let order = if cube_first { " - cube then range.cube" } else { " - range then cube.cube" };
                format!("{}{order}", &stem[..stem.len().min(128 - order.len())])
            }
            _ => filename.to_owned(),
        }
    }
}

fn rgb(mut fields: std::str::SplitWhitespace<'_>) -> Option<[f64; 3]> {
    let point = [
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ];
    (fields.next().is_none() && point.iter().all(|v| (0.0..=1.0).contains(v))).then_some(point)
}

fn cube_samples(s: &str, allow_implicit_domain: bool) -> Option<(usize, Vec<[f64; 3]>)> {
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
            _ if (allow_implicit_domain || (minimum && maximum)) && points.len() < edge?.pow(3) => {
                points.push(rgb(line.split_whitespace())?);
            }
            _ => return None,
        }
    }
    let edge = edge?;
    if points.len() != edge.pow(3) || minimum != maximum {
        return None;
    }
    Some((edge, points))
}

#[cfg(test)]
fn cube(s: &str) -> Option<Cube> {
    let (edge, points) = cube_samples(s, false)?;
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

fn tetrahedral(points: &[[f64; 3]], edge: usize, rgb: [f64; 3]) -> [f64; 3] {
    // The Avid cube lookup clamps its inputs and uses a red-fastest lattice.
    let last = edge - 1;
    let coordinate = rgb.map(|component| component.clamp(0.0, 1.0) * last as f64);
    let base = coordinate.map(|value| (value as usize).min(last - 1));
    let fraction = std::array::from_fn::<_, 3, _>(|axis| coordinate[axis] - base[axis] as f64);
    let strides = [1, edge, edge * edge];
    let mut order = [0, 1, 2];
    order.sort_by(|&a, &b| fraction[b].total_cmp(&fraction[a]));
    let mut index = base[0] + edge * base[1] + edge * edge * base[2];
    let mut result = points[index];
    for axis in order {
        let before = points[index];
        index += strides[axis];
        for channel in 0..3 {
            result[channel] += fraction[axis] * (points[index][channel] - before[channel]);
        }
    }
    result
}

fn ordered_cube(points: &[[f64; 3]], edge: usize, cube_first: bool) -> Result<EmbeddedCube, String> {
    let expand = |rgb: [f64; 3]| rgb.map(|v| {
        let black = SourceColorRange::VIDEO_BLACK as f64;
        let white = SourceColorRange::VIDEO_WHITE as f64;
        (v * ((1 << SourceColorRange::BIT_DEPTH) - 1) as f64 - black) / (white - black)
    });
    let output_edge = PreparedSourceLut::ORDERED_EDGE;
    let points = prproj::convert::avid_color::cube_lut(&|r, g, b| {
        let rgb = [r, g, b];
        let [r, g, b] = if cube_first {
            expand(tetrahedral(points, edge, rgb))
        } else {
            tetrahedral(points, edge, expand(rgb))
        };
        (r, g, b)
    }, i64::from(output_edge)).map_err(|error| error.to_string())?;
    Ok((output_edge, points.into()))
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

fn load(folder: &Path, name: &str, filename: &str, usage: CubeUse) -> Result<(EmbeddedCube, [SourceInput; 2]), String> {
    let (descriptor, descriptor_pin) = read_asset(
        folder.join(filename).with_extension("xml"),
        MAX_DESCRIPTOR_BYTES,
    )?;
    if xml_lut(&descriptor, false).is_none_or(|(n, f)| n != name || f != filename) {
        return Err("the project LUT descriptor disagrees with the edit's saved transform".into());
    }
    let (contents, cube_pin) = read_asset(folder.join(filename), MAX_CUBE_BYTES)?;
    let error = match usage {
        CubeUse::Affine => "use a complete affine cube with edge length 2–65 and an explicit unit RGB domain",
        CubeUse::Retimed => "the retimed LUT needs a complete 33×33×33 cube with a unit RGB domain",
        CubeUse::Ordered { .. } => "the ordered Color Adapter needs a complete 33×33×33 cube with an explicit unit RGB domain",
    };
    let (edge, samples) = cube_samples(&contents, usage == CubeUse::Retimed).ok_or(error)?;
    let grid = match usage {
        CubeUse::Affine => (2, Arc::from(affine_corners(&samples, edge).ok_or(error)?)),
        CubeUse::Retimed if edge as u32 == PreparedSourceLut::FULL_EDGE => {
            (edge as u32, samples.into_iter().map(|[r, g, b]| (r, g, b)).collect())
        }
        CubeUse::Ordered { cube_first } if edge as u32 == PreparedSourceLut::FULL_EDGE => {
            ordered_cube(&samples, edge, cube_first)?
        }
        _ => return Err(error.into()),
    };
    Ok((grid, [descriptor_pin, cube_pin]))
}

pub(crate) fn prepare_extended_sdr(
    prepared: &mut premiere_adapter::PreparedPremiereInput,
    graph: &SourceGraph,
    ffprobe: &Path,
    result: &mut Applied,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let input = &mut prepared.input;
    if input.options.bin_mode != "separate" || !input.sequences.iter().any(|s| {
        s.events.iter().any(|e| e.color_correction.as_ref()
            .and_then(prproj::convert::avid_color::ExtendedSdrGrade::from_correction).is_some())
    }) {
        return Ok(());
    }
    let clips: HashMap<_, _> = input.clips.iter().map(|c| (c.id(), c)).collect();
    let sidecars: HashMap<_, _> = graph.sidecars.iter().filter(|s| s.adapter == "avid.source")
        .map(|s| (s.object.as_str(), &s.data)).collect();
    let losses: HashSet<_> = graph.losses.iter()
        .filter(|l| matches!(l.aspect.as_str(), "source_color_adapter" | "picture_effect" | "transform" | "matte"))
        .map(|l| l.object.as_str()).collect();
    let mut other_picture = HashSet::new();
    other_picture.extend(input.picture_effects.iter().map(|e| (e.sequence, e.event)));
    other_picture.extend(input.source_luts.iter().map(|e| (e.sequence, e.event)));
    other_picture.extend(input.multicam_uses.iter().map(|e| (e.sequence, e.event)));
    other_picture.extend(input.picture_nests.iter().map(|e| (e.sequence, e.event)));
    for dissolve in &input.video_dissolves {
        other_picture.extend([(dissolve.sequence, dissolve.outgoing), (dissolve.sequence, dissolve.incoming)]);
    }
    let mut verified = HashMap::new();
    for (si, source) in graph.sequences.iter().enumerate() {
        let Some(sequence) = input.sequences.get(si) else { continue };
        if losses.contains(source.id.as_str()) {
            continue;
        }
        for placement in &source.placements {
            let Some(&(sequence_index, ei)) = prepared.ownership.get(&placement.id) else { continue };
            if sequence_index != si || other_picture.contains(&(si, ei)) || !matches!(placement.effects.as_slice(),
                [effect] if effect.kind == "avid.color_correction"
                    && effect.params.get("measured_static_tone") == Some(&Value::Bool(true)))
                || losses.contains(placement.id.as_str())
                || sidecars.get(placement.item.as_str())
                    .is_some_and(|s| s.get("input_color_transforms").is_some())
                || input.disabled_edits.contains(&(si, ei))
            {
                continue;
            }
            let Some(edit) = sequence.events.get(ei) else { continue };
            let Some(clip @ PreparedClip::Original(original)) = clips.get(edit.clip.as_str()).copied()
                else { continue };
            if !supported_extended_sdr_edit(sequence, edit, clip) { continue }
            let video = original.video.as_ref().expect("qualified extended SDR picture");
            let frames = video.duration / ticks_per_unit(video.rate);
            check()?;
            let probe = verified.entry((&original.path, frames)).or_insert_with(|| {
                let path = Path::new(&original.path);
                let stamp = original_stamp(path)?;
                avid_adapter::original::verify_extended_sdr_picture(path, ffprobe, frames)?;
                recheck_originals(std::slice::from_ref(&stamp))?;
                result.pictures.push(stamp);
                Ok::<_, String>(())
            });
            if probe.is_ok() {
                input.extended_sdr_grades.insert((si, ei));
            }
        }
    }
    Ok(())
}

pub(crate) fn apply(
    prepared: &mut premiere_adapter::PreparedPremiereInput,
    graph: &SourceGraph,
    bins: &[SourceInput],
    ffprobe: &Path,
    color_media: ColorMedia,
    approved_managed: Option<&HashSet<&str>>,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Applied, String> {
    let mut result = Applied::default();
    if prepared.input.options.bin_mode != "separate" || !has_candidates(graph, color_media) {
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
    let nested_owners: HashSet<_> = graph.sidecars.iter()
        .filter(|sidecar| sidecar.adapter == "avid.nested_motion")
        .map(|sidecar| sidecar.object.as_str())
        .collect();
    let mut cubes = HashMap::<(PathBuf, String, String, CubeUse), _>::new();
    let mut pictures = HashMap::new();
    let mut managed_pictures = HashMap::new();
    let mut managed_picture_budget = MANAGED_PICTURE_BUDGET;
    let mut full_grid_edits = 0;
    let mut ordered_edits = 0;
    let mut pinned = HashMap::<PathBuf, String>::new();
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
        let rate = source.rate;
        let legacy_clock = rate == Rational::integer(24);
        if sequence.rate != Fraction::new(rate.num(), rate.den())
            || !supports_picture_motion_clock(sequence.rate)
            || (sequence.width, sequence.height) != (1920, 1080)
            || source
                .frame_size
                .is_some_and(|canvas| canvas != (1920, 1080))
        {
            continue;
        }
        let linked_stack_or_dissolve = color_media == ColorMedia::Linked
            && (source.tracks.iter().any(|track| track.kind == StreamKind::Video && track.index != 1)
                || source.transitions.iter().any(|t| t.kind == StreamKind::Video));
        let candidates: Vec<_> = issues.iter().filter_map(|row| adapter(row, color_media)).collect();
        let mut counts = HashMap::new();
        for candidate in &candidates {
            *counts.entry(candidate.owner).or_insert(0usize) += 1;
        }
        let placements: HashMap<_, _> = source
            .placements
            .iter()
            .map(|p| (p.id.as_str(), p))
            .collect();
        let video_transition_endpoints: HashSet<_> = source.transitions.iter()
            .filter(|t| t.kind == StreamKind::Video)
            .flat_map(|t| t.outgoing.placements.iter().chain(&t.incoming.placements))
            .map(String::as_str)
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
        for Adapter { facts, owner, reordered, stack } in candidates {
            check()?;
            if counts[owner] != 1 {
                continue;
            }
            let nested = nested_owners.contains(owner);
            let xml = text(facts, "text").unwrap();
            let range = SourceColorRange::from_saved_xml(xml);
            let expand = range == Some(SourceColorRange::VideoToFull);
            // A plain range transform belongs to one opaque picture edit.
            // Other linked Color Adapter profiles keep their original layout checks.
            let plain_range = !nested && range.is_some();
            if !legacy_clock && (!plain_range || color_media != ColorMedia::Linked) {
                continue;
            }
            if linked_stack_or_dissolve && !plain_range {
                continue;
            }
            let attempt = (|| -> Result<(), String> {
                if !nested && !plain_color_events.contains(owner) {
                    return Err("the Color Adapter is part of an unqualified picture stack".into());
                }
                if range == Some(SourceColorRange::FullToVideo) && nested {
                    return Err("full-to-video scaling requires a plain linked picture edit".into());
                }
                let placement = placements
                    .get(owner)
                    .filter(|p| {
                        p.video_track.is_some()
                            && (color_media == ColorMedia::Rewrap || plain_range || p.video_track == Some(1))
                            && (p.speed.is_none() || (nested && p.speed.as_ref().is_some_and(|s|
                                s.ratio == (1, 2) || (color_media == ColorMedia::Linked && expand
                                    && s.ratio.0 > 0 && s.ratio.1 > 0))))
                            && p.frame_map.is_none()
                            && p.transform.is_none()
                            && p.effects.is_empty()
                    })
                    .ok_or("the source edit has an unresolved picture stack")?;
                if video_transition_endpoints.contains(owner) {
                    return Err("the Color Adapter is on a video transition endpoint".into());
                }
                let id = placement.item.as_str();
                let rewrap_picture = color_media == ColorMedia::Rewrap || stack.is_some()
                    && approved_managed.is_some_and(|items| items.contains(id));
                if stack.is_some() && !rewrap_picture {
                    return Err("ordered Color Adapter requires a verified managed ProRes LT rewrap".into());
                }
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
                let frames = Rational::new(
                    i128::from(picture.duration.num()) * i128::from(rate.num()),
                    i128::from(picture.duration.den()) * i128::from(rate.den()),
                )
                    .filter(|count| count.den() == 1 && count.num() > 0)
                    .map(Rational::num)
                    .ok_or("unknown source picture duration")?;
                let [path] = picture.locations.as_slice() else {
                    return Err("ambiguous picture path".into());
                };
                let clip = id.strip_prefix("clip:").unwrap_or(id);
                let package = if reordered {
                    facts.get("reordered_picture_source").and_then(|row| text(row, "picture_file"))
                } else if nested {
                    nested_picture_file(facts)
                } else {
                    text(facts, "picture_file")
                }.ok_or("missing saved picture package")?;
                if picture.online != Some(true)
                    || picture.rate != Some(rate)
                    || picture.frame_size != Some((1920, 1080))
                    || binding.source_offset != Some(Rational::integer(0))
                {
                    return Err("the edit does not use a qualified online picture".into());
                }
                if range.is_some() || rewrap_picture {
                    let saved_picture = sidecars.get(binding.media.as_str()).copied();
                    let saved_package = package.strip_prefix("urn:smpte:umid:")
                        .map(|package| package.replace('.', ""));
                    let same_package = saved_picture.is_some_and(|saved| {
                        saved.get("binding").is_none()
                            && saved_package.as_ref().is_some_and(|package| {
                                text(saved, "file_package").is_some_and(|id|
                                    id.eq_ignore_ascii_case(package))
                            })
                    });
                    let picture_mode = if rewrap_picture { ColorMedia::Rewrap } else { ColorMedia::Linked };
                    let qualified_media = match picture_mode {
                        ColorMedia::Linked if nested => matches!(clips.get(clip), Some(PreparedClip::Legacy(c))
                            if c.video.as_ref().is_some_and(|v| &v.path == path && v.codec == "AVdn"))
                            && picture.original.is_none() && picture.start == Some(Rational::ZERO),
                        ColorMedia::Linked => matches!(clips.get(clip), Some(PreparedClip::Original(c))
                            if &c.path == path && c.audio.is_none() && c.proxy_path.is_none()
                                && c.video.as_ref().is_some_and(|v|
                                    (legacy_clock && v.codec == "apch")
                                        || (plain_range && v.codec == "avc1")))
                            && picture.original.as_ref().is_some_and(|binding|
                                binding.container == "mov" && binding.stream_index == 0),
                        ColorMedia::Rewrap => matches!(clips.get(clip), Some(PreparedClip::Legacy(c))
                            if c.audio.is_empty()
                                && c.video.as_ref().is_some_and(|v| &v.path == path && v.codec == "apcs"))
                            && picture.original.is_none() && picture.start == Some(Rational::integer(0)),
                    };
                    let source = sidecars.get(id).copied();
                    let saved_transform = source_transform(source, package);
                    let source_matches = if reordered {
                        stack.zip(saved_transform.and_then(SourceColorStack::from_saved_xml))
                            .is_some_and(|(edit, file)| edit.is_reordered(file))
                    } else {
                        saved_transform.is_some_and(|saved| saved_color_transforms_match(xml, saved))
                    };
                    let edit_owned = stack.is_none() && (nested || (color_media == ColorMedia::Rewrap
                        && facts.get("managed_picture_span").is_some()))
                        && source.and_then(|s| s.get("input_color_transforms")).is_none();
                    if !qualified_media || !same_package || !(source_matches || edit_owned)
                    {
                        if stack.is_some() {
                            return Err("ordered Color Adapter requires matching saved transforms and the picture-only Avid ProRes LT package".into());
                        }
                        return Err(match color_media {
                            ColorMedia::Linked if nested => "nested range expansion requires its verified managed DNxHD package and matching color controls",
                            ColorMedia::Linked => "range scaling requires the saved source transform on its verified picture-only MOV",
                            ColorMedia::Rewrap if expand => "range expansion requires matching saved transforms and the picture-only Avid ProRes LT rewrap",
                            ColorMedia::Rewrap => "the retimed source LUT requires matching saved transforms and the picture-only Avid ProRes LT rewrap",
                        }.into());
                    }
                } else if picture.original.is_some()
                    || picture.start != Some(Rational::integer(0))
                    || !matches!(clips.get(clip), Some(PreparedClip::Legacy(c))
                        if c.video.as_ref().is_some_and(|v| v.path == *path))
                {
                    return Err("the edit does not use its saved Rec.709 managed picture".into());
                }
                // The prepared placement binding distinguishes repeated media,
                // including simultaneous edits with identical trims on other lanes.
                let &(_, event) = prepared.ownership.get(owner)
                    .filter(|(sequence, _)| *sequence == sequence_index)
                    .ok_or("the destination picture edit is unresolved")?;
                let source_speed = placement.speed.as_ref().map(|speed| speed.ratio);
                sequence.events.get(event).filter(|event| {
                    event.clip == clip
                        && event.video_track == placement.video_track
                        && event.transform.is_none()
                        && event.matte.is_none()
                        && event.color_correction.is_none()
                        && event.speed_ratio == source_speed
                        && event.time_remap.is_none()
                        && placement.record_start.to_units(TICKS_PER_SECOND)
                            == Some(event.record_start)
                        && placement.source_in.to_units(TICKS_PER_SECOND) == Some(event.source_in)
                        && placement.duration.to_units(TICKS_PER_SECOND) == Some(event.duration)
                }).ok_or("the destination picture edit or trim is unresolved")?;
                if nested && !prepared.input.constant_speeds.iter().any(|speed| {
                    speed.sequence == sequence_index && speed.event == event
                        && source_speed == Some((speed.ratio.1, speed.ratio.0))
                }) {
                    return Err(if source_speed == Some((1, 2)) {
                        "the destination edit lacks its verified 200% time mapping"
                    } else {
                        "the destination edit lacks its verified constant-speed time mapping"
                    }.into());
                }
                if prepared
                    .input
                    .picture_effects
                    .iter()
                    .any(|e| (e.sequence, e.event) == (sequence_index, event))
                {
                    return Err("the destination edit already carries a picture effect".into());
                }
                if stack.is_some() && (nested || placement.video_track != Some(1) || !legacy_clock
                    || placement.source_in.to_units(24).map(Value::Int).as_ref() != facts.get("source_in"))
                {
                    return Err("ordered Color Adapter requires one exact, unretimed V1 picture trim".into());
                }
                let external = if range.is_some() {
                    None
                } else {
                    let usage = match stack {
                        Some(stack) => CubeUse::Ordered { cube_first: stack.cube_first },
                        None if nested && color_media == ColorMedia::Rewrap => CubeUse::Retimed,
                        None => CubeUse::Affine,
                    };
                    if usage == CubeUse::Retimed && full_grid_edits == PreparedSourceLut::MAX_FULL_GRID_EDITS {
                        return Err("the project exceeds the supported number of full-grid retimed LUT edits".into());
                    }
                    if stack.is_some() && ordered_edits == PreparedSourceLut::MAX_ORDERED_EDITS {
                        return Err("the project exceeds the supported number of ordered Color Adapter edits".into());
                    }
                    let (name, filename) = match stack {
                        Some(stack) => (stack.cube_name.to_owned(), stack.cube_file.to_owned()),
                        None => xml_lut(xml, true).ok_or("unqualified external LUT descriptor")?,
                    };
                    let folder = project_lut_folder(&source.id, bins, usage != CubeUse::Affine)?;
                    let output_name = usage.filename(&filename);
                    let key = (folder, name, filename, usage);
                    if !cubes.contains_key(&key) && cubes.len() == MAX_DISTINCT_LUT_INPUTS {
                        return Err("project exceeds the supported number of distinct LUT assets".into());
                    }
                    if stack.is_some() && !cubes.contains_key(&key)
                        && cubes.keys().filter(|key| matches!(key.3, CubeUse::Ordered { .. })).count()
                            == PreparedSourceLut::MAX_ORDERED_EDITS
                    {
                        return Err("project exceeds the supported number of ordered LUT tables".into());
                    }
                    let (grid, inputs) = cubes
                        .entry(key)
                        .or_insert_with_key(|(folder, name, filename, usage)| load(folder, name, filename, *usage))
                        .as_ref()
                        .map_err(Clone::clone)?;
                    if inputs.iter().any(|input| pinned.get(&input.path)
                        .is_some_and(|revision| revision != &input.revision))
                    {
                        return Err("the project LUT assets changed during preparation".into());
                    }
                    Some((output_name, grid, inputs))
                };
                let picture_key = (path, package, frames, rate, range.is_some());
                let stamp = if rewrap_picture {
                    let saved_bytes = if let Some(span) = facts.get("managed_picture_span") {
                        let (Some(Value::Int(offset)), Some(Value::Int(bytes))) =
                            (span.get("offset"), span.get("length"))
                        else {
                            return Err("missing saved picture byte bounds".into());
                        };
                        Some((*offset, *bytes))
                    } else if nested || stack.is_some() {
                        None
                    } else {
                        return Err("no saved managed picture range".into());
                    };
                    let start = placement.source_in.to_units(24).ok_or("missing exact source trim")?;
                    let length = placement.speed.as_ref().map_or(placement.duration, |speed| speed.source_duration)
                        .to_units(24).ok_or("missing exact source length")?;
                    let (stamp, indexed) = managed_pictures.entry((path, package, frames)).or_insert_with(|| {
                        let boundaries = u64::try_from(frames).map_err(|_| "unknown source picture duration")? + 1;
                        managed_picture_budget = managed_picture_budget
                            .checked_sub(boundaries.max(MIN_MANAGED_PICTURE_CHARGE))
                            .ok_or("project exceeds the managed Color Adapter picture verification budget")?;
                        let linked = Path::new(path);
                        let stamp = original_stamp(linked)?;
                        let path = linked.canonicalize().map_err(|e| e.to_string())?;
                        let package = package.strip_prefix("urn:smpte:umid:")
                            .ok_or("missing source package")?.replace('.', "");
                        let indexed = crate::rewrap::verify_lt_range_picture(&path, ffprobe, frames, &package)?;
                        recheck_originals(std::slice::from_ref(&stamp))?;
                        Ok::<_, String>((stamp, indexed))
                    }).as_ref().map_err(Clone::clone)?;
                    if (nested || stack.is_some()) && !indexed.contains_frames(start, length) {
                        return Err(if nested {
                            "the nested Color Adapter source trim exceeds the indexed picture"
                        } else {
                            "the ordered Color Adapter source trim exceeds the indexed picture"
                        }.into());
                    }
                    if let Some((offset, bytes)) = saved_bytes {
                        indexed.verify_span(start, if nested { length - 1 } else { length }, offset, bytes)?;
                    }
                    stamp
                } else {
                    if !pictures.contains_key(&picture_key) && pictures.len() == MAX_DISTINCT_LUT_INPUTS {
                        return Err(
                            "project exceeds the supported number of distinct LUT picture files".into(),
                        );
                    }
                    pictures
                    .entry(picture_key)
                    .or_insert_with(|| {
                        let linked = Path::new(path);
                        let stamp = original_stamp(linked)?;
                        let path = linked.canonicalize().map_err(|e| e.to_string())?;
                        if nested {
                            crate::rewrap::verify_dnxhd_range_picture(&path, ffprobe, frames, package)?;
                        } else if plain_range && picture.codec.as_deref() == Some("avc1") {
                            avid_adapter::original::verify_avc_range_scale_picture(&path, ffprobe, rate, frames)?;
                        } else if range.is_some() {
                            avid_adapter::original::verify_range_scale_picture(&path, ffprobe, frames)?;
                        } else {
                            avid_adapter::original::verify_source_lut_picture(&path, ffprobe, package, frames)?;
                        }
                        recheck_originals(std::slice::from_ref(&stamp))?;
                        Ok::<_, String>(stamp)
                    })
                    .as_ref()
                    .map_err(Clone::clone)?
                };
                if verified_pictures.insert(stamp.path.clone()) {
                    result.pictures.push(stamp.clone());
                }
                let subject = crate::premiere_preparation::timeline_subject(
                    source,
                    StreamKind::Video,
                    placement.video_track.unwrap_or(1),
                    Some(placement.record_start),
                );
                if let Some((filename, (edge, points), inputs)) = external {
                    for input in inputs {
                        if pinned.insert(input.path.clone(), input.revision.clone()).is_none() {
                            result.inputs.push(input.clone());
                        }
                    }
                    prepared.input.source_luts.push(PreparedSourceLut {
                        sequence: sequence_index, event, filename: filename.clone(), edge: *edge, points: points.clone(),
                    });
                    full_grid_edits += usize::from(*edge == PreparedSourceLut::FULL_EDGE);
                    ordered_edits += usize::from(stack.is_some());
                    let operation = if *edge == PreparedSourceLut::FULL_EDGE { "Lumetri Look" } else { "Color Mode LUT" };
                    prepared.diagnostics.push(format!(
                        "{subject}: embedded {filename:?} as an editable clip {operation}."
                    ));
                } else {
                    let (effect, direction) = match range.ok_or("missing saved range transform")? {
                        SourceColorRange::VideoToFull => (PictureEffect::VideoToFullRange, "video-to-full-range"),
                        SourceColorRange::FullToVideo => (PictureEffect::FullToVideoRange, "full-range-to-video"),
                    };
                    prepared.input.picture_effects.push(PreparedPictureEffect {
                        sequence: sequence_index,
                        event,
                        effect,
                        reflection_before_motion: [false; 2],
                    });
                    prepared.diagnostics.push(format!(
                        "{subject}: applied Avid {direction} scaling as editable ASC CDL."
                    ));
                }
                result.recovered.insert(owner.into());
                Ok(())
            })();
            if let Err(reason) = attempt {
                let placement = placements.get(owner);
                let subject = crate::premiere_preparation::timeline_subject(
                    source,
                    StreamKind::Video,
                    placement.and_then(|p| p.video_track).unwrap_or(1),
                    placement.map(|p| p.record_start),
                );
                let control = match range {
                    Some(SourceColorRange::VideoToFull) => "range expansion",
                    Some(SourceColorRange::FullToVideo) => "full-range-to-video scaling",
                    None => "source LUT",
                };
                prepared.diagnostics.push(format!("{subject}: {control} unavailable: {reason}."));
            }
        }
    }
    for (_, picture) in managed_pictures.into_values().flatten() {
        result.managed_pictures.insert(picture.path.clone(), picture);
    }
    prepared.losses.retain(|loss| result.keeps_loss(loss));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_cube_keeps_the_saved_order_and_only_clamps_cube_inputs() {
        let mut corners = [[0.0; 3]; 8];
        corners[7] = [1.0, 0.0, 0.0];
        assert_eq!(tetrahedral(&corners, 2, [0.8, 0.4, 0.2]), [0.2, 0.0, 0.0]);
        let (_, before) = ordered_cube(&corners, 2, false).unwrap();
        let (_, after) = ordered_cube(&corners, 2, true).unwrap();
        assert_eq!(before[0], (0.0, 0.0, 0.0));
        assert_eq!(after[0], (-64.0 / 876.0, -64.0 / 876.0, -64.0 / 876.0));
        assert_eq!(before.last().unwrap().0, 1.0);
        assert_eq!(after.last().unwrap().0, 959.0 / 876.0);
    }

    #[test]
    fn full_to_video_requires_the_saved_direction_and_a_plain_linked_adapter() {
        let xml = concat!(
            "<ColorTransformationList><ColorTransformation><LinearLut>",
            "<Name>Levels scaling (full range to video levels)</Name><BitDepth>10</BitDepth>",
            "<Black>64</Black><White>940</White><Inverted/></LinearLut></ColorTransformation>",
            "</ColorTransformationList>",
        );
        let row = |text: &str, neutral, nested| {
            let mut facts = vec![
                ("text".into(), Value::Str(text.into())),
                ("neutral_wrapper".into(), Value::Bool(neutral)),
                ("picture_file".into(), Value::Str("file".into())),
                ("placement".into(), Value::Str("edit".into())),
            ];
            if nested {
                facts.push(("subclip_motion".into(), Value::Obj(vec![
                    ("sampled_picture_file".into(), Value::Str("file".into())),
                ])));
            }
            Value::Obj(vec![
                ("effect_id".into(), Value::Str("EFF2_LUTSFX".into())),
                ("color_adapter".into(), Value::Obj(facts)),
            ])
        };
        assert!(adapter(&row(xml, true, false), ColorMedia::Linked).is_some());
        assert!(adapter(&row(xml, true, false), ColorMedia::Rewrap).is_none());
        assert!(adapter(&row(xml, true, true), ColorMedia::Linked).is_none());
        assert!(adapter(&row(xml, false, false), ColorMedia::Linked).is_none());
        assert!(adapter(&row(&xml.replace("<Inverted/>", ""), true, false), ColorMedia::Linked).is_none());
    }

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
    fn full_grid_reader_accepts_a_default_unit_domain_and_keeps_interior_samples() {
        let mut rows = vec!["LUT_3D_SIZE 3".into()];
        for b in 0..3 {
            for g in 0..3 {
                for r in 0..3 {
                    rows.push(format!("{} {} {}", (r as f64 / 2.0).powi(2), g as f64 / 2.0, b as f64 / 2.0));
                }
            }
        }
        let body = rows.join("\n");
        let (edge, points) = cube_samples(&body, true).unwrap();
        assert_eq!((edge, points[13]), (3, [0.25, 0.5, 0.5]));
        assert!(affine_corners(&points, edge).is_none());
        assert!(cube_samples(&body, false).is_none());
        assert!(cube_samples(&format!("DOMAIN_MIN 0 0 0\n{body}"), true).is_none());
        assert!(cube_samples(&format!("DOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 0.5\n{body}"), true).is_none());
        assert!(cube_samples(&rows[..rows.len() - 1].join("\n"), true).is_none());
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
