//! Destination preparation for verified still Matte Key profiles.
//! Work from source receipts and leave the source graph available for export.
use avid_adapter::{
    original::{verify_imported_png, PngRole, PngVerificationError, RootMapping, VerifiedPng},
    ReadResult,
};
use avid_reader::source::SourceInput;
use graph::{Loss, MarkerOwner, Rational, StreamKind, Value};
use prproj::convert::{
    model::TICKS_PER_SECOND,
    prepared::{PngAlpha, PreparedPngEdit, PreparedPngMattes, PreparedPngStill},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

mod split;

const MATTE: &str = "EFF2_2DMATTE";
const ADAPTER: &str = "EFF2_LUTSFX";
const FRAME: i64 = TICKS_PER_SECOND / 24;
const CONTROL_PROFILE: &str = "imported_png_matte_controls";
const OVERLAY_PROFILE: &str = "imported_png_matte_overlay";
const OVERLAY_RATE: &str = "24000/1001";
const OVERLAY_FRAME: i64 = TICKS_PER_SECOND / 24_000 * 1001;

pub(crate) struct PrepareIssue {
    pub message: String,
    pub edit: Option<FailedPngEdit>,
}

pub(crate) struct FailedPngEdit {
    pub sequence: String,
    pub id: String,
    pub track: i64,
    pub record_start: Option<Rational>,
    pub saved_locators: Vec<String>,
    pub shared_overlay_source: bool,
}

impl From<String> for PrepareIssue {
    fn from(message: String) -> Self {
        Self { message, edit: None }
    }
}

impl From<&str> for PrepareIssue {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

#[derive(Default)]
pub(crate) struct Plan {
    pub picture: PreparedPngMattes,
    pub inputs: Vec<SourceInput>,
    pub overlay_omissions: Vec<Loss>,
    /// Masters reconstructed solely as split Matte Key fills.
    pub managed_fill_only: HashSet<String>,
    replaced: HashMap<String, HashMap<&'static str, HashSet<String>>>,
    recovered_reader_notes: HashSet<String>,
    recovered_gap_markers: HashMap<String, HashSet<usize>>,
}

impl Plan {
    pub fn apply(
        &mut self,
        input: &mut prproj::convert::prepared::PreparedProjectInput,
        graph: &graph::source::SourceGraph,
    ) -> Result<(), String> {
        let sequences: HashSet<_> = self.picture.edits.iter()
            .filter(|edit| edit.background.is_some()).map(|edit| edit.sequence).collect();
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
            let recovered = self.recovered_gap_markers.get(&source.id);
            let mut saved = source.markers.iter().enumerate();
            seq.markers.retain_mut(|marker| {
                let (index, saved) = saved.next().expect("marker counts already checked");
                if marker.owner == "sequence" && saved.owner == MarkerOwner::Edit {
                    marker.track = None;
                }
                recovered.is_none_or(|indices| !indices.contains(&index))
            });
        }
        split::position_graphics(self, &mut input.texts);
        input.png_mattes = std::mem::take(&mut self.picture);
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.picture.edits.is_empty() && self.picture.split_edits.is_empty()
    }
    pub fn extra_linked_originals(&self) -> usize {
        self.picture.stills.iter().map(|still| &still.path)
            .chain(self.picture.jpeg_fills.iter()
                .filter(|fill| fill.existing_clip.is_none() && fill.managed_start.is_none())
                .map(|fill| &fill.path))
            .collect::<HashSet<_>>().len()
    }
    pub fn keeps_loss(&self, loss: &Loss) -> bool {
        if loss.aspect == "managed_media_omitted" && self.managed_fill_only.contains(&loss.object) {
            return false;
        }
        // A sequence can have several losses of the same kind. Compare the
        // complete message and owner for each recovered occurrence.
        !self
            .replaced
            .get(&loss.object)
            .and_then(|by_aspect| by_aspect.get(loss.aspect.as_str()))
            .is_some_and(|messages| messages.contains(&loss.reason))
    }

    pub fn keeps_unresolved_master(&self, bin: &str, occurrence: &str, reason: &str) -> bool {
        self.replaced
            .get(bin)
            .and_then(|by_aspect| by_aspect.get("source_master"))
            .is_none_or(|messages| !messages.contains(&unresolved_master_reason(occurrence, reason)))
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

fn unresolved_master_reason(occurrence: &str, reason: &str) -> String {
    format!("{occurrence}: {reason}; native master facts retained on bin")
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
    omitted: bool,
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
    inputs: HashMap<Vec<String>, (VerifiedPng, PngRole)>,
    physical: HashMap<PathBuf, Vec<String>>,
    split: split::Sources<'a>,
}

impl<'a> Builder<'a> {
    fn inspect(
        &mut self,
        import: &Value,
        role: PngRole,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<&VerifiedPng, PngVerificationError> {
        let saved = locators(import).ok_or_else(|| "invalid saved PNG locator".to_owned())?;
        let needs_decode = self
            .inputs
            .get(&saved)
            .is_none_or(|(_, old)| *old != role && !(*old == PngRole::Matte && role == PngRole::Opaque));
        if needs_decode {
            check()?;
            let source = verify_imported_png(&saved, self.mappings, role)?;
            check()?;
            if self
                .physical
                .get(&source.input.path)
                .is_some_and(|other| *other != saved)
                || self
                    .inputs
                    .get(&saved)
                    .is_some_and(|(before, _)| before.input != source.input)
            {
                return Err(PngVerificationError::Invalid(
                    "conflicting PNG original identities or source changed during verification"
                        .into(),
                ));
            }
            self.physical.insert(source.input.path.clone(), saved.clone());
            self.inputs.insert(saved.clone(), (source, role));
        }
        Ok(&self.inputs[&saved].0)
    }

    fn add(
        &mut self,
        bin: usize,
        mob: &'a str,
        name: &str,
        import: &Value,
        alpha: PngAlpha,
        role: PngRole,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<usize, String> {
        let source = self.inspect(import, role, check).map_err(|error| error.to_string())?;
        if let Some(reason) = source.overlay_alpha_issue {
            return Err(reason.into());
        }
        let path = source.input.path.clone();
        if let Some(&index) = self.imported.get(&(bin, mob)) {
            let old = &self.plan.picture.stills[index];
            if old.path != path || old.alpha != alpha {
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
            path,
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
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Plan, PrepareIssue> {
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
                .any(|r| matches!(text(r, "picture_profile"), Some(CONTROL_PROFILE | OVERLAY_PROFILE | split::PROFILE)))
                .then_some((index, seq, receipt))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(Plan::default());
    }
    let jpeg_masters: HashSet<_> = candidates.iter().flat_map(|(_, _, receipt)| split::rows(receipt))
        .filter_map(|row| text(row.get("matte_inputs")?.get("fill")?, "mob")).collect();
    let mut builder = Builder {
        plan: Plan::default(),
        mappings,
        stills: HashMap::new(),
        imported: HashMap::new(),
        inputs: HashMap::new(),
        physical: HashMap::new(),
        split: if jpeg_masters.is_empty() { split::Sources::default() } else { split::Sources::new(read) },
    };
    for (bin, record) in graph.bins.iter().enumerate() {
        let Some(&receipt) = receipts.get(record.id.as_str()) else {
            continue;
        };
        let masters: HashMap<_, _> = [("unresolved_masters", false), ("omitted_media_masters", true)]
            .into_iter()
            .flat_map(|(key, omitted)| list(receipt, key).iter().map(move |master| (master, omitted)))
            .filter_map(|(master, omitted)| Some((text(master, "occurrence")?, (master, omitted))))
            .collect();
        for import in list(receipt, "imported_stills") {
            let (Some(id), Some(mob)) = (text(import, "id"), text(import, "mob")) else {
                continue;
            };
            if jpeg_masters.contains(mob) {
                builder.split.saved_imports.entry(mob)
                    .and_modify(|found| *found = None).or_insert(Some((bin, import)));
            }
            let Some(&(master, omitted)) = masters.get(id) else {
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
                        omitted,
                    },
                )
                .is_some()
            {
                return Err("same PNG master identity occurs in more than one selected bin".into());
            }
        }
    }
    for (index, seq, receipt) in candidates {
        let issues = list(receipt, "unresolved");
        let result = if issues.iter().any(|row| text(row, "picture_profile") == Some(CONTROL_PROFILE)) {
            prepare_sequence(&mut builder, read, index, seq, receipt, check)
        } else {
            (|| {
                if overlay_rows(receipt).next().is_some() {
                    prepare_overlays(&mut builder, read, index, seq, receipt, check)?;
                }
                if split::rows(receipt).next().is_some() {
                    split::prepare(&mut builder, read, index, seq, receipt, ffprobe, check)?;
                }
                Ok(())
            })()
        };
        result.map_err(|mut issue| {
            issue.message = format!("PNG Matte Key · {}: {}", seq.name, issue.message);
            issue
        })?;
    }
    let linked: HashSet<_> = builder.plan.picture.stills.iter().map(|still| still.path.as_path()).collect();
    builder.plan.inputs.extend(builder
        .inputs
        .into_values()
        .filter_map(|(source, _)| (linked.contains(source.input.path.as_path())
            || builder.split.used_alpha.contains(source.input.path.as_path())).then_some(source.input)));
    builder.plan.inputs.extend(builder.split.imports.into_values().flatten());
    builder.plan.inputs.extend(builder.split.managed_inputs.into_values().map(|(_, _, source)| source));
    builder.plan.inputs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(builder.plan)
}

fn valid_sequence_clock(seq: &graph::source::SourceSequence, rate: Rational) -> bool {
    seq.clock.as_ref().is_some_and(|clock| {
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
    })
}

fn prepare_sequence<'a>(
    builder: &mut Builder<'a>,
    read: &'a ReadResult,
    sequence: usize,
    seq: &'a graph::source::SourceSequence,
    receipt: &'a Value,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), PrepareIssue> {
    let rate = Rational::integer(24);
    if seq.rate != rate
        || !valid_sequence_clock(seq, rate)
        || seq.start.and_then(|start| start.to_units(24)).is_none()
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
    let issues = list(receipt, "unresolved");
    let mut gaps = HashMap::new();
    for gap in issues.iter().filter(|issue| issue.get("omitted_master").is_some()) {
        let id = text(gap, "occurrence").ok_or("missing PNG gap identity")?;
        if gaps.insert(id, gap).is_some() {
            return Err("duplicate PNG gap identity".into());
        }
    }
    let mut gap_markers = HashMap::new();
    for (index, marker) in seq.markers.iter().enumerate().filter(|(_, marker)|
        marker.owner == MarkerOwner::Sequence && marker.name.starts_with("Media omitted · V"))
    {
        if gap_markers.insert(marker.owner_occurrence.as_str(), (index, marker)).is_some() {
            return Err("duplicate PNG gap marker ownership".into());
        }
    }
    let gap_losses: HashSet<_> = read.graph.losses_for(&seq.id, "media_gap").collect();
    for issue in issues.iter().filter(|issue| issue.get("omitted_master").is_none()) {
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
            ).into());
        }
        let owner = text(issue, "owner").ok_or("missing effect occurrence")?;
        if !effects.insert(owner) {
            return Err("duplicate PNG effect occurrence".into());
        }
        let source_issue = |message, import| PrepareIssue {
            message,
            edit: Some(FailedPngEdit {
                sequence: seq.id.clone(),
                id: owner.into(),
                track: lane,
                record_start: Rational::new(frame.into(), 24),
                saved_locators: locators(import).unwrap_or_default(),
                shared_overlay_source: false,
            }),
        };
        let profile = (text(issue, "effect_id"), text(issue, "picture_profile"));
        let (index, aspect) = match profile {
            (Some(MATTE), Some(CONTROL_PROFILE)) if lane == 2 => {
                let (master, import, invert) =
                    matte_sources(issue, 1, "24").ok_or("unresolved saved fill or alpha")?;
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
                    PngRole::Matte,
                    check,
                ).map_err(|message| source_issue(message, import))?;
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
                let gap = if still.omitted {
                    let gap = gaps.remove(id).ok_or("omitted PNG master has no exact saved gap")?;
                    let (marker_index, marker) = gap_markers.remove(id)
                        .ok_or("omitted PNG master has no exact gap marker")?;
                    let expected_start = Rational::new(frame.into(), 24);
                    if !matches_omitted_event(gap, event, occurrence, reason)
                        || marker.name != format!("Media omitted · V{lane}")
                        || marker.start != expected_start
                        || marker.duration != Rational::new(1, 24).unwrap()
                        || !gap_losses.contains(marker.comment.as_str())
                    {
                        return Err("omitted PNG gap does not match its verified edit and source".into());
                    }
                    Some((marker_index, marker.comment.as_str()))
                } else {
                    None
                };
                let index = builder.add(still_bin, mob, &name, import, PngAlpha::Ignore, PngRole::Opaque, check)
                    .map_err(|message| source_issue(message, import))?;
                let bin_id = &read.graph.bins[still_bin].id;
                if let Some((marker_index, comment)) = gap {
                    builder.plan.recovered_gap_markers.entry(seq.id.clone()).or_default().insert(marker_index);
                    builder.plan.replace_loss(&seq.id, "media_gap", comment.into());
                    builder.plan.replace_loss(bin_id, "media_bin_omission",
                        format!("Omitted {name:?}, bin item {occurrence}. {reason}"));
                } else {
                    builder.plan.replace_loss(bin_id, "source_master",
                        unresolved_master_reason(occurrence, reason));
                }
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
                ).into())
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
            ).into());
        }
        if builder.plan.picture.edits.len() + frames.len() > 10_000 {
            return Err("too many PNG picture edits".into());
        }
    }
    if consumed.len() != native.len() || !gaps.is_empty() {
        return Err("some native picture inputs have no verified edit placement".into());
    }
    for (frame, [background, foreground]) in frames {
        let background =
            background.ok_or_else(|| format!("V2 frame {frame} has no exact V1 background"))?;
        builder.plan.picture.edits.push(PreparedPngEdit {
            sequence,
            start: frame * FRAME,
            duration: FRAME,
            background: Some(background),
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

fn valid_overlay_sequence(seq: &graph::source::SourceSequence, receipt: &Value) -> bool {
    let rate = Rational::new(24_000, 1001).unwrap();
    seq.rate == rate
        && valid_sequence_clock(seq, rate)
        && seq.frame_size.is_none_or(|size| size == (1920, 1080))
        && list(receipt, "compatibility_frame_size") == [Value::Int(1920), Value::Int(1080)]
}

struct SavedOverlay<'a> {
    owner: &'a str,
    frame: i64,
    length: i64,
    start: i64,
    duration: i64,
    master: &'a str,
    import: &'a Value,
    invert: bool,
    reason: &'a str,
}

impl<'a> SavedOverlay<'a> {
    fn read(issue: &'a Value, owners: &mut HashSet<&'a str>) -> Result<Self, &'static str> {
        let owner = text(issue, "owner").ok_or("missing effect occurrence")?;
        let frame = issue.get("record_start").and_then(|r| number(r, "value"))
            .filter(|frame| *frame >= 0).ok_or("unresolved Matte Key record position")?;
        let length = number(issue, "record_length").filter(|n| *n > 0)
            .ok_or("unresolved Matte Key length")?;
        if text(issue, "effect_id") != Some(MATTE)
            || number(issue, "track") != Some(2)
            || number(issue, "kind") != Some(1)
            || text(issue, "record_rate") != Some(OVERLAY_RATE)
            || !owners.insert(owner)
        {
            return Err("contradictory PNG overlay occurrence or record clock");
        }
        let start = frame.checked_mul(OVERLAY_FRAME).ok_or("Matte Key start exceeds Premiere timing bounds")?;
        let duration = length.checked_mul(OVERLAY_FRAME).ok_or("Matte Key length exceeds Premiere timing bounds")?;
        let (master, import, invert) =
            matte_sources(issue, length, OVERLAY_RATE).ok_or("unresolved saved fill or alpha")?;
        let reason = text(issue, "reason").ok_or("missing effect receipt reason")?;
        Ok(Self { owner, frame, length, start, duration, master, import, invert, reason })
    }

    fn failed_edit(&self, sequence: &str, shared_overlay_source: bool) -> FailedPngEdit {
        FailedPngEdit {
            sequence: sequence.into(), id: self.owner.into(), track: 2,
            record_start: Rational::new(self.start.into(), TICKS_PER_SECOND.into()),
            saved_locators: locators(self.import).unwrap_or_default(),
            shared_overlay_source,
        }
    }
}

fn overlay_rows(receipt: &Value) -> impl Iterator<Item = &Value> {
    list(receipt, "unresolved").iter()
        .filter(|issue| text(issue, "picture_profile") == Some(OVERLAY_PROFILE))
}

/// The same saved locators select the same PNG during this Review. Restrict
/// additional details to qualified overlay routes without inspecting more files.
pub(crate) fn matching_overlay_failures(
    source: &graph::source::SourceGraph,
    failed: &FailedPngEdit,
    limit: usize,
    check_cancel: impl Fn() -> bool,
) -> Vec<FailedPngEdit> {
    let mut matches = Vec::new();
    if !failed.shared_overlay_source || failed.saved_locators.is_empty() || limit == 0 {
        return matches;
    }
    let sequences: HashMap<_, _> = source.sequences.iter()
        .map(|seq| (seq.id.as_str(), seq)).collect();
    for receipt in source.sidecars.iter().filter(|row| row.adapter == "avid.source") {
        if check_cancel() {
            break;
        }
        let Some(&sequence) = sequences.get(receipt.object.as_str()) else { continue };
        if !valid_overlay_sequence(sequence, &receipt.data) {
            continue;
        }
        let mut owners = HashSet::new();
        let rows = overlay_rows(&receipt.data);
        let mut counts = HashMap::new();
        for row in overlay_rows(&receipt.data) {
            if check_cancel() {
                return matches;
            }
            if let Some(owner) = text(row, "owner") {
                *counts.entry(owner).or_insert(0usize) += 1;
            }
        }
        for row in rows {
            if check_cancel() || matches.len() == limit {
                return matches;
            }
            let Ok(overlay) = SavedOverlay::read(row, &mut owners) else { continue };
            if overlay.owner != failed.id
                && counts[overlay.owner] == 1
                && overlay.owner.strip_prefix(&sequence.id).is_some_and(|suffix| suffix.starts_with('/'))
                && locators(overlay.import).as_ref() == Some(&failed.saved_locators)
            {
                matches.push(overlay.failed_edit(&sequence.id, true));
            }
        }
    }
    matches
}

fn prepare_overlays<'a>(
    builder: &mut Builder<'a>,
    read: &'a ReadResult,
    sequence: usize,
    seq: &'a graph::source::SourceSequence,
    receipt: &'a Value,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), PrepareIssue> {
    if !valid_overlay_sequence(seq, receipt) {
        return Err("requires a 1080p23.976 sequence with a saved non-drop origin".into());
    }
    let bin = *read.sequence_bins.get(&seq.id).ok_or("missing source bin assignment")?;
    let mut owners = HashSet::new();
    for issue in overlay_rows(receipt) {
        let overlay = SavedOverlay::read(issue, &mut owners)?;
        let SavedOverlay { owner, frame, length, start, duration, master, import, invert, reason } = overlay;
        if builder.plan.picture.edits.len() >= 10_000 {
            return Err("too many PNG picture edits".into());
        }
        let unit = if length == 1 { "frame" } else { "frames" };
        let span = format!("V2 at record frame {frame} for {length} {unit} ({OVERLAY_RATE} fps)");
        let omission = match builder.inspect(import, PngRole::BinaryMatte, check) {
            Ok(source) => source.overlay_alpha_issue.map(str::to_owned),
            Err(PngVerificationError::MissingOriginal) => {
                let saved = locators(import).expect("verified locator list");
                Some(format!(
                    "The original PNG is unavailable. Restore it at {} or map its saved root",
                    saved.iter().map(|path| format!("{path:?}")).collect::<Vec<_>>().join(" or "),
                ))
            }
            Err(error) => return Err(PrepareIssue {
                message: error.to_string(),
                edit: Some(overlay.failed_edit(&seq.id, true)),
            }),
        };
        if let Some(reason) = omission {
            builder.plan.overlay_omissions.push(Loss {
                object: owner.into(),
                aspect: "picture_effect".into(),
                reason: format!("{span}: PNG Matte Key omitted. {reason}. The lower picture track is retained."),
            });
        } else {
            let foreground = builder.add(
                bin, master, "Matte Key overlay", import, PngAlpha::Straight { invert: !invert },
                PngRole::BinaryMatte, check,
            ).map_err(|message| PrepareIssue {
                message, edit: Some(overlay.failed_edit(&seq.id, false)),
            })?;
            builder.plan.picture.edits.push(PreparedPngEdit {
                sequence, start, duration, background: None, foreground: Some(foreground),
            });
        }
        builder.plan.replace_loss(&seq.id, "picture_effect", format!("{span}: {reason}"));
        builder.plan.recovered_reader_notes.insert(format!(
            "{:?}: V2 at frame {frame}: Matte Key (EFF2_2DMATTE) not converted", seq.name
        ));
    }
    Ok(())
}

fn matte_sources<'a>(issue: &'a Value, length: i64, rate: &str) -> Option<(&'a str, &'a Value, bool)> {
    let (fill, alpha, import, invert) = still_sources(issue, length, rate)?;
    (fill == alpha).then_some((fill, import, invert))
}

fn still_sources<'a>(issue: &'a Value, length: i64, rate: &str) -> Option<(&'a str, &'a str, &'a Value, bool)> {
    let inputs = issue.get("matte_inputs")?;
    if text(inputs, "background") != Some("lower_track") {
        return None;
    }
    let fill = inputs.get("fill")?;
    let matte = inputs.get("matte")?;
    let master = text(fill, "mob")?;
    if number(fill, "track") != Some(1)
        || number(matte, "track") != Some(2)
        || [fill, matte].iter().any(|input| {
            number(input, "start") != Some(0)
                || number(input, "length") != Some(length)
                || text(input, "rate") != Some(rate)
        })
    {
        return None;
    }
    let import = issue.get("matte_import")?;
    (flag(import, "ignore_alpha") == Some(false)).then_some((
        master,
        text(matte, "mob")?,
        import,
        flag(import, "invert_alpha")?,
    ))
}

fn matches_omitted_event(gap: &Value, event: &Value, master: &str, reason: &str) -> bool {
    text(gap, "omitted_master") == Some(master)
        && text(gap, "reason") == Some(reason)
        && text(event, "id").is_some_and(|id| text(gap, "occurrence") == Some(id))
        && ["owner", "track", "master_track"].iter().all(|key| {
            event.get(key).is_some_and(|value| gap.get(key) == Some(value))
        })
        && number(gap, "track_kind") == Some(1)
        && event.get("range").is_some_and(|range| gap.get("saved_range") == Some(range))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_gap_recovery_requires_exact_occurrence_master_and_range() {
        let event = graph::json::parse(r#"{"id":"edit","owner":"owner","track":2,"master_track":1,"range":{"record_start":4,"record_length":1}}"#).unwrap();
        let saved = r#"{"occurrence":"edit","owner":"owner","omitted_master":"import","reason":"missing managed file","track_kind":1,"track":2,"master_track":1,"saved_range":{"record_start":4,"record_length":1}}"#;
        let gap = graph::json::parse(saved).unwrap();
        assert!(matches_omitted_event(&gap, &event, "import", "missing managed file"));
        assert!(!matches_omitted_event(&gap, &event, "another import", "missing managed file"));
        for (old, changed) in [(r#""edit""#, r#""other edit""#), ("record_start\":4", "record_start\":5")] {
            let difference = graph::json::parse(&saved.replace(old, changed)).unwrap();
            assert!(!matches_omitted_event(&difference, &event, "import", "missing managed file"));
        }
    }
}
