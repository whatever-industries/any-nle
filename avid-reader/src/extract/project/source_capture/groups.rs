//! Full-ID, lane-specific native group facts. This does not interpret a camera
//! letter from an audio ordinal, a name, or a flattened edit timestamp.
use super::*;

fn endpoint(node: &Node) -> GroupEndpoint {
    GroupEndpoint {
        class: node.class,
        kind: int(node, "media_kind_id"),
        rate: float(node, "edit_rate").and_then(Rate::from_avid),
        mob: mob_id(node),
        track: int(node, "track_id"),
        start: int(node, "start_time"),
        length: int(node, "length"),
    }
}
fn reason(b: &SharedBudget, text: &str) -> Option<String> {
    Some(copy_text(b, text))
}
fn lane(ctx: &Ctx<'_>, node: &Node, label: Option<i64>, b: &SharedBudget) -> SourceGroupLane {
    let mut out = SourceGroupLane {
        is_ganged: node.get("is_ganged").and_then(|v| {
            if let Value::Bool(b) = v {
                Some(*b)
            } else {
                None
            }
        }),
        kind: int(node, "media_kind_id"),
        label,
        rate: float(node, "edit_rate").and_then(Rate::from_avid),
        length: int(node, "length"),
        selected: int(node, "selected"),
        alternatives: Vec::new(),
        reason: None,
    };
    if node.class != *b"SLCT" {
        out.reason = reason(b, "group lane is not an immediate selector");
        return out;
    }
    let tracks = ctx.tracks(node);
    if tracks.is_empty() {
        out.reason = reason(b, "group selector has no alternatives");
    }
    for track in tracks {
        let Some(n) = ctx.deref(track.get("component")) else {
            out.reason = reason(b, "group alternative is missing or undecoded");
            continue;
        };
        if !reserve(b, &mut out.alternatives, 1) {
            break;
        }
        out.alternatives.push(SourceGroupAlternative {
            endpoint: endpoint(n),
            pieces: Vec::new(),
            reason: if n.class == *b"SCLP" {
                None
            } else {
                reason(b, "group alternative is not an immediate source clip")
            },
        });
    }
    if out.label.is_none_or(|n| n <= 0)
        || !matches!(out.kind, Some(KIND_PICTURE | KIND_SOUND))
        || out.rate.is_none()
        || out.length.is_none_or(|n| n <= 0)
        || out
            .selected
            .is_none_or(|n| n < 0 || n as usize >= out.alternatives.len())
    {
        out.reason = reason(
            b,
            "group lane label, kind, clock, span or selected index is unresolved",
        );
    }
    out
}
impl InputContext {
    pub(in crate::extract::project) fn group(
        &self,
        ctx: &Ctx<'_>,
        mob: &Node,
        item: usize,
    ) -> SourceGroup {
        charge(&self.budget, size_of::<SourceGroup>());
        let id = formatted(
            &self.budget,
            format_args!("{}/bin/{}/item/{}", self.revision, self.bin, item),
        );
        let mut out = SourceGroup {
            id,
            bin: self.bin,
            mob: mob_id(mob),
            name: copy_text(
                &self.budget,
                mob.get("name").and_then(Value::as_str).unwrap_or(""),
            ),
            rate: float(mob, "edit_rate").and_then(Rate::from_avid),
            stored_rate: float(mob, "edit_rate"),
            length: int(mob, "length"),
            lanes: Vec::new(),
            marks: Vec::new(),
            markers: Vec::new(),
            reason: None,
        };
        if out.mob.is_none() || out.rate.is_none() || out.length.is_none_or(|n| n <= 0) {
            out.reason = reason(
                &self.budget,
                "native group identity, rate or length is unresolved",
            );
        }
        let mut labels = HashSet::new();
        for t in ctx.tracks(mob) {
            let Some(n) = ctx.deref(t.get("component")) else {
                out.reason = reason(
                    &self.budget,
                    "group track component is missing or undecoded",
                );
                continue;
            };
            let kind = int(n, "media_kind_id");
            if !matches!(kind, Some(KIND_PICTURE | KIND_SOUND)) {
                continue;
            }
            let label = int(t, "index");
            if !charge(&self.budget, 4 * size_of::<(Option<i64>, Option<i64>)>()) {
                break;
            }
            if !labels.insert((kind, label)) {
                self.budget.borrow_mut().fail("duplicate native group lane");
                break;
            }
            if !reserve(&self.budget, &mut out.lanes, 1) {
                break;
            }
            out.lanes.push(lane(ctx, n, label, &self.budget));
        }
        for key in ["_MLT_GRP", "_IN", "_OUT"] {
            if !reserve(&self.budget, &mut out.marks, 1) {
                break;
            }
            let mut found = None;
            for (name, value) in borrowed_attrs(ctx, mob) {
                if name == key {
                    if found.is_some() {
                        self.budget
                            .borrow_mut()
                            .fail("duplicate native group attribute");
                        break;
                    }
                    found = Some(value);
                }
            }
            let value = match found {
                None => Observed::Known(None),
                Some(Value::Int(n)) => Observed::Known(Some(*n)),
                _ => Observed::Unresolved(copy_text(
                    &self.budget,
                    "native group attribute is not an integer",
                )),
            };
            out.marks.push((copy_text(&self.budget, key), value));
        }
        each_marker(ctx, mob, |m| {
            if reserve(&self.budget, &mut out.markers, 1) {
                out.markers.push(read_marker(ctx, m, &self.budget).0);
            }
        });
        out
    }
}

pub(super) struct UseFact {
    scope: usize,
    value: SourceGroupUse,
}
#[allow(clippy::too_many_arguments)]
pub(super) fn capture_use(
    c: &mut Capture,
    ctx: &Ctx<'_>,
    node: &Node,
    scope: usize,
    kind: i64,
    track: i64,
    position: i64,
    rate: Option<Rate>,
) {
    if node.class != *b"SLCT" || !matches!(kind, KIND_PICTURE | KIND_SOUND) {
        return;
    }
    let inputs = ctx.tracks(node);
    // Other selector shapes (including mute wrappers) keep their existing
    // interpretation. Do not advertise them as a qualified group occurrence.
    if inputs.is_empty()
        || !inputs.iter().all(|t| {
            ctx.deref(t.get("component")).is_some_and(|n| {
                n.class == *b"SCLP"
                    && mob_id(n).is_some_and(|id| {
                        ctx.mobs
                            .get(&id)
                            .is_none_or(|m| int(m, "usage_code") == Some(5))
                    })
            })
        })
    {
        return;
    }
    if !reserve(&c.budget, &mut c.group_uses, 1) {
        return;
    }
    let mut alternatives = Vec::new();
    for t in inputs {
        if !reserve(&c.budget, &mut alternatives, 1) {
            return;
        }
        alternatives.push(endpoint(ctx.deref(t.get("component")).unwrap()));
    }
    let value = SourceGroupUse {
        is_ganged: node.get("is_ganged").and_then(|v| {
            if let Value::Bool(b) = v {
                Some(*b)
            } else {
                None
            }
        }),
        id: formatted(&c.budget, format_args!("{}/group-use", c.scopes[scope].id)),
        kind,
        track,
        position: Some(position),
        length: int(node, "length"),
        rate,
        selected: int(node, "selected"),
        alternatives,
        binding: Binding {
            placements: Vec::new(),
            reason: None,
        },
        reason: None,
    };
    c.group_uses.push(UseFact { scope, value });
}
pub(super) fn finish_uses(
    facts: &mut Vec<UseFact>,
    scopes: &[Scope],
    starts: &[usize],
    events: &[SourceEvent],
    issues: &[SourceIssue],
    b: &SharedBudget,
) -> Vec<SourceGroupUse> {
    let mut out = Vec::new();
    for mut f in facts.drain(..) {
        let scope = &scopes[f.scope];
        let known = scope.position_known
            && scope.group_path
            && !scope.retimed
            && f.value.rate.is_some()
            && f.value.length.is_some_and(|n| n > 0);
        if !known {
            f.value.position = None;
            f.value.reason = reason(
                b,
                "group occurrence owner clock or enclosing span is unqualified",
            );
        }
        f.value.binding = binding_scope(scope, starts, events, issues, b);
        if reserve(b, &mut out, 1) {
            out.push(f.value);
        }
    }
    out
}

// The ordinary resolver intentionally permits composition extent outside emitted
// leaves. A group gap is stronger: prove full requested coverage by native FILL
// or source segments before treating the complement of media as a known gap.
#[allow(clippy::too_many_arguments)]
fn observed_layout(
    routes: &HashMap<MobId, MobRoute>,
    id: MobId,
    label: i64,
    start: i64,
    length: i64,
    kind: i64,
    rate: Rate,
    depth: usize,
    b: &SharedBudget,
) -> Result<(), &'static str> {
    // Conservatively charge traversal witnesses too: a shared/branching source
    // DAG cannot consume unbounded work before the fact allocator gets a turn.
    if !charge(b, size_of::<RouteWitness>()) {
        return Err("group traversal exceeds source fact budget");
    }
    if depth > MAX_NESTING {
        return Err("group source chain is too deep");
    }
    let end = start
        .checked_add(length)
        .filter(|_| start >= 0 && length > 0)
        .ok_or("group source range is invalid")?;
    let mob = routes.get(&id).ok_or("group source mob is unavailable")?;
    if mob.rate != Some(rate) {
        return Err("group source chain has mixed or unknown clocks");
    }
    if mob.length.is_none_or(|n| end > n) {
        return Err("group source span exceeds known native extent");
    }
    if mob.kind == Some(MOB_MASTER) {
        return Ok(());
    }
    if mob.kind != Some(MOB_COMPOSITION) {
        return Err("group source chain does not reach a master");
    }
    let mut tracks = mob
        .tracks
        .iter()
        .filter(|t| t.label == Some(label) && t.kind == Some(kind));
    let track = tracks.next().ok_or("group source track is missing")?;
    if tracks.next().is_some() || track.rate != Some(rate) || !track.range_qualified {
        return Err("group source track is ambiguous or unqualified");
    }
    let mut at = start;
    for seg in track.segments.iter() {
        if !charge(b, size_of::<RouteSegment>()) {
            return Err("group traversal exceeds source fact budget");
        }
        let n = seg
            .observed_length
            .ok_or("group segment length is unknown")?;
        let seg_end = seg
            .position
            .checked_add(n)
            .filter(|_| n >= 0)
            .ok_or("group segment extent is invalid")?;
        let lo = start.max(seg.position);
        let hi = end.min(seg_end);
        if lo >= hi {
            continue;
        }
        if lo != at {
            return Err("group source coverage has an unexplained gap or overlap");
        }
        if (if seg.rate_present {
            seg.rate
        } else {
            track.rate
        }) != Some(rate)
        {
            return Err("group segment clock is unqualified");
        }
        match seg.action.as_ref() {
            RouteAction::Empty => {}
            RouteAction::Source {
                id: Some(inner),
                observed_label: Some(label),
                observed_start: Some(source),
                rate: component_rate,
                rate_present,
                ..
            } => {
                if (if *rate_present {
                    *component_rate
                } else {
                    track.rate
                }) != Some(rate)
                {
                    return Err("group source component clock is unqualified");
                }
                let source = source
                    .checked_add(lo - seg.position)
                    .ok_or("group source offset overflows")?;
                observed_layout(
                    routes,
                    *inner,
                    *label,
                    source,
                    hi - lo,
                    kind,
                    rate,
                    depth + 1,
                    b,
                )?;
            }
            _ => {
                return Err(
                    "group layout contains an unsupported selector, effect or source branch",
                )
            }
        }
        at = hi;
    }
    if at != end {
        return Err("group source coverage ends before the requested extent");
    }
    Ok(())
}
pub(super) fn resolve_group(
    group: &mut SourceGroup,
    routes: &HashMap<MobId, MobRoute>,
    b: &SharedBudget,
) {
    let (Some(rate), Some(length)) = (group.rate, group.length.filter(|n| *n > 0)) else {
        return;
    };
    for lane in &mut group.lanes {
        if lane.reason.is_some() {
            continue;
        }
        if lane.rate != Some(rate) || lane.length != Some(length) {
            lane.reason = reason(b, "group lane clock or extent differs from group owner");
            continue;
        }
        for alt in &mut lane.alternatives {
            if alt.reason.is_some() {
                continue;
            }
            let e = alt.endpoint;
            let (Some(mob), Some(label), Some(start)) = (
                e.mob,
                e.track.filter(|n| *n > 0),
                e.start.filter(|n| *n >= 0),
            ) else {
                alt.reason = reason(b, "group source reference lacks identity, track or offset");
                continue;
            };
            if e.kind != lane.kind || e.rate != Some(rate) || e.length != Some(length) {
                alt.reason = reason(
                    b,
                    "group alternative clock, kind or extent differs from its owner",
                );
                continue;
            }
            if let Err(issue) = observed_layout(
                routes,
                mob,
                label,
                start,
                length,
                lane.kind.unwrap(),
                rate,
                0,
                b,
            ) {
                alt.reason = reason(b, issue);
                continue;
            }
            let mut pieces = Vec::new();
            let mut issues = Vec::new();
            let context = IssueContext {
                owner: &group.id,
                record: 0,
                position_known: true,
                rate: Some(rate),
                kind: lane.kind.unwrap(),
                track: lane.label.unwrap(),
            };
            qualified_resolve(
                routes,
                mob,
                label,
                start,
                length,
                0,
                lane.kind.unwrap(),
                rate,
                0,
                Vec::new(),
                &context,
                b,
                &mut pieces,
                &mut issues,
            );
            if let Some(issue) = issues.first() {
                alt.reason = reason(b, &issue.reason);
                continue;
            }
            pieces.sort_by_key(|p| p.record);
            let mut at = 0;
            for p in pieces {
                let Some(end) = p.record.checked_add(p.length) else {
                    alt.reason = reason(b, "group piece endpoint overflows");
                    break;
                };
                if p.record < at || p.length <= 0 || end > length {
                    alt.reason = reason(b, "group pieces overlap or exceed owner extent");
                    break;
                }
                if p.record > at && reserve(b, &mut alt.pieces, 1) {
                    alt.pieces.push(SourceGroupPiece::Gap {
                        start: at,
                        length: p.record - at,
                    });
                }
                if !reserve(b, &mut alt.pieces, 1) {
                    break;
                }
                alt.pieces.push(SourceGroupPiece::Media {
                    start: p.record,
                    length: p.length,
                    master: p.master,
                    track: p.label,
                    source_in: p.source,
                    rate: p.rate,
                });
                at = end;
            }
            if alt.reason.is_none() && at < length && reserve(b, &mut alt.pieces, 1) {
                alt.pieces.push(SourceGroupPiece::Gap {
                    start: at,
                    length: length - at,
                });
            }
        }
    }
}
