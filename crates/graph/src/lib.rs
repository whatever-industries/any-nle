//! `anynle.graph/1`: the program-neutral edit graph (docs/GRAPH-MODEL.md).
//!
//! Every program gets one reader into this graph and one writer out of it. The concepts follow
//! the Codex/Claude working contract (docs/AVB-PORT-COORDINATION.md, "For additional NLE pairs"):
//! stable identity, rational time, bin hierarchy, media facts kept apart, placements, typed
//! properties, and losses with the object they belong to. Program-native data that has no
//! portable meaning yet lives in [`Sidecar`]s owned by one adapter; choices that depend on the
//! source/target pair (bin mode, colour tables, marker conflicts) are not in the graph at all.
//!
//! Loading is strict like the converter's model: unknown keys, wrong types, dangling references
//! and duplicate IDs are errors.

pub mod json;
pub mod picture_chain;
pub mod time;
pub mod source;
mod validation;

use std::collections::{HashMap, HashSet};
use std::fmt;

pub use json::Value;
pub use time::Rational;

pub const SCHEMA: &str = "anynle.graph/1";
pub const MAX_GRAPH_BYTES: usize = 64 << 20;
/// Longest list accepted anywhere in graph JSON (model/1 caps clips and events at 100,000 and
/// frame maps at 100,000; this leaves room for whole projects).
pub const MAX_LIST: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct GraphError(pub String);

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GraphError {}

pub type R<T> = Result<T, GraphError>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(GraphError(msg.into()))
}

// --- the graph ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamKind {
    Video,
    Audio,
}

impl StreamKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::Video => "video",
            StreamKind::Audio => "audio",
        }
    }

    fn parse(s: &str) -> Option<StreamKind> {
        match s {
            "video" => Some(StreamKind::Video),
            "audio" => Some(StreamKind::Audio),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub project: Project,
    pub media: Vec<Media>,
    pub items: Vec<Item>,
    pub bins: Vec<Bin>,
    pub sequences: Vec<Sequence>,
    pub losses: Vec<Loss>,
    pub sidecars: Vec<Sidecar>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
}

/// One stream of source media. Identity (`id`), where it may be found (`locations`), what it is
/// (codec, size, rates) and whether it is online are separate facts.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub id: String,
    pub kind: StreamKind,
    /// Known file locations, the first preferred.
    pub locations: Vec<String>,
    pub codec: Option<String>,
    pub frame_size: Option<(i64, i64)>,
    /// Picture rate (video).
    pub rate: Option<Rational>,
    pub sample_rate: Option<i64>,
    pub bits: Option<i64>,
    pub duration: Rational,
    /// The media's own start timecode, as a time.
    pub start: Rational,
    /// The source track this stream carries (1-based), when known.
    pub stream: Option<i64>,
    /// `None` = not checked.
    pub online: Option<bool>,
}

/// A media stream as one item uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemMedia {
    pub media: String,
    /// The item's own gain on this stream (audio), dB.
    pub gain_db: f64,
}

/// A bin item: today a master clip over one or more media streams.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub media: Vec<ItemMedia>,
    /// SMPTE material UMID, 64 upper-case hex digits.
    pub umid: Option<String>,
    pub tape: Option<String>,
    pub rate: Option<Rational>,
    /// Label colour as 16-bit RGB.
    pub color_rgb16: Option<[i64; 3]>,
    /// Audio streams that play as one multichannel group (e.g. a stereo pair), by `stream`.
    pub stream_groups: Vec<Vec<i64>>,
    /// Item markers; times are from the item's start.
    pub markers: Vec<Marker>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bin {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    /// Item IDs, in bin order. An item may sit in several bins.
    pub items: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub id: String,
    pub name: String,
    pub rate: Rational,
    pub frame_size: (i64, i64),
    /// Start timecode, as a time.
    pub start: Rational,
    pub tracks: Vec<Track>,
    pub placements: Vec<Placement>,
    pub transitions: Vec<Transition>,
    pub markers: Vec<Marker>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub kind: StreamKind,
    pub index: i64,
    pub level_db: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackRef {
    pub kind: StreamKind,
    pub index: i64,
}

/// One edit: an item's source range at a record position on one picture track and/or audio
/// tracks. Times are sequence times except `source_in` and volume keys (item source time).
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: String,
    pub item: String,
    pub record_start: Rational,
    pub source_in: Rational,
    pub duration: Rational,
    pub video_track: Option<i64>,
    pub audio_tracks: Vec<i64>,
    /// The item stream playing on each audio track; empty = the item's audio in order.
    pub audio_streams: Vec<i64>,
    /// Multichannel edits: one sequence track playing several item streams.
    pub grouped_audio: Vec<GroupedAudio>,
    pub gain_db: f64,
    pub volume_db: Option<f64>,
    pub volume_keys: Vec<VolumeKey>,
    /// Per audio track; `None` = not set. Empty = none set.
    pub track_gains_db: Vec<Option<f64>>,
    /// Per audio track, percent -100 (left) .. 100 (right).
    pub track_pans: Vec<Option<f64>>,
    pub speed: Option<Speed>,
    /// The source frame (item picture frames from the media start) shown at each record frame.
    pub frame_map: Option<Vec<i64>>,
    pub transform: Option<Transform>,
    /// Effects without a neutral form yet, named `<program>.<effect>` (docs/GRAPH-MODEL.md).
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GroupedAudio {
    pub track: i64,
    pub streams: Vec<i64>,
    pub gain_db: Option<f64>,
    pub pans: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VolumeKey {
    pub at: Rational,
    pub db: f64,
}

/// Constant speed: `ratio` = record frames / source frames (negative = reverse) over
/// `source_duration` of source.
#[derive(Clone, Debug, PartialEq)]
pub struct Speed {
    pub ratio: (i64, i64),
    pub source_duration: Rational,
}

/// Picture position, scale, rotation and crop. Fractions of the frame from its centre, x right,
/// y down; scale in percent; rotation in degrees clockwise; crop left, top, right, bottom of the
/// source.
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    pub position: (f64, f64),
    pub scale: (f64, f64),
    pub rotation: f64,
    pub anchor: (f64, f64),
    pub crop: [f64; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub kind: String,
    pub params: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerOwner {
    /// The sequence's (or item's) own marker.
    Sequence,
    /// On filler (an empty stretch of a track).
    Filler,
    /// On an edit (`placement`).
    Edit,
}

impl MarkerOwner {
    pub fn as_str(self) -> &'static str {
        match self {
            MarkerOwner::Sequence => "sequence",
            MarkerOwner::Filler => "filler",
            MarkerOwner::Edit => "edit",
        }
    }

    fn parse(s: &str) -> Option<MarkerOwner> {
        match s {
            "sequence" => Some(MarkerOwner::Sequence),
            "filler" => Some(MarkerOwner::Filler),
            "edit" => Some(MarkerOwner::Edit),
            _ => None,
        }
    }
}

/// A colour named in one program's palette (`palette` = that program, e.g. `avid`). Mapping it to
/// another palette is a pair choice, made outside the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub palette: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub name: String,
    pub comment: String,
    pub start: Rational,
    pub duration: Rational,
    pub user: String,
    pub color: Option<Label>,
    pub track: Option<TrackRef>,
    pub owner: MarkerOwner,
    pub placement: Option<String>,
    /// Item source time of an edit marker.
    pub source: Option<Rational>,
    /// A copy of a marker the item itself carries.
    pub inherited: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub kind: StreamKind,
    pub track: i64,
    /// Sequence time of the cut it sits on.
    pub cut: Rational,
    pub duration: Rational,
    /// From the transition's start to the cut.
    pub cut_offset: Rational,
    /// e.g. `constant_power`, `custom_fade`.
    pub style: String,
}

/// Something a reader or writer did not convert, or only approximated: on which object, which
/// part of it (`aspect`, e.g. `transform`), and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Loss {
    pub object: String,
    pub aspect: String,
    pub reason: String,
}

/// Program-native data with no portable meaning, owned by one adapter and keyed by object.
#[derive(Clone, Debug, PartialEq)]
pub struct Sidecar {
    pub adapter: String,
    pub object: String,
    pub data: Value,
}

impl Graph {
    pub fn sidecar(&self, adapter: &str, object: &str) -> Option<&Value> {
        self.sidecars.iter().find(|s| s.adapter == adapter && s.object == object).map(|s| &s.data)
    }

    /// Losses on `object` and `aspect`, in order.
    pub fn losses_for<'a>(&'a self, object: &'a str, aspect: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.losses.iter().filter(move |l| l.object == object && l.aspect == aspect).map(|l| l.reason.as_str())
    }

    /// Check identity and references (also run by [`Graph::from_json`]).
    pub fn validate(&self) -> R<()> {
        let mut ids: HashSet<&str> = HashSet::new();
        let mut all: Vec<(&str, &str)> = Vec::new();
        all.extend(self.media.iter().map(|m| (m.id.as_str(), "media")));
        all.extend(self.items.iter().map(|i| (i.id.as_str(), "item")));
        all.extend(self.bins.iter().map(|b| (b.id.as_str(), "bin")));
        for s in &self.sequences {
            all.push((s.id.as_str(), "sequence"));
            all.extend(s.placements.iter().map(|p| (p.id.as_str(), "placement")));
        }
        for (id, what) in &all {
            if id.is_empty() {
                return fail(format!("{what}: empty id"));
            }
            if !ids.insert(id) {
                return fail(format!("{what} {id:?}: duplicate id"));
            }
        }
        let kinds: HashMap<&str, &str> = all.iter().copied().collect();
        let media: HashMap<&str, &Media> = self.media.iter().map(|m| (m.id.as_str(), m)).collect();
        for m in &self.media {
            if m.duration.is_negative() || m.start.is_negative() {
                return fail(format!("media {:?}: negative time", m.id));
            }
        }
        for item in &self.items {
            let w = format!("item {:?}", item.id);
            if item.media.is_empty() {
                return fail(format!("{w}: needs at least one media stream"));
            }
            for im in &item.media {
                if !media.contains_key(im.media.as_str()) {
                    return fail(format!("{w}: unknown media {:?}", im.media));
                }
            }
            let streams: HashSet<i64> =
                item.media.iter().filter_map(|im| media[im.media.as_str()].stream.filter(|_| media[im.media.as_str()].kind == StreamKind::Audio)).collect();
            if item.stream_groups.iter().flatten().any(|s| !streams.contains(s)) {
                return fail(format!("{w}: a stream group names a stream the item lacks"));
            }
            for m in &item.markers {
                check_marker(m, &w, None)?;
            }
        }
        let mut seen_bins: HashSet<&str> = HashSet::new();
        for b in &self.bins {
            if let Some(p) = &b.parent {
                if !seen_bins.contains(p.as_str()) {
                    return fail(format!("bin {:?}: parent {p:?} must be an earlier bin", b.id));
                }
            }
            seen_bins.insert(&b.id);
            for i in &b.items {
                if kinds.get(i.as_str()) != Some(&"item") {
                    return fail(format!("bin {:?}: unknown item {i:?}", b.id));
                }
            }
        }
        for s in &self.sequences {
            let w = format!("sequence {:?}", s.id);
            let placements: HashSet<&str> = s.placements.iter().map(|p| p.id.as_str()).collect();
            for p in &s.placements {
                let pw = format!("{w} placement {:?}", p.id);
                if kinds.get(p.item.as_str()) != Some(&"item") {
                    return fail(format!("{pw}: unknown item {:?}", p.item));
                }
                if p.record_start.is_negative() || p.source_in.is_negative() || p.duration.num() <= 0 {
                    return fail(format!("{pw}: times out of range"));
                }
                if p.video_track.is_none() && p.audio_tracks.is_empty() && p.grouped_audio.is_empty() {
                    return fail(format!("{pw}: needs a picture or audio track"));
                }
                if p.effects.iter().any(|e| !e.kind.contains('.')) {
                    return fail(format!("{pw}: effect kinds are <program>.<effect>"));
                }
            }
            for m in &s.markers {
                check_marker(m, &w, Some(&placements))?;
            }
        }
        for l in &self.losses {
            if !kinds.contains_key(l.object.as_str()) || l.aspect.is_empty() {
                return fail(format!("loss on unknown object {:?}", l.object));
            }
        }
        for sc in &self.sidecars {
            if sc.adapter.is_empty() || !kinds.contains_key(sc.object.as_str()) {
                return fail(format!("sidecar for unknown object {:?}", sc.object));
            }
        }
        validation::check(self)
    }
}

fn check_marker(m: &Marker, w: &str, placements: Option<&HashSet<&str>>) -> R<()> {
    if m.start.is_negative() || m.duration.is_negative() {
        return fail(format!("{w} marker {:?}: negative time", m.name));
    }
    match (placements, &m.placement) {
        (None, Some(_)) => fail(format!("{w} marker {:?}: item markers have no placement", m.name)),
        (Some(ps), Some(p)) if !ps.contains(p.as_str()) => fail(format!("{w} marker {:?}: unknown placement {p:?}", m.name)),
        _ if m.owner == MarkerOwner::Edit && (m.placement.is_none() || m.source.is_none() || m.track.is_none()) => {
            fail(format!("{w} marker {:?}: an edit marker needs placement, source and track", m.name))
        }
        _ => Ok(()),
    }
}

// --- JSON ---------------------------------------------------------------------------------------

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

fn t(r: Rational) -> Value {
    Value::Str(r.to_string())
}

fn f(x: f64) -> Value {
    Value::Float(x)
}

fn opt<T>(o: &Option<T>, conv: impl Fn(&T) -> Value) -> Value {
    o.as_ref().map_or(Value::Null, conv)
}

fn arr<T>(v: &[T], conv: impl Fn(&T) -> Value) -> Value {
    Value::Arr(v.iter().map(conv).collect())
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn pair_f(p: (f64, f64)) -> Value {
    Value::Arr(vec![f(p.0), f(p.1)])
}

fn marker_json(m: &Marker) -> Value {
    obj(vec![
        ("name", s(&m.name)),
        ("comment", s(&m.comment)),
        ("start", t(m.start)),
        ("duration", t(m.duration)),
        ("user", s(&m.user)),
        ("color", opt(&m.color, |l| obj(vec![("palette", s(&l.palette)), ("name", s(&l.name))]))),
        ("track", opt(&m.track, |r| obj(vec![("kind", s(r.kind.as_str())), ("index", Value::Int(r.index))]))),
        ("owner", s(m.owner.as_str())),
        ("placement", opt(&m.placement, |p| s(p))),
        ("source", opt(&m.source, |r| t(*r))),
        ("inherited", Value::Bool(m.inherited)),
    ])
}

impl Graph {
    pub fn to_json(&self) -> Value {
        let media = arr(&self.media, |m| {
            obj(vec![
                ("id", s(&m.id)),
                ("kind", s(m.kind.as_str())),
                ("locations", arr(&m.locations, |l| s(l))),
                ("codec", opt(&m.codec, |c| s(c))),
                ("frame_size", opt(&m.frame_size, |(w, h)| Value::Arr(vec![Value::Int(*w), Value::Int(*h)]))),
                ("rate", opt(&m.rate, |r| t(*r))),
                ("sample_rate", opt(&m.sample_rate, |r| Value::Int(*r))),
                ("bits", opt(&m.bits, |b| Value::Int(*b))),
                ("duration", t(m.duration)),
                ("start", t(m.start)),
                ("stream", opt(&m.stream, |n| Value::Int(*n))),
                ("online", opt(&m.online, |b| Value::Bool(*b))),
            ])
        });
        let items = arr(&self.items, |i| {
            obj(vec![
                ("id", s(&i.id)),
                ("name", s(&i.name)),
                ("media", arr(&i.media, |im| obj(vec![("media", s(&im.media)), ("gain_db", f(im.gain_db))]))),
                ("umid", opt(&i.umid, |u| s(u))),
                ("tape", opt(&i.tape, |u| s(u))),
                ("rate", opt(&i.rate, |r| t(*r))),
                ("color_rgb16", opt(&i.color_rgb16, |c| arr(c, |n| Value::Int(*n)))),
                ("stream_groups", arr(&i.stream_groups, |g| arr(g, |n| Value::Int(*n)))),
                ("markers", arr(&i.markers, marker_json)),
            ])
        });
        let bins = arr(&self.bins, |b| {
            obj(vec![("id", s(&b.id)), ("name", s(&b.name)), ("parent", opt(&b.parent, |p| s(p))), ("items", arr(&b.items, |i| s(i)))])
        });
        let sequences = arr(&self.sequences, |q| {
            obj(vec![
                ("id", s(&q.id)),
                ("name", s(&q.name)),
                ("rate", t(q.rate)),
                ("frame_size", Value::Arr(vec![Value::Int(q.frame_size.0), Value::Int(q.frame_size.1)])),
                ("start", t(q.start)),
                (
                    "tracks",
                    arr(&q.tracks, |tr| {
                        obj(vec![("kind", s(tr.kind.as_str())), ("index", Value::Int(tr.index)), ("level_db", opt(&tr.level_db, |d| f(*d)))])
                    }),
                ),
                ("placements", arr(&q.placements, placement_json)),
                (
                    "transitions",
                    arr(&q.transitions, |x| {
                        obj(vec![
                            ("kind", s(x.kind.as_str())),
                            ("track", Value::Int(x.track)),
                            ("cut", t(x.cut)),
                            ("duration", t(x.duration)),
                            ("cut_offset", t(x.cut_offset)),
                            ("style", s(&x.style)),
                        ])
                    }),
                ),
                ("markers", arr(&q.markers, marker_json)),
            ])
        });
        obj(vec![
            ("schema", s(SCHEMA)),
            ("project", obj(vec![("name", s(&self.project.name))])),
            ("media", media),
            ("items", items),
            ("bins", bins),
            ("sequences", sequences),
            ("losses", arr(&self.losses, |l| obj(vec![("object", s(&l.object)), ("aspect", s(&l.aspect)), ("reason", s(&l.reason))]))),
            ("sidecars", arr(&self.sidecars, |c| obj(vec![("adapter", s(&c.adapter)), ("object", s(&c.object)), ("data", c.data.clone())]))),
        ])
    }

    /// The graph as JSON text. Refuses a non-finite number (NaN or infinity), which JSON
    /// cannot hold and [`Graph::from_json_text`] would not read back.
    pub fn to_json_text(&self) -> R<String> {
        self.validate()?;
        let v = self.to_json();
        fn finite(v: &Value) -> bool {
            match v {
                Value::Float(x) => x.is_finite(),
                Value::Arr(a) => a.iter().all(finite),
                Value::Obj(p) => p.iter().all(|(_, x)| finite(x)),
                _ => true,
            }
        }
        if !finite(&v) {
            return fail("graph: a number is NaN or infinite");
        }
        Ok(json::write(&v))
    }

    pub fn from_json_text(text: &str) -> R<Graph> {
        if text.len() > MAX_GRAPH_BYTES {
            return fail("graph: file too large");
        }
        Graph::from_json(&json::parse(text).map_err(GraphError)?)
    }

    pub fn from_json(v: &Value) -> R<Graph> {
        let o = Obj::new(v, "graph", &["schema", "project", "media", "items", "bins", "sequences"], &["losses", "sidecars"])?;
        if o.str("schema")? != SCHEMA {
            return fail(format!("graph.schema: expected {SCHEMA}"));
        }
        let p = Obj::new(o.req("project"), "graph.project", &["name"], &[])?;
        let mut media = Vec::new();
        for (i, m) in o.list("media")?.iter().enumerate() {
            let m = Obj::new(
                m,
                &format!("graph.media[{i}]"),
                &["id", "kind", "locations", "duration", "start"],
                &["codec", "frame_size", "rate", "sample_rate", "bits", "stream", "online"],
            )?;
            media.push(Media {
                id: m.str("id")?,
                kind: m.kind("kind")?,
                locations: m.list("locations")?.iter().map(|l| text(l, &m.at("locations"))).collect::<R<_>>()?,
                codec: m.opt_str("codec")?,
                frame_size: m.opt_size("frame_size")?,
                rate: m.opt_time("rate")?,
                sample_rate: m.opt_int("sample_rate")?,
                bits: m.opt_int("bits")?,
                duration: m.time("duration")?,
                start: m.time("start")?,
                stream: m.opt_int("stream")?,
                online: m.opt_bool("online")?,
            });
        }
        let mut items = Vec::new();
        for (i, it) in o.list("items")?.iter().enumerate() {
            let w = format!("graph.items[{i}]");
            let it = Obj::new(it, &w, &["id", "name", "media"], &["umid", "tape", "rate", "color_rgb16", "stream_groups", "markers"])?;
            let mut im = Vec::new();
            for (j, x) in it.list("media")?.iter().enumerate() {
                let x = Obj::new(x, &format!("{w}.media[{j}]"), &["media"], &["gain_db"])?;
                im.push(ItemMedia { media: x.str("media")?, gain_db: x.opt_float("gain_db")?.unwrap_or(0.0) });
            }
            let color_rgb16 = match it.opt("color_rgb16") {
                None => None,
                Some(c) => {
                    let v = ints(c, &it.at("color_rgb16"))?;
                    if v.len() != 3 || v.iter().any(|n| !(0..=65535).contains(n)) {
                        return fail(format!("{w}.color_rgb16: expected [r, g, b] 0..65535"));
                    }
                    Some([v[0], v[1], v[2]])
                }
            };
            let umid = it.opt_str("umid")?;
            if umid.as_ref().is_some_and(|u| u.len() != 64 || !u.bytes().all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c))) {
                return fail(format!("{w}.umid: expected 64 upper-case hex digits"));
            }
            items.push(Item {
                id: it.str("id")?,
                name: it.str("name")?,
                media: im,
                umid,
                tape: it.opt_str("tape")?,
                rate: it.opt_time("rate")?,
                color_rgb16,
                stream_groups: it.opt_list("stream_groups")?.iter().map(|g| ints(g, &it.at("stream_groups"))).collect::<R<_>>()?,
                markers: markers(&it, "markers")?,
            });
        }
        let mut bins = Vec::new();
        for (i, b) in o.list("bins")?.iter().enumerate() {
            let b = Obj::new(b, &format!("graph.bins[{i}]"), &["id", "name", "items"], &["parent"])?;
            bins.push(Bin {
                id: b.str("id")?,
                name: b.str("name")?,
                parent: b.opt_str("parent")?,
                items: b.list("items")?.iter().map(|x| text(x, &b.at("items"))).collect::<R<_>>()?,
            });
        }
        let mut sequences = Vec::new();
        for (i, q) in o.list("sequences")?.iter().enumerate() {
            let w = format!("graph.sequences[{i}]");
            let q = Obj::new(q, &w, &["id", "name", "rate", "frame_size", "placements"], &["start", "tracks", "transitions", "markers"])?;
            let mut tracks = Vec::new();
            for (j, tr) in q.opt_list("tracks")?.iter().enumerate() {
                let tr = Obj::new(tr, &format!("{w}.tracks[{j}]"), &["kind", "index"], &["level_db"])?;
                tracks.push(Track { kind: tr.kind("kind")?, index: tr.int("index")?, level_db: tr.opt_float("level_db")? });
            }
            let mut placements = Vec::new();
            for (j, p) in q.list("placements")?.iter().enumerate() {
                placements.push(placement(p, &format!("{w}.placements[{j}]"))?);
            }
            let mut transitions = Vec::new();
            for (j, x) in q.opt_list("transitions")?.iter().enumerate() {
                let x = Obj::new(x, &format!("{w}.transitions[{j}]"), &["kind", "track", "cut", "duration", "cut_offset", "style"], &[])?;
                transitions.push(Transition {
                    kind: x.kind("kind")?,
                    track: x.int("track")?,
                    cut: x.time("cut")?,
                    duration: x.time("duration")?,
                    cut_offset: x.time("cut_offset")?,
                    style: x.str("style")?,
                });
            }
            sequences.push(Sequence {
                id: q.str("id")?,
                name: q.str("name")?,
                rate: q.time("rate")?,
                frame_size: q.opt_size("frame_size")?.expect("required"),
                start: q.opt_time("start")?.unwrap_or(Rational::ZERO),
                tracks,
                placements,
                transitions,
                markers: markers(&q, "markers")?,
            });
        }
        let mut losses = Vec::new();
        for (i, l) in o.opt_list("losses")?.iter().enumerate() {
            let l = Obj::new(l, &format!("graph.losses[{i}]"), &["object", "aspect", "reason"], &[])?;
            losses.push(Loss { object: l.str("object")?, aspect: l.str("aspect")?, reason: l.str("reason")? });
        }
        let mut sidecars = Vec::new();
        for (i, c) in o.opt_list("sidecars")?.iter().enumerate() {
            let c = Obj::new(c, &format!("graph.sidecars[{i}]"), &["adapter", "object"], &["data"])?;
            sidecars.push(Sidecar { adapter: c.str("adapter")?, object: c.str("object")?, data: c.raw("data")?.clone() });
        }
        let g = Graph { project: Project { name: p.str("name")? }, media, items, bins, sequences, losses, sidecars };
        g.validate()?;
        Ok(g)
    }
}

fn placement_json(p: &Placement) -> Value {
    obj(vec![
        ("id", s(&p.id)),
        ("item", s(&p.item)),
        ("record_start", t(p.record_start)),
        ("source_in", t(p.source_in)),
        ("duration", t(p.duration)),
        ("video_track", opt(&p.video_track, |n| Value::Int(*n))),
        ("audio_tracks", arr(&p.audio_tracks, |n| Value::Int(*n))),
        ("audio_streams", arr(&p.audio_streams, |n| Value::Int(*n))),
        (
            "grouped_audio",
            arr(&p.grouped_audio, |g| {
                obj(vec![
                    ("track", Value::Int(g.track)),
                    ("streams", arr(&g.streams, |n| Value::Int(*n))),
                    ("gain_db", opt(&g.gain_db, |d| f(*d))),
                    ("pans", arr(&g.pans, |x| opt(x, |d| f(*d)))),
                ])
            }),
        ),
        ("gain_db", f(p.gain_db)),
        ("volume_db", opt(&p.volume_db, |d| f(*d))),
        ("volume_keys", arr(&p.volume_keys, |k| obj(vec![("at", t(k.at)), ("db", f(k.db))]))),
        ("track_gains_db", arr(&p.track_gains_db, |x| opt(x, |d| f(*d)))),
        ("track_pans", arr(&p.track_pans, |x| opt(x, |d| f(*d)))),
        (
            "speed",
            opt(&p.speed, |sp| {
                obj(vec![("ratio", Value::Arr(vec![Value::Int(sp.ratio.0), Value::Int(sp.ratio.1)])), ("source_duration", t(sp.source_duration))])
            }),
        ),
        ("frame_map", opt(&p.frame_map, |v| arr(v, |n| Value::Int(*n)))),
        (
            "transform",
            opt(&p.transform, |x| {
                obj(vec![
                    ("position", pair_f(x.position)),
                    ("scale", pair_f(x.scale)),
                    ("rotation", f(x.rotation)),
                    ("anchor", pair_f(x.anchor)),
                    ("crop", arr(&x.crop, |c| f(*c))),
                ])
            }),
        ),
        ("effects", arr(&p.effects, |e| obj(vec![("kind", s(&e.kind)), ("params", e.params.clone())]))),
    ])
}

fn placement(v: &Value, w: &str) -> R<Placement> {
    let p = Obj::new(
        v,
        w,
        &["id", "item", "record_start", "source_in", "duration"],
        &[
            "video_track", "audio_tracks", "audio_streams", "grouped_audio", "gain_db", "volume_db", "volume_keys", "track_gains_db",
            "track_pans", "speed", "frame_map", "transform", "effects",
        ],
    )?;
    let mut grouped_audio = Vec::new();
    for (j, g) in p.opt_list("grouped_audio")?.iter().enumerate() {
        let g = Obj::new(g, &format!("{w}.grouped_audio[{j}]"), &["track", "streams"], &["gain_db", "pans"])?;
        grouped_audio.push(GroupedAudio {
            track: g.int("track")?,
            streams: ints(g.req("streams"), &g.at("streams"))?,
            gain_db: g.opt_float("gain_db")?,
            pans: opt_floats(g.opt("pans"), &g.at("pans"))?,
        });
    }
    let mut volume_keys = Vec::new();
    for (j, k) in p.opt_list("volume_keys")?.iter().enumerate() {
        let k = Obj::new(k, &format!("{w}.volume_keys[{j}]"), &["at", "db"], &[])?;
        volume_keys.push(VolumeKey { at: k.time("at")?, db: k.float("db")? });
    }
    let speed = match p.opt("speed") {
        None => None,
        Some(sp) => {
            let sp = Obj::new(sp, &p.at("speed"), &["ratio", "source_duration"], &[])?;
            let r = ints(sp.req("ratio"), &sp.at("ratio"))?;
            if r.len() != 2 || r[0] == 0 || r[1] <= 0 {
                return fail(format!("{w}.speed.ratio: expected [record, source] frames, record non-zero"));
            }
            Some(Speed { ratio: (r[0], r[1]), source_duration: sp.time("source_duration")? })
        }
    };
    let transform = match p.opt("transform") {
        None => None,
        Some(x) => {
            let x = Obj::new(x, &p.at("transform"), &[], &["position", "scale", "rotation", "anchor", "crop"])?;
            let pair = |k: &str, d: f64| -> R<(f64, f64)> {
                match x.opt(k) {
                    None => Ok((d, d)),
                    Some(v) => {
                        let v = floats(v, &x.at(k))?;
                        if v.len() != 2 {
                            return fail(format!("{}: expected [x, y]", x.at(k)));
                        }
                        Ok((v[0], v[1]))
                    }
                }
            };
            let crop = match x.opt("crop") {
                None => [0.0; 4],
                Some(v) => {
                    let v = floats(v, &x.at("crop"))?;
                    if v.len() != 4 {
                        return fail(format!("{}: expected [left, top, right, bottom]", x.at("crop")));
                    }
                    [v[0], v[1], v[2], v[3]]
                }
            };
            Some(Transform {
                position: pair("position", 0.0)?,
                scale: pair("scale", 100.0)?,
                rotation: x.opt_float("rotation")?.unwrap_or(0.0),
                anchor: pair("anchor", 0.0)?,
                crop,
            })
        }
    };
    let mut effects = Vec::new();
    for (j, e) in p.opt_list("effects")?.iter().enumerate() {
        let e = Obj::new(e, &format!("{w}.effects[{j}]"), &["kind"], &["params"])?;
        effects.push(Effect { kind: e.str("kind")?, params: e.raw("params")?.clone() });
    }
    Ok(Placement {
        id: p.str("id")?,
        item: p.str("item")?,
        record_start: p.time("record_start")?,
        source_in: p.time("source_in")?,
        duration: p.time("duration")?,
        video_track: p.opt_int("video_track")?,
        audio_tracks: p.opt("audio_tracks").map_or(Ok(Vec::new()), |v| ints(v, &p.at("audio_tracks")))?,
        audio_streams: p.opt("audio_streams").map_or(Ok(Vec::new()), |v| ints(v, &p.at("audio_streams")))?,
        grouped_audio,
        gain_db: p.opt_float("gain_db")?.unwrap_or(0.0),
        volume_db: p.opt_float("volume_db")?,
        volume_keys,
        track_gains_db: opt_floats(p.opt("track_gains_db"), &p.at("track_gains_db"))?,
        track_pans: opt_floats(p.opt("track_pans"), &p.at("track_pans"))?,
        speed,
        frame_map: p.opt("frame_map").map(|v| ints(v, &p.at("frame_map"))).transpose()?,
        transform,
        effects,
    })
}

fn markers(o: &Obj, key: &str) -> R<Vec<Marker>> {
    let mut out = Vec::new();
    for (i, m) in o.opt_list(key)?.iter().enumerate() {
        let w = format!("{}[{i}]", o.at(key));
        let m = Obj::new(
            m,
            &w,
            &["name", "start"],
            &["comment", "duration", "user", "color", "track", "owner", "placement", "source", "inherited"],
        )?;
        let color = match m.opt("color") {
            None => None,
            Some(c) => {
                let c = Obj::new(c, &m.at("color"), &["palette", "name"], &[])?;
                Some(Label { palette: c.str("palette")?, name: c.str("name")? })
            }
        };
        let track = match m.opt("track") {
            None => None,
            Some(r) => {
                let r = Obj::new(r, &m.at("track"), &["kind", "index"], &[])?;
                Some(TrackRef { kind: r.kind("kind")?, index: r.int("index")? })
            }
        };
        let owner = match m.opt("owner") {
            None => MarkerOwner::Sequence,
            Some(v) => MarkerOwner::parse(&text(v, &m.at("owner"))?).ok_or_else(|| GraphError(format!("{w}.owner: sequence, filler or edit")))?,
        };
        out.push(Marker {
            name: m.opt_str("name")?.unwrap_or_default(),
            comment: m.opt_str("comment")?.unwrap_or_default(),
            start: m.time("start")?,
            duration: m.opt_time("duration")?.unwrap_or(Rational::ZERO),
            user: m.opt_str("user")?.unwrap_or_default(),
            color,
            track,
            owner,
            placement: m.opt_str("placement")?,
            source: m.opt_time("source")?,
            inherited: m.opt_bool("inherited")?.unwrap_or(false),
        });
    }
    Ok(out)
}

// --- strict object reading ----------------------------------------------------------------------

struct Obj<'a> {
    pairs: &'a [(String, Value)],
    w: String,
}

fn text(v: &Value, w: &str) -> R<String> {
    match v {
        Value::Str(s) if s.len() <= 32768 => Ok(s.clone()),
        _ => fail(format!("{w}: expected text")),
    }
}

fn ints(v: &Value, w: &str) -> R<Vec<i64>> {
    match v {
        Value::Arr(a) if a.len() <= MAX_LIST => a.iter().map(|x| if let Value::Int(n) = x { Ok(*n) } else { fail(format!("{w}: expected integers")) }).collect(),
        _ => fail(format!("{w}: expected a list")),
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Float(x) => Some(*x),
        Value::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn floats(v: &Value, w: &str) -> R<Vec<f64>> {
    match v {
        Value::Arr(a) if a.len() <= MAX_LIST => a.iter().map(|x| number(x).ok_or_else(|| GraphError(format!("{w}: expected numbers")))).collect(),
        _ => fail(format!("{w}: expected a list")),
    }
}

fn opt_floats(v: Option<&Value>, w: &str) -> R<Vec<Option<f64>>> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Arr(a)) if a.len() <= MAX_LIST => a
            .iter()
            .map(|x| match x {
                Value::Null => Ok(None),
                x => number(x).map(Some).ok_or_else(|| GraphError(format!("{w}: expected numbers or null"))),
            })
            .collect(),
        Some(_) => fail(format!("{w}: expected a list")),
    }
}

impl<'a> Obj<'a> {
    fn new(v: &'a Value, w: &str, required: &[&str], optional: &[&str]) -> R<Obj<'a>> {
        let Value::Obj(pairs) = v else { return fail(format!("{w}: expected an object")) };
        let mut keys = HashSet::new();
        if let Some((key, _)) = pairs.iter().find(|(key, _)| !keys.insert(key.as_str())) {
            return fail(format!("{w}: repeated key {key:?}"));
        }
        if let Some((k, _)) = pairs.iter().find(|(k, _)| !required.contains(&k.as_str()) && !optional.contains(&k.as_str())) {
            return fail(format!("{w}: unknown key {k:?}"));
        }
        for k in required {
            if !pairs.iter().any(|(p, v)| p == k && *v != Value::Null) {
                return fail(format!("{w}: missing {k:?}"));
            }
        }
        Ok(Obj { pairs, w: w.to_string() })
    }

    fn at(&self, k: &str) -> String {
        format!("{}.{k}", self.w)
    }

    /// A present, non-null member.
    fn opt(&self, k: &str) -> Option<&'a Value> {
        self.pairs.iter().find(|(p, _)| p == k).map(|(_, v)| v).filter(|v| **v != Value::Null)
    }

    fn req(&self, k: &str) -> &'a Value {
        self.opt(k).expect("checked by Obj::new")
    }

    /// A required opaque JSON payload may itself be null; absence is different.
    fn raw(&self, k: &str) -> R<&'a Value> {
        self.pairs.iter().find(|(p, _)| p == k).map(|(_,v)|v)
            .ok_or_else(|| GraphError(format!("{}: missing {k:?}",self.w)))
    }

    fn str(&self, k: &str) -> R<String> {
        text(self.req(k), &self.at(k))
    }

    fn opt_str(&self, k: &str) -> R<Option<String>> {
        self.opt(k).map(|v| text(v, &self.at(k))).transpose()
    }

    fn int(&self, k: &str) -> R<i64> {
        match self.req(k) {
            Value::Int(n) => Ok(*n),
            _ => fail(format!("{}: expected an integer", self.at(k))),
        }
    }

    fn opt_int(&self, k: &str) -> R<Option<i64>> {
        self.opt(k).map(|_| self.int(k)).transpose()
    }

    fn float(&self, k: &str) -> R<f64> {
        number(self.req(k)).ok_or_else(|| GraphError(format!("{}: expected a number", self.at(k))))
    }

    fn opt_float(&self, k: &str) -> R<Option<f64>> {
        self.opt(k).map(|_| self.float(k)).transpose()
    }

    fn opt_bool(&self, k: &str) -> R<Option<bool>> {
        match self.opt(k) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => fail(format!("{}: expected true or false", self.at(k))),
        }
    }

    fn time(&self, k: &str) -> R<Rational> {
        match self.req(k) {
            Value::Str(s) => Rational::parse(s).ok_or_else(|| GraphError(format!("{}: expected a reduced \"num/den\"", self.at(k)))),
            _ => fail(format!("{}: expected a reduced \"num/den\"", self.at(k))),
        }
    }

    fn opt_time(&self, k: &str) -> R<Option<Rational>> {
        self.opt(k).map(|_| self.time(k)).transpose()
    }

    fn kind(&self, k: &str) -> R<StreamKind> {
        StreamKind::parse(&self.str(k)?).ok_or_else(|| GraphError(format!("{}: video or audio", self.at(k))))
    }

    fn opt_size(&self, k: &str) -> R<Option<(i64, i64)>> {
        match self.opt(k) {
            None => Ok(None),
            Some(v) => {
                let v = ints(v, &self.at(k))?;
                if v.len() != 2 || v.iter().any(|n| *n <= 0) {
                    return fail(format!("{}: expected [width, height]", self.at(k)));
                }
                Ok(Some((v[0], v[1])))
            }
        }
    }

    fn list(&self, k: &str) -> R<&'a [Value]> {
        match self.req(k) {
            Value::Arr(a) if a.len() <= MAX_LIST => Ok(a),
            _ => fail(format!("{}: expected a list of at most {MAX_LIST}", self.at(k))),
        }
    }

    fn opt_list(&self, k: &str) -> R<&'a [Value]> {
        match self.opt(k) {
            None => Ok(&[]),
            Some(_) => self.list(k),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Value {
        json::parse(
            r#"{"schema": "anynle.graph/1", "project": {"name": "P"},
            "media": [{"id": "m", "kind": "video", "locations": ["/a.mxf"], "duration": "1", "start": "0"}],
            "items": [{"id": "i", "name": "Clip", "media": [{"media": "m"}]}],
            "bins": [{"id": "b", "name": "Bin", "items": ["i"]}],
            "sequences": [{"id": "s", "name": "Seq", "rate": "24", "frame_size": [1920, 1080],
                "placements": [{"id": "p", "item": "i", "record_start": "0", "source_in": "0", "duration": "1/2", "video_track": 1}],
                "markers": [{"name": "M", "start": "1/24", "owner": "edit", "placement": "p", "source": "0", "track": {"kind": "video", "index": 1}}]}]}"#,
        )
        .unwrap()
    }

    /// `minimal()` with one edit applied at a JSON path (object keys, list indexes as numbers).
    fn edited(path: &[&str], value: Value) -> R<Graph> {
        let mut v = minimal();
        let mut at = &mut v;
        for (n, key) in path.iter().enumerate() {
            let last = n + 1 == path.len();
            at = match at {
                Value::Obj(pairs) => {
                    if last && !pairs.iter().any(|(k, _)| k == key) {
                        pairs.push((key.to_string(), Value::Null));
                    }
                    &mut pairs.iter_mut().find(|(k, _)| k == key).unwrap().1
                }
                Value::Arr(items) => &mut items[key.parse::<usize>().unwrap()],
                _ => panic!("bad path"),
            };
        }
        *at = value;
        Graph::from_json(&v)
    }

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn minimal_round_trips() {
        let g = Graph::from_json(&minimal()).unwrap();
        assert_eq!(Graph::from_json_text(&g.to_json_text().unwrap()).unwrap(), g);
    }

    #[test]
    fn refuses_malformed_graphs() {
        let cases: Vec<(&[&str], Value, &str)> = vec![
            (&["schema"], s("anynle.graph/2"), "schema"),
            (&["surprise"], Value::Int(1), "unknown key"),
            (&["items", "0", "id"], s("m"), "duplicate id"),
            (&["items", "0", "media", "0", "media"], s("nope"), "unknown media"),
            (&["bins", "0", "items", "0"], s("m"), "unknown item"),
            (&["bins", "0", "parent"], s("b"), "earlier bin"),
            (&["sequences", "0", "placements", "0", "item"], s("ghost"), "unknown item"),
            (&["sequences", "0", "placements", "0", "duration"], s("2/4"), "reduced"),
            (&["sequences", "0", "placements", "0", "duration"], s("0"), "out of range"),
            (&["sequences", "0", "placements", "0", "video_track"], Value::Null, "picture or audio"),
            (&["sequences", "0", "placements", "0", "video_track"], s("1"), "integer"),
            (&["sequences", "0", "markers", "0", "placement"], s("q"), "unknown placement"),
            (&["sequences", "0", "markers", "0", "source"], Value::Null, "edit marker needs"),
            (&["sequences", "0", "placements", "0", "effects"], Value::Arr(vec![json::parse(r#"{"kind": "blur", "params": {}}"#).unwrap()]), "<program>.<effect>"),
            (&["media", "0", "kind"], s("subtitle"), "video or audio"),
            (&["items", "0", "umid"], s("abc"), "hex"),
            (&["items", "0", "stream_groups"], json::parse("[[1, 2]]").unwrap(), "stream group"),
            (&["losses"], json::parse(r#"[{"object": "ghost", "aspect": "x", "reason": "y"}]"#).unwrap(), "unknown object"),
            (&["sidecars"], json::parse(r#"[{"adapter": "avid", "object": "ghost", "data": 1}]"#).unwrap(), "unknown object"),
            (&["bins", "0", "items"], Value::Arr(vec![s("i"); MAX_LIST + 1]), "at most"),
        ];
        for (path, value, expected) in cases {
            match edited(path, value) {
                Ok(_) => panic!("{path:?}: accepted"),
                Err(e) => assert!(e.0.contains(expected), "{path:?}: {e} (expected {expected:?})"),
            }
        }
    }

    #[test]
    fn non_finite_numbers_are_not_written() {
        let mut g = Graph::from_json(&minimal()).unwrap();
        g.sequences[0].placements[0].gain_db = f64::NAN;
        assert!(g.to_json_text().is_err());
    }

    fn audio_graph() -> Graph {
        let mut g = Graph::from_json(&minimal()).unwrap();
        let mut a = g.media[0].clone();
        a.id = "a1".into(); a.kind = StreamKind::Audio; a.rate = None;
        a.sample_rate = Some(48000); a.bits = Some(24); a.stream = Some(1);
        let mut b = a.clone(); b.id = "a2".into(); b.stream = Some(2); b.duration = Rational::integer(4);
        g.media.extend([a,b]);
        g.items[0].media.extend([ItemMedia {media:"a1".into(),gain_db:0.0},ItemMedia {media:"a2".into(),gain_db:0.0}]);
        g
    }

    fn refuses_all_entry_points(g: &Graph, expected: &str) {
        for result in [g.validate(), Graph::from_json(&g.to_json()).map(|_|()),
            Graph::from_json_text(&json::write(&g.to_json())).map(|_|()), g.to_json_text().map(|_|())] {
            let error = result.unwrap_err().to_string();
            assert!(error.contains(expected), "{error}, expected {expected}");
        }
    }

    #[test]
    fn typed_and_json_graphs_share_neutral_scalar_and_routing_checks() {
        let base = audio_graph();
        let mut cases = Vec::new();
        let mut g = base.clone();g.sequences[0].rate=Rational::ZERO;cases.push((g,"rate must be positive"));
        let mut g = base.clone();g.media[0].rate=Some(Rational::integer(-24));cases.push((g,"rate must be positive"));
        let mut g = base.clone();g.media[1].sample_rate=Some(0);cases.push((g,"sample rate must be positive"));
        let mut g = base.clone();g.sequences[0].tracks=vec![Track {kind:StreamKind::Audio,index:1,level_db:None};2];cases.push((g,"duplicate declared track"));
        let mut g = base.clone();g.sequences[0].placements[0].audio_tracks=vec![1];g.sequences[0].placements[0].audio_streams=vec![99];cases.push((g,"stream the item lacks"));
        let mut g = base.clone();g.sequences[0].placements[0].audio_tracks=vec![1,2];g.sequences[0].placements[0].audio_streams=vec![1];cases.push((g,"cardinality"));
        let mut g = base.clone();g.sequences[0].placements[0].audio_tracks=vec![1];g.sequences[0].placements[0].grouped_audio=vec![GroupedAudio {track:1,streams:vec![1,2],gain_db:None,pans:vec![]}];cases.push((g,"share a destination"));
        let mut g = base.clone();g.sequences[0].placements[0].grouped_audio=vec![GroupedAudio {track:3,streams:vec![1,1],gain_db:None,pans:vec![]}];cases.push((g,"duplicate track/stream"));
        let mut g = base.clone();g.sidecars=vec![Sidecar{adapter:"avid".into(),object:"i".into(),data:Value::Null};2];cases.push((g,"duplicate adapter/object"));
        let mut g = base.clone();g.sequences[0].transitions=vec![Transition {kind:StreamKind::Audio,track:1,cut:Rational::integer(1),duration:Rational::integer(1),cut_offset:Rational::integer(2),style:"x".into()}];cases.push((g,"invalid transition timing"));
        for (g,why) in cases {refuses_all_entry_points(&g,why);}
    }

    #[test]
    fn sparse_tracks_audio_subframes_and_non_premiere_clocks_remain_valid() {
        let mut g=audio_graph();
        let s=&mut g.sequences[0];s.rate=Rational::new(1,7).unwrap();
        s.tracks=vec![Track{kind:StreamKind::Audio,index:9,level_db:Some(-2.0)}];
        let p=&mut s.placements[0];p.video_track=None;p.audio_tracks=vec![2,3];
        p.audio_streams=vec![2,2];p.record_start=Rational::new(1,48000).unwrap();
        p.source_in=Rational::integer(1);p.duration=Rational::new(1,48000).unwrap();
        g.validate().unwrap();
        assert_eq!(Graph::from_json_text(&g.to_json_text().unwrap()).unwrap(),g);
    }

    #[test]
    fn source_bounds_follow_selected_streams_and_absolute_frame_maps() {
        let mut g=audio_graph();
        let p=&mut g.sequences[0].placements[0];p.video_track=None;p.audio_tracks=vec![1];p.audio_streams=vec![2];
        p.source_in=Rational::integer(1);p.duration=Rational::integer(1);
        g.validate().unwrap(); // A1 is shorter, but is not selected.
        g.sequences[0].placements[0].audio_streams=vec![1];
        refuses_all_entry_points(&g,"source range exceeds selected media");
        let mut g=Graph::from_json(&minimal()).unwrap();
        g.sequences[0].rate=Rational::integer(7);g.media[0].rate=Some(Rational::integer(7));
        let p=&mut g.sequences[0].placements[0];p.duration=Rational::new(2,7).unwrap();
        p.source_in=Rational::integer(99);p.frame_map=Some(vec![5,6]);
        g.validate().unwrap(); // Absolute source frames do not add source_in again.
        g.sequences[0].placements[0].frame_map=Some(vec![6,7]);
        refuses_all_entry_points(&g,"frame map exceeds selected picture");
    }

    #[test]
    fn public_value_input_cannot_shadow_schema_fields_or_native_json_keys() {
        let mut v=minimal();
        if let Value::Obj(fields)=&mut v {fields.push(("schema".into(),Value::Str(SCHEMA.into())));}
        assert!(Graph::from_json(&v).unwrap_err().to_string().contains("repeated key"));
        let mut g=Graph::from_json(&minimal()).unwrap();
        g.sidecars.push(Sidecar {adapter:"avid".into(),object:"i".into(),data:Value::Obj(vec![("x".into(),Value::Int(1)),("x".into(),Value::Int(2))])});
        assert!(g.validate().unwrap_err().to_string().contains("repeated JSON key"));
        g.sidecars[0].data=Value::Float(f64::INFINITY);
        assert!(g.validate().is_err());
        assert!(Graph::from_json(&g.to_json()).is_err());
        assert!(g.to_json_text().is_err());
    }
}
