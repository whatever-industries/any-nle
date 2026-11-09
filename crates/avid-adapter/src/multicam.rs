//! Native group topology -> portable multicamera source facts. Matching uses
//! exact saved endpoints and equal relative offsets, never a camera label/time.
use super::*;
use avid_reader::io::MobId;

#[derive(Hash, PartialEq, Eq)]
struct Key {
    kind: i64,
    endpoints: Vec<(MobId, i64)>,
}
fn reserve<T>(b: &mut Budget, values: &mut Vec<T>, additional: usize) -> Result<(), String> {
    let required = values
        .len()
        .checked_add(additional)
        .ok_or("group vector length overflow")?;
    if required <= values.capacity() {
        return Ok(());
    }
    let capacity = required
        .max(
            values
                .capacity()
                .checked_mul(2)
                .ok_or("group capacity overflow")?,
        )
        .max(4);
    b.charge(
        (capacity - values.capacity())
            .checked_mul(size_of::<T>())
            .ok_or("group allocation overflow")?,
    )?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| "group allocation failed")?;
    if values.capacity() > capacity {
        b.charge(
            (values.capacity() - capacity)
                .checked_mul(size_of::<T>())
                .ok_or("group capacity overflow")?,
        )?;
    }
    Ok(())
}
fn key(
    kind: i64,
    endpoints: impl Iterator<Item = av::GroupEndpoint>,
    budget: &mut Budget,
) -> Result<Option<Key>, String> {
    let mut out = Vec::new();
    for e in endpoints {
        let (Some(mob), Some(track)) = (e.mob, e.track.filter(|n| *n > 0)) else {
            return Ok(None);
        };
        if e.class != *b"SCLP" {
            return Ok(None);
        }
        reserve(budget, &mut out, 1)?;
        out.push((mob, track));
    }
    Ok((!out.is_empty()).then_some(Key {
        kind,
        endpoints: out,
    }))
}
pub(super) struct Index<'a> {
    lanes: HashMap<Key, Vec<(&'a av::SourceGroup, &'a av::SourceGroupLane)>>,
}
fn portable_kind(kind: Option<i64>) -> Option<StreamKind> {
    match kind {
        Some(KIND_PICTURE) => Some(StreamKind::Video),
        Some(KIND_SOUND) => Some(StreamKind::Audio),
        _ => None,
    }
}
fn uncertainty(s: &str, b: &mut Budget) -> Result<Option<String>, String> {
    b.charge(size_of::<String>() + s.len())?;
    Ok(Some(s.into()))
}
fn member<'a>(
    mob: MobId,
    bin: usize,
    copies: &HashMap<MobId, Vec<(usize, usize)>>,
    out: &'a ReadResult,
) -> Option<&'a SourceItem> {
    let all = copies.get(&mob)?;
    let mut local = all.iter().filter(|(b, _)| *b == bin);
    let index = match (local.next(), local.next()) {
        (Some((_, i)), None) => *i,
        (None, None) if all.len() == 1 => all[0].1,
        _ => return None,
    };
    out.graph.items.get(index)
}
pub(super) fn append<'a>(
    groups: &'a [av::SourceGroup],
    copies: &HashMap<MobId, Vec<(usize, usize)>>,
    out: &mut ReadResult,
    budget: &mut Budget,
) -> Result<Index<'a>, String> {
    let mut index = Index {
        lanes: HashMap::new(),
    };
    for g in groups {
        reserve(budget, &mut out.graph.multicams, 1)?;
        budget.charge(g.id.len() + g.name.len() + g.reason.as_ref().map_or(0, String::len))?;
        let mut m = Multicam {
            id: g.id.clone(),
            name: g.name.clone(),
            rate: g.rate.map(rational),
            duration: g
                .length
                .filter(|n| *n > 0)
                .zip(g.rate)
                .map(|(n, r)| time(n, r))
                .transpose()?,
            lanes: Vec::new(),
            reason: g.reason.clone(),
        };
        for l in &g.lanes {
            let (Some(kind), Some(label)) = (portable_kind(l.kind), l.label.filter(|n| *n > 0))
            else {
                m.reason =
                    uncertainty("native group includes an unresolved lane identity", budget)?;
                continue;
            };
            reserve(budget, &mut m.lanes, 1)?;
            budget.charge(l.reason.as_ref().map_or(0, String::len))?;
            let mut lane = MulticamLane {
                kind,
                index: label,
                selected: l
                    .selected
                    .filter(|n| *n >= 0 && (*n as usize) < l.alternatives.len()),
                alternatives: Vec::new(),
                reason: l.reason.clone(),
            };
            for (i, a) in l.alternatives.iter().enumerate() {
                reserve(budget, &mut lane.alternatives, 1)?;
                let layout_reason = a
                    .reason
                    .as_ref()
                    .or(l.reason.as_ref())
                    .or(g.reason.as_ref());
                budget.charge(g.id.len() + 128 + layout_reason.map_or(0, String::len))?;
                let mut alt = MulticamAlternative {
                    id: format!("{}/lane/{}/{}/alternative/{i}", g.id, kind.as_str(), label),
                    pieces: Vec::new(),
                    reason: layout_reason.cloned(),
                };
                for p in &a.pieces {
                    reserve(budget, &mut alt.pieces, 1)?;
                    let Some(rate) = g.rate else {
                        alt.reason = uncertainty("group piece clock unresolved", budget)?;
                        break;
                    };
                    let (start, length, source) = match p {
                        av::SourceGroupPiece::Gap { start, length } => (*start, *length, None),
                        av::SourceGroupPiece::Media {
                            start,
                            length,
                            master,
                            track,
                            source_in,
                            rate: source_rate,
                        } => {
                            let Some(item) = member(*master, g.bin, copies, out) else {
                                alt.reason=uncertainty("group member is missing, unsupported or ambiguous in its source bin",budget)?;
                                break;
                            };
                            budget.charge(item.id.len())?;
                            (
                                *start,
                                *length,
                                Some((item.id.clone(), *track, time(*source_in, *source_rate)?)),
                            )
                        }
                    };
                    let (item, stream, source_in) = match source {
                        Some((id, stream, source_in)) => (Some(id), Some(stream), Some(source_in)),
                        None => (None, None, None),
                    };
                    alt.pieces.push(MulticamPiece {
                        start: time(start, rate)?,
                        duration: time(length, rate)?,
                        item,
                        stream,
                        source_in,
                    });
                }
                lane.alternatives.push(alt);
            }
            if lane.selected.is_none() && lane.reason.is_none() {
                lane.reason = uncertainty("native default alternative is unresolved", budget)?;
            }
            m.lanes.push(lane);
            if let Some(k) = key(
                l.kind.unwrap(),
                l.alternatives.iter().map(|a| a.endpoint),
                budget,
            )? {
                budget.charge(
                    4 * (size_of::<Key>() + size_of::<(&av::SourceGroup, &av::SourceGroupLane)>()),
                )?;
                let values = index.lanes.entry(k).or_default();
                reserve(budget, values, 1)?;
                values.push((g, l));
            }
        }
        let bin = out
            .graph
            .bins
            .get_mut(g.bin)
            .ok_or("group references unknown source bin")?;
        reserve(budget, &mut bin.items, 1)?;
        budget.charge(g.id.len())?;
        bin.items.push(g.id.clone());
        if m.lanes.is_empty() && m.reason.is_none() {
            m.reason = uncertainty("native group has no resolved picture/audio lane", budget)?;
        }
        out.graph.multicams.push(m);
        // Reserve native value expansion before allocating the receipt. These
        // fields are provenance, not inferred authoring method or timecode.
        let mut reservation = Budget { used: budget.used };
        reservation.charge(8192 + g.id.len() * 3 + g.reason.as_ref().map_or(0, String::len))?;
        for l in &g.lanes {
            reservation.charge(4096 + l.reason.as_ref().map_or(0, String::len))?;
            for a in &l.alternatives {
                reservation.charge(
                    4096 + a
                        .pieces
                        .len()
                        .checked_mul(2048)
                        .ok_or("group receipt budget overflow")?
                        + a.reason.as_ref().map_or(0, String::len),
                )?;
            }
        }
        for (k, v) in &g.marks {
            reservation.charge(1024 + k.len())?;
            if let av::Observed::Unresolved(r) = v {
                reservation.charge(r.len())?;
            }
        }
        for marker in &g.markers {
            reserve_marker(marker, &mut reservation)?;
        }
        sidecar(out, budget, "avid.group", &g.id, native(g))?;
    }
    Ok(index)
}
pub(super) fn endpoint(e: av::GroupEndpoint) -> Value {
    obj(vec![
        ("class", s(String::from_utf8_lossy(&e.class))),
        ("kind", e.kind.map(Value::Int).unwrap_or(Value::Null)),
        ("mob", e.mob.map(|m| s(m.urn())).unwrap_or(Value::Null)),
        ("track", e.track.map(Value::Int).unwrap_or(Value::Null)),
        ("start", e.start.map(Value::Int).unwrap_or(Value::Null)),
        ("length", e.length.map(Value::Int).unwrap_or(Value::Null)),
        (
            "rate",
            e.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
        ),
    ])
}
fn native(g: &av::SourceGroup) -> Value {
    obj(vec![
        ("mob", g.mob.map(|m| s(m.urn())).unwrap_or(Value::Null)),
        (
            "stored_rate",
            g.stored_rate.map(raw_float).unwrap_or(Value::Null),
        ),
        (
            "native_length",
            g.length.map(Value::Int).unwrap_or(Value::Null),
        ),
        ("reason", opt_s(g.reason.as_deref())),
        (
            "authoring_method",
            s("unknown; name and usage4 do not identify the Group versus MultiGroup command"),
        ),
        (
            "marks",
            obj(g
                .marks
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str(),
                        match v {
                            av::Observed::Known(Some(n)) => Value::Int(*n),
                            av::Observed::Known(None) => Value::Null,
                            av::Observed::Unresolved(r) => obj(vec![("unresolved", s(r))]),
                        },
                    )
                })
                .collect()),
        ),
        (
            "markers",
            Value::Arr(
                g.markers
                    .iter()
                    .map(|m| {
                        obj(vec![
                            ("name", s(&m.name)),
                            ("comment", s(&m.comment)),
                            ("user", s(&m.user)),
                            ("color", s(&m.color)),
                            ("native", marker_native(m)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "lanes",
            Value::Arr(
                g.lanes
                    .iter()
                    .map(|l| {
                        obj(vec![
                            ("kind", l.kind.map(Value::Int).unwrap_or(Value::Null)),
                            (
                                "is_ganged",
                                l.is_ganged.map(Value::Bool).unwrap_or(Value::Null),
                            ),
                            ("label", l.label.map(Value::Int).unwrap_or(Value::Null)),
                            (
                                "selected",
                                l.selected.map(Value::Int).unwrap_or(Value::Null),
                            ),
                            (
                                "rate",
                                l.rate.map(|r| s(rational(r))).unwrap_or(Value::Null),
                            ),
                            ("reason", opt_s(l.reason.as_deref())),
                            ("length", l.length.map(Value::Int).unwrap_or(Value::Null)),
                            (
                                "alternatives",
                                Value::Arr(
                                    l.alternatives
                                        .iter()
                                        .map(|a| {
                                            obj(vec![
                                                ("endpoint", endpoint(a.endpoint)),
                                                ("reason", opt_s(a.reason.as_deref())),
                                                (
                                                    "members",
                                                    Value::Arr(
                                                        a.pieces
                                                            .iter()
                                                            .filter_map(|p| match p {
                                                                av::SourceGroupPiece::Media {
                                                                    master,
                                                                    track,
                                                                    start,
                                                                    length,
                                                                    source_in,
                                                                    rate,
                                                                } => Some(obj(vec![
                                                                    ("master", s(master.urn())),
                                                                    ("track", Value::Int(*track)),
                                                                    ("start", Value::Int(*start)),
                                                                    ("length", Value::Int(*length)),
                                                                    (
                                                                        "source_in",
                                                                        Value::Int(*source_in),
                                                                    ),
                                                                    ("rate", s(rational(*rate))),
                                                                ])),
                                                                _ => None,
                                                            })
                                                            .collect(),
                                                    ),
                                                ),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}
fn relative_start(
    u: &av::SourceGroupUse,
    g: &av::SourceGroup,
    l: &av::SourceGroupLane,
) -> Option<i64> {
    if u.rate != g.rate || l.rate != g.rate || u.alternatives.len() != l.alternatives.len() {
        return None;
    }
    let mut source = None;
    for (a, b) in u.alternatives.iter().zip(&l.alternatives) {
        if a.kind != Some(u.kind) || a.rate != u.rate || a.length != u.length {
            return None;
        }
        let delta = a.start?.checked_sub(b.endpoint.start?)?;
        if delta < 0 || source.is_some_and(|n| n != delta) {
            return None;
        }
        source = Some(delta);
    }
    let source = source?;
    let end = source.checked_add(u.length?)?;
    (u.length? > 0 && end <= g.length?).then_some(source)
}
impl Index<'_> {
    pub(super) fn append_uses(
        &self,
        seq: &av::SourceSequence,
        map: &HashMap<String, String>,
        out: &mut ReadResult,
        budget: &mut Budget,
    ) -> Result<(), String> {
        for u in &seq.group_uses {
            reserve(budget, &mut out.graph.multicam_uses, 1)?;
            budget.charge(u.id.len() + seq.id.len() + u.reason.as_ref().map_or(0, String::len))?;
            let Some(kind) = portable_kind(Some(u.kind)) else {
                continue;
            };
            let mut candidates = Vec::new();
            if let Some(k) = key(u.kind, u.alternatives.iter().copied(), budget)? {
                if let Some(all) = self.lanes.get(&k) {
                    let has_local = all
                        .iter()
                        .any(|(g, l)| g.bin == seq.info.bin && relative_start(u, g, l).is_some());
                    for &(g, l) in all {
                        if has_local && g.bin != seq.info.bin {
                            continue;
                        }
                        if let Some(start) = relative_start(u, g, l) {
                            reserve(budget, &mut candidates, 1)?;
                            candidates.push((g, l, start));
                        }
                    }
                }
            }
            let unique = candidates.len() == 1;
            let reason = if u.reason.is_some() {
                u.reason.clone()
            } else if !unique {
                uncertainty(
                    "native group occurrence has no unique exact endpoint/offset binding",
                    budget,
                )?
            } else {
                None
            };
            let mut binding = SourceBinding {
                placements: Vec::new(),
                reason: u.binding.reason.clone(),
            };
            let mut missing = false;
            budget.charge(u.binding.reason.as_ref().map_or(0, String::len))?;
            let mut seen = HashSet::new();
            for id in &u.binding.placements {
                if let Some(id) = map.get(id) {
                    budget.charge(4 * size_of::<&String>())?;
                    if seen.insert(id) {
                        reserve(budget, &mut binding.placements, 1)?;
                        budget.charge(id.len())?;
                        binding.placements.push(id.clone());
                    }
                } else {
                    missing = true;
                }
            }
            if missing {
                binding.reason = uncertainty(
                    "one or more native group descendants are not portable placements",
                    budget,
                )?;
            }
            let mut ids = Vec::new();
            let mut group_ids = HashSet::new();
            for (g, _, _) in &candidates {
                budget.charge(4 * size_of::<&String>())?;
                if !group_ids.insert(&g.id) {
                    continue;
                }
                reserve(budget, &mut ids, 1)?;
                budget.charge(g.id.len())?;
                ids.push(g.id.clone());
            }
            let selected = u
                .selected
                .filter(|n| *n >= 0 && (*n as usize) < u.alternatives.len());
            let reason = if selected.is_none() && reason.is_none() {
                uncertainty("native group use selection is unresolved", budget)?
            } else {
                reason
            };
            out.graph.multicam_uses.push(MulticamUse {
                id: u.id.clone(),
                sequence: seq.id.clone(),
                candidates: ids,
                kind,
                lane: unique.then(|| candidates[0].1.label).flatten(),
                track: u.track,
                selected,
                record_start: u
                    .position
                    .zip(u.rate)
                    .map(|(n, r)| time(n, r))
                    .transpose()?,
                duration: u
                    .length
                    .filter(|n| *n > 0)
                    .zip(u.rate)
                    .map(|(n, r)| time(n, r))
                    .transpose()?,
                source_in: if unique {
                    Some(time(
                        candidates[0].2,
                        u.rate.ok_or("matched group without source rate")?,
                    )?)
                } else {
                    None
                },
                binding,
                reason,
            });
        }
        Ok(())
    }
}
