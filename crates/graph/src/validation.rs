//! Format-independent validation shared by typed and JSON graph/1 callers.
//! Destination clocks, frame grids, codec/profile limits and layout caps belong
//! to their adapters. In particular audio edits may have subframe positions.
use super::*;

pub(super) fn require(ok: bool, w: &str, reason: &str) -> R<()> {
    if ok {
        Ok(())
    } else {
        fail(format!("{w}: {reason}"))
    }
}
pub(super) fn list(n: usize, w: &str) -> R<()> {
    require(n <= MAX_LIST, w, "list exceeds graph limit")
}
pub(super) fn text(s: &str, w: &str) -> R<()> {
    require(s.len() <= 32768, w, "text exceeds graph limit")
}
pub(super) fn finite(n: f64, w: &str) -> R<()> {
    require(n.is_finite(), w, "number is NaN or infinite")
}
fn pan(n: f64, w: &str) -> R<()> {
    finite(n, w)?;
    require((-100.0..=100.0).contains(&n), w, "pan is outside -100..100")
}
pub(super) fn rate(n: Rational, w: &str) -> R<()> {
    require(n.num() > 0, w, "rate must be positive")
}
pub(super) fn positive_labels(values: &[i64], w: &str) -> R<()> {
    require(
        values.iter().all(|&n| n > 0),
        w,
        "track/stream labels must be positive",
    )
}
pub(super) fn distinct(values: &[i64], w: &str) -> R<()> {
    let mut seen = HashSet::new();
    require(
        values.iter().all(|n| seen.insert(n)),
        w,
        "duplicate track/stream label",
    )
}

// The text JSON parser rejects duplicates/nonfinite numbers and caps nesting.
// A public Value or typed graph must not bypass those safeguards. The depth is
// counted from the complete graph root, so serialized graphs remain readable.
pub(super) fn value(v: &Value, root_depth: usize, w: &str) -> R<()> {
    let mut pending = vec![(v, root_depth)];
    while let Some((v, depth)) = pending.pop() {
        require(depth <= 64, w, "JSON nested too deeply")?;
        match v {
            Value::Float(n) => finite(*n, w)?,
            Value::Arr(a) => {
                list(a.len(), w)?;
                pending.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::Obj(a) => {
                list(a.len(), w)?;
                let mut keys = HashSet::new();
                require(
                    a.iter().all(|(k, _)| keys.insert(k)),
                    w,
                    "repeated JSON key",
                )?;
                pending.extend(a.iter().map(|(_, v)| (v, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn marker(m: &Marker, w: &str) -> R<()> {
    for s in [&m.name, &m.comment, &m.user] {
        text(s, w)?;
    }
    if let Some(c) = &m.color {
        text(&c.palette, w)?;
        text(&c.name, w)?;
    }
    if let Some(t) = m.track {
        positive_labels(&[t.index], w)?;
    }
    require(
        m.source.is_none_or(|t| !t.is_negative()),
        w,
        "negative marker source time",
    )
}

pub(super) fn check(g: &Graph) -> R<()> {
    text(&g.project.name, "project name")?;
    for (n, label) in [
        (g.media.len(), "media"),
        (g.items.len(), "items"),
        (g.bins.len(), "bins"),
        (g.sequences.len(), "sequences"),
        (g.losses.len(), "losses"),
        (g.sidecars.len(), "sidecars"),
    ] {
        list(n, label)?;
    }
    let media: HashMap<_, _> = g.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let ranges: HashMap<_, _> = g
        .media
        .iter()
        .map(|m| (m.id.as_str(), MediaFacts::from(m)))
        .collect();
    for m in &g.media {
        let w = format!("media {:?}", m.id);
        text(&m.id, &w)?;
        list(m.locations.len(), &w)?;
        for p in &m.locations {
            text(p, &w)?;
        }
        if let Some(c) = &m.codec {
            text(c, &w)?;
        }
        if let Some(r) = m.rate {
            rate(r, &w)?;
        }
        require(
            m.frame_size.is_none_or(|(x, y)| x > 0 && y > 0),
            &w,
            "raster must be positive",
        )?;
        require(
            m.sample_rate.is_none_or(|n| n > 0),
            &w,
            "sample rate must be positive",
        )?;
        require(
            m.bits.is_none_or(|n| n > 0),
            &w,
            "bit depth must be positive",
        )?;
        require(
            m.stream.is_none_or(|n| n > 0),
            &w,
            "stream label must be positive",
        )?;
    }
    for i in &g.items {
        let w = format!("item {:?}", i.id);
        for s in [&i.id, &i.name] {
            text(s, &w)?;
        }
        if let Some(s) = &i.tape {
            text(s, &w)?;
        }
        require(
            i.umid.as_ref().is_none_or(|u| {
                u.len() == 64
                    && u.bytes()
                        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
            }),
            &w,
            "expected 64 upper-case hex digits",
        )?;
        require(
            i.color_rgb16
                .is_none_or(|c| c.iter().all(|n| (0..=65535).contains(n))),
            &w,
            "color is outside 0..65535",
        )?;
        if let Some(r) = i.rate {
            rate(r, &w)?;
        }
        list(i.media.len(), &w)?;
        list(i.stream_groups.len(), &w)?;
        list(i.markers.len(), &w)?;
        let mut refs = HashSet::new();
        let mut audio_labels = HashSet::new();
        for im in &i.media {
            finite(im.gain_db, &w)?;
            require(refs.insert(&im.media), &w, "duplicate media reference")?;
            let m = media[im.media.as_str()];
            if m.kind == StreamKind::Audio {
                if let Some(label) = m.stream {
                    require(
                        audio_labels.insert(label),
                        &w,
                        "ambiguous audio stream labels",
                    )?;
                }
            }
        }
        let mut groups = HashSet::new();
        for group in &i.stream_groups {
            list(group.len(), &w)?;
            require(!group.is_empty(), &w, "empty stream group")?;
            distinct(group, &w)?;
            require(groups.insert(group), &w, "duplicate stream group")?;
        }
        for m in &i.markers {
            marker(m, &w)?;
        }
    }
    let routes: HashMap<_, _> = g
        .items
        .iter()
        .map(|item| {
            Ok((
                item.id.as_str(),
                ItemRoutes::new(
                    item.media
                        .iter()
                        .map(|im| (im.media.as_str(), Some(Rational::ZERO))),
                    &ranges,
                )?,
            ))
        })
        .collect::<R<_>>()?;
    for b in &g.bins {
        for s in [&b.id, &b.name] {
            text(s, "bin")?;
        }
        list(b.items.len(), "bin items")?;
    }
    for s in &g.sequences {
        let w = format!("sequence {:?}", s.id);
        for name in [&s.id, &s.name] {
            text(name, &w)?;
        }
        rate(s.rate, &w)?;
        require(
            s.frame_size.0 > 0 && s.frame_size.1 > 0,
            &w,
            "raster must be positive",
        )?;
        require(!s.start.is_negative(), &w, "negative sequence start")?;
        for n in [
            s.tracks.len(),
            s.placements.len(),
            s.transitions.len(),
            s.markers.len(),
        ] {
            list(n, &w)?;
        }
        let mut tracks = HashSet::new();
        for t in &s.tracks {
            positive_labels(&[t.index], &w)?;
            require(
                tracks.insert((t.kind, t.index)),
                &w,
                "duplicate declared track",
            )?;
            if let Some(n) = t.level_db {
                finite(n, &w)?;
            }
        }
        for p in &s.placements {
            let pw = format!("{w} placement {:?}", p.id);
            text(&p.id, &pw)?;
            placement(p, &routes[p.item.as_str()], s.rate, &pw)?;
        }
        for t in &s.transitions {
            positive_labels(&[t.track], &w)?;
            text(&t.style, &w)?;
            require(
                !t.cut.is_negative()
                    && t.duration.num() > 0
                    && !t.cut_offset.is_negative()
                    && t.cut_offset <= t.duration,
                &w,
                "invalid transition timing",
            )?;
            require(
                t.cut
                    .checked_sub(t.cut_offset)
                    .is_some_and(|n| !n.is_negative()),
                &w,
                "transition starts before sequence zero",
            )?;
            let after = t
                .duration
                .checked_sub(t.cut_offset)
                .ok_or_else(|| GraphError(format!("{w}: transition time overflow")))?;
            require(
                t.cut.checked_add(after).is_some(),
                &w,
                "transition time overflow",
            )?;
        }
        for m in &s.markers {
            marker(m, &w)?;
        }
    }
    for l in &g.losses {
        for s in [&l.object, &l.aspect, &l.reason] {
            text(s, "loss")?;
        }
    }
    let mut sidecars = HashSet::new();
    for s in &g.sidecars {
        text(&s.adapter, "sidecar")?;
        text(&s.object, "sidecar")?;
        require(
            sidecars.insert((&s.adapter, &s.object)),
            "sidecar",
            "duplicate adapter/object sidecar",
        )?;
        value(&s.data, 3, "sidecar data")?;
    }
    Ok(())
}

/// Only range/routing facts are shared. Unknown source/2 timecodes never pass
/// through a manufactured legacy Media with a zero start.
pub(super) struct MediaFacts {
    pub kind: StreamKind,
    pub duration: Rational,
    pub rate: Option<Rational>,
    pub stream: Option<i64>,
}
impl From<&Media> for MediaFacts {
    fn from(m: &Media) -> Self {
        Self {
            kind: m.kind,
            duration: m.duration,
            rate: m.rate,
            stream: m.stream,
        }
    }
}

#[derive(Clone, Copy)]
struct SelectedMedia<'a> {
    facts: &'a MediaFacts,
    offset: Option<Rational>,
}

/// Borrowed, per-item indices are built once. Per-placement validation touches
/// only its selected streams, not every stream in a large shared item.
pub(super) struct ItemRoutes<'a> {
    video: Vec<SelectedMedia<'a>>,
    audio: Vec<SelectedMedia<'a>>,
    by_label: HashMap<i64, SelectedMedia<'a>>,
}
impl<'a> ItemRoutes<'a> {
    pub(super) fn new(
        media_refs: impl Iterator<Item = (&'a str, Option<Rational>)>,
        media: &'a HashMap<&str, MediaFacts>,
    ) -> R<Self> {
        let mut result = Self {
            video: Vec::new(),
            audio: Vec::new(),
            by_label: HashMap::new(),
        };
        for (id, offset) in media_refs {
            let facts = media
                .get(id)
                .ok_or_else(|| GraphError(format!("item routing: unknown media {id:?}")))?;
            let selected = SelectedMedia { facts, offset };
            match facts.kind {
                StreamKind::Video => result.video.push(selected),
                StreamKind::Audio => {
                    result.audio.push(selected);
                    if let Some(label) = facts.stream {
                        require(
                            result.by_label.insert(label, selected).is_none(),
                            "item routing",
                            "ambiguous audio stream labels",
                        )?;
                    }
                }
            }
        }
        Ok(result)
    }
}

pub(super) fn placement(p: &Placement, routes: &ItemRoutes, seq_rate: Rational, w: &str) -> R<()> {
    require(
        !p.record_start.is_negative() && !p.source_in.is_negative() && p.duration.num() > 0,
        w,
        "times out of range",
    )?;
    require(
        p.video_track.is_some() || !p.audio_tracks.is_empty() || !p.grouped_audio.is_empty(),
        w,
        "needs a picture or audio track",
    )?;
    for n in [
        p.audio_tracks.len(),
        p.audio_streams.len(),
        p.grouped_audio.len(),
        p.volume_keys.len(),
        p.track_gains_db.len(),
        p.track_pans.len(),
        p.effects.len(),
    ] {
        list(n, w)?;
    }
    finite(p.gain_db, w)?;
    for n in p.volume_db.iter().chain(p.track_gains_db.iter().flatten()) {
        finite(*n, w)?;
    }
    for n in p.track_pans.iter().flatten() {
        pan(*n, w)?;
    }
    if let Some(t) = &p.transform {
        for n in [
            t.position.0,
            t.position.1,
            t.scale.0,
            t.scale.1,
            t.rotation,
            t.anchor.0,
            t.anchor.1,
        ]
        .into_iter()
        .chain(t.crop)
        {
            finite(n, w)?;
        }
    }
    for key in &p.volume_keys {
        finite(key.db, w)?;
        require(!key.at.is_negative(), w, "negative volume-key source time")?;
    }
    if let Some(sp) = &p.speed {
        require(
            sp.ratio.0 != 0 && sp.ratio.1 > 0 && sp.source_duration.num() > 0,
            w,
            "invalid speed/source duration",
        )?;
    }
    require(
        p.record_start.checked_add(p.duration).is_some(),
        w,
        "record range overflow",
    )?;
    positive_labels(&p.audio_tracks, w)?;
    distinct(&p.audio_tracks, w)?;
    require(
        p.video_track.is_none_or(|n| n > 0),
        w,
        "video track must be positive",
    )?;
    for n in [
        p.track_gains_db.len(),
        p.track_pans.len(),
        p.audio_streams.len(),
    ] {
        require(
            n == 0 || n == p.audio_tracks.len(),
            w,
            "audio routing/gain/pan cardinality mismatch",
        )?;
    }
    let audio = &routes.audio;
    let video = &routes.video;
    let mut selected = Vec::new();
    if p.video_track.is_some() {
        require(
            video.len() == 1,
            w,
            "picture placement needs one unambiguous picture stream",
        )?;
        if p.frame_map.is_none() {
            selected.push(video[0]);
        }
    }
    if p.audio_streams.is_empty() {
        require(
            p.audio_tracks.len() <= audio.len(),
            w,
            "more audio tracks than source streams",
        )?;
        selected.extend(audio.iter().take(p.audio_tracks.len()).copied());
    } else {
        for label in &p.audio_streams {
            let m = routes.by_label.get(label).ok_or_else(|| {
                GraphError(format!("{w}: audio routing names a stream the item lacks"))
            })?;
            selected.push(*m);
        }
    }
    let mut destinations: HashSet<_> = p.audio_tracks.iter().copied().collect();
    for group in &p.grouped_audio {
        positive_labels(&[group.track], w)?;
        require(
            destinations.insert(group.track),
            w,
            "mono/grouped audio routes share a destination track",
        )?;
        list(group.streams.len(), w)?;
        list(group.pans.len(), w)?;
        require(!group.streams.is_empty(), w, "empty grouped-audio route")?;
        positive_labels(&group.streams, w)?;
        distinct(&group.streams, w)?;
        require(
            group.pans.is_empty() || group.pans.len() == group.streams.len(),
            w,
            "grouped-audio pan cardinality mismatch",
        )?;
        if let Some(n) = group.gain_db {
            finite(n, w)?;
        }
        for n in group.pans.iter().flatten() {
            pan(*n, w)?;
        }
        for label in &group.streams {
            let m = routes.by_label.get(label).ok_or_else(|| {
                GraphError(format!("{w}: grouped audio names a stream the item lacks"))
            })?;
            selected.push(*m);
        }
    }
    let span = p.speed.as_ref().map_or(p.duration, |s| s.source_duration);
    if !selected.is_empty() {
        let end = p
            .source_in
            .checked_add(span)
            .ok_or_else(|| GraphError(format!("{w}: source range overflow")))?;
        for selected in selected {
            if let Some(offset) = selected.offset {
                let start = offset
                    .checked_add(p.source_in)
                    .ok_or_else(|| GraphError(format!("{w}: source offset overflow")))?;
                let end = offset
                    .checked_add(end)
                    .ok_or_else(|| GraphError(format!("{w}: source range overflow")))?;
                require(
                    !start.is_negative() && end <= selected.facts.duration,
                    w,
                    "source range exceeds selected media",
                )?;
            }
        }
    }
    if let Some(map) = &p.frame_map {
        list(map.len(), w)?;
        require(
            p.video_track.is_some() && video.len() == 1,
            w,
            "frame map requires selected picture",
        )?;
        let picture_rate = video[0]
            .facts
            .rate
            .ok_or_else(|| GraphError(format!("{w}: frame map needs source picture rate")))?;
        // Cross multiplication is bounded by three i64 magnitudes. Cancel before
        // multiplication through exact rational conversions instead of rounding.
        let frames = rational_product(p.duration, seq_rate).and_then(|n| n.to_units(1));
        require(
            frames.is_some_and(|n| n > 0 && usize::try_from(n).ok() == Some(map.len())),
            w,
            "frame-map cardinality does not match record frames",
        )?;
        for &frame in map {
            require(frame >= 0, w, "negative absolute source frame")?;
            let end = Rational::new(
                (i128::from(frame) + 1) * i128::from(picture_rate.den()),
                i128::from(picture_rate.num()),
            )
            .ok_or_else(|| GraphError(format!("{w}: frame-map source time overflow")))?;
            require(
                end <= video[0].facts.duration,
                w,
                "frame map exceeds selected picture",
            )?;
        }
    }
    for effect in &p.effects {
        text(&effect.kind, w)?;
        require(
            effect.kind.contains('.'),
            w,
            "effect kinds are <program>.<effect>",
        )?;
        value(&effect.params, 7, w)?;
    }
    Ok(())
}

fn rational_product(a: Rational, b: Rational) -> Option<Rational> {
    Rational::new(
        i128::from(a.num()) * i128::from(b.num()),
        i128::from(a.den()) * i128::from(b.den()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn item_indices_are_built_once_and_reused_for_many_selected_streams() {
        let ids: Vec<_> = (1..=8192).map(|n| format!("audio:{n}")).collect();
        let facts: HashMap<_, _> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    id.as_str(),
                    MediaFacts {
                        kind: StreamKind::Audio,
                        duration: Rational::integer(2),
                        rate: None,
                        stream: Some(i as i64 + 1),
                    },
                )
            })
            .collect();
        let visits = std::cell::Cell::new(0);
        let routes = ItemRoutes::new(
            ids.iter().map(|id| {
                visits.set(visits.get() + 1);
                (id.as_str(), Some(Rational::ZERO))
            }),
            &facts,
        )
        .unwrap();
        assert_eq!(visits.get(), 8192);
        assert_eq!(routes.by_label.len(), 8192);
        let capacities = (routes.audio.capacity(), routes.by_label.capacity());
        let mut p = Placement {
            id: "p".into(),
            item: "i".into(),
            record_start: Rational::ZERO,
            source_in: Rational::ZERO,
            duration: Rational::integer(1),
            video_track: None,
            audio_tracks: vec![9],
            audio_streams: vec![1],
            grouped_audio: vec![],
            gain_db: 0.0,
            volume_db: None,
            volume_keys: vec![],
            track_gains_db: vec![],
            track_pans: vec![],
            speed: None,
            frame_map: None,
            transform: None,
            effects: vec![],
        };
        for label in 1..=8192 {
            p.audio_streams[0] = label;
            placement(&p, &routes, Rational::new(1, 7).unwrap(), "selected route").unwrap();
        }
        assert_eq!(visits.get(), 8192);
        assert_eq!(
            (routes.audio.capacity(), routes.by_label.capacity()),
            capacities
        );
    }
}
