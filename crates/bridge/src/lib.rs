//! `avid2prproj.model/1` <-> `anynle.graph/1` (docs/GRAPH-MODEL.md).
//!
//! The contract's first acceptance: every supported model/1 projects into the graph and back to
//! the same model, with no new losses, so the working Avid -> Premiere path can later run through
//! the graph without changing a byte. Model/1's pair options (bin mode, colour tables, marker
//! conflicts, Avid default pan) are not graph content; they travel beside it.
//!
//! Where a graph holds more than model/1 can (graphs not made here), [`to_model`] returns what it
//! dropped as losses. Projection refuses what model/1 cannot express (e.g. time off its tick
//! clock) and placements that fail model/1's shared reference, frame-grid or source-span rules.

use std::collections::{HashMap, HashSet};

use graph::{
    Bin, Effect, Graph, GraphError, GroupedAudio, Item, ItemMedia, Label, Loss, Marker as GMarker, MarkerOwner, Media, Placement,
    Project, Rational, Sequence as GSequence, Sidecar, Speed, StreamKind, Track, TrackRef, Transform as GTransform, Transition,
    Value, VolumeKey as GVolumeKey,
};
use prproj::convert::model::{
    AudioCrossfade, AudioFile, AuxTimecode, AvidInfo, Bin as MBin, Clip, ColorCorrection, Event, EventAudio, Fraction, Marker, Matte,
    validate_sequence_links, Model, Options, Sequence, StereoEdit, TrackLevel, Transform, VideoFile, VolumeKey, TICKS_PER_SECOND,
};

pub type R<T> = Result<T, GraphError>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(GraphError(msg.into()))
}

/// Adapter names for sidecars and effect kinds.
pub const AVID: &str = "avid";
pub const PREMIERE: &str = "premiere";
const COLOR_CORRECTION: &str = "avid.color_correction";
const ANIMATTE: &str = "avid.animatte";

// --- IDs: namespaced so model/1 names can never collide ------------------------------------------

fn item_id(clip: &str) -> String {
    format!("clip:{clip}")
}

fn bin_id(i: usize) -> String {
    format!("bin:{i}")
}

fn sequence_id(i: usize) -> String {
    format!("sequence:{i}")
}

fn placement_id(seq: usize, event: usize) -> String {
    format!("sequence:{seq}:{event}")
}

// --- small conversions ---------------------------------------------------------------------------

fn time(ticks: i64) -> R<Rational> {
    Rational::from_units(ticks, TICKS_PER_SECOND).ok_or_else(|| GraphError(format!("time {ticks} ticks out of range")))
}

fn ticks(t: Rational, what: &str) -> R<i64> {
    t.to_units(TICKS_PER_SECOND).ok_or_else(|| GraphError(format!("{what}: {t} s is not on model/1's 254016000000-tick clock")))
}

fn rate(f: Fraction) -> R<Rational> {
    Rational::new(f.num as i128, f.den as i128).ok_or_else(|| GraphError(format!("rate {f} out of range")))
}

fn fraction(r: Rational) -> Fraction {
    Fraction { num: r.num(), den: r.den() }
}

fn track_ref(label: &str) -> R<TrackRef> {
    let kind = match label.as_bytes().first() {
        Some(b'V') => StreamKind::Video,
        Some(b'A') => StreamKind::Audio,
        _ => return fail(format!("marker track {label:?}: expected V1/A1-style")),
    };
    let index: i64 = label[1..].parse().map_err(|_| GraphError(format!("marker track {label:?}: expected V1/A1-style")))?;
    let r = TrackRef { kind, index };
    // Only the canonical spelling maps back to the same text ("V01" would come back as "V1").
    if track_label(r) != label {
        return fail(format!("marker track {label:?}: not canonical (V1, A12)"));
    }
    Ok(r)
}

fn track_label(r: TrackRef) -> String {
    format!("{}{}", if r.kind == StreamKind::Video { 'V' } else { 'A' }, r.index)
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn opt_text(s: &Option<String>) -> Value {
    s.as_ref().map_or(Value::Null, |s| Value::Str(s.clone()))
}

fn losses(out: &mut Vec<Loss>, object: &str, aspect: &str, reasons: &[String]) {
    out.extend(reasons.iter().map(|r| Loss { object: object.to_string(), aspect: aspect.to_string(), reason: r.clone() }));
}

// --- model/1 -> graph ----------------------------------------------------------------------------

fn marker_to_graph(m: &Marker, seq: Option<usize>) -> R<GMarker> {
    let placement = match (m.event, seq) {
        (None, _) => None,
        (Some(e), Some(s)) => Some(placement_id(s, usize::try_from(e).map_err(|_| GraphError("marker event out of range".into()))?)),
        (Some(_), None) => return fail(format!("clip marker {:?}: names an event", m.name)),
    };
    let owner = match m.owner.as_str() {
        "sequence" => MarkerOwner::Sequence,
        "filler" => MarkerOwner::Filler,
        "edit" => MarkerOwner::Edit,
        other => return fail(format!("marker owner {other:?}")),
    };
    Ok(GMarker {
        name: m.name.clone(),
        comment: m.comment.clone(),
        start: time(m.start)?,
        duration: time(m.duration)?,
        user: m.user.clone(),
        color: (!m.color.is_empty()).then(|| Label { palette: AVID.to_string(), name: m.color.clone() }),
        track: m.track.as_deref().map(track_ref).transpose()?,
        owner,
        placement,
        source: m.source.map(time).transpose()?,
        inherited: m.inherited,
    })
}

fn avid_sidecar(a: &AvidInfo) -> Value {
    obj(vec![
        ("mob_id", Value::Str(a.mob_id.clone())),
        ("columns", Value::Obj(a.columns.iter().map(|(k, v)| (k.clone(), Value::Str(v.clone()))).collect())),
        ("start_timecode", opt_text(&a.start_timecode)),
        (
            "aux_timecodes",
            Value::Arr(
                a.aux_timecodes
                    .iter()
                    .map(|x| {
                        obj(vec![
                            ("track_index", Value::Int(x.track_index)),
                            ("timecode", Value::Str(x.timecode.clone())),
                            ("frames", Value::Int(x.frames)),
                            ("fps", Value::Int(x.fps)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("source_file", opt_text(&a.source_file)),
        ("source_path", opt_text(&a.source_path)),
    ])
}

fn color_correction_params(c: &ColorCorrection) -> Value {
    obj(vec![
        ("saturation", c.saturation.map_or(Value::Null, Value::Float)),
        ("tone", Value::Obj(c.tone.iter().map(|(k, v)| (k.clone(), Value::Float(*v))).collect())),
        (
            "curves",
            Value::Obj(
                c.curves
                    .iter()
                    .map(|(k, pts)| (k.clone(), Value::Arr(pts.iter().map(|(x, y)| Value::Arr(vec![Value::Float(*x), Value::Float(*y)])).collect())))
                    .collect(),
            ),
        ),
    ])
}

fn matte_params(m: &Matte) -> Value {
    let shapes = m
        .shapes
        .iter()
        .zip(&m.opacities)
        .zip(&m.feathers)
        .map(|((keys, opacity), feather)| {
            obj(vec![
                ("opacity", Value::Float(*opacity)),
                ("feather", Value::Arr(vec![Value::Float(feather.0), Value::Float(feather.1)])),
                (
                    "keyframes",
                    Value::Arr(
                        keys.iter()
                            .map(|(frame, points)| {
                                obj(vec![
                                    ("frame", Value::Int(*frame)),
                                    ("points", Value::Arr(points.iter().map(|p| Value::Arr(p.iter().map(|v| Value::Float(*v)).collect())).collect())),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    obj(vec![("inverted", Value::Bool(m.inverted)), ("shapes", Value::Arr(shapes))])
}

fn event_to_graph(e: &Event, seq: usize, index: usize, lost: &mut Vec<Loss>) -> R<Placement> {
    let id = placement_id(seq, index);
    let mut effects = Vec::new();
    if let Some(c) = &e.color_correction {
        effects.push(Effect { kind: COLOR_CORRECTION.to_string(), params: color_correction_params(c) });
        losses(lost, &id, "color_correction", &c.not_converted);
    }
    if let Some(m) = &e.matte {
        if m.shapes.len() != m.opacities.len() || m.shapes.len() != m.feathers.len() {
            return fail(format!("{id}: matte shapes, opacities and feathers differ in length"));
        }
        effects.push(Effect { kind: ANIMATTE.to_string(), params: matte_params(m) });
        losses(lost, &id, "matte", &m.not_converted);
    }
    if let Some(t) = &e.transform {
        losses(lost, &id, "transform", &t.not_converted);
    }
    let speed = match (e.speed_ratio, e.source_duration) {
        (None, None) => None,
        (Some(ratio), Some(sd)) => Some(Speed { ratio, source_duration: time(sd)? }),
        _ => return fail(format!("{id}: speed_ratio and source_duration come together")),
    };
    Ok(Placement {
        item: item_id(&e.clip),
        record_start: time(e.record_start)?,
        source_in: time(e.source_in)?,
        duration: time(e.duration)?,
        video_track: e.video_track,
        audio_tracks: e.audio_tracks.clone(),
        audio_streams: e.audio_channels.clone(),
        grouped_audio: e
            .stereo
            .iter()
            .map(|s| GroupedAudio { track: s.audio_track, streams: s.channels.clone(), gain_db: s.gain_db, pans: s.pans.clone() })
            .collect(),
        gain_db: e.audio.gain_db,
        volume_db: e.audio.volume_db,
        volume_keys: e.audio.volume_keys.iter().map(|k| Ok(GVolumeKey { at: time(k.ticks)?, db: k.db })).collect::<R<_>>()?,
        track_gains_db: e.audio_gains.clone(),
        track_pans: e.audio_pans.clone(),
        speed,
        frame_map: e.time_remap.clone(),
        transform: e.transform.as_ref().map(|t| GTransform {
            position: t.position,
            scale: t.scale,
            rotation: t.rotation,
            anchor: t.anchor,
            crop: t.crop,
        }),
        effects,
        id,
    })
}

/// Project a model/1 into the graph. Returns the graph and model/1's pair options.
pub fn to_graph(model: &Model) -> R<(Graph, Options)> {
    if model.clips.values().any(|c| !c.multichannel_groups.is_empty()) {
        return Err(GraphError("multichannel declarations require source graph/2; graph/1 cannot preserve their provenance".into()));
    }
    let mut media = Vec::new();
    let mut items = Vec::new();
    let mut sidecars = Vec::new();
    let mut lost = Vec::new();
    for clip in model.clips.values() {
        let id = item_id(&clip.id);
        let mut uses = Vec::new();
        if let Some(v) = &clip.video {
            let mid = format!("{id}:video");
            media.push(Media {
                id: mid.clone(),
                kind: StreamKind::Video,
                locations: vec![v.path.clone()],
                codec: Some(v.codec.clone()),
                frame_size: Some((v.width, v.height)),
                rate: Some(rate(v.rate)?),
                sample_rate: None,
                bits: None,
                duration: time(v.duration)?,
                start: time(v.start)?,
                stream: None,
                online: None,
            });
            uses.push(ItemMedia { media: mid, gain_db: 0.0 });
        }
        for (i, a) in clip.audio.iter().enumerate() {
            let mid = format!("{id}:audio:{i}");
            media.push(Media {
                id: mid.clone(),
                kind: StreamKind::Audio,
                locations: vec![a.path.clone()],
                codec: None,
                frame_size: None,
                rate: None,
                sample_rate: Some(a.sample_rate),
                bits: Some(a.bits),
                duration: time(a.duration)?,
                start: time(a.start)?,
                stream: a.avid_track,
                online: None,
            });
            uses.push(ItemMedia { media: mid, gain_db: a.gain_db });
        }
        if let Some(a) = &clip.avid {
            sidecars.push(Sidecar { adapter: AVID.to_string(), object: id.clone(), data: avid_sidecar(a) });
        }
        if let Some(td) = clip.time_display {
            sidecars.push(Sidecar { adapter: PREMIERE.to_string(), object: id.clone(), data: obj(vec![("time_display", Value::Int(td))]) });
        }
        items.push(Item {
            id,
            name: clip.name.clone(),
            media: uses,
            umid: clip.material_umid.clone(),
            tape: clip.tape_name.clone(),
            rate: clip.rate.map(rate).transpose()?,
            color_rgb16: clip.avid.as_ref().and_then(|a| a.color_rgb16),
            stream_groups: clip.stereo_pairs.iter().map(|(l, r)| vec![*l, *r]).collect(),
            markers: clip.markers.iter().map(|m| marker_to_graph(m, None)).collect::<R<_>>()?,
        });
    }
    let bins = model
        .bins
        .iter()
        .enumerate()
        .map(|(i, b)| Bin { id: bin_id(i), name: b.name.clone(), parent: None, items: b.clips.iter().map(|c| item_id(c)).collect() })
        .collect();
    let mut sequences = Vec::new();
    for (si, s) in model.sequences.iter().enumerate() {
        let placements = s.events.iter().enumerate().map(|(ei, e)| event_to_graph(e, si, ei, &mut lost)).collect::<R<_>>()?;
        sequences.push(GSequence {
            id: sequence_id(si),
            name: s.name.clone(),
            rate: rate(s.rate)?,
            frame_size: (s.width, s.height),
            start: time(s.start)?,
            tracks: s.track_levels.iter().map(|l| Track { kind: StreamKind::Audio, index: l.track, level_db: Some(l.db) }).collect(),
            placements,
            transitions: s
                .audio_crossfades
                .iter()
                .map(|x| {
                    Ok(Transition {
                        kind: StreamKind::Audio,
                        track: x.track,
                        cut: time(x.cut)?,
                        duration: time(x.duration)?,
                        cut_offset: time(x.cut_offset)?,
                        style: x.style.clone(),
                    })
                })
                .collect::<R<_>>()?,
            markers: s.markers.iter().map(|m| marker_to_graph(m, Some(si))).collect::<R<_>>()?,
        });
    }
    let g = Graph { project: Project { name: model.project_name.clone() }, media, items, bins, sequences, losses: lost, sidecars };
    g.validate()?;
    Ok((g, model.options.clone()))
}

// --- graph -> model/1 ----------------------------------------------------------------------------

/// Typed reads of sidecar/effect values the bridge wrote (strict: a wrong shape is an error).
struct V<'a>(&'a Value, String);

impl<'a> V<'a> {
    fn key(&self, k: &str) -> R<V<'a>> {
        match self.0.get(k) {
            Some(v) => Ok(V(v, format!("{}.{k}", self.1))),
            None => fail(format!("{}: missing {k:?}", self.1)),
        }
    }

    fn text(&self) -> R<String> {
        match self.0 {
            Value::Str(s) => Ok(s.clone()),
            _ => fail(format!("{}: expected text", self.1)),
        }
    }

    fn opt_text(&self) -> R<Option<String>> {
        match self.0 {
            Value::Null => Ok(None),
            _ => self.text().map(Some),
        }
    }

    fn int(&self) -> R<i64> {
        match self.0 {
            Value::Int(n) => Ok(*n),
            _ => fail(format!("{}: expected an integer", self.1)),
        }
    }

    fn num(&self) -> R<f64> {
        match self.0 {
            Value::Float(x) => Ok(*x),
            Value::Int(n) => Ok(*n as f64),
            _ => fail(format!("{}: expected a number", self.1)),
        }
    }

    fn list(&self) -> R<Vec<V<'a>>> {
        match self.0 {
            Value::Arr(a) => Ok(a.iter().enumerate().map(|(i, v)| V(v, format!("{}[{i}]", self.1))).collect()),
            _ => fail(format!("{}: expected a list", self.1)),
        }
    }

    fn pairs(&self) -> R<Vec<(String, V<'a>)>> {
        match self.0 {
            Value::Obj(p) => Ok(p.iter().map(|(k, v)| (k.clone(), V(v, format!("{}.{k}", self.1)))).collect()),
            _ => fail(format!("{}: expected an object", self.1)),
        }
    }
}

pub fn avid_from_sidecar(v: &Value, color: Option<[i64; 3]>) -> R<AvidInfo> {
    let v = V(v, "avid sidecar".to_string());
    Ok(AvidInfo {
        mob_id: v.key("mob_id")?.text()?,
        columns: v.key("columns")?.pairs()?.into_iter().map(|(k, x)| Ok((k, x.text()?))).collect::<R<_>>()?,
        color_rgb16: color,
        start_timecode: v.key("start_timecode")?.opt_text()?,
        aux_timecodes: v
            .key("aux_timecodes")?
            .list()?
            .iter()
            .map(|x| {
                Ok(AuxTimecode {
                    track_index: x.key("track_index")?.int()?,
                    timecode: x.key("timecode")?.text()?,
                    frames: x.key("frames")?.int()?,
                    fps: x.key("fps")?.int()?,
                })
            })
            .collect::<R<_>>()?,
        source_file: v.key("source_file")?.opt_text()?,
        source_path: v.key("source_path")?.opt_text()?,
    })
}

pub fn color_correction_from(p: &Value, not_converted: Vec<String>) -> R<ColorCorrection> {
    let v = V(p, COLOR_CORRECTION.to_string());
    let sat = v.key("saturation")?;
    Ok(ColorCorrection {
        saturation: if *sat.0 == Value::Null { None } else { Some(sat.num()?) },
        tone: v.key("tone")?.pairs()?.into_iter().map(|(k, x)| Ok((k, x.num()?))).collect::<R<_>>()?,
        curves: v
            .key("curves")?
            .pairs()?
            .into_iter()
            .map(|(k, pts)| {
                let pts = pts
                    .list()?
                    .iter()
                    .map(|p| {
                        let xy = p.list()?;
                        if xy.len() != 2 {
                            return fail(format!("{}: expected [x, y]", p.1));
                        }
                        Ok((xy[0].num()?, xy[1].num()?))
                    })
                    .collect::<R<_>>()?;
                Ok((k, pts))
            })
            .collect::<R<_>>()?,
        not_converted,
    })
}

pub fn matte_from(p: &Value, not_converted: Vec<String>) -> R<Matte> {
    let v = V(p, ANIMATTE.to_string());
    let inverted = match v.key("inverted")?.0 {
        Value::Bool(b) => *b,
        _ => return fail("avid.animatte.inverted: expected true or false"),
    };
    let (mut shapes, mut opacities, mut feathers) = (Vec::new(), Vec::new(), Vec::new());
    for shape in v.key("shapes")?.list()? {
        opacities.push(shape.key("opacity")?.num()?);
        let f = shape.key("feather")?.list()?;
        if f.len() != 2 {
            return fail(format!("{}: expected two values", shape.1));
        }
        feathers.push((f[0].num()?, f[1].num()?));
        let mut keys = Vec::new();
        for k in shape.key("keyframes")?.list()? {
            let mut points = Vec::new();
            for p in k.key("points")?.list()? {
                let xs = p.list()?;
                if xs.len() != 6 {
                    return fail(format!("{}: expected 6 numbers", p.1));
                }
                let mut out = [0.0; 6];
                for (slot, x) in out.iter_mut().zip(&xs) {
                    *slot = x.num()?;
                }
                points.push(out);
            }
            keys.push((k.key("frame")?.int()?, points));
        }
        shapes.push(keys);
    }
    Ok(Matte { shapes, inverted, opacities, feathers, not_converted })
}

fn marker_from_graph(m: &GMarker, events: &HashMap<&str, usize>) -> R<Marker> {
    let event = match &m.placement {
        None => None,
        Some(p) => Some(*events.get(p.as_str()).ok_or_else(|| GraphError(format!("marker {:?}: unknown placement {p:?}", m.name)))? as i64),
    };
    let color = match &m.color {
        None => String::new(),
        Some(l) if l.palette == AVID => l.name.clone(),
        // model/1 colours are Avid names; another palette's label needs a mapping first.
        Some(l) => return fail(format!("marker {:?}: colour from palette {:?}; model/1 holds Avid names only", m.name, l.palette)),
    };
    Ok(Marker {
        name: m.name.clone(),
        comment: m.comment.clone(),
        start: ticks(m.start, "marker start")?,
        duration: ticks(m.duration, "marker duration")?,
        user: m.user.clone(),
        color,
        track: m.track.map(track_label),
        owner: m.owner.as_str().to_string(),
        event,
        source: m.source.map(|s| ticks(s, "marker source")).transpose()?,
        inherited: m.inherited,
    })
}

fn drop(lost: &mut Vec<Loss>, object: &str, aspect: &str, reason: impl Into<String>) {
    lost.push(Loss { object: object.to_string(), aspect: aspect.to_string(), reason: reason.into() });
}

/// Rebuild a model/1 from the graph and the pair options. Returns the model and what model/1
/// could not hold (empty for any graph made by [`to_graph`]). Rechecks model/1's placement
/// references, record frame grid and source range before returning a writer input.
pub fn to_model(g: &Graph, options: &Options) -> R<(Model, Vec<Loss>)> {
    g.validate()?;
    let mut lost = Vec::new();
    let media: HashMap<&str, &Media> = g.media.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut clips = Vec::new();
    let mut clip_ids = HashSet::new();
    for item in &g.items {
        let clip_id = item.id.strip_prefix("clip:").unwrap_or(&item.id).to_string();
        if !clip_ids.insert(clip_id.clone()) {
            return fail(format!("item {:?}: model/1 clip id collision {:?}", item.id, clip_id));
        }
        let mut video = None;
        let mut audio = Vec::new();
        for im in &item.media {
            let m = media[im.media.as_str()];
            if m.locations.len() > 1 {
                drop(&mut lost, &m.id, "locations", format!("{} further locations (model/1 keeps one path)", m.locations.len() - 1));
            }
            if m.online.is_some() {
                drop(&mut lost, &m.id, "online", "online state (model/1 does not record it)");
            }
            let Some(path) = m.locations.first().cloned() else { return fail(format!("media {:?}: no location", m.id)) };
            let unheld: &[(&str, bool)] = match m.kind {
                StreamKind::Video => &[("sample rate", m.sample_rate.is_some()), ("bits", m.bits.is_some()), ("stream", m.stream.is_some())],
                StreamKind::Audio => &[("codec", m.codec.is_some()), ("frame size", m.frame_size.is_some()), ("rate", m.rate.is_some())],
            };
            for (what, set) in unheld {
                if *set {
                    drop(&mut lost, &m.id, "media", format!("{what} on {} media (model/1 does not keep it)", m.kind.as_str()));
                }
            }
            match m.kind {
                StreamKind::Video => {
                    if video.is_some() {
                        return fail(format!("item {:?}: model/1 holds one picture stream per clip", item.id));
                    }
                    let (Some(codec), Some((width, height)), Some(r)) = (&m.codec, m.frame_size, m.rate) else {
                        return fail(format!("media {:?}: model/1 needs codec, frame size and rate", m.id));
                    };
                    if im.gain_db != 0.0 {
                        drop(&mut lost, &item.id, "gain", "gain on a picture stream");
                    }
                    video = Some(VideoFile {
                        path,
                        codec: codec.clone(),
                        width,
                        height,
                        rate: fraction(r),
                        duration: ticks(m.duration, "media duration")?,
                        start: ticks(m.start, "media start")?,
                    });
                }
                StreamKind::Audio => {
                    let (Some(sample_rate), Some(bits)) = (m.sample_rate, m.bits) else {
                        return fail(format!("media {:?}: model/1 needs sample rate and bits", m.id));
                    };
                    audio.push(AudioFile {
                        path,
                        sample_rate,
                        duration: ticks(m.duration, "media duration")?,
                        bits,
                        avid_track: m.stream,
                        start: ticks(m.start, "media start")?,
                        gain_db: im.gain_db,
                    });
                }
            }
        }
        let avid = match g.sidecar(AVID, &item.id) {
            Some(v) => Some(avid_from_sidecar(v, item.color_rgb16)?),
            None => {
                if item.color_rgb16.is_some() {
                    drop(&mut lost, &item.id, "color", "label colour (model/1 keeps it in the Avid block)");
                }
                None
            }
        };
        let time_display = match g.sidecar(PREMIERE, &item.id) {
            None => None,
            Some(v) => Some(V(v, "premiere sidecar".to_string()).key("time_display")?.int()?),
        };
        let mut stereo_pairs = Vec::new();
        for group in &item.stream_groups {
            match group.as_slice() {
                [l, r] => stereo_pairs.push((*l, *r)),
                _ => drop(&mut lost, &item.id, "stream_groups", format!("a {}-stream group (model/1 pairs only)", group.len())),
            }
        }
        clips.push(Clip {
            id: clip_id,
            name: item.name.clone(),
            material_umid: item.umid.clone(),
            tape_name: item.tape.clone(),
            video,
            audio,
            avid,
            time_display,
            stereo_pairs,
            multichannel_groups: vec![],
            rate: item.rate.map(fraction),
            markers: item.markers.iter().map(|m| marker_from_graph(m, &HashMap::new())).collect::<R<_>>()?,
        });
    }
    let clip_of = |id: &str| id.strip_prefix("clip:").unwrap_or(id).to_string();
    let mut bins = Vec::new();
    for b in &g.bins {
        if b.parent.is_some() {
            drop(&mut lost, &b.id, "hierarchy", "nested bin flattened (model/1 bins are flat)");
        }
        bins.push(MBin { name: b.name.clone(), clips: b.items.iter().map(|i| clip_of(i)).collect() });
    }
    // (placement, aspect) losses put back into the model/1 part that carries them.
    let mut restored: std::collections::HashSet<(String, &str)> = std::collections::HashSet::new();
    let mut sequences = Vec::new();
    for s in &g.sequences {
        let events_ids: HashMap<&str, usize> = s.placements.iter().enumerate().map(|(i, p)| (p.id.as_str(), i)).collect();
        let mut events = Vec::new();
        for p in &s.placements {
            let mut color_correction = None;
            let mut matte = None;
            for e in &p.effects {
                let repeated = match e.kind.as_str() {
                    COLOR_CORRECTION => color_correction.is_some(),
                    ANIMATTE => matte.is_some(),
                    _ => false,
                };
                if repeated {
                    drop(&mut lost, &p.id, "effects", format!("a second {} (model/1 holds one per edit)", e.kind));
                    continue;
                }
                match e.kind.as_str() {
                    COLOR_CORRECTION => {
                        restored.insert((p.id.clone(), "color_correction"));
                        color_correction = Some(color_correction_from(&e.params, g.losses_for(&p.id, "color_correction").map(String::from).collect())?)
                    }
                    ANIMATTE => {
                        restored.insert((p.id.clone(), "matte"));
                        matte = Some(matte_from(&e.params, g.losses_for(&p.id, "matte").map(String::from).collect())?);
                    }
                    other => drop(&mut lost, &p.id, "effects", format!("effect {other} (no model/1 form)")),
                }
            }
            if p.transform.is_some() {
                restored.insert((p.id.clone(), "transform"));
            }
            events.push(Event {
                clip: clip_of(&p.item),
                record_start: ticks(p.record_start, "record start")?,
                source_in: ticks(p.source_in, "source in")?,
                duration: ticks(p.duration, "duration")?,
                video_track: p.video_track,
                audio_tracks: p.audio_tracks.clone(),
                audio: EventAudio {
                    gain_db: p.gain_db,
                    volume_db: p.volume_db,
                    volume_keys: p.volume_keys.iter().map(|k| Ok(VolumeKey { ticks: ticks(k.at, "volume key")?, db: k.db })).collect::<R<_>>()?,
                },
                audio_channels: p.audio_streams.clone(),
                stereo: p
                    .grouped_audio
                    .iter()
                    .map(|x| StereoEdit { audio_track: x.track, channels: x.streams.clone(), gain_db: x.gain_db, pans: x.pans.clone() })
                    .collect(),
                audio_gains: p.track_gains_db.clone(),
                audio_pans: p.track_pans.clone(),
                speed_ratio: p.speed.as_ref().map(|sp| sp.ratio),
                source_duration: p.speed.as_ref().map(|sp| ticks(sp.source_duration, "source duration")).transpose()?,
                color_correction,
                transform: p.transform.as_ref().map(|t| Transform {
                    position: t.position,
                    scale: t.scale,
                    rotation: t.rotation,
                    anchor: t.anchor,
                    crop: t.crop,
                    not_converted: g.losses_for(&p.id, "transform").map(String::from).collect(),
                }),
                matte,
                time_remap: p.frame_map.clone(),
            });
        }
        let mut track_levels = Vec::new();
        for t in &s.tracks {
            match (t.kind, t.level_db) {
                (StreamKind::Audio, Some(db)) => track_levels.push(TrackLevel { track: t.index, db }),
                (StreamKind::Video, Some(_)) => drop(&mut lost, &s.id, "tracks", format!("level on picture track V{}", t.index)),
                (_, None) => {}
            }
        }
        let mut audio_crossfades = Vec::new();
        for x in &s.transitions {
            if x.kind == StreamKind::Video {
                drop(&mut lost, &s.id, "transitions", format!("picture transition on V{} (model/1 has audio crossfades only)", x.track));
                continue;
            }
            audio_crossfades.push(AudioCrossfade {
                track: x.track,
                cut: ticks(x.cut, "transition cut")?,
                duration: ticks(x.duration, "transition duration")?,
                cut_offset: ticks(x.cut_offset, "transition offset")?,
                style: x.style.clone(),
            });
        }
        sequences.push(Sequence {
            name: s.name.clone(),
            rate: fraction(s.rate),
            width: s.frame_size.0,
            height: s.frame_size.1,
            markers: s.markers.iter().map(|m| marker_from_graph(m, &events_ids)).collect::<R<_>>()?,
            events,
            track_levels,
            audio_crossfades,
            start: ticks(s.start, "sequence start")?,
        });
    }
    for sc in &g.sidecars {
        if sc.adapter != AVID && sc.adapter != PREMIERE {
            drop(&mut lost, &sc.object, "sidecar", format!("{} data (no model/1 form)", sc.adapter));
        }
    }
    // The graph's own losses stay losses, except those restored into model/1's not_converted.
    for l in &g.losses {
        if !restored.contains(&(l.object.clone(), l.aspect.as_str())) {
            lost.push(l.clone());
        }
    }
    let model = Model { project_name: g.project.name.clone(), options: options.clone(), clips: clips.into_iter().collect(), bins, sequences };
    validate_sequence_links(&model.clips, &model.sequences).map_err(|e| GraphError(e.to_string()))?;
    Ok((model, lost))
}
