// Source identities, timing, routing and occurrence validation.
use super::*;

pub(super) fn graph(graph: &SourceGraph) -> R<()> {
    use crate::validation::{
        distinct, finite, list, positive_labels, rate, require, text, value, ItemRoutes,
        MediaFacts,
    };
    text(&graph.project.name, "source project")?;
    for n in [
        graph.media.len(),
        graph.items.len(),
        graph.bins.len(),
        graph.sequences.len(),
        graph.losses.len(),
        graph.sidecars.len(),
        graph.multicams.len(),
        graph.multicam_uses.len(),
    ] {
        list(n, "source graph")?;
    }
    let mut ids = HashMap::new();
    for m in &graph.media {
        add_id(&mut ids, &m.id, "media")?;
    }
    for i in &graph.items {
        add_id(&mut ids, &i.id, "item")?;
    }
    for b in &graph.bins {
        add_id(&mut ids, &b.id, "bin")?;
    }
    for s in &graph.sequences {
        add_id(&mut ids, &s.id, "sequence")?;
        for p in s.placements.iter().chain(&s.disabled_placements) {
            add_id(&mut ids, &p.id, "placement")?;
        }
        for t in &s.transitions {
            add_id(&mut ids, &t.id, "transition")?;
        }
        for t in &s.texts { add_id(&mut ids, &t.id, "text")?; }
    }
    let media: HashMap<_, _> = graph.media.iter().map(|m| (m.id.as_str(), m)).collect();
    for m in &graph.multicams {
        add_id(&mut ids, &m.id, "multicam")?;
        for lane in &m.lanes { for a in &lane.alternatives { add_id(&mut ids, &a.id, "multicam alternative")?; } }
    }
    for u in &graph.multicam_uses { add_id(&mut ids, &u.id, "multicam use")?; }
    multicam::validate(graph, &media)?;
    let ranges: HashMap<_, _> = graph
        .media
        .iter()
        .map(|m| {
            (
                m.id.as_str(),
                MediaFacts {
                    kind: m.kind,
                    duration: m.duration,
                    rate: m.rate,
                    stream: m.stream,
                },
            )
        })
        .collect();
    for m in &graph.media {
        let w = format!("source media {:?}", m.id);
        if let Some(clock) = &m.clock {
            validate_clock(clock, m.start, &w)?;
        }
        list(m.locations.len(), &w)?;
        for path in &m.locations {
            text(path, &w)?;
        }
        if let Some(codec) = &m.codec {
            text(codec, &w)?;
        }
        require(
            !m.duration.is_negative() && m.start.is_none_or(|t| !t.is_negative()),
            &w,
            "negative media time",
        )?;
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
        if let Some(o) = &m.original {
            list(o.saved_locators.len(), &w)?;
            require(!o.saved_locators.is_empty(), &w, "original stream needs saved locators")?;
            for p in &o.saved_locators { text(p, &w)?; }
            text(&o.container, &w)?;
            rate(o.time_base, &w)?;
            require(o.stream_index >= 0 && o.start_pts == 0 && o.duration_ts > 0, &w, "unsupported original stream origin or extent")?;
            if matches!(o.container.as_str(), "jpeg" | "png" | "tiff") {
                let still_raster = match o.container.as_str() {
                    "tiff" => m.frame_size == Some((1920, 64)),
                    "png" => matches!(m.frame_size, Some((1920, 1080) | (1536, 1024))),
                    _ => m.frame_size == Some((1920, 1080)),
                };
                require(m.kind == StreamKind::Video && m.codec.as_deref() == Some(o.container.as_str())
                    && m.online == Some(true) && o.stream_index == 0 && o.duration_ts == 1
                    && o.time_base == Rational::new(1, 24).unwrap() && m.rate == Some(Rational::integer(24))
                    && still_raster
                    && m.duration.to_units(24).is_some_and(|n| n > 0),
                    &w, "invalid original still or editorial hold")?;
            } else {
                require(Rational::new(o.duration_ts as i128 * o.time_base.num() as i128, o.time_base.den() as i128) == Some(m.duration), &w, "original physical duration contradicts stream clock")?;
            }
            match m.kind {
                StreamKind::Video => require(o.channels.is_none() && o.channel_index.is_none() && o.sample_representation.is_none(), &w, "picture cannot have audio channel facts")?,
                StreamKind::Audio => {
                    require(matches!((o.channels,o.channel_index),(Some(n),Some(c))
                        if (0..n).contains(&c)
                            && ((1..=2).contains(&n) || n == 8 && o.container == "wav"
                                && m.sample_rate == Some(48000)
                                && o.sample_representation.as_deref() == Some("pcm16"))),
                        &w, "invalid physical audio channel")?;
                    require(match o.sample_representation.as_deref() {
                        Some("aac") => m.bits.is_none(),
                        Some("mp3") => m.bits.is_none()
                            && o.container == "mp3"
                            && m.codec.as_deref() == Some("mp3")
                            && m.online == Some(true)
                            && o.stream_index == 0
                            && m.sample_rate.is_some_and(|rate| {
                                matches!(rate, 44_100 | 48_000)
                                    && Some(o.time_base) == Rational::new(1, rate.into())
                            }),
                        Some("pcm16") => m.bits == Some(16),
                        Some("pcm16be") => m.bits == Some(16) && o.container == "mov"
                            && o.stream_index == 1 && o.channels == Some(2)
                            && m.sample_rate == Some(48_000)
                            && o.time_base == Rational::new(1, 48_000).unwrap(),
                        Some("pcm24") => m.bits == Some(24),
                        _ => false,
                    }, &w, "audio sample representation contradicts bit depth or media profile")?;
                }
            }
        }
        if let Some(r) = m.rate {
            rate(r, &w)?;
        }
    }
    for i in &graph.items {
        let w = format!("source item {:?}", i.id);
        text(&i.name, &w)?;
        if let Some(t) = &i.tape {
            text(t, &w)?;
        }
        if let Some(r) = i.rate {
            rate(r, &w)?;
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
        list(i.media.len(), &w)?;
        list(i.stream_groups.len(), &w)?;
        list(i.markers.len(), &w)?;
        require(!i.media.is_empty(), &w, "needs at least one media stream")?;
        let mut refs = HashSet::new();
        let mut labels = HashSet::new();
        for im in &i.media {
            finite(im.gain_db, &w)?;
            require(refs.insert(&im.media), &w, "duplicate media reference")?;
            let m = media
                .get(im.media.as_str())
                .ok_or_else(|| GraphError(format!("{w}: unknown media {:?}", im.media)))?;
            if m.kind == StreamKind::Audio {
                if let Some(label) = m.stream {
                    require(labels.insert(label), &w, "ambiguous audio stream labels")?;
                }
            }
        }
        let mut groups = HashSet::new();
        for group in &i.stream_groups {
            list(group.len(), &w)?;
            distinct(group, &w)?;
            require(
                !group.is_empty() && group.iter().all(|n| labels.contains(n)),
                &w,
                "invalid stream group",
            )?;
            require(groups.insert(group), &w, "duplicate stream group")?;
        }
        for m in &i.markers {
            check_marker(m, &HashMap::new(), &w)?;
        }
    }
    let routes: HashMap<_, _> = graph
        .items
        .iter()
        .map(|item| {
            Ok((
                item.id.as_str(),
                ItemRoutes::new(
                    item.media
                        .iter()
                        .map(|im| (im.media.as_str(), im.source_offset)),
                    &ranges,
                )?,
            ))
        })
        .collect::<R<_>>()?;
    let mut bins = HashSet::new();
    for b in &graph.bins {
        text(&b.name, "source bin")?;
        list(b.items.len(), "source bin")?;
        require(
            b.parent.as_ref().is_none_or(|p| bins.contains(p.as_str())),
            &b.id,
            "parent must be an earlier bin",
        )?;
        bins.insert(b.id.as_str());
        require(
            b.items
                .iter()
                .all(|id| matches!(ids.get(id.as_str()), Some(&"item" | &"multicam"))),
            &b.id,
            "unknown item",
        )?;
    }
    for s in &graph.sequences {
        let w = format!("source sequence {:?}", s.id);
        if let Some(clock) = &s.clock {
            validate_clock(clock, s.start, &w)?;
        }
        text(&s.name, &w)?;
        rate(s.rate, &w)?;
        require(
            s.frame_size.is_none_or(|(x, y)| x > 0 && y > 0),
            &w,
            "raster must be positive",
        )?;
        require(
            s.start.is_none_or(|t| !t.is_negative()),
            &w,
            "negative sequence start",
        )?;
        for n in [
            s.tracks.len(),
            s.placements.len(),
            s.disabled_placements.len(),
            s.transitions.len(),
            s.markers.len(),
            s.texts.len(),
        ] {
            list(n, &w)?;
        }
        let mut text_ids = HashSet::new();
        for t in &s.texts {
            text(&t.id, &w)?;
            require(!t.id.is_empty() && text_ids.insert(&t.id), &w, "duplicate or empty text occurrence id")?;
            positive_labels(&[t.track], &w)?;
            if let Some(c) = &t.content {
                text(c, &w)?;
                require(!c.chars().any(|c| c.is_control() && c != '\n' && c != '\t'), &w, "source text must use LF paragraphs and printable characters")?;
            }
            if let Some(reason) = &t.reason { text(reason, &w)?; require(!reason.is_empty(), &w, "empty text uncertainty")?; }
            require(t.record_start.is_none_or(|n| !n.is_negative()), &w, "negative text start")?;
            require(t.duration.is_none_or(|n| n.num() > 0), &w, "nonpositive text duration")?;
            if let (Some(start), Some(duration)) = (t.record_start, t.duration) {
                require(start.checked_add(duration).is_some(), &w, "text end overflow")?;
            }
            require(t.reason.is_some() || (t.content.is_some() && t.record_start.is_some() && t.duration.is_some()), &w, "unresolved text needs a reason")?;
        }
        let mut tracks = HashSet::new();
        let mut empty_tracks = HashSet::new();
        for t in &s.tracks {
            positive_labels(&[t.index], &w)?;
            require(
                tracks.insert((t.kind, t.index)),
                &w,
                "duplicate declared track",
            )?;
            if let Some(name) = &t.name {
                text(name, &w)?;
                require(!name.is_empty() && !name.chars().any(char::is_control),
                    &w, "invalid source track name")?;
            }
            if t.empty {
                empty_tracks.insert((t.kind, t.index));
            }
            if let Some(n) = t.level_db {
                finite(n, &w)?;
            }
        }
        let placements: HashMap<_, _> =
            s.placements.iter().map(|p| (p.id.as_str(), p)).collect();
        for p in s.placements.iter().chain(&s.disabled_placements) {
            require(
                usize::from(p.video_track.is_some())
                    + p.audio_tracks.len()
                    + p.grouped_audio.len()
                    == 1,
                &w,
                "source placement must occupy exactly one lane",
            )?;
            let lane = p.video_track.map(|index| (StreamKind::Video, index))
                .or_else(|| p.audio_tracks.first().map(|&index| (StreamKind::Audio, index)))
                .or_else(|| p.grouped_audio.first().map(|group| (StreamKind::Audio, group.track)));
            require(lane.is_none_or(|lane| !empty_tracks.contains(&lane)),
                &w, "placement contradicts an empty source track")?;
            let routing = routes
                .get(p.item.as_str())
                .ok_or_else(|| GraphError(format!("{w}: unknown item {:?}", p.item)))?;
            crate::validation::placement(
                p,
                routing,
                s.rate,
                &format!("{w} placement {:?}", p.id),
            )?;
            let mut has_linear_pan = false;
            for effect in &p.effects {
                if effect.kind == crate::audio_pan::LINEAR_KIND {
                    require(
                        !has_linear_pan && p.audio_tracks.len() == 1
                            && crate::audio_pan::read_linear(effect, p.duration).is_some(),
                        &w,
                        "Linear pan needs one mono audio placement and ordered edit-local keys within its span",
                    )?;
                    has_linear_pan = true;
                }
                if effect.kind == crate::picture_chain::REFLECTION_KIND {
                    require(
                        p.video_track.is_some()
                            && crate::picture_chain::Reflection::from_params(&effect.params).is_some(),
                        &w,
                        "reflection needs a picture placement and two boolean axes with at least one enabled",
                    )?;
                }
                if effect.kind == crate::picture_motion::LINEAR_KIND {
                    require(
                        p.video_track.is_some()
                            && p.transform.is_none()
                            && p.effects.len() == 1
                            && crate::picture_motion::read_linear(effect).is_some(),
                        &w,
                        "Linear Motion needs a standalone picture effect with two ordered item-source keys",
                    )?;
                }
            }
        }
        for t in &s.transitions {
            let tw = format!("{w} transition {:?}", t.id);
            positive_labels(&[t.track], &tw)?;
            text(&t.style, &tw)?;
            require(
                !t.cut.is_negative()
                    && t.duration.num() > 0
                    && !t.cut_offset.is_negative()
                    && t.cut_offset <= t.duration,
                &tw,
                "invalid transition timing",
            )?;
            let start = t
                .cut
                .checked_sub(t.cut_offset)
                .ok_or_else(|| GraphError(format!("{tw}: transition time overflow")))?;
            let after = t
                .duration
                .checked_sub(t.cut_offset)
                .ok_or_else(|| GraphError(format!("{tw}: transition time overflow")))?;
            require(
                !start.is_negative() && t.cut.checked_add(after).is_some(),
                &tw,
                "transition range out of bounds",
            )?;
            for endpoint in [&t.outgoing, &t.incoming] {
                check_binding(endpoint, &placements, true, &tw)?;
                if endpoint.reason.is_none() {
                    for id in &endpoint.placements {
                        let p = placements[id.as_str()];
                        let on_lane = match t.kind {
                            StreamKind::Video => p.video_track == Some(t.track),
                            StreamKind::Audio => {
                                p.audio_tracks.contains(&t.track)
                                    || p.grouped_audio.iter().any(|g| g.track == t.track)
                            }
                        };
                        require(on_lane, &tw, "resolved endpoint is on a different track")?;
                    }
                }
            }
            if t.outgoing.reason.is_none() && t.incoming.reason.is_none() {
                require(
                    !t.outgoing
                        .placements
                        .iter()
                        .any(|id| t.incoming.placements.contains(id)),
                    &tw,
                    "transition endpoints overlap",
                )?;
                if t.outgoing.placements.len() == 1 && t.incoming.placements.len() == 1 {
                    let end = t
                        .cut
                        .checked_add(after)
                        .ok_or_else(|| GraphError(format!("{tw}: transition time overflow")))?;
                    for id in [&t.outgoing.placements[0], &t.incoming.placements[0]] {
                        let p = placements[id.as_str()];
                        let placement_end =
                            p.record_start.checked_add(p.duration).ok_or_else(|| {
                                GraphError(format!("{tw}: endpoint record range overflow"))
                            })?;
                        require(
                            p.record_start <= start && placement_end >= end,
                            &tw,
                            "resolved endpoint does not cover the transition interval",
                        )?;
                    }
                }
            }
        }
        for m in &s.markers {
            check_marker(m, &placements, &w)?;
        }
    }
    for l in &graph.losses {
        require(
            ids.contains_key(l.object.as_str()) && !l.aspect.is_empty(),
            "source loss",
            "unknown object or empty aspect",
        )?;
        for text_value in [&l.object, &l.aspect, &l.reason] {
            text(text_value, "source loss")?;
        }
    }
    let mut sidecars = HashSet::new();
    for s in &graph.sidecars {
        require(
            !s.adapter.is_empty() && ids.contains_key(s.object.as_str()),
            "source sidecar",
            "unknown object or empty adapter",
        )?;
        text(&s.adapter, "source sidecar")?;
        text(&s.object, "source sidecar")?;
        require(
            sidecars.insert((&s.adapter, &s.object)),
            "source sidecar",
            "duplicate adapter/object sidecar",
        )?;
        value(&s.data, 3, "source sidecar data")?;
    }
    Ok(())
}

fn validate_clock(clock: &SourceClock, start: Option<Rational>, w: &str) -> R<()> {
    use crate::validation::{rate, require, text};
    rate(clock.rate, w)?;
    rate(clock.provenance.source_rate, w)?;
    require(
        clock.display_base > 0,
        w,
        "clock display base must be positive",
    )?;
    for value in [
        &clock.provenance.source_package,
        &clock.provenance.timecode_track,
        &clock.provenance.route,
    ] {
        text(value, w)?;
        require(!value.is_empty(), w, "clock provenance must be nonempty")?;
    }
    // Cancel rate factors before multiplication, so representable exact offsets do not
    // fail merely because the unreduced product exceeds i128.
    let mut n = [
        clock.provenance.source_offset.unsigned_abs() as u128,
        clock.rate.num() as u128,
        clock.provenance.source_rate.den() as u128,
    ];
    let mut d = [
        clock.rate.den() as u128,
        clock.provenance.source_rate.num() as u128,
    ];
    for n in &mut n {
        for d in &mut d {
            let (mut a, mut b) = (*n, *d);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            *n /= a;
            *d /= a;
        }
    }
    require(d == [1, 1], w, "clock offset is not a whole timecode count")?;
    let converted = n
        .into_iter()
        .try_fold(1u128, |a, b| a.checked_mul(b))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| GraphError(format!("{w}: clock offset overflows count")))?;
    let effective = if clock.provenance.source_offset < 0 {
        clock.provenance.origin_count.checked_sub(converted)
    } else {
        clock.provenance.origin_count.checked_add(converted)
    };
    require(
        effective == Some(clock.effective_count),
        w,
        "clock effective count contradicts origin and source offset",
    )?;
    let exact = clock
        .exact_start()
        .ok_or_else(|| GraphError(format!("{w}: clock start is not representable")))?;
    require(
        start == Some(exact),
        w,
        "source start contradicts qualified clock",
    )
}

fn add_id<'a>(ids: &mut HashMap<&'a str, &'static str>, id: &'a str, kind: &'static str) -> R<()> {
    crate::validation::text(id, "source id")?;
    crate::validation::require(!id.is_empty(), "source graph", "empty id")?;
    crate::validation::require(ids.insert(id, kind).is_none(), id, "duplicate id")
}

fn check_binding(
    binding: &SourceBinding,
    placements: &HashMap<&str, &Placement>,
    require_endpoint: bool,
    w: &str,
) -> R<()> {
    use crate::validation::{list, require, text};
    list(binding.placements.len(), w)?;
    if let Some(reason) = &binding.reason {
        text(reason, w)?;
        require(!reason.is_empty(), w, "unresolved binding needs a reason")?;
    }
    let mut seen = HashSet::new();
    for id in &binding.placements {
        require(seen.insert(id), w, "duplicate binding placement")?;
        require(
            placements.contains_key(id.as_str()),
            w,
            "binding names an unknown placement",
        )?;
    }
    require(
        !require_endpoint || binding.reason.is_some() || !binding.placements.is_empty(),
        w,
        "resolved endpoint needs a placement",
    )
}

fn check_marker(m: &SourceMarker, placements: &HashMap<&str, &Placement>, w: &str) -> R<()> {
    use crate::validation::{positive_labels, rate, require, text};
    for s in [&m.name, &m.comment, &m.user, &m.owner_occurrence] {
        text(s, w)?;
    }
    require(
        !m.owner_occurrence.is_empty(),
        w,
        "source marker needs owner occurrence",
    )?;
    require(
        m.start.is_some() || m.owner_local.is_some() || m.binding.reason.is_some(),
        w,
        "unknown marker positions need an unresolved reason",
    )?;
    require(
        m.start.is_none_or(|n| !n.is_negative())
            && !m.duration.is_negative()
            && m.source.is_none_or(|n| !n.is_negative()),
        w,
        "negative portable marker time",
    )?;
    if let Some(r) = m.owner_rate {
        rate(r, w)?;
    }
    if let Some(t) = m.track {
        positive_labels(&[t.index], w)?;
    }
    if let Some(c) = &m.color {
        text(&c.palette, w)?;
        text(&c.name, w)?;
    }
    check_binding(&m.binding, placements, m.owner == MarkerOwner::Edit, w)
}
