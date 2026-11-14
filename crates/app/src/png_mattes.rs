//! Destination preparation for the original PNG Matte Key profile (CONV-0143).
//! Work from source receipts and leave the source graph available for export.
use avid_adapter::{
    original::{verify_imported_png, PngRole, RootMapping},
    ReadResult,
};
use avid_reader::source::SourceInput;
use graph::{Loss, Rational, StreamKind, Value};
use prproj::convert::{
    model::TICKS_PER_SECOND,
    prepared::{PngAlpha, PreparedPngEdit, PreparedPngMattes, PreparedPngStill},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

const MATTE: &str = "EFF2_2DMATTE";
const ADAPTER: &str = "EFF2_LUTSFX";
const FRAME: i64 = TICKS_PER_SECOND / 24;

#[derive(Default)]
pub(crate) struct Plan {
    pub picture: PreparedPngMattes,
    pub inputs: Vec<SourceInput>,
    replaced: HashMap<String, HashMap<&'static str, HashSet<String>>>,
    recovered_reader_notes: HashSet<String>,
}

impl Plan {
    pub fn apply(
        &mut self,
        input: &mut prproj::convert::prepared::PreparedProjectInput,
        graph: &graph::source::SourceGraph,
    ) -> Result<(), String> {
        let sequences: HashSet<_> = self.picture.edits.iter().map(|e| e.sequence).collect();
        for index in sequences {
            let seq = &mut input.sequences[index];
            let source = &graph.sequences[index];
            seq.start = source
                .start
                .and_then(|s| s.to_units(TICKS_PER_SECOND))
                .ok_or("PNG Matte Key sequence start exceeds Premiere timing bounds")?;
            // The normal adapter already reports the ownership loss. Keep the
            // original comment text when the edit marker moves to the sequence.
            if seq.markers.len() != source.markers.len() {
                return Err("one or more PNG Matte Key markers have unresolved timing".into());
            }
            for (marker, saved) in seq.markers.iter_mut().zip(&source.markers) {
                if marker.owner == "sequence" && saved.owner == graph::MarkerOwner::Edit {
                    marker.track = None;
                }
            }
        }
        input.png_mattes = std::mem::take(&mut self.picture);
        Ok(())
    }
    pub fn keeps_loss(&self, loss: &Loss) -> bool {
        // A sequence can have several losses of the same kind. Compare the
        // complete message and owner for each recovered occurrence.
        !self
            .replaced
            .get(&loss.object)
            .and_then(|by_aspect| by_aspect.get(loss.aspect.as_str()))
            .is_some_and(|messages| messages.contains(&loss.reason))
    }

    fn replace_loss(&mut self, owner: &str, aspect: &'static str, message: String) {
        self.replaced
            .entry(owner.into())
            .or_default()
            .entry(aspect)
            .or_default()
            .insert(message);
    }

    pub fn keeps_reader_note(&self, note: &str) -> bool {
        !self.recovered_reader_notes.contains(note.trim())
    }
}

fn text<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    match row.get(key)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}
fn number(row: &Value, key: &str) -> Option<i64> {
    match row.get(key)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}
fn flag(row: &Value, key: &str) -> Option<bool> {
    match row.get(key)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}
fn list<'a>(row: &'a Value, key: &str) -> &'a [Value] {
    match row.get(key) {
        Some(Value::Arr(a)) => a,
        _ => &[],
    }
}
fn empty_list(row: &Value, key: &str) -> bool {
    matches!(row.get(key), Some(Value::Arr(a)) if a.is_empty())
}
fn null(row: &Value, key: &str) -> bool {
    row.get(key) == Some(&Value::Null)
}
fn locators(row: &Value) -> Option<Vec<String>> {
    let paths = list(row, "saved_locators");
    if !(1..=3).contains(&paths.len()) {
        return None;
    }
    paths
        .iter()
        .map(|p| match p {
            Value::Str(s) if !s.is_empty() && s.len() <= 4096 => Some(s.clone()),
            _ => None,
        })
        .collect()
}

struct Still<'a> {
    bin: usize,
    id: &'a str,
    master: &'a Value,
    import: &'a Value,
}

impl<'a> Still<'a> {
    fn name(&self) -> Option<&str> {
        text(self.master, "name")
    }
    fn unresolved_reason(&self) -> Option<&'a str> {
        let [track] = list(self.master, "tracks") else {
            return None;
        };
        let package = text(track, "file_package")?;
        let reason = text(self.master, "reason")?;
        (reason == "unsupported picture compression \"RI00\""
            || reason
                .strip_prefix("media for file mob ")
                .and_then(|r| r.strip_suffix(" not found"))
                == Some(package))
        .then_some(reason)
    }

    fn qualified(&self) -> Option<()> {
        let [track] = list(self.master, "tracks") else {
            return None;
        };
        let descriptor = track.get("descriptor")?;
        let length = number(self.import, "master_length")?;
        let package = text(self.import, "file_mob")?.strip_prefix("urn:smpte:umid:")?;
        (length > 0
            && number(self.import, "source_length") == Some(length)
            && number(self.import, "track") == Some(1)
            && number(self.import, "file_track") == Some(1)
            && number(self.import, "file_in") == Some(0)
            && text(self.import, "rate") == Some("24")
            && number(self.import, "import_aspect") == Some(3)
            && number(self.import, "color_level") == Some(1)
            && flag(self.import, "ignore_alpha") == Some(true)
            && flag(self.import, "invert_alpha") == Some(false)
            && self.unresolved_reason().is_some()
            && text(self.master, "qualified_rate") == Some("24")
            && number(self.master, "length") == Some(length)
            && empty_list(self.master, "marker_native")
            && empty_list(self.master, "raw_groups")
            && empty_list(self.master, "stereo_pairs")
            && self.master.get("input_color_transforms").is_none()
            && number(track, "kind") == Some(1)
            && number(track, "index") == Some(1)
            && text(track, "file_package")
                .is_some_and(|p| p.eq_ignore_ascii_case(&package.replace('.', "")))
            && track.get("offset").and_then(|o| number(o, "value")) == Some(0)
            && number(track, "clip_length") == Some(length)
            && text(descriptor, "compression") == Some("RI00")
            && number(descriptor, "resolution_id") == Some(0)
            && number(descriptor, "width") == Some(1920)
            && number(descriptor, "height") == Some(1080)
            && list(descriptor, "display_aspect") == [Value::Int(16), Value::Int(9)])
        .then_some(())
    }
}

struct Builder<'a> {
    plan: Plan,
    mappings: &'a [RootMapping],
    stills: HashMap<&'a str, Still<'a>>,
    imported: HashMap<(usize, &'a str), usize>,
    inputs: HashMap<Vec<String>, (SourceInput, PngRole)>,
    physical: HashMap<PathBuf, Vec<String>>,
}

impl<'a> Builder<'a> {
    fn add(
        &mut self,
        bin: usize,
        mob: &'a str,
        name: &str,
        import: &Value,
        alpha: PngAlpha,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<usize, String> {
        let saved = locators(import).ok_or("invalid saved PNG locator")?;
        let role = if alpha == PngAlpha::Ignore {
            PngRole::Opaque
        } else {
            PngRole::Matte
        };
        let needs_decode = self
            .inputs
            .get(&saved)
            .is_none_or(|(_, old)| *old == PngRole::Opaque && role == PngRole::Matte);
        if needs_decode {
            check()?;
            let source = verify_imported_png(&saved, self.mappings, role)?;
            check()?;
            if self
                .physical
                .get(&source.path)
                .is_some_and(|other| *other != saved)
                || self
                    .inputs
                    .get(&saved)
                    .is_some_and(|(before, _)| *before != source)
            {
                return Err(
                    "conflicting PNG original identities or source changed during verification"
                        .into(),
                );
            }
            self.physical.insert(source.path.clone(), saved.clone());
            self.inputs.insert(saved.clone(), (source, role));
        }
        let path = &self.inputs[&saved].0.path;
        if let Some(&index) = self.imported.get(&(bin, mob)) {
            let old = &self.plan.picture.stills[index];
            if old.path != *path || old.alpha != alpha {
                return Err("one saved PNG master has contradictory interpretations".into());
            }
            return Ok(index);
        }
        let index = self.plan.picture.stills.len();
        if index == 10_000 {
            return Err("too many linked PNG stills".into());
        }
        self.imported.insert((bin, mob), index);
        self.plan.picture.stills.push(PreparedPngStill {
            bin,
            path: path.clone(),
            name: name.into(),
            alpha,
        });
        Ok(index)
    }
}

pub(crate) fn prepare(
    read: &ReadResult,
    mappings: &[RootMapping],
    separate: bool,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Plan, String> {
    if !separate {
        return Ok(Plan::default());
    }
    let graph = &read.graph;
    let receipts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == "avid.source")
        .map(|s| (s.object.as_str(), &s.data))
        .collect();
    let candidates: Vec<_> = graph
        .sequences
        .iter()
        .enumerate()
        .filter_map(|(index, seq)| {
            let receipt = *receipts.get(seq.id.as_str())?;
            list(receipt, "unresolved")
                .iter()
                .any(|r| text(r, "picture_profile") == Some("imported_png_matte_controls"))
                .then_some((index, seq, receipt))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(Plan::default());
    }
    let mut builder = Builder {
        plan: Plan::default(),
        mappings,
        stills: HashMap::new(),
        imported: HashMap::new(),
        inputs: HashMap::new(),
        physical: HashMap::new(),
    };
    for (bin, record) in graph.bins.iter().enumerate() {
        let Some(&receipt) = receipts.get(record.id.as_str()) else {
            continue;
        };
        let masters: HashMap<_, _> = list(receipt, "unresolved_masters")
            .iter()
            .filter_map(|m| Some((text(m, "occurrence")?, m)))
            .collect();
        for import in list(receipt, "imported_stills") {
            let (Some(id), Some(mob)) = (text(import, "id"), text(import, "mob")) else {
                continue;
            };
            let Some(&master) = masters.get(id) else {
                continue;
            };
            if text(master, "mob_id") != Some(mob) {
                return Err("PNG master and import disagree about their saved identity".into());
            }
            if builder
                .stills
                .insert(
                    mob,
                    Still {
                        bin,
                        id,
                        master,
                        import,
                    },
                )
                .is_some()
            {
                return Err("same PNG master identity occurs in more than one selected bin".into());
            }
        }
    }
    for (index, seq, receipt) in candidates {
        prepare_sequence(&mut builder, read, index, seq, receipt, check)
            .map_err(|e| format!("PNG Matte Key · {}: {e}", seq.name))?;
    }
    builder.plan.inputs = builder
        .inputs
        .into_values()
        .map(|(source, _)| source)
        .collect();
    builder.plan.inputs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(builder.plan)
}

fn prepare_sequence<'a>(
    builder: &mut Builder<'a>,
    read: &'a ReadResult,
    sequence: usize,
    seq: &'a graph::source::SourceSequence,
    receipt: &'a Value,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let rate = Rational::integer(24);
    let valid_clock = seq.clock.as_ref().is_some_and(|clock| {
        clock.rate == rate
            && clock.display_base == 24
            && !clock.drop
            && clock.provenance.route == "direct sequence TC1"
            && clock.provenance.source_rate == rate
            && clock.provenance.source_offset == 0
            && clock.provenance.origin_count == clock.effective_count
            && clock
                .exact_start()
                .is_some_and(|start| Some(start) == seq.start)
            && seq.start.and_then(|start| start.to_units(24)).is_some()
    });
    if seq.rate != rate
        || !valid_clock
        || seq.frame_size.is_some_and(|size| size != (1920, 1080))
        || list(receipt, "compatibility_frame_size") != [Value::Int(1920), Value::Int(1080)]
        || !seq.placements.is_empty()
        || !seq.transitions.is_empty()
        || !seq.texts.is_empty()
        || !empty_list(receipt, "native_transitions")
        || !empty_list(receipt, "audio_track_formats")
        || !empty_list(receipt, "combiner_channels")
        || receipt.get("native_group_uses").is_some()
        || seq
            .tracks
            .iter()
            .any(|t| t.kind != StreamKind::Video || t.index > 2 || t.level_db.is_some())
    {
        return Err("requires a 1080p24 still-only sequence with a saved non-drop origin".into());
    }
    let bin = *read
        .sequence_bins
        .get(&seq.id)
        .ok_or("missing source bin assignment")?;
    let native = list(receipt, "native_events");
    if native.len() > 20_000 || list(receipt, "unresolved").len() > 20_000 {
        return Err("too many native PNG picture edits".into());
    }
    let mut by_owner = HashMap::new();
    for row in native {
        // The direct adapter owns a single input Sequence and its sole
        // picture SourceClip. Keep this exact ancestry, not a slot-only join.
        let owner = text(row, "owner").ok_or("missing native edit owner")?;
        let (adapter_owner, _) = owner
            .rsplit_once("/component/0/component/")
            .ok_or("native PNG input has unexpected component ancestry")?;
        if by_owner.insert(adapter_owner, row).is_some() {
            return Err("ambiguous native PNG edit owner".into());
        }
    }
    let mut frames = BTreeMap::<i64, [Option<usize>; 2]>::new();
    let mut consumed = HashSet::new();
    let mut effects = HashSet::new();
    for issue in list(receipt, "unresolved") {
        let lane = number(issue, "track")
            .filter(|n| (1..=2).contains(n))
            .ok_or("unsupported native track")?;
        let frame = issue
            .get("record_start")
            .and_then(|r| number(r, "value"))
            .filter(|n| {
                *n >= 0
                    && n.checked_add(1)
                        .and_then(|n| n.checked_mul(FRAME))
                        .is_some()
            })
            .ok_or("unresolved Matte Key record position")?;
        if number(issue, "kind") != Some(1)
            || number(issue, "record_length") != Some(1)
            || text(issue, "record_rate") != Some("24")
        {
            return Err(format!(
                "V{lane} frame {frame} is outside the verified one-frame profile"
            ));
        }
        let owner = text(issue, "owner").ok_or("missing effect occurrence")?;
        if !effects.insert(owner) {
            return Err("duplicate PNG effect occurrence".into());
        }
        let profile = (text(issue, "effect_id"), text(issue, "picture_profile"));
        let (index, aspect) = match profile {
            (Some(MATTE), Some("imported_png_matte_controls")) if lane == 2 => {
                let (master, import, invert) =
                    matte_sources(issue).ok_or("unresolved saved fill or alpha")?;
                let name = match invert {
                    true => "Matte Key source — Avid Invert",
                    false => "Matte Key source — Avid Normal",
                };
                let index = builder.add(
                    bin,
                    master,
                    name,
                    import,
                    PngAlpha::Straight { invert: !invert },
                    check,
                )?;
                builder.plan.recovered_reader_notes.insert(format!(
                    "{:?}: V{lane} at frame {frame}: Matte Key (EFF2_2DMATTE) not converted",
                    seq.name
                ));
                (index, "picture_effect")
            }
            (Some(ADAPTER), Some("empty_color_adapter_controls")) => {
                // The neutral adapter was traversed through its one input. Join
                // that native query to this effect by owner ancestry and slot.
                let event = by_owner
                    .get(owner)
                    .ok_or("empty Color Adapter has no saved picture input")?;
                let (id, mob) = plain_source(event, lane, frame)
                    .ok_or("unverified source range for PNG edit")?;
                if !consumed.insert(id) {
                    return Err("one PNG edit matched several Color Adapter occurrences".into());
                }
                let still = builder
                    .stills
                    .get(mob)
                    .ok_or("PNG original could not be joined to its saved master")?;
                still.qualified().ok_or_else(|| {
                    format!(
                        "unverified opaque PNG import for {:?}: {}",
                        still.name().unwrap_or("unknown still"),
                        text(still.master, "reason").unwrap_or("no native media diagnosis"),
                    )
                })?;
                let name = still
                    .name()
                    .ok_or("PNG master has no saved name")?
                    .to_owned();
                let (still_bin, occurrence, import) = (still.bin, still.id, still.import);
                let reason = still
                    .unresolved_reason()
                    .ok_or("missing recovered PNG master diagnosis")?;
                let index = builder.add(still_bin, mob, &name, import, PngAlpha::Ignore, check)?;
                let bin_id = &read.graph.bins[still_bin].id;
                builder.plan.replace_loss(
                    bin_id,
                    "source_master",
                    format!("{occurrence}: {reason}; native master facts retained on bin"),
                );
                builder
                    .plan
                    .recovered_reader_notes
                    .insert(format!("{name:?}: skipped ({reason})"));
                builder.plan.replace_loss(
                    &seq.id,
                    "source_route",
                    format!("{id}: master clip was not converted"),
                );
                (index, "source_color_adapter")
            }
            _ => {
                return Err(format!(
                    "V{lane} frame {frame} has an unverified picture effect or Color Adapter"
                ))
            }
        };
        let reason = text(issue, "reason").ok_or("missing effect receipt reason")?;
        builder.plan.replace_loss(
            &seq.id,
            aspect,
            format!("V{lane} at record frame {frame} for 1 frame (24 fps): {reason}"),
        );
        if frames.entry(frame).or_default()[lane as usize - 1]
            .replace(index)
            .is_some()
        {
            return Err(format!(
                "overlapping native PNG edits on V{lane} at frame {frame}"
            ));
        }
        if builder.plan.picture.edits.len() + frames.len() > 10_000 {
            return Err("too many PNG picture edits".into());
        }
    }
    if consumed.len() != native.len() {
        return Err("some native picture inputs have no verified edit placement".into());
    }
    for (frame, [background, foreground]) in frames {
        let background =
            background.ok_or_else(|| format!("V2 frame {frame} has no exact V1 background"))?;
        builder.plan.picture.edits.push(PreparedPngEdit {
            sequence,
            start: frame * FRAME,
            background,
            foreground,
        });
    }
    if read
        .graph
        .losses
        .iter()
        .any(|l| l.object == seq.id && builder.plan.keeps_loss(l))
    {
        return Err("additional unresolved sequence content prevents reliable compositing".into());
    }
    Ok(())
}

fn matte_sources(issue: &Value) -> Option<(&str, &Value, bool)> {
    let inputs = issue.get("matte_inputs")?;
    if text(inputs, "background") != Some("lower_track") {
        return None;
    }
    let fill = inputs.get("fill")?;
    let matte = inputs.get("matte")?;
    let master = text(fill, "mob")?;
    if text(matte, "mob") != Some(master)
        || number(fill, "track") != Some(1)
        || number(matte, "track") != Some(2)
        || [fill, matte].iter().any(|input| {
            number(input, "start") != Some(0)
                || number(input, "length") != Some(1)
                || text(input, "rate") != Some("24")
        })
    {
        return None;
    }
    let import = issue.get("matte_import")?;
    (flag(import, "ignore_alpha") == Some(false)).then_some((
        master,
        import,
        flag(import, "invert_alpha")?,
    ))
}

fn plain_source(event: &Value, lane: i64, frame: i64) -> Option<(&str, &str)> {
    let range = event.get("range")?;
    let master = text(event, "master")?;
    let [source] = list(range, "route") else {
        return None;
    };
    let controls = event.get("controls")?;
    (number(event, "kind") == Some(1)
        && number(event, "track") == Some(lane)
        && number(event, "master_track") == Some(1)
        && null(event, "placement_id")
        && null(event, "combined")
        && null(event, "reason")
        && ["color", "picture", "animatte", "speed"]
            .iter()
            .all(|key| null(controls, key))
        && list(event, "via") == [Value::Str("effect EFF2_LUTSFX not converted".into())]
        && number(range, "record_start") == Some(frame)
        && number(range, "record_length") == Some(1)
        && number(range, "source_in") == Some(0)
        && number(range, "source_length") == Some(1)
        && text(range, "record_rate") == Some("24")
        && text(range, "source_rate") == Some("24")
        && text(source, "mob") == Some(master)
        && number(source, "track") == Some(1)
        && text(source, "rate") == Some("24")
        && source.get("source_offset").and_then(|v| number(v, "value")) == Some(0)
        && source.get("length").and_then(|v| number(v, "value")) == Some(1))
    .then_some((text(event, "id")?, master))
}
