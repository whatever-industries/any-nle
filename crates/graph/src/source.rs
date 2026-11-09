//! Qualified source interpretation (`anynle.graph/2`). Unknown source facts are
//! explicit, and this type is never implicitly a legacy/prepared graph/1.
use super::*;
use std::io::Write;
mod multicam;
pub use multicam::{Multicam, MulticamLane, MulticamAlternative, MulticamPiece, MulticamUse};

pub const SCHEMA: &str = "anynle.graph/2";

/// Exact source timecode facts. Display base and physical counting rate are distinct.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceClock {
    pub effective_count: u64,
    pub rate: Rational,
    pub display_base: u16,
    pub drop: bool,
    pub provenance: SourceClockProvenance,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SourceClockProvenance {
    pub origin_count: u64,
    pub source_offset: i64,
    pub source_rate: Rational,
    pub source_package: String,
    pub timecode_track: String,
    pub route: String,
}
impl SourceClock {
    pub fn exact_start(&self) -> Option<Rational> {
        (self.rate.num() > 0)
            .then(|| {
                Rational::new(
                    self.effective_count as i128 * self.rate.den() as i128,
                    self.rate.num() as i128,
                )
            })
            .flatten()
    }
    fn validate(&self, start: Option<Rational>, w: &str) -> R<()> {
        use super::validation::{rate, require, text};
        rate(self.rate, w)?;
        rate(self.provenance.source_rate, w)?;
        require(
            self.display_base > 0,
            w,
            "clock display base must be positive",
        )?;
        for value in [
            &self.provenance.source_package,
            &self.provenance.timecode_track,
            &self.provenance.route,
        ] {
            text(value, w)?;
            require(!value.is_empty(), w, "clock provenance must be nonempty")?;
        }
        // Cancel rate factors before multiplication, so representable exact offsets do not
        // fail merely because the unreduced product exceeds i128.
        let mut n = [
            self.provenance.source_offset.unsigned_abs() as u128,
            self.rate.num() as u128,
            self.provenance.source_rate.den() as u128,
        ];
        let mut d = [
            self.rate.den() as u128,
            self.provenance.source_rate.num() as u128,
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
        let effective = if self.provenance.source_offset < 0 {
            self.provenance.origin_count.checked_sub(converted)
        } else {
            self.provenance.origin_count.checked_add(converted)
        };
        require(
            effective == Some(self.effective_count),
            w,
            "clock effective count contradicts origin and source offset",
        )?;
        let exact = self
            .exact_start()
            .ok_or_else(|| GraphError(format!("{w}: clock start is not representable")))?;
        require(
            start == Some(exact),
            w,
            "source start contradicts qualified clock",
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceGraph {
    pub project: Project,
    pub media: Vec<SourceMedia>,
    pub items: Vec<SourceItem>,
    pub bins: Vec<Bin>,
    pub sequences: Vec<SourceSequence>,
    pub losses: Vec<Loss>,
    pub sidecars: Vec<Sidecar>,
    /// Full source angle layouts, distinct from selected leaf-media placements.
    pub multicams: Vec<Multicam>,
    pub multicam_uses: Vec<MulticamUse>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceMedia {
    pub id: String,
    pub kind: StreamKind,
    pub locations: Vec<String>,
    pub codec: Option<String>,
    pub frame_size: Option<(i64, i64)>,
    pub rate: Option<Rational>,
    pub sample_rate: Option<i64>,
    pub bits: Option<i64>,
    pub duration: Rational,
    pub start: Option<Rational>,
    pub clock: Option<SourceClock>,
    pub stream: Option<i64>,
    pub online: Option<bool>,
    /// Explicit physical original-file selection, independent of the logical lane label.
    pub original: Option<OriginalStream>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OriginalStream {
    pub saved_locators: Vec<String>,
    pub container: String,
    pub stream_index: i64,
    pub channel_index: Option<i64>,
    pub channels: Option<i64>,
    pub time_base: Rational,
    pub start_pts: i64,
    pub duration_ts: i64,
    /// Codec sample representation. AAC is compressed and has no PCM bit depth.
    pub sample_representation: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceItemMedia {
    pub media: String,
    pub gain_db: f64,
    /// Exact item-to-file source offset, or unresolved. Never infer zero from None.
    pub source_offset: Option<Rational>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceItem {
    pub id: String,
    pub name: String,
    pub media: Vec<SourceItemMedia>,
    pub umid: Option<String>,
    pub tape: Option<String>,
    pub rate: Option<Rational>,
    pub color_rgb16: Option<[i64; 3]>,
    pub stream_groups: Vec<Vec<i64>>,
    pub markers: Vec<SourceMarker>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceSequence {
    pub id: String,
    pub name: String,
    pub rate: Rational,
    /// Qualified native canvas; absence does not imply a destination default.
    pub frame_size: Option<(i64, i64)>,
    pub start: Option<Rational>,
    pub clock: Option<SourceClock>,
    pub tracks: Vec<Track>,
    pub placements: Vec<Placement>,
    pub transitions: Vec<SourceTransition>,
    pub markers: Vec<SourceMarker>,
    /// Source text occurrences, independent of underlying picture/media edits.
    pub texts: Vec<SourceText>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind { Caption, Graphic }
impl TextKind {
    pub fn as_str(self) -> &'static str { match self { Self::Caption => "caption", Self::Graphic => "graphic" } }
}
/// Text content and its qualified record span. Appearance is not implied.
/// Missing content or timing is retained with a scoped explanation.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceText {
    pub id: String,
    pub kind: TextKind,
    pub track: i64,
    pub content: Option<String>,
    pub record_start: Option<Rational>,
    pub duration: Option<Rational>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceBinding {
    pub placements: Vec<String>,
    /// None means resolved; a nonempty reason preserves an unresolved binding.
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceTransition {
    pub id: String,
    pub kind: StreamKind,
    pub track: i64,
    pub cut: Rational,
    pub duration: Rational,
    pub cut_offset: Rational,
    pub style: String,
    pub outgoing: SourceBinding,
    pub incoming: SourceBinding,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceMarker {
    pub name: String,
    pub comment: String,
    pub user: String,
    pub color: Option<Label>,
    /// Qualified record/item-relative time, or unresolved.
    pub start: Option<Rational>,
    pub duration: Rational,
    pub track: Option<TrackRef>,
    pub owner: MarkerOwner,
    /// Input-revision-scoped native occurrence, not an object address.
    pub owner_occurrence: String,
    pub owner_local: Option<Rational>,
    pub owner_rate: Option<Rational>,
    pub binding: SourceBinding,
    pub source: Option<Rational>,
    pub inherited: bool,
}

impl SourceGraph {
    pub fn sidecar(&self, adapter: &str, object: &str) -> Option<&Value> {
        self.sidecars
            .iter()
            .find(|s| s.adapter == adapter && s.object == object)
            .map(|s| &s.data)
    }
    pub fn losses_for<'a>(
        &'a self,
        object: &'a str,
        aspect: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.losses
            .iter()
            .filter(move |l| l.object == object && l.aspect == aspect)
            .map(|l| l.reason.as_str())
    }

    pub fn validate(&self) -> R<()> {
        use super::validation::{
            distinct, finite, list, positive_labels, rate, require, text, value, ItemRoutes,
            MediaFacts,
        };
        text(&self.project.name, "source project")?;
        for n in [
            self.media.len(),
            self.items.len(),
            self.bins.len(),
            self.sequences.len(),
            self.losses.len(),
            self.sidecars.len(),
            self.multicams.len(),
            self.multicam_uses.len(),
        ] {
            list(n, "source graph")?;
        }
        let mut ids = HashMap::new();
        for m in &self.media {
            add_id(&mut ids, &m.id, "media")?;
        }
        for i in &self.items {
            add_id(&mut ids, &i.id, "item")?;
        }
        for b in &self.bins {
            add_id(&mut ids, &b.id, "bin")?;
        }
        for s in &self.sequences {
            add_id(&mut ids, &s.id, "sequence")?;
            for p in &s.placements {
                add_id(&mut ids, &p.id, "placement")?;
            }
            for t in &s.transitions {
                add_id(&mut ids, &t.id, "transition")?;
            }
            for t in &s.texts { add_id(&mut ids, &t.id, "text")?; }
        }
        let media: HashMap<_, _> = self.media.iter().map(|m| (m.id.as_str(), m)).collect();
        for m in &self.multicams {
            add_id(&mut ids, &m.id, "multicam")?;
            for lane in &m.lanes { for a in &lane.alternatives { add_id(&mut ids, &a.id, "multicam alternative")?; } }
        }
        for u in &self.multicam_uses { add_id(&mut ids, &u.id, "multicam use")?; }
        multicam::validate(self, &media)?;
        let ranges: HashMap<_, _> = self
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
        for m in &self.media {
            let w = format!("source media {:?}", m.id);
            if let Some(clock) = &m.clock {
                clock.validate(m.start, &w)?;
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
                require(Rational::new(o.duration_ts as i128 * o.time_base.num() as i128, o.time_base.den() as i128) == Some(m.duration), &w, "original physical duration contradicts stream clock")?;
                match m.kind {
                    StreamKind::Video => require(o.channels.is_none() && o.channel_index.is_none() && o.sample_representation.is_none(), &w, "picture cannot have audio channel facts")?,
                    StreamKind::Audio => {
                        require(matches!((o.channels,o.channel_index),(Some(n),Some(c)) if (1..=2).contains(&n) && (0..n).contains(&c)), &w, "invalid physical audio channel")?;
                        require(match o.sample_representation.as_deref() { Some("aac") => m.bits.is_none(), Some("pcm16") => m.bits==Some(16), Some("pcm24") => m.bits==Some(24), _=>false }, &w, "audio sample representation contradicts bit depth")?;
                    }
                }
            }
            if let Some(r) = m.rate {
                rate(r, &w)?;
            }
        }
        for i in &self.items {
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
        let routes: HashMap<_, _> = self
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
        for b in &self.bins {
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
        for s in &self.sequences {
            let w = format!("source sequence {:?}", s.id);
            if let Some(clock) = &s.clock {
                clock.validate(s.start, &w)?;
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
            let placements: HashMap<_, _> =
                s.placements.iter().map(|p| (p.id.as_str(), p)).collect();
            for p in &s.placements {
                require(
                    usize::from(p.video_track.is_some())
                        + p.audio_tracks.len()
                        + p.grouped_audio.len()
                        == 1,
                    &w,
                    "source placement must occupy exactly one lane",
                )?;
                let routing = routes
                    .get(p.item.as_str())
                    .ok_or_else(|| GraphError(format!("{w}: unknown item {:?}", p.item)))?;
                super::validation::placement(
                    p,
                    routing,
                    s.rate,
                    &format!("{w} placement {:?}", p.id),
                )?;
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
        for l in &self.losses {
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
        for s in &self.sidecars {
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
}

fn add_id<'a>(ids: &mut HashMap<&'a str, &'static str>, id: &'a str, kind: &'static str) -> R<()> {
    super::validation::text(id, "source id")?;
    super::validation::require(!id.is_empty(), "source graph", "empty id")?;
    super::validation::require(ids.insert(id, kind).is_none(), id, "duplicate id")
}

fn check_binding(
    binding: &SourceBinding,
    placements: &HashMap<&str, &Placement>,
    require_endpoint: bool,
    w: &str,
) -> R<()> {
    use super::validation::{list, require, text};
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
    use super::validation::{positive_labels, rate, require, text};
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

// Streaming serialization holds no cloned graph/Value tree. A count pass checks
// the complete encoding before a caller's output writer receives its first byte.
struct JsonOut<'a, W: Write> {
    writer: &'a mut W,
    bytes: usize,
}
macro_rules! member {
    ($o:ident,$first:ident,$key:literal,$write:expr) => {{
        $o.field($key, $first)?;
        $write?;
    }};
}
impl<W: Write> JsonOut<'_, W> {
    fn raw(&mut self, text: &str) -> R<()> {
        self.bytes = self
            .bytes
            .checked_add(text.len())
            .filter(|n| *n <= MAX_GRAPH_BYTES)
            .ok_or_else(|| GraphError("source graph encoding exceeds 64 MiB".into()))?;
        self.writer
            .write_all(text.as_bytes())
            .map_err(|e| GraphError(format!("source graph output: {e}")))
    }
    fn text(&mut self, text: &str) -> R<()> {
        self.raw("\"")?;
        let mut start = 0;
        for (i, c) in text.char_indices() {
            let escaped = match c {
                '"' => Some("\\\""),
                '\\' => Some("\\\\"),
                '\n' => Some("\\n"),
                '\r' => Some("\\r"),
                '\t' => Some("\\t"),
                _ => None,
            };
            if let Some(e) = escaped {
                self.raw(&text[start..i])?;
                self.raw(e)?;
                start = i + c.len_utf8();
            } else if (c as u32) < 0x20 {
                self.raw(&text[start..i])?;
                self.raw(&format!("\\u{:04x}", c as u32))?;
                start = i + c.len_utf8();
            }
        }
        self.raw(&text[start..])?;
        self.raw("\"")
    }
    fn field(&mut self, name: &str, first: &mut bool) -> R<()> {
        if !*first {
            self.raw(",")?;
        }
        *first = false;
        self.text(name)?;
        self.raw(":")
    }
    fn object(&mut self, body: impl FnOnce(&mut Self, &mut bool) -> R<()>) -> R<()> {
        self.raw("{")?;
        body(self, &mut true)?;
        self.raw("}")
    }
    fn array<T>(&mut self, items: &[T], mut item: impl FnMut(&mut Self, &T) -> R<()>) -> R<()> {
        self.raw("[")?;
        for (i, v) in items.iter().enumerate() {
            if i > 0 {
                self.raw(",")?;
            }
            item(self, v)?;
        }
        self.raw("]")
    }
    fn value(&mut self, v: &Value) -> R<()> {
        match v {
            Value::Null => self.raw("null"),
            Value::Bool(b) => self.raw(if *b { "true" } else { "false" }),
            Value::Int(n) => self.raw(&n.to_string()),
            Value::Float(n) => self.raw(&format!("{n:?}")),
            Value::Str(s) => self.text(s),
            Value::Arr(a) => self.array(a, Self::value),
            Value::Obj(a) => self.object(|o, f| {
                for (k, v) in a {
                    o.field(k, f)?;
                    o.value(v)?;
                }
                Ok(())
            }),
        }
    }
    fn integer(&mut self, n: i64) -> R<()> {
        self.raw(&n.to_string())
    }
    fn float(&mut self, n: f64) -> R<()> {
        self.raw(&format!("{n:?}"))
    }
    fn boolean(&mut self, n: bool) -> R<()> {
        self.raw(if n { "true" } else { "false" })
    }
    fn time(&mut self, n: Rational) -> R<()> {
        self.text(&n.to_string())
    }
    fn optional<T>(&mut self, n: Option<T>, render: impl FnOnce(&mut Self, T) -> R<()>) -> R<()> {
        match n {
            Some(v) => render(self, v),
            None => self.raw("null"),
        }
    }
    fn times(&mut self, n: Option<Rational>) -> R<()> {
        self.optional(n, Self::time)
    }
    fn integers(&mut self, n: &[i64]) -> R<()> {
        self.array(n, |o, n| o.integer(*n))
    }
    fn label(&mut self, c: &Label) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "palette", o.text(&c.palette));
            member!(o, f, "name", o.text(&c.name));
            Ok(())
        })
    }
    fn track(&mut self, t: TrackRef) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "kind", o.text(t.kind.as_str()));
            member!(o, f, "index", o.integer(t.index));
            Ok(())
        })
    }
    fn binding(&mut self, b: &SourceBinding) -> R<()> {
        self.object(|o, f| {
            member!(
                o,
                f,
                "placements",
                o.array(&b.placements, |o, id| o.text(id))
            );
            member!(o, f, "reason", o.optional(b.reason.as_deref(), Self::text));
            Ok(())
        })
    }
    fn marker(&mut self, m: &SourceMarker) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "name", o.text(&m.name));
            member!(o, f, "comment", o.text(&m.comment));
            member!(o, f, "user", o.text(&m.user));
            member!(o, f, "color", o.optional(m.color.as_ref(), Self::label));
            member!(o, f, "start", o.times(m.start));
            member!(o, f, "duration", o.time(m.duration));
            member!(o, f, "track", o.optional(m.track, Self::track));
            member!(o, f, "owner", o.text(m.owner.as_str()));
            member!(o, f, "owner_occurrence", o.text(&m.owner_occurrence));
            member!(o, f, "owner_local", o.times(m.owner_local));
            member!(o, f, "owner_rate", o.times(m.owner_rate));
            member!(o, f, "binding", o.binding(&m.binding));
            member!(o, f, "source", o.times(m.source));
            member!(o, f, "inherited", o.boolean(m.inherited));
            Ok(())
        })
    }
    fn media(&mut self, m: &SourceMedia) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&m.id));
            member!(o, f, "kind", o.text(m.kind.as_str()));
            member!(o, f, "locations", o.array(&m.locations, |o, p| o.text(p)));
            member!(o, f, "codec", o.optional(m.codec.as_deref(), Self::text));
            member!(
                o,
                f,
                "frame_size",
                o.optional(m.frame_size, |o, (x, y)| o.integers(&[x, y]))
            );
            member!(o, f, "rate", o.times(m.rate));
            member!(
                o,
                f,
                "sample_rate",
                o.optional(m.sample_rate, Self::integer)
            );
            member!(o, f, "bits", o.optional(m.bits, Self::integer));
            member!(o, f, "duration", o.time(m.duration));
            member!(o, f, "start", o.times(m.start));
            member!(o, f, "clock", o.optional(m.clock.as_ref(), Self::clock));
            member!(o, f, "stream", o.optional(m.stream, Self::integer));
            member!(o, f, "online", o.optional(m.online, Self::boolean));
            if let Some(original) = &m.original { member!(o, f, "original", o.original(original)); }
            Ok(())
        })
    }
    fn original(&mut self, v: &OriginalStream) -> R<()> {
        self.object(|o,f| {
            member!(o,f,"saved_locators",o.array(&v.saved_locators,|o,p|o.text(p)));
            member!(o,f,"container",o.text(&v.container));
            member!(o,f,"stream_index",o.integer(v.stream_index));
            member!(o,f,"channel_index",o.optional(v.channel_index,Self::integer));
            member!(o,f,"channels",o.optional(v.channels,Self::integer));
            member!(o,f,"time_base",o.time(v.time_base));
            member!(o,f,"start_pts",o.integer(v.start_pts));
            member!(o,f,"duration_ts",o.integer(v.duration_ts));
            member!(o,f,"sample_representation",o.optional(v.sample_representation.as_deref(),Self::text));
            Ok(())
        })
    }
    fn item(&mut self, i: &SourceItem) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&i.id));
            member!(o, f, "name", o.text(&i.name));
            member!(
                o,
                f,
                "media",
                o.array(&i.media, |o, m| o.object(|o, f| {
                    member!(o, f, "media", o.text(&m.media));
                    member!(o, f, "gain_db", o.float(m.gain_db));
                    member!(o, f, "source_offset", o.times(m.source_offset));
                    Ok(())
                }))
            );
            member!(o, f, "umid", o.optional(i.umid.as_deref(), Self::text));
            member!(o, f, "tape", o.optional(i.tape.as_deref(), Self::text));
            member!(o, f, "rate", o.times(i.rate));
            member!(
                o,
                f,
                "color_rgb16",
                o.optional(i.color_rgb16, |o, c| o.integers(&c))
            );
            member!(
                o,
                f,
                "stream_groups",
                o.array(&i.stream_groups, |o, g| o.integers(g))
            );
            member!(o, f, "markers", o.array(&i.markers, Self::marker));
            Ok(())
        })
    }
    fn placement(&mut self, p: &Placement) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&p.id));
            member!(o, f, "item", o.text(&p.item));
            member!(o, f, "record_start", o.time(p.record_start));
            member!(o, f, "source_in", o.time(p.source_in));
            member!(o, f, "duration", o.time(p.duration));
            member!(
                o,
                f,
                "video_track",
                o.optional(p.video_track, Self::integer)
            );
            member!(o, f, "audio_tracks", o.integers(&p.audio_tracks));
            member!(o, f, "audio_streams", o.integers(&p.audio_streams));
            member!(
                o,
                f,
                "grouped_audio",
                o.array(&p.grouped_audio, |o, g| o.object(|o, f| {
                    member!(o, f, "track", o.integer(g.track));
                    member!(o, f, "streams", o.integers(&g.streams));
                    member!(o, f, "gain_db", o.optional(g.gain_db, Self::float));
                    member!(
                        o,
                        f,
                        "pans",
                        o.array(&g.pans, |o, p| o.optional(*p, Self::float))
                    );
                    Ok(())
                }))
            );
            member!(o, f, "gain_db", o.float(p.gain_db));
            member!(o, f, "volume_db", o.optional(p.volume_db, Self::float));
            member!(
                o,
                f,
                "volume_keys",
                o.array(&p.volume_keys, |o, k| o.object(|o, f| {
                    member!(o, f, "at", o.time(k.at));
                    member!(o, f, "db", o.float(k.db));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "track_gains_db",
                o.array(&p.track_gains_db, |o, n| o.optional(*n, Self::float))
            );
            member!(
                o,
                f,
                "track_pans",
                o.array(&p.track_pans, |o, n| o.optional(*n, Self::float))
            );
            member!(
                o,
                f,
                "speed",
                o.optional(p.speed.as_ref(), |o, s| o.object(|o, f| {
                    member!(o, f, "ratio", o.integers(&[s.ratio.0, s.ratio.1]));
                    member!(o, f, "source_duration", o.time(s.source_duration));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "frame_map",
                o.optional(p.frame_map.as_ref(), |o, m| o.integers(m))
            );
            member!(
                o,
                f,
                "transform",
                o.optional(p.transform.as_ref(), |o, t| o.object(|o, f| {
                    member!(
                        o,
                        f,
                        "position",
                        o.array(&[t.position.0, t.position.1], |o, n| o.float(*n))
                    );
                    member!(
                        o,
                        f,
                        "scale",
                        o.array(&[t.scale.0, t.scale.1], |o, n| o.float(*n))
                    );
                    member!(o, f, "rotation", o.float(t.rotation));
                    member!(
                        o,
                        f,
                        "anchor",
                        o.array(&[t.anchor.0, t.anchor.1], |o, n| o.float(*n))
                    );
                    member!(o, f, "crop", o.array(&t.crop, |o, n| o.float(*n)));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "effects",
                o.array(&p.effects, |o, e| o.object(|o, f| {
                    member!(o, f, "kind", o.text(&e.kind));
                    member!(o, f, "params", o.value(&e.params));
                    Ok(())
                }))
            );
            Ok(())
        })
    }
    fn sequence(&mut self, s: &SourceSequence) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "id", o.text(&s.id));
            member!(o, f, "name", o.text(&s.name));
            member!(o, f, "rate", o.time(s.rate));
            member!(
                o,
                f,
                "frame_size",
                o.optional(s.frame_size, |o, (x, y)| o.integers(&[x, y]))
            );
            member!(o, f, "start", o.times(s.start));
            member!(o, f, "clock", o.optional(s.clock.as_ref(), Self::clock));
            member!(
                o,
                f,
                "tracks",
                o.array(&s.tracks, |o, t| o.object(|o, f| {
                    member!(o, f, "kind", o.text(t.kind.as_str()));
                    member!(o, f, "index", o.integer(t.index));
                    member!(o, f, "level_db", o.optional(t.level_db, Self::float));
                    Ok(())
                }))
            );
            member!(o, f, "placements", o.array(&s.placements, Self::placement));
            member!(
                o,
                f,
                "transitions",
                o.array(&s.transitions, |o, t| o.object(|o, f| {
                    member!(o, f, "id", o.text(&t.id));
                    member!(o, f, "kind", o.text(t.kind.as_str()));
                    member!(o, f, "track", o.integer(t.track));
                    member!(o, f, "cut", o.time(t.cut));
                    member!(o, f, "duration", o.time(t.duration));
                    member!(o, f, "cut_offset", o.time(t.cut_offset));
                    member!(o, f, "style", o.text(&t.style));
                    member!(o, f, "outgoing", o.binding(&t.outgoing));
                    member!(o, f, "incoming", o.binding(&t.incoming));
                    Ok(())
                }))
            );
            member!(o, f, "markers", o.array(&s.markers, Self::marker));
            if !s.texts.is_empty() {
                member!(o, f, "texts", o.array(&s.texts, |o, t| o.object(|o, f| {
                    member!(o, f, "id", o.text(&t.id));
                    member!(o, f, "kind", o.text(t.kind.as_str()));
                    member!(o, f, "track", o.integer(t.track));
                    member!(o, f, "content", o.optional(t.content.as_deref(), Self::text));
                    member!(o, f, "record_start", o.times(t.record_start));
                    member!(o, f, "duration", o.times(t.duration));
                    member!(o, f, "reason", o.optional(t.reason.as_deref(), Self::text));
                    Ok(())
                })));
            }
            Ok(())
        })
    }
    fn graph(&mut self, g: &SourceGraph) -> R<()> {
        self.object(|o, f| {
            member!(o, f, "schema", o.text(SCHEMA));
            member!(
                o,
                f,
                "project",
                o.object(|o, f| {
                    member!(o, f, "name", o.text(&g.project.name));
                    Ok(())
                })
            );
            member!(o, f, "media", o.array(&g.media, Self::media));
            member!(o, f, "items", o.array(&g.items, Self::item));
            member!(
                o,
                f,
                "bins",
                o.array(&g.bins, |o, b| o.object(|o, f| {
                    member!(o, f, "id", o.text(&b.id));
                    member!(o, f, "name", o.text(&b.name));
                    member!(o, f, "parent", o.optional(b.parent.as_deref(), Self::text));
                    member!(o, f, "items", o.array(&b.items, |o, i| o.text(i)));
                    Ok(())
                }))
            );
            member!(o, f, "sequences", o.array(&g.sequences, Self::sequence));
            if !g.multicams.is_empty() { member!(o, f, "multicams", o.array(&g.multicams, Self::multicam)); }
            if !g.multicam_uses.is_empty() { member!(o, f, "multicam_uses", o.array(&g.multicam_uses, Self::multicam_use)); }
            member!(
                o,
                f,
                "losses",
                o.array(&g.losses, |o, l| o.object(|o, f| {
                    member!(o, f, "object", o.text(&l.object));
                    member!(o, f, "aspect", o.text(&l.aspect));
                    member!(o, f, "reason", o.text(&l.reason));
                    Ok(())
                }))
            );
            member!(
                o,
                f,
                "sidecars",
                o.array(&g.sidecars, |o, s| o.object(|o, f| {
                    member!(o, f, "adapter", o.text(&s.adapter));
                    member!(o, f, "object", o.text(&s.object));
                    member!(o, f, "data", o.value(&s.data));
                    Ok(())
                }))
            );
            Ok(())
        })
    }
    fn multicam(&mut self, m:&Multicam)->R<()> {
        self.object(|o,f|{
            member!(o,f,"id",o.text(&m.id)); member!(o,f,"name",o.text(&m.name));
            member!(o,f,"rate",o.times(m.rate)); member!(o,f,"duration",o.times(m.duration));
            member!(o,f,"reason",o.optional(m.reason.as_deref(),Self::text));
            member!(o,f,"lanes",o.array(&m.lanes,|o,l|o.object(|o,f|{
                member!(o,f,"kind",o.text(l.kind.as_str())); member!(o,f,"index",o.integer(l.index));
                member!(o,f,"selected",o.optional(l.selected,Self::integer));
                member!(o,f,"reason",o.optional(l.reason.as_deref(),Self::text));
                member!(o,f,"alternatives",o.array(&l.alternatives,|o,a|o.object(|o,f|{
                    member!(o,f,"id",o.text(&a.id)); member!(o,f,"reason",o.optional(a.reason.as_deref(),Self::text));
                    member!(o,f,"pieces",o.array(&a.pieces,|o,p|o.object(|o,f|{
                        member!(o,f,"start",o.time(p.start)); member!(o,f,"duration",o.time(p.duration));
                        member!(o,f,"item",o.optional(p.item.as_deref(),Self::text));
                        member!(o,f,"stream",o.optional(p.stream,Self::integer));
                        member!(o,f,"source_in",o.times(p.source_in)); Ok(())
                    }))); Ok(())
                }))); Ok(())
            }))); Ok(())
        })
    }
    fn multicam_use(&mut self,u:&MulticamUse)->R<()> {
        self.object(|o,f|{
            member!(o,f,"id",o.text(&u.id)); member!(o,f,"sequence",o.text(&u.sequence));
            member!(o,f,"candidates",o.array(&u.candidates,|o,id|o.text(id)));
            member!(o,f,"kind",o.text(u.kind.as_str())); member!(o,f,"lane",o.optional(u.lane,Self::integer));
            member!(o,f,"track",o.integer(u.track)); member!(o,f,"selected",o.optional(u.selected,Self::integer));
            member!(o,f,"record_start",o.times(u.record_start)); member!(o,f,"duration",o.times(u.duration));
            member!(o,f,"source_in",o.times(u.source_in)); member!(o,f,"binding",o.binding(&u.binding));
            member!(o,f,"reason",o.optional(u.reason.as_deref(),Self::text)); Ok(())
        })
    }
    fn clock(&mut self, c: &SourceClock) -> R<()> {
        self.object(|o, f| {
            member!(
                o,
                f,
                "effective_count",
                o.text(&c.effective_count.to_string())
            );
            member!(o, f, "rate", o.time(c.rate));
            member!(o, f, "display_base", o.integer(c.display_base as i64));
            member!(o, f, "drop", o.boolean(c.drop));
            member!(
                o,
                f,
                "provenance",
                o.object(|o, f| {
                    member!(
                        o,
                        f,
                        "origin_count",
                        o.text(&c.provenance.origin_count.to_string())
                    );
                    member!(o, f, "source_offset", o.integer(c.provenance.source_offset));
                    member!(o, f, "source_rate", o.time(c.provenance.source_rate));
                    member!(o, f, "source_package", o.text(&c.provenance.source_package));
                    member!(o, f, "timecode_track", o.text(&c.provenance.timecode_track));
                    member!(o, f, "route", o.text(&c.provenance.route));
                    Ok(())
                })
            );
            Ok(())
        })
    }
}

fn parse_original(v: &Value) -> R<OriginalStream> {
    let o = Obj::new(v,"original stream", &["saved_locators","container","stream_index","time_base","start_pts","duration_ts"], &["channel_index","channels","sample_representation"])?;
    Ok(OriginalStream {
        saved_locators: o.list("saved_locators")?.iter().map(|v|text(v,"saved locator")).collect::<R<_>>()?,
        container:o.str("container")?, stream_index:o.int("stream_index")?, channel_index:o.opt_int("channel_index")?, channels:o.opt_int("channels")?,
        time_base:o.time("time_base")?,start_pts:o.int("start_pts")?, duration_ts:o.int("duration_ts")?, sample_representation:o.opt_str("sample_representation")?,
    })
}

fn parse_clock(v: &Value, w: &str) -> R<SourceClock> {
    fn count(v: &Value, w: &str) -> R<u64> {
        let Value::Str(s) = v else {
            return fail(format!("{w}: expected canonical decimal u64 string"));
        };
        if s.is_empty()
            || s.len() > 20
            || (s.len() > 1 && s.starts_with('0'))
            || !s.bytes().all(|c| c.is_ascii_digit())
        {
            return fail(format!("{w}: expected canonical decimal u64 string"));
        }
        s.parse()
            .map_err(|_| GraphError(format!("{w}: count exceeds u64")))
    }
    let c = Obj::new(
        v,
        w,
        &[
            "effective_count",
            "rate",
            "display_base",
            "drop",
            "provenance",
        ],
        &[],
    )?;
    let p = Obj::new(
        c.req("provenance"),
        &c.at("provenance"),
        &[
            "origin_count",
            "source_offset",
            "source_rate",
            "source_package",
            "timecode_track",
            "route",
        ],
        &[],
    )?;
    Ok(SourceClock {
        effective_count: count(c.req("effective_count"), &c.at("effective_count"))?,
        rate: c.time("rate")?,
        display_base: u16::try_from(c.int("display_base")?)
            .map_err(|_| GraphError(format!("{w}: display base outside u16")))?,
        drop: c.opt_bool("drop")?.expect("required"),
        provenance: SourceClockProvenance {
            origin_count: count(p.req("origin_count"), &p.at("origin_count"))?,
            source_offset: p.int("source_offset")?,
            source_rate: p.time("source_rate")?,
            source_package: p.str("source_package")?,
            timecode_track: p.str("timecode_track")?,
            route: p.str("route")?,
        },
    })
}

impl SourceGraph {
    /// Count the bounded encoding without constructing a graph-wide Value tree.
    pub fn encoded_len(&self) -> R<usize> {
        self.validate()?;
        let mut sink = std::io::sink();
        let mut out = JsonOut {
            writer: &mut sink,
            bytes: 0,
        };
        out.graph(self)?;
        Ok(out.bytes)
    }
    pub fn write_json(&self, writer: &mut impl Write) -> R<()> {
        self.encoded_len()?;
        JsonOut { writer, bytes: 0 }.graph(self)
    }
    pub fn to_json_text(&self) -> R<String> {
        let len = self.encoded_len()?;
        let mut bytes = Vec::with_capacity(len);
        JsonOut {
            writer: &mut bytes,
            bytes: 0,
        }
        .graph(self)?;
        String::from_utf8(bytes).map_err(|_| GraphError("source graph output is not UTF-8".into()))
    }

    pub fn from_json_text(text: &str) -> R<Self> {
        if text.len() > MAX_GRAPH_BYTES {
            return fail("source graph: file too large");
        }
        Self::from_json(&json::parse(text).map_err(GraphError)?)
    }
    pub fn from_json(v: &Value) -> R<Self> {
        // A caller-supplied Value has not passed the text parser. Check nested
        // shapes and encoded size before cloning any retained native payload.
        super::validation::value(v, 0, "source graph")?;
        let mut sink = std::io::sink();
        JsonOut {
            writer: &mut sink,
            bytes: 0,
        }
        .value(v)?;
        let o = Obj::new(
            v,
            "source graph",
            &["schema", "project", "media", "items", "bins", "sequences"],
            &["losses", "sidecars", "multicams", "multicam_uses"],
        )?;
        if o.str("schema")? != SCHEMA {
            return fail(format!("source graph.schema: expected {SCHEMA}"));
        }
        let project = Obj::new(o.req("project"), "source graph.project", &["name"], &[])?;
        let mut media = Vec::new();
        for (i, m) in o.list("media")?.iter().enumerate() {
            let m = Obj::new(
                m,
                &format!("source graph.media[{i}]"),
                &["id", "kind", "locations", "duration"],
                &[
                    "codec",
                    "frame_size",
                    "rate",
                    "sample_rate",
                    "bits",
                    "start",
                    "clock",
                    "stream",
                    "online",
                    "original",
                ],
            )?;
            media.push(SourceMedia {
                id: m.str("id")?,
                kind: m.kind("kind")?,
                locations: m
                    .list("locations")?
                    .iter()
                    .map(|p| text(p, &m.at("locations")))
                    .collect::<R<_>>()?,
                codec: m.opt_str("codec")?,
                frame_size: m.opt_size("frame_size")?,
                rate: m.opt_time("rate")?,
                sample_rate: m.opt_int("sample_rate")?,
                bits: m.opt_int("bits")?,
                duration: m.time("duration")?,
                start: m.opt_time("start")?,
                clock: m
                    .opt("clock")
                    .map(|v| parse_clock(v, &m.at("clock")))
                    .transpose()?,
                stream: m.opt_int("stream")?,
                online: m.opt_bool("online")?,
                original: m.opt("original").map(parse_original).transpose()?,
            });
        }
        let mut items = Vec::new();
        for (i, item) in o.list("items")?.iter().enumerate() {
            let w = format!("source graph.items[{i}]");
            let item = Obj::new(
                item,
                &w,
                &["id", "name", "media"],
                &[
                    "umid",
                    "tape",
                    "rate",
                    "color_rgb16",
                    "stream_groups",
                    "markers",
                ],
            )?;
            let mut uses = Vec::new();
            for (j, m) in item.list("media")?.iter().enumerate() {
                let m = Obj::new(
                    m,
                    &format!("{w}.media[{j}]"),
                    &["media"],
                    &["gain_db", "source_offset"],
                )?;
                uses.push(SourceItemMedia {
                    media: m.str("media")?,
                    gain_db: m.opt_float("gain_db")?.unwrap_or(0.0),
                    source_offset: m.opt_time("source_offset")?,
                });
            }
            let color_rgb16 = if let Some(v) = item.opt("color_rgb16") {
                let ns = ints(v, &item.at("color_rgb16"))?;
                if ns.len() != 3 {
                    return fail(format!("{w}.color_rgb16: expected [r,g,b]"));
                }
                Some([ns[0], ns[1], ns[2]])
            } else {
                None
            };
            items.push(SourceItem {
                id: item.str("id")?,
                name: item.str("name")?,
                media: uses,
                umid: item.opt_str("umid")?,
                tape: item.opt_str("tape")?,
                rate: item.opt_time("rate")?,
                color_rgb16,
                stream_groups: item
                    .opt_list("stream_groups")?
                    .iter()
                    .map(|g| ints(g, &item.at("stream_groups")))
                    .collect::<R<_>>()?,
                markers: parse_markers(&item)?,
            });
        }
        let mut bins = Vec::new();
        for (i, b) in o.list("bins")?.iter().enumerate() {
            let b = Obj::new(
                b,
                &format!("source graph.bins[{i}]"),
                &["id", "name", "items"],
                &["parent"],
            )?;
            bins.push(Bin {
                id: b.str("id")?,
                name: b.str("name")?,
                parent: b.opt_str("parent")?,
                items: b
                    .list("items")?
                    .iter()
                    .map(|v| text(v, &b.at("items")))
                    .collect::<R<_>>()?,
            });
        }
        let mut sequences = Vec::new();
        for (i, s) in o.list("sequences")?.iter().enumerate() {
            let w = format!("source graph.sequences[{i}]");
            let s = Obj::new(
                s,
                &w,
                &["id", "name", "rate", "placements"],
                &[
                    "frame_size",
                    "start",
                    "clock",
                    "tracks",
                    "transitions",
                    "markers",
                    "texts",
                ],
            )?;
            let mut tracks = Vec::new();
            for (j, t) in s.opt_list("tracks")?.iter().enumerate() {
                let t = Obj::new(
                    t,
                    &format!("{w}.tracks[{j}]"),
                    &["kind", "index"],
                    &["level_db"],
                )?;
                tracks.push(Track {
                    kind: t.kind("kind")?,
                    index: t.int("index")?,
                    level_db: t.opt_float("level_db")?,
                });
            }
            let placements = s
                .list("placements")?
                .iter()
                .enumerate()
                .map(|(j, p)| super::placement(p, &format!("{w}.placements[{j}]")))
                .collect::<R<_>>()?;
            let mut transitions = Vec::new();
            for (j, t) in s.opt_list("transitions")?.iter().enumerate() {
                let tw = format!("{w}.transitions[{j}]");
                let t = Obj::new(
                    t,
                    &tw,
                    &[
                        "id",
                        "kind",
                        "track",
                        "cut",
                        "duration",
                        "cut_offset",
                        "style",
                        "outgoing",
                        "incoming",
                    ],
                    &[],
                )?;
                transitions.push(SourceTransition {
                    id: t.str("id")?,
                    kind: t.kind("kind")?,
                    track: t.int("track")?,
                    cut: t.time("cut")?,
                    duration: t.time("duration")?,
                    cut_offset: t.time("cut_offset")?,
                    style: t.str("style")?,
                    outgoing: parse_binding(t.req("outgoing"), &t.at("outgoing"))?,
                    incoming: parse_binding(t.req("incoming"), &t.at("incoming"))?,
                });
            }
            sequences.push(SourceSequence {
                id: s.str("id")?,
                name: s.str("name")?,
                rate: s.time("rate")?,
                frame_size: s.opt_size("frame_size")?,
                start: s.opt_time("start")?,
                clock: s
                    .opt("clock")
                    .map(|v| parse_clock(v, &s.at("clock")))
                    .transpose()?,
                tracks,
                placements,
                transitions,
                markers: parse_markers(&s)?,
                texts: s.opt_list("texts")?.iter().enumerate().map(|(j, v)| {
                    let t = Obj::new(v, &format!("{w}.texts[{j}]"), &["id", "kind", "track"], &["content", "record_start", "duration", "reason"])?;
                    let kind = match t.str("kind")?.as_str() {
                        "caption" => TextKind::Caption, "graphic" => TextKind::Graphic,
                        _ => return fail(format!("{w}: unknown text kind")),
                    };
                    Ok(SourceText { id: t.str("id")?, kind, track: t.int("track")?, content: t.opt_str("content")?, record_start: t.opt_time("record_start")?, duration: t.opt_time("duration")?, reason: t.opt_str("reason")? })
                }).collect::<R<_>>()?,
            });
        }
        let mut losses = Vec::new();
        for (i, l) in o.opt_list("losses")?.iter().enumerate() {
            let l = Obj::new(
                l,
                &format!("source graph.losses[{i}]"),
                &["object", "aspect", "reason"],
                &[],
            )?;
            losses.push(Loss {
                object: l.str("object")?,
                aspect: l.str("aspect")?,
                reason: l.str("reason")?,
            });
        }
        let mut sidecars = Vec::new();
        for (i, s) in o.opt_list("sidecars")?.iter().enumerate() {
            let s = Obj::new(
                s,
                &format!("source graph.sidecars[{i}]"),
                &["adapter", "object"],
                &["data"],
            )?;
            sidecars.push(Sidecar {
                adapter: s.str("adapter")?,
                object: s.str("object")?,
                data: s.raw("data")?.clone(),
            });
        }
        let source = Self {
            project: Project {
                name: project.str("name")?,
            },
            media,
            items,
            bins,
            sequences,
            losses,
            sidecars,
            multicams: o.opt_list("multicams")?.iter().enumerate().map(|(i,v)|Multicam::parse(v,&format!("source graph.multicams[{i}]"))).collect::<R<_>>()?,
            multicam_uses: o.opt_list("multicam_uses")?.iter().enumerate().map(|(i,v)|MulticamUse::parse(v,&format!("source graph.multicam_uses[{i}]"))).collect::<R<_>>()?,
        };
        source.validate()?;
        Ok(source)
    }
}

fn parse_binding(value: &Value, w: &str) -> R<SourceBinding> {
    let b = Obj::new(value, w, &["placements"], &["reason"])?;
    Ok(SourceBinding {
        placements: b
            .list("placements")?
            .iter()
            .map(|v| text(v, &b.at("placements")))
            .collect::<R<_>>()?,
        reason: b.opt_str("reason")?,
    })
}

fn parse_markers(o: &Obj) -> R<Vec<SourceMarker>> {
    let mut result = Vec::new();
    for (i, m) in o.opt_list("markers")?.iter().enumerate() {
        let w = format!("{}.markers[{i}]", o.w);
        let m = Obj::new(
            m,
            &w,
            &["name", "duration", "owner", "owner_occurrence", "binding"],
            &[
                "comment",
                "user",
                "color",
                "start",
                "track",
                "owner_local",
                "owner_rate",
                "source",
                "inherited",
            ],
        )?;
        let color = match m.opt("color") {
            None => None,
            Some(v) => {
                let v = Obj::new(v, &m.at("color"), &["palette", "name"], &[])?;
                Some(Label {
                    palette: v.str("palette")?,
                    name: v.str("name")?,
                })
            }
        };
        let track = match m.opt("track") {
            None => None,
            Some(v) => {
                let v = Obj::new(v, &m.at("track"), &["kind", "index"], &[])?;
                Some(TrackRef {
                    kind: v.kind("kind")?,
                    index: v.int("index")?,
                })
            }
        };
        let owner = MarkerOwner::parse(&m.str("owner")?)
            .ok_or_else(|| GraphError(format!("{w}: unknown marker owner")))?;
        result.push(SourceMarker {
            name: m.str("name")?,
            comment: m.opt_str("comment")?.unwrap_or_default(),
            user: m.opt_str("user")?.unwrap_or_default(),
            color,
            start: m.opt_time("start")?,
            duration: m.time("duration")?,
            track,
            owner,
            owner_occurrence: m.str("owner_occurrence")?,
            owner_local: m.opt_time("owner_local")?,
            owner_rate: m.opt_time("owner_rate")?,
            binding: parse_binding(m.req("binding"), &m.at("binding"))?,
            source: m.opt_time("source")?,
            inherited: m.opt_bool("inherited")?.unwrap_or(false),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> SourceGraph {
        SourceGraph::from_json_text(r#"{
          "schema":"anynle.graph/2","project":{"name":"Source Δ"},
          "media":[
            {"id":"v","kind":"video","locations":["/v.mxf"],"rate":"24","duration":"4","start":null},
            {"id":"a1","kind":"audio","locations":["/a1.mxf"],"sample_rate":48000,"bits":24,"stream":1,"duration":"4","start":null},
            {"id":"a2","kind":"audio","locations":["/a2.mxf"],"sample_rate":48000,"bits":24,"stream":2,"duration":"4","start":"0"}],
          "items":[{"id":"i","name":"Master","media":[{"media":"v","source_offset":"0"},{"media":"a1","source_offset":null},{"media":"a2","source_offset":"0"}],"stream_groups":[[1,2]]}],
          "bins":[{"id":"b","name":"Bin","items":["i"]}],
          "sequences":[{"id":"s","name":"Original overlap","rate":"24","frame_size":[1920,1080],"start":null,
            "tracks":[{"kind":"audio","index":9}],
            "placements":[
              {"id":"picture","item":"i","record_start":"0","source_in":"0","duration":"2","video_track":1},
              {"id":"out","item":"i","record_start":"0","source_in":"0","duration":"5/4","grouped_audio":[{"track":3,"streams":[1,2]}]},
              {"id":"in","item":"i","record_start":"3/4","source_in":"2","duration":"5/4","grouped_audio":[{"track":3,"streams":[1,2]}]}],
            "transitions":[{"id":"t","kind":"audio","track":3,"cut":"1","duration":"1/2","cut_offset":"1/4","style":"avid.DISSOLVE","outgoing":{"placements":["out"]},"incoming":{"placements":["in"]}}],
            "markers":[{"name":"Unresolved owner clock","duration":"0","owner":"edit","owner_occurrence":"sha256:fixture/SEQU/2/TNFX/child","owner_local":"-1/24","owner_rate":"24","start":null,"binding":{"placements":["out","in"],"reason":"retimed owner clock is unqualified"}}]}],
          "sidecars":[{"adapter":"avid.source","object":"s","data":null}]
        }"#).unwrap()
    }
    fn multicam_example()->SourceGraph{
        let mut g=example();
        let media=|stream|MulticamPiece{start:Rational::new(1,1).unwrap(),duration:Rational::new(3,1).unwrap(),item:Some("i".into()),stream:Some(stream),source_in:Some(Rational::ZERO)};
        let gap=MulticamPiece{start:Rational::ZERO,duration:Rational::new(1,1).unwrap(),item:None,stream:None,source_in:None};
        g.multicams.push(Multicam{id:"group".into(),name:"Offset group".into(),rate:Some(Rational::new(24,1).unwrap()),duration:Some(Rational::new(4,1).unwrap()),reason:None,lanes:vec![
            MulticamLane{kind:StreamKind::Video,index:1,selected:Some(0),reason:None,alternatives:vec![
                MulticamAlternative{id:"angle-a".into(),pieces:vec![gap.clone(),media(1)],reason:None},
                MulticamAlternative{id:"angle-b".into(),pieces:vec![MulticamPiece{start:Rational::ZERO,duration:Rational::new(4,1).unwrap(),..media(1)}],reason:None}]},
            // The one audio alternative is independent of the two video angles.
            MulticamLane{kind:StreamKind::Audio,index:3,selected:Some(0),reason:None,alternatives:vec![MulticamAlternative{id:"audio-only-choice".into(),pieces:vec![gap,media(2)],reason:None}]},
        ]});
        g.bins[0].items.push("group".into());
        g.multicam_uses.push(MulticamUse{id:"use".into(),sequence:"s".into(),candidates:vec!["group".into()],kind:StreamKind::Video,lane:Some(1),track:1,selected:Some(0),record_start:Some(Rational::ZERO),duration:Some(Rational::new(1,1).unwrap()),source_in:Some(Rational::ZERO),binding:SourceBinding{placements:vec![],reason:None},reason:None});
        g
    }
    #[test]
    fn multicam_round_trip_preserves_independent_audio_and_selected_gap(){
        let mut g=multicam_example();let text=g.to_json_text().unwrap();
        assert_eq!(text.len(),g.encoded_len().unwrap());assert_eq!(SourceGraph::from_json_text(&text).unwrap(),g);
        // Known source gap emits no leaf placement but remains a real occurrence.
        assert!(g.multicam_uses[0].binding.placements.is_empty());
        assert_eq!(g.multicams[0].lanes[1].alternatives.len(),1);
        g.multicams[0].lanes[0].alternatives[0].pieces.clear();
        g.multicams[0].lanes[0].alternatives[0].reason=Some("mixed clock remains native-only".into());
        g.multicam_uses[0].reason=Some("selected layout unresolved".into());
        assert_eq!(SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap(),g);
    }
    #[test]
    fn malformed_multicams_refuse_through_typed_value_and_text_apis(){
        let g=multicam_example();
        let mut bad=g.clone();bad.multicams[0].lanes[0].alternatives[0].pieces[0].item=Some("i".into());refuses_all(&bad,"gap/source fields");
        let mut bad=g.clone();bad.multicams[0].lanes[0].alternatives[0].pieces[1].start=Rational::ZERO;refuses_all(&bad,"contiguous partition");
        let mut bad=g.clone();bad.multicams[0].lanes[1].alternatives[0].pieces[1].stream=Some(99);refuses_all(&bad,"matching source stream");
        let mut bad=g.clone();bad.multicams[0].lanes[0].selected=Some(2);refuses_all(&bad,"selected alternative");
        let mut bad=g.clone();bad.multicams[0].lanes[0].alternatives[1].id="i".into();refuses_all(&bad,"duplicate");
        let mut bad=g.clone();bad.multicam_uses[0].binding.placements=vec!["missing".into()];refuses_all(&bad,"placement binding");
        let mut bad=g.clone();bad.multicam_uses[0].source_in=Some(Rational::new(4,1).unwrap());refuses_all(&bad,"source extent");
        let mut bad=g.clone();bad.multicam_uses[0].candidates.clear();refuses_all(&bad,"requires a reason");
        let mut bad=g.clone();bad.multicams[0].rate=Some(Rational::new(25,1).unwrap());refuses_all(&bad,"rate differs");
        let mut bad=g;bad.multicams[0].lanes[0].alternatives[0].pieces[1].source_in=Some(Rational::new(i64::MAX as i128,1).unwrap());refuses_all(&bad,"source range");
    }
    fn raw_json(g: &SourceGraph) -> String {
        let mut bytes = Vec::new();
        JsonOut {
            writer: &mut bytes,
            bytes: 0,
        }
        .graph(g)
        .unwrap();
        String::from_utf8(bytes).unwrap()
    }
    fn refuses_all(g: &SourceGraph, expected: &str) {
        let text = raw_json(g);
        for e in [
            g.validate(),
            SourceGraph::from_json(&json::parse(&text).unwrap()).map(|_| ()),
            SourceGraph::from_json_text(&text).map(|_| ()),
            g.to_json_text().map(|_| ()),
        ] {
            let e = e.unwrap_err().to_string();
            assert!(e.contains(expected), "{e}, expected {expected}");
        }
    }
    #[test]
    fn original_stream_clocks_channels_and_compressed_representation_round_trip(){
        let mut g=example();
        for (channel,m) in g.media.iter_mut().filter(|m|m.kind==StreamKind::Audio).enumerate(){
            m.bits=None;m.codec=Some("aac".into());m.locations=vec!["/Synthetic/source.mp4".into()];m.original=Some(OriginalStream{saved_locators:vec!["/Saved/source.mp4".into()],container:"mov".into(),stream_index:1,channel_index:Some(channel as i64),channels:Some(2),time_base:Rational::new(1,48000).unwrap(),start_pts:0,duration_ts:192000,sample_representation:Some("aac".into())});
        }
        let encoded=g.to_json_text().unwrap();assert_eq!(SourceGraph::from_json_text(&encoded).unwrap(),g);
        assert_eq!(g.media[1].stream,Some(1));assert_eq!(g.media[2].original.as_ref().unwrap().channel_index,Some(1));
        let mut bad=g.clone();bad.media[1].bits=Some(16);refuses_all(&bad,"sample representation");
        let mut bad=g.clone();bad.media[2].original.as_mut().unwrap().channel_index=Some(2);refuses_all(&bad,"physical audio channel");
        let mut bad=g;bad.media[1].original.as_mut().unwrap().duration_ts=191999;refuses_all(&bad,"physical duration");
    }
    #[test]
    fn source_round_trip_preserves_unknowns_native_owners_and_original_overlaps() {
        let mut g = example();
        g.project.name = "quotes \" \n \\ and control \u{0001} Δ".into();
        let text = g.to_json_text().unwrap();
        assert_eq!(g.encoded_len().unwrap(), text.len());
        assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
        assert_eq!(
            SourceGraph::from_json_text(&text)
                .unwrap()
                .to_json_text()
                .unwrap(),
            text
        );
        assert_eq!(g.media[0].start, None);
        assert_eq!(g.media[2].start, Some(Rational::ZERO));
        assert_eq!(g.items[0].media[1].source_offset, None);
        assert_eq!(g.items[0].media[2].source_offset, Some(Rational::ZERO));
        assert_eq!(g.sequences[0].start, None);
        assert_eq!(g.sequences[0].markers[0].start, None);
        assert_eq!(
            g.sequences[0].placements[1].duration,
            Rational::new(5, 4).unwrap()
        );
        assert_eq!(
            g.sequences[0].placements[2].record_start,
            Rational::new(3, 4).unwrap()
        );
        assert!(Graph::from_json_text(&text).is_err());
        let old=json::parse(r#"{"schema":"anynle.graph/1","project":{"name":"Legacy"},"media":[],"items":[],"bins":[],"sequences":[]}"#).unwrap();
        assert!(Graph::from_json(&old).is_ok());
        assert!(SourceGraph::from_json(&old).is_err());
    }
    #[test]
    fn unresolved_bindings_are_data_but_false_resolved_references_are_errors() {
        let mut g = example();
        g.sequences[0].transitions[0].incoming = SourceBinding {
            placements: vec![],
            reason: Some("native child expands ambiguously".into()),
        };
        g.validate().unwrap();
        g.sequences[0].transitions[0].incoming.reason = None;
        refuses_all(&g, "resolved endpoint needs");
        let mut g = example();
        g.sequences[0].transitions[0].incoming.placements = vec!["ghost".into()];
        refuses_all(&g, "unknown placement");
        let mut g = example();
        g.sequences[0].transitions[0].incoming.placements = vec!["picture".into()];
        refuses_all(&g, "different track");
        let mut g = example();
        g.sequences[0].transitions[0].incoming.placements = vec!["out".into()];
        refuses_all(&g, "endpoints overlap");
        let mut g = example();
        g.sequences[0].markers[0].binding = SourceBinding {
            placements: vec![],
            reason: Some("owner timing is unknown".into()),
        };
        g.validate().unwrap();
        g.sequences[0].markers[0].owner_local = None;
        g.validate().unwrap();
        assert_eq!(
            SourceGraph::from_json_text(&g.to_json_text().unwrap())
                .unwrap()
                .sequences[0]
                .markers[0]
                .owner_local,
            None
        );
        g.sequences[0].markers[0].binding.reason = Some(String::new());
        refuses_all(&g, "needs a reason");
    }
    #[test]
    fn known_offsets_bound_actual_selected_streams_unknown_offsets_are_not_zero() {
        let mut g = example();
        for m in &mut g.items[0].media[1..] {
            m.source_offset = None;
        }
        g.sequences[0].placements[1].source_in = Rational::integer(100);
        g.validate().unwrap();
        g.items[0].media[2].source_offset = Some(Rational::ZERO);
        refuses_all(&g, "source range exceeds selected media");
        g.items[0].media[2].source_offset = Some(Rational::integer(-100));
        g.sequences[0].placements[2].source_in = Rational::integer(102);
        g.validate().unwrap();
    }
    #[test]
    fn schema_and_native_payload_shadowing_cannot_bypass_value_entrypoint() {
        let g = example();
        let mut v = json::parse(&g.to_json_text().unwrap()).unwrap();
        if let Value::Obj(p) = &mut v {
            p.push(("schema".into(), Value::Str(SCHEMA.into())));
        }
        assert!(SourceGraph::from_json(&v)
            .unwrap_err()
            .to_string()
            .contains("repeated JSON key"));
        let mut g = example();
        g.sidecars.push(g.sidecars[0].clone());
        refuses_all(&g, "duplicate adapter/object");
        let mut g = example();
        g.sidecars[0].data = Value::Float(f64::NAN);
        assert!(g.validate().is_err());
        assert!(g.to_json_text().is_err());
    }
    #[test]
    fn source_lanes_are_independent_and_native_grouped_channels_stay_atomic() {
        let mut g = example();
        g.sequences[0].placements[0].audio_tracks = vec![1];
        refuses_all(&g, "exactly one lane");
        let mut g = example();
        g.sequences[0].placements[1].grouped_audio[0].streams = vec![1, 2];
        g.validate().unwrap();
        g.sequences[0].placements[1].audio_tracks = vec![1];
        refuses_all(&g, "exactly one lane");
    }
    #[test]
    fn resolved_single_pair_covers_the_whole_transition_interval() {
        let g = example();
        g.validate().unwrap(); // Both original edits cover .75..1.25.
        let mut bad = g.clone();
        bad.sequences[0].transitions[0].cut = Rational::integer(5);
        refuses_all(&bad, "does not cover the transition interval");
        let mut bad = g.clone();
        bad.sequences[0].placements[1].duration = Rational::integer(1);
        refuses_all(&bad, "does not cover the transition interval");
        let mut bad = g.clone();
        bad.sequences[0].placements[2].record_start = Rational::integer(1);
        refuses_all(&bad, "does not cover the transition interval");
        // A candidate interval is not claimed qualified by an unresolved binding.
        let mut unresolved = bad.clone();
        unresolved.sequences[0].transitions[0].incoming.reason =
            Some("native expansion needs qualification".into());
        unresolved.validate().unwrap();
        // Structurally resolved multi-piece relationships remain source facts;
        // destination preparation is responsible for qualifying their coverage.
        let mut multi = bad;
        let mut second = multi.sequences[0].placements[1].clone();
        second.id = "out-second".into();
        multi.sequences[0].placements.push(second);
        multi.sequences[0].transitions[0]
            .outgoing
            .placements
            .push("out-second".into());
        multi.validate().unwrap();
        let mut overflow = g.clone();
        overflow.sequences[0].transitions[0].cut = Rational::integer(i64::MAX);
        refuses_all(&overflow, "transition time overflow");
    }
    #[test]
    fn invalid_graphs_and_over_budget_encodings_write_no_caller_bytes() {
        let mut g = example();
        g.sequences[0].rate = Rational::ZERO;
        let mut bytes = Vec::new();
        assert!(g.write_json(&mut bytes).is_err());
        assert!(bytes.is_empty());
        let mut g = example();
        g.sidecars[0].data = Value::Str("x".repeat(MAX_GRAPH_BYTES));
        assert!(g
            .write_json(&mut bytes)
            .unwrap_err()
            .to_string()
            .contains("exceeds 64 MiB"));
        assert!(bytes.is_empty());
    }
    #[test]
    fn streaming_writer_propagates_io_failures_without_panicking() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("controlled failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let e = example().write_json(&mut Broken).unwrap_err();
        assert!(e.to_string().contains("controlled failure"));
    }
    fn clock_example() -> SourceClock {
        SourceClock {
            effective_count: 240,
            rate: Rational::integer(24),
            display_base: 24,
            drop: false,
            provenance: SourceClockProvenance {
                origin_count: 238,
                source_offset: 4000,
                source_rate: Rational::integer(48000),
                source_package: "physical-package".into(),
                timecode_track: "timecode-track".into(),
                route: "file-source-clip".into(),
            },
        }
    }
    #[test]
    fn typed_clocks_round_trip_and_unknown_starts_remain_unknown() {
        let mut g = example();
        let c = clock_example();
        g.media[0].start = c.exact_start();
        g.media[0].clock = Some(c.clone());
        g.sequences[0].start = c.exact_start();
        g.sequences[0].clock = Some(c);
        let text = g.to_json_text().unwrap();
        assert!(text.contains("\"effective_count\":\"240\""));
        assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
        let unknown = example();
        assert!(unknown.media[0].clock.is_none());
        assert!(unknown.media[0].start.is_none());
        assert!(unknown.media[2].clock.is_none());
        assert_eq!(unknown.media[2].start, Some(Rational::ZERO));
        // Old, pre-integration source audits omit clock and remain readable as unknown.
        let old = unknown
            .to_json_text()
            .unwrap()
            .replace("\"clock\":null,", "");
        assert_eq!(SourceGraph::from_json_text(&old).unwrap(), unknown);
    }
    #[test]
    fn clock_contradictions_refuse_through_typed_value_and_text_apis() {
        let mut g = example();
        let c = clock_example();
        g.media[0].start = c.exact_start();
        g.media[0].clock = Some(c);
        let mut bad = g.clone();
        bad.media[0].start = Some(Rational::ZERO);
        refuses_all(&bad, "start contradicts");
        let mut bad = g.clone();
        bad.media[0].start = None;
        refuses_all(&bad, "start contradicts");
        let mut bad = g.clone();
        bad.media[0].clock.as_mut().unwrap().effective_count += 1;
        refuses_all(&bad, "count contradicts");
        let mut bad = g.clone();
        bad.media[0]
            .clock
            .as_mut()
            .unwrap()
            .provenance
            .source_offset = 1;
        refuses_all(&bad, "whole timecode count");
        let mut bad = g.clone();
        bad.media[0].clock.as_mut().unwrap().display_base = 0;
        refuses_all(&bad, "base must be positive");
        let mut bad = g.clone();
        bad.media[0].clock.as_mut().unwrap().rate = Rational::ZERO;
        refuses_all(&bad, "rate must be positive");
        let mut bad = g.clone();
        bad.media[0]
            .clock
            .as_mut()
            .unwrap()
            .provenance
            .route
            .clear();
        refuses_all(&bad, "provenance must be nonempty");
        let mut bad = g;
        bad.media[0]
            .clock
            .as_mut()
            .unwrap()
            .provenance
            .source_package = "x".repeat(32769);
        refuses_all(&bad, "text");
    }
    #[test]
    fn u64_counts_and_cancelled_offset_products_are_exact_without_panics() {
        let mut g = example();
        let mut c = clock_example();
        c.effective_count = u64::MAX;
        c.rate = Rational::integer(3);
        c.provenance.origin_count = u64::MAX;
        c.provenance.source_offset = 0;
        c.provenance.source_rate = Rational::integer(3);
        g.media[0].start = c.exact_start();
        g.media[0].clock = Some(c.clone());
        assert_eq!(
            g.media[0].start,
            Some(Rational::integer(6148914691236517205))
        );
        let text = g.to_json_text().unwrap();
        assert!(text.contains("18446744073709551615"));
        assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
        c.rate = Rational::new(i64::MAX as i128, i64::MAX as i128 - 1).unwrap();
        c.provenance.source_rate = c.rate;
        c.provenance.origin_count = 0;
        c.provenance.source_offset = i64::MAX;
        c.effective_count = i64::MAX as u64;
        g.media[0].start = c.exact_start();
        g.media[0].clock = Some(c.clone());
        assert_eq!(g.media[0].start, Some(Rational::integer(i64::MAX - 1)));
        g.validate().unwrap();
        c.rate = Rational::integer(1);
        c.provenance.source_rate = Rational::integer(1);
        c.provenance.origin_count = u64::MAX;
        c.provenance.source_offset = i64::MIN;
        c.effective_count = i64::MAX as u64;
        g.media[0].start = c.exact_start();
        g.media[0].clock = Some(c);
        g.validate().unwrap();
        g.media[0].clock.as_mut().unwrap().provenance.source_offset = 1;
        refuses_all(&g, "count contradicts");
        let mut c = clock_example();
        c.effective_count = u64::MAX;
        c.provenance.origin_count = u64::MAX;
        c.provenance.source_offset = 0;
        c.rate = Rational::integer(1);
        g.media[0].clock = Some(c);
        refuses_all(&g, "not representable");
    }
    #[test]
    fn clock_wire_counts_require_canonical_u64_strings() {
        let mut g = example();
        let c = clock_example();
        g.media[0].start = c.exact_start();
        g.media[0].clock = Some(c);
        let original = g.to_json_text().unwrap();
        for bad in [
            "\"01\"",
            "\"-1\"",
            "\"+1\"",
            "\"1.0\"",
            "\"18446744073709551616\"",
            "240",
        ] {
            let text = original.replacen(
                "\"effective_count\":\"240\"",
                &format!("\"effective_count\":{bad}"),
                1,
            );
            assert!(SourceGraph::from_json_text(&text).is_err(), "{bad}");
            assert!(
                SourceGraph::from_json(&json::parse(&text).unwrap()).is_err(),
                "{bad}"
            );
        }
    }
    #[test]
    fn unknown_sequence_canvas_round_trips_without_inventing_a_default() {
        let mut g = example();
        g.sequences[0].frame_size = None;
        let text = g.to_json_text().unwrap();
        assert!(text.contains("\"frame_size\":null"));
        assert_eq!(SourceGraph::from_json_text(&text).unwrap(), g);
        assert_eq!(
            SourceGraph::from_json(&json::parse(&text).unwrap()).unwrap(),
            g
        );
        let missing = text.replace("\"frame_size\":null,", "");
        assert_eq!(SourceGraph::from_json_text(&missing).unwrap(), g);
        g.sequences[0].frame_size = Some((2048, 1080));
        assert_eq!(
            SourceGraph::from_json_text(&g.to_json_text().unwrap()).unwrap(),
            g
        );
        g.sequences[0].frame_size = Some((0, 1080));
        let text = raw_json(&g);
        assert!(g.validate().is_err());
        assert!(g.to_json_text().is_err());
        assert!(SourceGraph::from_json_text(&text).is_err());
        assert!(SourceGraph::from_json(&json::parse(&text).unwrap()).is_err());
    }
}
