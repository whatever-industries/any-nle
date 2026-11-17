//! Optional source witnesses attached to the existing owned extraction program.
//! No native node/address survives bin compilation. Legacy callers never enable
//! capture and continue through the same compatibility finish implementation.
use super::*;
use crate::source::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::mem::size_of;
use std::rc::Rc;
mod groups;
mod managed_multistream;
mod imported_audio;
mod matte;
mod source_color;
mod timewarp;
pub(super) use timewarp::{direct_input as timewarp_input, is_sampled_effect, uses_editable_position};
pub(super) use matte::neutral_keys as neutral_effect_keys;

type SharedBudget = Rc<RefCell<Budget>>;
struct Budget {
    limit: usize,
    used: usize,
    peak: usize,
    error: Option<String>,
    shared: HashSet<usize>,
}
impl Budget {
    fn charge(&mut self, bytes: usize) -> bool {
        if self.error.is_some() {
            return false;
        }
        match self.used.checked_add(bytes).filter(|n| *n <= self.limit) {
            Some(n) => {
                self.used = n;
                self.peak = self.peak.max(n);
                true
            }
            None => {
                self.error = Some("accounted source facts exceed configured limit".into());
                false
            }
        }
    }
    fn fail(&mut self, reason: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(reason.into());
        }
    }
    fn release(&mut self, bytes: usize) {
        if let Some(remaining) = self.used.checked_sub(bytes) {
            self.used = remaining;
        } else {
            self.fail("source allocation accounting underflow");
        }
    }
}
fn charge(budget: &SharedBudget, bytes: usize) -> bool {
    budget.borrow_mut().charge(bytes)
}
// Source-owned growth is reserved/accounted before allocation. Existing decoder
// and compatibility-compiler working storage have their own bounded lifetimes.
fn reserve<T>(budget: &SharedBudget, values: &mut Vec<T>, additional: usize) -> bool {
    reserve_with_floor(budget, values, additional, 4)
}
fn reserve_with_floor<T>(budget: &SharedBudget, values: &mut Vec<T>, additional: usize, floor: usize) -> bool {
    let Some(required) = values.len().checked_add(additional) else {
        budget.borrow_mut().fail("source vector length overflow");
        return false;
    };
    if required <= values.capacity() {
        return true;
    }
    let capacity = required
        .max(values.capacity().checked_mul(2).unwrap_or(required))
        .max(floor);
    let Some(bytes) = (capacity - values.capacity()).checked_mul(size_of::<T>()) else {
        budget
            .borrow_mut()
            .fail("source vector allocation overflow");
        return false;
    };
    if !charge(budget, bytes) {
        return false;
    }
    if values.try_reserve_exact(capacity - values.len()).is_err() {
        budget.borrow_mut().fail("source fact allocation failed");
        return false;
    }
    // Standard Vec currently reports requested capacity; any extra reported slots
    // are conservatively accounted too, before accepting the retained allocation.
    if values.capacity() > capacity
        && !charge(
            budget,
            (values.capacity() - capacity).saturating_mul(size_of::<T>()),
        )
    {
        return false;
    }
    true
}
fn copy_text(budget: &SharedBudget, text: &str) -> String {
    if !charge(budget, text.len()) {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(text);
    out
}
fn copy_text_properties<'a, V: AsRef<str>>(
    budget: &SharedBudget,
    properties: Result<impl IntoIterator<Item = (&'static str, usize, &'a str, V)>, &'static str>,
    retained: &mut Vec<NativeTextProperty>,
    reason: &mut Option<String>,
) -> bool {
    match properties {
        Ok(properties) => {
            for (scope, list, name, value) in properties {
                if !reserve(budget, retained, 1) {
                    return false;
                }
                retained.push(NativeTextProperty {
                    scope: copy_text(budget, scope),
                    list,
                    name: copy_text(budget, name),
                    value: copy_text(budget, value.as_ref()),
                });
            }
        }
        Err(error) => *reason = Some(copy_text(budget, error)),
    }
    true
}
fn formatted(budget: &SharedBudget, args: std::fmt::Arguments<'_>) -> String {
    struct Counter(usize, bool);
    impl std::fmt::Write for Counter {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            match self.0.checked_add(s.len()) {
                Some(n) => {
                    self.0 = n;
                    Ok(())
                }
                None => {
                    self.1 = true;
                    Err(std::fmt::Error)
                }
            }
        }
    }
    let mut counter = Counter(0, false);
    if counter.write_fmt(args).is_err() || counter.1 {
        budget.borrow_mut().fail("source string length overflow");
        return String::new();
    }
    if !charge(budget, counter.0) {
        return String::new();
    }
    let mut out = String::with_capacity(counter.0);
    let _ = out.write_fmt(args);
    out
}
fn known(value: Option<i64>, budget: &SharedBudget, reason: &str) -> Observed<i64> {
    value
        .map(Observed::Known)
        .unwrap_or_else(|| Observed::Unresolved(copy_text(budget, reason)))
}
fn borrowed_attrs<'a>(ctx: &Ctx<'a>, node: &Node) -> &'a [(String, Value)] {
    match ctx
        .deref(node.get("attributes"))
        .and_then(|a| a.get("entries"))
    {
        Some(Value::Attrs(items)) => items,
        _ => &[],
    }
}
const MAX_INPUT_TRANSFORM_BYTES: usize = 16 * 1024;
fn input_transform_text<'a>(ctx: &Ctx<'a>, file: &Node) -> Option<Result<&'a str, &'static str>> {
    if int(file, "mob_type_id") != Some(MOB_SOURCE) {
        return None;
    }
    let mut values = borrowed_attrs(ctx, file).iter()
        .filter(|(key, _)| key == "_COLOR_INPUT_TRANSFORMATION");
    let (_, value) = values.next()?;
    Some(if values.next().is_some() {
        Err("duplicate Avid source color transformation attributes")
    } else if let Some(text) = value.as_str() {
        if !text.is_empty() && text.len() <= MAX_INPUT_TRANSFORM_BYTES {
            Ok(text)
        } else {
            Err("empty or oversized Avid source color transformation")
        }
    } else {
        Err("Avid source color transformation is not saved as text")
    })
}
fn saved_input_transform(ctx: &Ctx<'_>, file: &Node, budget: &SharedBudget) -> Option<Observed<String>> {
    Some(match input_transform_text(ctx, file)? {
        Ok(text) => Observed::Known(copy_text(budget, text)),
        Err(reason) => Observed::Unresolved(copy_text(budget, reason)),
    })
}
fn each_marker<'a>(ctx: &Ctx<'a>, node: &Node, mut action: impl FnMut(&'a Node)) {
    for (key, value) in borrowed_attrs(ctx, node) {
        if key != "_TMP_CRM" {
            continue;
        }
        let Some(list) = ctx.deref(Some(value)) else {
            continue;
        };
        match (&list.class, list.get("items")) {
            (b"TMCS", Some(Value::RefList(items))) => {
                for item in items {
                    if let Some(marker) = ctx.deref(Some(item)).filter(|m| m.class == *b"TMBC") {
                        action(marker);
                    }
                }
            }
            (b"TMBC", _) => action(list),
            _ => {}
        }
    }
}
fn read_marker(ctx: &Ctx<'_>, node: &Node, budget: &SharedBudget) -> (AvidMarker, Observed<i64>) {
    let mut fields = [""; 7];
    let mut length = None;
    let mut created = None;
    let mut track = None;
    for (key, value) in borrowed_attrs(ctx, node) {
        let text = value.as_str().unwrap_or("");
        match key.as_str() {
            "_ATN_CRM_COM" => fields[0] = text,
            "_ATN_CRM_MARKNAME" => fields[1] = text,
            "_ATN_CRM_USER" => fields[2] = text,
            "_ATN_CRM_COLOR_EXTENDED" => fields[3] = text,
            "_ATN_CRM_COLOR" => fields[4] = text,
            "_ATN_CRM_ID" => fields[5] = text,
            "_ATN_CRM_TRK" => track = value.as_str(),
            "_ATN_CRM_LENGTH" => length = value.as_int().filter(|n| *n > 0),
            "_ATN_CRM_LONG_CREATE_DATE" => created = value.as_int(),
            _ => {}
        }
    }
    let local = known(
        int(node, "comp_offset"),
        budget,
        "marker comp_offset is absent",
    );
    let mut marker = AvidMarker {
        frame: int(node, "comp_offset").unwrap_or(0),
        length,
        created,
        name: copy_text(budget, fields[1]),
        comment: copy_text(budget, fields[0]),
        user: copy_text(budget, fields[2]),
        color: copy_text(
            budget,
            if fields[3].is_empty() {
                fields[4]
            } else {
                fields[3]
            },
        ),
        id: copy_text(budget, fields[5]),
        track_attr: track.map(|t| copy_text(budget, t)),
        ..Default::default()
    };
    if let Some(Value::List(color)) = node.get("color") {
        for (i, value) in color.iter().take(3).enumerate() {
            marker.rgb16[i] = value.as_int().unwrap_or(0);
        }
    }
    (marker, local)
}
fn sum(values: impl IntoIterator<Item = usize>) -> usize {
    values
        .into_iter()
        .try_fold(0usize, |a, b| a.checked_add(b))
        .unwrap_or(usize::MAX)
}
fn array<T>(capacity: usize) -> usize {
    capacity.checked_mul(size_of::<T>()).unwrap_or(usize::MAX)
}
fn first_shared(budget: &SharedBudget, key: usize) -> bool {
    let mut budget = budget.borrow_mut();
    if budget.shared.contains(&key) {
        return false;
    }
    if !budget.charge(4 * size_of::<usize>()) {
        return false;
    }
    budget.shared.insert(key);
    true
}
fn strings(values: &[String]) -> usize {
    sum(std::iter::once(array::<String>(values.len())).chain(values.iter().map(String::capacity)))
}
fn marker_cost(m: &AvidMarker) -> usize {
    sum([
        size_of::<AvidMarker>(),
        m.name.capacity(),
        m.comment.capacity(),
        m.user.capacity(),
        m.color.capacity(),
        m.id.capacity(),
        m.track_attr.as_ref().map_or(0, String::capacity),
    ])
}
fn color_cost(c: &ColorCorrection) -> usize {
    sum(
        std::iter::once(array::<(String, ParamValue)>(c.params.capacity())).chain(
            c.params.iter().map(|(k, v)| {
                sum([
                    k.capacity(),
                    if let ParamValue::Bytes(b) = v {
                        b.capacity()
                    } else {
                        0
                    },
                ])
            }),
        ),
    )
}
fn picture_cost(picture: &PictureEffect) -> usize {
    sum([size_of::<PictureEffect>(), color_cost(&picture.params)])
}
fn matte_cost(m: &AniMatteData) -> usize {
    sum(
        std::iter::once(array::<crate::animatte::AniMatteState>(m.states.capacity()))
            .chain(m.states.iter().map(|s| {
                sum(
                    std::iter::once(array::<crate::animatte::AniMatteShape>(s.shapes.capacity()))
                        .chain(
                            s.shapes.iter().map(|s| {
                                array::<crate::animatte::AniMattePoint>(s.points.capacity())
                            }),
                        ),
                )
            }))
            .chain([
                m.tracker_xy
                    .as_ref()
                    .map_or(0, |v| array::<[f64; 2]>(v.capacity())),
                strings(&m.not_converted),
            ]),
    )
}

/// Linked OP1a file tracks use kind-local labels. The container descriptor
/// owns the file locator and each matching child carries the stream extent.
fn linked_op1a_child<'a>(ctx: &Ctx<'a>, top: &'a Node, kind: i64, track: Option<i64>) -> Option<&'a Node> {
    if top.class != *b"MULD" { return None; }
    let locator = |descriptor: &Node| ctx.deref(descriptor.get("physical_media"))
        .filter(|node| node.class == *b"MDES")
        .and_then(|node| ctx.deref(node.get("locator")))
        .filter(|node| node.class == *b"FILE");
    let saved = locator(top)?;
    let Value::RefList(children) = top.get("descriptors")? else { return None; };
    let [picture, first_audio, rest @ ..] = children.as_slice() else { return None; };
    if rest.len() > 1 || rest.first() == Some(first_audio) { return None; }
    let picture = ctx.deref(Some(picture)).filter(|node| node.class == *b"CDCI")?;
    let first = ctx.deref(Some(first_audio)).filter(|node| node.class == *b"PCMA")?;
    let second = match rest {
        [] if int(first, "channels") == Some(2) => None,
        [reference] if int(first, "channels") == Some(1) => Some(ctx.deref(Some(reference))
            .filter(|node| node.class == *b"PCMA" && int(node, "channels") == Some(1))?),
        _ => return None,
    };
    for descriptor in [Some(picture), Some(first), second].into_iter().flatten() {
        if !locator(descriptor).is_some_and(|child|
            ["path_posix", "path_utf8", "path"].into_iter().all(|key| child.get(key) == saved.get(key)))
        {
            return None;
        }
    }
    match (kind, track) {
        (KIND_PICTURE, Some(1)) => Some(picture),
        (KIND_SOUND, Some(1)) => Some(first),
        (KIND_SOUND, Some(2)) => Some(second.unwrap_or(first)),
        _ => None,
    }
}

fn master_cost(m: &MasterClip) -> usize {
    sum([
        size_of::<SourceMaster>(),
        m.name.capacity(),
        array::<MediaTrack>(m.tracks.capacity()),
        sum(m.tracks.iter().map(|t| match &t.essence {
            Some(Essence::Picture { compression, .. }) => compression.capacity(),
            Some(Essence::Other(s)) => s.capacity(),
            _ => 0,
        })),
        array::<(String, String)>(m.columns.capacity()),
        sum(m
            .columns
            .iter()
            .map(|(k, v)| sum([k.capacity(), v.capacity()]))),
        array::<AuxTimecode>(m.aux_timecodes.capacity()),
        m.source_file.as_ref().map_or(0, String::capacity),
        m.source_path.as_ref().map_or(0, String::capacity),
        array::<(i64, i64)>(m.stereo_pairs.capacity()),
        strings(&m.other_groups),
        m.output_mix.payload_capacity(),
        array::<(i64, i64, AvidMarker)>(m.markers.capacity()),
        sum(m.markers.iter().map(|(_, _, m)| marker_cost(m))),
    ])
}

pub(super) struct InputContext {
    revision: String,
    bin: usize,
    budget: SharedBudget,
}
#[derive(Debug)]
pub(super) struct MasterMeta {
    pub(super) id: String,
    pub(super) offsets: Vec<Observed<i64>>,
    pub(super) marker_witnesses: Vec<MasterMarkerWitness>,
    pub(super) originals: Vec<Option<OriginalTrack>>,
    pub(super) file_clocks: Vec<Option<SavedFileClock>>,
    pub(super) managed_multistream: Option<ManagedMultistream>,
    pub(super) imported_audio: Option<Box<SavedPcmImport>>,
    pub(super) pcm_prefix: Option<crate::source::SourcePcmPrefix>,
    pub(super) input_transforms: Vec<SourceInputTransform>,
    pub(super) rendered_freeze: bool,
    pub(super) rendered_gain: Option<MobId>,
    pub(super) imported_still: Option<SourceImportedStill>,
}
impl InputContext {
    pub(super) fn failed(&self) -> bool {
        self.budget.as_ref().borrow().error.is_some()
    }
    pub(super) fn retain_route(
        &self,
        values: &mut Vec<(MobId, MobRoute)>,
        id: MobId,
        route: MobRoute,
    ) {
        if !charge(&self.budget, route_cost(&route, &self.budget))
            || !reserve(&self.budget, values, 1)
        {
            return;
        }
        values.push((id, route));
    }
    pub(super) fn retain_item(&self, values: &mut Vec<Item>, item: Item) {
        let cost = match &item {
            Item::Master { name, result, .. } => sum([
                name.capacity(),
                result.as_ref().map_or_else(|e| e.capacity(), master_cost),
            ]),
            Item::Skipped(s) => sum([s.name.capacity(), s.reason.capacity()]),
            Item::Sequence(_) => 0,
            Item::Subclip(_) => 0,
            Item::Group(_) => 0,
            Item::GroupDependency { name, .. } => name.capacity(),
        };
        if !charge(&self.budget, cost) || !reserve(&self.budget, values, 1) {
            return;
        }
        values.push(item);
    }
    pub(super) fn subclip(&self, ctx: &Ctx<'_>, mob: &Node, item: usize) -> SourceSubclip {
        let budget = &self.budget;
        let id = formatted(budget, format_args!("{}/bin/{}/item/{}", self.revision, self.bin, item));
        let attribute = |name| {
            let mut values = borrowed_attrs(ctx, mob).iter().filter(|(key, _)| key == name);
            match (values.next(), values.next()) {
                (Some((_, value)), None) => known(value.as_int(), budget, "non-integer attribute"),
                (None, _) => known(None, budget, "attribute absent"),
                (_, Some(_)) => known(None, budget, "duplicate attribute"),
            }
        };
        let mut tracks = Vec::new();
        for track in ctx.tracks(mob) {
            if !reserve(budget, &mut tracks, 1) {
                break;
            }
            let component = ctx.deref(track.get("component"));
            let source = component.filter(|c| c.class == *b"SCLP");
            tracks.push(SourceSubclipTrack {
                label: int(track, "index"),
                component_class: component.map(|c| c.class),
                kind: component.and_then(|c| int(c, "media_kind_id")),
                rate: component.and_then(|c| float(c, "edit_rate")).and_then(Rate::from_avid),
                length: component.and_then(|c| int(c, "length")),
                target_mob: source.and_then(mob_id),
                target_track: source.and_then(|c| int(c, "track_id")),
                target_offset: source.and_then(|c| int(c, "start_time")),
            });
        }
        SourceSubclip {
            id,
            bin: self.bin,
            mob: mob_id(mob),
            name: copy_text(budget, mob.get("name").and_then(Value::as_str).unwrap_or("")),
            rate: float(mob, "edit_rate").and_then(Rate::from_avid),
            backing_length: int(mob, "length"),
            start: attribute("_START"),
            end: attribute("_END"),
            tracks,
        }
    }
    pub(super) fn master_meta(
        &self,
        ctx: &Ctx<'_>,
        mob: &Node,
        item: usize,
        master: Option<&MasterClip>,
        ambiguous: &std::collections::HashSet<MobId>,
        pcm_prefix: Option<&super::pcm_prefix::Binding<'_>>,
    ) -> MasterMeta {
        let id = formatted(
            &self.budget,
            format_args!("{}/bin/{}/item/{}", self.revision, self.bin, item),
        );
        let mut offsets = Vec::new();
        let mut marker_witnesses = Vec::new();
        let mut originals = Vec::new();
        let mut file_clocks = Vec::new();
        let mut input_transforms = Vec::new();
        if let Some(master) = master {
            for native in &master.tracks {
                let mut matched = None;
                let mut count = 0;
                for track in ctx.tracks(mob) {
                    if int(track, "index") == Some(native.index)
                        && ctx
                            .deref(track.get("component"))
                            .and_then(|c| int(c, "media_kind_id"))
                            == Some(native.kind)
                    {
                        matched = Some(track);
                        count += 1;
                    }
                }
                if !reserve(&self.budget, &mut offsets, 1) {
                    break;
                }
                let clip = matched.and_then(|track| pcm_prefix.and_then(|p| p.clip(native.kind, native.index))
                    .or_else(|| ctx.only_source_clip(track).ok()));
                let offset = if count == 1 {
                    known(
                        clip.and_then(|c| int(c, "start_time")),
                        &self.budget,
                        "master SourceClip start is absent or unresolved",
                    )
                } else {
                    Observed::Unresolved(copy_text(
                        &self.budget,
                        "master source track is ambiguous",
                    ))
                };
                offsets.push(offset);
                if !reserve(&self.budget, &mut originals, 1) { break; }
                let file = ctx.mobs.get(&native.file_mob);
                if native.kind == KIND_PICTURE && !ambiguous.contains(&native.file_mob) {
                    if let Some(value) = file.and_then(|file| saved_input_transform(ctx, file, &self.budget)) {
                        if !reserve(&self.budget, &mut input_transforms, 1) { break; }
                        input_transforms.push(SourceInputTransform {
                            track: native.index,
                            file_mob: native.file_mob,
                            value,
                        });
                    }
                }
                let file_track = clip.and_then(|c| int(c, "track_id"));
                let descriptor = file.and_then(|m| ctx.deref(m.get("descriptor")));
                if !reserve(&self.budget, &mut file_clocks, 1) { break; }
                file_clocks.push(file.and_then(|file| saved_file_clock(ctx, file, native, file_track?, ambiguous)));
                let original = descriptor.and_then(|root_descriptor| {
                    let d = if matches!(Rate::from_avid(master.edit_rate).map(|rate| (rate.num, rate.den)),
                        Some((25, 1) | (30_000, 1001)))
                    {
                        linked_op1a_child(ctx, root_descriptor, native.kind, file_track)
                            .unwrap_or(root_descriptor)
                    } else {
                        root_descriptor
                    };
                    let mut locators = Vec::new();
                    let mut current = Some(d);
                    // Bounded descriptor-only chain; unknown nodes never become path witnesses.
                    for _ in 0..8 {
                        let Some(node) = current else { break; };
                        if let Some(locator) = ctx.deref(node.get("locator")).filter(|n| n.class == *b"FILE") {
                            for field in ["path_posix", "path_utf8", "path"] {
                                if let Some(path) = locator.get(field).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                                    if !locators.iter().any(|p| p == path) && reserve(&self.budget, &mut locators, 1) {
                                        locators.push(copy_text(&self.budget, path));
                                    }
                                }
                            }
                        }
                        current = ctx.deref(node.get("physical_media")).filter(|n| n.class == *b"MDES");
                    }
                    if locators.is_empty() { return None; }
                    if !charge(&self.budget, size_of::<OriginalTrack>()) { return None; }
                    let motion_audio_track_start = if native.kind == KIND_SOUND
                        && d.class == *b"MPGA" && d.get("sub_frame_alignment") == Some(&Value::UInt(0))
                        && d.get("origin") == Some(&Value::UInt(0))
                    {
                        file.and_then(|m| {
                            let rate = float(m, "edit_rate").and_then(Rate::from_avid)?;
                            if !matches!((rate.num, rate.den), (24, 1) | (24_000, 1001) | (25, 1) | (30_000, 1001)) {
                                return None;
                            }
                            let tracks = ctx.tracks(m);
                            let mut found = tracks.iter().copied().filter(|t| {
                                int(t, "index") == file_track && file_track.is_some()
                                    && ctx.deref(t.get("component")).and_then(|c| int(c, "media_kind_id")) == Some(KIND_SOUND)
                            });
                            let t = found.next()?;
                            if found.next().is_some() { return None; }
                            let child = ctx.only_source_clip(t).ok()?;
                            (int(child, "start_time") == Some(0)
                                && float(child, "edit_rate").and_then(Rate::from_avid) == Some(rate))
                                .then(|| int(t, "start_pos")).flatten()
                        })
                    } else { None };
                    Some(OriginalTrack {
                        locators,
                        descriptor_class: copy_text(&self.budget, std::str::from_utf8(&d.class).unwrap_or("????")),
                        descriptor_rate: float(d, "edit_rate").unwrap_or(0.0),
                        descriptor_length: int(d, "length").unwrap_or(0),
                        channels: int(d, "channels"),
                        sample_rate: float(d, "sample_rate"),
                        stored_quantization_bits: int(d, "quantization_bits"),
                        display_raster: int(d, "display_width").zip(int(d, "display_height")),
                        file_track,
                        motion_audio_track_start,
                    })
                });
                originals.push(original);
            }
            let rate = Rate::from_avid(master.edit_rate);
            for (track_index, track) in ctx.tracks(mob).into_iter().enumerate() {
                let Some(component) = ctx.deref(track.get("component")) else {
                    continue;
                };
                let Some(kind @ (KIND_PICTURE | KIND_SOUND)) = int(component, "media_kind_id")
                else {
                    continue;
                };
                let label = int(track, "index").unwrap_or(0);
                let owner = formatted(&self.budget, format_args!("{id}/track/{track_index}"));
                master_marker_witnesses(
                    ctx,
                    component,
                    &owner,
                    kind,
                    label,
                    Some(0),
                    rate,
                    false,
                    0,
                    &self.budget,
                    &mut marker_witnesses,
                );
            }
        }
        let imported_still = self.imported_still(ctx, mob, &id, ambiguous);
        let managed_multistream = master.and_then(|master| {
            managed_multistream::capture(ctx, mob, master, ambiguous, &self.budget)
        });
        let imported_audio = master.and_then(|master| {
            imported_audio::capture(ctx, mob, master, &offsets, &file_clocks, ambiguous, &self.budget)
        });
        MasterMeta {
            rendered_freeze: false,
            rendered_gain: None,
            id,
            offsets,
            marker_witnesses,
            originals,
            imported_audio,
            pcm_prefix: pcm_prefix.map(|p| p.fact),
            file_clocks,
            managed_multistream,
            input_transforms,
            imported_still,
        }
    }
    fn imported_still(
        &self, ctx: &Ctx<'_>, mob: &Node, id: &str, ambiguous: &HashSet<MobId>,
    ) -> Option<SourceImportedStill> {
        let master = mob_id(mob)?;
        if ambiguous.contains(&master) || int(mob, "usage_code") != Some(0) {
            return None;
        }
        let attrs = import_settings(ctx, mob)?;
        if unique_attribute(attrs, "_IMPORTPICTSEQ").and_then(Value::as_int) != Some(0) {
            return None;
        }
        let Value::List(tracks) = mob.get("tracks")? else { return None };
        let [Value::Node(track)] = tracks.as_slice() else { return None };
        let label = int(track, "index").filter(|&n| n > 0)?;
        let clip = ctx.deref(track.get("component"))?;
        if clip.class != *b"SCLP" || int(clip, "media_kind_id") != Some(KIND_PICTURE) {
            return None;
        }
        let file_mob = mob_id(clip)?;
        let file_track = int(clip, "track_id").filter(|&n| n > 0)?;
        let file = ctx.mobs.get(&file_mob)?;
        if ambiguous.contains(&file_mob) || int(file, "mob_type_id") != Some(MOB_SOURCE) {
            return None;
        }
        let import = picture_import(ctx, attrs, &self.budget);
        if import.locators.is_empty() || import.locators.iter().any(|path| {
            !path.rsplit_once('.').is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("png"))
        }) {
            return None;
        }
        Some(SourceImportedStill {
            id: copy_text(&self.budget, id),
            bin: self.bin,
            master,
            track: label,
            file_mob,
            file_track,
            rate: float(mob, "edit_rate").and_then(Rate::from_avid),
            master_length: int(mob, "length"),
            file_in: int(clip, "start_time"),
            source_length: int(clip, "length"),
            import_aspect: unique_attribute(attrs, "_IMPORTASPECT").and_then(Value::as_int),
            color_level: unique_attribute(attrs, "_COLORLEVEL").and_then(Value::as_int),
            import,
        })
    }
    pub(super) fn sequence_capture(&self, mob: &Node, item: usize) -> Capture {
        charge(
            &self.budget,
            size_of::<SequenceInfo>() + mob.get("name").and_then(Value::as_str).map_or(0, str::len),
        );
        Capture::new(
            formatted(
                &self.budget,
                format_args!("{}/bin/{}/item/{}", self.revision, self.bin, item),
            ),
            float(mob, "edit_rate").and_then(Rate::from_avid),
            self.budget.clone(),
        )
    }
}

fn saved_file_clock(
    ctx: &Ctx<'_>, file: &Node, native: &MediaTrack, file_track: i64,
    ambiguous: &HashSet<MobId>,
) -> Option<SavedFileClock> {
    let file_tracks = ctx.tracks(file);
    let [track] = file_tracks.as_slice() else { return None; };
    if int(track, "index") != Some(file_track) { return None; }
    let component = ctx.deref(track.get("component"))?;
    let wrapped = component.class == *b"SEQU";
    let clip = if wrapped {
        if native.kind != KIND_PICTURE || file_track != 1
            || ambiguous.contains(&native.file_mob)
            || float(file, "edit_rate") != Some(24.0)
            || int(file, "mob_type_id") != Some(MOB_SOURCE)
        {
            return None;
        }
        bracketed_24_component(ctx, component, KIND_PICTURE, *b"SCLP")?
    } else {
        component
    };
    if clip.class != *b"SCLP" || int(clip, "media_kind_id") != Some(native.kind)
        || int(clip, "length") != Some(native.clip_length)
    {
        return None;
    }
    let rate = float(clip, "edit_rate").and_then(Rate::from_avid)?;
    let package = mob_id(clip)?;
    if wrapped && (package == native.file_mob || ambiguous.contains(&package)
        || int(clip, "track_id") != Some(1))
    {
        return None;
    }
    let physical = ctx.mobs.get(&package)?;
    if int(physical, "mob_type_id") != Some(MOB_SOURCE)
        || float(physical, "edit_rate").and_then(Rate::from_avid) != Some(rate)
    {
        return None;
    }
    let mut tc = ctx.tracks(physical).into_iter()
        .filter(|t| int(t, "index") == Some(1))
        .filter_map(|t| ctx.deref(t.get("component")))
        .filter(|c| int(c, "media_kind_id") == Some(KIND_TIMECODE));
    let owner = tc.next()?;
    if tc.next().is_some() { return None; }
    let segment = if wrapped {
        bracketed_24_component(ctx, owner, KIND_TIMECODE, *b"TCCP")?
    } else {
        let segments = ctx.segments(owner);
        let mut segments = segments.into_iter().filter(|(_, s)| int(s, "length").is_some_and(|n| n > 0));
        let (0, segment) = segments.next()? else { return None; };
        if segments.next().is_some() { return None; }
        segment
    };
    if segment.class != *b"TCCP"
        || float(segment, "edit_rate").and_then(Rate::from_avid) != Some(rate)
    {
        return None;
    }
    let count = int(segment, "start")?;
    let offset = int(clip, "start_time")?;
    if count < 0 || offset < 0
        || offset.checked_add(native.clip_length)? > int(segment, "length")?
    {
        return None;
    }
    Some(SavedFileClock {
        package, rate, count, offset, base: int(segment, "fps")?,
        drop: int(segment, "flags")? & 1 != 0, file_track,
    })
}

// Avid's linked 24p file and physical TC1 tracks use explicit zero-length
// boundary fillers. Other children can change the origin or the binding.
fn bracketed_24_component<'a>(
    ctx: &Ctx<'a>, wrapper: &'a Node, kind: i64, class: [u8; 4],
) -> Option<&'a Node> {
    let matching = |node: &Node| {
        int(node, "media_kind_id") == Some(kind) && float(node, "edit_rate") == Some(24.0)
    };
    if wrapper.class != *b"SEQU" || !matching(wrapper) {
        return None;
    }
    let Value::RefList(children) = wrapper.get("components")? else { return None; };
    let [first, middle, last] = children.as_slice() else { return None; };
    for filler in [first, last] {
        let node = ctx.deref(Some(filler))?;
        if node.class != *b"FILL" || int(node, "length") != Some(0) || !matching(node) {
            return None;
        }
    }
    let node = ctx.deref(Some(middle))?;
    (node.class == class && matching(node) && int(node, "length")? > 0).then_some(node)
}

struct Scope {
    id: String,
    class: [u8; 4],
    from: usize,
    to: usize,
    rate: Option<Rate>,
    position_known: bool,
    retimed: bool,
    record_length: Option<i64>,
    children: usize,
    group_path: bool,
    group_warp_input: bool,
    audio_track_program: bool,
    freeze_color_input: bool,
}
struct MarkerFact {
    id: String,
    owner: String,
    kind: i64,
    track: i64,
    owner_kind: MarkerOwner,
    marker: AvidMarker,
    local: Observed<i64>,
    rate: Option<Rate>,
    record: Option<i64>,
}
struct TransitionFact {
    id: String,
    kind: i64,
    track: i64,
    position: Observed<i64>,
    length: Observed<i64>,
    cutpoint: Observed<i64>,
    rate: Option<Rate>,
    effect: Option<String>,
    audio_curve_attribute: Option<i64>,
    audio_curve_parameter: Option<i64>,
    outgoing: Option<String>,
    incoming: Option<String>,
}
struct QueryFact {
    op: usize,
    owner: String,
    rate: Option<Rate>,
    start: Option<i64>,
    label: Option<i64>,
    length: Option<i64>,
    position_known: bool,
    enclosing_span_known: bool,
    record_rate: Option<Rate>,
    retimed: bool,
    motion: Option<(i64, i64)>,
    motion_adapter_phase: Option<i64>,
    saved_timewarp: Option<Vec<i64>>,
    dry_gain_bypass: bool,
    equalizer: Option<Box<SourceEqualizer>>,
    audiosuite: Option<Box<SourceAudioSuiteRender>>,
    gain_dry_input: Option<Box<(MobId, i64)>>,
    constant_speed: Option<(i64, i64)>,
    nested_picture_motion: Option<Box<SourceSubclipMotion>>,
}
pub(super) fn automatic_adapter_attrs<'a>(ctx: &Ctx<'a>, node: &Node) -> Option<&'a [(String, Value)]> {
    match ctx.deref(node.get("attributes"))?.get("entries")? {
        Value::Attrs(a) if a.iter().any(|(k,v)| k == "_MIXMATCH_MOTIONADAPTER" && v.as_int() == Some(2)) => Some(a),
        _ => None,
    }
}

/// Admit only the saved progressive automatic adapter, not a creative speed or a graph.
pub(super) fn qualified_automatic_adapter<'a>(
    ctx: &Ctx<'a>, c: &'a Node, attrs: &[(String, Value)], kind: i64, length: i64,
) -> Option<(&'a Node, MobId, i64, i64, i64, i64, (i64, i64))> {
    let attr = |name: &str| {
        let mut values = attrs.iter().filter(|(k, _)| k == name);
        let (_, v) = values.next()?;
        values.next().is_none().then(|| v.as_int()).flatten()
    };
    let rate = |node| float(node, "edit_rate").and_then(Rate::from_avid);
    let absent = |name| matches!(c.get(name), None | Some(Value::Null));
    let phase = int(c, "phase_offset").filter(|p| *p >= 0)?;
    if !matches!(kind, KIND_PICTURE | KIND_SOUND)
        || c.get("effect_id").and_then(Value::as_str) != Some(if kind == KIND_PICTURE {
            "EFF_TIMEWARP.MOTION_CTL"
        } else { "EFF_TIMEWARP.AUDIO_TIME_WARP" })
        || int(c, "mc_mode") != Some(0)
        || !(2..=16_384).contains(&length)
        || attr("_MIXMATCH_MOTIONADAPTER") != Some(2) || attr("_MFX_INPUT_FORMAT") != Some(2)
        || attr("_MFX_OUTPUT_FORMAT") != Some(2) || attr("_SFX_RENDER") != Some(2)
        || attr("_SFX_IGNORE_RENDER_SETTING") != Some(1)
        || attrs.iter().any(|(k, _)| k == "_MFX_GRAPH_ENABLED")
        || !absent("source_param_list") || !absent("precomputed")
        || float(c, "offset_adjust") != Some(0.0)
        || c.get("new_source_calculation") != Some(&Value::Bool(false))
    { return None; }
    let params = ctx.deref(c.get("param_list"))?;
    let Value::RefList(params) = params.get("items")? else { return None; };
    let [param] = params.as_slice() else { return None; };
    let param = ctx.deref(Some(param))?;
    let Some(Value::Uuid(uuid)) = param.get("uuid") else { return None; };
    let curve = ctx.deref(param.get("control_track"))?;
    let Value::List(points) = curve.get("control_points")? else { return None; };
    let [Value::Node(point)] = points.as_slice() else { return None; };
    let Value::List(offset) = point.get("offset")? else { return None; };
    let [Value::Int(anchor), Value::Int(1)] = offset.as_slice() else { return None; };
    let anchor = anchor.checked_neg().filter(|n| *n >= 0)?;
    if uuid.to_string() != "8d568283-847e-11d5-935a-50f857c10000"
        || int(param, "value") != Some(0) || int(point, "value") != Some(0)
        || int(point, "timescale") != Some(1) || !matches!(point.get("pp"), Some(Value::List(v)) if v.is_empty())
    { return None; }
    let tracks = ctx.tracks(c);
    let [track] = tracks.as_slice() else { return None; };
    let component = ctx.deref(track.get("component"))?;
    if component.class != *b"SCLP" && component.class != *b"SEQU" { return None; }
    let parts = ctx.segments(component);
    let mut meaningful = parts.iter().filter(|(_, n)| !(n.class == *b"FILL" && int(n, "length") == Some(0)));
    let (0, clip) = *meaningful.next()? else { return None; };
    if clip.class != *b"SCLP" || meaningful.next().is_some() { return None; }
    let source_length = int(clip, "length")?;
    let source_in = int(clip, "start_time")?;
    let label = int(clip, "track_id").filter(|n| *n > 0)?;
    let ratio = Rate::automatic_adapter_span(rate(clip)?, rate(c)?)?;
    let qualified_range = match ratio {
        (1000, 1001) => anchor == phase && source_in == phase && source_length == (length * 1001 + 999) / 1000,
        (1001, 1000) if kind == KIND_SOUND && source_length == length + 1 => {
            // The saved key names the absolute 24 fps anchor. The saved phase
            // can wrap each 1001 record frames while the source trim stays absolute.
            (phase == anchor || phase == anchor % 1001)
                && i128::from(source_in) == i128::from(anchor) * 1000 / 1001
        }
        (1001, 1000) => anchor == phase && source_length == length
            && i128::from(source_in) == i128::from(phase) * 1000 / 1001,
        _ => false,
    };
    if !qualified_range || int(clip, "media_kind_id") != Some(kind)
        || !matches!(c.get("speed_ratio"), Some(Value::List(v))
            if v.as_slice() == [Value::Int(ratio.0), Value::Int(ratio.1)])
    {
        return None;
    }
    Some((clip, mob_id(clip)?, source_in, phase, source_length, label, ratio))
}
struct TextFact { scope: usize, text: SourceText }
pub(super) struct Capture {
    id: String,
    rate: Option<Rate>,
    budget: SharedBudget,
    scopes: Vec<Scope>,
    stack: Vec<usize>,
    roots: usize,
    markers: Vec<MarkerFact>,
    transitions: Vec<TransitionFact>,
    queries: Vec<QueryFact>,
    adapter_issues: Vec<SourceIssue>,
    texts: Vec<TextFact>,
    group_uses: Vec<groups::UseFact>,
    charged_ops: usize,
    primary_timecode_seen: bool,
    clock: Option<SequenceClock>,
    track_names: Vec<crate::source::SourceTrackName>,
    hd_24_still_canvas: bool,
}
impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture").field("id", &self.id).finish()
    }
}
impl Capture {
    pub(super) fn track_names(&mut self, ctx: &Ctx<'_>, tracks: &[&Node]) {
        // An unnamed duplicate slot can also make a named lane ambiguous.
        let mut lanes = std::collections::BTreeMap::new();
        for track in tracks {
            let Some(label @ 1..=99) = int(track, "index") else { continue };
            let Some(component) = ctx.deref(track.get("component")) else { continue };
            let Some(kind @ (KIND_PICTURE | KIND_SOUND)) = int(component, "media_kind_id") else { continue };
            let entry = lanes.entry((kind, label)).or_insert((None, 0usize, false, false));
            entry.1 += 1;
            if let Some(Value::Attrs(attrs)) = ctx.deref(track.get("attributes")).and_then(|node| node.get("entries")) {
                let mut names = attrs.iter().filter(|(key, _)| key == "_COMMENT");
                let first = names.next().and_then(|(_, value)| value.as_str());
                if names.next().is_some() {
                    entry.1 += 1;
                }
                if entry.0.is_none() {
                    entry.0 = first.filter(|name| !name.is_empty());
                    if entry.0.is_some() {
                        let mut formats = attrs.iter().filter(|(key, _)| key == "_TRACK_FORMAT");
                        let format = formats.next();
                        let unique_format = formats.next().is_none();
                        let mono = unique_format && format.is_some_and(|(_, value)| value.as_int() == Some(1));
                        entry.2 = kind == KIND_PICTURE || mono;
                        let supported = entry.2 || (unique_format && format.is_none());
                        let filler = |node: &Node| node.class == *b"FILL"
                            && int(node, "media_kind_id") == Some(kind)
                            && int(node, "length").is_some_and(|length| length >= 0);
                        entry.3 = supported && (filler(component) || (component.class == *b"SEQU"
                            && matches!(component.get("components"), Some(Value::RefList(children))
                                if children.iter().all(|child| ctx.deref(Some(child)).is_some_and(filler)))));
                    }
                }
            }
        }
        for ((kind, label), (name, count, plain_layout, plain_empty)) in lanes {
            let Some(name) = name else { continue };
            if !reserve(&self.budget, &mut self.track_names, 1) {
                return;
            }
            self.track_names.push(crate::source::SourceTrackName {
                kind, label,
                name: copy_text(&self.budget, name),
                ambiguous: count != 1,
                plain_layout,
                plain_empty,
            });
        }
    }

    pub(super) fn capture_pvol(
        &mut self, ctx: &Ctx<'_>, node: &Node, tracks: &[&Node], position: i64,
    ) -> (Option<super::volume::Capture<VolumePoint>>, Option<super::volume::Capture<PanPoint>>) {
        let volume = super::volume::capture_volume(ctx, node, tracks, position, |points| {
            charge(&self.budget, points * size_of::<VolumePoint>())
        });
        let pan = super::volume::capture_pan(ctx, node, tracks, position, |points| {
            charge(&self.budget, points * size_of::<PanPoint>())
        });
        (volume, pan)
    }
    fn new(id: String, rate: Option<Rate>, budget: SharedBudget) -> Self {
        charge(&budget, size_of::<Self>());
        Self {
            id,
            rate,
            budget,
            scopes: Vec::new(),
            stack: Vec::new(),
            roots: 0,
            markers: Vec::new(),
            transitions: Vec::new(),
            queries: Vec::new(),
            adapter_issues: Vec::new(),
            texts: Vec::new(),
            group_uses: Vec::new(),
            charged_ops: 0,
            primary_timecode_seen: false,
            clock: None,
            track_names: Vec::new(),
            hd_24_still_canvas: false,
        }
    }
    pub(super) fn reserve_track_format(&mut self, values: &mut Vec<(i64, i64)>) -> bool {
        reserve(&self.budget, values, 1)
    }
    pub(super) fn failed(&self) -> bool {
        self.budget.as_ref().borrow().error.is_some()
    }
    pub(super) fn fail(&self, reason: impl Into<String>) {
        self.budget.borrow_mut().fail(reason);
    }
    pub(super) fn next_owner(&self) -> String {
        match self.stack.last().map(|i| &self.scopes[*i]) {
            Some(s) => formatted(
                &self.budget,
                format_args!("{}/component/{}", s.id, s.children),
            ),
            None => formatted(
                &self.budget,
                format_args!("{}/component/{}", self.id, self.roots),
            ),
        }
    }
    pub(super) fn direct_sequence_effect(&self) -> bool {
        matches!(self.stack.as_slice(), [parent, effect]
            if self.scopes[*parent].class == *b"SEQU" && self.scopes[*effect].position_known)
    }
    pub(super) fn bind_audio_track_program(&mut self) -> bool {
        let [owner] = self.stack.as_slice() else { return false };
        let scope = &mut self.scopes[*owner];
        if !scope.position_known {
            return false;
        }
        scope.audio_track_program = true;
        true
    }
    pub(super) fn enter(
        &mut self,
        ctx: &Ctx<'_>,
        node: &Node,
        position: i64,
        kind: i64,
        track: i64,
        from: usize,
    ) {
        let id = self.next_owner();
        let parent = self.stack.last().map(|i| &self.scopes[*i]);
        let inherited = parent.map_or(self.rate, |s| s.rate);
        let rate = match node.get("edit_rate") {
            Some(_) => float(node, "edit_rate").and_then(Rate::from_avid),
            None => inherited,
        };
        let retimed = parent.is_some_and(|s| s.retimed) || node.class == *b"SPED";
        let known = parent.map_or(self.rate.is_some(), |s| s.position_known)
            && rate == self.rate
            && track > 0;
        let group_path = parent.is_none_or(|s| s.group_path
            && (s.class == *b"SEQU" || s.group_warp_input || s.audio_track_program));
        if let Some(i) = self.stack.last() {
            self.scopes[*i].children += 1;
        } else {
            self.roots += 1;
        }
        if !reserve(&self.budget, &mut self.scopes, 1) || !reserve(&self.budget, &mut self.stack, 1)
        {
            return;
        }
        let index = self.scopes.len();
        self.scopes.push(Scope {
            id,
            class: node.class,
            from,
            to: from,
            rate,
            position_known: known,
            retimed,
            record_length: int(node, "length"),
            children: 0,
            group_path,
            group_warp_input: false,
            audio_track_program: false,
            freeze_color_input: false,
        });
        self.stack.push(index);
        groups::capture_use(self, ctx, node, index, kind, track, position, rate);
        let id = &self.scopes[index].id;
        if kind == KIND_PICTURE {
            if let Some(text_kind) = super::text::kind(node) {
                if !reserve(&self.budget, &mut self.texts, 1) { return; }
                let mut native_properties = Vec::new();
                let mut native_properties_reason = None;
                let decoded = match super::text::body(ctx, node, text_kind) {
                    Ok(body) => {
                        // Reserve the parser's bounded output growth and stack before
                        // decoding; no database or decoded node escapes this scope.
                        if !charge(&self.budget, body.len().saturating_mul(2).saturating_add(2048)) { return; }
                        if super::text::is_legacy_titler(node) {
                            native_properties_reason = Some(copy_text(
                                &self.budget, "legacy Titler+ appearance is unqualified",
                            ));
                            super::text::decode_legacy_titler(body)
                        } else {
                            let kept = match text_kind {
                                TextKind::SubCap => copy_text_properties(
                                    &self.budget,
                                    super::text::subcap_properties(ctx, node),
                                    &mut native_properties,
                                    &mut native_properties_reason,
                                ),
                                TextKind::TitlerPlus => copy_text_properties(
                                    &self.budget,
                                    super::text::properties(body),
                                    &mut native_properties,
                                    &mut native_properties_reason,
                                ),
                            };
                            if !kept {
                                return;
                            }
                            super::text::decode(body, text_kind)
                        }
                    },
                    Err(reason) => Err(reason),
                };
                let (content, reason) = match decoded {
                    Ok(content) => (Some(content), None),
                    Err(reason) => (None, Some(copy_text(&self.budget, reason))),
                };
                self.texts.push(TextFact { scope: index, text: SourceText {
                    id: copy_text(&self.budget, id), kind: text_kind, track, content,
                    record_start: Some(position), length: int(node, "length"), rate,
                    reason,
                    native_properties,
                    native_properties_reason,
                }});
            }
        }
        let mut n = 0;
        each_marker(ctx, node, |native| {
            if !reserve(&self.budget, &mut self.markers, 1) {
                return;
            }
            let (marker, local) = read_marker(ctx, native, &self.budget);
            let record = match local {
                Observed::Known(offset) if !retimed && known && rate == self.rate => {
                    position.checked_add(offset)
                }
                _ => None,
            };
            let marker_id = formatted(&self.budget, format_args!("{id}/marker/{n}"));
            n += 1;
            let owner = copy_text(&self.budget, id);
            self.markers.push(MarkerFact {
                id: marker_id,
                owner,
                kind,
                track,
                owner_kind: if node.class == *b"FILL" {
                    MarkerOwner::Filler
                } else {
                    MarkerOwner::Edit
                },
                marker,
                local,
                rate,
                record,
            });
        });
    }
    pub(super) fn leave(&mut self, to: usize) {
        if let Some(i) = self.stack.pop() {
            self.scopes[i].to = to;
            if let Some(parent) = self.stack.last().copied() {
                if self.scopes[parent].class == *b"SEQU"
                    && self.scopes[i].rate != self.scopes[parent].rate
                {
                    self.scopes[parent].position_known = false;
                }
            }
        }
    }
    pub(super) fn invalidate_position(&mut self, reason: &str) {
        if let Some(i) = self.stack.last() {
            self.scopes[*i].position_known = false;
        }
        let _ = reason;
    }
    pub(super) fn sequence_markers(&mut self, ctx: &Ctx<'_>, mob: &Node) {
        self.hd_24_still_canvas = self.rate == Rate::new(24, 1)
            && source_color::hd_24_canvas(ctx, mob);
        let mut n = 0;
        each_marker(ctx, mob, |native| {
            if !reserve(&self.budget, &mut self.markers, 1) {
                return;
            }
            let (marker, local) = read_marker(ctx, native, &self.budget);
            let id = formatted(&self.budget, format_args!("{}/marker/{n}", self.id));
            n += 1;
            let owner = copy_text(&self.budget, &self.id);
            let record = match local {
                Observed::Known(offset) if self.rate.is_some() => Some(offset),
                _ => None,
            };
            self.markers.push(MarkerFact {
                id,
                owner,
                kind: 0,
                track: 0,
                owner_kind: MarkerOwner::Sequence,
                marker,
                local,
                rate: self.rate,
                record,
            });
        });
    }
    /// A direct, sequence-spanning TCCP can place its own markers in record
    /// time. Absolute start additionally requires an unambiguous, qualified TC1.
    pub(super) fn timecode_track(
        &mut self,
        ctx: &Ctx<'_>,
        native_track: &Node,
        component: &Node,
        track: i64,
        ordinal: usize,
        sequence_length: Option<i64>,
    ) {
        let rate = float(component, "edit_rate").and_then(Rate::from_avid);
        let length = int(component, "length");
        let qualified = component.class == *b"TCCP"
            && track > 0
            // A native Track can carry an explicit start_pos. This bounded
            // direct-root interpretation has no qualified nonzero-origin map.
            && matches!(native_track.get("start_pos"), None | Some(Value::Int(0)))
            && rate.is_some()
            && rate == self.rate
            && length.is_some_and(|n| n >= 0)
            && length == sequence_length;
        if track == 1 {
            if self.primary_timecode_seen {
                self.clock = None;
            } else if qualified {
                let profile = match (rate, int(component, "fps"), int(component, "flags")) {
                    (Some(rate), Some(24), Some(0))
                        if rate == Rate::new(24_000, 1001).unwrap()
                            || rate == Rate::new(24, 1).unwrap() =>
                    {
                        Some((rate, 24, false, 24 * 60 * 60 * 24))
                    }
                    (Some(rate), Some(25), Some(0)) if rate == Rate::new(25, 1).unwrap() => {
                        Some((rate, 25, false, 25 * 60 * 60 * 24))
                    }
                    (Some(rate), Some(30), Some(0)) if rate == Rate::new(30_000, 1001).unwrap() => {
                        Some((rate, 30, false, 30 * 60 * 60 * 24))
                    }
                    (Some(rate), Some(30), Some(1))
                        if rate == Rate::new(30_000, 1001).unwrap() =>
                    {
                        // Two labels are omitted in 54 minutes per hour.
                        Some((rate, 30, true, (30 * 60 * 60 - 2 * 54) * 24))
                    }
                    _ => None,
                };
                if let Some((rate, display_base, drop, day)) = profile {
                    self.clock = int(component, "start")
                        .filter(|count| (0..day).contains(count))
                        .map(|count| SequenceClock {
                            count: count as u64,
                            rate,
                            display_base,
                            drop,
                            track_ordinal: ordinal,
                        });
                }
            }
            self.primary_timecode_seen = true;
        }
        // Separate metadata namespace: collecting a marker must not renumber
        // existing A/V occurrences or choose among equal track labels.
        let mut n = 0;
        each_marker(ctx, component, |native| {
            if !reserve(&self.budget, &mut self.markers, 1) {
                return;
            }
            let (marker, local) = read_marker(ctx, native, &self.budget);
            let owner = formatted(&self.budget, format_args!("{}/metadata/{ordinal}", self.id));
            let id = formatted(&self.budget, format_args!("{owner}/marker/{n}"));
            n += 1;
            let record = match local {
                Observed::Known(offset)
                    if qualified && offset >= 0 && Some(offset) <= length => Some(offset),
                _ => None,
            };
            self.markers.push(MarkerFact {
                id,
                owner,
                kind: KIND_TIMECODE,
                track,
                owner_kind: MarkerOwner::Sequence,
                marker,
                local,
                rate,
                record,
            });
        });
    }
    pub(super) fn transition(
        &mut self,
        ctx: &Ctx<'_>,
        node: &Node,
        position: i64,
        kind: i64,
        track: i64,
        outgoing: Option<String>,
    ) -> usize {
        let scope = self.stack.last().map(|i| &self.scopes[*i]);
        let owner = scope.map_or(self.id.as_str(), |s| s.id.as_str());
        let id = formatted(
            &self.budget,
            format_args!("{owner}/transition/{}", self.transitions.len()),
        );
        let observed = |name: &str| {
            int(node, name).map(Observed::Known).unwrap_or_else(|| {
                Observed::Unresolved(formatted(
                    &self.budget,
                    format_args!("transition {name} is absent"),
                ))
            })
        };
        let rate = scope.and_then(|s| s.rate);
        let position = if scope.is_some_and(|s| s.position_known) {
            Observed::Known(position)
        } else {
            Observed::Unresolved(copy_text(
                &self.budget,
                "transition position follows unresolved native timing",
            ))
        };
        let effect = node
            .get("effect_id")
            .and_then(Value::as_str)
            .map(|s| copy_text(&self.budget, s));
        let (audio_curve_attribute, audio_curve_parameter) =
            if kind == KIND_SOUND && effect.as_deref() == Some("BLEND_AUDIO_DISSOLVE") {
                let mut attrs = borrowed_attrs(ctx, node)
                    .iter()
                    .filter(|(key, _)| key == "_ATN_AUDIO_DISSOLVE_CURVETYPE");
                let attribute = attrs.next().and_then(|(_, value)| value.as_int());
                let attribute = attribute.filter(|_| attrs.next().is_none());
                let parameter = ctx.deref(node.get("param_list")).and_then(|list| {
                    let Value::RefList(items) = list.get("items")? else {
                        return None;
                    };
                    let mut matches = items
                        .iter()
                        .filter_map(|item| ctx.deref(Some(item)))
                        .filter(|item| {
                            item.class == *b"PRIT"
                                && matches!(item.get("uuid"), Some(Value::Uuid(uuid))
                                    if uuid.to_string() == "8134fc48-3bd6-4904-9520-7a6360cc7127")
                        });
                    let value = int(matches.next()?, "value");
                    value.filter(|_| matches.next().is_none())
                });
                (attribute, parameter)
            } else {
                (None, None)
            };
        if !reserve(&self.budget, &mut self.transitions, 1) {
            return usize::MAX;
        }
        let index = self.transitions.len();
        self.transitions.push(TransitionFact {
            id,
            kind,
            track,
            position,
            length: observed("length"),
            cutpoint: observed("cutpoint"),
            rate,
            effect,
            audio_curve_attribute,
            audio_curve_parameter,
            outgoing,
            incoming: None,
        });
        index
    }
    pub(super) fn bind_incoming(&mut self, index: usize, owner: Option<String>) {
        if let Some(t) = self.transitions.get_mut(index) {
            t.incoming = owner;
        }
    }
    /// A verified Warp picture input keeps the enclosing record clock and full span.
    pub(super) fn bind_group_warp_input(&mut self, ctx: &Ctx<'_>, input: &Node) {
        let Some((&index, parents)) = self.stack.split_last() else { return };
        let scope = &self.scopes[index];
        let Some(length) = scope.record_length.filter(|&n| n > 0) else { return };
        if !scope.group_path || !scope.position_known || scope.retimed
            || parents.iter().any(|&i| self.scopes[i].group_warp_input)
            || float(input, "edit_rate").and_then(Rate::from_avid) != scope.rate
        {
            return;
        }
        let select = if input.class == *b"SEQU" {
            let Some(Value::RefList(children)) = input.get("components") else { return };
            let mut candidate = None;
            for child in children {
                let Some(node) = ctx.deref(Some(child)) else { return };
                if node.class == *b"FILL" && int(node, "length") == Some(0) {
                    continue;
                }
                if candidate.replace(node).is_some() { return; }
            }
            let Some(candidate) = candidate else { return };
            candidate
        } else {
            input
        };
        if select.class != *b"SLCT" || int(select, "length") != Some(length)
            || float(select, "edit_rate").and_then(Rate::from_avid) != scope.rate
        {
            return;
        }
        let tracks = ctx.tracks(select);
        if tracks.len() >= 2 && tracks.iter().all(|track| {
            ctx.deref(track.get("component")).is_some_and(|leaf| {
                leaf.class == *b"SCLP" && mob_id(leaf).is_some_and(|id| {
                    ctx.mobs.get(&id).is_some_and(|m| int(m, "usage_code") == Some(5))
                })
            })
        }) {
            self.scopes[index].group_warp_input = true;
        }
    }
    pub(super) fn query(&mut self, op: usize, node: &Node, motion: Option<(i64, i64)>, motion_adapter_phase: Option<i64>) {
        self.query_with_timewarp(op, node, motion, motion_adapter_phase, None, false);
    }
    pub(super) fn query_picture_subclip(
        &mut self, ctx: &Ctx<'_>, op: usize, node: &Node, ambiguous: &HashSet<MobId>,
    ) {
        let index = self.queries.len();
        self.query(op, node, None, None);
        let Some(rate) = self.rate else { return };
        if float(node, "edit_rate").and_then(Rate::from_avid) != Some(rate)
            || !(self.stack.len() == 1 || self.direct_sequence_effect())
        {
            return;
        }
        let Some(fact) = self.queries.get_mut(index) else { return };
        if fact.position_known && !fact.retimed {
            fact.nested_picture_motion = source_color::standalone_subclip_motion(ctx, node, rate, ambiguous)
                .filter(|_| charge(&self.budget, size_of::<SourceSubclipMotion>()))
                .map(Box::new);
        }
    }
    pub(super) fn query_constant_speed(
        &mut self, ctx: &Ctx<'_>, op: usize, effect: &Node, input: &Node, ratio: (i64, i64),
    ) {
        let index = self.queries.len();
        self.query(op, input, Some(ratio), None);
        if self.rate == Rate::new(24, 1)
            && self.stack.iter().rev().skip(1).all(|&i| self.scopes[i].class == *b"SEQU")
        {
            if let Some(query) = self.queries.get_mut(index) {
                query.constant_speed = timewarp::constant_speed(ctx, effect, input, ratio);
            }
        }
    }
    pub(super) fn bind_freeze_under_color(&mut self, ctx: &Ctx<'_>, effect: &Node, input: &Node, length: i64) {
        if !self.direct_sequence_effect() || self.rate != Rate::new(24, 1)
            || !matches!(effect.get("tracks"), Some(Value::List(tracks)) if tracks.len() == 1)
            || !matte_track(ctx, effect, 1).is_some_and(|node| std::ptr::eq(node, input))
            || !matches!(effect.get("precomputed"), None | Some(Value::Null))
            || source_color::saved_payload(ctx, effect).and_then(SourceColorRange::from_saved_xml)
                != Some(SourceColorRange::VideoToFull)
        {
            return;
        }
        if timewarp::single_component(ctx, input, effect).is_some_and(|node| {
            node.class == *b"SPED" && int(node, "length") == Some(length)
                && float(node, "edit_rate") == Some(24.0)
        }) {
            self.scopes[*self.stack.last().unwrap()].freeze_color_input = true;
        }
    }

    pub(super) fn query_rendered_freeze(&mut self, op: usize, render: &Node) -> bool {
        if self.rate != Rate::new(24, 1)
            || self.stack.iter().rev().skip(1).any(|&i| {
                let ancestor = &self.scopes[i];
                ancestor.class != *b"SEQU" && !ancestor.freeze_color_input
            })
        {
            return false;
        }
        self.query_with_timewarp(op, render, None, None, None, true);
        !self.failed()
    }
    pub(super) fn query_single_audio_effect_input(&mut self, op: usize, input: &Node) -> bool {
        if self.rate != Rate::new(24, 1)
            || self.stack.iter().rev().skip(1)
                .any(|&i| {
                    let scope = &self.scopes[i];
                    !matches!(&scope.class, b"SEQU" | b"PVOL") && !scope.audio_track_program
                })
        {
            return false;
        }
        self.query_with_timewarp(op, input, None, None, None, true);
        !self.failed()
    }
    pub(super) fn query_dry_gain(&mut self, op: usize, input: &Node) -> bool {
        if !self.query_single_audio_effect_input(op, input) {
            return false;
        }
        let Some(query) = self.queries.last_mut().filter(|q| q.op == op) else { return false };
        query.dry_gain_bypass = true;
        true
    }
    pub(super) fn query_equalizer(&mut self, op: usize, input: &Node, effect: SourceEqualizer) -> bool {
        let bytes = size_of::<SourceEqualizer>() + effect.filter_name.len()
            + effect.bands.len() * size_of::<SourceEqualizerBand>()
            + effect.pitch.as_ref().map_or(0, |_| size_of::<SourcePitchCache>());
        if !self.query_single_audio_effect_input(op, input) || !charge(&self.budget, bytes) {
            return false;
        }
        let Some(query) = self.queries.last_mut().filter(|q| q.op == op) else { return false };
        query.equalizer = Some(Box::new(effect));
        true
    }
    pub(super) fn query_audiosuite(&mut self, op: usize, input: &Node, render: SourceAudioSuiteRender) -> bool {
        let bytes = size_of::<SourceAudioSuiteRender>() + render.name.len() + render.chunk.len();
        if !self.query_single_audio_effect_input(op, input) || !charge(&self.budget, bytes) {
            return false;
        }
        let Some(query) = self.queries.last_mut().filter(|q| q.op == op) else { return false };
        query.audiosuite = Some(Box::new(render));
        true
    }
    pub(super) fn retain_gain_input(&mut self, op: usize, dry: (MobId, i64)) {
        if let Some(query) = self.queries.last_mut().filter(|q| q.op == op) {
            if charge(&self.budget, size_of::<(MobId, i64)>()) {
                query.gain_dry_input = Some(Box::new(dry));
            }
        }
    }
    fn query_with_timewarp(&mut self, op: usize, node: &Node, motion: Option<(i64, i64)>, motion_adapter_phase: Option<i64>, saved_timewarp: Option<Vec<i64>>, rendered: bool) {
        let Some(scope) = self.stack.last().map(|i| &self.scopes[*i]) else {
            return;
        };
        if !reserve(&self.budget, &mut self.queries, 1) {
            return;
        }
        let record_length = if motion.is_some() {
            scope.record_length
        } else {
            int(node, "length")
        };
        let enclosing_span_known = record_length.is_some()
            && self.stack.iter().all(|i| {
                let owner = &self.scopes[*i];
                owner.class == *b"SEQU" || owner.audio_track_program || owner.record_length == record_length
            });
        self.queries.push(QueryFact {
            op,
            owner: copy_text(&self.budget, &scope.id),
            rate: match node.get("edit_rate") {
                Some(_) => float(node, "edit_rate").and_then(Rate::from_avid),
                None => scope.rate,
            },
            start: int(node, "start_time"),
            label: int(node, "track_id"),
            length: int(node, "length"),
            position_known: scope.position_known,
            enclosing_span_known,
            record_rate: self.rate,
            retimed: scope.retimed && !rendered,
            motion,
            motion_adapter_phase,
            saved_timewarp,
            dry_gain_bypass: false,
            equalizer: None,
            audiosuite: None,
            gain_dry_input: None,
            constant_speed: None,
            nested_picture_motion: None,
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn note_disabled_clip(
        &mut self,
        ctx: &Ctx<'_>,
        selector: &Node,
        active_track: &Node,
        backing_track: &Node,
        record: i64,
        kind: i64,
        track: i64,
    ) {
        let (Some(active), Some(backing)) = (
            ctx.deref(active_track.get("component")),
            ctx.deref(backing_track.get("component")),
        ) else { return };
        let Some(scope) = self.stack.last().map(|&i| &self.scopes[i]) else {
            return;
        };
        let mut flags = borrowed_attrs(ctx, selector).iter()
            .filter(|(name, _)| name == "_DISABLE_CLIP_FLAG");
        let Some(length) = scope.record_length.filter(|&n| n > 0) else { return };
        if !matches!(flags.next(), Some((_, Value::Int(1))))
            || flags.next().is_some()
            || int(selector, "selected") != Some(0)
            || active.class != *b"FILL"
            || !matches!(&backing.class, b"SCLP" | b"TKFX")
            || !matches!(kind, KIND_PICTURE | KIND_SOUND)
            || !scope.group_path
            || !scope.position_known
            || [selector, active, backing].iter().any(|node| {
                int(node, "length") != Some(length)
                    || int(node, "media_kind_id") != Some(kind)
                    || float(node, "edit_rate").and_then(Rate::from_avid) != self.rate
            })
        {
            return;
        }
        let mut route = Vec::new();
        if let (Some(mob), Some(label)) = (mob_id(backing), int(backing, "track_id")) {
            if !reserve(&self.budget, &mut route, 1) {
                return;
            }
            route.push(witness(mob, label, self.rate, int(backing, "start_time"), Some(length), &self.budget));
        }
        let absent = |node: &Node, key| matches!(node.get(key), None | Some(Value::Null));
        let plain = |node: &Node| {
            ["param_list", "precomputed", "session_attrs", "right_bob"]
                .into_iter().all(|key| absent(node, key))
        };
        let neutral_selector = borrowed_attrs(ctx, selector).len() == 1
            && [selector, active, backing].iter().all(|node| plain(node))
            && absent(selector, "left_bob") && absent(active, "left_bob")
            && absent(active, "attributes");
        let plain_audio = neutral_selector && kind == KIND_SOUND
            && absent(backing, "left_bob") && absent(backing, "attributes");
        let direct = self.stack.iter().rev().skip(1).all(|&i| self.scopes[i].class == *b"SEQU");
        let plain_mono = plain_audio && backing.class == *b"SCLP";
        let picture_package = || {
            if !neutral_selector || !direct || kind != KIND_PICTURE || track != 1
                || self.rate != Rate::new(24, 1) || backing.class != *b"SCLP"
                || int(backing, "track_id") != Some(1) || !absent(backing, "effect_id")
                || int(active_track, "lock_number") != Some(0)
                || int(backing_track, "lock_number") != Some(0)
                || !absent(backing_track, "filler_proxy")
                || !matches!(borrowed_attrs(ctx, backing), [(name, Value::Str(xml))]
                    if name == "_IMAGE_BOUNDS_OVERRIDE" && matte::equal_1600_picture_bounds(xml))
            {
                return None;
            }
            let proxy = ctx.deref(active_track.get("filler_proxy"))?;
            if proxy.class != *b"TRKR"
                || int(proxy, "media_kind_id") != Some(KIND_PICTURE)
                || float(proxy, "edit_rate").and_then(Rate::from_avid) != self.rate
                || int(proxy, "relative_scope") != Some(1) || int(proxy, "relative_track") != Some(-1)
                || !plain(proxy) || !absent(proxy, "left_bob") || !absent(proxy, "attributes")
            {
                return None;
            }
            // DIDP is a saved witness for this file and trim. Its byte offsets
            // are unnecessary because the destination links the verified file.
            let cache = ctx.deref(backing.get("left_bob"))?;
            let start = int(backing, "start_time").filter(|&value| value >= 0)?;
            if cache.class != *b"DIDP" || int(cache, "sample_num") != Some(start)
                || int(cache, "length") != Some(length)
                || int(cache, "track_type") != Some(KIND_PICTURE) || int(cache, "track_index") != Some(1)
                || int(cache, "strip") != Some(0) || cache.get("spos_invalid") != Some(&Value::Bool(false))
            {
                return None;
            }
            mob_id(cache)
        };
        let stereo_sources = || {
            if !plain_audio
                || backing.class != *b"TKFX"
                || self.rate != Rate::new(24, 1)
                || !direct
                || backing.get("effect_id").and_then(Value::as_str) != Some("EFF2_AUDIO_CHANNEL_COMBINER")
                || !["keyframes", "trackman"].into_iter().all(|key| absent(backing, key))
                || ["mc_mode", "num_scalars", "info_current", "info_smooth", "info_is_reversed"]
                    .into_iter().any(|key| int(backing, key) != Some(0))
            {
                return None;
            }
            let inputs = ctx.tracks(backing);
            let [left, right] = inputs.as_slice() else { return None };
            let source = |input: &Node, channel| {
                let node = ctx.deref(input.get("component"))?;
                if int(input, "index") != Some(channel)
                    || int(input, "lock_number") != Some(0)
                    || node.class != *b"SCLP"
                    || int(node, "track_id") != Some(channel)
                    || int(node, "media_kind_id") != Some(KIND_SOUND)
                    || int(node, "length") != Some(length)
                    || float(node, "edit_rate").and_then(Rate::from_avid) != self.rate
                    || !plain(node) || !absent(node, "left_bob") || !absent(node, "attributes") || !absent(node, "effect_id")
                {
                    return None;
                }
                Some((mob_id(node)?, int(node, "start_time").filter(|&start| start >= 0)?))
            };
            let (first_mob, first_start) = source(left, 1)?;
            let (second_mob, second_start) = source(right, 2)?;
            (first_mob == second_mob && first_start == second_start).then_some((first_mob, first_start))
        };
        let plain_stereo = if let Some((mob, start)) = stereo_sources() {
            if !reserve(&self.budget, &mut route, 2) {
                return;
            }
            route.extend([1, 2].map(|channel| witness(mob, channel, self.rate, Some(start), Some(length), &self.budget)));
            true
        } else {
            false
        };
        add_classified_issue(
            &IssueContext {
                owner: &scope.id,
                record,
                position_known: true,
                rate: self.rate,
                kind,
                track,
            },
            SourceIssueKind::DisabledClip { length, backing_class: backing.class, plain_mono, plain_stereo, neutral_picture: picture_package() },
            "Disabled Avid clip kept as a gap. The inactive clip cannot be re-enabled in the converted timeline.",
            route,
            &self.budget,
            &mut self.adapter_issues,
        );
    }

    pub(super) fn layered_submaster(&mut self, ctx: &Ctx<'_>, effect: &Node, tracks: &[&Node], record: i64, track: i64) -> bool {
        const MAX_INPUTS: usize = 32;
        let Some(scope) = self.stack.last().map(|i| &self.scopes[*i]) else {
            return false;
        };
        let direct = self.stack.iter().rev().skip(1).all(|&i| self.scopes[i].class == *b"SEQU");
        let position_known = direct && scope.position_known && !scope.retimed;
        let length = scope.record_length.filter(|len| position_known && *len > 0);
        let input_count = tracks.len();
        let mut inputs = Vec::new();
        if !reserve(&self.budget, &mut inputs, input_count.min(MAX_INPUTS)) {
            return false;
        }
        for input in tracks.iter().take(MAX_INPUTS) {
            let component = ctx.deref(input.get("component"));
            let segment = component.and_then(|node| {
                if node.class == *b"SEQU" { sole_picture_segment(ctx, node) } else { Some(node) }
            });
            let segment_effect = segment.and_then(|node| node.get("effect_id").and_then(Value::as_str))
                .filter(|id| !id.is_empty() && id.len() <= 64)
                .map(|id| copy_text(&self.budget, id));
            inputs.push(SubmasterInput {
                track: int(input, "index"),
                component_class: component.map(|node| node.class),
                segment_class: segment.map(|node| node.class),
                segment_effect,
                picture_source: component.and_then(|node| submaster::picture_source(ctx, effect, node)),
            });
        }
        let plain_layers = self.rate.is_some_and(|rate| {
            (Some(rate) == Rate::new(24, 1) || Some(rate) == Rate::new(24_000, 1001))
                && length.is_some_and(|length| plain_layered_submaster(ctx, effect, tracks, &inputs, length, rate))
        });
        let index = self.adapter_issues.len();
        add_classified_issue(
            &IssueContext {
                owner: &scope.id, record, position_known, rate: self.rate,
                kind: KIND_PICTURE, track,
            },
            SourceIssueKind::LayeredSubmaster { length, input_count },
            "Avid layered Submaster omitted. Its picture inputs need an editable composite.",
            Vec::new(),
            &self.budget,
            &mut self.adapter_issues,
        );
        if let Some(issue) = self.adapter_issues.get_mut(index) {
            issue.submaster_inputs = inputs;
            if plain_layers {
                issue.picture_profile = Some(SourcePictureProfile::PlainLayeredSubmaster);
            }
        }
        plain_layers
    }
    pub(super) fn unsupported_picture_effect<'a>(
        &mut self, ctx: &Ctx<'a>, node: &'a Node, record: i64, kind: i64, track: i64, effect: &str,
        ambiguous: &HashSet<MobId>,
    ) {
        let Some(scope) = self.stack.last().map(|i| &self.scopes[*i]) else {
            return;
        };
        let direct = self.stack.iter().rev().skip(1).all(|&i| self.scopes[i].class == *b"SEQU");
        let position_known = direct && scope.position_known && !scope.retimed;
        let length = scope.record_length.filter(|len| position_known && *len > 0);
        let (issue, reason) = match effect {
            "EFF2_2DMATTE" => (
                SourceIssueKind::MatteKey { length },
                "Avid Matte Key (EFF2_2DMATTE) omitted; fill and alpha compositing are not converted",
            ),
            "EFF2_LUTSFX" => (
                SourceIssueKind::ColorAdapter { length },
                "Avid Color Adapter (EFF2_LUTSFX) omitted. Check source color on this edit in Premiere.",
            ),
            _ => return,
        };
        let index = self.adapter_issues.len();
        add_classified_issue(
            &IssueContext {
                owner: &scope.id,
                record,
                position_known,
                rate: self.rate,
                kind,
                track,
            },
            issue,
            reason,
            Vec::new(),
            &self.budget,
            &mut self.adapter_issues,
        );
        if effect == "EFF2_2DMATTE" {
            if let Some(issue) = self.adapter_issues.get_mut(index) {
                if !charge(&self.budget, size_of::<MatteInputs>()) {
                    return;
                }
                let inputs = matte_inputs(ctx, node);
                issue.matte_import = matte_import(ctx, &inputs, ambiguous, &self.budget);
                issue.matte_inputs = Some(Box::new(inputs));
                if let (Some(1), Some(import)) = (length, &issue.matte_import) {
                    if self.rate == Rate::new(24, 1)
                        && matte::imported_png(ctx, node, &inputs, import)
                    {
                        issue.picture_profile = Some(SourcePictureProfile::ImportedPngMatte);
                    }
                }
                if let (Some(span), Some(matte)) = (length, inputs.matte) {
                    if self.rate == Some(matte.rate) && matte.length == span {
                        self.capture_matte_input_markers(ctx, node, record, track, span);
                    }
                }
            }
        } else if let (Some(span), Some(issue)) = (length, self.adapter_issues.get_mut(index)) {
            if matte::empty_color_adapter(ctx, node).is_some() {
                issue.picture_profile = Some(SourcePictureProfile::EmptyColorAdapter);
            } else if self.rate == Rate::new(24, 1) {
                issue.color_adapter = source_color::capture_adapter(
                    ctx, node, span, ambiguous, &self.budget, self.hd_24_still_canvas,
                )
                .filter(|_| charge(&self.budget, size_of::<SourceColorAdapter>()))
                .map(Box::new);
                if issue.color_adapter.as_ref().is_some_and(|adapter| adapter.centered_still_fit) {
                    issue.reason = copy_text(&self.budget,
                        "Avid still-image color conversion remains unresolved, including its effect on the border levels. Check this edit's color in Premiere.");
                } else if issue.color_adapter.as_ref().is_some_and(|adapter| adapter.subclip_motion.is_some()) {
                    issue.reason = copy_text(&self.budget,
                        "Avid Color Adapter and retimed subclip need a verified picture cadence. The saved motion controls are retained.");
                } else if issue.color_adapter.as_ref().is_some_and(|adapter| {
                    matches!(&adapter.picture_file, Observed::Unresolved(reason)
                        if reason == source_color::MANAGED_INPUT_TRANSFORM_CONFLICT)
                }) {
                    issue.reason = copy_text(&self.budget,
                        "Avid Color Adapter omitted because the picture clip saves a different color transform.");
                }
            }
        }
    }
    // A marked matte leaf can share the composite's record clock even
    // when the composite cannot emit a portable placement.
    fn capture_matte_input_markers(
        &mut self, ctx: &Ctx<'_>, effect: &Node, record: i64, track: i64, span: i64,
    ) {
        let Some(input) = matte_track(ctx, effect, 3) else { return };
        if input.get("edit_rate").is_some()
            && float(input, "edit_rate").and_then(Rate::from_avid) != self.rate
        {
            return;
        }
        let Some(leaf) = (match &input.class {
            b"SCLP" => Some(input),
            b"SEQU" => sole_picture_segment(ctx, input),
            _ => None,
        }) else { return };
        // SEQU has no stored length. The sole nonempty leaf establishes it.
        if leaf.class != *b"SCLP" || int(leaf, "length") != Some(span) {
            return;
        }
        let scope = &self.scopes[*self.stack.last().unwrap()];
        let owner = formatted(&self.budget, format_args!("{}/matte-input", scope.id));
        let op_index = scope.from;
        let mut ordinal = 0;
        each_marker(ctx, leaf, |native| {
            if !reserve(&self.budget, &mut self.markers, 1) {
                return;
            }
            let (marker, local) = read_marker(ctx, native, &self.budget);
            let record = match local {
                Observed::Known(offset) if (0..span).contains(&offset) => record.checked_add(offset),
                _ => None,
            };
            let id = formatted(&self.budget, format_args!("{owner}/marker/{ordinal}"));
            ordinal += 1;
            self.markers.push(MarkerFact {
                id, owner: copy_text(&self.budget, &owner), kind: KIND_PICTURE, track,
                owner_kind: MarkerOwner::Edit, marker, local, rate: self.rate, record,
            });
        });
        // A scope with no ops keeps the occurrence independent of overlapping edits.
        if ordinal > 0 && reserve(&self.budget, &mut self.scopes, 1) {
            self.scopes.push(Scope {
                id: owner, class: *b"SCLP", from: op_index, to: op_index,
                rate: self.rate, position_known: true, retimed: false,
                record_length: Some(span), children: 0, group_path: false, group_warp_input: false,
                audio_track_program: false, freeze_color_input: false,
            });
        }
    }
    pub(super) fn unresolved_motion(&mut self, record: i64, kind: i64, track: i64, reason: &'static str) {
        self.add_motion_issue(record, kind, track, SourceIssueKind::RouteTiming, reason);
    }

    pub(super) fn unresolved_effect(
        &mut self, node: &Node, record: i64, kind: i64, track: i64,
        effect: &'static str, reason: &'static str,
    ) {
        let timing = self.stack.last().and_then(|&i| {
            let scope = &self.scopes[i];
            let rate = self.rate?;
            let length = scope.record_length.filter(|&len| len > 0)?;
            // The track program and SEQU supply the accumulated position.
            // An enclosing Pan Volume can own the same whole interval.
            (scope.position_known && record >= 0 && record.checked_add(length).is_some()
                && track > 0 && matches!(kind, KIND_PICTURE | KIND_SOUND)
                && float(node, "edit_rate").and_then(Rate::from_avid) == Some(rate)
                && self.stack.iter().rev().skip(1).all(|&i| {
                    let parent = &self.scopes[i];
                    parent.class == *b"SEQU" || (kind == KIND_SOUND && !parent.retimed
                        && parent.rate == Some(rate)
                        && (parent.audio_track_program
                            || parent.class == *b"PVOL" && parent.record_length == Some(length)))
                }))
                .then_some(length)
        });
        let issue = timing.map_or(SourceIssueKind::RouteTiming, |length|
            SourceIssueKind::UntranslatedEffect { effect, length });
        self.add_motion_issue(record, kind, track, issue, reason);
    }

    fn add_motion_issue(
        &mut self, record: i64, kind: i64, track: i64, issue: SourceIssueKind, reason: &'static str,
    ) {
        let Some(scope) = self.stack.last().map(|i| &self.scopes[*i]) else {
            return;
        };
        add_classified_issue(
            &IssueContext {
                owner: &scope.id,
                record,
                position_known: scope.position_known,
                rate: self.rate,
                kind,
                track,
            },
            issue,
            reason,
            Vec::new(),
            &self.budget,
            &mut self.adapter_issues,
        );
    }
    pub(super) fn resize_render_probe(&self) -> Option<usize> {
        self.direct_sequence_effect().then_some(self.adapter_issues.len())
    }

    pub(super) fn bind_resize_render(
        &mut self, ctx: &Ctx<'_>, effect: &Node, length: i64, first_issue: usize,
        ambiguous: &HashSet<MobId>,
    ) {
        let Some(rate) = self.rate else { return };
        let [issue] = &mut self.adapter_issues[first_issue..] else { return };
        if !matches!(issue.issue_kind, SourceIssueKind::Timewarp { length: span } if span == length)
            || issue.record_rate != Some(rate) || !issue.route.is_empty()
        {
            return;
        }
        let Some(controls) = issue.timewarp.as_mut() else { return };
        let Some(render) = timewarp::precompute(ctx, effect, length, rate, ambiguous) else { return };
        if !reserve(&self.budget, &mut issue.route, render.len()) { return; }
        issue.route.extend(render.into_iter().map(|(id, start, span)|
            witness(id, 1, Some(rate), Some(start), Some(span), &self.budget)));
        let resize = &self.scopes[*self.stack.last().unwrap()];
        controls.resize_render_owner = Some(copy_text(&self.budget, &resize.id));
    }

    pub(super) fn unresolved_timewarp(
        &mut self, ctx: &Ctx<'_>, effect: &Node, record: i64, kind: i64, track: i64,
        reason: &'static str, ambiguous: &HashSet<MobId>,
    ) {
        let Some(scope) = self.stack.last().map(|i| &self.scopes[*i]) else { return };
        let timing = self.rate.zip(scope.record_length).filter(|(rate, length)| {
            scope.position_known && record >= 0 && track > 0 && kind == KIND_PICTURE
                && *length > 0 && float(effect, "edit_rate").and_then(Rate::from_avid) == Some(*rate)
                // A failed check inside a qualified map shape can indicate
                // contradictory timing data. Keep the existing refusal there.
                && !matches!((rate.num, rate.den, *length), (24, 1, 48) | (24_000, 1001, 1666))
                && self.stack.iter().rev().skip(1).all(|&i| {
                    let parent = &self.scopes[i];
                    parent.class == *b"SEQU"
                        || (!parent.retimed && parent.rate == Some(*rate)
                            && parent.record_length == Some(*length))
                })
        });
        let controls = timewarp::observe(ctx, effect, &self.budget);
        let previous = self.adapter_issues.len();
        let Some((rate, length)) = timing else {
            self.unresolved_motion(record, kind, track, reason);
            if let Some(issue) = self.adapter_issues.get_mut(previous) {
                issue.timewarp = controls;
            }
            return;
        };
        let mut route = Vec::new();
        if self.stack.iter().rev().skip(1).all(|&i| self.scopes[i].class == *b"SEQU") {
            if let Some(precompute) = timewarp::precompute(ctx, effect, length, rate, ambiguous) {
                if !reserve(&self.budget, &mut route, precompute.len()) { return; }
                route.extend(precompute.into_iter().map(|(id, start, span)|
                    witness(id, 1, Some(rate), Some(start), Some(span), &self.budget)));
            }
        }
        add_classified_issue(
            &IssueContext { owner: &scope.id, record, position_known: true, rate: Some(rate), kind, track },
            SourceIssueKind::Timewarp { length }, reason, route, &self.budget, &mut self.adapter_issues,
        );
        if let Some(issue) = self.adapter_issues.get_mut(previous) {
            issue.timewarp = controls;
        }
    }
    pub(super) fn query_sampled_timewarp(
        &mut self, ctx: &Ctx<'_>, op: usize, effect: &Node, input: &Node,
        ratio: (i64, i64), record: i64, kind: i64, track: i64, sparse: bool,
        ambiguous: &HashSet<MobId>,
    ) -> bool {
        let rate = if sparse { Rate::new(24_000, 1001) } else { Rate::new(24, 1) };
        let direct = kind == KIND_PICTURE && self.rate == rate
            && self.stack.iter().rev().skip(1).all(|&i| self.scopes[i].class == *b"SEQU")
            && timewarp::master_picture_matches(ctx, input);
        let mut owned = Vec::new();
        let mut retain = |frames: &[i64]| {
            if !reserve(&self.budget, &mut owned, frames.len()) { return false; }
            owned.extend_from_slice(frames);
            true
        };
        let qualified = direct && if sparse {
            timewarp::visible_sparse_frames(ctx, effect, input, ratio)
                .is_some_and(|frames| retain(&frames))
        } else if let Some(frames) = timewarp::visible_linear_ramp(ctx, effect, input, ratio, &self.budget) {
            owned = frames;
            true
        } else {
            timewarp::visible_frames(ctx, effect, input, ratio)
                .is_some_and(|frames| frames.is_none_or(|frames| retain(&frames)))
        };
        if !qualified {
            let reason = if sparse {
                "Avid Timewarp uses an unsupported Position curve; constant speed would change its cadence"
            } else {
                "Avid Timewarp has no supported saved picture map; a constant speed would change its cadence"
            };
            self.unresolved_timewarp(ctx, effect, record, kind, track, reason, ambiguous);
            return false;
        }
        let map = (!owned.is_empty()).then_some(owned);
        self.query_with_timewarp(op, input, Some(ratio), None, map, false);
        !self.failed()
    }
    fn op_cost(&mut self, op: &Op) -> usize {
        let mut cost = 0;
        match op {
            Op::Source(q) | Op::Motion { query: q, .. } => cost = strings(&q.via),
            Op::Problem { message, .. } => cost = message.capacity(),
            Op::Color { value, .. } => {
                let key = Arc::as_ptr(value) as usize;
                if first_shared(&self.budget, key) {
                    cost = color_cost(value);
                }
            }
            Op::Picture { value, .. } => {
                let key = Arc::as_ptr(value) as usize;
                if first_shared(&self.budget, key) {
                    cost = picture_cost(value);
                }
            }
            Op::PictureChain { chain, .. } => {
                // Static stages share picture payloads already charged by Op::Picture.
                cost = chain.stages.capacity() * size_of::<PictureStage>();
            }
            Op::Matte { value, .. } => {
                let key = Arc::as_ptr(value) as usize;
                if first_shared(&self.budget, key) {
                    cost = matte_cost(value);
                }
            }
            _ => {}
        }
        cost
    }
    pub(super) fn prepare_op(&mut self, op: &Op, ops: &mut Vec<Op>) -> bool {
        let cost = self.op_cost(op);
        if !charge(&self.budget, cost) || !reserve(&self.budget, ops, 1) {
            return false;
        }
        self.charged_ops = ops.len() + 1;
        true
    }
    pub(super) fn account_ops(&mut self, ops: &[Op]) {
        // Synthetic compatibility programs may bypass CompileWalk::push_op. The
        // production source path is fully charged before retaining each operation.
        for op in &ops[self.charged_ops..] {
            let cost = self.op_cost(op);
            if !charge(&self.budget, size_of::<Op>() + cost) {
                break;
            }
        }
        self.charged_ops = ops.len();
    }
}

struct Piece {
    master: MobId,
    label: i64,
    record: i64,
    source: i64,
    length: i64,
    rate: Rate,
    route: Vec<RouteWitness>,
    subclip_mix: SubclipMix,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SubclipMix {
    Exclude,
    CenterPanCount(u8),
    ExplicitGain(SourceSubclipGain),
}
struct IssueContext<'a> {
    owner: &'a str,
    record: i64,
    position_known: bool,
    rate: Option<Rate>,
    kind: i64,
    track: i64,
}
fn clone_observed(value: &Observed<i64>, budget: &SharedBudget) -> Observed<i64> {
    match value {
        Observed::Known(n) => Observed::Known(*n),
        Observed::Unresolved(s) => Observed::Unresolved(copy_text(budget, s)),
    }
}
fn clone_route(route: &[RouteWitness], budget: &SharedBudget) -> Vec<RouteWitness> {
    let mut out = Vec::new();
    // Most witnesses hold one hop. Reserve those slots without rounding up
    // every per-edit route to four elements.
    if !reserve_with_floor(budget, &mut out, route.len(), 1) {
        return out;
    }
    for r in route {
        out.push(RouteWitness {
            mob: r.mob,
            track: r.track,
            rate: r.rate,
            source_offset: clone_observed(&r.source_offset, budget),
            length: clone_observed(&r.length, budget),
        });
    }
    out
}
fn witness(
    mob: MobId,
    label: i64,
    rate: Option<Rate>,
    start: Option<i64>,
    length: Option<i64>,
    budget: &SharedBudget,
) -> RouteWitness {
    RouteWitness {
        mob,
        track: label,
        rate,
        source_offset: known(start, budget, "source start is absent"),
        length: known(length, budget, "source length is absent"),
    }
}
fn add_issue(
    context: &IssueContext<'_>,
    reason: &str,
    route: Vec<RouteWitness>,
    budget: &SharedBudget,
    issues: &mut Vec<SourceIssue>,
) {
    add_classified_issue(context, SourceIssueKind::Route, reason, route, budget, issues);
}
fn add_rate_issue(
    context: &IssueContext<'_>,
    reason: &str,
    route: Vec<RouteWitness>,
    budget: &SharedBudget,
    issues: &mut Vec<SourceIssue>,
) {
    add_classified_issue(context, SourceIssueKind::RouteTiming, reason, route, budget, issues);
}
fn add_classified_issue(
    context: &IssueContext<'_>,
    issue_kind: SourceIssueKind,
    reason: &str,
    route: Vec<RouteWitness>,
    budget: &SharedBudget,
    issues: &mut Vec<SourceIssue>,
) {
    if !reserve(budget, issues, 1) {
        return;
    }
    issues.push(SourceIssue {
        owner: copy_text(budget, context.owner),
        reason: copy_text(budget, reason),
        issue_kind,
        route,
        record_start: if context.position_known {
            Observed::Known(context.record)
        } else {
            Observed::Unresolved(copy_text(budget, "record position is unresolved"))
        },
        record_rate: context.rate,
        kind: context.kind,
        track: context.track,
        matte_inputs: None,
        matte_import: None,
        color_adapter: None,
        submaster_inputs: Vec::new(),
        picture_profile: None,
        timewarp: None,
        nested_picture_motion: None,
    });
}

fn matte_inputs<'a>(ctx: &Ctx<'a>, effect: &'a Node) -> MatteInputs {
    let mut inputs = MatteInputs {
        background: MatteBackground::Unresolved,
        fill: None,
        matte: None,
    };
    let Some(Value::List(tracks)) = effect.get("tracks") else { return inputs };
    if tracks.len() != 3 {
        return inputs;
    }
    let mut saved = [None; 3];
    for track in tracks {
        let Value::Node(track) = track else { return inputs };
        let Some(index @ 1..=3) = int(track, "index") else { return inputs };
        let slot = &mut saved[index as usize - 1];
        if slot.replace(track).is_some() {
            return inputs;
        }
    }
    let picture = |i: usize| ctx.deref(saved[i]?.get("component"))
        .filter(|node| int(node, "media_kind_id") == Some(KIND_PICTURE));
    inputs.background = match picture(0) {
        Some(node) if ctx.is_empty(node) => MatteBackground::LowerTrack,
        Some(_) => MatteBackground::Explicit,
        None => MatteBackground::Unresolved,
    };
    inputs.fill = picture(1).and_then(|node| matte_source(ctx, node));
    inputs.matte = picture(2).and_then(|node| matte_source(ctx, node));
    inputs
}

fn matte_track<'a>(ctx: &Ctx<'a>, effect: &Node, track: i64) -> Option<&'a Node> {
    let Value::List(tracks) = effect.get("tracks")? else { return None };
    let mut matched = tracks.iter().filter_map(|entry| match entry {
        Value::Node(node) if int(node, "index") == Some(track) => Some(node),
        _ => None,
    });
    let native = matched.next()?;
    if matched.next().is_some() {
        return None;
    }
    ctx.deref(native.get("component")).filter(|node| int(node, "media_kind_id") == Some(KIND_PICTURE))
}

fn sole_picture_segment<'a>(ctx: &Ctx<'a>, node: &Node) -> Option<&'a Node> {
    let Value::RefList(children) = node.get("components")? else { return None };
    let mut sole = None;
    for reference in children {
        let component = ctx.deref(Some(reference))?;
        if component.class == *b"FILL" && int(component, "length") == Some(0) {
            continue;
        }
        if sole.replace(component).is_some() {
            return None;
        }
    }
    sole
}

fn plain_layered_submaster(
    ctx: &Ctx<'_>, effect: &Node, tracks: &[&Node], inputs: &[SubmasterInput], length: i64,
    rate: Rate,
) -> bool {
    let [background, lower, upper] = inputs else { return false };
    let (Some(lower_picture), Some(upper_picture)) = (lower.picture_source, upper.picture_source) else {
        return false;
    };
    let node_rate = |node: &Node| float(node, "edit_rate").and_then(Rate::from_avid);
    if !submaster::neutral_controls(ctx, effect, length)
        || node_rate(effect) != Some(rate)
        || [background.track, lower.track, upper.track] != [Some(1), Some(2), Some(3)]
        || lower_picture.track != 1 || upper_picture.track != 1
        || lower_picture.local_start != 0 || lower_picture.length != length
    {
        return false;
    }
    let no_controls = |node: &Node| ["effect_id", "left_bob", "right_bob", "session_attrs", "precomputed", "param_list"]
        .iter().all(|key| matches!(node.get(key), None | Some(Value::Null | Value::Ref(0))));
    let no_attrs = |node: &Node| matches!(node.get("attributes"), None | Some(Value::Null | Value::Ref(0)));
    // The two verified DNx sources are opaque 16:9 pictures. This saved
    // override centers the same full-frame bounds in every rectangle.
    const BOUNDS: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?><Bounds>",
        "  <Framing>-1600/1 -900/1 3200/1 1800/1</Framing>",
        "  <Valid>-1600/1 -900/1 3200/1 1800/1</Valid>",
        "  <Essence>-1600/1 -900/1 3200/1 1800/1</Essence>",
        "  <Source>-1600/1 -900/1 3200/1 1800/1</Source></Bounds>",
    );
    tracks.iter().enumerate().all(|(index, track)| {
        if int(track, "lock_number") != Some(0) {
            return false;
        }
        let Some(proxy) = ctx.deref(track.get("filler_proxy")) else { return false };
        if proxy.class != *b"TRKR" || int(proxy, "media_kind_id") != Some(KIND_PICTURE)
            || int(proxy, "relative_scope") != Some(i64::from(index == 0))
            || int(proxy, "relative_track") != Some(-1) || int(proxy, "length") != Some(i64::from(i32::MAX))
            || !no_controls(proxy) || !no_attrs(proxy)
        {
            return false;
        }
        let Some(sequence) = ctx.deref(track.get("component")) else { return false };
        if sequence.class != *b"SEQU" || node_rate(sequence) != Some(rate)
            || int(sequence, "media_kind_id") != Some(KIND_PICTURE)
            || !no_controls(sequence) || !no_attrs(sequence)
        {
            return false;
        }
        let Some(Value::RefList(children)) = sequence.get("components") else { return false };
        let fill = |reference: &Value| ctx.deref(Some(reference)).filter(|node| {
            node.class == *b"FILL" && node_rate(node) == Some(rate)
                && int(node, "media_kind_id") == Some(KIND_PICTURE) && no_controls(node) && no_attrs(node)
        });
        if index == 0 {
            return matches!(children.as_slice(), [only] if fill(only).and_then(|n| int(n, "length")) == Some(length));
        }
        let [lead, clip, tail] = children.as_slice() else { return false };
        let Some(clip) = ctx.deref(Some(clip)) else { return false };
        let attrs = borrowed_attrs(ctx, clip);
        fill(lead).is_some() && fill(tail).is_some() && clip.class == *b"SCLP" && no_controls(clip)
            && attrs.len() == 2
            && unique_attribute(attrs, "_IMAGE_BOUNDS_OVERRIDE").and_then(Value::as_str) == Some(BOUNDS)
            && unique_attribute(attrs, "_BOUNDS_REFORMATTING_OPTION").and_then(Value::as_int) == Some(1)
    })
}

fn matte_import(
    ctx: &Ctx<'_>, inputs: &MatteInputs, ambiguous: &HashSet<MobId>, budget: &SharedBudget,
) -> Option<MatteImport> {
    let (fill, matte) = (inputs.fill?, inputs.matte?);
    if fill.mob != matte.mob || fill.track == matte.track || ambiguous.contains(&fill.mob) {
        return None;
    }
    let master = ctx.mobs.get(&fill.mob)?;
    if int(master, "mob_type_id") != Some(MOB_MASTER) {
        return None;
    }
    Some(picture_import(ctx, import_settings(ctx, master)?, budget))
}

fn unique_attribute<'a>(attrs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    let mut matches = attrs.iter().filter(|(name, _)| name == key);
    let (_, value) = matches.next()?;
    matches.next().is_none().then_some(value)
}

fn import_settings<'a>(ctx: &Ctx<'a>, master: &Node) -> Option<&'a [(String, Value)]> {
    let settings = ctx.deref(unique_attribute(borrowed_attrs(ctx, master), "_IMPORTSETTING"))
        .filter(|node| node.class == *b"ATTR")?;
    let Value::Attrs(attrs) = settings.get("entries")? else { return None };
    Some(attrs)
}

fn picture_import(ctx: &Ctx<'_>, attrs: &[(String, Value)], budget: &SharedBudget) -> MatteImport {
    let flag = |key| match unique_attribute(attrs, key).and_then(Value::as_int) {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    };
    let ignore_alpha = flag("_IGNOREALPHA");
    let invert_alpha = flag("_INVERTALPHA");
    let mut locators = Vec::new();
    if let Some(locator) = ctx.deref(unique_attribute(attrs, "_SRCFILE")).filter(|node| node.class == *b"FILE") {
        const MAX_LOCATOR_BYTES: usize = 4096;
        for key in ["path_posix", "path_utf8", "path"] {
            if let Some(path) = locator.get(key).and_then(Value::as_str)
                .filter(|path| !path.is_empty() && path.len() <= MAX_LOCATOR_BYTES)
            {
                if !locators.iter().any(|seen| seen == path) && reserve(budget, &mut locators, 1) {
                    locators.push(copy_text(budget, path));
                }
            }
        }
    }
    MatteImport { ignore_alpha, invert_alpha, locators }
}

fn matte_source<'a>(ctx: &Ctx<'a>, mut node: &'a Node) -> Option<MatteInput> {
    for _ in 0..8 {
        if int(node, "media_kind_id") != Some(KIND_PICTURE) {
            return None;
        }
        match &node.class {
            b"SCLP" => {
                let mob = mob_id(node)?;
                if mob.material.data1 == 0 && mob.label == [0; 12] {
                    return None;
                }
                let track = int(node, "track_id").filter(|n| *n > 0)?;
                let start = int(node, "start_time")?;
                let length = int(node, "length").filter(|n| *n > 0)?;
                let rate = Rate::from_avid(float(node, "edit_rate")?)?;
                return Some(MatteInput { mob, track, start, length, rate });
            }
            b"SEQU" => {
                node = sole_picture_segment(ctx, node)?;
            }
            b"TKFX" if node.get("effect_id").and_then(Value::as_str) == Some("EFF2_LUTSFX") => {
                let Some(Value::List(tracks)) = node.get("tracks") else { return None };
                let [Value::Node(track)] = tracks.as_slice() else { return None };
                node = ctx.deref(track.get("component"))?;
            }
            _ => return None,
        }
    }
    None
}
fn qualified_query(
    routes: &HashMap<MobId, MobRoute>,
    q: &Query,
    fact: &QueryFact,
    budget: &SharedBudget,
) -> (Vec<Piece>, Vec<SourceIssue>) {
    let mut pieces = Vec::new();
    let mut issues = Vec::new();
    let mut route = Vec::new();
    if !reserve_with_floor(budget, &mut route, 1, 1) {
        return (pieces, issues);
    }
    route.push(witness(
        q.id,
        q.label,
        fact.rate,
        fact.start,
        fact.length,
        budget,
    ));
    let context = IssueContext {
        owner: &fact.owner,
        record: q.record,
        position_known: fact.position_known,
        rate: fact.record_rate,
        kind: q.kind,
        track: q.track,
    };
    if fact.motion_adapter_phase.is_some() {
        let source_rate = fact.rate;
        if source_rate.zip(fact.record_rate)
            .and_then(|(source, record)| Rate::automatic_adapter_span(source, record)).is_none()
            || !fact.position_known || !fact.enclosing_span_known
            || !fact.label.is_some_and(|n| n > 0)
            || !routes.get(&q.id).is_some_and(|r| r.kind == Some(MOB_MASTER) && r.rate == source_rate)
        {
            add_rate_issue(&context, "Avid Motion Adapter does not resolve to a qualified direct master at its saved source rate", route, budget, &mut issues);
        } else if reserve(budget, &mut pieces, 1) {
            pieces.push(Piece { master: q.id, label: q.label, record: q.record,
                source: q.offset, length: q.length, rate: source_rate.unwrap(), route,
                subclip_mix: SubclipMix::Exclude });
        }
        return (pieces, issues);
    }
    let reason = if fact.start.is_none() {
        Some("source start is absent")
    } else if fact.label.is_none() {
        Some("source track label is absent")
    } else if fact.label.is_some_and(|n| n <= 0) {
        Some("source track label is not positive")
    } else if fact.length.is_none() {
        Some("source length is absent")
    } else if fact.rate.is_some() && fact.record_rate.is_some() && fact.rate != fact.record_rate {
        add_rate_issue(
            &context,
            "source component rate differs from its record owner; conversion is unqualified",
            route,
            budget,
            &mut issues,
        );
        return (pieces, issues);
    } else if !fact.enclosing_span_known {
        Some("enclosing component span is unresolved or differs from its source input")
    } else if !fact.position_known {
        Some("record position follows unresolved native timing")
    } else if fact.rate.is_none() {
        Some("source owner rate is unresolved")
    } else if fact.retimed && fact.motion.is_none() {
        Some("retimed route range is unresolved")
    } else {
        None
    };
    if let Some(reason) = reason {
        add_issue(&context, reason, route, budget, &mut issues);
    } else {
        qualified_resolve(
            routes,
            q.id,
            q.label,
            q.offset,
            q.length,
            q.record,
            q.kind,
            fact.rate.unwrap(),
            q.depth,
            route,
            if fact.motion.is_none() { SubclipMix::CenterPanCount(0) } else { SubclipMix::Exclude },
            &context,
            budget,
            &mut pieces,
            &mut issues,
        );
    }
    (pieces, issues)
}
#[allow(clippy::too_many_arguments)]
fn qualified_resolve(
    routes: &HashMap<MobId, MobRoute>,
    id: MobId,
    label: i64,
    offset: i64,
    length: i64,
    record: i64,
    kind: i64,
    rate: Rate,
    depth: usize,
    route: Vec<RouteWitness>,
    mix: SubclipMix,
    context: &IssueContext<'_>,
    budget: &SharedBudget,
    pieces: &mut Vec<Piece>,
    issues: &mut Vec<SourceIssue>,
) {
    if budget.as_ref().borrow().error.is_some() {
        return;
    }
    if depth > MAX_NESTING {
        return add_issue(context, "source chain is too deep", route, budget, issues);
    }
    if offset < 0 || length <= 0 || record < 0 {
        return add_issue(
            context,
            "source range is negative or empty",
            route,
            budget,
            issues,
        );
    }
    let Some(end) = offset.checked_add(length) else {
        return add_issue(context, "source range overflows", route, budget, issues);
    };
    let Some(mob) = routes.get(&id) else {
        return add_issue(
            context,
            "source mob is not in the selected bins",
            route,
            budget,
            issues,
        );
    };
    if mob.rate != Some(rate) {
        let mut route = route;
        if reserve_with_floor(budget, &mut route, 1, 1) {
            route.push(witness(
                id,
                label,
                mob.rate,
                Some(offset),
                Some(length),
                budget,
            ));
        }
        return add_rate_issue(
            context,
            "mixed or unknown route-owner rates are not qualified",
            route,
            budget,
            issues,
        );
    }
    match mob.kind {
        Some(MOB_MASTER) => {
            if kind == KIND_SOUND && mob.refused_pcm_prefix {
                return add_rate_issue(
                    &IssueContext { record, ..*context },
                    "PCM and MPEG audio master has unqualified saved ranges or channel bindings",
                    route, budget, issues,
                );
            }
            if matches!(mix, SubclipMix::ExplicitGain(_))
                && !mob.length.is_some_and(|length| end <= length)
            {
                return add_issue(
                    context, "saved subclip gain extends beyond the known master range",
                    route, budget, issues,
                );
            }
            if reserve(budget, pieces, 1) {
                pieces.push(Piece {
                    master: id,
                    label,
                    record,
                    source: offset,
                    length,
                    rate,
                    route,
                    subclip_mix: mix,
                });
            }
        }
        Some(MOB_COMPOSITION) => {
            let mut track = None;
            let mut count = 0;
            for t in &mob.tracks {
                if t.label == Some(label) && t.kind == Some(kind) {
                    track = Some(t);
                    count += 1;
                }
            }
            if count != 1 {
                return add_issue(
                    context,
                    "source composition track is absent or ambiguous",
                    route,
                    budget,
                    issues,
                );
            }
            let track = track.unwrap();
            if track.rate != Some(rate) {
                return add_rate_issue(
                    context,
                    "source composition track rate differs or is unknown",
                    route,
                    budget,
                    issues,
                );
            }
            if !track.range_qualified {
                return add_issue(
                    context,
                    "source composition units or ranges are unresolved",
                    route,
                    budget,
                    issues,
                );
            }
            // A nested span must stay inside one saved Pan Volume component.
            // Never use the general composition splitter to recover only part of it.
            if matches!(mix, SubclipMix::CenterPanCount(1..))
                && !track.segments.iter().any(|segment| {
                    matches!(segment.action.as_ref(), RouteAction::SubclipPanVolume { gain: None, .. })
                        && segment.position <= offset
                        && segment.observed_length
                            .and_then(|length| segment.position.checked_add(length))
                            .is_some_and(|segment_end| end <= segment_end)
                })
            {
                return add_issue(
                    context,
                    "nested audio subclip range is not covered by one qualified Pan Volume input",
                    route,
                    budget,
                    issues,
                );
            }
            for seg in track.segments.iter() {
                if budget.as_ref().borrow().error.is_some() {
                    return;
                }
                let Some(seg_length) = seg.observed_length else {
                    add_issue(
                        context,
                        "source composition segment length is absent",
                        clone_route(&route, budget),
                        budget,
                        issues,
                    );
                    continue;
                };
                let Some(seg_end) = seg.position.checked_add(seg_length) else {
                    add_issue(
                        context,
                        "source composition range overflows",
                        clone_route(&route, budget),
                        budget,
                        issues,
                    );
                    continue;
                };
                let lo = offset.max(seg.position);
                let hi = end.min(seg_end);
                if lo >= hi {
                    continue;
                }
                let Some(at) = record.checked_add(lo - offset) else {
                    add_issue(
                        context,
                        "resolved record position overflows",
                        clone_route(&route, budget),
                        budget,
                        issues,
                    );
                    continue;
                };
                let (action, mix) = match seg.action.as_ref() {
                    RouteAction::Selected {
                        inner: Some(inner), ..
                    } => (inner.as_ref(), mix),
                    RouteAction::SubclipPanVolume { inner, gain }
                        if kind == KIND_SOUND && mob.usage == Some(2) => match (mix, gain) {
                        (SubclipMix::CenterPanCount(count), None) =>
                            (inner.as_ref(), SubclipMix::CenterPanCount(count + 1)),
                        (SubclipMix::CenterPanCount(0), Some(gain)) if lo == offset && hi == end =>
                            (inner.as_ref(), SubclipMix::ExplicitGain(*gain)),
                        _ => (seg.action.as_ref(), mix),
                    },
                    other => (other, mix),
                };
                match action {
                    RouteAction::Empty => {}
                    RouteAction::Source {
                        id: Some(inner),
                        observed_label: Some(inner_label),
                        observed_start: Some(start),
                        rate: component_rate,
                        rate_present,
                        ..
                    } => {
                        if matches!(mix, SubclipMix::ExplicitGain(_))
                            && !routes.get(inner).is_some_and(|mob| mob.kind == Some(MOB_MASTER))
                        {
                            add_issue(context, "saved subclip gain has no direct audio master",
                                clone_route(&route, budget), budget, issues);
                            continue;
                        }
                        if matches!(mix, SubclipMix::CenterPanCount(1..)) && !routes.get(inner).is_some_and(|mob| {
                            mob.kind == Some(MOB_MASTER) || (mob.kind == Some(MOB_COMPOSITION) && mob.usage == Some(2))
                        }) {
                            add_issue(context, "subclip Pan Volume input is neither an audio master nor an audio subclip",
                                clone_route(&route, budget), budget, issues);
                            continue;
                        }
                        let component_rate = if *rate_present {
                            *component_rate
                        } else {
                            track.rate
                        };
                        let mut next = clone_route(&route, budget);
                        if !reserve_with_floor(budget, &mut next, 1, 1) {
                            return;
                        }
                        next.push(witness(
                            *inner,
                            *inner_label,
                            component_rate,
                            Some(*start),
                            Some(seg_length),
                            budget,
                        ));
                        if component_rate != Some(rate) {
                            add_rate_issue(
                                context,
                                "mixed source component rate is unqualified",
                                next,
                                budget,
                                issues,
                            );
                            continue;
                        }
                        let Some(source) = start.checked_add(lo - seg.position) else {
                            add_issue(
                                context,
                                "resolved source offset overflows",
                                next,
                                budget,
                                issues,
                            );
                            continue;
                        };
                        qualified_resolve(
                            routes,
                            *inner,
                            *inner_label,
                            source,
                            hi - lo,
                            at,
                            kind,
                            rate,
                            depth + 1,
                            next,
                            mix,
                            context,
                            budget,
                            pieces,
                            issues,
                        );
                    }
                    RouteAction::Source { .. } => add_issue(
                        context,
                        "source component start, label or mob is absent",
                        clone_route(&route, budget),
                        budget,
                        issues,
                    ),
                    _ => add_issue(
                        context,
                        "source composition branch is not qualified",
                        clone_route(&route, budget),
                        budget,
                        issues,
                    ),
                }
            }
        }
        _ => add_issue(
            context,
            "source route does not resolve to a master",
            route,
            budget,
            issues,
        ),
    }
}

fn binding(
    owner: Option<&str>,
    scopes: &[Scope],
    starts: &[usize],
    events: &[SourceEvent],
    issues: &[SourceIssue],
    budget: &SharedBudget,
) -> Binding {
    let unresolved = |reason: &str| Binding {
        placements: vec![],
        reason: Some(copy_text(budget, reason)),
    };
    let Some(owner) = owner else {
        return unresolved("native adjacent owner is absent");
    };
    let Some(scope) = scopes.iter().find(|s| s.id == owner) else {
        return unresolved("native owner scope is unresolved");
    };
    binding_scope(scope, starts, events, issues, budget)
}
fn binding_scope(scope: &Scope, starts: &[usize], events: &[SourceEvent], issues: &[SourceIssue], budget: &SharedBudget) -> Binding {
    let unresolved = |reason: &str| Binding { placements: vec![], reason: Some(copy_text(budget, reason)) };
    let from = starts.get(scope.from).copied().unwrap_or(events.len());
    let to = starts.get(scope.to).copied().unwrap_or(events.len());
    let selected = events.get(from..to).unwrap_or(&[]);
    let mut placements = Vec::new();
    if !reserve(budget, &mut placements, selected.len()) {
        return unresolved("source fact budget exhausted");
    }
    for e in selected {
        placements.push(copy_text(budget, &e.id));
    }
    let reason = if issues.iter().any(|i| {
        !matches!(i.issue_kind, SourceIssueKind::DisabledClip { .. })
            && (i.owner == scope.id
                || i.owner.strip_prefix(&scope.id).is_some_and(|rest| rest.starts_with('/')))
    }) {
        Some(copy_text(
            budget,
            "native owner includes an unresolved source branch",
        ))
    } else if selected.is_empty() {
        Some(copy_text(
            budget,
            "native owner produced no source placements",
        ))
    } else {
        None
    };
    Binding { placements, reason }
}
fn copy_via(via: &[String], budget: &SharedBudget) -> Vec<String> {
    let mut out = Vec::new();
    if !reserve(budget, &mut out, via.len()) {
        return out;
    }
    for value in via {
        out.push(copy_text(budget, value));
    }
    out
}
fn base_event(q: &Query, p: &Piece, budget: &SharedBudget) -> SeqEvent {
    SeqEvent {
        kind: q.kind,
        track: q.track,
        record_start: p.record,
        length: p.length,
        master: p.master,
        master_track: p.label,
        source_in: p.source,
        via: copy_via(&q.via, budget),
        speed: None,
        source_length: p.length,
        combined: None,
        edit_gain_db: None,
        edit_pan: None,
        color: None,
        picture: None,
        picture_nested: false,
        animatte: None,
    }
}
fn finish_source(
    mut pending: PendingSequence,
    routes: &HashMap<MobId, MobRoute>,
) -> SourceSequence {
    let mut capture = pending.source.take().expect("source capture enabled");
    capture.account_ops(&pending.ops);
    // Every native channel emits a Combined operation even when its input did
    // not produce a qualified event. Preserve this denominator independently.
    let mut combiner_channels = Vec::new();
    for op in &pending.ops {
        if let Op::Combined { group, channel, .. } = op {
            let (Some(groups), Some(channels)) = (group.checked_add(1), channel.checked_add(1))
            else {
                capture
                    .budget
                    .borrow_mut()
                    .fail("source combiner membership overflow");
                break;
            };
            if groups > combiner_channels.len() {
                let additional = groups - combiner_channels.len();
                if !reserve(&capture.budget, &mut combiner_channels, additional) {
                    break;
                }
                combiner_channels.resize(groups, 0usize);
            }
            combiner_channels[*group] = combiner_channels[*group].max(channels);
        }
    }
    let mut events = Vec::new();
    let mut starts = Vec::new();
    let mut unresolved = std::mem::take(&mut capture.adapter_issues);
    let mut info = pending.info;
    // The source path does not run the legacy flattened resolver. Every emitted
    // range below was independently resolved from observed route/rate witnesses.
    for (op_index, op) in pending.ops.into_iter().enumerate() {
        if !reserve(&capture.budget, &mut starts, 1) {
            break;
        }
        starts.push(events.len());
        match op {
            Op::Source(q) | Op::Motion { query: q, .. } => {
                let Some(fact) = capture.queries.iter_mut().find(|f| f.op == op_index) else {
                    continue;
                };
                let (pieces, mut issues) = qualified_query(routes, &q, fact, &capture.budget);
                if let (true, [issue]) = (pieces.is_empty(), issues.as_mut_slice()) {
                    if issue.reason == "source composition branch is not qualified" && issue.route.len() == 1 {
                        issue.nested_picture_motion = fact.nested_picture_motion.take();
                    }
                }
                if !reserve(&capture.budget, &mut unresolved, issues.len()) {
                    break;
                }
                unresolved.extend(issues);
                // Motion spans are qualified facts, not a rendered sampling claim.
                let motion = fact.motion;
                let audio_effect = fact.dry_gain_bypass || fact.equalizer.is_some() || fact.audiosuite.is_some();
                if (motion.is_some() || audio_effect)
                    && (pieces.len() != 1 || unresolved.iter().any(|i| i.owner == fact.owner))
                {
                    let context = IssueContext {
                        owner: &fact.owner,
                        record: q.record,
                        position_known: fact.position_known,
                        rate: fact.record_rate,
                        kind: q.kind,
                        track: q.track,
                    };
                    let report = if fact.saved_timewarp.is_some() || audio_effect { add_rate_issue } else { add_issue };
                    let reason = if fact.dry_gain_bypass {
                        "AudioSuite Gain dry input does not resolve to one qualified range"
                    } else if fact.equalizer.is_some() {
                        "Avid legacy EQ dry input does not resolve to one qualified range"
                    } else if fact.audiosuite.is_some() {
                        "AudioSuite render does not resolve to one qualified range"
                    } else { "motion source does not have one qualified range" };
                    report(
                        &context,
                        reason,
                        Vec::new(),
                        &capture.budget,
                        &mut unresolved,
                    );
                    continue;
                }
                for (piece_index, piece) in pieces.into_iter().enumerate() {
                    if !reserve(&capture.budget, &mut events, 1) {
                        break;
                    }
                    let id = formatted(
                        &capture.budget,
                        format_args!("{}/event/{piece_index}", fact.owner),
                    );
                    let owner = copy_text(&capture.budget, &fact.owner);
                    let mut event = base_event(&q, &piece, &capture.budget);
                    if let Some(ratio) = motion {
                        let Some(scope) = capture.scopes.iter().find(|s| s.id == fact.owner) else {
                            continue;
                        };
                        // Stored record length is read directly on the motion owner.
                        let length = scope.record_length;
                        let Some(length) = length.filter(|n| *n > 0) else {
                            let context = IssueContext {
                                owner: &fact.owner,
                                record: q.record,
                                position_known: fact.position_known,
                                rate: fact.record_rate,
                                kind: q.kind,
                                track: q.track,
                            };
                            add_issue(
                                &context,
                                "motion record length is absent or invalid",
                                piece.route,
                                &capture.budget,
                                &mut unresolved,
                            );
                            continue;
                        };
                        event.length = length;
                        event.speed = fact.motion_adapter_phase.is_none().then_some(ratio);
                        event.source_length = piece.length;
                    }
                    let range = QualifiedRange {
                        record_start: piece.record,
                        length: event.length,
                        record_rate: capture.rate.unwrap(),
                        source_in: piece.source,
                        source_length: piece.length,
                        source_rate: piece.rate,
                        route: piece.route,
                    };
                    events.push(SourceEvent {
                        id,
                        owner,
                        event,
                        range: Some(range),
                        reason: None,
                        volume_automation: None,
                        pan_automation: None,
                        subclip_center_pan_count: match piece.subclip_mix {
                            SubclipMix::CenterPanCount(count) => count,
                            _ => 0,
                        },
                        subclip_gain: match piece.subclip_mix {
                            SubclipMix::ExplicitGain(gain) => Some(gain),
                            _ => None,
                        },
                        picture_extension: None,
                        picture_chain: Vec::new(),
                        motion_adapter_phase: fact.motion_adapter_phase,
                        motion_adapter_center_pan: false,
                        saved_timewarp: fact.saved_timewarp.take(),
                        dry_gain_bypass: fact.dry_gain_bypass,
                        equalizer: fact.equalizer.take(),
                        audiosuite: fact.audiosuite.take(),
                        gain_dry_input: fact.gain_dry_input.take(),
                        constant_speed: fact.constant_speed,
                        plain_submaster: false,
                    });
                }
            }
            Op::Problem {
                position,
                kind,
                track,
                message,
            } => {
                if reserve(&capture.budget, &mut info.problems, 1) {
                    info.problems.push(formatted(
                        &capture.budget,
                        format_args!(
                            "{} at frame {}: {}",
                            kind_label(kind, track),
                            position,
                            message
                        ),
                    ));
                }
            }
            Op::PanVolume { before, gain, pans, saved_center_pan, mut automation, mut pan_automation } => {
                let produced = &mut events[starts[before]..];
                let single = produced.len() == 1;
                let has_pan = pan_automation.is_some();
                for e in produced {
                    if single && e.motion_adapter_phase.is_some() {
                        e.motion_adapter_center_pan = saved_center_pan;
                    }
                    e.event.edit_gain_db = gain;
                    e.event.edit_pan = if matches!(e.event.combined, Some((_, 1))) {
                        pans[1]
                    } else {
                        pans[0]
                    };
                    if e.volume_automation.is_some() {
                        e.volume_automation = Some(VolumeAutomation::Unsupported(
                            "nested PVOL automation",
                        ));
                    } else if let Some(captured) = automation.take() {
                        e.volume_automation = Some(captured.resolve(
                            single, e, "volume owner is not one exact same-rate mono edit",
                        ));
                    }
                    if e.pan_automation.is_some() {
                        e.pan_automation = Some(PanAutomation::Unsupported("nested PVOL pan automation"));
                    } else if has_pan && !single {
                        e.pan_automation = Some(PanAutomation::Unsupported(
                            "pan owner is not one exact same-rate mono edit",
                        ));
                    } else if let Some(captured) = pan_automation.take() {
                        e.pan_automation = Some(captured.resolve(
                            single, e, "pan owner is not one exact same-rate mono edit",
                        ));
                    }
                }
            }
            Op::Color { before, value } => {
                for e in &mut events[starts[before]..] {
                    e.picture_chain.clear();
                    if !charge(&capture.budget, color_cost(&value)) {
                        break;
                    }
                    e.event.color = Some((*value).clone());
                }
            }
            Op::Picture { before, value } => {
                for e in &mut events[starts[before]..] {
                    e.picture_chain.clear();
                    if e.event.picture.is_some() {
                        e.event.picture_nested = true;
                    } else {
                        if !charge(&capture.budget, picture_cost(&value)) {
                            break;
                        }
                        e.event.picture = Some((*value).clone());
                    }
                }
            }
            Op::PictureExtension { before, capture: picture } => {
                let produced = &mut events[starts[before]..];
                let single = produced.len() == 1;
                for event in produced {
                    event.picture_chain.clear();
                    let extension = picture.resolve(single, event);
                    if let Some(saved) = event.picture_extension.as_deref_mut() {
                        *saved = extension;
                    } else if charge(&capture.budget, size_of::<PictureExtension>()) {
                        event.picture_extension = Some(Box::new(extension));
                    } else {
                        break;
                    }
                }
            }
            Op::PictureChain { before, chain } => {
                let produced = &mut events[starts[before]..];
                if let [event] = produced {
                    if event.event.via.is_empty() && event.event.speed.is_none()
                        && event.motion_adapter_phase.is_none() && event.saved_timewarp.is_none()
                        && event.range.as_ref().is_some_and(|range| {
                            range.record_start == chain.position && range.length == chain.length
                                && range.record_rate == chain.rate && range.source_rate == chain.rate
                                && range.source_length == chain.length
                        })
                    {
                        event.picture_chain = chain.stages;
                        continue;
                    }
                }
                if chain.stages.iter().any(|stage| matches!(stage, PictureStage::SpectraMatte { .. })) {
                    for event in produced {
                        event.range = None;
                        event.reason = Some(copy_text(&capture.budget, "SpectraMatte foreground does not bind to one exact same-rate edit"));
                    }
                }
            }
            Op::PlainSubmaster { before, length, position } => {
                let [event] = &mut events[starts[before]..] else { continue };
                event.plain_submaster = event.range.as_ref().is_some_and(|r| {
                    r.record_start == position && r.length == length && r.source_length == length
                        && r.record_rate == r.source_rate && r.route.len() == 1
                });
            }
            Op::Matte {
                before,
                length,
                value,
            } => {
                let produced = events.len() - starts[before];
                for e in &mut events[starts[before]..] {
                    e.picture_chain.clear();
                    if produced == 1 && e.event.length == length && e.event.animatte.is_none() {
                        if !charge(&capture.budget, matte_cost(&value)) {
                            break;
                        }
                        e.event.animatte = Some((*value).clone());
                    } else if reserve(&capture.budget, &mut e.event.via, 1) {
                        e.event.via.push(copy_text(
                            &capture.budget,
                            "AniMatte over more than one edit not converted",
                        ));
                    }
                }
            }
            Op::Combined {
                before,
                group,
                channel,
            } => {
                for e in &mut events[starts[before]..] {
                    e.event.combined = Some((group, channel));
                }
            }
        }
        if capture.failed() {
            break;
        }
    }
    if reserve(&capture.budget, &mut starts, 1) {
        starts.push(events.len());
    }
    let mut transitions = Vec::new();
    let has_gain_gaps = unresolved.iter().any(|issue| matches!(issue.issue_kind,
        SourceIssueKind::UntranslatedEffect { effect: "AudioSuite Gain", .. }));
    for t in capture.transitions {
        if !reserve(&capture.budget, &mut transitions, 1) {
            break;
        }
        let gain_gap_edges = if has_gain_gaps && t.kind == KIND_SOUND
            && t.effect.as_deref() == Some("BLEND_AUDIO_DISSOLVE") {
            [t.outgoing.as_deref(), t.incoming.as_deref()].map(|owner| {
                let owner = owner?;
                let scope = capture.scopes.iter().find(|scope| scope.id == owner)?;
                if !scope.position_known || !scope.group_path || scope.rate != t.rate || scope.retimed {
                    return None;
                }
                if scope.class == *b"FILL" {
                    let Observed::Known(overlap) = t.length else { return None };
                    return scope.record_length.filter(|&n| n >= overlap)
                        .map(|_| crate::source::GainGapEdge::Filler);
                }
                let mut matching = unresolved.iter().filter(|issue| issue.kind == t.kind
                    && issue.track == t.track && issue.record_rate == t.rate
                    && (issue.owner == owner || issue.owner.strip_prefix(owner).is_some_and(|suffix| suffix.starts_with('/'))));
                let issue = matching.next()?;
                let SourceIssueKind::UntranslatedEffect { effect: "AudioSuite Gain", length } = issue.issue_kind else { return None };
                (matching.next().is_none() && scope.record_length == Some(length))
                    .then(|| crate::source::GainGapEdge::Gain(copy_text(&capture.budget, &issue.owner)))
            })
        } else {
            [None, None]
        };
        let filler_scope = |owner: Option<&str>| {
            let owner = owner?;
            capture.scopes.iter().find(|scope| scope.id == owner)
                .filter(|scope| scope.class == *b"FILL" && scope.group_path
                    && scope.position_known && scope.rate == t.rate)
        };
        let outgoing_filler = if matches!((t.kind, t.effect.as_deref()),
            (KIND_SOUND, Some("BLEND_AUDIO_DISSOLVE"))
                | (KIND_PICTURE, Some("EFF2_BLEND_DISSOLVE" | "EFF2_BLEND_DISSOLVE_LOG" | "EFF2_BLEND_DISSOLVE_LIN")))
            && matches!(t.cutpoint, Observed::Known(0)) && t.rate.is_some()
        {
            filler_scope(t.outgoing.as_deref()).filter(|scope| !scope.retimed)
                .and_then(|scope| scope.record_length).filter(|&n| n > 0)
        } else {
            None
        };
        let incoming_filler = filler_scope(t.incoming.as_deref())
            .and_then(|scope| scope.record_length).filter(|&n| n > 0);
        transitions.push(SourceTransition {
            id: t.id,
            kind: t.kind,
            track: t.track,
            position: t.position,
            length: t.length,
            cutpoint: t.cutpoint,
            rate: t.rate,
            effect: t.effect,
            audio_curve_attribute: t.audio_curve_attribute,
            audio_curve_parameter: t.audio_curve_parameter,
            outgoing_filler,
            incoming_filler,
            gain_gap_edges,
            outgoing: binding(
                t.outgoing.as_deref(),
                &capture.scopes,
                &starts,
                &events,
                &unresolved,
                &capture.budget,
            ),
            incoming: binding(
                t.incoming.as_deref(),
                &capture.scopes,
                &starts,
                &events,
                &unresolved,
                &capture.budget,
            ),
        });
    }
    let mut markers = Vec::new();
    for m in capture.markers {
        if !reserve(&capture.budget, &mut markers, 1) {
            break;
        }
        let mut binding = if m.owner_kind == MarkerOwner::Sequence {
            Binding {
                placements: vec![],
                reason: None,
            }
        } else {
            binding(
                Some(&m.owner),
                &capture.scopes,
                &starts,
                &events,
                &unresolved,
                &capture.budget,
            )
        };
        if m.record.is_none() {
            binding.reason = Some(copy_text(
                &capture.budget,
                "owner-local marker record transform is unresolved",
            ));
        }
        markers.push(SourceMarker {
            id: m.id,
            owner: m.owner,
            kind: m.kind,
            track: m.track,
            owner_kind: m.owner_kind,
            marker: m.marker,
            local_offset: m.local,
            local_rate: m.rate,
            record_frame: m.record,
            binding,
        });
    }
    info.events.clear();
    info.markers.clear();
    info.transitions.clear();
    let mut texts = Vec::new();
    for mut fact in capture.texts {
        let scope = &capture.scopes[fact.scope];
        let timing_known = !scope.retimed && scope.position_known
            && fact.text.rate == capture.rate
            && fact.text.length.is_some_and(|n| n > 0)
            && fact.text.record_start.is_some_and(|n| n >= 0 && n.checked_add(fact.text.length.unwrap_or(0)).is_some())
            && capture.scopes.iter().all(|ancestor| {
                // Prefix boundaries distinguish component/1 from component/10.
                let is_ancestor = scope.id.strip_prefix(&ancestor.id).is_some_and(|tail| tail.starts_with('/'));
                !is_ancestor || (ancestor.class == *b"SEQU" && ancestor.position_known && !ancestor.retimed && ancestor.rate == capture.rate)
            });
        if !timing_known {
            fact.text.record_start = None;
            fact.text.reason = Some(match &fact.text.reason {
                Some(reason) => formatted(&capture.budget, format_args!("{reason}; text owner-to-record timing is unresolved or nested under an unsupported component")),
                None => copy_text(&capture.budget, "text owner-to-record timing is unresolved or nested under an unsupported component"),
            });
        }
        if reserve(&capture.budget, &mut texts, 1) { texts.push(fact.text); }
    }
    let group_uses = groups::finish_uses(&mut capture.group_uses, &capture.scopes, &starts, &events, &unresolved, &capture.budget);
    // Scopes are temporary. All exported owners already have their own
    // charged identifiers, so subsequent sequences can reuse these bytes.
    let scope_bytes = sum(std::iter::once(array::<Scope>(capture.scopes.capacity()))
        .chain(capture.scopes.iter().map(|scope| scope.id.len())));
    drop(capture.scopes);
    capture.budget.borrow_mut().release(scope_bytes);
    SourceSequence {
        id: capture.id,
        info,
        rate: capture.rate,
        clock: capture.clock,
        track_names: capture.track_names,
        combiner_channels,
        events,
        transitions,
        markers,
        unresolved,
        texts,
        group_uses,
    }
}

fn route_cost(route: &MobRoute, budget: &SharedBudget) -> usize {
    let mut cost = sum([
        size_of::<MobRoute>(),
        route.name.capacity(),
        array::<RouteTrack>(route.tracks.capacity()),
    ]);
    fn action_cost(action: &Arc<RouteAction>, budget: &SharedBudget) -> usize {
        if !first_shared(budget, Arc::as_ptr(action) as usize) {
            return 0;
        }
        sum([
            size_of::<RouteAction>(),
            match action.as_ref() {
                RouteAction::Selected {
                    inner: Some(inner), ..
                } | RouteAction::SubclipPanVolume { inner, .. } => action_cost(inner, budget),
                RouteAction::Other(s) => s.capacity(),
                _ => 0,
            },
        ])
    }
    for track in &route.tracks {
        cost = sum([cost, size_of::<Arc<Vec<RouteSegment>>>()]);
        if first_shared(budget, Arc::as_ptr(&track.segments) as usize) {
            cost = sum([
                cost,
                array::<RouteSegment>(track.segments.capacity()),
                sum(track
                    .segments
                    .iter()
                    .map(|s| action_cost(&s.action, budget))),
            ]);
        }
    }
    cost
}

pub(super) fn read_source_with<T: Borrow<AvbFile>>(
    inputs: &[SourceInput],
    limits: SourceLimits,
    mut open: impl FnMut(&Path) -> crate::io::Result<T>,
) -> Result<SourceProject, SourceReadError> {
    let budget = Rc::new(RefCell::new(Budget {
        limit: limits.max_fact_bytes.min(MAX_SOURCE_FACT_BYTES),
        used: 0,
        peak: 0,
        error: None,
        shared: HashSet::new(),
    }));
    if !charge(&budget, size_of::<SourceProject>()) {
        return Err(crate::source::failure(
            inputs
                .first()
                .map_or(Path::new("<source>"), |i| i.path.as_path()),
            "accounted source facts exceed configured limit",
        ));
    }
    let mut masters = Vec::new();
    let mut imported_stills = Vec::new();
    let mut subclips = Vec::new();
    let mut groups = Vec::new();
    let mut group_dependencies = Vec::new();
    let mut sequences = Vec::new();
    let mut skipped = Vec::new();
    let mut duplicates = Vec::new();
    let mut routes = HashMap::new();
    let mut seen = HashSet::new();
    for (bin, input) in inputs.iter().enumerate() {
        if input.revision.len() != 64 || !input.revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(crate::source::failure(
                &input.path,
                "source input revision must be SHA-256 hex",
            ));
        }
        if !charge(&budget, input.revision.len() + size_of::<InputContext>()) {
            return Err(crate::source::failure(
                &input.path,
                "accounted source facts exceed configured limit",
            ));
        }
        let context = InputContext {
            revision: input.revision.to_ascii_lowercase(),
            bin,
            budget: budget.clone(),
        };
        let file =
            open(&input.path).map_err(|e| crate::source::failure(&input.path, e.to_string()))?;
        let summary = compile_bin_inner(file.borrow(), bin, Some(&context));
        drop(file);
        for (id, route) in summary.routes {
            if !routes.contains_key(&id) {
                // Conservative hash-slot accounting includes key/value slots and
                // bucket/reference overhead before the map can grow.
                if !charge(&budget, 4 * size_of::<(MobId, MobRoute)>()) {
                    break;
                }
                routes.insert(id, route);
            }
        }
        for item in summary.items {
            match item {
                Item::Master {
                    id,
                    bin,
                    name,
                    result,
                    source,
                } => {
                    if let Some(id) = id {
                        if !charge(&budget, 4 * size_of::<MobId>()) {
                            break;
                        }
                        if !seen.insert(id) {
                            if !reserve(&budget, &mut duplicates, 1) {
                                break;
                            }
                            duplicates.push((copy_text(&budget, &name), bin));
                        }
                    }
                    let mut meta = source.expect("source master witness");
                    if let Some(still) = meta.imported_still.take() {
                        if !reserve(&budget, &mut imported_stills, 1) {
                            break;
                        }
                        imported_stills.push(still);
                    }
                    match result {
                        Ok(master) => {
                            let rate = Rate::from_avid(master.edit_rate);
                            if !reserve(&budget, &mut masters, 1) {
                                break;
                            }
                            masters.push(SourceMaster {
                                id: meta.id,
                                master,
                                rate,
                                offsets: meta.offsets,
                                marker_witnesses: meta.marker_witnesses,
                                originals: meta.originals,
                                file_clocks: meta.file_clocks,
                                managed_multistream: meta.managed_multistream,
                                imported_audio: meta.imported_audio,
                                pcm_prefix: meta.pcm_prefix,
                                input_transforms: meta.input_transforms,
                                rendered_freeze: meta.rendered_freeze,
                                rendered_gain: meta.rendered_gain,
                            });
                        }
                        Err(reason) => {
                            if !reserve(&budget, &mut skipped, 1) {
                                break;
                            }
                            skipped.push(Skipped { name, reason });
                        }
                    }
                }
                Item::Sequence(seq) => {
                    if !charge(&budget, size_of::<PendingSequence>())
                        || !reserve(&budget, &mut sequences, 1)
                    {
                        break;
                    }
                    sequences.push(seq);
                }
                Item::Subclip(subclip) => {
                    if !reserve(&budget, &mut subclips, 1) {
                        break;
                    }
                    subclips.push(subclip);
                }
                Item::Group(group) => {
                    if !reserve(&budget, &mut groups, 1) { break; }
                    groups.push(group);
                }
                Item::GroupDependency { id, name } => {
                    if !reserve(&budget, &mut group_dependencies, 1) { break; }
                    group_dependencies.push((id, name));
                }
                Item::Skipped(item) => {
                    if !reserve(&budget, &mut skipped, 1) {
                        break;
                    }
                    skipped.push(item);
                }
            }
        }
        if let Some(error) = &budget.as_ref().borrow().error {
            return Err(crate::source::failure(&input.path, error.clone()));
        }
        // Allocation addresses are memoization only, never source IDs. Cache
        // payloads are bin-local; forgetting keys between bins prevents allocator
        // address reuse from undercharging a later bin's unrelated allocation.
        budget.borrow_mut().shared.clear();
    }
    let last = inputs
        .last()
        .map_or(Path::new("<source>"), |i| i.path.as_path());
    let mut output = Vec::new();
    let mut referenced = HashSet::new();
    for group in &groups {
        for lane in &group.lanes {
            for alt in &lane.alternatives {
                if alt.endpoint.class == *b"SCLP" {
                    if let Some(id) = alt.endpoint.mob {
                        if !charge(&budget, 4 * size_of::<MobId>()) { break; }
                        referenced.insert(id);
                    }
                }
            }
        }
    }
    for (id, name) in group_dependencies {
        // Usage alone cannot prove a hidden dependency. Keep an explicit skip
        // for usage5 items not referenced by any retained native group.
        if id.is_some_and(|id| referenced.contains(&id)) { continue; }
        if !reserve(&budget, &mut skipped, 1) { break; }
        skipped.push(Skipped { name, reason: copy_text(&budget,
            "composition usage5 is not referenced by a retained source group") });
    }
    for group in &mut groups { groups::resolve_group(group, &routes, &budget); }
    for pending in sequences {
        if !charge(&budget, size_of::<SourceSequence>()) || !reserve(&budget, &mut output, 1) {
            break;
        }
        output.push(finish_source(pending, &routes));
        if budget.as_ref().borrow().error.is_some() {
            break;
        }
    }
    if let Some(error) = &budget.as_ref().borrow().error {
        return Err(crate::source::failure(last, error.clone()));
    }
    let accounted_bytes = budget.as_ref().borrow().peak;
    Ok(SourceProject {
        masters,
        imported_stills,
        subclips,
        groups,
        sequences: output,
        skipped,
        duplicates,
        accounted_bytes,
    })
}

pub(crate) fn extract_source_paths(
    inputs: &[SourceInput],
    limits: SourceLimits,
) -> Result<SourceProject, SourceReadError> {
    read_source_with(inputs, limits, AvbFile::open)
}
/// Observe the bin immediately before opening it; cancellation never leaves a decoded bin retained.
pub(crate) fn extract_source_paths_observed(inputs:&[SourceInput],limits:SourceLimits,before:&mut dyn FnMut(usize,&Path)->Result<(),String>)->Result<SourceProject,SourceReadError>{
    extract_source_paths_inspected(inputs, limits, before, &mut |_| Ok(()))
}
pub(crate) fn extract_source_paths_inspected(
    inputs: &[SourceInput], limits: SourceLimits,
    before: &mut dyn FnMut(usize, &Path) -> Result<(), String>,
    inspect: &mut dyn FnMut(&AvbFile) -> Result<(), String>,
) -> Result<SourceProject, SourceReadError> {
    let mut n = 0;
    read_source_with(inputs, limits, |path| {
        before(n, path).map_err(|e| crate::io::Error::new(e, 0))?;
        n += 1;
        let file = AvbFile::open(path)?;
        inspect(&file).map_err(|e| crate::io::Error::new(e, 0))?;
        Ok(file)
    })
}
/// Synthetic/differential helper. Caller-owned decoded files are not opened or copied.
/// Their input revisions must correspond to `inputs` just as in the path API.
pub(crate) fn extract_source_files(
    inputs: &[SourceInput],
    files: &[&AvbFile],
    limits: SourceLimits,
) -> Result<SourceProject, SourceReadError> {
    if inputs.len() != files.len() {
        return Err(crate::source::failure(
            Path::new("<source>"),
            "input/file length mismatch",
        ));
    }
    let mut index = 0;
    read_source_with(inputs, limits, |_| {
        let file = files[index];
        index += 1;
        Ok(file)
    })
}

pub(super) fn route_range_qualified(ctx: &Ctx<'_>, component: &Node) -> bool {
    let mut position = 0i64;
    if let Some(Value::RefList(children)) = component.get("components") {
        for child in children {
            let Some(node) = ctx.deref(Some(child)) else {
                return false;
            };
            let Some(length) = int(node, "length") else {
                return false;
            };
            if length < 0 || node.class == *b"TNFX" {
                return false;
            }
            let Some(next) = position.checked_add(length) else {
                return false;
            };
            position = next;
        }
        true
    } else {
        int(component, "length").is_some_and(|n| n >= 0)
    }
}

#[allow(clippy::too_many_arguments)]
fn master_marker_witnesses(
    ctx: &Ctx<'_>,
    node: &Node,
    owner: &str,
    kind: i64,
    track: i64,
    position: Option<i64>,
    inherited: Option<Rate>,
    retimed: bool,
    depth: usize,
    budget: &SharedBudget,
    out: &mut Vec<MasterMarkerWitness>,
) {
    if depth > MAX_NESTING {
        return;
    }
    let rate = match node.get("edit_rate") {
        Some(_) => float(node, "edit_rate").and_then(Rate::from_avid),
        None => inherited,
    };
    let retimed = retimed || node.class == *b"SPED";
    let position = if retimed || rate != inherited {
        None
    } else {
        position
    };
    each_marker(ctx, node, |marker| {
        if !reserve(budget, out, 1) {
            return;
        }
        let offset = int(marker, "comp_offset");
        let local_offset = known(offset, budget, "marker comp_offset is absent");
        let master_frame = if !retimed && rate == inherited {
            position.zip(offset).and_then(|(p, o)| p.checked_add(o))
        } else {
            None
        };
        out.push(MasterMarkerWitness {
            owner: copy_text(budget, owner),
            kind,
            track,
            local_offset,
            local_rate: rate,
            master_frame,
        });
    });
    if let Some(Value::RefList(children)) = node.get("components") {
        let mut at = position;
        for (i, child) in children.iter().enumerate() {
            let Some(child) = ctx.deref(Some(child)) else {
                at = None;
                continue;
            };
            let owner = formatted(budget, format_args!("{owner}/component/{i}"));
            master_marker_witnesses(
                ctx,
                child,
                &owner,
                kind,
                track,
                at,
                rate,
                retimed,
                depth + 1,
                budget,
                out,
            );
            let child_rate = match child.get("edit_rate") {
                Some(_) => float(child, "edit_rate").and_then(Rate::from_avid),
                None => rate,
            };
            at = if child.class == *b"TNFX" || child_rate != rate {
                None
            } else {
                at.zip(int(child, "length"))
                    .and_then(|(p, n)| p.checked_add(n))
            };
        }
    } else {
        for (i, t) in ctx.tracks(node).into_iter().enumerate() {
            if let Some(child) = ctx.deref(t.get("component")) {
                let owner = formatted(budget, format_args!("{owner}/input/{i}"));
                master_marker_witnesses(
                    ctx,
                    child,
                    &owner,
                    kind,
                    track,
                    position,
                    rate,
                    retimed,
                    depth + 1,
                    budget,
                    out,
                );
            }
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn fact_growth_refuses_before_vector_or_string_allocation() {
        let budget = Rc::new(RefCell::new(Budget {
            limit: 1,
            used: 0,
            peak: 0,
            error: None,
            shared: HashSet::new(),
        }));
        let mut values = Vec::<String>::new();
        assert!(!reserve(&budget, &mut values, 1));
        assert_eq!(values.capacity(), 0);
        let budget = Rc::new(RefCell::new(Budget {
            limit: 2,
            used: 0,
            peak: 0,
            error: None,
            shared: HashSet::new(),
        }));
        let text = formatted(&budget, format_args!("more than two"));
        assert_eq!(text.capacity(), 0);
        assert!(budget.as_ref().borrow().error.is_some());
    }

    #[test]
    fn dropped_temporaries_leave_budget_room_without_lowering_the_high_water_mark() {
        let mut budget = Budget {
            limit: 10,
            used: 0,
            peak: 0,
            error: None,
            shared: HashSet::new(),
        };
        assert!(budget.charge(9));
        budget.release(6);
        assert!(budget.charge(5));
        assert_eq!((budget.used, budget.peak), (8, 9));
        assert!(!budget.charge(3));
    }
}

#[cfg(test)]
mod wrapped_clock_tests {
    use super::*;
    use crate::{document::AvbDocument, file::Object};

    #[test]
    fn linked_picture_clock_wrapper_rejects_position_changing_filler() {
        let bytes = AvbDocument::new_empty_bin(19, "Clock control", 1, false).to_bytes().unwrap();
        let mut file = AvbFile::parse(&bytes).unwrap();
        let start = file.objects.len() as u32;
        let build = |class, length| Object::Decoded(Node {
            class,
            type_name: "clock control",
            fields: vec![
                ("media_kind_id", Value::Int(KIND_TIMECODE)),
                ("edit_rate", Value::Float(24.0)),
                ("length", Value::Int(length)),
            ],
        });
        file.objects.extend([
            Some(build(*b"FILL", 0)),
            Some(build(*b"TCCP", 48)),
            Some(build(*b"FILL", 0)),
        ]);
        let wrapper = Node {
            class: *b"SEQU",
            type_name: "clock control wrapper",
            fields: vec![
                ("media_kind_id", Value::Int(KIND_TIMECODE)),
                ("edit_rate", Value::Float(24.0)),
                ("components", Value::RefList((start..start + 3).map(Value::Ref).collect())),
            ],
        };
        let clock_leaf = |file: &AvbFile| {
            let ctx = Ctx { file, mobs: HashMap::new() };
            bracketed_24_component(&ctx, &wrapper, KIND_TIMECODE, *b"TCCP")
                .and_then(|component| int(component, "length"))
        };
        assert_eq!(clock_leaf(&file), Some(48));
        file.objects[start as usize] = Some(build(*b"FILL", 1));
        assert_eq!(clock_leaf(&file), None);
    }
}
