//! Owned conversion summaries. No decoded object table, source bytes, or borrowed nodes
//! escape compile_bin. The event program defers only cross-bin resolution and operations
//! whose result depends on the number of resolved events (speed, matte, nested effects).
use super::*;
use std::borrow::Borrow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod source_capture;
mod text;
mod volume;
mod picture_extension;
mod freeze;
mod gain;
pub(crate) use source_capture::{extract_source_paths, extract_source_files, extract_source_paths_observed};

#[derive(Debug)]
pub struct ProjectReadError {
    pub path: PathBuf,
    pub error: crate::io::Error,
}
impl std::fmt::Display for ProjectReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}
impl std::error::Error for ProjectReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Extract an already-selected, ordered list. At most one decoded bin is live at a time.
/// Repeated-path selection is the caller's policy (the CLIs use inputs::unique_bins).
/// A read failure returns before a result is published; no output files are created here.
pub fn extract_project_paths(paths: &[PathBuf]) -> Result<BinContents, ProjectReadError> {
    read_paths_with(paths, AvbFile::open)
}

fn read_paths_with<T: Borrow<AvbFile>>(paths: &[PathBuf], mut open: impl FnMut(&Path) -> crate::io::Result<T>) -> Result<BinContents, ProjectReadError> {
    let mut project = Project::default();
    for (bin, path) in paths.iter().enumerate() {
        let file = open(path).map_err(|error| ProjectReadError { path: path.clone(), error })?;
        let summary = compile_bin(file.borrow(), bin);
        drop(file); // A lifecycle invariant, also exercised with a tracked loader in tests.
        project.push(summary);
    }
    Ok(project.finish())
}

/// Compatibility API for callers that already own decoded files. The path API avoids
/// keeping those files alive; both use exactly the same semantic compilation/resolution.
pub fn extract_project(files: &[&AvbFile]) -> BinContents {
    let mut project = Project::default();
    for (bin, file) in files.iter().enumerate() {
        project.push(compile_bin(file, bin));
    }
    project.finish()
}

// A source route only needs the supported range-routing facts, not arbitrary attributes,
// descriptors, unselected graph branches, or source bytes.
#[derive(Debug)]
struct MobRoute {
    rate: Option<crate::source::Rate>,
    length: Option<i64>,
    kind: Option<i64>,
    name: String,
    usage: Option<i64>,
    tracks: Vec<RouteTrack>,
}
#[derive(Debug)]
struct RouteTrack {
    rate: Option<crate::source::Rate>,
    range_qualified: bool,
    label: Option<i64>,
    kind: Option<i64>,
    segments: Arc<Vec<RouteSegment>>,
}
#[derive(Debug)]
struct RouteSegment {
    position: i64,
    length: i64,
    observed_length: Option<i64>,
    rate: Option<crate::source::Rate>,
    rate_present: bool,
    action: Arc<RouteAction>,
}
#[derive(Debug)]
enum RouteAction {
    Empty,
    Source { id: Option<MobId>, label: i64, start: i64,
        observed_label: Option<i64>, observed_start: Option<i64>, rate: Option<crate::source::Rate>, rate_present: bool },
    Selected { ordinal: usize, inner: Option<Arc<RouteAction>> },
    Other(String),
}
// Memoization is confined to this bin: keys are transient node addresses, never
// media identities. Only owned supported facts escape through the shared values.
#[derive(Default)]
struct CompileCache {
    actions: HashMap<(usize, bool), Arc<RouteAction>>,
    segments: HashMap<usize, Arc<Vec<RouteSegment>>>,
    colors: HashMap<usize, Arc<ColorCorrection>>,
    pictures: HashMap<usize, Arc<PictureEffect>>,
    mattes: HashMap<(u32, u32, u32, u32), Option<Arc<AniMatteData>>>,
    freezes: HashMap<MobId, freeze::Freeze>,
    ambiguous: std::collections::HashSet<MobId>,
    gain_renders: HashMap<MobId, MobId>,
}
fn route_action(ctx: &Ctx<'_>, node: &Node, selector: bool, cache: &mut CompileCache) -> Arc<RouteAction> {
    let key = (node as *const Node as usize, selector);
    if let Some(value) = cache.actions.get(&key) {
        return value.clone();
    }
    let value = Arc::new(match &node.class {
        b"FILL" => RouteAction::Empty,
        b"SCLP" => RouteAction::Source {
            id: mob_id(node),
            label: int(node, "track_id").unwrap_or(0),
            start: int(node, "start_time").unwrap_or(0),
            observed_label: int(node, "track_id"),
            observed_start: int(node, "start_time"),
            rate: float(node, "edit_rate").and_then(crate::source::Rate::from_avid),
            rate_present: node.get("edit_rate").is_some(),
        },
        b"SLCT" if selector => {
            let ordinal = int(node, "selected").unwrap_or(0) as usize;
            let tracks = ctx.tracks(node);
            let inner = tracks.get(ordinal).and_then(|t| ctx.deref(t.get("component"))).map(|n| {
                if &n.class == b"SCLP" {
                    route_action(ctx, n, false, cache)
                } else {
                    Arc::new(RouteAction::Other(n.class_str()))
                }
            });
            RouteAction::Selected { ordinal, inner }
        }
        _ => RouteAction::Other(node.class_str()),
    });
    cache.actions.insert(key, value.clone());
    value
}
fn route_segments(ctx: &Ctx<'_>, node: &Node, cache: &mut CompileCache) -> Arc<Vec<RouteSegment>> {
    let key = node as *const Node as usize;
    if let Some(value) = cache.segments.get(&key) {
        return value.clone();
    }
    let value: Arc<Vec<RouteSegment>> = Arc::new(
        ctx.segments(node)
            .into_iter()
            .map(|(position, seg)| RouteSegment {
                position,
                length: int(seg, "length").unwrap_or(0),
                observed_length: int(seg, "length"),
                rate: float(seg, "edit_rate").and_then(crate::source::Rate::from_avid),
                rate_present: seg.get("edit_rate").is_some(),
                action: route_action(ctx, seg, true, cache),
            })
            .collect(),
    );
    cache.segments.insert(key, value.clone());
    value
}

fn route(ctx: &Ctx<'_>, mob: &Node, cache: &mut CompileCache) -> MobRoute {
    let kind = int(mob, "mob_type_id");
    let composition = kind == Some(MOB_COMPOSITION);
    MobRoute {
        rate: float(mob, "edit_rate").and_then(crate::source::Rate::from_avid),
        length: int(mob, "length"),
        kind,
        name: if composition {
            mob.get("name").and_then(Value::as_str).unwrap_or("").into()
        } else {
            String::new()
        },
        usage: if composition { int(mob, "usage_code") } else { None },
        tracks: if composition {
            ctx.tracks(mob)
                .into_iter()
                .map(|t| {
                    let component = ctx.deref(t.get("component"));
                    RouteTrack {
                        range_qualified: component.is_some_and(|c| source_capture::route_range_qualified(ctx, c)),
                        rate: component.and_then(|c| match c.get("edit_rate") {
                            Some(_) => float(c, "edit_rate"), None => float(mob, "edit_rate")
                        }).and_then(crate::source::Rate::from_avid),
                        label: int(t, "index"),
                        kind: component.and_then(|c| int(c, "media_kind_id")),
                        segments: component.map(|c| route_segments(ctx, c, cache)).unwrap_or_default(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        },
    }
}

#[derive(Debug)]
struct BinSummary {
    routes: Vec<(MobId, MobRoute)>,
    items: Vec<Item>,
}
#[derive(Debug)]
enum Item {
    Master {
        id: Option<MobId>,
        bin: usize,
        name: String,
        result: Result<MasterClip, String>,
        source: Option<source_capture::MasterMeta>,
    },
    Sequence(PendingSequence),
    Group(crate::source::SourceGroup),
    GroupDependency { id: Option<MobId>, name: String },
    Skipped(Skipped),
}
fn compile_bin(file: &AvbFile, bin: usize) -> BinSummary {
    compile_bin_inner(file, bin, None)
}
fn retain_item(items:&mut Vec<Item>,source:Option<&source_capture::InputContext>,item:Item){
    if let Some(source)=source{source.retain_item(items,item);}else{items.push(item);}
}
fn compile_bin_inner(file: &AvbFile, bin: usize, source: Option<&source_capture::InputContext>) -> BinSummary {
    let items = bin_items(file);
    let mut ctx = Ctx { file, mobs: HashMap::new() };
    let mut ambiguous = std::collections::HashSet::new();
    // Keep the existing last-item local lookup; the project route index below is first-item.
    for mob in &items {
        if let Some(id) = mob_id(mob) {
            if ctx.mobs.insert(id, *mob).is_some() && source.is_some() {
                ambiguous.insert(id);
            }
        }
    }
    let mut cache = CompileCache::default();
    if source.is_some() {
        let mut repeated_renders = std::collections::HashSet::new();
        for mob in &items {
            if let Some((id, binding)) = freeze::binding(&ctx, mob, &ambiguous) {
                if cache.freezes.insert(id, binding).is_some() {
                    repeated_renders.insert(id);
                }
            }
        }
        cache.freezes.retain(|id, _| !repeated_renders.contains(id));
        cache.ambiguous = ambiguous;
    }
    let freeze_owners: std::collections::HashSet<_> = cache.freezes.values().map(|freeze| freeze.owner).collect();
    let mut summary = BinSummary {
        routes: Vec::new(),
        items: Vec::new(),
    };
    for (item_index, mob) in items.into_iter().enumerate() {
        if source.is_some_and(|source|source.failed()){break;}
        if let Some(id) = mob_id(mob) {
            let value=route(&ctx,mob,&mut cache);
            if let Some(source)=source{source.retain_route(&mut summary.routes,id,value);}
            else{summary.routes.push((id,value));}
            if source.is_some_and(|source|source.failed()){break;}
        }
        let freeze = mob_id(mob).and_then(|id| cache.freezes.get(&id));
        let named_mob = freeze.and_then(|binding| ctx.mobs.get(&binding.owner).copied()).unwrap_or(mob);
        let name = named_mob.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        match (int(mob, "mob_type_id"), int(mob, "usage_code")) {
            (Some(MOB_MASTER), _) => {
                let result = master(&ctx, mob).map(|mut m| {
                    m.bin = bin;
                    if freeze.is_some() {
                        m.name.clone_from(&name);
                    }
                    m
                });
                retain_item(&mut summary.items,source,Item::Master {
                    id: mob_id(mob),
                    bin,
                    name,
                    source: source.map(|source| {
                        let mut meta = source.master_meta(&ctx, mob, item_index, result.as_ref().ok());
                        meta.rendered_freeze = freeze.is_some();
                        meta
                    }),
                    result,
                });
            }
            (Some(MOB_SOURCE), _) => {}
            (Some(MOB_COMPOSITION), Some(0)) => retain_item(&mut summary.items,source,Item::Sequence(
                compile_sequence_inner(&ctx, bin, mob, &mut cache,
                    source.map(|source| source.sequence_capture(mob, item_index))))),
            (Some(MOB_COMPOSITION), Some(4)) if source.is_some() =>
                retain_item(&mut summary.items, source, Item::Group(source.unwrap().group(&ctx, mob, item_index))),
            (Some(MOB_COMPOSITION), Some(5)) if source.is_some() =>
                retain_item(&mut summary.items, source, Item::GroupDependency { id: mob_id(mob), name }),
            (Some(MOB_COMPOSITION), Some(6))
                if mob_id(mob).is_some_and(|id| freeze_owners.contains(&id)) => {}
            (Some(MOB_COMPOSITION), usage) => retain_item(&mut summary.items,source,Item::Skipped(Skipped {
                name,
                reason: format!("composition usage {:?} (subclip/group clip) as a bin item is not converted yet", usage),
            })),
            (other, _) => retain_item(&mut summary.items,source,Item::Skipped(Skipped {
                name,
                reason: format!("mob type {:?}", other),
            })),
        }
    }
    for item in &mut summary.items {
        if let Item::Master { id: Some(id), source: Some(meta), .. } = item {
            meta.rendered_gain = cache.gain_renders.get(id).copied();
        }
    }
    summary
}

#[derive(Default)]
struct Project {
    out: BinContents,
    routes: HashMap<MobId, MobRoute>,
    seen_masters: std::collections::HashSet<MobId>,
    sequences: Vec<PendingSequence>,
}
impl Project {
    fn push(&mut self, summary: BinSummary) {
        for (id, route) in summary.routes {
            self.routes.entry(id).or_insert(route);
        }
        for item in summary.items {
            match item {
                Item::Master { id, bin, name, result, .. } => {
                    if let Some(id) = id {
                        if !self.seen_masters.insert(id) {
                            self.out.duplicates.push((name.clone(), bin));
                        }
                    }
                    match result {
                        Ok(m) => self.out.masters.push(m),
                        Err(reason) => self.out.skipped.push(Skipped { name, reason }),
                    }
                }
                Item::Sequence(seq) => self.sequences.push(seq),
                Item::Group(_) => unreachable!("group facts are enabled only for source extraction"),
                Item::GroupDependency { .. } => unreachable!("group facts are enabled only for source extraction"),
                Item::Skipped(item) => self.out.skipped.push(item),
            }
        }
    }
    fn finish(mut self) -> BinContents {
        // Input/bin-item order, not hash-map iteration, determines project output order.
        for seq in self.sequences {
            self.out.sequences.push(seq.finish(&self.routes));
        }
        self.out
    }
}

#[derive(Debug)]
struct Query {
    id: MobId,
    label: i64,
    offset: i64,
    length: i64,
    record: i64,
    via: Vec<String>,
    depth: usize,
    kind: i64,
    track: i64,
}
#[derive(Debug)]
enum Op {
    Source(Query),
    Motion { query: Query, length: i64, ratio: (i64, i64) },
    Problem { position: i64, kind: i64, track: i64, message: String },
    PanVolume {
        before: usize, gain: Option<f64>, pans: [Option<f64>; 2],
        automation: Option<volume::Capture>,
    },
    Color { before: usize, value: Arc<ColorCorrection> },
    Picture { before: usize, value: Arc<PictureEffect> },
    PictureExtension { before: usize, capture: picture_extension::Capture },
    PictureChain { before: usize, chain: picture_extension::OrderedChain },
    Matte { before: usize, length: i64, value: Arc<AniMatteData> },
    Combined { before: usize, group: usize, channel: usize },
}
#[derive(Debug)]
struct PendingSequence {
    info: SequenceInfo,
    ops: Vec<Op>,
    source: Option<source_capture::Capture>,
}
fn problem(info: &mut SequenceInfo, kind: i64, track: i64, position: i64, message: String) {
    info.problems.push(format!("{} at frame {}: {}", kind_label(kind, track), position, message));
}
impl PendingSequence {
    fn finish(self, routes: &HashMap<MobId, MobRoute>) -> SequenceInfo {
        self.finish_with_starts(routes).0
    }
    fn finish_with_starts(self, routes: &HashMap<MobId, MobRoute>) -> (SequenceInfo, Vec<usize>) {
        let Self { mut info, ops, .. } = self;
        // A modifier's `before` is an instruction index, not a guessed event index.
        // Its child may resolve to zero, one or many events. Recording the event count
        // at every instruction also handles an empty child (before == modifier index).
        let mut starts = Vec::with_capacity(ops.len());
        for op in ops {
            starts.push(info.events.len());
            match op {
                Op::Source(q) => {
                    let (events, errors) = Resolver::query(routes, &q);
                    info.events.extend(events);
                    for (at, e) in errors {
                        problem(&mut info, q.kind, q.track, at, e);
                    }
                }
                Op::Motion { query: q, length, ratio } => {
                    let (mut events, errors) = Resolver::query(routes, &q);
                    if events.len() != 1 || !errors.is_empty() {
                        let what = errors.into_iter().map(|(_, e)| e).collect::<Vec<_>>().join("; ");
                        problem(
                            &mut info,
                            q.kind,
                            q.track,
                            q.record,
                            format!("motion effect source spans {} pieces {}", events.len(), what),
                        );
                    } else {
                        let mut e = events.pop().unwrap();
                        e.source_length = q.length;
                        e.length = length;
                        e.speed = Some(ratio);
                        info.events.push(e);
                    }
                }
                Op::Problem {
                    position,
                    kind,
                    track,
                    message,
                } => problem(&mut info, kind, track, position, message),
                Op::PanVolume { before, gain, pans, .. } => {
                    for e in &mut info.events[starts[before]..] {
                        e.edit_gain_db = gain;
                        e.edit_pan = if matches!(e.combined, Some((_, 1))) { pans[1] } else { pans[0] };
                    }
                }
                Op::Color { before, value } => {
                    for e in &mut info.events[starts[before]..] {
                        e.color = Some((*value).clone());
                    }
                }
                Op::Picture { before, value } => {
                    for e in &mut info.events[starts[before]..] {
                        if e.picture.is_some() {
                            e.picture_nested = true;
                        } else {
                            e.picture = Some((*value).clone());
                        }
                    }
                }
                Op::PictureExtension { .. } | Op::PictureChain { .. } => {}
                Op::Matte { before, length, value } => {
                    let produced = info.events.len() - starts[before];
                    for e in &mut info.events[starts[before]..] {
                        if produced == 1 && e.length == length && e.animatte.is_none() {
                            e.animatte = Some((*value).clone());
                        } else {
                            e.via.push("AniMatte over more than one edit not converted".into());
                        }
                    }
                }
                Op::Combined { before, group, channel } => {
                    for e in &mut info.events[starts[before]..] {
                        e.combined = Some((group, channel));
                    }
                }
            }
        }
        starts.push(info.events.len());
        (info, starts)
    }
}

struct CompileWalk<'s, 'a> {
    ctx: &'s Ctx<'a>,
    info: &'s mut SequenceInfo,
    ops: &'s mut Vec<Op>,
    cache: &'s mut CompileCache,
    kind: i64,
    track: i64,
    source: Option<&'s mut source_capture::Capture>,
}
impl<'s, 'a> CompileWalk<'s, 'a> {
    fn push_op(&mut self, op: Op) {
        if let Some(source) = &mut self.source {
            if !source.prepare_op(&op, self.ops) { return; }
        }
        self.ops.push(op);
    }
    fn problem(&mut self, position: i64, message: String) {
        self.push_op(Op::Problem {
            position,
            kind: self.kind,
            track: self.track,
            message,
        });
    }
    fn component(&mut self, c: &'a Node, position: i64, via: &[String], depth: usize) {
        let before = self.ops.len();
        if let Some(source) = &mut self.source {
            if source.failed() { return; }
            source.enter(self.ctx, c, position, self.kind, self.track, self.ops.len());
            if source.failed() { return; }
        }
        self.component_inner(c, position, via, depth);
        if self.source.as_ref().is_some_and(|s| !s.failed()) && self.kind == KIND_PICTURE
            && matches!(self.ops.last(), Some(Op::Picture { before: start, .. } | Op::PictureExtension { before: start, .. }) if *start == before)
        {
            if let Some(chain) = picture_extension::ordered_chain(self.ctx, c, &self.ops[before..], before, position) {
                self.push_op(Op::PictureChain { before, chain });
            }
        }
        if let Some(source) = &mut self.source { source.account_ops(self.ops); source.leave(self.ops.len()); }
    }
    fn component_inner(&mut self, c: &'a Node, position: i64, via: &[String], depth: usize) {
        if depth > MAX_NESTING {
            return self.problem(position, "nesting too deep".into());
        }
        let ctx = self.ctx;
        let length = int(c, "length").unwrap_or(0);
        // Markers on this component, at timeline frames. Effects and groups pass their own
        // position to their inputs, so nested owners land at the same frames.
        let owner = if &c.class == b"FILL" { MarkerOwner::Filler } else { MarkerOwner::Edit };
        if self.source.is_none() { for marker in ctx.markers(c, position) {
            self.info.markers.push(SeqMarker {
                owner: owner.clone(),
                kind: self.kind,
                track: self.track,
                marker,
            });
        }}
        match &c.class {
            b"SEQU" => {
                let mut pos = position;
                let mut prior_owner = None;
                let mut pending_transitions = Vec::new();
                if let Some(Value::RefList(children)) = c.get("components") {
                    for child in children {
                        match child.as_index().and_then(|i| ctx.file.get(i)) {
                            Some(crate::file::Object::Decoded(n)) => {
                                let len = int(n, "length").unwrap_or(0);
                                if &n.class == b"TNFX" {
                                    if self.source.is_some() {
                                        let Some(next) = pos.checked_sub(len) else {
                                            self.source.as_ref().unwrap().fail("native transition position overflows"); return;
                                        }; pos = next;
                                    } else { pos -= len; }
                                    if int(n, "length").is_none() {
                                        if let Some(source) = &mut self.source { source.invalidate_position("transition length is absent"); }
                                    }
                                    if let Some(source) = &mut self.source {
                                        let transition = source.transition(n, pos, self.kind, self.track, prior_owner.clone());
                                        pending_transitions.push(transition);
                                    }
                                    if self.source.is_none(){ self.info.transitions.push(SeqTransition {
                                        kind: self.kind,
                                        track: self.track,
                                        position: pos,
                                        length: len,
                                        cutpoint: int(n, "cutpoint").unwrap_or(0),
                                        effect: n.get("effect_id").and_then(Value::as_str).map(str::to_string),
                                    }); }
                                } else {
                                    let incoming = self.source.as_ref().map(|s| s.next_owner());
                                    self.component(n, pos, via, depth + 1);
                                    if let Some(source) = &mut self.source {
                                        for transition in pending_transitions.drain(..) {
                                            source.bind_incoming(transition, incoming.clone());
                                        }
                                    }
                                    prior_owner = incoming;
                                    if int(n, "length").is_none() {
                                        if let Some(source) = &mut self.source { source.invalidate_position("component length is absent"); }
                                    }
                                    if self.source.is_some() {
                                        let Some(next)=pos.checked_add(len) else {
                                            self.source.as_ref().unwrap().fail("native component position overflows"); return;
                                        };pos=next;
                                    }else{pos+=len;}
                                }
                            }
                            Some(crate::file::Object::Unsupported { class, .. }) => {
                                // Its length is unknown, so later positions on this track are too.
                                let class = String::from_utf8_lossy(class).into_owned();
                                return self.problem(pos, format!("{} not decoded; rest of track skipped", class));
                            }
                            None => {}
                        }
                    }
                }
            }
            b"FILL" => {}
            b"SCLP" => {
                if length <= 0 {
                    return;
                }
                let Some(id) = mob_id(c) else { return };
                if id.material.data1 == 0 && id.label == [0; 12] {
                    return; // null reference (e.g. original-source end of chain)
                }
                if let Some(source) = &mut self.source {
                    source.query(self.ops.len(), c, None, None);
                    if source.failed() { return; }
                }
                self.push_op(Op::Source(Query {
                    id,
                    label: int(c, "track_id").unwrap_or(0),
                    offset: int(c, "start_time").unwrap_or(0),
                    length,
                    record: position,
                    via: via.to_vec(),
                    depth,
                    kind: self.kind,
                    track: self.track,
                }));
            }
            b"SLCT" => {
                let tracks = ctx.tracks(c);
                let selected = int(c, "selected").unwrap_or(0) as usize;
                match tracks.get(selected).and_then(|t| ctx.deref(t.get("component"))) {
                    Some(inner) => {
                        let mut via = via.to_vec();
                        via.push(format!("group clip angle {}", selected + 1));
                        self.component(inner, position, &via, depth + 1)
                    }
                    None => self.problem(position, "group clip with no selected angle".into()),
                }
            }
            b"SPED" => {
                let ratio = match c.get("speed_ratio") {
                    Some(Value::List(v)) if v.len() == 2 => (v[0].as_int().unwrap_or(0), v[1].as_int().unwrap_or(0)),
                    _ => (0, 0),
                };
                if let Some(source) = &mut self.source {
                    if freeze::candidate(ctx, c, ratio) {
                        if let Some((_, id)) = freeze::sequence_render(ctx, c, &self.cache.freezes)
                            .filter(|(render, _)| self.kind == KIND_PICTURE && source.query_rendered_freeze(self.ops.len(), render))
                        {
                            self.push_op(Op::Source(Query {
                                id, label: 1, offset: 0, length, record: position,
                                via: via.to_vec(), depth, kind: self.kind, track: self.track,
                            }));
                        } else {
                            source.unresolved_motion(position, self.kind, self.track,
                                "Avid Freeze Frame has no qualified full generated picture binding");
                        }
                        return;
                    }
                }
                // An automatic Motion Adapter may carry phase and sampling
                // state beyond the stored span ratio. Do not treat it as an
                // ordinary constant speed effect in the qualified source path.
                if let Some(source) = &mut self.source {
                    if let Some(attrs) = source_capture::automatic_adapter_attrs(ctx, c) {
                        if let Some((input, id, source_in, phase, source_length, label, ratio)) =
                            source_capture::qualified_automatic_adapter(ctx, c, attrs, self.kind, length)
                        {
                            source.query(self.ops.len(), input, Some(ratio), Some(phase));
                            if source.failed() { return; }
                            self.push_op(Op::Motion {
                                query: Query { id, label, offset: source_in, length: source_length,
                                    record: position, via: via.to_vec(), depth, kind: self.kind, track: self.track },
                                length, ratio,
                            });
                        } else {
                            source.unresolved_motion_adapter(position, self.kind, self.track);
                        }
                        return;
                    }
                }
                if let Some(source) = &mut self.source {
                    let sparse = source_capture::uses_editable_position(ctx, c);
                    if sparse || source_capture::is_sampled_effect(ctx, c) {
                        let input = source_capture::timewarp_input(ctx, c);
                        let route = input.and_then(|n| Some((n, mob_id(n)?,
                            int(n, "track_id")?, int(n, "start_time")?, int(n, "length")?)));
                        let Some((input, id, label, offset, source_length)) = route else {
                            source.unresolved_motion(position, self.kind, self.track,
                                "Avid Timewarp picture input does not resolve to one direct source");
                            return;
                        };
                        if source.query_sampled_timewarp(ctx, self.ops.len(), c, input, ratio,
                            position, self.kind, self.track, sparse)
                        {
                            self.push_op(Op::Motion {
                                query: Query { id, label, offset, length: source_length, record: position,
                                    via: via.to_vec(), depth, kind: self.kind, track: self.track },
                                length, ratio,
                            });
                        }
                        return;
                    }
                }
                let inputs: Vec<&Node> = ctx.tracks(c).into_iter().filter_map(|t| ctx.deref(t.get("component"))).collect();
                let [input] = inputs.as_slice() else {
                    return self.problem(position, format!("motion effect with {} inputs not converted", inputs.len()));
                };
                // The input is a SourceClip, or a Sequence holding one plus zero-length filler.
                let input: &Node = if &input.class == b"SEQU" {
                    let parts: Vec<&Node> = ctx
                        .segments(input)
                        .into_iter()
                        .map(|(_, n)| n)
                        .filter(|n| !(&n.class == b"FILL" && int(n, "length").unwrap_or(0) == 0))
                        .collect();
                    match parts.as_slice() {
                        [one] => one,
                        _ => return self.problem(position, format!("motion effect over {} segments not converted", parts.len())),
                    }
                } else {
                    input
                };
                if &input.class != b"SCLP" || ratio.1 == 0 {
                    return self.problem(position, format!("motion effect over {} not converted", input.class_str()));
                }
                let source_length = int(input, "length").unwrap_or(0);
                let Some(id) = mob_id(input) else { return };
                let mut via = via.to_vec();
                if c.get("source_param_list").is_some() {
                    via.push("timewarp parameters present (ramps not decoded; constant speed assumed)".into());
                }
                if let Some(source) = &mut self.source {
                    source.query(self.ops.len(), input, Some(ratio), None);
                    if source.failed() { return; }
                }
                self.push_op(Op::Motion {
                    query: Query {
                        id,
                        label: int(input, "track_id").unwrap_or(0),
                        offset: int(input, "start_time").unwrap_or(0),
                        length: source_length,
                        record: position,
                        via,
                        depth,
                        kind: self.kind,
                        track: self.track,
                    },
                    length,
                    ratio,
                });
            }
            b"ASPI" if self.source.is_some() && self.kind == KIND_SOUND && gain::candidate(c) => {
                let source = self.source.as_mut().unwrap();
                if let Some((render, id, origin)) = gain::binding(ctx, c, &self.cache.ambiguous)
                    .filter(|(render, _, _)| source.query_rendered_gain(self.ops.len(), render))
                {
                    self.cache.gain_renders.insert(id, origin);
                    self.push_op(Op::Source(Query {
                        id, label: int(render, "track_id").unwrap(), offset: 0,
                        length, record: position, via: via.to_vec(), depth,
                        kind: self.kind, track: self.track,
                    }));
                } else {
                    source.unresolved_motion(position, self.kind, self.track,
                        "AudioSuite Gain needs a matching full mono PCM24 render on a 24 fps edit");
                }
            }
            b"TKFX" | b"PVOL" => {
                let effect = c.get("effect_id").and_then(Value::as_str).unwrap_or(c.type_name).to_string();
                let tracks = ctx.tracks(c);
                let mut inputs: Vec<&Node> = tracks.iter().filter_map(|t| ctx.deref(t.get("component"))).collect();
                if self.source.is_some() && self.kind == KIND_PICTURE && text::kind(c).is_some() {
                    // The source capture retains the text occurrence independently of
                    // the media edit beneath it (including a filler-only title).
                    match inputs.as_slice() {
                        [input] => self.component(input, position, via, depth + 1),
                        [] => {},
                        many if many.iter().all(|input| ctx.is_empty(input)) => {
                            for input in many { self.component(input, position, via, depth + 1); }
                        },
                        _ => self.problem(position, "text effect has unsupported multiple picture inputs".into()),
                    }
                    return;
                }
                if &c.class == b"PVOL" && self.kind == KIND_SOUND {
                    let automation = self.source.as_mut().and_then(|source| {
                        source.capture_volume(ctx, c, &tracks, position)
                    });
                    if automation.is_some() {
                        inputs = tracks.iter().filter_map(|track| {
                            let input = ctx.deref(track.get("component"))?;
                            let control = input.class == *b"CTRL"
                                && matches!(int(track, "control_code"), Some(1000 | 2000));
                            (!control).then_some(input)
                        }).collect();
                    }
                    if let [input] = inputs.as_slice() {
                        // The edit's own volume/pan: annotate every event made under it. For a
                        // stereo edit (PVOL over a channel combiner) the PVOL pan is channel 1's
                        // and the attribute _PAN_1_1 holds channel 2's (KB AVB-0005, Phase 21).
                        let gain = (c.get("level_set") == Some(&Value::Bool(true)))
                            .then(|| int(c, "level").filter(|l| *l > 0).map(|l| 20.0 * (l as f64 / (1u64 << 29) as f64).log10()))
                            .flatten();
                        let pan_set = c.get("pan_set") == Some(&Value::Bool(true));
                        let pan1 = pan_set.then(|| int(c, "pan").map(pan_percent)).flatten();
                        let pan2 = pan_set
                            .then(|| match ctx.deref(c.get("attributes")).and_then(|a| a.get("entries")) {
                                Some(Value::Attrs(e)) => e.iter().find(|(k, _)| k == "_PAN_1_1").and_then(|(_, v)| v.as_int()).map(pan_percent),
                                _ => None,
                            })
                            .flatten();
                        let before = self.ops.len();
                        self.component(input, position, via, depth + 1);
                        self.push_op(Op::PanVolume {
                            before,
                            gain,
                            pans: [pan1, pan2],
                            automation,
                        });
                        return;
                    }
                }
                if effect == "EFF2_RGB_COLOR_CORRECTION" && self.kind == KIND_PICTURE {
                    if let [input] = inputs.as_slice() {
                        let cc = self
                            .cache
                            .colors
                            .entry(c as *const Node as usize)
                            .or_insert_with(|| Arc::new(color_params(ctx, c)))
                            .clone();
                        let before = self.ops.len();
                        self.component(input, position, via, depth + 1);
                        self.push_op(Op::Color { before, value: cc });
                        return;
                    }
                }
                let picture_kind = match effect.as_str() {
                    "EFF2_BLEND_RESIZE" => Some(PictureKind::Resize),
                    "EFF2_SBLEND"
                        if match ctx.deref(c.get("attributes")).and_then(|a| a.get("entries")) {
                            Some(Value::Attrs(e)) => e.iter().any(|(k, v)| k == "_EFFECT_PLUGIN_NAME" && v.as_str() == Some("3DWarp")),
                            _ => false,
                        } =>
                    {
                        Some(PictureKind::Warp)
                    }
                    _ => None,
                };
                if self.source.is_some() && self.kind == KIND_PICTURE {
                    if let (Some(Value::List(tracks)), [input]) =
                        (c.get("tracks"), inputs.as_slice())
                    {
                        if let Some(flip) = picture_extension::flip(&effect).filter(|_| tracks.len() == 1) {
                            let before = self.ops.len();
                            self.component(input, position, via, depth + 1);
                            self.push_op(Op::PictureExtension {
                                before, capture: picture_extension::Capture::new(c, position, flip),
                            });
                            return;
                        }
                    }
                }
                if let (Some(kind), true) = (picture_kind, self.kind == KIND_PICTURE) {
                    // 3D Warp: track 1 is the background (filler showing the track below), track 2
                    // the picture it moves; only an empty background is handled.
                    let empty = |n: &Node| ctx.is_empty(n);
                    let (input, ok) = match (kind, inputs.as_slice()) {
                        (PictureKind::Resize, [one]) => (Some(*one), true),
                        (PictureKind::Warp, [background, picture]) => (Some(*picture), empty(background)),
                        _ => (None, false),
                    };
                    if let (Some(input), true) = (input, ok) {
                        let fx = self
                            .cache
                            .pictures
                            .entry(c as *const Node as usize)
                            .or_insert_with(|| {
                                Arc::new(PictureEffect {
                                    kind,
                                    params: color_params(ctx, c),
                                })
                            })
                            .clone();
                        let before = self.ops.len();
                        self.component(input, position, via, depth + 1);
                        self.push_op(Op::Picture { before, value: fx });
                        if self.source.is_some() {
                            let animated = match kind {
                                PictureKind::Resize => picture_extension::resize(ctx, c),
                                PictureKind::Warp => picture_extension::warp(ctx, c),
                            };
                            if let Some(effect) = animated {
                                self.push_op(Op::PictureExtension {
                                    before, capture: picture_extension::Capture::new(c, position, effect),
                                });
                            }
                        }
                        return;
                    }
                }
                if effect == "EFF_ANIMATTE" && self.kind == KIND_PICTURE {
                    // Track 1 is the background, track 2 the picture the matte cuts out (FilmCraft
                    // PR #1056 routing guard); only an empty background and unreversed effect.
                    // Nonzero info_smooth is an observed Acceleration value. The mask geometry
                    // survives, but its easing is explicitly reported as unconverted.
                    let reference = |k: &str| match c.get(k) {
                        Some(Value::Ref(i)) => Some(*i),
                        None | Some(Value::Null) => Some(0),
                        _ => None,
                    };
                    let acceleration = int(c, "info_smooth").and_then(|v| u32::try_from(v).ok());
                    let simple = int(c, "info_is_reversed") == Some(0);
                    if let ([background, picture], true, Some(keyframes), Some(manager), Some(acceleration)) =
                        (inputs.as_slice(), simple, reference("keyframes"), reference("trackman"), acceleration)
                    {
                        let frames = u32::try_from(length).unwrap_or(0);
                        if ctx.is_empty(background) {
                            let data = self
                                .cache
                                .mattes
                                .entry((keyframes, manager, frames, acceleration))
                                .or_insert_with(|| animatte(ctx.file, keyframes, manager, frames, acceleration).map(Arc::new))
                                .clone();
                            if let Some(data) = data {
                                let before = self.ops.len();
                                self.component(picture, position, via, depth + 1);
                                self.push_op(Op::Matte { before, length, value: data });
                                return;
                            }
                        }
                    }
                }
                if effect == "EFF2_AUDIO_CHANNEL_COMBINER" && self.kind == KIND_SOUND && inputs.len() > 1 {
                    // One multichannel edit: each input track is one channel, in order.
                    let group = self.info.combiner_count;
                    self.info.combiner_count += 1;
                    for (channel, input) in inputs.iter().enumerate() {
                        let before = self.ops.len();
                        self.component(input, position, via, depth + 1);
                        self.push_op(Op::Combined { before, group, channel });
                    }
                    return;
                }
                match inputs.as_slice() {
                    [input] => {
                        let mut via = via.to_vec();
                        via.push(if &c.class == b"PVOL" {
                            format!(
                                "clip volume/pan (level {}, pan {}) not converted",
                                int(c, "level").unwrap_or(0),
                                int(c, "pan").unwrap_or(0)
                            )
                        } else {
                            format!("effect {} not converted", effect)
                        });
                        self.component(input, position, &via, depth + 1)
                    }
                    [] => self.problem(position, format!("effect {} with no input clip ({} frames) not converted", effect, length)),
                    many => self.problem(position, format!("effect {} with {} inputs not converted", effect, many.len())),
                }
            }
            other => {
                let class = String::from_utf8_lossy(other).into_owned();
                self.problem(position, format!("{} ({} frames) not converted", class, length));
            }
        }
    }
}
#[cfg(test)]
fn compile_sequence(ctx: &Ctx<'_>, f: usize, mob: &Node, cache: &mut CompileCache) -> PendingSequence {
    compile_sequence_inner(ctx, f, mob, cache, None)
}
fn compile_sequence_inner(ctx: &Ctx<'_>, f: usize, mob: &Node, cache: &mut CompileCache,
    mut source: Option<source_capture::Capture>) -> PendingSequence {
    let mut ops = Vec::new();
    let mut info = SequenceInfo {
        name: mob.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        mob_id: mob_id(mob),
        bin: f,
        edit_rate: float(mob, "edit_rate").unwrap_or(0.0),
        length: int(mob, "length").unwrap_or(0),
        start: None,
        events: Vec::new(),
        transitions: Vec::new(),
        problems: Vec::new(),
        audio_track_formats: Vec::new(),
        markers: Vec::new(),
        combiner_count: 0,
    };
    if let Some(source) = &mut source { source.sequence_markers(ctx, mob); }
    if source.is_none() { for marker in ctx.markers(mob, 0) {
        info.markers.push(SeqMarker {
            owner: MarkerOwner::Sequence,
            kind: 0,
            track: 0,
            marker,
        });
    }}
    let mut tc_index = i64::MAX;
    for (track_ordinal, track) in ctx.tracks(mob).into_iter().enumerate() {
        let index = int(track, "index").unwrap_or(0);
        let component = match ctx.deref(track.get("component")) {
            Some(c) => c,
            None => continue, // MASK/ECCP and other undecoded track kinds
        };
        match int(component, "media_kind_id") {
            Some(KIND_TIMECODE) => {
                if let Some(source) = &mut source {
                    source.timecode_track(ctx, track, component, index, track_ordinal, int(mob, "length"));
                }
                // Sequence start: the first TCCP on the lowest-labelled timecode track.
                if index < tc_index {
                    if let Some((_, tc)) = ctx.segments(component).into_iter().find(|(_, c)| &c.class == b"TCCP") {
                        tc_index = index;
                        info.start = Some(Timecode {
                            frames: int(tc, "start").unwrap_or(0),
                            fps: int(tc, "fps").unwrap_or(0),
                            drop: int(tc, "flags").unwrap_or(0) & 1 != 0,
                        });
                    }
                }
            }
            Some(kind @ (KIND_PICTURE | KIND_SOUND)) => {
                if kind == KIND_SOUND {
                    let format = match ctx.deref(track.get("attributes")).and_then(|a| a.get("entries")) {
                        Some(Value::Attrs(e)) => e.iter().find(|(k, _)| k == "_TRACK_FORMAT").and_then(|(_, v)| v.as_int()).unwrap_or(1),
                        _ => 1,
                    };
                    if source.as_mut().is_none_or(|source|source.reserve_track_format(&mut info.audio_track_formats)){
                        info.audio_track_formats.push((index,format));
                    }
                }
                let mut w = CompileWalk {
                    ctx,
                    info: &mut info,
                    ops: &mut ops,
                    cache,
                    kind,
                    track: index,
                    source: source.as_mut(),
                };
                w.component(component, 0, &[], 0);
            }
            _ => {}
        }
    }
    PendingSequence { info, ops, source }
}

struct Resolver<'s> {
    all: &'s HashMap<MobId, MobRoute>,
    kind: i64,
    track: i64,
}
impl<'s> Resolver<'s> {
    fn query(all: &'s HashMap<MobId, MobRoute>, q: &Query) -> (Vec<SeqEvent>, Vec<(i64, String)>) {
        let resolver = Self {
            all,
            kind: q.kind,
            track: q.track,
        };
        let (mut pieces, mut errors) = (Vec::new(), Vec::new());
        resolver.resolve(q.id, q.label, q.offset, q.length, q.record, q.via.clone(), q.depth, &mut pieces, &mut errors);
        (pieces, errors)
    }
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        &self,
        id: MobId,
        track_label: i64,
        offset: i64,
        length: i64,
        record: i64,
        via: Vec<String>,
        depth: usize,
        pieces: &mut Vec<SeqEvent>,
        errors: &mut Vec<(i64, String)>,
    ) {
        if depth > MAX_NESTING {
            return errors.push((record, "source chain too deep".into()));
        }
        let Some(mob) = self.all.get(&id) else {
            return errors.push((record, format!("source clip {} is in none of the bins given", id.urn())));
        };
        match mob.kind {
            Some(MOB_MASTER) => pieces.push(SeqEvent {
                kind: self.kind,
                track: self.track,
                record_start: record,
                length,
                master: id,
                master_track: track_label,
                source_in: offset,
                via,
                speed: None,
                source_length: length,
                combined: None,
                edit_gain_db: None,
                edit_pan: None,
                color: None,
                picture: None,
                picture_nested: false,
                animatte: None,
            }),
            Some(MOB_COMPOSITION) => {
                let label = match mob.usage {
                    Some(2) => format!("subclip {:?}", mob.name),
                    Some(4) => format!("group clip {:?}", mob.name),
                    other => format!("composition {:?} (usage {:?})", mob.name, other),
                };
                let Some(track) = mob.tracks.iter().find(|t| t.label == Some(track_label) && t.kind == Some(self.kind)) else {
                    return errors.push((record, format!("{} has no {}", label, kind_label(self.kind, track_label))));
                };
                let mut via = via;
                via.push(label.clone());
                for seg in track.segments.iter() {
                    let pos = seg.position;
                    let lo = offset.max(pos);
                    let hi = (offset + length).min(pos + seg.length);
                    if lo >= hi {
                        continue;
                    }
                    let at = record + (lo - offset);
                    match seg.action.as_ref() {
                        RouteAction::Empty => {}
                        RouteAction::Source {
                            id: Some(inner),
                            label: track,
                            start,
                            ..
                        } => self.resolve(*inner, *track, *start + (lo - pos), hi - lo, at, via.clone(), depth + 1, pieces, errors),
                        RouteAction::Source { id: None, .. } => errors.push((at, format!("{} segment has no mob ID", label))),
                        RouteAction::Selected { ordinal, inner } => match inner.as_deref() {
                            Some(RouteAction::Source {
                                id: Some(m),
                                label: track,
                                start,
                                ..
                            }) => {
                                let mut v = via.clone();
                                v.push(format!("angle {}", ordinal + 1));
                                self.resolve(*m, *track, *start + (lo - pos), hi - lo, at, v, depth + 1, pieces, errors);
                            }
                            Some(RouteAction::Source { id: None, .. }) => errors.push((at, format!("{} angle has no mob ID", label))),
                            Some(RouteAction::Other(class)) => errors.push((at, format!("{} angle is {} (not converted)", label, class))),
                            None => errors.push((at, format!("{} has no selected angle", label))),
                            _ => unreachable!("selected routes only compile sources or class names"),
                        },
                        RouteAction::Other(class) => errors.push((at, format!("{} segment is {} (not converted)", label, class))),
                    }
                }
            }
            other => errors.push((record, format!("source clip is mob type {:?}, not a master clip", other))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::AvbDocument;
    use crate::file::Object;
    use std::cell::Cell;
    use std::rc::Rc;

    fn node(class: [u8; 4], type_name: &'static str) -> Node {
        Node {
            class,
            type_name,
            fields: Vec::new(),
        }
    }
    trait Put {
        fn put(&mut self, key: &'static str, value: Value);
    }
    impl Put for Node {
        fn put(&mut self, key: &'static str, value: Value) {
            self.fields.push((key, value));
        }
    }

    fn id(n: u32) -> MobId {
        MobId::from_urn(&format!(
            "urn:smpte:umid:060a2b34.01010101.01010f20.13010203.{n:08x}.9abcdef0.11223344.55667788"
        ))
        .unwrap()
    }
    fn file() -> AvbFile {
        AvbFile::parse(&AvbDocument::new_empty_bin(19, "Synthetic streaming", 1, false).to_bytes().unwrap()).unwrap()
    }
    fn add(f: &mut AvbFile, node: Node) -> u32 {
        let index = f.objects.len() as u32;
        f.objects.push(Some(Object::Decoded(node)));
        index
    }
    fn item(f: &mut AvbFile, index: u32) {
        let root = match f.objects[f.root_index as usize].as_mut().unwrap() {
            Object::Decoded(n) => n,
            _ => unreachable!(),
        };
        let items = root.fields.iter_mut().find(|(k, _)| *k == "items").unwrap();
        let Value::List(items) = &mut items.1 else { unreachable!() };
        let mut entry = node([0; 4], "BinItem");
        entry.put("mob", Value::Ref(index));
        items.push(Value::Node(Box::new(entry)));
    }
    fn source(f: &mut AvbFile, target: MobId, kind: i64, label: i64, start: i64, length: i64) -> u32 {
        let mut n = node(*b"SCLP", "SourceClip");
        n.put("length", Value::Int(length));
        n.put("media_kind_id", Value::Int(kind));
        n.put("mob_id", Value::MobId(target));
        n.put("track_id", Value::Int(label));
        n.put("start_time", Value::Int(start));
        add(f, n)
    }
    fn mob(f: &mut AvbFile, identity: MobId, name: &str, kind: i64, usage: i64, tracks: &[(i64, u32)]) -> u32 {
        // Real masters need a local file mob and a supported essence track.
        let generated;
        let tracks = if kind == MOB_MASTER && tracks.is_empty() {
            let file_id = id(identity.material.data1 + 1000);
            mob(f, file_id, "Synthetic file mob", MOB_SOURCE, 0, &[]);
            let clip = source(f, file_id, KIND_PICTURE, 1, 0, 100);
            generated = [(1, clip)];
            &generated[..]
        } else {
            tracks
        };
        let mut n = node(*b"CMPO", "Composition");
        n.put("name", Value::Str(name.into()));
        n.put("mob_id", Value::MobId(identity));
        n.put("mob_type_id", Value::Int(kind));
        n.put("usage_code", Value::Int(usage));
        n.put("length", Value::Int(100));
        n.put("edit_rate", Value::Float(25.0));
        n.put(
            "tracks",
            Value::List(
                tracks
                    .iter()
                    .map(|(label, c)| {
                        let mut track = node([0; 4], "Track");
                        track.put("index", Value::Int(*label));
                        track.put("component", Value::Ref(*c));
                        Value::Node(Box::new(track))
                    })
                    .collect(),
            ),
        );
        let index = add(f, n);
        item(f, index);
        index
    }
    fn project(files: Vec<AvbFile>) -> BinContents {
        let mut p = Project::default();
        for (bin, f) in files.into_iter().enumerate() {
            let summary = compile_bin(&f, bin);
            drop(f);
            p.push(summary);
        }
        p.finish()
    }
    #[test]
    fn forward_cross_bin_subclip_keeps_source_offset_and_track_namespace() {
        let mut a = file();
        let v = source(&mut a, id(2), KIND_PICTURE, 1, 2, 10);
        let s = source(&mut a, id(1), KIND_SOUND, 1, 7, 10);
        mob(&mut a, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, v), (1, s)]);
        let mut b = file();
        let c = source(&mut b, id(1), KIND_PICTURE, 1, 40, 100);
        mob(&mut b, id(2), "Subclip", MOB_COMPOSITION, 2, &[(1, c)]);
        let mut c = file();
        mob(&mut c, id(1), "Master", MOB_MASTER, 0, &[]);
        let out = project(vec![a, b, c]);
        let seq = &out.sequences[0];
        assert!(seq.problems.is_empty());
        assert_eq!(seq.events.len(), 2);
        assert_eq!((seq.events[0].kind, seq.events[0].source_in), (KIND_PICTURE, 42));
        assert_eq!((seq.events[1].kind, seq.events[1].source_in), (KIND_SOUND, 7));
        assert_eq!(out.masters[0].bin, 2);
    }
    #[test]
    fn first_project_route_and_separate_master_copies_are_preserved() {
        let mut a = file();
        let s = source(&mut a, id(2), KIND_PICTURE, 1, 0, 10);
        mob(&mut a, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, s)]);
        let c = source(&mut a, id(1), KIND_PICTURE, 1, 4, 100);
        mob(&mut a, id(2), "First", MOB_COMPOSITION, 2, &[(1, c)]);
        mob(&mut a, id(1), "First master", MOB_MASTER, 0, &[]);
        let mut b = file();
        let c = source(&mut b, id(1), KIND_PICTURE, 1, 70, 100);
        mob(&mut b, id(2), "Later", MOB_COMPOSITION, 2, &[(1, c)]);
        mob(&mut b, id(1), "Later master", MOB_MASTER, 0, &[]);
        let out = project(vec![a, b]);
        assert_eq!(out.sequences[0].events[0].source_in, 4);
        assert_eq!(out.masters.len(), 2);
        assert_eq!(out.duplicates, vec![("Later master".into(), 1)]);
        assert_eq!(out.masters[0].name, "First master");
        assert_eq!(out.masters[1].name, "Later master");
    }
    #[test]
    fn missing_references_and_cycles_retain_diagnostics() {
        let mut a = file();
        let missing = source(&mut a, id(99), KIND_PICTURE, 1, 0, 10);
        let cycle = source(&mut a, id(2), KIND_SOUND, 1, 0, 10);
        mob(&mut a, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, missing), (1, cycle)]);
        mob(&mut a, id(2), "Cycle", MOB_COMPOSITION, 2, &[(1, cycle)]);
        let out = project(vec![a]);
        let seq = &out.sequences[0];
        assert!(seq.events.is_empty());
        assert!(seq.problems[0].contains("none of the bins"));
        assert!(seq.problems[1].contains("source chain too deep"));
    }
    #[test]
    fn modifiers_apply_to_resolved_events_not_source_queries() {
        let mut f = file();
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        let s = source(&mut f, id(1), KIND_PICTURE, 1, 0, 10);
        let seq_id = mob(&mut f, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, s)]);
        let ctx = Ctx {
            file: &f,
            mobs: HashMap::new(),
        };
        let mut seq = compile_sequence(&ctx, 0, f.node(seq_id).unwrap(), &mut CompileCache::default());
        seq.ops.push(Op::Combined {
            before: 0,
            group: 3,
            channel: 1,
        });
        seq.ops.push(Op::PanVolume {
            before: 0,
            gain: Some(-6.0),
            pans: [Some(-100.0), Some(25.0)],
            automation: None,
        });
        let mut routes = HashMap::new();
        routes.insert(
            id(1),
            MobRoute {
                rate: None,
                length: None,
                kind: Some(MOB_MASTER),
                name: String::new(),
                usage: None,
                tracks: vec![],
            },
        );
        let out = seq.finish(&routes);
        let e = &out.events[0];
        assert_eq!(e.combined, Some((3, 1)));
        assert_eq!(e.edit_gain_db, Some(-6.0));
        assert_eq!(e.edit_pan, Some(25.0));
    }
    #[test]
    fn summary_drops_irrelevant_payloads() {
        let mut f = file();
        let mut unused = node(*b"ATTR", "Attributes");
        unused.put(
            "entries",
            Value::Attrs(vec![("Synthetic unused payload".into(), Value::Bytes(vec![7; 4 << 20]))]),
        );
        add(&mut f, unused);
        let m = mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        if let Some(Object::Decoded(n)) = f.objects[m as usize].as_mut() {
            n.put("irrelevant", Value::Bytes(vec![8; 4 << 20]));
        }
        let summary = compile_bin(&f, 0);
        drop(f);
        assert!(format!("{summary:?}").len() < 4096, "unconsumed object payload escaped extraction");
    }
    fn segments(f: &mut AvbFile, kind: i64, children: &[u32]) -> u32 {
        let length = children.iter().map(|i| int(f.node(*i).unwrap(), "length").unwrap_or(0)).sum();
        let mut n = node(*b"SEQU", "Sequence");
        n.put("length", Value::Int(length));
        n.put("media_kind_id", Value::Int(kind));
        n.put("components", Value::RefList(children.iter().map(|i| Value::Ref(*i)).collect()));
        add(f, n)
    }
    #[test]
    fn shared_routes_are_memoized_within_a_bin() {
        let mut f = file();
        let source = source(&mut f, id(1), KIND_PICTURE, 1, 0, 10);
        let a = mob(&mut f, id(2), "One", MOB_COMPOSITION, 2, &[(1, source)]);
        let b = mob(&mut f, id(3), "Two", MOB_COMPOSITION, 2, &[(1, source)]);
        let ctx = Ctx {
            file: &f,
            mobs: HashMap::new(),
        };
        let mut cache = CompileCache::default();
        let a = route(&ctx, f.node(a).unwrap(), &mut cache);
        let b = route(&ctx, f.node(b).unwrap(), &mut cache);
        assert!(Arc::ptr_eq(&a.tracks[0].segments, &b.tracks[0].segments));
    }
    #[test]
    fn split_routes_keep_ranges_and_motion_cardinality_checks() {
        let mut f = file();
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        let first = source(&mut f, id(1), KIND_PICTURE, 1, 20, 5);
        let second = source(&mut f, id(1), KIND_PICTURE, 1, 30, 5);
        let parts = segments(&mut f, KIND_PICTURE, &[first, second]);
        mob(&mut f, id(2), "Subclip", MOB_COMPOSITION, 2, &[(1, parts)]);
        let edit = source(&mut f, id(2), KIND_PICTURE, 1, 3, 5);
        mob(&mut f, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, edit)]);
        let mut project = Project::default();
        project.push(compile_bin(&f, 0));
        drop(f);
        let mut sequence = project.sequences.pop().unwrap();
        sequence.ops.push(Op::PanVolume {
            before: 0,
            gain: Some(-6.0),
            pans: [None, None],
            automation: None,
        });
        // Both pieces of one source query receive the enclosing edit's settings.
        let info = sequence.finish(&project.routes);
        assert_eq!(
            info.events.iter().map(|e| (e.record_start, e.source_in, e.length)).collect::<Vec<_>>(),
            vec![(0, 23, 2), (2, 30, 3)]
        );
        assert!(info.events.iter().all(|e| e.edit_gain_db == Some(-6.0)));
        // The same route must still refuse a motion effect that spans two pieces.
        let query = Query {
            id: id(2),
            label: 1,
            offset: 3,
            length: 5,
            record: 0,
            via: vec![],
            depth: 0,
            kind: KIND_PICTURE,
            track: 1,
        };
        let sequence = PendingSequence {
            source: None,
            info: SequenceInfo {
                events: vec![],
                problems: vec![],
                ..info
            },
            ops: vec![Op::Motion {
                query,
                length: 10,
                ratio: (1, 2),
            }],
        };
        let info = sequence.finish(&project.routes);
        assert!(info.events.is_empty());
        assert!(info.problems[0].contains("source spans 2 pieces"));
    }
    #[test]
    fn selected_routes_do_not_widen_supported_group_semantics() {
        for (selected, class, expected) in [(0, *b"SCLP", ""), (1, *b"SCLP", "has no selected angle"), (0, *b"SPED", "angle is SPED")] {
            let mut f = file();
            mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
            let inner = source(&mut f, id(1), KIND_PICTURE, 1, 20, 100);
            if let Some(Object::Decoded(n)) = f.objects[inner as usize].as_mut() {
                n.class = class;
            }
            let mut selector = node(*b"SLCT", "Selector");
            selector.put("length", Value::Int(100));
            selector.put("media_kind_id", Value::Int(KIND_PICTURE));
            selector.put("selected", Value::Int(selected));
            let mut track = node([0; 4], "Track");
            track.put("component", Value::Ref(inner));
            selector.put("tracks", Value::List(vec![Value::Node(Box::new(track))]));
            let selector = add(&mut f, selector);
            mob(&mut f, id(2), "Group", MOB_COMPOSITION, 4, &[(1, selector)]);
            let edit = source(&mut f, id(2), KIND_PICTURE, 1, 2, 10);
            mob(&mut f, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, edit)]);
            let info = project(vec![f]).sequences.remove(0);
            if expected.is_empty() {
                assert!(info.problems.is_empty());
                assert_eq!(info.events[0].source_in, 22);
                assert_eq!(info.events[0].via.last().unwrap(), "angle 1");
            } else {
                assert!(info.events.is_empty());
                assert!(info.problems[0].contains(expected), "{:?}", info.problems);
            }
        }
    }
    #[test]
    fn delayed_and_local_diagnostics_keep_traversal_order() {
        let mut f = file();
        let missing = source(&mut f, id(99), KIND_PICTURE, 1, 0, 10);
        let mut unsupported = node(*b"GRFX", "Graphic");
        unsupported.put("length", Value::Int(5));
        let unsupported = add(&mut f, unsupported);
        let missing_again = source(&mut f, id(98), KIND_PICTURE, 1, 0, 10);
        let components = segments(&mut f, KIND_PICTURE, &[missing, unsupported, missing_again]);
        mob(&mut f, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, components)]);
        let info = project(vec![f]).sequences.remove(0);
        assert_eq!(info.problems.len(), 3);
        assert!(info.problems[0].contains("at frame 0: source clip"));
        assert!(info.problems[1].contains("at frame 10: GRFX"));
        assert!(info.problems[2].contains("at frame 15: source clip"));
    }
    #[test]
    fn nested_picture_and_matte_scopes_keep_inner_effect_and_report_split_edits() {
        let mut f = file();
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        let edit = source(&mut f, id(1), KIND_PICTURE, 1, 0, 10);
        mob(&mut f, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, edit)]);
        let mut project = Project::default();
        project.push(compile_bin(&f, 0));
        drop(f);
        let mut sequence = project.sequences.pop().unwrap();
        for kind in [PictureKind::Resize, PictureKind::Warp] {
            sequence.ops.push(Op::Picture {
                before: 0,
                value: Arc::new(PictureEffect {
                    kind,
                    params: ColorCorrection::default(),
                }),
            });
        }
        let matte = Arc::new(AniMatteData {
            states: vec![],
            tracker_xy: None,
            not_converted: vec![],
        });
        sequence.ops.push(Op::Matte {
            before: 0,
            length: 10,
            value: matte.clone(),
        });
        let mut info = sequence.finish(&project.routes);
        assert_eq!(info.events[0].picture.as_ref().unwrap().kind, PictureKind::Resize);
        assert!(info.events[0].picture_nested);
        assert!(info.events[0].animatte.is_some());
        info.events.clear();
        let query = || Query {
            id: id(1),
            label: 1,
            offset: 0,
            length: 5,
            record: 0,
            via: vec![],
            depth: 0,
            kind: KIND_PICTURE,
            track: 1,
        };
        let sequence = PendingSequence {
            source: None,
            info,
            ops: vec![
                Op::Source(query()),
                Op::Source(query()),
                Op::Matte {
                    before: 0,
                    length: 10,
                    value: matte,
                },
            ],
        };
        let info = sequence.finish(&project.routes);
        assert!(info
            .events
            .iter()
            .all(|e| e.animatte.is_none() && e.via.last().unwrap().contains("more than one edit")));
    }
    #[test]
    fn full_ids_with_same_legacy_short_id_resolve_separately() {
        let mut f = file();
        let first = id(1);
        let mut second = first;
        second.instance[2] ^= 1;
        assert_eq!(first.material, second.material);
        assert_ne!(first, second);
        let a = source(&mut f, first, KIND_PICTURE, 1, 1, 10);
        let b = source(&mut f, second, KIND_PICTURE, 1, 2, 10);
        mob(&mut f, id(3), "Timeline", MOB_COMPOSITION, 0, &[(1, a), (2, b)]);
        mob(&mut f, first, "First", MOB_MASTER, 0, &[]);
        mob(&mut f, second, "Second", MOB_MASTER, 0, &[]);
        let out = project(vec![f]);
        assert_eq!(out.masters.len(), 2);
        assert!(out.duplicates.is_empty());
        assert_eq!(out.sequences[0].events[0].master, first);
        assert_eq!(out.sequences[0].events[1].master, second);
    }
    struct Tracked {
        file: AvbFile,
        live: Rc<Cell<usize>>,
    }
    impl Borrow<AvbFile> for Tracked {
        fn borrow(&self) -> &AvbFile {
            &self.file
        }
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.live.set(self.live.get() - 1);
        }
    }
    #[test]
    fn decoded_bin_is_dropped_before_next_open_and_on_failure() {
        let live = Rc::new(Cell::new(0));
        let opens = Cell::new(0);
        let paths = vec![PathBuf::from("first"), PathBuf::from("bad"), PathBuf::from("never")];
        let out = read_paths_with(&paths, |p| {
            assert_eq!(live.get(), 0, "previous decoded bin is retained");
            opens.set(opens.get() + 1);
            if p == Path::new("bad") {
                return Err(crate::io::Error::new("synthetic failure", 0));
            }
            live.set(1);
            Ok(Tracked {
                file: file(),
                live: live.clone(),
            })
        });
        let err = out.unwrap_err();
        assert_eq!(err.path, PathBuf::from("bad"));
        assert_eq!(opens.get(), 2);
        assert_eq!(live.get(), 0);
        let out = read_paths_with(&paths[..1], |_| {
            assert_eq!(live.get(), 0);
            live.set(1);
            Ok(Tracked {
                file: file(),
                live: live.clone(),
            })
        })
        .unwrap();
        assert!(out.masters.is_empty());
        assert_eq!(live.get(), 0);
    }

    fn source_inputs(count:usize)->Vec<crate::source::SourceInput>{
        (0..count).map(|i|crate::source::SourceInput{
            path:PathBuf::from(format!("/synthetic/source-{i}.avb")),revision:format!("{:064x}",i+1),
        }).collect()
    }
    fn source_project(files:&[AvbFile])->crate::source::SourceProject{
        let inputs=source_inputs(files.len());let refs:Vec<_>=files.iter().collect();
        crate::source::extract_source_files(&inputs,&refs,crate::source::SourceLimits::default()).unwrap()
    }

    #[test]
    fn source_picture_chain_keeps_input_order_and_rejects_an_unresolved_second_input() {
        use crate::source::{PictureExtension, PictureStage};

        fn reflection(f: &mut AvbFile, effect: &str, inputs: &[u32]) -> u32 {
            let mut n = node(*b"TKFX", "TrackEffect");
            n.put("length", Value::Int(10));
            n.put("edit_rate", Value::Float(25.0));
            n.put("media_kind_id", Value::Int(KIND_PICTURE));
            n.put("effect_id", Value::Str(effect.into()));
            n.put("tracks", Value::List(inputs.iter().map(|&input| {
                let mut track = node([0; 4], "Track");
                track.put("component", Value::Ref(input));
                Value::Node(Box::new(track))
            }).collect()));
            add(f, n)
        }

        let mut f = file();
        let leaf = source(&mut f, id(1), KIND_PICTURE, 1, 7, 10);
        let inner = reflection(&mut f, "EFF2_BLEND_FLIP_HORZ", &[leaf]);
        let wrapped = segments(&mut f, KIND_PICTURE, &[inner]);
        let middle = reflection(&mut f, "EFF2_BLEND_FLIP_VERT", &[wrapped]);
        let outer = reflection(&mut f, "EFF2_BLEND_FLIP_BOTH", &[middle]);
        let incomplete = reflection(&mut f, "EFF2_BLEND_FLIP_BOTH", &[middle, 1000]);
        mob(&mut f, id(2), "Chain", MOB_COMPOSITION, 0, &[(1, outer)]);
        mob(&mut f, id(3), "Unresolved input", MOB_COMPOSITION, 0, &[(1, incomplete)]);
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);

        let out = source_project(&[f]);
        let [positive, negative] = out.sequences.as_slice() else { panic!("expected two sequences") };
        let [edit] = positive.events.as_slice() else { panic!("expected one edit") };
        assert_eq!(edit.range.as_ref().map(|r| (r.record_start, r.source_in, r.length)), Some((0, 7, 10)));
        let axes = edit.picture_chain.iter().map(|stage| match stage {
            PictureStage::Extension(PictureExtension::Flip { horizontal, vertical, .. }) =>
                (*horizontal, *vertical),
            _ => panic!("expected a reflection"),
        }).collect::<Vec<_>>();
        assert_eq!(axes, [(true, false), (false, true), (true, true)]);

        // The older flattened walk drops the missing track reference. A single
        // surviving media edit is insufficient evidence for an ordered chain.
        let [edit] = negative.events.as_slice() else { panic!("expected the surviving edit") };
        assert!(edit.picture_chain.is_empty());
        assert!(matches!(edit.picture_extension, Some(PictureExtension::Unsupported(_))));
    }

    fn timeline(f:&mut AvbFile,kind:i64,components:&[u32])->u32{
        let mut sequence=node(*b"SEQU","Sequence");
        sequence.put("media_kind_id",Value::Int(kind));
        sequence.put("length",Value::Int(30));
        sequence.put("components",Value::RefList(components.iter().copied().map(Value::Ref).collect()));
        add(f,sequence)
    }
    fn marker_on(f:&mut AvbFile,component:u32,offset:Option<i64>){
        let mut attrs=node(*b"ATTR","Attributes");
        attrs.put("entries",Value::Attrs(vec![("_ATN_CRM_MARKNAME".into(),Value::Str("Scoped marker".into()))]));
        let attrs=add(f,attrs);
        let mut marker=node(*b"TMBC","Marker");
        if let Some(offset)=offset{marker.put("comp_offset",Value::Int(offset));}
        marker.put("attributes",Value::Ref(attrs));
        let marker=add(f,marker);
        let mut attrs=node(*b"ATTR","Attributes");
        attrs.put("entries",Value::Attrs(vec![("_TMP_CRM".into(),Value::Ref(marker))]));
        let attrs=add(f,attrs);
        let Object::Decoded(n)=f.objects[component as usize].as_mut().unwrap()else{unreachable!()};
        n.put("attributes",Value::Ref(attrs));
    }
    fn marked_timecode(f: &mut AvbFile, class: [u8; 4], rate: Option<f64>, length: i64, offset: Option<i64>) -> u32 {
        let mut tc = node(class, "Timecode");
        tc.put("media_kind_id", Value::Int(KIND_TIMECODE));
        tc.put("length", Value::Int(length));
        if let Some(rate) = rate { tc.put("edit_rate", Value::Float(rate)); }
        // Display base is deliberately different from the owner edit rate.
        tc.put("fps", Value::Int(24));
        tc.put("start", Value::Int(86400));
        let tc = add(f, tc);
        marker_on(f, tc, offset);
        tc
    }
    #[test]
    fn source_direct_timecode_marker_uses_owner_rate_without_changing_legacy_clock() {
        let mut f = file();
        let tc = marked_timecode(&mut f, *b"TCCP", Some(25.0), 100, Some(24));
        mob(&mut f, id(3), "Metadata marker", MOB_COMPOSITION, 0, &[(1, tc)]);
        let out = source_project(std::slice::from_ref(&f));
        let seq = &out.sequences[0];
        assert_eq!(seq.markers.len(), 1);
        let m = &seq.markers[0];
        assert_eq!(m.kind, KIND_TIMECODE);
        assert_eq!(m.track, 1);
        assert_eq!(m.owner_kind, MarkerOwner::Sequence);
        assert_eq!(m.local_offset, crate::source::Observed::Known(24));
        assert_eq!(m.local_rate, crate::source::Rate::new(25, 1));
        assert_eq!(m.record_frame, Some(24));
        assert!(m.binding.placements.is_empty() && m.binding.reason.is_none());
        let legacy = project(vec![f]);
        assert!(legacy.sequences[0].markers.is_empty());
        let start = legacy.sequences[0].start.as_ref().unwrap();
        assert_eq!((start.frames, start.fps), (86400, 24));
    }

    #[test]
    fn sequence_clock_requires_a_unique_direct_tc1_with_the_exact_record_rate() {
        for (record_rate, label, rate, length, flags, duplicate, known) in [
            (23.976, 1, 23.976, 100, 0, false, true),
            (23.976, 2, 23.976, 100, 0, false, false),
            (23.976, 1, 24.0, 100, 0, false, false),
            (24.0, 1, 24.0, 100, 0, false, true),
            (24.0, 1, 23.976, 100, 0, false, false),
            (23.976, 1, 23.976, 99, 0, false, false),
            (23.976, 1, 23.976, 100, 1, false, false),
            (23.976, 1, 23.976, 100, 0, true, false),
        ] {
            let mut f = file();
            let tc = marked_timecode(&mut f, *b"TCCP", Some(rate), length, None);
            let Object::Decoded(t) = f.objects[tc as usize].as_mut().unwrap() else { unreachable!() };
            t.put("flags", Value::Int(flags));
            let alternate = marked_timecode(&mut f, *b"TCCP", Some(23.976), 100, None);
            let Object::Decoded(t) = f.objects[alternate as usize].as_mut().unwrap() else { unreachable!() };
            t.fields.retain(|(k, _)| !matches!(*k, "fps" | "start"));
            t.put("fps", Value::Int(25));
            t.put("start", Value::Int(90000));
            t.put("flags", Value::Int(0));
            let mut tracks = vec![(4, alternate), (label, tc)];
            if duplicate {
                tracks.push((1, alternate));
            }
            let m = mob(&mut f, id(3), "TC1", MOB_COMPOSITION, 0, &tracks);
            let Object::Decoded(m) = f.objects[m as usize].as_mut().unwrap() else { unreachable!() };
            m.fields.retain(|(k, _)| *k != "edit_rate");
            m.put("edit_rate", Value::Float(record_rate));

            let out = source_project(&[f]);
            assert_eq!(
                out.sequences[0].clock,
                known.then(|| crate::source::SequenceClock {
                    count: 86400,
                    rate: crate::source::Rate::from_avid(record_rate).unwrap(),
                    display_base: 24,
                    track_ordinal: 1,
                })
            );
        }
    }
    #[test]
    fn source_timecode_marker_uncertainty_is_retained_instead_of_inventing_record_time() {
        for (class, rate, length, track, offset) in [
            (*b"TCCP", Some(24.0), 100, 1, Some(24)),
            (*b"TCCP", None, 100, 1, Some(24)),
            (*b"TCCP", Some(25.0), 99, 1, Some(24)),
            (*b"MASK", Some(25.0), 100, 1, Some(24)),
            (*b"TCCP", Some(25.0), 100, 0, Some(24)),
            (*b"TCCP", Some(25.0), 100, 1, None),
            (*b"TCCP", Some(25.0), 100, 1, Some(-1)),
            (*b"TCCP", Some(25.0), 100, 1, Some(101)),
        ] {
            let mut f = file();
            let tc = marked_timecode(&mut f, class, rate, length, offset);
            mob(&mut f, id(3), "Unresolved metadata marker", MOB_COMPOSITION, 0, &[(track, tc)]);
            let out = source_project(&[f]);
            let m = &out.sequences[0].markers[0];
            assert_eq!(m.record_frame, None, "{class:?}/{rate:?}/{length}/{track}/{offset:?}");
            assert!(m.binding.reason.as_ref().unwrap().contains("transform"));
            assert_eq!(m.marker.name, "Scoped marker");
        }
    }
    #[test]
    fn source_timecode_marker_does_not_assume_a_nonzero_track_origin_is_zero() {
        for (origin, expected) in [
            (None, Some(24)),
            (Some(Value::Int(0)), Some(24)),
            (Some(Value::Int(1)), None),
            (Some(Value::Int(-1)), None),
            (Some(Value::Str("0".into())), None),
        ] {
            let mut f = file();
            let tc = marked_timecode(&mut f, *b"TCCP", Some(25.0), 100, Some(24));
            let composition = mob(&mut f, id(3), "Track origin", MOB_COMPOSITION, 0, &[(1, tc)]);
            if let Some(origin) = origin {
                let Object::Decoded(n) = f.objects[composition as usize].as_mut().unwrap() else { unreachable!() };
                let Value::List(tracks) = &mut n.fields.iter_mut().find(|(name, _)| *name == "tracks").unwrap().1 else { unreachable!() };
                let Value::Node(track) = &mut tracks[0] else { unreachable!() };
                track.put("start_pos", origin);
            }
            let out = source_project(&[f]);
            let marker = &out.sequences[0].markers[0];
            assert_eq!(marker.record_frame, expected);
            assert_eq!(marker.binding.reason.is_some(), expected.is_none());
            assert_eq!(marker.local_offset, crate::source::Observed::Known(24));
        }
    }
    #[test]
    fn source_timecode_marker_namespace_does_not_renumber_av_or_merge_equal_labels() {
        let mut f = file();
        let tc1 = marked_timecode(&mut f, *b"TCCP", Some(25.0), 100, Some(24));
        let tc2 = marked_timecode(&mut f, *b"TCCP", Some(25.0), 100, Some(25));
        let clip = source(&mut f, id(1), KIND_PICTURE, 1, 0, 10);
        mob(&mut f, id(3), "Metadata marker identities", MOB_COMPOSITION, 0, &[(1, tc1), (1, tc2), (1, clip)]);
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        let out = source_project(&[f]);
        let seq = &out.sequences[0];
        assert_eq!(seq.markers.len(), 2);
        assert_ne!(seq.markers[0].id, seq.markers[1].id);
        assert_ne!(seq.markers[0].owner, seq.markers[1].owner);
        assert!(seq.events[0].id.ends_with("/component/0/event/0"));
    }
    #[test]
    fn source_transition_keeps_native_ranges_and_structural_endpoint_ids(){
        let mut f=file();
        let outgoing=source(&mut f,id(1),KIND_PICTURE,1,0,20);
        let incoming=source(&mut f,id(1),KIND_PICTURE,1,20,20);
        let mut transition=node(*b"TNFX","Transition");
        transition.put("length",Value::Int(10));transition.put("cutpoint",Value::Int(4));
        transition.put("effect_id",Value::Str("Synthetic dissolve".into()));
        let transition=add(&mut f,transition);
        let sequence=timeline(&mut f,KIND_PICTURE,&[outgoing,transition,incoming]);
        marker_on(&mut f,incoming,Some(2));
        mob(&mut f,id(3),"Source timeline",MOB_COMPOSITION,0,&[(1,sequence)]);
        mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let files=[f];let out=source_project(&files);let seq=&out.sequences[0];
        assert_eq!(seq.events.len(),2);assert!(seq.info.events.is_empty());
        assert_eq!((seq.events[0].event.record_start,seq.events[0].event.length),(0,20));
        assert_eq!((seq.events[1].event.record_start,seq.events[1].event.length),(10,20));
        assert!(seq.events.iter().all(|e|e.range.is_some()));
        let t=&seq.transitions[0];
        assert_eq!(t.outgoing.placements,vec![seq.events[0].id.clone()]);
        assert_eq!(t.incoming.placements,vec![seq.events[1].id.clone()]);
        assert!(t.outgoing.reason.is_none()&&t.incoming.reason.is_none());
        assert_eq!(t.position,crate::source::Observed::Known(10));
        assert_eq!(t.cutpoint,crate::source::Observed::Known(4));
        let marker=&seq.markers[0];
        assert_eq!(marker.local_offset,crate::source::Observed::Known(2));assert_eq!(marker.record_frame,Some(12));
        assert_eq!(marker.binding.placements,vec![seq.events[1].id.clone()]);
        let again=source_project(&files);
        assert_eq!(seq.events.iter().map(|e|&e.id).collect::<Vec<_>>(),again.sequences[0].events.iter().map(|e|&e.id).collect::<Vec<_>>());
    }
    #[test]
    fn source_presence_and_mixed_rate_failures_are_facts_not_fake_zero_ranges(){
        let mut f=file();let source_clip=source(&mut f,id(1),KIND_PICTURE,1,0,10);
        let Object::Decoded(n)=f.objects[source_clip as usize].as_mut().unwrap()else{unreachable!()};
        n.fields.retain(|(k,_)|*k!="start_time");
        marker_on(&mut f,source_clip,None);
        mob(&mut f,id(3),"Unknown offset",MOB_COMPOSITION,0,&[(1,source_clip)]);
        mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(&[f]);let seq=&out.sequences[0];
        assert!(seq.events.is_empty());assert!(seq.unresolved[0].reason.contains("start is absent"));
        assert!(matches!(seq.unresolved[0].route[0].source_offset,crate::source::Observed::Unresolved(_)));
        assert!(matches!(seq.markers[0].local_offset,crate::source::Observed::Unresolved(_)));
        assert_eq!(seq.markers[0].record_frame,None);
        let mut f=file();let source_clip=source(&mut f,id(2),KIND_PICTURE,1,0,10);
        mob(&mut f,id(3),"Mixed route",MOB_COMPOSITION,0,&[(1,source_clip)]);
        let inner=source(&mut f,id(1),KIND_PICTURE,1,0,100);
        let sub=mob(&mut f,id(2),"Subclip",MOB_COMPOSITION,2,&[(1,inner)]);
        let Object::Decoded(n)=f.objects[sub as usize].as_mut().unwrap()else{unreachable!()};
        n.fields.iter_mut().find(|(k,_)|*k=="edit_rate").unwrap().1=Value::Float(24.0);
        mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(&[f]);let seq=&out.sequences[0];
        assert!(seq.events.is_empty());
        assert!(seq.unresolved.iter().any(|i|i.reason.contains("route-owner rates")
            && i.issue_kind == crate::source::SourceIssueKind::RouteTiming));
        assert!(seq.unresolved.iter().flat_map(|i|&i.route).any(|w|w.rate==crate::source::Rate::new(24,1)));
    }
    #[test]
    fn automatic_motion_adapter_is_not_an_ordinary_speed_edit() {
        let mut f = file();
        let input = source(&mut f, id(1), KIND_PICTURE, 1, 0, 20);
        let mut attrs = node(*b"ATTR", "Attributes");
        attrs.put("entries", Value::Attrs(vec![("_MIXMATCH_MOTIONADAPTER".into(), Value::Int(2))]));
        let attrs = add(&mut f, attrs);
        let mut speed = node(*b"SPED", "MotionEffect");
        speed.put("media_kind_id", Value::Int(KIND_PICTURE));
        speed.put("length", Value::Int(10));
        speed.put("speed_ratio", Value::List(vec![Value::Int(1000), Value::Int(1001)]));
        speed.put("attributes", Value::Ref(attrs));
        let mut track = node([0; 4], "Track");
        track.put("component", Value::Ref(input));
        speed.put("tracks", Value::List(vec![Value::Node(Box::new(track))]));
        let speed = add(&mut f, speed);
        mob(&mut f, id(3), "Adapter", MOB_COMPOSITION, 0, &[(1, speed)]);
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        let out = source_project(&[f]);
        let seq = &out.sequences[0];
        assert!(seq.events.is_empty());
        assert_eq!(seq.unresolved.len(), 1);
        assert_eq!(seq.unresolved[0].issue_kind, crate::source::SourceIssueKind::RouteTiming);
        assert!(seq.unresolved[0].reason.contains("Motion Adapter"));
    }
    #[test]
    fn selected_sparse_position_cannot_fall_back_to_its_unity_ratio() {
        let mut f = file();
        let input = source(&mut f, id(1), KIND_PICTURE, 1, 0, 20);
        let mut curve = node(*b"PRCL", "ParamClip");
        curve.put("interp_kind", Value::Int(6));
        let keys = [(0, 0.0), (5, 2.5), (14, 16.5), (19, 19.0)];
        curve.put("control_points", Value::List(keys.into_iter().map(|(frame, value)| {
            let mut point = node([0; 4], "ParamControlPoint");
            point.put("offset", Value::List(vec![Value::Int(frame), Value::Int(1)]));
            point.put("value", Value::Float(value));
            Value::Node(Box::new(point))
        }).collect()));
        let curve = add(&mut f, curve);
        let mut item = node(*b"PRIT", "ParameterItem");
        item.put("uuid", Value::Uuid(crate::io::Uuid {
            data1: 0x8d56827a, data2: 0x847e, data3: 0x11d5,
            data4: [0x93, 0x5a, 0x50, 0xf8, 0x57, 0xc1, 0x00, 0x00],
        }));
        item.put("enable", Value::Bool(true));
        item.put("control_track", Value::Ref(curve));
        let item = add(&mut f, item);
        let mut params = node(*b"PRLS", "ParameterList");
        params.put("items", Value::RefList(vec![Value::Ref(item)]));
        let params = add(&mut f, params);
        for selector in [2, 1] {
            let mut attrs = node(*b"ATTR", "Attributes");
            attrs.put("entries", Value::Attrs(vec![("_MFX_GRAPH_ENABLED".into(), Value::Int(selector))]));
            let attrs = add(&mut f, attrs);
            let mut effect = node(*b"SPED", "MotionEffect");
            effect.put("effect_id", Value::Str("EFF_ADV_MOTION_CTL".into()));
            effect.put("media_kind_id", Value::Int(KIND_PICTURE));
            effect.put("length", Value::Int(20));
            effect.put("speed_ratio", Value::List(vec![Value::Int(1), Value::Int(1)]));
            effect.put("attributes", Value::Ref(attrs));
            effect.put("param_list", Value::Ref(params));
            let mut track = node([0; 4], "Track");
            track.put("component", Value::Ref(input));
            effect.put("tracks", Value::List(vec![Value::Node(Box::new(track))]));
            let effect = add(&mut f, effect);
            mob(&mut f, id(selector as u32 + 2), "Synthetic position graph", MOB_COMPOSITION, 0, &[(1, effect)]);
        }
        mob(&mut f, id(1), "Master", MOB_MASTER, 0, &[]);
        let out = source_project(&[f]);
        let [selected, inactive] = out.sequences.as_slice() else { panic!("expected two sequences") };
        assert!(selected.events.is_empty());
        assert!(selected.unresolved.iter().any(|issue| {
            issue.issue_kind == crate::source::SourceIssueKind::RouteTiming
                && issue.reason.contains("unsupported Position curve")
        }));
        assert_eq!(inactive.events.len(), 1);
        assert_eq!(inactive.events[0].event.speed, Some((1, 1)));
    }
    #[test]
    fn source_retimed_marker_keeps_local_owner_without_inventing_record_phase(){
        let mut f=file();let input=source(&mut f,id(1),KIND_PICTURE,1,0,20);
        let mut speed=node(*b"SPED","MotionEffect");
        speed.put("media_kind_id",Value::Int(KIND_PICTURE));speed.put("length",Value::Int(10));
        speed.put("speed_ratio",Value::List(vec![Value::Int(1),Value::Int(2)]));
        let mut track=node([0;4],"Track");track.put("component",Value::Ref(input));
        speed.put("tracks",Value::List(vec![Value::Node(Box::new(track))]));let speed=add(&mut f,speed);
        marker_on(&mut f,speed,Some(2));
        mob(&mut f,id(3),"Retimed",MOB_COMPOSITION,0,&[(1,speed)]);mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(&[f]);let seq=&out.sequences[0];
        assert_eq!(seq.events.len(),1);assert_eq!(seq.events[0].event.speed,Some((1,2)));
        let range=seq.events[0].range.as_ref().unwrap();assert_eq!((range.length,range.source_length),(10,20));
        let marker=&seq.markers[0];assert_eq!(marker.local_offset,crate::source::Observed::Known(2));
        assert_eq!(marker.record_frame,None);assert!(marker.binding.reason.as_ref().unwrap().contains("transform"));
    }
    #[test]
    fn source_expansion_preserves_one_to_many_ownership(){
        let mut f=file();let reference=source(&mut f,id(2),KIND_PICTURE,1,0,10);
        marker_on(&mut f,reference,Some(2));
        mob(&mut f,id(3),"Expanded",MOB_COMPOSITION,0,&[(1,reference)]);
        let first=source(&mut f,id(1),KIND_PICTURE,1,20,5);let second=source(&mut f,id(1),KIND_PICTURE,1,40,5);
        let inner=timeline(&mut f,KIND_PICTURE,&[first,second]);
        mob(&mut f,id(2),"Two-piece subclip",MOB_COMPOSITION,2,&[(1,inner)]);mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(&[f]);let seq=&out.sequences[0];assert_eq!(seq.events.len(),2);
        assert_eq!(seq.markers[0].binding.placements,seq.events.iter().map(|e|e.id.clone()).collect::<Vec<_>>());
        assert_eq!(seq.markers[0].binding.reason,None);
    }
    #[test]
    fn source_paths_release_decoder_before_next_bin_and_limit_before_open(){
        use std::cell::Cell;use std::rc::Rc;
        struct Tracked{file:AvbFile,active:Rc<Cell<usize>>}
        impl Borrow<AvbFile> for Tracked{fn borrow(&self)->&AvbFile{&self.file}}
        impl Drop for Tracked{fn drop(&mut self){self.active.set(self.active.get()-1);}}
        let mut a=file();let reference=source(&mut a,id(1),KIND_PICTURE,1,0,10);
        mob(&mut a,id(3),"Forward",MOB_COMPOSITION,0,&[(1,reference)]);
        let mut b=file();mob(&mut b,id(1),"Master",MOB_MASTER,0,&[]);
        let mut files=vec![a,b].into_iter();let active=Rc::new(Cell::new(0));
        let out=source_capture::read_source_with(&source_inputs(2),crate::source::SourceLimits::default(),|_|{
            assert_eq!(active.get(),0);active.set(1);Ok(Tracked{file:files.next().unwrap(),active:active.clone()})
        }).unwrap();
        assert_eq!(active.get(),0);assert_eq!(out.sequences[0].events.len(),1);
        assert!(out.accounted_bytes>0&&out.accounted_bytes<=crate::source::MAX_SOURCE_FACT_BYTES);
        let opened=Cell::new(false);
        let refusal=source_capture::read_source_with(&source_inputs(1),crate::source::SourceLimits{max_fact_bytes:0},|_|{
            opened.set(true);Ok(file())
        });
        assert!(refusal.is_err());assert!(!opened.get());
    }

    #[test]
    fn source_original_locators_preserve_raw_descriptor_and_channel_namespaces(){
        let mut f=file();
        let mut locator=node(*b"FILE","MacFileLocator");locator.put("path_posix",Value::Str("/Synthetic/source.mp4".into()));locator.put("path_utf8",Value::Str("/Synthetic/source.mp4".into()));let locator=add(&mut f,locator);
        let mut physical=node(*b"MDES","MediaDescriptor");physical.put("locator",Value::Ref(locator));let physical=add(&mut f,physical);
        let mut descriptor=node(*b"MPGA","MPGADescriptor");descriptor.put("physical_media",Value::Ref(physical));descriptor.put("edit_rate",Value::Float(46.875));descriptor.put("length",Value::Int(96000));descriptor.put("channels",Value::Int(2));descriptor.put("sample_rate",Value::Float(48000.0));descriptor.put("quantization_bits",Value::Int(16));let descriptor=add(&mut f,descriptor);
        let file_mob=mob(&mut f,id(10),"Synthetic file",MOB_SOURCE,0,&[]);
        let Object::Decoded(m)=f.objects[file_mob as usize].as_mut().unwrap()else{unreachable!()};m.put("descriptor",Value::Ref(descriptor));
        let a1=source(&mut f,id(10),KIND_SOUND,1,0,50);let a2=source(&mut f,id(10),KIND_SOUND,2,0,50);
        mob(&mut f,id(1),"Synthetic linked master",MOB_MASTER,0,&[(7,a1),(9,a2)]);
        let facts=source_project(&[f]);let m=&facts.masters[0];assert_eq!(m.originals.len(),2);
        let a=m.originals[0].as_ref().unwrap();let b=m.originals[1].as_ref().unwrap();
        assert_eq!(a.locators,vec!["/Synthetic/source.mp4"]);assert_eq!(a.descriptor_rate,46.875);assert_eq!(a.sample_rate,Some(48000.0));assert_eq!(a.stored_quantization_bits,Some(16));assert_eq!(a.file_track,Some(1));assert_eq!(b.file_track,Some(2));assert_eq!(m.master.tracks[0].index,7);assert_eq!(m.master.tracks[1].index,9);
        assert!(facts.accounted_bytes>std::mem::size_of::<crate::source::OriginalTrack>()*2);
    }

    #[test]
    fn source_master_absence_and_known_zero_offsets_are_distinct(){
        let mut f=file();let master=mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let known=source_project(std::slice::from_ref(&f));assert_eq!(known.masters[0].offsets,vec![crate::source::Observed::Known(0)]);
        let component={let Object::Decoded(mob)=f.objects[master as usize].as_ref().unwrap()else{unreachable!()};
            let Value::List(tracks)=mob.get("tracks").unwrap()else{unreachable!()};
            let Value::Node(track)=&tracks[0]else{unreachable!()};track.get("component").unwrap().as_index().unwrap()};
        let Object::Decoded(n)=f.objects[component as usize].as_mut().unwrap()else{unreachable!()};n.fields.retain(|(k,_)|*k!="start_time");
        marker_on(&mut f,component,None);
        let out=source_project(&[f]);let master=&out.masters[0];
        assert_eq!(master.master.tracks[0].offset,0); // Compatibility payload only.
        assert!(matches!(master.offsets[0],crate::source::Observed::Unresolved(_)));
        assert_eq!(master.master.markers.len(),1);assert_eq!(master.marker_witnesses.len(),1);
        assert!(matches!(master.marker_witnesses[0].local_offset,crate::source::Observed::Unresolved(_)));
        assert_eq!(master.marker_witnesses[0].master_frame,None);
    }

    #[test]
    fn source_unknown_effect_span_mismatch_is_not_promoted_to_a_portable_range(){
        let mut f=file();let input=source(&mut f,id(1),KIND_PICTURE,1,0,10);
        let mut effect=node(*b"TKFX","TrackEffect");effect.put("media_kind_id",Value::Int(KIND_PICTURE));
        effect.put("length",Value::Int(5));effect.put("effect_id",Value::Str("Synthetic unknown effect".into()));
        let mut track=node([0;4],"Track");track.put("component",Value::Ref(input));
        effect.put("tracks",Value::List(vec![Value::Node(Box::new(track))]));let effect=add(&mut f,effect);
        mob(&mut f,id(3),"Unqualified span",MOB_COMPOSITION,0,&[(1,effect)]);mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(&[f]);assert!(out.sequences[0].events.is_empty());
        assert!(out.sequences[0].unresolved.iter().any(|i|i.reason.contains("enclosing component span")));
    }

    #[test]
    fn source_master_offsets_use_media_kind_and_label_namespaces(){
        let mut f=file();mob(&mut f,id(10),"Video file",MOB_SOURCE,0,&[]);mob(&mut f,id(11),"Audio file",MOB_SOURCE,0,&[]);
        let video=source(&mut f,id(10),KIND_PICTURE,1,3,100);let audio=source(&mut f,id(11),KIND_SOUND,1,7,100);
        mob(&mut f,id(1),"AV master",MOB_MASTER,0,&[(1,video),(1,audio)]);
        let out=source_project(&[f]);assert_eq!(out.masters[0].offsets,
            vec![crate::source::Observed::Known(3),crate::source::Observed::Known(7)]);
    }
    #[test]
    fn source_motion_input_explicit_rate_is_not_overridden_by_effect_owner(){
        let mut f=file();let input=source(&mut f,id(1),KIND_PICTURE,1,0,20);
        let Object::Decoded(n)=f.objects[input as usize].as_mut().unwrap()else{unreachable!()};n.put("edit_rate",Value::Float(50.0));
        let mut speed=node(*b"SPED","MotionEffect");speed.put("media_kind_id",Value::Int(KIND_PICTURE));speed.put("length",Value::Int(10));
        speed.put("speed_ratio",Value::List(vec![Value::Int(1),Value::Int(2)]));
        let mut track=node([0;4],"Track");track.put("component",Value::Ref(input));speed.put("tracks",Value::List(vec![Value::Node(Box::new(track))]));let speed=add(&mut f,speed);
        mob(&mut f,id(3),"Unqualified input rate",MOB_COMPOSITION,0,&[(1,speed)]);mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(&[f]);assert!(out.sequences[0].events.is_empty());
        assert!(out.sequences[0].unresolved.iter().any(|i|i.reason.contains("rate differs from its record owner")));
    }

    #[test]
    fn source_combiner_expected_channels_include_unresolved_inputs(){
        let mut f=file();
        let a=source(&mut f,id(1),KIND_SOUND,1,0,10);
        let missing=source(&mut f,id(99),KIND_SOUND,2,0,10);
        let c=source(&mut f,id(1),KIND_SOUND,3,0,10);
        let mut effect=node(*b"TKFX","TrackEffect");
        effect.put("media_kind_id",Value::Int(KIND_SOUND));effect.put("length",Value::Int(10));
        effect.put("effect_id",Value::Str("EFF2_AUDIO_CHANNEL_COMBINER".into()));
        effect.put("tracks",Value::List([a,missing,c].iter().map(|input|{
            let mut track=node([0;4],"Track");track.put("component",Value::Ref(*input));Value::Node(Box::new(track))
        }).collect()));
        let effect=add(&mut f,effect);
        mob(&mut f,id(3),"Incomplete three-channel combiner",MOB_COMPOSITION,0,&[(1,effect)]);
        mob(&mut f,id(1),"Master",MOB_MASTER,0,&[]);
        let out=source_project(std::slice::from_ref(&f));let seq=&out.sequences[0];
        assert_eq!(seq.events.len(),2);assert_eq!(seq.combiner_channels,vec![3]);
        assert_eq!(seq.events.iter().map(|e|e.event.combined).collect::<Vec<_>>(),vec![Some((0,0)),Some((0,2))]);
        assert_eq!(seq.unresolved.len(),1);assert!(seq.unresolved[0].reason.contains("not in the selected bins"));
        let legacy=project(vec![f]);assert_eq!(legacy.sequences[0].events.len(),2);
        assert_eq!(legacy.sequences[0].events.iter().map(|e|e.combined).collect::<Vec<_>>(),vec![Some((0,0)),Some((0,2))]);
    }

    fn text_uuid(hex: &str) -> crate::io::Uuid {
        let bytes: Vec<u8>=(0..16).map(|n|u8::from_str_radix(&hex[n*2..n*2+2],16).unwrap()).collect();
        crate::io::Reader::new(&bytes,0,false).raw_uuid().unwrap()
    }
    fn subcap(f: &mut AvbFile, duplicate: bool, appearance: &[u32]) -> u32 {
        let mut data=node(*b"AVUP","CFUserParam");data.put("uuid",Value::Uuid(text_uuid("4af0193369ac2545b9e82206362fd233")));data.put("data",Value::Bytes(b"Content Alpha\0".to_vec()));let data=add(f,data);
        let mut parameter=node(*b"PRIT","ParameterItem");parameter.put("uuid",Value::Uuid(text_uuid("961a0231335b514595e0317fddf547df")));parameter.put("value",Value::Ref(data));parameter.put("control_track",Value::Ref(0));let parameter=add(f,parameter);
        let mut items = vec![Value::Ref(parameter); if duplicate { 2 } else { 1 }];
        items.extend(appearance.iter().copied().map(Value::Ref));
        let mut list=node(*b"PRLS","ParameterList");list.put("items",Value::RefList(items));let list=add(f,list);
        let mut attrs=node(*b"ATTR","Attributes");attrs.put("entries",Value::Attrs(vec![("_COMMENT".into(),Value::Str("Wrong note".into())),("_TTEXT".into(),Value::Str("Wrong cache".into()))]));let attrs=add(f,attrs);
        let mut effect=node(*b"TKFX","TrackEffect");effect.put("media_kind_id",Value::Int(KIND_PICTURE));effect.put("length",Value::Int(10));effect.put("edit_rate",Value::Float(25.0));effect.put("param_list",Value::Ref(list));effect.put("attributes",Value::Ref(attrs));effect.put("effect_id",Value::Str("E4FE520A-B815-447F-A969-2D5E1C5F6C14".into()));add(f,effect)
    }
    #[test]
    fn source_text_on_filler_keeps_each_occurrence_without_inventing_media_edits() {
        let mut f=file();let caption=subcap(&mut f,false,&[]);
        let mut fill=node(*b"FILL","Filler");fill.put("media_kind_id",Value::Int(KIND_PICTURE));fill.put("length",Value::Int(5));let fill=add(&mut f,fill);
        let seq=timeline(&mut f,KIND_PICTURE,&[fill,caption,caption]);mob(&mut f,id(3),"Text timeline",MOB_COMPOSITION,0,&[(2,seq)]);
        let output=source_project(std::slice::from_ref(&f));let seq=&output.sequences[0];assert!(seq.events.is_empty());assert_eq!(seq.texts.len(),2);
        assert_eq!(seq.texts.iter().map(|t|t.record_start).collect::<Vec<_>>(),vec![Some(5),Some(15)]);
        assert!(seq.texts.iter().all(|t|t.content.as_deref()==Some("Content Alpha")&&t.length==Some(10)&&t.track==2&&t.reason.is_none()));assert_ne!(seq.texts[0].id,seq.texts[1].id);
        assert!(seq.texts.iter().all(|t|t.native_properties.is_empty() && t.native_properties_reason.is_none()));
        let output=crate::source::extract_source_files(&source_inputs(1),&[&f],crate::source::SourceLimits{max_fact_bytes:64});assert!(output.is_err());
    }
    #[test]
    fn duplicate_content_and_unqualified_owner_clocks_remain_scoped_text_facts() {
        let mut f=file();let caption=subcap(&mut f,true,&[]);mob(&mut f,id(3),"Duplicate text",MOB_COMPOSITION,0,&[(1,caption)]);
        let out=source_project(&[f]);assert_eq!(out.sequences[0].texts[0].content,None);assert!(out.sequences[0].texts[0].reason.as_deref().unwrap().contains("duplicate"));
        let mut f=file();let caption=subcap(&mut f,false,&[]);
        let Object::Decoded(n)=f.objects[caption as usize].as_mut().unwrap()else{unreachable!()};n.fields.iter_mut().find(|(k,_)| *k=="edit_rate").unwrap().1=Value::Float(24.0);
        mob(&mut f,id(3),"Mixed clock text",MOB_COMPOSITION,0,&[(1,caption)]);let out=source_project(&[f]);let text=&out.sequences[0].texts[0];assert!(text.content.is_some());assert_eq!(text.record_start,None);assert!(text.reason.is_some());
    }

    fn subcap_appearance(f: &mut AvbFile, id: &str, value_type: i64, value: Value) -> u32 {
        let mut p = node(*b"PRIT", "ParameterItem");
        p.put("uuid", Value::Uuid(text_uuid(id)));
        p.put("value_type", Value::Int(value_type));
        p.put("value", value);
        p.put("control_track", Value::Ref(0));
        add(f, p)
    }

    fn subcap_appearance_avup(f: &mut AvbFile, id: &str, data: &[u8]) -> Value {
        let mut p = node(*b"AVUP", "CFUserParam");
        p.put("uuid", Value::Uuid(text_uuid(id)));
        p.put("data", Value::Bytes(data.to_vec()));
        Value::Ref(add(f, p))
    }

    #[test]
    fn subcap_keeps_local_appearance_without_resolving_sync() {
        let mut f = file();
        let face = subcap_appearance_avup(&mut f, "823122720aaa764cba3df88c918b67b5", "Fixture Face é\0".as_bytes());
        let sync = subcap_appearance_avup(&mut f, "aa2c2fc09372d611b4e40030658a6504", &[0; 840]);
        let properties = [
            subcap_appearance(&mut f, "38a1f47289ea4240bc9338c5f985fb68", 1, Value::Int(60)),
            subcap_appearance(&mut f, "3e42e35b209d89439d4127dd94258812", 4, face),
            subcap_appearance(&mut f, "0b4dae43a79ac94aa58a15837d68a1fb", 1, Value::Int(-3)),
            subcap_appearance(&mut f, "aadaff7abc12b44da6eca507158a4061", 4, sync),
        ];
        let caption = subcap(&mut f, false, &properties);
        mob(&mut f, id(3), "Local appearance", MOB_COMPOSITION, 0, &[(1, caption)]);
        let output = source_project(&[f]);
        let text = &output.sequences[0].texts[0];
        assert_eq!(text.content.as_deref(), Some("Content Alpha"));
        assert!(text.native_properties_reason.is_none());
        let actual = text.native_properties.iter()
            .map(|p| (p.scope.as_str(), p.list, p.name.as_str(), p.value.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(actual, [
            ("local", 1, "FontSize", "60"),
            ("local", 1, "FONTFACE", "Fixture Face é"),
            ("local", 1, "ExtraLeading", "-3"),
            ("local", 1, "SynchronizedAttributesSet", "opaque-present"),
        ]);
    }

    #[test]
    fn bad_subcap_appearance_keeps_independently_qualified_text() {
        for case in ["duplicate", "parameter type", "animated", "AVUP type", "UTF-8"] {
            let mut f = file();
            let size = subcap_appearance(&mut f, "38a1f47289ea4240bc9338c5f985fb68", 1, Value::Int(60));
            let mut properties = vec![size];
            match case {
                "duplicate" => properties.push(size),
                "parameter type" => set(&mut f, size, "value_type", Value::Int(2)),
                "animated" => set(&mut f, size, "control_track", Value::Ref(123)),
                _ => {
                    let (ty, body) = if case == "AVUP type" {
                        ("4af0193369ac2545b9e82206362fd233", b"Fixture Face\0".as_slice())
                    } else {
                        ("823122720aaa764cba3df88c918b67b5", b"Fixture\0Face\0".as_slice())
                    };
                    let face = subcap_appearance_avup(&mut f, ty, body);
                    properties.push(subcap_appearance(&mut f, "3e42e35b209d89439d4127dd94258812", 4, face));
                }
            }
            let caption = subcap(&mut f, false, &properties);
            mob(&mut f, id(3), "Malformed appearance", MOB_COMPOSITION, 0, &[(1, caption)]);
            let output = source_project(&[f]);
            let text = &output.sequences[0].texts[0];
            assert_eq!(text.content.as_deref(), Some("Content Alpha"));
            assert!(text.native_properties.is_empty(), "{case}");
            assert!(text.native_properties_reason.as_deref().is_some_and(|reason| reason.contains(case)), "{case}");
        }
    }

    fn set(f:&mut AvbFile,index:u32,key:&'static str,value:Value){
        let Object::Decoded(n)=f.objects[index as usize].as_mut().unwrap()else{unreachable!()};
        if let Some((_,v))=n.fields.iter_mut().find(|(k,_)|*k==key){*v=value;}else{n.put(key,value);}
    }
    fn group_selector(f:&mut AvbFile,kind:i64,selected:i64,start:i64,length:i64,targets:&[MobId])->u32{
        let mut tracks=Vec::new();
        for &target in targets{
            let c=source(f,target,kind,1,start,length);set(f,c,"edit_rate",Value::Float(25.0));
            let mut t=node([0;4],"Track");t.put("component",Value::Ref(c));tracks.push(Value::Node(Box::new(t)));
        }
        let mut n=node(*b"SLCT","Selector");n.put("length",Value::Int(length));n.put("media_kind_id",Value::Int(kind));
        n.put("edit_rate",Value::Float(25.0));n.put("selected",Value::Int(selected));n.put("tracks",Value::List(tracks));add(f,n)
    }
    fn native_group_fixture()->AvbFile{
        let mut f=file();mob(&mut f,id(1),"Camera A",MOB_MASTER,0,&[]);mob(&mut f,id(2),"Camera B",MOB_MASTER,0,&[]);
        let mut fill=node(*b"FILL","Filler");fill.put("length",Value::Int(25));fill.put("media_kind_id",Value::Int(KIND_PICTURE));
        let fill=add(&mut f,fill);let a=source(&mut f,id(1),KIND_PICTURE,1,0,75);let a=segments(&mut f,KIND_PICTURE,&[fill,a]);
        mob(&mut f,id(10),"",MOB_COMPOSITION,5,&[(1,a)]);
        let b=source(&mut f,id(2),KIND_PICTURE,1,0,100);mob(&mut f,id(11),"",MOB_COMPOSITION,5,&[(1,b)]);
        let selector=group_selector(&mut f,KIND_PICTURE,0,0,100,&[id(10),id(11)]);
        mob(&mut f,id(20),"Group",MOB_COMPOSITION,4,&[(1,selector)]);
        // Selected camera A is still a group occurrence over its leading gap.
        let first=group_selector(&mut f,KIND_PICTURE,0,0,25,&[id(10),id(11)]);
        let second=group_selector(&mut f,KIND_PICTURE,1,25,25,&[id(10),id(11)]);
        let edits=segments(&mut f,KIND_PICTURE,&[first,second]);mob(&mut f,id(30),"Cuts",MOB_COMPOSITION,0,&[(1,edits)]);
        f
    }
    #[test]
    fn source_groups_retain_unused_angles_gaps_and_structural_selections(){
        use crate::source::SourceGroupPiece;
        let f=native_group_fixture();let out=source_project(&[f]);assert_eq!(out.groups.len(),1);
        assert!(out.skipped.is_empty(),"{:?}",out.skipped);
        let group=&out.groups[0];assert_eq!(group.lanes[0].alternatives.len(),2);
        assert!(matches!(&group.lanes[0].alternatives[0].pieces[..],[SourceGroupPiece::Gap{start:0,length:25},SourceGroupPiece::Media{start:25,length:75,source_in:0,..}]));
        assert!(matches!(&group.lanes[0].alternatives[1].pieces[..],[SourceGroupPiece::Media{start:0,length:100,..}]));
        let uses=&out.sequences[0].group_uses;assert_eq!(uses.len(),2);assert_eq!(uses[0].position,Some(0));assert!(uses[0].binding.placements.is_empty());
        assert_eq!(uses[1].position,Some(25));assert_eq!(uses[1].selected,Some(1));assert_eq!(uses[1].binding.placements.len(),1);
        let event=&out.sequences[0].events[0];assert_eq!(uses[1].binding.placements[0],event.id);assert_eq!(event.event.master,id(2));
    }
    #[test]
    fn unproven_group_tail_and_mixed_filler_clock_remain_unresolved(){
        for mixed in [false,true]{
            let mut f=native_group_fixture();
            let index=f.objects.iter().position(|o|matches!(o,Some(Object::Decoded(n))if n.class==*b"FILL")).unwrap()as u32;
            set(&mut f,index,if mixed{"edit_rate"}else{"length"},if mixed{Value::Float(24.0)}else{Value::Int(24)});
            let out=source_project(&[f]);let alt=&out.groups[0].lanes[0].alternatives[0];
            assert!(alt.pieces.is_empty());assert!(alt.reason.is_some(),"unobserved tail or mixed units cannot become a known gap");
        }
    }
    #[test]
    fn usage5_requires_full_id_reference_and_group_capture_is_bounded(){
        let mut f=native_group_fixture();mob(&mut f,id(99),"Unreferenced internal composition",MOB_COMPOSITION,5,&[]);
        let out=source_project(&[f]);assert_eq!(out.skipped.len(),1);assert!(out.skipped[0].reason.contains("not referenced"));
        let f=native_group_fixture();let inputs=source_inputs(1);
        let result=crate::source::extract_source_files(&inputs,&[&f],crate::source::SourceLimits{max_fact_bytes:2048});
        assert!(result.unwrap_err().message.contains("accounted"));
        // Standalone compatibility extraction keeps its original skip/flatten policy.
        let legacy=project(vec![f]);assert_eq!(legacy.sequences[0].events.len(),1);assert!(legacy.skipped.iter().any(|s|s.name=="Group"));
    }

}
