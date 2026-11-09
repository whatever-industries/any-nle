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

type SharedBudget = Rc<RefCell<Budget>>;
struct Budget {
    limit: usize,
    used: usize,
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
}
fn charge(budget: &SharedBudget, bytes: usize) -> bool {
    budget.borrow_mut().charge(bytes)
}
// Source-owned growth is reserved/accounted before allocation. Existing decoder
// and compatibility-compiler working storage have their own bounded lifetimes.
fn reserve<T>(budget: &SharedBudget, values: &mut Vec<T>, additional: usize) -> bool {
    let Some(required) = values.len().checked_add(additional) else {
        budget.borrow_mut().fail("source vector length overflow");
        return false;
    };
    if required <= values.capacity() {
        return true;
    }
    let capacity = required
        .max(values.capacity().checked_mul(2).unwrap_or(required))
        .max(4);
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
            Item::Group(_) => 0,
            Item::GroupDependency { name, .. } => name.capacity(),
        };
        if !charge(&self.budget, cost) || !reserve(&self.budget, values, 1) {
            return;
        }
        values.push(item);
    }
    pub(super) fn master_meta(
        &self,
        ctx: &Ctx<'_>,
        mob: &Node,
        item: usize,
        master: Option<&MasterClip>,
    ) -> MasterMeta {
        let id = formatted(
            &self.budget,
            format_args!("{}/bin/{}/item/{}", self.revision, self.bin, item),
        );
        let mut offsets = Vec::new();
        let mut marker_witnesses = Vec::new();
        let mut originals = Vec::new();
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
                let offset = if count == 1 {
                    known(
                        matched
                            .and_then(|track| ctx.only_source_clip(track).ok())
                            .and_then(|c| int(c, "start_time")),
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
                let descriptor = ctx.mobs.get(&native.file_mob).and_then(|m| ctx.deref(m.get("descriptor")));
                let original = descriptor.and_then(|d| {
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
                    Some(OriginalTrack {
                        locators,
                        descriptor_class: copy_text(&self.budget, std::str::from_utf8(&d.class).unwrap_or("????")),
                        descriptor_rate: float(d, "edit_rate").unwrap_or(0.0),
                        descriptor_length: int(d, "length").unwrap_or(0),
                        channels: int(d, "channels"),
                        sample_rate: float(d, "sample_rate"),
                        stored_quantization_bits: int(d, "quantization_bits"),
                        file_track: matched.and_then(|t| ctx.only_source_clip(t).ok()).and_then(|c| int(c, "track_id")),
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
        MasterMeta {
            id,
            offsets,
            marker_witnesses,
            originals,
        }
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
    texts: Vec<TextFact>,
    group_uses: Vec<groups::UseFact>,
    charged_ops: usize,
}
impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture").field("id", &self.id).finish()
    }
}
impl Capture {
    pub(super) fn capture_volume(
        &mut self, ctx: &Ctx<'_>, node: &Node, tracks: &[&Node], position: i64,
    ) -> Option<super::volume::Capture> {
        super::volume::capture(ctx, node, tracks, position, |points| {
            charge(&self.budget, points * size_of::<VolumePoint>())
        })
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
            texts: Vec::new(),
            group_uses: Vec::new(),
            charged_ops: 0,
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
        let group_path = parent.is_none_or(|s| s.group_path && s.class == *b"SEQU");
        if let Some(i) = self.stack.last() {
            self.scopes[*i].children += 1;
        } else {
            self.roots += 1;
        }
        if !reserve(&self.budget, &mut self.scopes, 1) || !reserve(&self.budget, &mut self.stack, 1)
        {
            return;
        }
        let scope_id = copy_text(&self.budget, &id);
        let index = self.scopes.len();
        self.scopes.push(Scope {
            id: scope_id,
            class: node.class,
            from,
            to: from,
            rate,
            position_known: known,
            retimed,
            record_length: int(node, "length"),
            children: 0,
            group_path,
        });
        self.stack.push(index);
        groups::capture_use(self, ctx, node, index, kind, track, position, rate);
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
                        if text_kind == TextKind::TitlerPlus {
                            match super::text::properties(body) {
                                Ok(properties) => for (scope, list, name, value) in properties {
                                    if !reserve(&self.budget, &mut native_properties, 1) { return; }
                                    native_properties.push(NativeTextProperty {
                                        scope: copy_text(&self.budget, scope), list,
                                        name: copy_text(&self.budget, name), value: copy_text(&self.budget, value),
                                    });
                                },
                                Err(reason) => native_properties_reason = Some(copy_text(&self.budget, reason)),
                            }
                        }
                        super::text::decode(body, text_kind)
                    },
                    Err(reason) => Err(reason),
                };
                let (content, reason) = match decoded {
                    Ok(content) => (Some(content), None),
                    Err(reason) => (None, Some(copy_text(&self.budget, reason))),
                };
                self.texts.push(TextFact { scope: index, text: SourceText {
                    id: copy_text(&self.budget, &id), kind: text_kind, track, content,
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
            let owner = copy_text(&self.budget, &id);
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
    /// A marker on a direct, sequence-spanning TCCP has a qualified record-local
    /// clock without qualifying the sequence's absolute start timecode. Preserve
    /// other top-level timecode owners as unresolved facts; do not treat an
    /// arbitrary metadata graph as an editable A/V placement.
    pub(super) fn timecode_track_markers(
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
    pub(super) fn query(&mut self, op: usize, node: &Node, motion: Option<(i64, i64)>) {
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
                owner.class == *b"SEQU" || owner.record_length == record_length
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
            retimed: scope.retimed,
            motion,
        });
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
                    cost = color_cost(&value.params);
                }
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
    if !reserve(budget, &mut out, route.len()) {
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
    if !reserve(budget, issues, 1) {
        return;
    }
    issues.push(SourceIssue {
        owner: copy_text(budget, context.owner),
        reason: copy_text(budget, reason),
        route,
        record_start: if context.position_known {
            Observed::Known(context.record)
        } else {
            Observed::Unresolved(copy_text(budget, "record position is unresolved"))
        },
        record_rate: context.rate,
        kind: context.kind,
        track: context.track,
    });
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
    if !reserve(budget, &mut route, 1) {
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
    let reason = if fact.start.is_none() {
        Some("source start is absent")
    } else if fact.label.is_none() {
        Some("source track label is absent")
    } else if fact.label.is_some_and(|n| n <= 0) {
        Some("source track label is not positive")
    } else if fact.length.is_none() {
        Some("source length is absent")
    } else if !fact.enclosing_span_known {
        Some("enclosing component span is unresolved or differs from its source input")
    } else if !fact.position_known {
        Some("record position follows unresolved native timing")
    } else if fact.rate.is_none() {
        Some("source owner rate is unresolved")
    } else if fact.rate != fact.record_rate {
        Some("source component rate differs from its record owner; conversion is unqualified")
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
        if reserve(budget, &mut route, 1) {
            route.push(witness(
                id,
                label,
                mob.rate,
                Some(offset),
                Some(length),
                budget,
            ));
        }
        return add_issue(
            context,
            "mixed or unknown route-owner rates are not qualified",
            route,
            budget,
            issues,
        );
    }
    match mob.kind {
        Some(MOB_MASTER) => {
            if reserve(budget, pieces, 1) {
                pieces.push(Piece {
                    master: id,
                    label,
                    record,
                    source: offset,
                    length,
                    rate,
                    route,
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
            if track.rate != Some(rate) || !track.range_qualified {
                return add_issue(
                    context,
                    "source composition units or ranges are unresolved",
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
                let action = match seg.action.as_ref() {
                    RouteAction::Selected {
                        inner: Some(inner), ..
                    } => inner.as_ref(),
                    other => other,
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
                        let component_rate = if *rate_present {
                            *component_rate
                        } else {
                            track.rate
                        };
                        let mut next = clone_route(&route, budget);
                        if !reserve(budget, &mut next, 1) {
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
                            add_issue(
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
        i.owner == scope.id
            || i.owner
                .strip_prefix(&scope.id)
                .is_some_and(|rest| rest.starts_with('/'))
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
    let mut unresolved = Vec::new();
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
                let Some(fact) = capture.queries.iter().find(|f| f.op == op_index) else {
                    continue;
                };
                let (pieces, issues) = qualified_query(routes, &q, fact, &capture.budget);
                if !reserve(&capture.budget, &mut unresolved, issues.len()) {
                    break;
                }
                unresolved.extend(issues);
                // Motion spans are qualified facts, not a rendered sampling claim.
                let motion = fact.motion;
                if motion.is_some()
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
                    add_issue(
                        &context,
                        "motion source does not have one qualified range",
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
                        event.speed = Some(ratio);
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
                        picture_extension: None,
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
            Op::PanVolume { before, gain, pans, mut automation } => {
                let produced = &mut events[starts[before]..];
                let single = produced.len() == 1;
                for e in produced {
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
                        e.volume_automation = Some(captured.resolve(single, e));
                    }
                }
            }
            Op::Color { before, value } => {
                for e in &mut events[starts[before]..] {
                    if !charge(&capture.budget, color_cost(&value)) {
                        break;
                    }
                    e.event.color = Some((*value).clone());
                }
            }
            Op::Picture { before, value } => {
                for e in &mut events[starts[before]..] {
                    if e.event.picture.is_some() {
                        e.event.picture_nested = true;
                    } else {
                        if !charge(&capture.budget, color_cost(&value.params)) {
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
                    event.picture_extension = Some(picture.resolve(single, event));
                }
            }
            Op::Matte {
                before,
                length,
                value,
            } => {
                let produced = events.len() - starts[before];
                for e in &mut events[starts[before]..] {
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
    for t in capture.transitions {
        if !reserve(&capture.budget, &mut transitions, 1) {
            break;
        }
        transitions.push(SourceTransition {
            id: t.id,
            kind: t.kind,
            track: t.track,
            position: t.position,
            length: t.length,
            cutpoint: t.cutpoint,
            rate: t.rate,
            effect: t.effect,
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
    SourceSequence {
        id: capture.id,
        info,
        rate: capture.rate,
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
                } => action_cost(inner, budget),
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
                    match result {
                        Ok(master) => {
                            let meta = source.expect("source master witness");
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
    let accounted_bytes = budget.as_ref().borrow().used;
    Ok(SourceProject {
        masters,
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
    let mut n=0;
    read_source_with(inputs,limits,|path|{before(n,path).map_err(|e|crate::io::Error::new(e,0))?;n+=1;AvbFile::open(path)})
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
            error: None,
            shared: HashSet::new(),
        }));
        let mut values = Vec::<String>::new();
        assert!(!reserve(&budget, &mut values, 1));
        assert_eq!(values.capacity(), 0);
        let budget = Rc::new(RefCell::new(Budget {
            limit: 2,
            used: 0,
            error: None,
            shared: HashSet::new(),
        }));
        let text = formatted(&budget, format_args!("more than two"));
        assert_eq!(text.capacity(), 0);
        assert!(budget.as_ref().borrow().error.is_some());
    }
}
