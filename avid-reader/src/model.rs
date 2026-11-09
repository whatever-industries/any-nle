//! Emit the converter's JSON model (`avid2prproj.model/1`, docs/ARCHITECTURE.md ADR-0001)
//! from an extracted bin plus an MXF identity index of the media folder.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::extract::{format_timecode, AvidMarker, BinContents, Essence, MarkerOwner, MasterClip, SeqEvent, SequenceInfo, KIND_PICTURE, KIND_SOUND};
use crate::io::MobId;
use crate::json::Json;
use crate::mxf;

pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

/// File-package UMID (64 hex) -> MXF path, keeping only UMIDs that name exactly one file.
#[derive(Debug, Default)]
pub struct MediaIndex {
    pub by_file_package: HashMap<String, PathBuf>,
    /// MXF start timecode per file, in ticks (what Premiere stores as the media Start).
    pub start_ticks: HashMap<PathBuf, i64>,
    /// MXF file-package essence duration per file, in ticks: the length Premiere's own
    /// import uses (PRPROJ-0018). Avid's descriptor length can be shorter.
    pub duration_ticks: HashMap<PathBuf, i64>,
    /// Unambiguous picture-coding UL from each MXF header.
    pub picture_coding: HashMap<PathBuf, [u8; 16]>,
    pub ambiguous: Vec<String>,
    pub unreadable: Vec<(PathBuf, String)>,
    pub files_scanned: usize,
}

impl MediaIndex {
    pub fn scan(root: &Path) -> crate::io::Result<MediaIndex> {
        Self::scan_all(&[root])
    }

    /// Several media folders (Avid media often spans drives): one index; a file package found
    /// in more than one file, in any of the folders, is ambiguous as before. Repeated roots
    /// and files reached through overlapping selections are scanned once.
    pub fn scan_all(roots: &[&Path]) -> crate::io::Result<MediaIndex> {
        let mut index = MediaIndex::default();
        let mut seen: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut scanned_roots = HashSet::new();
        let mut scanned_files = HashSet::new();
        for root in roots {
        // The model requires absolute media paths.
        let root = std::fs::canonicalize(root).map_err(|e| crate::io::Error::new(format!("{}: {}", root.display(), e), 0))?;
        if !scanned_roots.insert(root.clone()) { continue; }
        for path in mxf::find_mxf_files(&root, 6, 200_000)? {
            // Canonical roots and non-symlink traversal give the same absolute file path
            // under overlapping selections. Count/read it once, including failed headers.
            if !scanned_files.insert(path.clone()) { continue; }
            index.files_scanned += 1;
            match mxf::read_identity(&path) {
                Ok(mut id) => {
                    if let Some(coding) = id.picture_essence_coding {
                        index.picture_coding.insert(path.clone(), coding);
                    }
                    if let Some((frames, base, drop)) = id.start_timecode() {
                        // Non-drop counts frames at the integer base; drop-frame is not handled yet.
                        if !drop && base > 0 {
                            if let Some(t) = ticks(frames as i64, (base as i64, 1)) {
                                index.start_ticks.insert(path.clone(), t);
                            }
                        }
                    }
                    if let [(num, den, len)] = id.file_tracks.as_slice() {
                        if *num > 0 && *den > 0 {
                            if let Some(t) = ticks(*len, (*num, *den)) {
                                index.duration_ticks.insert(path.clone(), t);
                            }
                        }
                    }
                    // Ambiguity counts distinct files, not repeated references in one header.
                    id.file_packages.sort_unstable();
                    id.file_packages.dedup();
                    for uid in id.file_packages {
                        seen.entry(uid).or_default().push(path.clone());
                    }
                }
                Err(e) => index.unreadable.push((path, e.to_string())),
            }
        }
        }
        for (uid, paths) in seen {
            if paths.len() == 1 {
                index.by_file_package.insert(uid, paths.into_iter().next().unwrap());
            } else {
                index.ambiguous.push(uid);
            }
        }
        index.ambiguous.sort();
        Ok(index)
    }
}

/// Premiere time display code (Clip → Modify → Timecode → Time Display Format; codes from
/// Adobe's CEP sample, SRC-003) matching an Avid rate; `drop` from the source timecode.
pub fn time_display_code(rate: f64, drop: bool) -> Option<i64> {
    match rate_fraction(rate)? {
        (24, 1) => Some(100),
        (25, 1) => Some(101),
        (30000, 1001) => Some(if drop { 102 } else { 103 }),
        (30, 1) => Some(104),
        (50, 1) => Some(105),
        (60000, 1001) => Some(if drop { 106 } else { 107 }),
        (60, 1) => Some(108),
        (24000, 1001) => Some(110),
        (48, 1) => Some(113),
        _ => None,
    }
}

/// Avid edit rates are decimals (24.0, 23.976, 29.97); the model wants exact fractions.
pub fn rate_fraction(rate: f64) -> Option<(i64, i64)> {
    if !(rate > 0.0 && rate < 1000.0) {
        return None;
    }
    if (rate - rate.round()).abs() < 1e-9 {
        return Some((rate.round() as i64, 1));
    }
    let base = (rate * 1.001).round();
    if (rate - base * 1000.0 / 1001.0).abs() < 1e-3 {
        return Some((base as i64 * 1000, 1001));
    }
    None
}

fn ticks(length: i64, (num, den): (i64, i64)) -> Option<i64> {
    let total = (length as i128) * (TICKS_PER_SECOND as i128) * (den as i128);
    if total % (num as i128) != 0 {
        return None;
    }
    i64::try_from(total / num as i128).ok()
}

fn picture_codec(compression: &str, coding: Option<&[u8; 16]>) -> Option<&'static str> {
    match compression {
        "AVHD" => Some("AVdn"),
        "RI00" if coding == Some(&mxf::PRORES_422_LT_CODING) => Some("apcs"),
        "RI00" if coding == Some(&mxf::DNXHR_LB_CODING) => Some("AVdn"),
        _ => None,
    }
}

fn pixel_aspect(display: (i64, i64), raster: (i64, i64)) -> Option<(i128, i128)> {
    let (n, d) = display;
    let (width, height) = raster;
    if n <= 0 || d <= 0 || width <= 0 || height <= 0 {
        return None;
    }
    let mut a = (n as i128) * (height as i128);
    let mut b = (d as i128) * (width as i128);
    let (mut x, mut y) = (a, b);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    a /= x;
    b /= x;
    Some((a, b))
}

pub struct Conversion {
    pub model: Json,
    pub problems: Vec<String>,
}

/// `bin_names[i]` names the i-th bin given to the extractor; each becomes a Premiere bin.
pub fn build(contents: &BinContents, media: &MediaIndex, project: &str, bin_names: &[String], bin_mode: &str) -> Conversion {
    let mut problems: Vec<String> = contents.skipped.iter().map(|s| format!("{:?}: skipped ({})", s.name, s.reason)).collect();
    for (name, bin) in &contents.duplicates {
        problems.push(format!("{:?} is also in bin {:?}; a copy is imported there with that bin's attributes", name, bin_names[*bin]));
    }
    let mut clips = Vec::new();
    let mut ids: Vec<Vec<Json>> = vec![Vec::new(); bin_names.len()];
    // Every converted copy, per master clip ID, in bin order.
    let mut copies: HashMap<MobId, Vec<(String, &MasterClip, Json)>> = HashMap::new();
    // Avid audio tracks (by label) whose media was not found: the clip converts without them.
    let mut missing_audio: HashMap<(MobId, usize), Vec<i64>> = HashMap::new();
    for (n, master) in contents.masters.iter().enumerate() {
        let id = format!("clip-{}", n + 1);
        match clip(master, media, &id) {
            Ok((j, notes, missing)) => {
                if !missing.is_empty() {
                    missing_audio.insert((master.mob_id, master.bin), missing);
                }
                problems.extend(notes.into_iter().map(|n| format!("{:?}: {}", master.name, n)));
                ids[master.bin].push(Json::str(id.clone()));
                copies.entry(master.mob_id).or_default().push((id, master, j.clone()));
                clips.push(j);
            }
            Err(e) => problems.push(format!("{:?}: skipped ({})", master.name, e)),
        }
    }
    for list in copies.values() {
        if list.len() > 1 && list.iter().any(|(_, _, j)| strip_id(j) != strip_id(&list[0].2)) {
            let bins: Vec<&str> = list.iter().map(|(_, m, _)| bin_names[m.bin].as_str()).collect();
            problems.push(format!("{:?}: the copies in bins {:?} differ (columns, grouping or gain)", list[0].1.name, bins));
        }
    }
    let mut sequences = Vec::new();
    for seq in &contents.sequences {
        // A sequence uses the copy in its own bin, else the first bin's copy.
        let converted: HashMap<MobId, (String, &MasterClip)> = copies
            .iter()
            .map(|(mob, list)| {
                let (id, m, _) = list.iter().find(|(_, m, _)| m.bin == seq.bin).unwrap_or(&list[0]);
                (*mob, (id.clone(), *m))
            })
            .collect();
        match sequence(seq, &converted, &missing_audio, &mut problems) {
            Ok(j) => sequences.push(j),
            Err(e) => problems.push(format!("sequence {:?}: skipped ({})", seq.name, e)),
        }
    }
    let bins = bin_names
        .iter()
        .zip(ids)
        .map(|(name, ids)| Json::obj().with("name", Json::str(name.clone())).with("clips", Json::Array(ids)))
        .collect();
    let model = Json::obj()
        .with("schema", Json::str("avid2prproj.model/1"))
        .with("project", Json::obj().with("name", Json::str(project)))
        .with("options", Json::obj().with("bin_mode", Json::str(bin_mode)))
        .with("clips", Json::Array(clips))
        .with("bins", Json::Array(bins))
        .with("sequences", Json::Array(sequences));
    Conversion { model, problems }
}

/// A clip's JSON without its id, for comparing copies.
fn strip_id(j: &Json) -> Json {
    match j {
        Json::Object(entries) => Json::Object(entries.iter().filter(|(k, _)| k != "id").cloned().collect()),
        other => other.clone(),
    }
}

fn ticks_per_frame((num, den): (i64, i64)) -> Option<i64> {
    let t = TICKS_PER_SECOND as i128 * den as i128;
    (t % num as i128 == 0).then(|| (t / num as i128) as i64)
}

/// A marker's own fields; positions are added by the caller.
fn marker_json(m: &AvidMarker, tpf: i64) -> Json {
    let mut j = Json::obj()
        .with("name", Json::str(m.name.clone()))
        .with("comment", Json::str(m.comment.clone()))
        .with("user", Json::str(m.user.clone()))
        .with("color", Json::str(m.color.clone()))
        .with("rgb16", Json::Array(m.rgb16.iter().map(|v| Json::Int(*v)).collect()));
    if let Some(l) = m.length {
        j.push("duration_ticks", Json::Int(l * tpf));
    }
    if !m.id.is_empty() {
        j.push("avid_id", Json::str(m.id.clone()));
    }
    j
}

fn track_name(kind: i64, track: i64) -> String {
    format!("{}{}", if kind == KIND_PICTURE { "V" } else { "A" }, track)
}

/// The already-extracted channels belonging to one edit span. Mono edits stand alone.
fn transition_members(events: &[SeqEvent], anchor: usize) -> Vec<usize> {
    let a = &events[anchor];
    let Some((group, _)) = a.combined else { return vec![anchor] };
    events.iter().enumerate().filter_map(|(i, e)| {
        (e.combined.is_some_and(|(g, _)| g == group)
            && (e.kind, e.track, e.master, e.record_start, e.length, e.source_in, e.speed, e.source_length)
                == (a.kind, a.track, a.master, a.record_start, a.length, a.source_in, a.speed, a.source_length))
            .then_some(i)
    }).collect()
}

fn sequence(
    seq: &SequenceInfo,
    clips: &HashMap<MobId, (String, &MasterClip)>,
    missing_audio: &HashMap<(MobId, usize), Vec<i64>>,
    problems: &mut Vec<String>,
) -> Result<Json, String> {
    let rate = rate_fraction(seq.edit_rate).ok_or(format!("unsupported edit rate {}", seq.edit_rate))?;
    let tpf = ticks_per_frame(rate).ok_or("frame is not a whole number of ticks")?;
    let note = |problems: &mut Vec<String>, what: String| problems.push(format!("sequence {:?}: {}", seq.name, what));
    for p in &seq.problems {
        note(problems, p.clone());
    }

    // Events usable as plain cuts.
    let mut events: Vec<SeqEvent> = Vec::new();
    for e in &seq.events {
        let at = format!("{} at frame {}", track_name(e.kind, e.track), e.record_start);
        let Some((_, master)) = clips.get(&e.master) else {
            note(problems, format!("{}: clip not converted (see above)", at));
            continue;
        };
        if e.kind == KIND_SOUND && missing_audio.get(&(e.master, master.bin)).is_some_and(|m| m.contains(&e.master_track)) {
            note(problems, format!("{}: source audio A{} has no media; this audio is not placed", at, e.master_track));
            continue;
        }
        if let Some((n, d)) = e.speed {
            // KB AVB-0010: speed_ratio describes stored timeline/source spans, not
            // necessarily exact rendered sampling (the observed 200% case stores 50/99).
            // Constant speed only; frame-rate adapters arrive here as clip-rate mismatches.
            if n == 0 || d == 0 {
                note(problems, format!("{}: speed_ratio {}/{} not convertible; event left out", at, n, d));
                continue;
            }
            note(problems, format!("{}: speed {:.2}% from stored Avid span ratio {}/{}{}; approximate playback, Avid frame sampling may differ", at, 100.0 * d as f64 / n.unsigned_abs() as f64, n, d,
                                   if n < 0 { ", reversed" } else { "" }));
        }
        if (master.edit_rate - seq.edit_rate).abs() > 1e-6 {
            note(problems, format!("{}: clip rate {} differs from sequence rate; event left out", at, master.edit_rate));
            continue;
        }
        if !e.via.is_empty() {
            note(problems, format!("{}: placed; {}", at, e.via.join("; ")));
        }
        events.push(e.clone());
    }

    // Transitions overlap the clips around them; Premiere clips abut at the cut instead.
    let mut crossfades = Vec::new();
    let mut seen_transitions = HashSet::new();
    for t in &seq.transitions {
        // Channel walks can describe the same track transition more than once.
        if !seen_transitions.insert((t.kind, t.track, t.position, t.length, t.cutpoint, t.effect.clone())) { continue; }
        let at = format!("{} transition at frame {}", track_name(t.kind, t.track), t.position);
        let cut = t.position.checked_add(t.cutpoint);
        let overlap_end = t.position.checked_add(t.length);
        if t.position < 0 || t.length <= 0 || !(0..=t.length).contains(&t.cutpoint) || cut.is_none() || overlap_end.is_none() {
            note(problems, format!("{}: invalid transition timing; left out", at));
            continue;
        }
        let cut = cut.unwrap();
        let same = |e: &SeqEvent| e.kind == t.kind && e.track == t.track;
        let out_i = events.iter().position(|e| same(e) && e.record_start.checked_add(e.length) == overlap_end);
        let in_i = events.iter().position(|e| same(e) && e.record_start == t.position);
        let (Some(o), Some(i)) = (out_i, in_i) else {
            note(problems, format!("{}: not between two converted clips; left out", at));
            continue;
        };
        let outgoing = transition_members(&events, o);
        let incoming = transition_members(&events, i);
        let out_length = cut.checked_sub(events[o].record_start).filter(|&n| n > 0);
        let in_length = events[i].length.checked_sub(t.cutpoint).filter(|&n| n > 0);
        let source_in = events[i].source_in.checked_add(t.cutpoint);
        if outgoing.iter().any(|o| incoming.contains(o)) || out_length.is_none() || in_length.is_none() || source_in.is_none() {
            note(problems, format!("{}: not between two positive converted spans; left out", at));
            continue;
        }
        // Trim every channel of each stereo edit together, before assembling the group.
        for o in outgoing { events[o].length = out_length.unwrap(); }
        for i in incoming {
            events[i].source_in = source_in.unwrap();
            events[i].length = in_length.unwrap();
            events[i].record_start = cut;
        }
        if t.kind == KIND_SOUND {
            crossfades.push(
                Json::obj()
                    .with("audio_track", Json::Int(t.track))
                    .with("cut_ticks", Json::Int(cut * tpf))
                    .with("duration_ticks", Json::Int(t.length * tpf))
                    .with("cut_offset_ticks", Json::Int(t.cutpoint * tpf)),
            );
            note(problems, format!("{}: audio crossfade; Avid shape {:?} written as Constant Power", at, t.effect));
        } else {
            note(problems, format!("{}: video transition {:?} not converted; clips cut at frame {}", at, t.effect, cut));
        }
    }

    // Count the original combiner channels before media filtering, so a surviving channel
    // cannot masquerade as a complete stereo edit (or part of a larger channel group).
    let mut combined_channels: HashMap<usize, HashSet<usize>> = HashMap::new();
    for e in &seq.events {
        if let Some((group, channel)) = e.combined {
            combined_channels.entry(group).or_default().insert(channel);
        }
    }
    // Multichannel edits (stereo track combiners): one entry per edit, channels in order.
    let mut multichannel: HashMap<usize, (MobId, i64, i64, i64, i64, Vec<(usize, i64, Option<f64>, Option<f64>)>)> = HashMap::new();
    for e in events.iter().filter(|e| e.length > 0) {
        if let Some((g, ch)) = e.combined {
            let entry = multichannel.entry(g).or_insert((e.master, e.record_start, e.length, e.source_in, e.track, Vec::new()));
            if (entry.0, entry.1, entry.2, entry.3) != (e.master, e.record_start, e.length, e.source_in) {
                note(problems, format!("A{} at frame {}: multichannel edit mixes sources; left out", e.track, e.record_start));
                entry.5.clear();
                continue;
            }
            entry.5.push((ch, e.master_track, e.edit_gain_db, e.edit_pan));
        }
    }

    // Link picture and sound pieces of the same clip span into one event.
    let mut groups: Vec<(MobId, i64, i64, i64, Option<i64>, Vec<(i64, i64, Option<f64>, Option<f64>)>)> = Vec::new();
    let mut speeds: HashMap<usize, ((i64, i64), i64)> = HashMap::new();
    let mut colors: HashMap<usize, crate::color::ColorCorrection> = HashMap::new();
    let mut pictures: HashMap<usize, (crate::transform::PictureEffect, bool)> = HashMap::new();
    let mut mattes: HashMap<usize, crate::animatte::AniMatteData> = HashMap::new();
    for e in events.iter().filter(|e| e.length > 0 && e.combined.is_none()) {
        let key = (e.master, e.record_start, e.length, e.source_in);
        // Speed-changed pieces only link with pieces at the same speed.
        let found = groups
            .iter()
            .enumerate()
            .position(|(gi, g)| (g.0, g.1, g.2, g.3) == key && speeds.get(&gi).map(|s| s.0) == e.speed);
        let gi = match found {
            Some(i) => i,
            None => {
                groups.push((e.master, e.record_start, e.length, e.source_in, None, Vec::new()));
                if let Some(sp) = e.speed {
                    speeds.insert(groups.len() - 1, (sp, e.source_length));
                }
                groups.len() - 1
            }
        };
        let g = &mut groups[gi];
        if e.kind == KIND_PICTURE {
            let at = if g.4.is_some() {
                // Same clip twice on two video tracks at once: keep them separate events.
                groups.push((e.master, e.record_start, e.length, e.source_in, Some(e.track), Vec::new()));
                groups.len() - 1
            } else {
                g.4 = Some(e.track);
                gi
            };
            if let Some(cc) = &e.color {
                colors.insert(at, cc.clone());
            }
            if let Some(fx) = &e.picture {
                pictures.insert(at, (fx.clone(), e.picture_nested));
            }
            if let Some(m) = &e.animatte {
                mattes.insert(at, m.clone());
            }
        } else {
            g.5.push((e.track, e.master_track, e.edit_gain_db, e.edit_pan));
        }
    }
    // Each multichannel edit joins the event of the same clip span (e.g. its picture), or
    // becomes its own event.
    let mut stereo_of: HashMap<usize, Vec<(i64, Vec<i64>, Option<f64>, Vec<Option<f64>>)>> = HashMap::new();
    let mut edits: Vec<_> = multichannel.into_iter().filter(|(_, v)| !v.5.is_empty()).collect();
    edits.sort_by_key(|(g, _)| *g);
    for (group, (master, record, length, source_in, track, mut chans)) in edits {
        chans.sort_by_key(|c| c.0);
        let original = &combined_channels[&group];
        let mono = chans.len() == 1 && original.len() == 2 && original.contains(&0) && original.contains(&1);
        if !mono && (chans.len() != original.len() || !(2..=8).contains(&chans.len())) {
            note(problems, format!("A{} at frame {}: incomplete or unsupported multichannel edit; left out", track, record));
            continue;
        }
        let channels: Vec<i64> = chans.iter().map(|c| c.1).collect();
        let gain = chans.iter().find_map(|c| c.2);
        let pans: Vec<Option<f64>> = chans.iter().map(|c| c.3).collect();
        let key = (master, record, length, source_in);
        let i = match groups.iter().position(|g| (g.0, g.1, g.2, g.3) == key) {
            Some(i) => i,
            None => {
                groups.push((master, record, length, source_in, None, Vec::new()));
                groups.len() - 1
            }
        };
        if mono {
            let (side, channel, gain, pan) = chans[0];
            let pan = pan.or(Some(if side == 0 { -100.0 } else { 100.0 }));
            groups[i].5.push((track, channel, gain, pan));
            note(problems, format!("A{} at frame {}: stereo edit is missing a channel; source A{} placed as mono with its gain and pan", track, record, channel));
        } else {
            stereo_of.entry(i).or_default().push((track, channels, gain, pans));
        }
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|&i| (groups[i].1, groups[i].4.unwrap_or(0)));
    let mut width_height = None;
    let mut out_events = Vec::new();
    let mut out_index: HashMap<(MobId, i64, i64, i64), usize> = HashMap::new();
    for i in order {
        let (master, record, length, source_in, video, audio) = groups[i].clone();
        out_index.entry((master, record, length, source_in)).or_insert(out_events.len());
        let (id, m) = &clips[&master];
        if video.is_some() && width_height.is_none() {
            width_height = m.tracks.iter().find_map(|t| match &t.essence {
                Some(Essence::Picture { width, height, .. }) => Some((*width, *height)),
                _ => None,
            });
        }
        let mut ev = Json::obj()
            .with("clip", Json::str(id.clone()))
            .with("record_start_ticks", Json::Int(record * tpf))
            .with("source_in_ticks", Json::Int(source_in * tpf))
            .with("duration_ticks", Json::Int(length * tpf));
        if let Some(v) = video {
            ev.push("video_track", Json::Int(v));
        }
        if let Some(((n, d), source_length)) = speeds.get(&i) {
            ev.push("speed_ratio", Json::Array(vec![Json::Int(*n), Json::Int(*d)]));
            ev.push("source_duration_ticks", Json::Int(source_length * tpf));
        }
        if !audio.is_empty() {
            ev.push("audio_tracks", Json::Array(audio.iter().map(|a| Json::Int(a.0)).collect()));
            ev.push("audio_channels", Json::Array(audio.iter().map(|a| Json::Int(a.1)).collect()));
            // Per-track edit gain/pan from Avid; null = not set on the edit.
            let opt = |v: Option<f64>| v.map(Json::Float).unwrap_or(Json::Null);
            if audio.iter().any(|a| a.2.is_some()) {
                ev.push("audio_gains_db", Json::Array(audio.iter().map(|a| opt(a.2)).collect()));
            }
            if audio.iter().any(|a| a.3.is_some()) {
                ev.push("audio_pans", Json::Array(audio.iter().map(|a| opt(a.3)).collect()));
            }
        }
        if let Some(cc) = colors.get(&i) {
            // Avid Color Correction -> what the writer applies as Lumetri Color.
            let it = cc.interpret();
            let at = format!("{} at frame {}", track_name(KIND_PICTURE, video.unwrap_or(0)), record);
            let mut j = Json::obj();
            if let Some(sat) = it.saturation {
                j.push("saturation", Json::Float(sat));
            }
            if !it.tone.is_empty() {
                j.push("tone", Json::Object(it.tone.iter().map(|(n, v)| (n.to_lowercase(), Json::Float(*v))).collect()));
            }
            if !it.curves.is_empty() {
                j.push(
                    "curves",
                    Json::Object(
                        it.curves
                            .iter()
                            .map(|(n, pts)| (n.to_string(), Json::Array(pts.iter().map(|(x, y)| Json::Array(vec![Json::Float(*x), Json::Float(*y)])).collect())))
                            .collect(),
                    ),
                );
            }
            if !it.not_converted.is_empty() {
                note(problems, format!("{}: color correction controls not converted yet: {}", at, it.not_converted.join(", ")));
                j.push("not_converted", Json::Array(it.not_converted.iter().map(|n| Json::str(n.clone())).collect()));
            }
            for n in &it.notes {
                note(problems, format!("{}: color correction: {}", at, n));
            }
            ev.push("color_correction", j);
        }
        if let Some(data) = mattes.get(&i) {
            // Avid AniMatte -> what the writer applies as a Premiere mask.
            let at = format!("{} at frame {}", track_name(KIND_PICTURE, video.unwrap_or(0)), record);
            let size = m.tracks.iter().find_map(|t| match &t.essence {
                Some(Essence::Picture { width, height, .. }) => Some((*width, *height)),
                _ => None,
            });
            if size == Some((1920, 1080)) {
                ev.push("matte", crate::animatte::matte_json(data));
                if data.tracker_xy.is_some() {
                    note(problems, format!("{}: AniMatte with one tracked point converted as per-frame mask keyframes", at));
                }
                for loss in &data.not_converted {
                    note(problems, format!("{}: {}", at, loss));
                }
            } else {
                note(problems, format!("{}: AniMatte not converted (calibrated for 1920x1080 sources only, this is {:?})", at, size));
            }
        }
        if let Some((fx, nested)) = pictures.get(&i) {
            // Avid Resize / 3D Warp -> what the writer applies as Motion.
            let t = fx.interpret();
            let at = format!("{} at frame {}", track_name(KIND_PICTURE, video.unwrap_or(0)), record);
            let pair = |(a, b): (f64, f64)| Json::Array(vec![Json::Float(a), Json::Float(b)]);
            let mut j = Json::obj()
                .with("position", pair(t.position))
                .with("scale", pair(t.scale))
                .with("rotation", Json::Float(t.rotation))
                .with("anchor", pair(t.anchor))
                .with("crop", Json::Array(t.crop.iter().map(|c| Json::Float(*c)).collect()));
            let mut not_converted = t.not_converted.clone();
            if *nested {
                not_converted.push(format!("{} over another Resize/3D Warp (only the inner one is converted)", fx.name()));
            }
            if !not_converted.is_empty() {
                note(problems, format!("{}: {} controls not converted yet: {}", at, fx.name(), not_converted.join(", ")));
                j.push("not_converted", Json::Array(not_converted.iter().map(|n| Json::str(n.clone())).collect()));
            }
            if !t.is_identity() || !not_converted.is_empty() {
                ev.push("transform", j);
            }
        }
        if let Some(list) = stereo_of.get(&i) {
            ev.push(
                "stereo",
                Json::Array(
                    list.iter()
                        .map(|(t, ch, gain, pans)| {
                            let mut j = Json::obj().with("audio_track", Json::Int(*t)).with("channels", Json::Array(ch.iter().map(|c| Json::Int(*c)).collect()));
                            if let Some(g) = gain {
                                j.push("gain_db", Json::Float(*g));
                            }
                            if pans.iter().any(|p| p.is_some()) {
                                j.push("pans", Json::Array(pans.iter().map(|p| p.map(Json::Float).unwrap_or(Json::Null)).collect()));
                            }
                            j
                        })
                        .collect(),
                ),
            );
        }
        out_events.push(ev);
    }
    // Markers. An edit's marker matching one on its master clip (content and source frame) is
    // shown in Premiere by the clip itself ("inherited"); others belong to the edit alone (User
    // decision 2026-10-15). Positions: sequence-relative ticks; edits also give the master
    // clip offset (source_ticks) the marker sits on.
    let mut markers = Vec::new();
    for sm in &seq.markers {
        let mut j = marker_json(&sm.marker, tpf);
        j.push("start_ticks", Json::Int(sm.marker.frame * tpf));
        if sm.kind != 0 {
            j.push("track", Json::str(track_name(sm.kind, sm.track)));
        }
        let owner = match sm.owner {
            MarkerOwner::Sequence => "sequence",
            MarkerOwner::Filler => "filler",
            MarkerOwner::Edit => "edit",
        };
        if sm.owner == MarkerOwner::Edit {
            let f = sm.marker.frame;
            let piece = events.iter().find(|e| e.length > 0 && e.kind == sm.kind && e.track == sm.track && e.record_start <= f && f < e.record_start + e.length);
            let Some(e) = piece else {
                note(problems, format!("{} marker at frame {}: its edit was not converted; placed on the sequence", track_name(sm.kind, sm.track), f));
                j.push("owner", Json::str("sequence"));
                markers.push(j);
                continue;
            };
            let along = f - e.record_start;
            let source = match e.speed {
                Some((n, d)) if n > 0 => e.source_in + along * d / n,
                Some((n, d)) if n < 0 => e.source_in + e.source_length - 1 - along * d / -n,
                _ => e.source_in + along,
            };
            // Already shown by the Premiere source clip: same content on the master at this
            // source frame (one frame of slack under speed effects, whose rounding is unverified).
            let slack = if e.speed.is_some() { 1 } else { 0 };
            let inherited = clips.get(&e.master).map_or(false, |(_, m)| {
                m.markers.iter().any(|(_, _, mm)| mm.same_content(&sm.marker) && (mm.frame - source).abs() <= slack)
            });
            j.push("source_ticks", Json::Int(source * tpf));
            j.push("inherited", Json::Bool(inherited));
            if let Some(ix) = out_index.get(&(e.master, e.record_start, e.length, e.source_in)) {
                j.push("event", Json::Int(*ix as i64));
            }
        }
        j.push("owner", Json::str(owner));
        markers.push(j);
    }

    let (w, h) = width_height.unwrap_or((1920, 1080));
    let stereo_tracks: Vec<Json> = seq.audio_track_formats.iter().filter(|(_, f)| *f == 2).map(|(t, _)| Json::Int(*t)).collect();
    let mut out = Json::obj()
        .with("name", Json::str(seq.name.clone()))
        .with("rate", Json::str(format!("{}/{}", rate.0, rate.1)))
        .with("width", Json::Int(w))
        .with("height", Json::Int(h))
        .with("events", Json::Array(out_events));
    if !crossfades.is_empty() {
        out.push("audio_crossfades", Json::Array(crossfades));
    }
    if !markers.is_empty() {
        out.push("markers", Json::Array(markers));
    }
    if !stereo_tracks.is_empty() {
        out.push("stereo_audio_tracks", Json::Array(stereo_tracks));
    }
    if let Some(tc) = &seq.start {
        match rate_fraction(tc.fps as f64).and_then(ticks_per_frame) {
            Some(t) => out.push("start_ticks", Json::Int(tc.frames * t)),
            None => note(problems, format!("start timecode at {} fps not converted", tc.fps)),
        }
    }
    Ok(out)
}

/// The model clip, report notes, and the labels of Avid audio tracks left out because their
/// media was not found (the picture and other tracks still convert; a missing picture refuses
/// the clip).
fn clip(m: &MasterClip, media: &MediaIndex, id: &str) -> Result<(Json, Vec<String>, Vec<i64>), String> {
    let locate = |umid: String| media.by_file_package.get(&umid).cloned().ok_or(format!("media for file mob {} not found", umid));
    let mut video = None;
    let mut audio = Vec::new();
    let mut notes = Vec::new();
    let mut missing = Vec::new();
    if m.output_mix != crate::extract::MasterOutputMix::Absent {
        use crate::extract::MasterOutputMix as M;
        let detail = match &m.output_mix {
            M::Integer(n) => format!("integer {n}"),
            M::Text(v) => format!("text ({} UTF-8 bytes)", v.len()),
            M::Bytes(v) => format!("bytes ({} bytes)", v.len()),
            M::Reference(n) => format!("bin-local reference {n}"),
            M::Null => "explicit null".into(),
            M::Absent => unreachable!(),
        };
        notes.push(format!("Master Clip Output Mix Format (_AUDIO_OUTPUT_FORMAT {detail}) is present; legacy model/1 cannot retain this setting and does not apply Avid output mixing; use graph/2 for the raw observation"));
    }
    for t in &m.tracks {
        // Picture lengths count frames; sound lengths count samples at the sample rate.
        let rate = match t.kind {
            KIND_SOUND if t.edit_rate.fract() == 0.0 && t.edit_rate > 0.0 && t.edit_rate <= 384_000.0 => {
                (t.edit_rate as i64, 1)
            }
            _ => rate_fraction(t.edit_rate).ok_or(format!("unsupported edit rate {}", t.edit_rate))?,
        };
        let duration = ticks(t.length, rate).ok_or("duration is not a whole number of ticks")?;
        let path_buf = match locate(t.file_mob.umid_hex()) {
            Ok(p) => p,
            Err(e) if t.kind == KIND_SOUND => {
                notes.push(format!("audio track A{}: {}; converted without it", t.index, e));
                missing.push(t.index);
                continue;
            }
            Err(e) => return Err(e),
        };
        let start = media.start_ticks.get(&path_buf).copied().unwrap_or(0);
        // Premiere's own import takes the length from the MXF; a shorter Avid value left the
        // file silent in Premiere 26.5.2 (SPOOGE, 1,062,001 vs 1,064,000 samples).
        let duration = media.duration_ticks.get(&path_buf).copied().unwrap_or(duration);
        let path = path_buf.to_str().ok_or("media path is not UTF-8")?.to_string();
        match (t.kind, &t.essence) {
            (KIND_PICTURE, Some(Essence::Picture { compression, width, height, display_aspect, .. })) => {
                if video.is_some() {
                    return Err("more than one picture track".into());
                }
                // RI00 is generic: require an exact qualified MXF picture-coding label.
                let codec = picture_codec(compression, media.picture_coding.get(&path_buf)).ok_or_else(||
                    format!("unsupported picture compression {:?} with MXF picture-coding {:?}", compression, media.picture_coding.get(&path_buf)))?;
                if let Some((n, d)) = display_aspect {
                    if let Some((par_n, par_d)) = pixel_aspect((*n, *d), (*width, *height)) {
                        if par_n != par_d {
                            notes.push(format!("display aspect {}:{} on {}x{} raster implies pixel aspect {}:{}; avid2prproj.model/1 has no pixel-aspect field, so this is not retained", n, d, width, height, par_n, par_d));
                        }
                    }
                }
                video = Some(
                    Json::obj()
                        .with("path", Json::str(path))
                        .with("codec", Json::str(codec))
                        .with("width", Json::Int(*width))
                        .with("height", Json::Int(*height))
                        .with("rate", Json::str(format!("{}/{}", rate.0, rate.1)))
                        .with("duration_ticks", Json::Int(duration))
                        .with("start_ticks", Json::Int(start)),
                );
            }
            (KIND_SOUND, Some(Essence::Sound { sample_rate, bits, channels })) => {
                if *channels != 1 {
                    return Err(format!("audio file mob with {} channels", channels));
                }
                audio.push(
                    Json::obj()
                        .with("path", Json::str(path))
                        .with("sample_rate", Json::Int(sample_rate.round() as i64))
                        .with("bits", Json::Int(*bits))
                        .with("duration_ticks", Json::Int(duration))
                        // From the bin's track label; filenames are not reliably A01-style.
                        .with("avid_track", Json::Int(t.index))
                        .with("start_ticks", Json::Int(start))
                        .with("gain_db", Json::Float(t.gain_db.unwrap_or(0.0))),
                );
            }
            _ => return Err(format!("track {} has an unsupported descriptor", t.index)),
        }
    }
    if video.is_none() && audio.is_empty() {
        return Err("no media found for any clip track; clip and its edits left out".into());
    }
    let mut avid = Json::obj().with("mob_id", Json::str(m.mob_id.urn()));
    avid.push("columns", Json::Object(m.columns.iter().map(|(k, v)| (k.clone(), Json::str(v.clone()))).collect()));
    if let Some([r, g, b]) = m.color {
        avid.push("color_rgb16", Json::Array(vec![Json::Int(r), Json::Int(g), Json::Int(b)]));
    }
    if let Some(tc) = &m.start {
        avid.push(
            "start",
            Json::obj()
                .with("timecode", Json::str(format_timecode(tc.frames, tc.fps)))
                .with("frames", Json::Int(tc.frames))
                .with("fps", Json::Int(tc.fps)),
        );
    }
    if !m.aux_timecodes.is_empty() {
        avid.push(
            "aux_timecodes",
            Json::Array(
                m.aux_timecodes
                    .iter()
                    .map(|a| {
                        Json::obj()
                            .with("track_index", Json::Int(a.track_index))
                            .with("aux_tc_hypothesis", Json::Int(a.track_index - 2))
                            .with("timecode", Json::str(format_timecode(a.start, a.fps)))
                            .with("start", Json::Int(a.start))
                            .with("fps", Json::Int(a.fps))
                            .with("position", Json::Int(a.position))
                            .with("length", Json::Int(a.length))
                    })
                    .collect(),
            ),
        );
    }
    if let Some(s) = &m.source_file {
        avid.push("source_file", Json::str(s.clone()));
    }
    if let Some(s) = &m.source_path {
        avid.push("source_path", Json::str(s.clone()));
    }
    let mut out = Json::obj()
        .with("id", Json::str(id))
        .with("name", Json::str(m.name.clone()))
        .with("material_umid", Json::str(m.mob_id.umid_hex()));
    // Avid's clip rate (FPS column), needed to build audio-only clips at the right timebase.
    if let Some((n, d)) = rate_fraction(m.edit_rate) {
        out.push("rate", Json::str(format!("{}/{}", n, d)));
    }
    // Avid stereo grouping (`_CHANNEL_GROUP_LIST`), as pairs of Avid audio track labels.
    let mut pairs = Vec::new();
    for &(left, right) in &m.stereo_pairs {
        let has_track = |label| m.tracks.iter().any(|t| t.kind == KIND_SOUND && t.index == label);
        if !has_track(left) || !has_track(right) { continue; }
        if missing.contains(&left) || missing.contains(&right) {
            notes.push(format!("stereo pair A{}/A{} has missing media; any available audio imported as mono", left, right));
            continue;
        }
        pairs.push(Json::Array(vec![Json::Int(left), Json::Int(right)]));
    }
    if !pairs.is_empty() {
        out.push("stereo_pairs", Json::Array(pairs));
    }
    let mut multichannel = Vec::new();
    for raw in &m.other_groups {
        if let Some((format, channels)) = crate::extract::parse_multichannel_group(raw) {
            if channels.iter().all(|n| !missing.contains(n) && m.tracks.iter().any(|t|t.kind==KIND_SOUND&&t.index==*n)) {
                multichannel.push(Json::obj().with("format",Json::str(format)).with("channels",Json::Array(channels.into_iter().map(Json::Int).collect())));
            }
        }
    }
    if !multichannel.is_empty() { out.push("multichannel_groups", Json::Array(multichannel)); }
    // Audio-only items show timecode at Premiere's indeterminate timebase (23.976 by default)
    // unless told otherwise; give them Avid's FPS (PRPROJ-0017).
    if let Some(code) = time_display_code(m.edit_rate, m.start.as_ref().map_or(false, |t| t.drop)) {
        out.push("time_display", Json::Int(code));
    }
    if let Some(v) = video {
        out.push("video", v);
    }
    out.push("audio", Json::Array(audio));
    if !m.markers.is_empty() {
        // Clip markers at master-relative ticks, with the Avid track they sit on.
        if let Some(tpf) = rate_fraction(m.edit_rate).and_then(ticks_per_frame) {
            out.push(
                "markers",
                Json::Array(
                    m.markers
                        .iter()
                        .map(|(kind, track, mk)| {
                            let mut j = marker_json(mk, tpf);
                            j.push("start_ticks", Json::Int(mk.frame * tpf));
                            j.push("track", Json::str(track_name(*kind, *track)));
                            j
                        })
                        .collect(),
                ),
            );
        }
    }
    out.push("avid", avid);
    Ok((out, notes, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        assert_eq!(rate_fraction(24.0), Some((24, 1)));
        assert_eq!(rate_fraction(23.976), Some((24000, 1001)));
        assert_eq!(rate_fraction(29.97), Some((30000, 1001)));
        assert_eq!(rate_fraction(48000.0), None);
        assert_eq!(rate_fraction(f64::NAN), None);
        assert_eq!(ticks(242, (24, 1)), Some(2_561_328_000_000));
        assert_eq!(time_display_code(24.0, false), Some(100));
        assert_eq!(time_display_code(23.976, false), Some(110));
        assert_eq!(time_display_code(29.97, true), Some(102));
        assert_eq!(time_display_code(12.0, false), None);
    }

    #[test]
    fn prores_lt_requires_both_avid_and_mxf_identity() {
        let mut standard = mxf::PRORES_422_LT_CODING;
        standard[14] = 3;
        assert_eq!(picture_codec("RI00", Some(&mxf::PRORES_422_LT_CODING)), Some("apcs"));
        assert_eq!(picture_codec("RI00", Some(&standard)), None);
        assert_eq!(picture_codec("RI00", None), None);
        assert_eq!(picture_codec("AVHD", None), Some("AVdn"));
        assert_eq!(picture_codec("unknown", Some(&mxf::PRORES_422_LT_CODING)), None);
        assert_eq!(pixel_aspect((256, 135), (1920, 1080)), Some((16, 15)));
        assert_eq!(pixel_aspect((16, 9), (1920, 1080)), Some((1, 1)));
    }

    #[test]
    fn native_dnxhr_lb_requires_its_exact_mxf_label() {
        assert_eq!(picture_codec("RI00", Some(&mxf::DNXHR_LB_CODING)), Some("AVdn"));
        assert_eq!(picture_codec("RI00", None), None);
        assert_eq!(picture_codec("unknown", Some(&mxf::DNXHR_LB_CODING)), None);
        for byte in 0..16 {
            let mut other = mxf::DNXHR_LB_CODING;
            other[byte] ^= 1;
            assert_eq!(picture_codec("RI00", Some(&other)), None);
        }
    }
}

#[cfg(test)]
mod reliability_tests {
    use super::*;
    use crate::extract::MediaTrack;
    use crate::io::Uuid;

    fn audio_master(number: u32, name: &str, tracks: bool) -> MasterClip {
        let mob = MobId { label: [0; 12], length: 0x13, instance: [0; 3],
            material: Uuid { data1: number, data2: 0, data3: 0, data4: [0; 8] } };
        MasterClip { name: name.into(), mob_id: mob, edit_rate: 24.0, length: 24,
            tracks: if tracks { vec![MediaTrack { kind: KIND_SOUND, index: 1, file_mob: mob,
                edit_rate: 48000.0, length: 48000, essence: Some(Essence::Sound {
                    sample_rate: 48000.0, bits: 24, channels: 1 }), offset: 0, clip_length: 24,
                gain_db: None, pan_raw: None }] } else { vec![] },
            columns: vec![], color: None, start: None, aux_timecodes: vec![], source_file: None,
            source_path: None, bin: 0, stereo_pairs: vec![], other_groups: vec![], output_mix: Default::default(), markers: vec![] }
    }

    #[test]
    fn no_media_clip_is_reported_and_omitted_while_online_clip_survives() {
        let missing = audio_master(1, "Missing sound", true);
        let empty = audio_master(2, "No tracks", false);
        let online = audio_master(3, "Online sound", true);
        let mut media = MediaIndex::default();
        media.by_file_package.insert(online.mob_id.umid_hex(), PathBuf::from("/synthetic/sound.mxf"));
        let contents = BinContents { masters: vec![missing, empty, online], ..Default::default() };
        let conversion = build(&contents, &media, "Regression", &["Bin".into()], "separate");
        fn field<'a>(j: &'a Json, key: &str) -> &'a Json {
            let Json::Object(pairs) = j else { panic!("expected object") };
            &pairs.iter().find(|(k, _)| k == key).unwrap().1
        }
        let Json::Array(clips) = field(&conversion.model, "clips") else { panic!("expected clips") };
        assert_eq!(clips.len(), 1);
        assert_eq!(field(&clips[0], "name"), &Json::str("Online sound"));
        assert_eq!(conversion.problems.len(), 2);
        assert!(conversion.problems.iter().all(|p| p.contains("no media found")));
        let Json::Array(bins) = field(&conversion.model, "bins") else { panic!("expected bins") };
        assert_eq!(field(&bins[0], "clips"), &Json::Array(vec![Json::str("clip-3")]));
    }

    fn property<'a>(j: &'a Json, key: &str) -> Option<&'a Json> {
        let Json::Object(pairs) = j else { panic!("expected object") };
        pairs.iter().find(|(k, _)| k == key).map(|(_, value)| value)
    }

    #[test]
    fn legacy_model_reports_present_master_mix_without_inventing_a_field() {
        let mut master = audio_master(1, "Mix observation", true);
        let mut media = MediaIndex::default();
        media.by_file_package.insert(master.mob_id.umid_hex(), PathBuf::from("/synthetic/sound.mxf"));
        let (_, absent, _) = clip(&master, &media, "clip-1").unwrap();
        assert!(absent.iter().all(|n| !n.contains("Output Mix Format")));
        for raw in [4, 5, -17] {
            master.output_mix = crate::extract::MasterOutputMix::Integer(raw);
            let (model, notes, _) = clip(&master, &media, "clip-1").unwrap();
            assert!(notes.iter().any(|n| n.contains(&format!("_AUDIO_OUTPUT_FORMAT integer {raw}")) && n.contains("does not apply Avid output mixing")));
            assert!(property(&model, "master_output_mix").is_none());
            assert!(property(property(&model, "avid").unwrap(), "master_output_mix").is_none());
        }
    }

    fn list<'a>(j: &'a Json, key: &str) -> &'a [Json] {
        match property(j, key) {
            Some(Json::Array(values)) => values,
            None => &[],
            _ => panic!("expected array"),
        }
    }

    fn stereo_master() -> MasterClip {
        let mut m = audio_master(1, "Stereo fixture", true);
        let mut right = m.tracks[0].clone();
        right.index = 2;
        right.file_mob = audio_master(2, "", false).mob_id;
        m.tracks.push(right);
        let mut video = m.tracks[0].clone();
        video.kind = KIND_PICTURE;
        video.file_mob = audio_master(3, "", false).mob_id;
        video.edit_rate = 24.0;
        video.length = 24;
        video.essence = Some(Essence::Picture { compression: "AVHD".into(), resolution_id: 1253,
            width: 1920, height: 1080, display_aspect: Some((16, 9)) });
        m.tracks.insert(0, video);
        m.stereo_pairs.push((1, 2));
        m
    }

    fn media_for(m: &MasterClip, missing: &[i64]) -> MediaIndex {
        let mut index = MediaIndex::default();
        for track in &m.tracks {
            if track.kind == KIND_SOUND && missing.contains(&track.index) { continue; }
            index.by_file_package.insert(track.file_mob.umid_hex(),
                PathBuf::from(format!("/synthetic/{}-{}.mxf", track.kind, track.index)));
        }
        index
    }

    fn stereo_sequence(m: &MasterClip, pan: Option<f64>) -> SequenceInfo {
        let event = |kind, source, combined| SeqEvent { kind, track: 1, record_start: 0,
            length: 24, master: m.mob_id, master_track: source, source_in: 0, via: vec![],
            speed: None, source_length: 24, combined, edit_gain_db: Some(-3.0), edit_pan: pan,
            color: None, picture: None, picture_nested: false, animatte: None };
        let mut seq = SequenceInfo::default();
        seq.name = "Stereo sequence".into();
        seq.bin = m.bin;
        seq.edit_rate = 24.0;
        seq.length = 24;
        seq.events = vec![event(KIND_PICTURE, 1, None),
            event(KIND_SOUND, 1, Some((0, 0))), event(KIND_SOUND, 2, Some((0, 1)))];
        seq.audio_track_formats.push((1, 2));
        seq
    }

    // Optional cross-component fixture export; the CI test itself always checks the reader.
    // A new destination is required so prior evidence cannot be overwritten.
    fn export_model(name: &str, model: &Json) {
        if let Some(directory) = std::env::var_os("ANYNLE_READER_MODEL_EXPORT") {
            use std::io::Write;
            let path = PathBuf::from(directory).join(format!("{name}.model.json"));
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path).unwrap();
            file.write_all(model.to_pretty().as_bytes()).unwrap();
        }
    }

    #[test]
    fn incomplete_stereo_pair_does_not_reference_missing_clip_audio() {
        for missing in [1, 2] {
            let master = stereo_master();
            let conversion = build(&BinContents { masters: vec![master.clone()], ..Default::default() },
                &media_for(&master, &[missing]), "Partial stereo", &["Bin".into()], "separate");
            let clip = &list(&conversion.model, "clips")[0];
            assert_eq!(list(clip, "audio").len(), 1);
            assert!(list(clip, "stereo_pairs").is_empty(), "pair still names unavailable media");
            assert!(conversion.problems.iter().any(|p| p.contains("stereo pair") && p.contains("mono")));
        }
    }

    #[test]
    fn incomplete_stereo_edit_keeps_the_surviving_channel_and_pan() {
        for (missing, explicit_pan, expected_pan, name) in [
            (1, None, 100.0, "reader-stereo-missing-left"),
            (2, None, -100.0, "reader-stereo-missing-right"),
            (2, Some(-25.0), -25.0, "reader-stereo-missing-right-pan"),
        ] {
            let master = stereo_master();
            let contents = BinContents { masters: vec![master.clone()],
                sequences: vec![stereo_sequence(&master, explicit_pan)], ..Default::default() };
            let conversion = build(&contents, &media_for(&master, &[missing]),
                "Partial stereo", &["Bin".into()], "separate");
            let sequence = &list(&conversion.model, "sequences")[0];
            let events = list(sequence, "events");
            assert_eq!(events.len(), 1);
            assert!(list(&events[0], "stereo").is_empty(), "a one-channel stereo edit is invalid");
            assert_eq!(property(&events[0], "video_track"), Some(&Json::Int(1)));
            assert_eq!(list(&events[0], "audio_tracks"), &[Json::Int(1)]);
            assert_eq!(list(&events[0], "audio_channels"), &[Json::Int(3-missing)]);
            assert_eq!(list(&events[0], "audio_pans"), &[Json::Float(expected_pan)]);
            assert_eq!(list(&events[0], "audio_gains_db"), &[Json::Float(-3.0)]);
            assert!(conversion.problems.iter().any(|p| p.contains("placed as mono")));
            export_model(name, &conversion.model);
        }
    }

    #[test]
    fn complete_stereo_and_picture_only_controls_remain_valid() {
        for (missing, name) in [(&[][..], "reader-stereo-complete"),
                                (&[1, 2][..], "reader-stereo-missing-both")] {
            let master = stereo_master();
            let contents = BinContents { masters: vec![master.clone()],
                sequences: vec![stereo_sequence(&master, None)], ..Default::default() };
            let conversion = build(&contents, &media_for(&master, missing),
                "Stereo control", &["Bin".into()], "separate");
            let event = &list(&list(&conversion.model, "sequences")[0], "events")[0];
            assert_eq!(list(event, "stereo").len(), if missing.is_empty() { 1 } else { 0 });
            assert_eq!(property(event, "video_track"), Some(&Json::Int(1)));
            if !missing.is_empty() { assert!(list(event, "audio_tracks").is_empty()); }
            export_model(name, &conversion.model);
        }
    }

    #[test]
    fn incomplete_audio_only_stereo_stays_on_its_original_track() {
        let mut master = stereo_master();
        master.tracks.retain(|t| t.kind == KIND_SOUND);
        let mut seq = stereo_sequence(&master, None);
        seq.events.retain(|e| e.kind == KIND_SOUND);
        for e in &mut seq.events { e.track = 4; e.record_start = 48; }
        let contents = BinContents { masters: vec![master.clone()], sequences: vec![seq], ..Default::default() };
        let conversion = build(&contents, &media_for(&master, &[1]),
            "Audio only", &["Bin".into()], "separate");
        let events = list(&list(&conversion.model, "sequences")[0], "events");
        assert_eq!(events.len(), 1);
        assert_eq!(property(&events[0], "record_start_ticks"), Some(&Json::Int(48 * TICKS_PER_SECOND / 24)));
        assert_eq!(property(&events[0], "video_track"), None);
        assert_eq!(list(&events[0], "audio_tracks"), &[Json::Int(4)]);
        assert_eq!(list(&events[0], "audio_channels"), &[Json::Int(2)]);
        assert_eq!(list(&events[0], "audio_pans"), &[Json::Float(100.0)]);
        export_model("reader-stereo-audio-only", &conversion.model);
    }

    #[test]
    fn incomplete_larger_channel_group_is_not_relabelled_as_stereo() {
        let mut master = stereo_master();
        let mut third = master.tracks.iter().find(|t| t.kind == KIND_SOUND).unwrap().clone();
        third.index = 3;
        third.file_mob = audio_master(5, "", false).mob_id;
        master.tracks.push(third);
        let mut seq = stereo_sequence(&master, None);
        let mut third = seq.events[1].clone();
        third.master_track = 3;
        third.combined = Some((0, 2));
        seq.events.push(third);
        let contents = BinContents { masters: vec![master.clone()], sequences: vec![seq], ..Default::default() };
        let conversion = build(&contents, &media_for(&master, &[3]),
            "Incomplete channel group", &["Bin".into()], "separate");
        let event = &list(&list(&conversion.model, "sequences")[0], "events")[0];
        assert!(list(event, "stereo").is_empty());
        assert!(list(event, "audio_tracks").is_empty());
        assert_eq!(property(event, "video_track"), Some(&Json::Int(1)));
        assert!(conversion.problems.iter().any(|p| p.contains("incomplete or unsupported multichannel edit")));
        export_model("reader-incomplete-channel-group", &conversion.model);
    }

    #[test]
    fn missing_audio_is_scoped_to_the_selected_bin_copy() {
        let first = stereo_master();
        let mut second = first.clone();
        second.bin = 1;
        // Same master MobID, but this bin copy has a different available A2 file mob.
        second.tracks.iter_mut().find(|t| t.kind == KIND_SOUND && t.index == 2).unwrap().file_mob =
            audio_master(4, "", false).mob_id;
        let mut index = media_for(&first, &[2]);
        index.by_file_package.extend(media_for(&second, &[]).by_file_package);
        let contents = BinContents { masters: vec![first.clone(), second.clone()],
            sequences: vec![stereo_sequence(&first, None), stereo_sequence(&second, None)], ..Default::default() };
        let conversion = build(&contents, &index, "Bin copies", &["First".into(), "Second".into()], "separate");
        let sequences = list(&conversion.model, "sequences");
        let first_event = &list(&sequences[0], "events")[0];
        let second_event = &list(&sequences[1], "events")[0];
        assert!(list(first_event, "stereo").is_empty());
        assert_eq!(list(first_event, "audio_channels"), &[Json::Int(1)]);
        assert_eq!(list(second_event, "stereo").len(), 1, "other bin's missing media leaked into this copy");
        export_model("reader-stereo-bin-copies", &conversion.model);
    }

    fn transition_sequence(stereo: bool, cutpoint: i64) -> SequenceInfo {
        let master = stereo_master();
        let mut seq = stereo_sequence(&master, None);
        seq.events.retain(|e| e.kind == KIND_SOUND && (stereo || e.master_track == 1));
        for e in &mut seq.events {
            e.length = 16;
            e.source_length = 16;
            if !stereo { e.combined = None; }
        }
        let mut incoming = seq.events.clone();
        for e in &mut incoming {
            e.record_start = 12;
            e.length = 12;
            e.source_length = 12;
            if stereo { e.combined = Some((1, e.master_track as usize - 1)); }
        }
        seq.events.extend(incoming);
        seq.transitions.push(crate::extract::SeqTransition { kind: KIND_SOUND, track: 1,
            position: 12, length: 4, cutpoint, effect: None });
        if !stereo { seq.audio_track_formats.clear(); }
        seq
    }

    fn convert_transition(seq: SequenceInfo, missing: &[i64]) -> Conversion {
        let master = stereo_master();
        build(&BinContents { masters: vec![master.clone()], sequences: vec![seq], ..Default::default() },
            &media_for(&master, missing), "Transition regression", &["Bin".into()], "separate")
    }

    fn assert_span(event: &Json, record: i64, source: i64, length: i64) {
        let tpf = TICKS_PER_SECOND / 24;
        assert_eq!(property(event, "record_start_ticks"), Some(&Json::Int(record * tpf)));
        assert_eq!(property(event, "source_in_ticks"), Some(&Json::Int(source * tpf)));
        assert_eq!(property(event, "duration_ticks"), Some(&Json::Int(length * tpf)));
    }

    #[test]
    fn stereo_transition_trims_both_channels_to_the_same_cut() {
        for cutpoint in [0, 2, 4] {
            let conversion = convert_transition(transition_sequence(true, cutpoint), &[]);
            let seq = &list(&conversion.model, "sequences")[0];
            let events = list(seq, "events");
            assert_eq!(events.len(), 2, "mismatched stereo channel spans dropped the edits");
            assert_span(&events[0], 0, 0, 12 + cutpoint);
            assert_span(&events[1], 12 + cutpoint, cutpoint, 12 - cutpoint);
            for event in events {
                let stereo = list(event, "stereo");
                assert_eq!(stereo.len(), 1);
                assert_eq!(list(&stereo[0], "channels"), &[Json::Int(1), Json::Int(2)]);
                assert!(list(event, "audio_tracks").is_empty());
            }
            assert_eq!(list(seq, "audio_crossfades").len(), 1);
            assert!(!conversion.problems.iter().any(|p| p.contains("mixes sources")));
            export_model(&format!("reader-stereo-transition-cut-{cutpoint}"), &conversion.model);
        }
    }

    #[test]
    fn duplicate_channel_transition_records_create_one_crossfade() {
        let mut seq = transition_sequence(true, 2);
        seq.transitions.push(seq.transitions[0].clone());
        seq.events.reverse(); // Channel iteration order must not affect normalization.
        let conversion = convert_transition(seq, &[]);
        let seq = &list(&conversion.model, "sequences")[0];
        assert_eq!(list(seq, "audio_crossfades").len(), 1, "duplicate transition was emitted twice");
        let events = list(seq, "events");
        assert_eq!(events.len(), 2);
        assert_span(&events[0], 0, 0, 14);
        assert_span(&events[1], 14, 2, 10);
        assert!(events.iter().all(|e| list(e, "stereo").len() == 1));
        export_model("reader-stereo-transition-duplicates", &conversion.model);
    }

    #[test]
    fn consecutive_stereo_transitions_keep_the_middle_edit_intact() {
        let master = stereo_master();
        let mut seq = stereo_sequence(&master, None);
        seq.events.retain(|e| e.kind == KIND_SOUND);
        let template = seq.events.clone();
        seq.events.clear();
        for (group, record, length) in [(0, 0, 12), (1, 8, 12), (2, 16, 8)] {
            for mut event in template.clone() {
                event.record_start = record;
                event.length = length;
                event.source_length = length;
                event.combined = Some((group, event.master_track as usize - 1));
                seq.events.push(event);
            }
        }
        seq.transitions = [8, 16].iter().map(|&position| crate::extract::SeqTransition {
            kind: KIND_SOUND, track: 1, position, length: 4, cutpoint: 2, effect: None }).collect();
        let conversion = convert_transition(seq, &[]);
        let seq = &list(&conversion.model, "sequences")[0];
        let events = list(seq, "events");
        assert_eq!(events.len(), 3);
        assert_span(&events[0], 0, 0, 10);
        assert_span(&events[1], 10, 2, 8);
        assert_span(&events[2], 18, 2, 6);
        assert_eq!(list(seq, "audio_crossfades").len(), 2);
        assert!(events.iter().all(|e| list(e, "stereo").len() == 1));
        export_model("reader-stereo-transition-consecutive", &conversion.model);
    }

    #[test]
    fn mono_and_missing_stereo_channel_transition_controls_keep_their_spans() {
        for (stereo, missing, name) in [(false, &[][..], "mono"),
            (true, &[1][..], "missing-left"), (true, &[2][..], "missing-right")] {
            let conversion = convert_transition(transition_sequence(stereo, 2), missing);
            let seq = &list(&conversion.model, "sequences")[0];
            let events = list(seq, "events");
            assert_eq!(events.len(), 2);
            assert_span(&events[0], 0, 0, 14);
            assert_span(&events[1], 14, 2, 10);
            assert!(events.iter().all(|e| list(e, "stereo").is_empty() && list(e, "audio_tracks") == [Json::Int(1)]));
            assert_eq!(list(seq, "audio_crossfades").len(), 1);
            export_model(&format!("reader-transition-{name}"), &conversion.model);
        }
    }

    #[test]
    fn video_transition_control_keeps_the_existing_cut_policy() {
        let mut seq = transition_sequence(false, 2);
        for event in &mut seq.events { event.kind = KIND_PICTURE; }
        seq.transitions[0].kind = KIND_PICTURE;
        let conversion = convert_transition(seq, &[]);
        let seq = &list(&conversion.model, "sequences")[0];
        let events = list(seq, "events");
        assert_eq!(events.len(), 2);
        assert_span(&events[0], 0, 0, 14);
        assert_span(&events[1], 14, 2, 10);
        assert!(events.iter().all(|e| property(e, "video_track") == Some(&Json::Int(1))));
        assert!(list(seq, "audio_crossfades").is_empty());
        assert!(conversion.problems.iter().any(|p| p.contains("video transition") && p.contains("not converted")));
        export_model("reader-video-transition-control", &conversion.model);
    }

    #[test]
    fn malformed_or_self_referencing_transition_does_not_trim_edits() {
        for (cutpoint, position, length) in [(-1, 12, 4), (5, 12, 4),
            (0, i64::MAX, 4), (0, 0, 16)] {
            let mut seq = transition_sequence(false, cutpoint);
            seq.transitions[0].position = position;
            seq.transitions[0].length = length;
            let conversion = convert_transition(seq, &[]);
            let seq = &list(&conversion.model, "sequences")[0];
            let events = list(seq, "events");
            assert_eq!(events.len(), 2);
            assert_span(&events[0], 0, 0, 16);
            assert_span(&events[1], 12, 0, 12);
            assert!(list(seq, "audio_crossfades").is_empty());
            assert!(conversion.problems.iter().any(|p| p.contains("transition") && p.contains("left out")));
        }
    }

}
