//! Internal sequence-only Avid Processing trial. Eligibility follows the AVB
//! source facts also used for graph/2. Publication keeps the native object graph.
mod correction;

use avid_reader::{
    bin_nav::{BinMob, BinNavigator},
    io::MobId,
    objects::{Node, Value},
    sequence_nav::{positions, spans_from_file},
    source::{
        extract_source_files, Observed, Rate, SourceColorRange, SourceInput, SourceIssueKind,
        SourceLimits,
    },
    AvbDocument, AvbFile, AvbIndex,
};
use prproj::{RandomUuids, UuidSource};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

#[derive(Debug, Serialize)]
pub struct Report {
    pub source_sha256: String,
    pub qualification: &'static str,
    pub sequences: Vec<Sequence>,
    pub qualified_replacements: usize,
    pub approximations: usize,
    pub published: bool,
}

#[derive(Debug, Serialize)]
pub struct Sequence {
    name: String,
    mob_id: String,
    processed_id: Option<String>,
    adapters: Vec<Adapter>,
}

#[derive(Debug, Serialize)]
struct Adapter {
    track: i64,
    record_start: Option<i64>,
    record_length: Option<i64>,
    adapter: String,
    status: &'static str,
    reason: String,
}

#[derive(Clone)]
struct Replacement {
    index: usize,
    wrapper: u32,
    transform: correction::Transform,
}

struct TrackPlan<'a> {
    mob: BinMob<'a>,
    component: u32,
    picture_slot: usize,
    replacements: Vec<Replacement>,
}

pub fn run(
    source: &Path,
    output: &Path,
    selected: &[String],
    review: bool,
    rec709_sdr: bool,
    accept_approximations: bool,
) -> Result<Report, String> {
    if !output
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("avb"))
    {
        return Err("Choose a new .avb destination.".into());
    }
    crate::output::preflight(output, None)?;
    if !review && selected.is_empty() {
        return Err("Select at least one sequence by its saved MobID.".into());
    }
    let index = AvbIndex::open(source).map_err(display)?;
    let source_sha256 = format!("{:x}", Sha256::digest(index.original_bytes()));
    let bin = AvbFile::parse(index.original_bytes()).map_err(display)?;
    let navigator = BinNavigator::new(&bin).map_err(|err| format!("Invalid bin: {err:?}"))?;
    let top_level = navigator
        .toplevel()
        .map_err(|err| format!("Invalid bin: {err:?}"))?;
    let mut files = HashMap::new();
    for entry in navigator
        .mobs()
        .map_err(|err| format!("Invalid bin: {err:?}"))?
    {
        if entry.mob.get("mob_type_id").and_then(Value::as_int) == Some(3) {
            if let Some(id) = mob_id(entry.mob) {
                files
                    .entry(id)
                    .and_modify(|value| *value = None)
                    .or_insert(Some(entry.mob));
            }
        }
    }
    let source_facts = extract_source_files(
        &[SourceInput {
            path: source.to_owned(),
            revision: source_sha256.clone(),
        }],
        &[&bin],
        SourceLimits::default(),
    )
    .map_err(display)?;

    let mut report = Report {
        source_sha256,
        qualification: "internal_trial",
        sequences: Vec::new(),
        qualified_replacements: 0,
        approximations: 0,
        published: false,
    };
    let mut plans = Vec::new();
    let mut found = HashSet::new();
    for &mob in &top_level {
        let id = mob_id(mob.mob).ok_or("Sequence has no saved MobID")?;
        let urn = id.urn();
        if !selected.is_empty() && !selected.contains(&urn) {
            continue;
        }
        if !found.insert(urn.clone()) {
            return Err(format!("Sequence MobID occurs more than once: {urn}"));
        }
        let mut sequence = Sequence {
            name: mob
                .mob
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            mob_id: urn,
            processed_id: None,
            adapters: Vec::new(),
        };
        let facts = source_facts
            .sequences
            .iter()
            .find(|facts| facts.info.mob_id == Some(id))
            .ok_or("Selected sequence has no unambiguous saved source facts")?;
        let picture = picture_track(&bin, mob.mob);
        let mut by_position = BTreeMap::new();
        if let Some((_, component)) = picture {
            for row in
                positions(&spans_from_file(&bin, component).map_err(display)?).map_err(display)?
            {
                if row.component.class == *b"TKFX" {
                    by_position
                        .entry(row.position)
                        .or_insert_with(Vec::new)
                        .push(row);
                }
            }
        }
        let mut replacements = BTreeMap::new();
        for issue in &facts.unresolved {
            let SourceIssueKind::ColorAdapter { length } = issue.issue_kind else {
                continue;
            };
            let start = match &issue.record_start {
                Observed::Known(value) => Some(*value),
                _ => None,
            };
            let adapter = issue.color_adapter.as_deref();
            let transform =
                adapter.and_then(|value| correction::Transform::from_saved_xml(&value.transform));
            let mut result = Adapter {
                track: issue.track,
                record_start: start,
                record_length: length,
                adapter: transform
                    .map_or("Unqualified Color Adapter", correction::Transform::name)
                    .into(),
                status: "adapter_kept",
                reason: String::new(),
            };
            let replacement = (|| {
                let transform = transform.ok_or(
                    "This saved transform or adapter stack has no measured editable mapping.",
                )?;
                if !rec709_sdr {
                    return Err("Select Rec.709 SDR only when it is the source Avid project's working space.");
                }
                let adapter = adapter.ok_or("The saved adapter payload is unavailable.")?;
                let rate = issue
                    .record_rate
                    .filter(|rate| Some(*rate) == facts.rate)
                    .ok_or(
                        "The edit and sequence do not establish the same saved picture clock.",
                    )?;
                if issue.track != 1
                    || adapter.master_track != 1
                    || !adapter.plain_same_rate_linked_input
                    || adapter.subclip_master.is_some()
                    || adapter.subclip_motion.is_some()
                    || adapter.managed_picture_span.is_some()
                {
                    return Err(
                        "Only the plain same-rate picture adapter trial profile is admitted.",
                    );
                }
                let Observed::Known(file_id) = adapter.picture_file else {
                    return Err(
                        "The edit and file package do not establish the same saved transform.",
                    );
                };
                if !files
                    .get(&file_id)
                    .copied()
                    .flatten()
                    .is_some_and(|file| picture_profile(&bin, file, rate))
                {
                    return Err("The file package does not match the saved clock and progressive 10-bit SDR video-level profile for this adapter.");
                }
                if facts
                    .transitions
                    .iter()
                    .any(|transition| transition.kind == 1)
                {
                    return Err("A video transition may use the adapter's handles.");
                }
                let Some([row]) = start.and_then(|at| by_position.get(&at)).map(Vec::as_slice)
                else {
                    return Err(
                        "The saved adapter does not identify one direct picture occurrence.",
                    );
                };
                let wrapper = bin
                    .node(row.component.object_id)
                    .ok_or("The saved effect is missing.")?;
                if wrapper.get("effect_id").and_then(Value::as_str) != Some(correction::ADAPTER)
                    || Some(row.component.length) != length
                    || !wrapper_matches(
                        &bin,
                        wrapper,
                        adapter.master,
                        adapter.source_in,
                        row.component.length,
                    )
                {
                    return Err(
                        "The saved effect and its input do not match the source occurrence.",
                    );
                }
                let original = index
                    .chunk(row.component.object_id)
                    .ok_or("The saved effect chunk is missing.")?;
                let encoded = avid_reader::payload_writer::encode_supported_chunk(
                    wrapper,
                    Some(&original[8..]),
                    index.big_endian,
                    Some((bin.object_count() - 1) as u32),
                    original.len(),
                )
                .map_err(|_| "The saved effect contains unqualified writer fields.")?;
                if encoded != original {
                    return Err("The saved effect has unpreserved native data.");
                }
                Ok(Replacement {
                    index: row.index,
                    wrapper: row.component.object_id,
                    transform,
                })
            })();
            match replacement {
                Ok(item) => {
                    result.reason = item.transform.reason().into();
                    if item.transform.is_approximation() {
                        report.approximations += 1;
                        if accept_approximations {
                            result.status = "trial_approximation";
                        } else {
                            result
                                .reason
                                .push_str(" Pass --accept-approximations to use this mapping.");
                        }
                    } else {
                        report.qualified_replacements += 1;
                        result.status = "qualified_replacement";
                    }
                    if replacements.insert(item.index, item).is_some() {
                        return Err(
                            "Multiple source facts identify the same saved adapter occurrence."
                                .into(),
                        );
                    }
                }
                Err(reason) => result.reason = reason.into(),
            }
            sequence.adapters.push(result);
        }
        if !accept_approximations {
            replacements.retain(|_, item| !item.transform.is_approximation());
        }
        if !replacements.is_empty() {
            let (picture_slot, component) = picture.ok_or("Missing planned picture track")?;
            plans.push((
                report.sequences.len(),
                TrackPlan {
                    mob,
                    component,
                    picture_slot,
                    replacements: replacements.into_values().collect(),
                },
            ));
        }
        report.sequences.push(sequence);
    }
    if selected.iter().any(|id| !found.contains(id)) {
        return Err("A selected MobID does not identify a top-level sequence in this bin.".into());
    }
    if review {
        return Ok(report);
    }
    if plans.is_empty() {
        if report.approximations > 0 {
            return Err(
                "Review the editable approximations and pass --accept-approximations for this run."
                    .into(),
            );
        }
        return Err(
            "No eligible Color Adapters in the selected sequences. Run Review for details.".into(),
        );
    }
    let mut document = AvbDocument::from_bytes(index.original_bytes().to_vec()).map_err(display)?;
    let plans_keep = plans
        .iter()
        .map(|(_, plan)| plan.mob.item_index)
        .collect::<HashSet<_>>();
    let removed = top_level
        .iter()
        .filter(|mob| !plans_keep.contains(&mob.item_index))
        .map(|mob| mob.item_index)
        .collect::<HashSet<_>>();
    let mut rows = list(document.root().ok_or("Missing native bin")?, "items")?.to_vec();
    let root = document.root_id();
    let mut ids = RandomUuids::new();
    let uid = ids.uuid4().bytes();
    let bin_uid = u64::from_be_bytes(uid[..8].try_into().unwrap());
    let bin_name = output
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("Choose a bin name representable as text.")?;
    document
        .set_field(root, "uid", Value::UInt(bin_uid))
        .map_err(display)?;
    // Native MCBR stores the low UID word in its first signed field.
    let origin = avid_reader::document_extra::create_bin_ref(
        &mut document,
        bin_uid as i32,
        (bin_uid >> 32) as i32,
        bin_name,
        bin_name,
    )
    .map_err(display)?;

    for (report_index, plan) in plans {
        let old_id = mob_id(plan.mob.mob).ok_or("Missing source MobID")?;
        if (1..bin.object_count() as u32)
            .filter(|&id| id != plan.mob.object_index)
            .filter_map(|id| bin.node(id))
            .any(|node| references_mob(node, old_id))
        {
            return Err(format!(
                "{} is referenced by another saved object.",
                old_id.urn()
            ));
        }
        let new_id = MobId {
            material: correction::uuid(u128::from_be_bytes(ids.uuid4().bytes())),
            ..old_id
        };
        let row = rows
            .get_mut(plan.mob.item_index)
            .ok_or("Missing planned bin item")?;
        let new_comp = apply(&mut document, &bin, plan, new_id, origin)?;
        let Value::Node(row) = row else {
            return Err("Invalid planned bin item".into());
        };
        let (_, mob) = row
            .fields
            .iter_mut()
            .find(|(key, _)| *key == "mob")
            .ok_or("Missing bin item composition")?;
        *mob = Value::Ref(new_comp);
        report.sequences[report_index].processed_id = Some(new_id.urn());
    }
    let rows = rows
        .into_iter()
        .enumerate()
        .filter_map(|(index, row)| (!removed.contains(&index)).then_some(row))
        .collect();
    document
        .set_field(root, "items", Value::List(rows))
        .map_err(display)?;
    document.finish_bin_copy().map_err(display)?;
    let current = AvbIndex::open(source).map_err(display)?;
    if current.original_bytes() != index.original_bytes() {
        return Err("The source bin changed after preparation. Review it again.".into());
    }
    document.write_new(output).map_err(display)?;
    report.published = true;
    Ok(report)
}

fn wrapper_matches(bin: &AvbFile, wrapper: &Node, master: MobId, start: i64, length: i64) -> bool {
    let Ok([Value::Node(track)]) = list(wrapper, "tracks") else {
        return false;
    };
    let Some(sequence) = referenced(bin, track, "component").filter(|node| node.class == *b"SEQU")
    else {
        return false;
    };
    let Some(Value::RefList(parts)) = sequence.get("components") else {
        return false;
    };
    let [first, middle, last] = parts.as_slice() else {
        return false;
    };
    if ![first, last].into_iter().all(|part| {
        part.as_index()
            .and_then(|id| bin.node(id))
            .is_some_and(|node| {
                node.class == *b"FILL" && node.get("length").and_then(Value::as_int) == Some(0)
            })
    }) {
        return false;
    }
    let Some(source) = middle.as_index().and_then(|id| bin.node(id)) else {
        return false;
    };
    source.class == *b"SCLP"
        && source.get("length").and_then(Value::as_int) == Some(length)
        && source.get("start_time").and_then(Value::as_int) == Some(start)
        && source.get("track_id").and_then(Value::as_int) == Some(1)
        && mob_id(source) == Some(master)
}

fn referenced<'a>(bin: &'a AvbFile, owner: &Node, field: &str) -> Option<&'a Node> {
    bin.node(owner.get(field)?.as_index()?)
}

fn picture_profile(bin: &AvbFile, file: &Node, rate: Rate) -> bool {
    let Some(picture) = referenced(bin, file, "descriptor").filter(|node| node.class == *b"CDCI")
    else {
        return false;
    };
    let int = |field| picture.get(field).and_then(Value::as_int);
    let video_levels = [
        ("component_width", SourceColorRange::BIT_DEPTH),
        ("black_ref_level", SourceColorRange::VIDEO_BLACK),
        ("white_ref_level", SourceColorRange::VIDEO_WHITE),
        ("frame_layout", 0),
    ]
    .into_iter()
    .all(|(field, expected)| int(field) == Some(expected));
    let same_clock = [file, picture]
        .into_iter()
        .all(|node| match node.get("edit_rate") {
            Some(Value::Float(value)) => Rate::from_avid(*value) == Some(rate),
            _ => false,
        });
    if !video_levels || !same_clock {
        return false;
    }
    // An unspecified SMPTE label is covered by the explicit Rec.709 SDR assertion.
    [
        ("coding_equations", 0x0401010102020000060e2b3404010101),
        (
            "transfer_characteristic",
            0x0401010101020000060e2b3404010101,
        ),
        ("color_primaries", 0x0401010103030000060e2b3404010106),
    ]
    .into_iter()
    .all(|(field, expected)| match picture.get(field) {
        None => true,
        Some(Value::Uuid(saved)) => {
            *saved == correction::uuid(0) || *saved == correction::uuid(expected)
        }
        _ => false,
    }) && ["display_width", "display_height"]
        .into_iter()
        .all(|field| int(field).is_some_and(|n| n > 0))
        && matches!(int("alpha_sampled_width"), None | Some(0))
}

fn picture_track(bin: &AvbFile, mob: &Node) -> Option<(usize, u32)> {
    let mut tracks = list(mob, "tracks")
        .ok()?
        .iter()
        .enumerate()
        .filter_map(|(slot, item)| {
            let Value::Node(track) = item else {
                return None;
            };
            let id = track.get("component").and_then(Value::as_index)?;
            let node = bin.node(id)?;
            (node.get("media_kind_id").and_then(Value::as_int) == Some(1))
                .then_some((slot, id, track, node))
        });
    let (slot, id, track, node) = tracks.next()?;
    (tracks.next().is_none()
        && track.get("index").and_then(Value::as_int) == Some(1)
        && node.class == *b"SEQU")
        .then_some((slot, id))
}

fn apply(
    document: &mut AvbDocument,
    bin: &AvbFile,
    plan: TrackPlan<'_>,
    new_id: MobId,
    origin: u32,
) -> Result<u32, String> {
    let sequence = bin.node(plan.component).ok_or("Missing prepared track")?;
    let Some(Value::RefList(components)) = sequence.get("components") else {
        return Err("Missing prepared components.".into());
    };
    let mut components = components.clone();
    for item in plan.replacements {
        let original = bin
            .node(item.wrapper)
            .ok_or("Missing prepared wrapper")?
            .clone();
        components[item.index] =
            Value::Ref(correction::make(document, original, item.transform).map_err(display)?);
    }
    let track_id = document
        .duplicate_saved_object(plan.component, *b"SEQU")
        .map_err(display)?;
    document
        .set_field(track_id, "components", Value::RefList(components))
        .map_err(display)?;
    let mut tracks = list(plan.mob.mob, "tracks")?.to_vec();
    let Value::Node(track) = &mut tracks[plan.picture_slot] else {
        return Err("Invalid picture track".into());
    };
    let (_, reference) = track
        .fields
        .iter_mut()
        .find(|(key, _)| *key == "component")
        .ok_or("Missing picture reference")?;
    *reference = Value::Ref(track_id);
    let attrs_id = plan
        .mob
        .mob
        .get("attributes")
        .and_then(Value::as_index)
        .ok_or("The saved composition has no attributes.")?;
    let attrs = bin.node(attrs_id).and_then(|node| node.get("entries"));
    let Some(Value::Attrs(entries)) = attrs else {
        return Err("The saved composition attributes are unavailable.".into());
    };
    let mut entries = entries.clone();
    entries.retain(|(name, _)| name != "_ORG_BIN");
    entries.push(("_ORG_BIN".into(), Value::Ref(origin)));
    let attrs = document
        .duplicate_saved_object(attrs_id, *b"ATTR")
        .map_err(display)?;
    document
        .set_field(attrs, "entries", Value::Attrs(entries))
        .map_err(display)?;

    // Put the new composition after the changed track and attributes so
    // Avid can resolve its root while loading the newly saved timeline.
    let composition = document
        .duplicate_saved_object(plan.mob.object_index, *b"CMPO")
        .map_err(display)?;
    document
        .set_field(composition, "attributes", Value::Ref(attrs))
        .map_err(display)?;
    document
        .set_field(composition, "tracks", Value::List(tracks))
        .map_err(display)?;
    document
        .set_field(composition, "mob_id", Value::MobId(new_id))
        .map_err(display)?;
    let original = plan
        .mob
        .mob
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Sequence");
    document
        .rename_mob(composition, &format!("{original} - Processed"))
        .map_err(display)?;
    Ok(composition)
}

fn list<'a>(node: &'a Node, field: &str) -> Result<&'a [Value], String> {
    match node.get(field) {
        Some(Value::List(values)) => Ok(values.as_slice()),
        _ => Err(format!("{} has no decoded {field} list.", node.type_name)),
    }
}

fn mob_id(node: &Node) -> Option<MobId> {
    match node.get("mob_id") {
        Some(Value::MobId(value)) => Some(*value),
        _ => None,
    }
}

fn references_mob(node: &Node, id: MobId) -> bool {
    fn contains(value: &Value, id: MobId) -> bool {
        match value {
            Value::MobId(value) => *value == id,
            Value::Node(node) => references_mob(node, id),
            Value::List(values) | Value::RefList(values) => {
                values.iter().any(|value| contains(value, id))
            }
            Value::Attrs(values) => values.iter().any(|(_, value)| contains(value, id)),
            _ => false,
        }
    }
    node.fields.iter().any(|(_, value)| contains(value, id))
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}
