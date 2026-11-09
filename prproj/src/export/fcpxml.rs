//! FCPXML export (port of `py_premiere/export/fcpxml.py`), byte-identical to the Python output.
//!
//! **Premiere cannot read what this writes.** It reads Final Cut Pro 7 XML (`<xmeml>`); this
//! emits FCPXML (`<fcpxml>`), which Resolve and FCP X read. A one-way export.
//!
//! Times are rationals in seconds (`N/Ds`); a tick is 1/254016000000 s, so every tick count
//! converts exactly. Per sequence: `resources` (one `format`, one `asset` per media file),
//! a `spine` (the first video track, `gap`s over the holes) and every other track's clips as
//! connected clips hung off the spine element covering their start.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::build::fmt::py_repr;
use crate::build::time::TICKS_PER_SECOND;
use crate::convert::model::py_str_repr;
use crate::error::{value, Error, Result};
use crate::model::project::py_path;
use crate::model::{Project, SequenceId, TrackItemId};

/// The DTD version the output declares.
pub const FCPXML_VERSION: &str = "1.9";

/// `MZ.Sequence.VideoTimeDisplayFormat` values that mean drop-frame timecode.
const DROP_FRAME_DISPLAY_FORMATS: [i64; 1] = [102];

/// `_seconds`: a tick count as an FCPXML rational-seconds string.
fn seconds(ticks: i128) -> String {
    if ticks == 0 {
        return "0s".into();
    }
    let tps = TICKS_PER_SECOND as i128;
    let (mut a, mut b) = (ticks.unsigned_abs(), tps as u128);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let g = a as i128;
    let (num, den) = (ticks / g, tps / g);
    if den == 1 {
        format!("{num}s")
    } else {
        format!("{num}/{den}s")
    }
}

/// `Path.absolute().as_uri()` for a POSIX path.
fn file_url(path: &str) -> String {
    let normalized = py_path(path).to_string_lossy().into_owned();
    let absolute = if normalized.starts_with('/') {
        normalized
    } else {
        let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        py_path(&format!("{cwd}/{normalized}")).to_string_lossy().into_owned()
    };
    // pathname2url: root ("/" or "//") gets an explicit empty authority; the tail is quoted.
    let (root, tail) = if absolute.starts_with("//") && !absolute.starts_with("///") {
        ("//", &absolute[2..])
    } else {
        ("/", absolute.trim_start_matches('/'))
    };
    // posixpath.splitroot keeps the extra slashes of 3+ in the tail; py_path never produces them.
    let mut out = String::from("file://");
    out.push_str(root);
    for &b in tail.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A minimal element: ordered attributes, children by arena index.
struct El {
    tag: &'static str,
    attrs: Vec<(&'static str, String)>,
    children: Vec<usize>,
}

struct Arena(Vec<El>);

impl Arena {
    fn new_el(&mut self, tag: &'static str, attrs: Vec<(&'static str, String)>) -> usize {
        self.0.push(El { tag, attrs, children: Vec::new() });
        self.0.len() - 1
    }

    fn sub(&mut self, parent: usize, tag: &'static str, attrs: Vec<(&'static str, String)>) -> usize {
        let id = self.new_el(tag, attrs);
        self.0[parent].children.push(id);
        id
    }

    /// `element.set(key, value)`: replaces in place or appends.
    fn set(&mut self, el: usize, key: &'static str, val: String) {
        let attrs = &mut self.0[el].attrs;
        match attrs.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = val,
            None => attrs.push((key, val)),
        }
    }

    /// `ET.tostring(root, encoding="unicode")` after `_indent`.
    fn write(&self, el: usize, depth: usize, out: &mut String) {
        let e = &self.0[el];
        out.push('<');
        out.push_str(e.tag);
        for (k, v) in &e.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            for c in v.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\r' => out.push_str("&#13;"),
                    '\n' => out.push_str("&#10;"),
                    '\t' => out.push_str("&#09;"),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        if e.children.is_empty() {
            out.push_str(" />");
            return;
        }
        out.push('>');
        for &child in &e.children {
            out.push('\n');
            out.push_str(&"    ".repeat(depth + 1));
            self.write(child, depth + 1, out);
        }
        out.push('\n');
        out.push_str(&"    ".repeat(depth));
        out.push_str("</");
        out.push_str(e.tag);
        out.push('>');
    }
}

/// `_Asset`: one emitted `<asset>`, its id and the span it must cover.
struct Asset {
    identifier: String,
    element: usize,
    span: i128,
}

struct Resources {
    element: usize,
    by_path: HashMap<String, usize>,
    assets: Vec<Asset>,
    next: u32,
}

impl Resources {
    fn mint(&mut self) -> String {
        let id = format!("r{}", self.next);
        self.next += 1;
        id
    }

    fn add_format(&mut self, a: &mut Arena, p: &Project, sequence: SequenceId) -> Result<String> {
        let (size, timebase) = (p.frame_size(sequence), p.timebase(sequence));
        let (Some((width, height)), Some(timebase)) = (size, timebase) else {
            return value("sequence has no frame size or timebase to derive a format from");
        };
        let identifier = self.mint();
        a.sub(
            self.element,
            "format",
            vec![
                ("id", identifier.clone()),
                ("name", format!("FFVideoFormat{height}p")),
                ("frameDuration", seconds(timebase as i128)),
                ("width", width.to_string()),
                ("height", height.to_string()),
            ],
        );
        Ok(identifier)
    }

    fn add_asset(&mut self, a: &mut Arena, p: &Project, clip: TrackItemId, format_id: &str, has_video: bool) -> Result<String> {
        let item = p.track_item_source(clip);
        let path = item.and_then(|i| p.item_media_path(i));
        let (Some(item), Some(path)) = (item, path) else {
            // Synthetic media and nested sequences have no file to point at.
            return Ok(String::new());
        };
        let path = py_path(path).to_string_lossy().into_owned();
        // An asset has to span every clip that reads from it (stills sit inside a 12-hour
        // phantom source).
        let needed = p.in_point(clip)?.ticks as i128 + p.duration(clip)?.ticks as i128;
        if let Some(&index) = self.by_path.get(&path) {
            let asset = &mut self.assets[index];
            if needed > asset.span {
                asset.span = needed;
                a.set(asset.element, "duration", seconds(needed));
            }
            // The asset is shared, so it declares every stream actually used.
            a.set(asset.element, if has_video { "hasVideo" } else { "hasAudio" }, "1".into());
            return Ok(asset.identifier.clone());
        }
        let identifier = self.mint();
        let element = a.sub(
            self.element,
            "asset",
            vec![
                ("id", identifier.clone()),
                ("name", p.item_name(item)),
                ("start", "0s".into()),
                ("duration", seconds(needed.max(0))),
                ("hasVideo", if has_video { "1" } else { "0" }.into()),
                ("hasAudio", if has_video { "0" } else { "1" }.into()),
                ("format", format_id.to_string()),
            ],
        );
        a.sub(element, "media-rep", vec![("kind", "original-media".into()), ("src", file_url(&path))]);
        self.by_path.insert(path, self.assets.len());
        self.assets.push(Asset { identifier: identifier.clone(), element, span: needed });
        Ok(identifier)
    }
}

fn clip_element(a: &mut Arena, p: &Project, clip: TrackItemId, reference: &str, offset: i128, lane: Option<i64>) -> Result<usize> {
    let mut attrs = vec![
        ("ref", reference.to_string()),
        ("name", p.track_item_name(clip)?),
        ("offset", seconds(offset)),
        ("start", seconds(p.in_point(clip)?.ticks as i128)),
        ("duration", seconds(p.duration(clip)?.ticks as i128)),
    ];
    if let Some(lane) = lane {
        attrs.push(("lane", lane.to_string()));
    }
    Ok(a.new_el("asset-clip", attrs))
}

/// `placed`: (start, end, element) of every spine element a connected clip can hang off.
type Placed = Vec<(i128, i128, usize)>;

struct Ctx<'a> {
    a: &'a mut Arena,
    p: &'a Project,
    resources: &'a mut Resources,
    format_id: &'a str,
}

fn connect(cx: &mut Ctx, clips: &[TrackItemId], placed: &Placed, lane: i64, has_video: bool) -> Result<()> {
    for &clip in clips {
        let reference = cx.resources.add_asset(cx.a, cx.p, clip, cx.format_id, has_video)?;
        if reference.is_empty() {
            continue;
        }
        let start = cx.p.start(clip)?.ticks as i128;
        let host = placed.iter().find(|(s, e, _)| *s <= start && start < *e);
        let Some(&(host_start, _, host)) = host else {
            // The format has no way to place a connected clip there.
            return Err(Error::NotImplemented(format!(
                "clip {} at {}s has no spine element to attach to - the primary video track must cover every connected clip's start",
                py_str_repr(&cx.p.track_item_name(clip)?),
                py_repr(cx.p.start(clip)?.seconds())
            )));
        };
        let element = clip_element(cx.a, cx.p, clip, &reference, start - host_start, Some(lane))?;
        cx.a.0[host].children.push(element);
    }
    Ok(())
}

fn build_spine(a: &mut Arena, p: &Project, sequence: SequenceId, resources: &mut Resources, format_id: &str) -> Result<usize> {
    let spine = a.new_el("spine", vec![]);
    let video = p.video_tracks(sequence);
    let primary: Vec<TrackItemId> = video.first().map(|&t| p.track_clips(t).to_vec()).unwrap_or_default();
    let mut keyed = Vec::with_capacity(primary.len());
    for clip in primary {
        keyed.push((p.start(clip)?.ticks, clip));
    }
    keyed.sort_by_key(|(start, _)| *start);
    let mut position: i128 = 0;
    let mut placed: Placed = Vec::new();
    for (_, clip) in keyed {
        let clip_start = p.start(clip)?.ticks as i128;
        if clip_start > position {
            let gap = a.sub(
                spine,
                "gap",
                vec![("name", "Gap".into()), ("offset", seconds(position)), ("duration", seconds(clip_start - position))],
            );
            placed.push((position, clip_start, gap));
        }
        let reference = resources.add_asset(a, p, clip, format_id, true)?;
        if !reference.is_empty() {
            let element = clip_element(a, p, clip, &reference, clip_start, None)?;
            a.0[spine].children.push(element);
            placed.push((clip_start, p.end(clip)?.ticks as i128, element));
        }
        position = position.max(p.end(clip)?.ticks as i128);
    }
    // The spine spans the whole sequence: audio routinely runs past the last video clip.
    let sequence_end = p.sequence_end(sequence)?.ticks as i128;
    if sequence_end > position {
        let gap = a.sub(
            spine,
            "gap",
            vec![("name", "Gap".into()), ("offset", seconds(position)), ("duration", seconds(sequence_end - position))],
        );
        placed.push((position, sequence_end, gap));
    }
    let mut cx = Ctx { a, p, resources, format_id };
    let mut lane = 0;
    for &track in video.iter().skip(1) {
        lane += 1;
        connect(&mut cx, p.track_clips(track), &placed, lane, true)?;
    }
    lane = 0;
    for &track in p.audio_tracks(sequence) {
        lane -= 1;
        connect(&mut cx, p.track_clips(track), &placed, lane, false)?;
    }
    Ok(spine)
}

/// `export_fcpxml` without the file: the document bytes for `sequence` (default: the first).
pub fn export_fcpxml_bytes(project: &Project, sequence: Option<SequenceId>) -> Result<Vec<u8>> {
    let sequence = match sequence {
        Some(s) => s,
        None => match project.sequences().first() {
            Some(&s) => s,
            None => return value("project has no sequence to export"),
        },
    };
    let mut a = Arena(Vec::new());
    let resources_el = a.new_el("resources", vec![]);
    let mut resources = Resources { element: resources_el, by_path: HashMap::new(), assets: Vec::new(), next: 1 };
    let format_id = resources.add_format(&mut a, project, sequence)?;
    let spine = build_spine(&mut a, project, sequence, &mut resources, &format_id)?;

    let root = a.new_el("fcpxml", vec![("version", FCPXML_VERSION.into())]);
    a.0[root].children.push(resources_el);
    let library = a.sub(root, "library", vec![]);
    let event = a.sub(library, "event", vec![("name", project.name())]);
    let project_el = a.sub(event, "project", vec![("name", project.sequence_name(sequence))]);
    let duration = seconds(project.sequence_end(sequence)?.ticks as i128);
    let drop_frame = project
        .video_display_format(sequence)?
        .is_some_and(|f| DROP_FRAME_DISPLAY_FORMATS.contains(&f));
    let sequence_el = a.sub(
        project_el,
        "sequence",
        vec![
            ("format", format_id),
            ("duration", duration),
            ("tcStart", "0s".into()),
            ("tcFormat", if drop_frame { "DF" } else { "NDF" }.into()),
        ],
    );
    a.0[sequence_el].children.push(spine);

    let mut text = String::new();
    a.write(root, 0, &mut text);
    let mut out = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n".to_vec();
    out.extend_from_slice(text.as_bytes());
    Ok(out)
}

/// `export_fcpxml(project, path, sequence)`: write `sequence` (default: the first) to `path`.
/// Returns the path written. Fails with `NotImplementedError` for a timeline whose connected
/// clips cannot be expressed.
pub fn export_fcpxml(project: &Project, path: impl AsRef<Path>, sequence: Option<SequenceId>) -> Result<PathBuf> {
    let bytes = export_fcpxml_bytes(project, sequence)?;
    let target = path.as_ref();
    std::fs::write(target, bytes).map_err(|e| crate::cli::os_error(target, &e))?;
    Ok(target.to_path_buf())
}
