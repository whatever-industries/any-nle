//! Source multicamera topology. A lane's alternatives have independent identities;
//! audio indices never imply a video camera identity. Gaps are explicit pieces.
use super::*;
use crate::validation::{list, rate, require, text as check_text};

#[derive(Clone, Debug, PartialEq)]
pub struct Multicam {
    pub id: String,
    pub name: String,
    pub rate: Option<Rational>,
    pub duration: Option<Rational>,
    pub lanes: Vec<MulticamLane>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MulticamLane {
    pub kind: StreamKind,
    pub index: i64,
    pub selected: Option<i64>,
    pub alternatives: Vec<MulticamAlternative>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MulticamAlternative {
    pub id: String,
    pub pieces: Vec<MulticamPiece>,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MulticamPiece {
    pub start: Rational,
    pub duration: Rational,
    /// All three source fields absent means an observed gap, never an unknown offset.
    pub item: Option<String>,
    pub stream: Option<i64>,
    pub source_in: Option<Rational>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MulticamUse {
    pub id: String,
    pub sequence: String,
    pub candidates: Vec<String>,
    pub kind: StreamKind,
    pub lane: Option<i64>,
    pub track: i64,
    pub selected: Option<i64>,
    pub record_start: Option<Rational>,
    pub duration: Option<Rational>,
    pub source_in: Option<Rational>,
    pub binding: SourceBinding,
    pub reason: Option<String>,
}
fn check_reason(reason: &Option<String>, w: &str) -> R<()> {
    if let Some(r) = reason {
        check_text(r, w)?;
        require(!r.is_empty(), w, "empty multicam uncertainty")?;
    }
    Ok(())
}
fn kind(o: &Obj<'_>) -> R<StreamKind> {
    StreamKind::parse(&o.str("kind")?)
        .ok_or_else(|| GraphError("invalid multicam lane kind".into()))
}
impl Multicam {
    pub(super) fn parse(v: &Value, w: &str) -> R<Self> {
        let o = Obj::new(
            v,
            w,
            &["id", "name", "lanes"],
            &["rate", "duration", "reason"],
        )?;
        let mut lanes = Vec::new();
        for (i, v) in o.list("lanes")?.iter().enumerate() {
            let w = format!("{w}.lanes[{i}]");
            let l = Obj::new(
                v,
                &w,
                &["kind", "index", "alternatives"],
                &["selected", "reason"],
            )?;
            let mut alternatives = Vec::new();
            for (j, v) in l.list("alternatives")?.iter().enumerate() {
                let w = format!("{w}.alternatives[{j}]");
                let a = Obj::new(v, &w, &["id", "pieces"], &["reason"])?;
                let mut pieces = Vec::new();
                for (k, v) in a.list("pieces")?.iter().enumerate() {
                    let p = Obj::new(
                        v,
                        &format!("{w}.pieces[{k}]"),
                        &["start", "duration"],
                        &["item", "stream", "source_in"],
                    )?;
                    pieces.push(MulticamPiece {
                        start: p.time("start")?,
                        duration: p.time("duration")?,
                        item: p.opt_str("item")?,
                        stream: p.opt_int("stream")?,
                        source_in: p.opt_time("source_in")?,
                    });
                }
                alternatives.push(MulticamAlternative {
                    id: a.str("id")?,
                    pieces,
                    reason: a.opt_str("reason")?,
                });
            }
            lanes.push(MulticamLane {
                kind: kind(&l)?,
                index: l.int("index")?,
                selected: l.opt_int("selected")?,
                alternatives,
                reason: l.opt_str("reason")?,
            });
        }
        Ok(Self {
            id: o.str("id")?,
            name: o.str("name")?,
            rate: o.opt_time("rate")?,
            duration: o.opt_time("duration")?,
            lanes,
            reason: o.opt_str("reason")?,
        })
    }
}
impl MulticamUse {
    pub(super) fn parse(v: &Value, w: &str) -> R<Self> {
        let o = Obj::new(
            v,
            w,
            &["id", "sequence", "candidates", "kind", "track", "binding"],
            &[
                "lane",
                "selected",
                "record_start",
                "duration",
                "source_in",
                "reason",
            ],
        )?;
        Ok(Self {
            id: o.str("id")?,
            sequence: o.str("sequence")?,
            candidates: o
                .list("candidates")?
                .iter()
                .map(|v| crate::text(v, w))
                .collect::<R<_>>()?,
            kind: kind(&o)?,
            lane: o.opt_int("lane")?,
            track: o.int("track")?,
            selected: o.opt_int("selected")?,
            record_start: o.opt_time("record_start")?,
            duration: o.opt_time("duration")?,
            source_in: o.opt_time("source_in")?,
            binding: parse_binding(o.req("binding"), w)?,
            reason: o.opt_str("reason")?,
        })
    }
}
pub(super) fn validate(g: &SourceGraph, media: &HashMap<&str, &SourceMedia>) -> R<()> {
    if g.multicams.is_empty() && g.multicam_uses.is_empty() {
        return Ok(());
    }
    let items: HashMap<_, _> = g.items.iter().map(|i| (i.id.as_str(), i)).collect();
    let groups: HashMap<_, _> = g.multicams.iter().map(|m| (m.id.as_str(), m)).collect();
    for m in &g.multicams {
        let w = format!("multicam {:?}", m.id);
        check_text(&m.name, &w)?;
        check_reason(&m.reason, &w)?;
        list(m.lanes.len(), &w)?;
        if let Some(r) = m.rate {
            rate(r, &w)?;
        }
        if let Some(d) = m.duration {
            require(d.num() > 0, &w, "nonpositive multicam duration")?;
        }
        require(
            m.reason.is_some() || (m.rate.is_some() && m.duration.is_some() && !m.lanes.is_empty()),
            &w,
            "unresolved multicam requires a reason",
        )?;
        let mut lanes = HashSet::new();
        for lane in &m.lanes {
            require(
                lane.index > 0 && lanes.insert((lane.kind, lane.index)),
                &w,
                "invalid or duplicate multicam lane",
            )?;
            check_reason(&lane.reason, &w)?;
            list(lane.alternatives.len(), &w)?;
            if let Some(n) = lane.selected {
                require(
                    n >= 0 && (n as usize) < lane.alternatives.len(),
                    &w,
                    "multicam selected alternative outside lane",
                )?;
            }
            require(
                lane.reason.is_some() || (!lane.alternatives.is_empty() && lane.selected.is_some()),
                &w,
                "unresolved multicam lane requires a reason",
            )?;
            for a in &lane.alternatives {
                check_reason(&a.reason, &w)?;
                list(a.pieces.len(), &w)?;
                let mut at = Rational::ZERO;
                for p in &a.pieces {
                    require(
                        p.start == at && p.duration.num() > 0,
                        &w,
                        "multicam pieces must form an explicit contiguous partition",
                    )?;
                    at = p
                        .start
                        .checked_add(p.duration)
                        .ok_or_else(|| GraphError("multicam piece end overflows".into()))?;
                    if let Some(end) = m.duration {
                        require(at <= end, &w, "multicam piece exceeds owner duration")?;
                    }
                    match (&p.item, p.stream, p.source_in) {
                        (None, None, None) => {}
                        (Some(id), Some(stream), Some(source_in)) => {
                            require(
                                stream > 0
                                    && !source_in.is_negative()
                                    && source_in.checked_add(p.duration).is_some(),
                                &w,
                                "invalid multicam source range",
                            )?;
                            let item = items.get(id.as_str()).ok_or_else(|| {
                                GraphError(format!("{w}: unknown multicam member {id:?}"))
                            })?;
                            require(
                                item.media
                                    .iter()
                                    .filter_map(|im| media.get(im.media.as_str()))
                                    .any(|m| {
                                        m.kind == lane.kind
                                            && (m.stream == Some(stream)
                                                || (m.kind == StreamKind::Video
                                                    && m.stream.is_none()
                                                    && stream == 1))
                                    }),
                                &w,
                                "multicam member has no matching source stream",
                            )?;
                        }
                        _ => {
                            return fail(format!(
                                "{w}: multicam gap/source fields must be distinct and complete"
                            ))
                        }
                    }
                }
                require(
                    a.reason.is_some() || m.duration == Some(at),
                    &w,
                    "multicam alternative does not cover owner duration",
                )?;
            }
        }
    }
    let sequences: HashMap<_, _> = g.sequences.iter().map(|s| (s.id.as_str(), s)).collect();
    let placements: HashMap<_, _> = g
        .sequences
        .iter()
        .flat_map(|s| {
            s.placements
                .iter()
                .map(move |p| (p.id.as_str(), s.id.as_str()))
        })
        .collect();
    for u in &g.multicam_uses {
        let w = format!("multicam use {:?}", u.id);
        check_reason(&u.reason, &w)?;
        list(u.candidates.len(), &w)?;
        list(u.binding.placements.len(), &w)?;
        check_reason(&u.binding.reason, &w)?;
        let sequence = sequences
            .get(u.sequence.as_str())
            .ok_or_else(|| GraphError(format!("{w}: unknown sequence")))?;
        require(
            u.track > 0 && u.lane.is_none_or(|n| n > 0) && u.selected.is_none_or(|n| n >= 0),
            &w,
            "invalid multicam use lane/index",
        )?;
        let mut refs = HashSet::new();
        for id in &u.candidates {
            require(
                groups.contains_key(id.as_str()) && refs.insert(id),
                &w,
                "invalid or duplicate multicam candidate",
            )?;
        }
        let mut refs = HashSet::new();
        for id in &u.binding.placements {
            require(
                placements.get(id.as_str()) == Some(&sequence.id.as_str()) && refs.insert(id),
                &w,
                "invalid multicam occurrence placement binding",
            )?;
        }
        require(
            u.record_start.is_none_or(|n| !n.is_negative())
                && u.source_in.is_none_or(|n| !n.is_negative())
                && u.duration.is_none_or(|n| n.num() > 0),
            &w,
            "invalid multicam occurrence span",
        )?;
        for start in [u.record_start, u.source_in].into_iter().flatten() {
            if let Some(d) = u.duration {
                require(
                    start.checked_add(d).is_some(),
                    &w,
                    "multicam occurrence end overflows",
                )?;
            }
        }
        if u.reason.is_none() {
            require(
                u.candidates.len() == 1
                    && u.lane.is_some()
                    && u.selected.is_some()
                    && u.record_start.is_some()
                    && u.source_in.is_some()
                    && u.duration.is_some(),
                &w,
                "unresolved multicam occurrence requires a reason",
            )?;
            let m = groups[u.candidates[0].as_str()];
            require(
                m.rate == Some(sequence.rate),
                &w,
                "multicam occurrence rate differs from sequence",
            )?;
            let lane = m
                .lanes
                .iter()
                .find(|l| l.kind == u.kind && Some(l.index) == u.lane)
                .ok_or_else(|| GraphError(format!("{w}: unknown source lane")))?;
            require(
                (u.selected.unwrap() as usize) < lane.alternatives.len(),
                &w,
                "multicam use selected alternative outside source lane",
            )?;
            require(
                m.duration.is_some_and(|d| {
                    u.source_in
                        .unwrap()
                        .checked_add(u.duration.unwrap())
                        .is_some_and(|end| end <= d)
                }),
                &w,
                "multicam occurrence exceeds source extent",
            )?;
        }
    }
    Ok(())
}
