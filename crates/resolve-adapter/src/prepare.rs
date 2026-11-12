use crate::{
    audio::{self, Envelope},
    media::VerifiedBinding,
    source::{mixer, ADAPTER, CLOCK_ROUTE},
    Result, DAY,
};
use graph::{
    source::{SourceGraph, SourceItem, SourceMarker, SourceSequence},
    Loss, MarkerOwner, Rational, StreamKind, Value,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub(crate) const MAX_EDITS_PER_LANE: usize = 512;
pub(crate) const MAX_CORE_MEDIA_ITEMS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Edit {
    pub(crate) start: i64,
    pub(crate) length: i64,
    pub(crate) source_in: i64,
    pub(crate) envelope: Option<Envelope>,
    pub(crate) binding: usize,
}

pub struct PreparedResolve {
    pub(crate) project_name: String,
    pub(crate) sequence_name: String,
    pub(crate) origin: i64,
    pub(crate) end: i64,
    pub(crate) lanes: [Vec<Edit>; 3],
    pub(crate) track_faders: [i32; 2],
    pub(crate) main_bus_fader: i32,
    pub(crate) track_mutes: [bool; 2],
    pub(crate) bindings: Vec<VerifiedBinding>,
    pub(crate) markers: Vec<PreparedMarker>,
    omitted_markers: Vec<String>,
}

impl PreparedResolve {
    pub fn omitted_markers(&self) -> &[String] {
        &self.omitted_markers
    }
}

pub(crate) fn rate() -> Rational {
    Rational::new(24000, 1001).expect("valid constant rate")
}

pub(crate) fn frames(t: Rational) -> Result<i64> {
    frames_on(t, rate())
}

pub(crate) fn frames_on(t: Rational, rate: Rational) -> Result<i64> {
    let n = i128::from(t.num()) * i128::from(rate.num());
    let d = i128::from(t.den()) * i128::from(rate.den());
    if n < 0 || n % d != 0 {
        return Err("time is outside the supported nonnegative frame grid".into());
    }
    Ok(i64::try_from(n / d)?)
}

pub(crate) fn simple_range(lane: &mut [Edit]) -> Result<i64> {
    lane.sort_by_key(|e| e.start);
    let mut end = 0;
    for e in lane {
        if e.start != end || e.length <= 0 {
            return Err(
                "the first Resolve profile requires contiguous straight cuts on each lane".into(),
            );
        }
        end = e
            .start
            .checked_add(e.length)
            .ok_or("record range overflow")?;
    }
    Ok(end)
}

pub fn prepare(
    graph: &SourceGraph,
    sequence_id: &str,
    canvas: (u16, u16),
    binding: VerifiedBinding,
) -> Result<PreparedResolve> {
    prepare_bindings(graph, sequence_id, canvas, vec![binding])
}

pub(crate) fn prepare_bindings(
    graph: &SourceGraph,
    sequence_id: &str,
    canvas: (u16, u16),
    bindings: Vec<VerifiedBinding>,
) -> Result<PreparedResolve> {
    graph.validate()?;
    if bindings.is_empty() || bindings.len() > MAX_CORE_MEDIA_ITEMS {
        return Err("Resolve media item count outside supported range".into());
    }
    if bindings.len() > 1 && bindings.iter().any(|binding| binding.atom.is_none()) {
        return Err("multiple core media items require direct Avid OP-Atom".into());
    }
    if canvas != (1920, 1080) {
        return Err("explicit 1920x1080 square-pixel destination canvas required".into());
    }
    let sequence = graph
        .sequences
        .iter()
        .find(|s| s.id == sequence_id)
        .ok_or("selected sequence identity absent")?;
    if sequence.rate != rate()
        || sequence.frame_size.is_some_and(|size| size != (1920, 1080))
        || !sequence.transitions.is_empty()
        || !sequence.texts.is_empty()
        || graph
            .multicam_uses
            .iter()
            .any(|use_| use_.sequence == sequence_id)
    {
        return Err(
            "unsupported Resolve sequence timing, canvas, transitions, text or Group topology"
                .into(),
        );
    }
    let native = sequence
        .clock
        .as_ref()
        .is_some_and(|clock| clock.provenance.route == CLOCK_ROUTE);
    let origin = sequence_origin(sequence, native)?;
    let track_faders = prepare_tracks(sequence, native)?;
    let timeline_receipt = graph.sidecar(ADAPTER, sequence_id);
    let main_bus_fader = timeline_receipt
        .and_then(|fields| fields.get(mixer::MAIN_BUS_LEVEL))
        .map(|value| {
            let level = match value {
                Value::Float(db) => *db,
                Value::Int(db) => *db as f64,
                _ => return Err("invalid Resolve main bus level"),
            };
            if !native {
                return Err("Resolve main bus level requires a native Resolve sequence");
            }
            mixer::fader_ticks(level).ok_or("unsupported Resolve main bus fader level")
        })
        .transpose()?;
    let mut track_mutes = [false; 2];
    if let Some(value) = timeline_receipt.and_then(|fields| fields.get(mixer::MUTED_AUDIO_TRACKS)) {
        if !native {
            return Err("Resolve track mute requires a native Resolve sequence".into());
        }
        let Value::Arr(tracks) = value else {
            return Err("invalid Resolve muted audio track list".into());
        };
        if !(1..=2).contains(&tracks.len()) {
            return Err("invalid Resolve muted audio track count".into());
        }
        for track in tracks {
            let slot = match track {
                Value::Int(1) => &mut track_mutes[0],
                Value::Int(2) => &mut track_mutes[1],
                _ => return Err("unsupported Resolve muted audio track index".into()),
            };
            if std::mem::replace(slot, true) {
                return Err("duplicate Resolve muted audio track index".into());
            }
        }
    }

    let mut item_indices = HashMap::new();
    let mut items = Vec::with_capacity(bindings.len());
    for (index, binding) in bindings.iter().enumerate() {
        let item = check_binding_profile(graph, binding, native)?;
        if item_indices.insert(item.id.as_str(), index).is_some() {
            return Err("duplicate Resolve binding for the same media item".into());
        }
        items.push(item);
    }
    if sequence.placements.len() > 3 * MAX_EDITS_PER_LANE {
        return Err("Resolve edit count limit exceeded".into());
    }
    let sidecars: HashMap<_, _> = if native {
        let selected: HashSet<_> = sequence.placements.iter().map(|p| p.id.as_str()).collect();
        graph
            .sidecars
            .iter()
            .filter(|side| side.adapter == ADAPTER && selected.contains(side.object.as_str()))
            .map(|side| (side.object.as_str(), &side.data))
            .collect()
    } else {
        HashMap::new()
    };
    let mut recovered = HashMap::new();
    let mut lanes: [Vec<Edit>; 3] = Default::default();
    for p in &sequence.placements {
        let index = *item_indices
            .get(p.item.as_str())
            .ok_or("placement references a different media item")?;
        let binding = &bindings[index];
        let lane = match (
            p.video_track,
            p.audio_tracks.as_slice(),
            p.audio_streams.as_slice(),
        ) {
            (Some(1), [], []) => 0,
            (None, [1], [1]) => 1,
            (None, [2], [2]) => 2,
            _ => return Err("unsupported or unresolved per-lane stream selection".into()),
        };
        check_placement(p, native && lane != 0)?;
        if lanes[lane].len() == MAX_EDITS_PER_LANE {
            return Err("Resolve edit count limit exceeded".into());
        }
        let start = frames(p.record_start)?;
        let source_in = frames(p.source_in)?;
        let length = frames(p.duration)?;
        if length == 0
            || source_in
                .checked_add(length)
                .is_none_or(|end| end > binding.frames)
        {
            return Err("edit exceeds the verified source range".into());
        }
        let mut edit = Edit {
            start,
            length,
            source_in,
            envelope: None,
            binding: index,
        };
        if native {
            let receipt = sidecars.get(p.id.as_str()).copied();
            if lane == 0 && receipt.is_some() {
                return Err("unsupported Resolve picture placement receipt".into());
            }
            if lane != 0 {
                if graph.sidecar(audio::UNQUALIFIED_FADE_ADAPTER, &p.id).is_some() {
                    return Err(format!("{}: unqualified Resolve clip fade points", p.id).into());
                }
                edit.envelope = audio::prepare(p, source_in, length, rate(), receipt)?;
                if let Some(loss) = edit.envelope.and_then(Envelope::recovered_loss) {
                    recovered.insert(p.id.as_str(), loss);
                }
            }
        }
        lanes[lane].push(edit);
    }
    let binding_refs: Vec<_> = bindings.iter().collect();
    check_losses(graph, sequence, &binding_refs, None, |loss| {
        (main_bus_fader.is_some()
            && loss.object == sequence_id
            && loss.aspect == mixer::MAIN_BUS_LOSS.0
            && loss.reason == mixer::MAIN_BUS_LOSS.1)
            || (track_mutes.iter().any(|&muted| muted)
                && loss.object == sequence_id
                && loss.aspect == mixer::TRACK_MUTE_LOSS.0
                && loss.reason == mixer::TRACK_MUTE_LOSS.1)
            || recovered
                .get(loss.object.as_str())
                .is_some_and(|(aspect, reason)| loss.aspect == *aspect && loss.reason == *reason)
    })?;
    let duration = simple_range(&mut lanes[0])?;
    for lane in &mut lanes[1..] {
        if simple_range(lane)? != duration {
            return Err("picture and mono lanes must cover the same record extent".into());
        }
    }
    let end = origin
        .checked_add(duration)
        .filter(|e| *e < DAY)
        .ok_or("timeline exceeds supported timecode day")?;
    if duration == 0 {
        return Err("empty timeline".into());
    }
    let (markers, mut omitted_markers) = prepare_markers(graph, sequence, duration)?;
    omitted_markers.extend(
        items
            .iter()
            .flat_map(|item| &item.markers)
            .map(|marker| marker.owner_occurrence.clone()),
    );
    Ok(PreparedResolve {
        project_name: checked_name(&graph.project.name)?,
        sequence_name: checked_name(&sequence.name)?,
        origin,
        end,
        lanes,
        track_faders,
        main_bus_fader: main_bus_fader.unwrap_or_default(),
        track_mutes,
        bindings,
        markers,
        omitted_markers,
    })
}

#[derive(PartialEq, Eq)]
pub(crate) struct PreparedMarker {
    pub(crate) frame: u64,
    pub(crate) duration: u64,
    pub(crate) color_bit: u64,
    pub(crate) name: String,
    pub(crate) note: String,
    pub(crate) custom_data: String,
}

pub(crate) fn prepare_markers(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    duration: i64,
) -> Result<(Vec<PreparedMarker>, Vec<String>)> {
    let mut native = HashMap::new();
    if let Some(data) = graph.sidecar(ADAPTER, &sequence.id).and_then(|row| row.get("markers")) {
        let Value::Arr(rows) = data else {
            return Err("invalid Resolve sequence marker receipt".into());
        };
        for row in rows {
            let (Some(Value::Int(frame)), Some(Value::Int(color)), Some(Value::Str(custom))) =
                (row.get("frame"), row.get("color_bit"), row.get("custom_data"))
            else { return Err("invalid Resolve sequence marker receipt".into()) };
            if native.insert(*frame, (*color, custom.as_str())).is_some() {
                return Err("duplicate frame in Resolve sequence marker receipt".into());
            }
        }
    }
    let mut counts = HashMap::new();
    for marker in &sequence.markers {
        if let Some(frame) = marker.start.and_then(|t| frames_on(t, sequence.rate).ok()) {
            *counts.entry(frame).or_insert(0) += 1;
        }
    }
    let mut kept = Vec::new();
    let mut omitted = Vec::new();
    let mut budget = 32 << 10;
    for marker in &sequence.markers {
        let frame = marker.start.and_then(|t| frames_on(t, sequence.rate).ok());
        let saved = frame.filter(|_| marker.owner == MarkerOwner::Sequence)
            .and_then(|frame| native.remove(&frame));
        let custom = saved.map_or("", |(_, custom)| custom);
        let bytes = marker.name.len() + marker.comment.len() + custom.len() + 128;
        if kept.len() < 128 && bytes <= budget {
            if let Some(mark) = prepare_marker(marker, sequence.rate, duration, &counts) {
                if saved.is_some_and(|(color, _)| u64::try_from(color).ok() != Some(mark.color_bit)) {
                    return Err("Resolve sequence marker color disagrees with its receipt".into());
                }
                kept.push(PreparedMarker { custom_data: custom.into(), ..mark });
                budget -= bytes;
                continue;
            }
        }
        omitted.push(marker.owner_occurrence.clone());
    }
    if !native.is_empty() {
        return Err("Resolve sequence marker receipt lacks a matching marker".into());
    }
    kept.sort_unstable_by_key(|marker| marker.frame);
    Ok((kept, omitted))
}

fn prepare_marker(
    marker: &SourceMarker,
    rate: Rational,
    end: i64,
    counts: &HashMap<i64, usize>,
) -> Option<PreparedMarker> {
    if marker.owner != MarkerOwner::Sequence
        || marker.track.is_some()
        || marker.source.is_some()
        || marker.inherited
        || !marker.user.is_empty()
        || marker.binding.reason.is_some()
        || !marker.binding.placements.is_empty()
        || marker
            .owner_rate
            .is_some_and(|owner_rate| owner_rate != rate)
        || marker.owner_local.is_some() && marker.owner_local != marker.start
    {
        return None;
    }
    let frame = frames_on(marker.start?, rate).ok()?;
    let duration = frames_on(marker.duration, rate).ok()?;
    if duration == 0 || frame.checked_add(duration)? > end || counts.get(&frame) != Some(&1) {
        return None;
    }
    let color = marker.color.as_ref()?;
    let color_bit = match (color.palette.as_str(), color.name.as_str()) {
        ("resolve", name) => crate::marker_palette::bit(name)?,
        ("avid", "Blue") => 2,
        ("avid", "Red") => 32,
        _ => return None,
    };
    Some(PreparedMarker {
        frame: frame as u64,
        duration: duration as u64,
        color_bit,
        name: marker.name.clone(),
        note: marker.comment.clone(),
        custom_data: String::new(),
    })
}

pub(crate) fn qualified_origin(sequence: &SourceSequence) -> Result<i64> {
    sequence_origin(sequence, false)
}

fn sequence_origin(sequence: &SourceSequence, native: bool) -> Result<i64> {
    let clock = sequence
        .clock
        .as_ref()
        .ok_or("qualified sequence clock required")?;
    let origin = i64::try_from(clock.effective_count)?;
    let provenance = &clock.provenance;
    let qualified = if native {
        provenance.route == CLOCK_ROUTE
            && provenance.timecode_track == "Sm2Sequence.MediaExtents"
            && provenance.source_package == sequence.id
            && provenance.origin_count == clock.effective_count
            && provenance.source_offset == 0
            && provenance.source_rate == clock.rate
            && sequence.frame_size == Some((1920, 1080))
    } else {
        provenance.route == "direct sequence TC1"
    };
    if clock.rate != rate()
        || clock.display_base != 24
        || clock.drop
        || origin >= DAY
        || clock.exact_start() != sequence.start
        || !qualified
    {
        return Err(if native {
            "qualified non-drop 23.976 Resolve sequence extent required"
        } else {
            "qualified non-drop 23.976 direct TC1 required"
        }
        .into());
    }

    Ok(origin)
}
pub(crate) fn check_tracks(sequence: &SourceSequence) -> Result<()> {
    prepare_tracks(sequence, false).map(|_| ())
}

fn prepare_tracks(sequence: &SourceSequence, allow_faders: bool) -> Result<[i32; 2]> {
    let expected = [
        (StreamKind::Video, 1),
        (StreamKind::Audio, 1),
        (StreamKind::Audio, 2),
    ];
    let layout = "expected exactly V1 and two independent mono lanes with qualified track levels";
    if sequence.tracks.len() != expected.len() {
        return Err(layout.into());
    }
    let mut faders = [0; 2];
    for (kind, index) in expected {
        let track = sequence
            .tracks
            .iter()
            .find(|t| t.kind == kind && t.index == index)
            .ok_or(layout)?;
        if let Some(db) = track.level_db {
            if !allow_faders || kind != StreamKind::Audio {
                return Err(layout.into());
            }
            faders[index as usize - 1] =
                mixer::fader_ticks(db).ok_or("unsupported Resolve track fader level")?;
        }
    }
    Ok(faders)
}
pub(crate) fn check_binding<'a>(
    graph: &'a SourceGraph,
    binding: &VerifiedBinding,
) -> Result<&'a SourceItem> {
    check_binding_profile(graph, binding, false)
}

fn check_binding_profile<'a>(
    graph: &'a SourceGraph,
    binding: &VerifiedBinding,
    native: bool,
) -> Result<&'a SourceItem> {
    let item = graph
        .items
        .iter()
        .find(|item| item.id == binding.item_id)
        .ok_or("verified binding item absent")?;
    if let Some(atom) = &binding.atom {
        if native || item.umid.as_deref() != Some(&hex::encode_upper(atom.master_package)) {
            return Err("direct OP-Atom requires the matching saved Avid master identity".into());
        }
    }
    if item.rate != Some(rate()) || !item.stream_groups.is_empty() || item.media.len() != 3 {
        return Err("unsupported item rate, routing or stream count".into());
    }
    for (index, selector) in binding.streams.iter().enumerate() {
        let link = item
            .media
            .iter()
            .find(|link| link.media == selector.media_id)
            .ok_or("binding stream does not belong to the selected item")?;
        let source = graph
            .media
            .iter()
            .find(|s| s.id == link.media)
            .ok_or("source media absent")?;
        let clock_ok = if let Some(atom) = &binding.atom {
            let file = &atom.files[index];
            let receipt = graph.sidecar("avid.source", &source.id);
            let package = Value::Str(hex::encode_upper(file.package));
            let expected_track = Value::Int(if index == 0 { 1 } else { index as i64 });
            receipt.and_then(|fields| fields.get("file_package")) == Some(&package)
                && receipt.and_then(|fields| fields.get("native_track")) == Some(&expected_track)
                && matches!(source.locations.as_slice(), [path]
                    if std::fs::canonicalize(path).is_ok_and(|path| path == file.path))
                && file.agrees_with_clock(source)
        } else if native {
            let start = Rational::new(i128::from(binding.timecode_frames) * 1001, 24000);
            let receipt = graph.sidecar(ADAPTER, &source.id);
            source.start == start
                && matches!(source.locations.as_slice(), [path] if Path::new(path) == binding.path)
                && if index == 0 {
                    source.clock.as_ref().is_some_and(|clock| {
                        clock.rate == rate()
                            && clock.display_base == 24
                            && !clock.drop
                            && clock.effective_count == binding.timecode_frames as u64
                            && clock.provenance.route == CLOCK_ROUTE
                            && clock.provenance.timecode_track == "BtVideoTime.Timecode"
                            && clock.provenance.source_package == item.id
                            && clock.provenance.origin_count == clock.effective_count
                            && clock.provenance.source_offset == 0
                            && clock.provenance.source_rate == rate()
                    })
                } else {
                    let saved_index = Value::Int((index - 1) as i64);
                    source.clock.is_none()
                        && receipt.and_then(|data| data.get("saved_selector")) == Some(&saved_index)
                        && receipt.and_then(|data| data.get("saved_cache_track"))
                            == Some(&saved_index)
                }
        } else {
            let clock = source
                .clock
                .as_ref()
                .ok_or("qualified source media clock required")?;
            clock.rate == rate()
                && clock.display_base == 24
                && !clock.drop
                && clock.effective_count == binding.timecode_frames as u64
        };
        let layout_ok = if index == 0 {
            source.kind == StreamKind::Video
                && source.rate == Some(rate())
                && source.frame_size == Some((1920, 1080))
                && source.codec.as_deref() == Some(if native { "dnxhd_lb" } else { "AVdn" })
                && (!native || source.stream == Some(1))
        } else {
            source.kind == StreamKind::Audio
                && source.sample_rate == Some(48000)
                && source.bits == Some(24)
                && source.stream == Some(index as i64)
                && (!native || source.codec.as_deref() == Some("pcm"))
        };
        if !layout_ok
            || link.gain_db != 0.0
            || link.source_offset != Rational::new(0, 1)
            || !clock_ok
            || frames(source.duration)? != binding.frames
        {
            return Err("binding disagrees with the source clock, playable range, offset or lane assignment".into());
        }
    }

    Ok(item)
}
pub(crate) fn check_losses(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    bindings: &[&VerifiedBinding],
    group: Option<&str>,
    recovered: impl Fn(&Loss) -> bool,
) -> Result<()> {
    // An unsupported effect may exist only as a source loss, with no portable
    // placement field. Refuse losses on the selected content rather than
    // writing an apparently faithful cut.
    if sequence.placements.len() > 3 * MAX_EDITS_PER_LANE {
        return Err("Resolve edit count limit exceeded".into());
    }
    let mut scoped_ids = HashSet::from([sequence.id.as_str()]);
    scoped_ids.extend(group);
    scoped_ids.extend(bindings.iter().map(|b| b.item_id.as_str()));
    scoped_ids.extend(sequence.placements.iter().map(|p| p.id.as_str()));
    scoped_ids.extend(
        bindings
            .iter()
            .flat_map(|b| &b.streams)
            .map(|stream| stream.media_id.as_str()),
    );
    if let Some(loss) = graph.losses.iter().find(|loss| {
        (loss.object == "project"
            || scoped_ids.contains(loss.object.as_str())
            || loss
                .object
                .match_indices('/')
                .any(|(i, _)| scoped_ids.contains(&loss.object[..i])))
            && !recovered(loss)
    }) {
        return Err(format!(
            "unsupported source loss at {} ({}): {}",
            loss.object, loss.aspect, loss.reason
        )
        .into());
    }
    Ok(())
}
pub(crate) fn check_plain(p: &graph::Placement) -> Result<()> {
    check_placement(p, false)
}

pub(crate) fn check_linked_placement(p: &graph::Placement) -> Result<()> {
    let mono = p.video_track.is_none() && matches!(p.audio_tracks.as_slice(), [1 | 2 | 3]);
    check_placement_profile(p, mono, p.video_track == Some(1), true)
}

fn check_placement(p: &graph::Placement, allow_audio_controls: bool) -> Result<()> {
    check_placement_profile(p, allow_audio_controls, false, false)
}

fn check_placement_profile(
    p: &graph::Placement,
    allow_audio_controls: bool,
    allow_picture_controls: bool,
    allow_speed: bool,
) -> Result<()> {
    if p.gain_db != 0.0
        || (!allow_audio_controls && (p.volume_db.is_some() || !p.volume_keys.is_empty()))
        || !p.track_gains_db.is_empty()
        || !p.track_pans.is_empty()
        || !p.grouped_audio.is_empty()
        || (!allow_speed && p.speed.is_some())
        || p.frame_map.is_some()
        || (!allow_picture_controls && (p.transform.is_some() || !p.effects.is_empty()))
    {
        return Err(format!("unsupported Resolve placement semantics at {}", p.id).into());
    }
    Ok(())
}
pub(crate) fn checked_name(name: &str) -> Result<String> {
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return Err(
            "project and sequence names must be nonempty and at most 255 UTF-8 bytes".into(),
        );
    }
    Ok(name.to_owned())
}

pub(crate) fn utf8_path(path: &Path) -> Result<&str> {
    path.to_str()
        .filter(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
        .ok_or_else(|| "media path exceeds the UTF-8 path profile".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_record_grid() {
        assert_eq!(frames(Rational::new(1001, 1000).unwrap()).unwrap(), 24);
        assert!(frames(Rational::new(1, 1000).unwrap()).is_err());
        assert!(frames(Rational::new(-1001, 24000).unwrap()).is_err());
    }

    #[test]
    fn rejects_overlap_and_gap() {
        let mut cuts = [
            Edit {
                start: 0,
                length: 10,
                source_in: 0,
                envelope: None,
                binding: 0,
            },
            Edit {
                start: 9,
                length: 1,
                source_in: 0,
                envelope: None,
                binding: 0,
            },
        ];
        assert!(simple_range(&mut cuts).is_err());
        cuts[1].start = 11;
        assert!(simple_range(&mut cuts).is_err());
        cuts[1].start = 10;
        assert_eq!(simple_range(&mut cuts).unwrap(), 11);
    }
    #[test]
    fn sequence_range_markers_require_exact_grid_owner_and_unique_frame() {
        use graph::{source::SourceBinding, Label};
        let mut marker = SourceMarker {
            name: "Test range".into(),
            comment: "Note".into(),
            user: String::new(),
            color: Some(Label {
                palette: "resolve".into(),
                name: "Lavender".into(),
            }),
            start: Some(Rational::ZERO),
            duration: Rational::new(1001, 6000).unwrap(),
            track: None,
            owner: MarkerOwner::Sequence,
            owner_occurrence: "test".into(),
            owner_local: Some(Rational::ZERO),
            owner_rate: Some(rate()),
            binding: SourceBinding {
                placements: vec![],
                reason: None,
            },
            source: None,
            inherited: false,
        };
        let counts = HashMap::from([(0, 1)]);
        let result = prepare_marker(&marker, rate(), 48, &counts).unwrap();
        assert_eq!(
            (result.frame, result.duration, result.color_bit),
            (0, 4, 2048)
        );
        assert!(prepare_marker(&marker, rate(), 3, &counts).is_none());
        assert!(prepare_marker(&marker, rate(), 48, &HashMap::from([(0, 2)])).is_none());
        marker.duration = Rational::ZERO;
        assert!(prepare_marker(&marker, rate(), 48, &counts).is_none());
        marker.duration = Rational::new(1001, 6000).unwrap();
        marker.owner_local = Rational::new(1001, 24000);
        assert!(prepare_marker(&marker, rate(), 48, &counts).is_none());
    }
}
