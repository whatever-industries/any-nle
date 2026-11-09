//! anynle.graph/1 -> FCPXML 1.9, the second program writer (ADR-0004). DaVinci Resolve and
//! Final Cut Pro import it.
//!
//! Layout (the same one as `prproj/src/export/fcpxml.rs`, which exports a parsed Premiere project
//! byte-identically to py_premiere and is kept separate for that reason):
//! - `resources`: one `format` per sequence and per picture format; one `asset` per media stream.
//!   The asset starts at the media's own timecode, so clip times are media timecode.
//! - One `event` per bin holding one `asset-clip` per media file of each item (FCPXML has no
//!   multi-file browser clip that Resolve imports), and one `event` "<project> sequences"
//!   holding a `project` per sequence. Spine offsets start at the sequence's `tcStart`.
//! - Each sequence is a `spine`: V1 edits, with `gap`s over the holes up to the sequence end.
//!   Edits on V2+ and audio-only edits hang off the spine element covering their start, as
//!   connected clips on `lane` = track - 1 (picture) or -track (audio).
//! - Every stream is an `asset-clip`. An edit with picture is its picture clip with each audio
//!   track connected below it on lane -track, so picture and sound move together. DaVinci
//!   Resolve 20 imports this form; it rejects sequences built from `clip` with nested
//!   `video`/`audio` ("No valid sequences", User test 2026-10-27).
//!
//! Not written yet, reported as losses: volume keys, pans, track levels, stereo grouping,
//! crossfades, speed, time remapping, transform, program-specific effects, marker colours, item
//! markers' colours, drop-frame flags. FCPXML has no tracks; lanes are how Resolve lays them out.
//! Read-back: Resolve imported the asset-clip form at the right times (User, 2026-10-27).

use std::collections::HashMap;

use graph::{Graph, GraphError, Item, Loss, Marker, Media, Placement, Rational, Sequence, StreamKind};

pub type R<T> = Result<T, GraphError>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(GraphError(msg.into()))
}

pub const FCPXML_VERSION: &str = "1.9";

// --- a small XML element tree --------------------------------------------------------------------

struct El {
    tag: &'static str,
    attrs: Vec<(&'static str, String)>,
    children: Vec<El>,
    /// Written after `children`: the DTD puts markers after story and anchored elements.
    markers: Vec<El>,
}

impl El {
    fn new(tag: &'static str, attrs: Vec<(&'static str, String)>) -> El {
        El { tag, attrs, children: Vec::new(), markers: Vec::new() }
    }

    fn with(mut self, child: El) -> El {
        self.children.push(child);
        self
    }

    fn write(&self, depth: usize, out: &mut String) {
        out.push_str(&"    ".repeat(depth));
        out.push('<');
        out.push_str(self.tag);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            for c in v.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\n' => out.push_str("&#10;"),
                    '\r' => out.push_str("&#13;"),
                    '\t' => out.push_str("&#9;"),
                    c if (c as u32) < 0x20 => {}
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        if self.children.is_empty() && self.markers.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push_str(">\n");
        for c in self.children.iter().chain(&self.markers) {
            c.write(depth + 1, out);
        }
        out.push_str(&"    ".repeat(depth));
        out.push_str("</");
        out.push_str(self.tag);
        out.push_str(">\n");
    }
}

/// FCPXML rational seconds: `0s`, `5s`, `1001/30000s`.
fn secs(t: Rational) -> String {
    format!("{t}s")
}

fn add(a: Rational, b: Rational) -> R<Rational> {
    a.checked_add(b).ok_or_else(|| GraphError("time out of range".into()))
}

fn sub(a: Rational, b: Rational) -> R<Rational> {
    a.checked_sub(b).ok_or_else(|| GraphError("time out of range".into()))
}

/// A POSIX or Windows absolute path as a `file://` URL (RFC 8089), bytes outside the unreserved
/// set percent-encoded.
fn file_url(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut out = String::from("file://");
    if !unified.starts_with('/') {
        out.push('/'); // C:/... -> file:///C:/...
    }
    for &b in unified.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~' | b'/' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn audio_rate(hz: i64) -> Option<&'static str> {
    Some(match hz {
        32000 => "32k",
        44100 => "44.1k",
        48000 => "48k",
        88200 => "88.2k",
        96000 => "96k",
        176400 => "176.4k",
        192000 => "192k",
        _ => return None,
    })
}

// --- writing --------------------------------------------------------------------------------------

struct Writer<'g> {
    g: &'g Graph,
    media: HashMap<&'g str, &'g Media>,
    items: HashMap<&'g str, &'g Item>,
    /// (width, height, rate) -> format id.
    formats: Vec<((i64, i64, Rational), String)>,
    /// media id -> asset id.
    assets: HashMap<&'g str, String>,
    resources: Vec<El>,
    next: u32,
    lost: Vec<Loss>,
}

impl<'g> Writer<'g> {
    fn mint(&mut self) -> String {
        self.next += 1;
        format!("r{}", self.next)
    }

    fn loss(&mut self, object: &str, aspect: &str, reason: impl Into<String>) {
        self.lost.push(Loss { object: object.to_string(), aspect: aspect.to_string(), reason: reason.into() });
    }

    fn format(&mut self, w: i64, h: i64, rate: Rational) -> R<String> {
        if let Some((_, id)) = self.formats.iter().find(|(k, _)| *k == (w, h, rate)) {
            return Ok(id.clone());
        }
        if rate.num() <= 0 {
            return fail(format!("frame rate {rate} is not positive"));
        }
        let frame = Rational::new(rate.den() as i128, rate.num() as i128).expect("positive rate");
        let id = self.mint();
        self.resources.push(El::new(
            "format",
            vec![("id", id.clone()), ("frameDuration", secs(frame)), ("width", w.to_string()), ("height", h.to_string())],
        ));
        self.formats.push(((w, h, rate), id.clone()));
        Ok(id)
    }

    fn asset(&mut self, item: &Item, media_id: &'g str) -> R<String> {
        if let Some(id) = self.assets.get(media_id) {
            return Ok(id.clone());
        }
        let m = self.media[media_id];
        let Some(location) = m.locations.first() else { return fail(format!("media {:?}: no location", m.id)) };
        let id = self.mint();
        let name = match (m.kind, m.stream) {
            (StreamKind::Video, _) => item.name.clone(),
            (StreamKind::Audio, Some(s)) => format!("{} A{s}", item.name),
            (StreamKind::Audio, None) => format!("{} audio", item.name),
        };
        let mut attrs = vec![("id", id.clone()), ("name", name), ("start", secs(m.start)), ("duration", secs(m.duration))];
        match m.kind {
            StreamKind::Video => {
                let (Some((w, h)), Some(rate)) = (m.frame_size, m.rate) else {
                    return fail(format!("media {:?}: picture without frame size or rate", m.id));
                };
                let format = self.format(w, h, rate)?;
                attrs.extend([("hasVideo", "1".to_string()), ("hasAudio", "0".to_string()), ("format", format)]);
            }
            StreamKind::Audio => {
                attrs.extend([("hasVideo", "0".to_string()), ("hasAudio", "1".to_string()), ("audioSources", "1".to_string()), ("audioChannels", "1".to_string())]);
                match m.sample_rate.and_then(audio_rate) {
                    Some(r) => attrs.push(("audioRate", r.to_string())),
                    None => self.loss(&m.id, "media", format!("sample rate {:?} has no FCPXML value", m.sample_rate)),
                }
            }
        }
        // No `uid`: it names one media file, and an item's streams (V01, A01, A02) share the
        // item's UMID, so it would merge them.
        let el = El::new("asset", attrs).with(El::new("media-rep", vec![("kind", "original-media".into()), ("src", file_url(location))]));
        self.resources.push(el);
        self.assets.insert(media_id, id.clone());
        Ok(id)
    }

    /// The item's picture stream and its audio streams (by stream number, in item order).
    fn streams(&self, item: &'g Item) -> (Option<&'g Media>, Vec<&'g Media>) {
        let mut video = None;
        let mut audio = Vec::new();
        for im in &item.media {
            let m = self.media[im.media.as_str()];
            match m.kind {
                StreamKind::Video if video.is_none() => video = Some(m),
                StreamKind::Video => {}
                StreamKind::Audio => audio.push(m),
            }
        }
        (video, audio)
    }

    fn marker_el(&mut self, owner: &str, m: &Marker, local: Rational) -> El {
        if m.color.is_some() {
            self.loss(owner, "marker_color", format!("marker {:?}: FCPXML 1.9 markers have no colour", m.name));
        }
        let mut attrs = vec![("start", secs(local)), ("duration", secs(if m.duration.num() > 0 { m.duration } else { Rational::ZERO })), ("value", m.name.clone())];
        if !m.comment.is_empty() {
            attrs.push(("note", m.comment.clone()));
        }
        El::new("marker", attrs)
    }

    /// The edit's picture stream (if it has a picture track) and its audio as (sequence track,
    /// stream, the track's own gain): routed streams, else the item's audio in order; a
    /// multichannel group plays its streams on consecutive tracks.
    #[allow(clippy::type_complexity)]
    fn edit_streams(&mut self, p: &Placement) -> R<(Option<&'g Media>, Vec<(i64, &'g Media, Option<f64>)>)> {
        let item = self.items[p.item.as_str()];
        let (video, audio) = self.streams(item);
        self.placement_losses(p);
        let mut tracks = Vec::new();
        for (i, track) in p.audio_tracks.iter().enumerate() {
            let stream = match p.audio_streams.get(i) {
                Some(s) => audio.iter().find(|m| m.stream == Some(*s)).copied(),
                None => audio.get(i).copied(),
            };
            match stream {
                Some(m) => tracks.push((*track, m, p.track_gains_db.get(i).copied().flatten())),
                None => self.loss(&p.id, "audio", format!("audio track A{track}: no item stream to play")),
            }
        }
        for group in &p.grouped_audio {
            self.loss(&p.id, "audio", format!("A{}: {}-channel group written as separate mono clips", group.track, group.streams.len()));
            for (k, s) in group.streams.iter().enumerate() {
                if let Some(m) = audio.iter().find(|m| m.stream == Some(*s)).copied() {
                    tracks.push((group.track + k as i64, m, group.gain_db));
                }
            }
        }
        let picture = match (p.video_track, video) {
            (Some(_), Some(v)) => Some(v),
            (Some(_), None) => return fail(format!("{}: picture edit of an item without picture", p.id)),
            (None, _) => None,
        };
        if picture.is_none() && tracks.is_empty() {
            return fail(format!("{}: no picture and no playable audio", p.id));
        }
        Ok((picture, tracks))
    }

    /// One stream of an edit as an `asset-clip` (DaVinci Resolve imports these; it rejects a
    /// sequence built from `clip` elements with nested `video`/`audio`, User test 2026-10-27).
    fn stream_clip(&mut self, p: &Placement, m: &'g Media, offset: Rational, lane: Option<i64>, track_gain: Option<f64>) -> R<El> {
        let item = self.items[p.item.as_str()];
        let asset = self.asset(item, m.id.as_str())?;
        let name = match m.kind {
            StreamKind::Video => item.name.clone(),
            StreamKind::Audio => m.stream.map_or_else(|| item.name.clone(), |s| format!("{} A{s}", item.name)),
        };
        let mut attrs = vec![("ref", asset)];
        if let Some(l) = lane {
            attrs.push(("lane", l.to_string()));
        }
        attrs.extend([("name", name), ("offset", secs(offset)), ("start", secs(add(m.start, p.source_in)?)), ("duration", secs(p.duration))]);
        let mut el = El::new("asset-clip", attrs);
        if m.kind == StreamKind::Audio {
            let db = p.gain_db + p.volume_db.unwrap_or(0.0) + track_gain.unwrap_or(0.0);
            if db != 0.0 {
                el.children.push(El::new("adjust-volume", vec![("amount", format!("{db}dB"))]));
            }
        }
        Ok(el)
    }

    /// Markers of an edit, in the clip's own timeline (its media time at the record start).
    fn edit_markers(&mut self, p: &Placement, clip_start: Rational, markers: &[&Marker]) -> R<Vec<El>> {
        let mut out = Vec::new();
        for m in markers {
            let local = add(clip_start, sub(m.start, p.record_start)?)?;
            out.push(self.marker_el(&p.id, m, local));
        }
        Ok(out)
    }

    /// An edit with picture: the picture clip with each audio track connected below it (lane
    /// -track), so picture and sound move together. `lane` = None on the spine.
    fn picture_edit(&mut self, p: &Placement, v: &'g Media, tracks: &[(i64, &'g Media, Option<f64>)], offset: Rational, lane: Option<i64>, markers: &[&Marker]) -> R<El> {
        let mut clip = self.stream_clip(p, v, offset, lane, None)?;
        let local = add(v.start, p.source_in)?;
        for (track, m, gain) in tracks {
            let el = self.stream_clip(p, m, local, Some(-*track), *gain)?;
            clip.children.push(el);
        }
        clip.markers = self.edit_markers(p, local, markers)?;
        Ok(clip)
    }

    fn placement_losses(&mut self, p: &Placement) {
        let id = p.id.clone();
        if !p.volume_keys.is_empty() {
            self.loss(&id, "audio", format!("{} volume keyframes (constant gain only)", p.volume_keys.len()));
        }
        if p.track_pans.iter().chain(p.grouped_audio.iter().flat_map(|g| &g.pans)).any(|x| x.is_some()) {
            self.loss(&id, "audio", "pan");
        }
        if p.speed.is_some() {
            self.loss(&id, "speed", "constant speed change");
        }
        if p.frame_map.is_some() {
            self.loss(&id, "speed", "time remapping");
        }
        if p.transform.is_some() {
            self.loss(&id, "transform", "position/scale/rotation/crop");
        }
        for e in &p.effects {
            self.loss(&id, "effects", format!("effect {}", e.kind));
        }
    }

    fn sequence(&mut self, s: &'g Sequence) -> R<El> {
        let format = self.format(s.frame_size.0, s.frame_size.1, s.rate)?;
        let mut end = Rational::ZERO;
        for p in &s.placements {
            end = end.max(add(p.record_start, p.duration)?);
        }
        for t in &s.tracks {
            if t.level_db.is_some_and(|d| d != 0.0) {
                self.loss(&s.id, "tracks", format!("track level on {}{}", if t.kind == StreamKind::Video { "V" } else { "A" }, t.index));
            }
        }
        for x in &s.transitions {
            self.loss(&s.id, "transitions", format!("{} transition on track {} ({})", x.kind.as_str(), x.track, x.style));
        }
        // Markers on an edit go into its clip; the rest onto the spine element covering them.
        let mut on_edit: HashMap<&str, Vec<&Marker>> = HashMap::new();
        let mut on_spine = Vec::new();
        for m in &s.markers {
            match &m.placement {
                Some(pid) => on_edit.entry(pid.as_str()).or_default().push(m),
                None => on_spine.push(m),
            }
        }
        // The spine: V1 placements in time order, gaps between and to the end.
        let mut primary: Vec<&Placement> = s.placements.iter().filter(|p| p.video_track == Some(1)).collect();
        primary.sort_by_key(|p| p.record_start);
        // Spine offsets count from the sequence's timecode origin (tcStart); the bookkeeping
        // below stays in record time from 0 and `origin` is added where offsets are written.
        let origin = s.start;
        let mut spine: Vec<(Rational, Rational, El)> = Vec::new();
        let mut at = Rational::ZERO;
        for p in primary {
            if p.record_start < at {
                self.loss(&p.id, "placement", "overlaps the previous V1 edit; written from where that one ends");
            }
            if p.record_start > at {
                spine.push((at, p.record_start, El::new("gap", vec![("name", "Gap".into()), ("offset", secs(add(origin, at)?)), ("start", "0s".into()), ("duration", secs(sub(p.record_start, at)?))])));
            }
            let marks = on_edit.remove(p.id.as_str()).unwrap_or_default();
            let (picture, tracks) = self.edit_streams(p)?;
            let el = self.picture_edit(p, picture.expect("V1 edit has a picture"), &tracks, add(origin, p.record_start.max(at))?, None, &marks)?;
            let stop = add(p.record_start, p.duration)?;
            spine.push((p.record_start.max(at), stop, el));
            at = at.max(stop);
        }
        if end > at {
            spine.push((at, end, El::new("gap", vec![("name", "Gap".into()), ("offset", secs(add(origin, at)?)), ("start", "0s".into()), ("duration", secs(sub(end, at)?))])));
        }
        // Connected clips: V2+ and audio-only edits, on the spine element covering their start.
        for p in &s.placements {
            if p.video_track == Some(1) {
                continue;
            }
            let Some(host) = spine.iter().position(|(a, b, _)| *a <= p.record_start && p.record_start < *b) else {
                self.loss(&p.id, "placement", "starts where the sequence has no spine (empty sequence)");
                continue;
            };
            let marks = on_edit.remove(p.id.as_str()).unwrap_or_default();
            // Inside the host, time is the host's own: start attr + (record - host offset).
            let offset = add(host_local_start(&spine[host].2)?, sub(p.record_start, spine[host].0)?)?;
            let (picture, tracks) = self.edit_streams(p)?;
            match (picture, p.video_track) {
                (Some(v), Some(vt)) => {
                    let el = self.picture_edit(p, v, &tracks, offset, Some(vt - 1), &marks)?;
                    spine[host].2.children.push(el);
                }
                _ => {
                    // Audio only: one connected clip per track; markers on the first.
                    for (k, (track, m, gain)) in tracks.iter().enumerate() {
                        let mut el = self.stream_clip(p, m, offset, Some(-*track), *gain)?;
                        if k == 0 {
                            el.markers = self.edit_markers(p, add(m.start, p.source_in)?, &marks)?;
                        }
                        spine[host].2.children.push(el);
                    }
                }
            }
        }
        for m in on_spine {
            match spine.iter().position(|(a, b, _)| *a <= m.start && m.start < *b) {
                Some(host) => {
                    let local = add(host_local_start(&spine[host].2)?, sub(m.start, spine[host].0)?)?;
                    let el = self.marker_el(&s.id, m, local);
                    spine[host].2.markers.push(el);
                }
                None => self.loss(&s.id, "markers", format!("marker {:?} lies past the sequence end", m.name)),
            }
        }
        let tc_start = s.start;
        let mut spine_el = El::new("spine", vec![]);
        spine_el.children = spine.into_iter().map(|(_, _, e)| e).collect();
        let seq = El::new(
            "sequence",
            vec![("format", format), ("duration", secs(end)), ("tcStart", secs(tc_start)), ("tcFormat", "NDF".into()), ("audioLayout", "mono".into()), ("audioRate", "48k".into())],
        )
        .with(spine_el);
        Ok(El::new("project", vec![("name", s.name.clone())]).with(seq))
    }

    /// An item in a bin: one `asset-clip` per stream (FCPXML has no multi-file browser clip that
    /// Resolve imports), the item's markers on the first.
    fn browser_clips(&mut self, item: &'g Item) -> R<Vec<El>> {
        let (video, audio) = self.streams(item);
        let streams: Vec<&'g Media> = video.into_iter().chain(audio).collect();
        if streams.len() > 1 {
            self.loss(&item.id, "item", format!("{} media files written as {} separate bin clips", streams.len(), streams.len()));
        }
        let mut out = Vec::new();
        for (k, m) in streams.iter().enumerate() {
            let asset = self.asset(item, m.id.as_str())?;
            let name = match m.kind {
                StreamKind::Video => item.name.clone(),
                StreamKind::Audio => m.stream.map_or_else(|| item.name.clone(), |s| format!("{} A{s}", item.name)),
            };
            let mut el = El::new("asset-clip", vec![("ref", asset), ("name", name), ("start", secs(m.start)), ("duration", secs(m.duration))]);
            if k == 0 {
                for mk in &item.markers {
                    let local = add(m.start, mk.start)?;
                    let marker = self.marker_el(&item.id, mk, local);
                    el.markers.push(marker);
                }
            }
            out.push(el);
        }
        Ok(out)
    }
}

/// The `start` attribute of a spine element (its own timeline's origin).
fn host_local_start(el: &El) -> R<Rational> {
    let text = el.attrs.iter().find(|(k, _)| *k == "start").map(|(_, v)| v.as_str()).unwrap_or("0s");
    Rational::parse(text.trim_end_matches('s')).ok_or_else(|| GraphError(format!("internal: bad start {text}")))
}

/// Write the graph as an FCPXML 1.9 document. Returns the document and what it could not hold.
pub fn write(g: &Graph) -> R<(String, Vec<Loss>)> {
    g.validate()?;
    let mut w = Writer {
        g,
        media: g.media.iter().map(|m| (m.id.as_str(), m)).collect(),
        items: g.items.iter().map(|i| (i.id.as_str(), i)).collect(),
        formats: Vec::new(),
        assets: HashMap::new(),
        resources: Vec::new(),
        next: 0,
        lost: Vec::new(),
    };
    let mut library = El::new("library", vec![]);
    for b in &w.g.bins {
        if b.parent.is_some() {
            w.loss(&b.id, "hierarchy", "nested bin written as a top-level event");
        }
        let mut event = El::new("event", vec![("name", b.name.clone())]);
        for id in &b.items {
            let item = w.items[id.as_str()];
            let clips = w.browser_clips(item)?;
            event.children.extend(clips);
        }
        library.children.push(event);
    }
    // Not the bare project name: a bin often has it too, and two events would share a name.
    let mut projects = El::new("event", vec![("name", format!("{} sequences", w.g.project.name))]);
    for s in &w.g.sequences {
        let p = w.sequence(s)?;
        projects.children.push(p);
    }
    if !projects.children.is_empty() {
        library.children.push(projects);
    }
    let mut resources = El::new("resources", vec![]);
    resources.children = std::mem::take(&mut w.resources);
    let root = El::new("fcpxml", vec![("version", FCPXML_VERSION.into())]).with(resources).with(library);
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n\n");
    root.write(0, &mut out);
    // Losses the graph already carries stay losses for this target too.
    let mut lost = g.losses.clone();
    lost.extend(w.lost);
    Ok((out, lost))
}
