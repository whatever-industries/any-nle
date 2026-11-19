//! Qualified Resolve video dissolves and mono audio crossfades.
use crate::{binary::unpack, prepare::frames_on, Result};
use flate2::bufread::ZlibDecoder;
use graph::{
    source::{SourceGraph, SourceSequence, SourceTransition},
    Loss, Rational, StreamKind, Value,
};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, io::Read};

pub(crate) const ADAPTER: &str = "resolve.transitions";
pub(crate) const INTERPRETATION: &str = "resolve.left_cross_dissolve_8";
pub(crate) const CENTER_INTERPRETATION: &str = "resolve.center_cross_dissolve_8";
pub(crate) const AUDIO_INTERPRETATION: &str = "resolve.center_cross_fade_0db_8";
const VIDEO_LEFT_PROFILE: &str = "resolve.left_cross_dissolve";
const VIDEO_CENTER_PROFILE: &str = "resolve.center_cross_dissolve";
const AUDIO_GAIN_PROFILE: &str = "resolve.center_cross_fade_0db";
const AUDIO_POWER_PROFILE: &str = "resolve.center_cross_fade_plus_3db";
pub(crate) const PREMIERE_AUDIO_STYLE: &str = "premiere.Constant Gain";
pub(crate) const PREMIERE_POWER_STYLE: &str = "premiere.Constant Power";
pub(crate) const AUDIO_FIELDS: &str =
    "00000002000000158012120000002c789c636660642016000000da0005";
const AUDIO_FIELDS_SHA: &str =
    "bc87b3394116f6be254b620a0fdfaec66a3130602aad452bef2dc8ef6d02ddc1";
pub(crate) const LEGACY_DURATION: i64 = 8;
pub(crate) const ALIGNMENT: &str = "3";
pub(crate) const POSITION: &str = "2";
pub(crate) const LOSS: &str =
    "Resolve transition fields retained; cut and handle mapping are unqualified";
pub(crate) const TIMELINE_EXPORT_LOSS: &str =
    "Resolve Cross Dissolve retained in its native sidecar; destination mapping required";
pub(crate) const EFFECT: &str = "000000020000003a800a37081238024a004a004a004a004a29088b015224ffffffe00000000000000c0000000000000000000800000000000c00000000000000f03f";
const EFFECT_DURATION: std::ops::Range<usize> = 50..54;
// Native synthetic transition envelope. Four inner bytes vary between saves.
pub(crate) const FIELDS: &str =
    "000000020000001b8012180000002c789c6366606400018ebc351e0c0400003124016f";

pub(crate) fn native_fields_match(encoded: &str) -> bool {
    fields_match(encoded, false)
}

pub(crate) fn timeline_export_fields_match(encoded: &str) -> bool {
    fields_match(encoded, true)
}

fn fields_match(encoded: &str, timeline_export: bool) -> bool {
    // The qualified body is one field containing 44 bytes of expanded state.
    if encoded.len() > 128 {
        return false;
    }
    let Ok(body) = unpack(encoded) else {
        return false;
    };
    // The 21.2 native timeline export added this field and cleared the four
    // bytes that vary in project saves. Do not relax the DRP interpretation.
    let body = if timeline_export {
        let Some(body) = body.strip_prefix(&[0x0a, 5, 0xa8, 1, 0x90, 0xa3, 5]) else {
            return false;
        };
        body
    } else {
        body.as_slice()
    };
    let [0x12, length, 0, 0, 0, 44, compressed @ ..] = body else {
        return false;
    };
    if usize::from(*length) != 4 + compressed.len() {
        return false;
    }
    let mut decoder = ZlibDecoder::new(compressed);
    let mut bytes = Vec::new();
    decoder.by_ref().take(45).read_to_end(&mut bytes).is_ok()
        && decoder.total_in() == compressed.len() as u64
        && bytes.len() == 44
        && bytes[..8] == [3, 0, 1, 0, 0, 0, 0, 0]
        && (!timeline_export || bytes[8..12] == [0; 4])
        && bytes[12..].iter().all(|&b| b == 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Video,
    Gain,
    Power,
}

impl Style {
    pub(crate) fn audio(self) -> bool {
        self != Self::Video
    }

    pub(crate) fn native_label(self) -> &'static str {
        match self {
            Self::Video => "Cross Dissolve",
            Self::Gain => "Cross Fade 0DB",
            Self::Power => "Cross Fade +3DB",
        }
    }

    pub(crate) fn from_label(label: &str) -> Option<Self> {
        match label {
            "Cross Dissolve" => Some(Self::Video),
            "Cross Fade 0DB" => Some(Self::Gain),
            "Cross Fade +3DB" => Some(Self::Power),
            _ => None,
        }
    }

    pub(crate) fn offset(self, duration: i64, alignment: &str) -> Option<i64> {
        if !(2..=i64::from(u32::MAX)).contains(&duration) {
            return None;
        }
        match alignment {
            ALIGNMENT if !self.audio() => Some(duration),
            "2" if duration % 2 == 0 => Some(duration / 2),
            _ => None,
        }
    }

    pub(crate) fn interpretation(self, duration: i64, alignment: &str) -> &'static str {
        let legacy = duration == LEGACY_DURATION;
        match (self, alignment, legacy) {
            (Self::Video, ALIGNMENT, true) => INTERPRETATION,
            (Self::Video, ALIGNMENT, false) => VIDEO_LEFT_PROFILE,
            (Self::Video, _, true) => CENTER_INTERPRETATION,
            (Self::Video, _, false) => VIDEO_CENTER_PROFILE,
            (Self::Gain, _, true) => AUDIO_INTERPRETATION,
            (Self::Gain, _, false) => AUDIO_GAIN_PROFILE,
            (Self::Power, _, _) => AUDIO_POWER_PROFILE,
        }
    }
}

pub(crate) fn video_effect(duration: i64) -> Result<Vec<u8>> {
    let mut bytes = hex::decode(EFFECT)?;
    bytes[EFFECT_DURATION].copy_from_slice(&u32::try_from(duration)?.to_le_bytes());
    Ok(bytes)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Dissolve {
    pub(crate) cut: i64,
    pub(crate) before: i64,
    pub(crate) duration: i64,
    pub(crate) id: String,
    pub(crate) style: Style,
}

impl Dissolve {
    pub(crate) fn audio(&self) -> bool {
        self.style.audio()
    }

    pub(crate) fn after(&self) -> i64 {
        self.duration - self.before
    }

    pub(crate) fn alignment(&self) -> &'static str {
        if self.before == self.duration { ALIGNMENT } else { "2" }
    }

    pub(crate) fn window(&self) -> std::ops::Range<i64> {
        let start = self.cut - self.before;
        start..start + self.duration
    }
}

pub(crate) struct SourceDissolve<'a> {
    pub(crate) prepared: Dissolve,
    pub(crate) incoming: &'a str,
    pub(crate) outgoing: &'a str,
}

/// The common graph stores source handles as overlapping placements. Resolve
/// stores the incoming preroll in its transition and abuts the visible edits.
pub(crate) fn from_source(sequence: &SourceSequence) -> Result<Vec<SourceDissolve<'_>>> {
    check_count(sequence.transitions.len())?;
    let prepared = sequence.transitions.iter()
        .map(|transition| prepare_source(sequence, transition))
        .collect::<Result<Vec<_>>>()?;
    validate_windows(&prepared)?;
    if sequence.transitions.iter().any(|transition| transition.style == "avid.EFF2_BLEND_DISSOLVE") {
        require_single_per_lane(&prepared)?;
    }
    Ok(prepared)
}

fn check_count(count: usize) -> Result<()> {
    if count > 2 * crate::prepare::MAX_EDITS_PER_LANE {
        return Err("Resolve transition count exceeds the linked layout limit".into());
    }
    Ok(())
}

pub(crate) fn require_single_per_lane(transitions: &[SourceDissolve<'_>]) -> Result<()> {
    let mut seen = [false; 2];
    for transition in transitions {
        let lane = usize::from(transition.prepared.audio());
        if std::mem::replace(&mut seen[lane], true) {
            return Err(format!(
                "Resolve transition {}: expected one transition per supported lane",
                transition.prepared.id
            ).into());
        }
    }
    Ok(())
}

fn validate_windows(transitions: &[SourceDissolve<'_>]) -> Result<()> {
    let mut ids = HashSet::new();
    let mut windows = Vec::with_capacity(transitions.len());
    for transition in transitions {
        let d = &transition.prepared;
        if !ids.insert(d.id.as_str()) {
            return Err(format!("duplicate Resolve transition ID {}", d.id).into());
        }
        let first = d.cut.checked_sub(d.before).filter(|first| *first >= 0);
        let Some(start) = first.filter(|first| first.checked_add(d.duration).is_some()) else {
            return Err(format!("Resolve transition {}: invalid blend interval", d.id).into());
        };
        windows.push((d.audio(), start, start + d.duration, d.id.as_str()));
    }
    windows.sort_unstable();
    for pair in windows.windows(2) {
        let [(a_lane, _, a_end, a_id), (b_lane, b_start, _, b_id)] = [pair[0], pair[1]];
        if a_lane == b_lane && a_end > b_start {
            return Err(format!("Resolve transitions {a_id} and {b_id} overlap on the same lane").into());
        }
    }
    Ok(())
}

fn prepare_source<'a>(
    sequence: &'a SourceSequence,
    transition: &SourceTransition,
) -> Result<SourceDissolve<'a>> {
    let refuse = |why| format!("Resolve transition {}: {why}", transition.id);
    let before = frames_on(transition.cut_offset, sequence.rate)?;
    let duration = frames_on(transition.duration, sequence.rate)?;
    let style = match (transition.kind, transition.style.as_str()) {
        (StreamKind::Video, "premiere.ADBE Cross Dissolve") if supported_rate(sequence.rate) => Style::Video,
        (StreamKind::Video, "avid.EFF2_BLEND_DISSOLVE")
            if supported_rate(sequence.rate) && duration == LEGACY_DURATION => Style::Video,
        (StreamKind::Audio, PREMIERE_AUDIO_STYLE) if sequence.rate == Rational::integer(24) => Style::Gain,
        (StreamKind::Audio, PREMIERE_POWER_STYLE) if sequence.rate == Rational::integer(24) => Style::Power,
        _ => return Err(refuse("unqualified transition style or edit rate").into()),
    };
    let audio = style.audio();
    let alignment = if before == duration { ALIGNMENT } else { "2" };
    if style.offset(duration, alignment) != Some(before)
        || transition.track != 1
        || transition.outgoing.reason.is_some()
        || transition.incoming.reason.is_some()
    {
        return Err(refuse("expected a left or centered V1 dissolve or centered 24 fps A1 crossfade").into());
    }
    let ([out], [input]) = (
        transition.outgoing.placements.as_slice(), transition.incoming.placements.as_slice()
    ) else {
        return Err(refuse("ambiguous endpoints").into());
    };
    let find = |id: &str| {
        sequence.placements.iter().find(|p| {
            p.id == id && if audio { p.audio_tracks.as_slice() == [1] } else { p.video_track == Some(1) }
        }).ok_or_else(|| refuse("missing transition endpoint on its saved lane"))
    };
    let outgoing = find(out)?;
    let incoming = find(input)?;
    let cut = frames_on(transition.cut, sequence.rate)?;
    let begin = transition.cut.checked_sub(transition.cut_offset).ok_or_else(|| refuse("invalid interval"))?;
    let after = transition.duration.checked_sub(transition.cut_offset).ok_or_else(|| refuse("invalid interval"))?;
    let end = transition.cut.checked_add(after).ok_or_else(|| refuse("invalid interval"))?;
    if outgoing.record_start.checked_add(outgoing.duration) != Some(end)
        || incoming.record_start != begin
        || incoming.duration <= transition.cut_offset
        || [outgoing, incoming].iter().any(|p| {
            p.transform.is_some() || !p.effects.is_empty() || p.speed.is_some() || p.frame_map.is_some()
                || (audio && (p.gain_db != 0.0 || p.volume_db.is_some() || !p.volume_keys.is_empty()))
        })
    {
        return Err(refuse("expected plain endpoints spanning the saved interval and a visible incoming edit").into());
    }
    Ok(SourceDissolve {
        prepared: Dissolve { cut, before, duration, id: transition.id.clone(), style },
        incoming: &incoming.id,
        outgoing: &outgoing.id,
    })
}

impl Dissolve {
    pub(crate) fn recover(&self, loss: &Loss, owner: &str) -> bool {
        loss.object == owner
            && loss.aspect == "transition"
            && matches!(
                loss.reason
                    .strip_prefix(self.id.as_str())
                    .and_then(|s| s.strip_prefix(": ")),
                Some(LOSS | TIMELINE_EXPORT_LOSS)
            )
    }
}

pub(crate) fn prepare(graph: &SourceGraph, sequence: &SourceSequence) -> Result<Option<Dissolve>> {
    Ok(prepare_native(graph, sequence)?.map(|source| source.prepared))
}

pub(crate) fn prepare_native<'a>(graph: &'a SourceGraph, sequence: &SourceSequence) -> Result<Option<SourceDissolve<'a>>> {
    let mut transitions = prepare_all_native(graph, sequence)?;
    if transitions.len() > 1 {
        return Err("this destination requires one transition on the selected timeline".into());
    }
    Ok(transitions.pop())
}

pub(crate) fn prepare_all_native<'a>(
    graph: &'a SourceGraph,
    sequence: &SourceSequence,
) -> Result<Vec<SourceDissolve<'a>>> {
    let Some(data) = graph.sidecar(ADAPTER, &sequence.id) else {
        return Ok(vec![]);
    };
    let Value::Arr(rows) = data else {
        return Err("invalid Resolve transition receipt".into());
    };
    if rows.is_empty() {
        return Err("empty Resolve transition receipt".into());
    }
    check_count(rows.len())?;
    let prepared = rows.iter().map(|row| prepare_native_row(graph, sequence, row))
        .collect::<Result<Vec<_>>>()?;
    validate_windows(&prepared)?;
    Ok(prepared)
}

fn prepare_native_row<'a>(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    row: &'a Value,
) -> Result<SourceDissolve<'a>> {
    let id = |row| str_field(row, "id").unwrap_or("<missing ID>");
    let unsupported = |reason| format!("Resolve transition {}: {reason}", id(row));
    let clock = sequence
        .clock
        .as_ref()
        .ok_or_else(|| unsupported("missing timeline clock"))?;
    let style = str_field(row, "type_label").and_then(Style::from_label)
        .ok_or_else(|| unsupported("unqualified Resolve transition type"))?;
    let audio = style.audio();
    let duration: i64 = str_field(row, "saved_duration").and_then(|s| s.parse().ok())
        .ok_or_else(|| unsupported("invalid saved duration"))?;
    let alignment = str_field(row, "alignment_type")
        .ok_or_else(|| unsupported("missing Resolve transition alignment"))?;
    let before = style.offset(duration, alignment)
        .ok_or_else(|| unsupported("unqualified Resolve dissolve alignment"))?;
    let after = duration - before;
    let effect_matches = if audio {
        row.get("effect_filters_sha256").is_none()
            && str_field(row, "fields_blob_sha256") == Some(AUDIO_FIELDS_SHA)
    } else {
        let hash = hex::encode(Sha256::digest(video_effect(duration)?));
        str_field(row, "effect_filters_sha256") == Some(hash.as_str())
    };
    if !supported_rate(sequence.rate)
        || sequence.frame_size != Some((1920, 1080))
        || clock.provenance.route != "Resolve archive clock"
        || (audio && sequence.rate != Rational::integer(24))
        || str_field(row, "kind") != Some(if audio { "audio" } else { "video" })
        || row.get("track") != Some(&Value::Int(1))
        || row.get("track_rate_numerator") != Some(&Value::Int(sequence.rate.num()))
        || row.get("track_rate_denominator") != Some(&Value::Int(sequence.rate.den()))
        || row.get("timeline_origin_frames").and_then(int)
            != i64::try_from(clock.effective_count).ok()
        || str_field(row, "interpretation") != Some(style.interpretation(duration, alignment))
        || str_field(row, "position") != Some(POSITION)
        || str_field(row, "flags") != Some("0")
        || !effect_matches
        || str_field(row, "native_track_id").is_none_or(str::is_empty)
        || str_field(row, "id").is_none_or(str::is_empty)
    {
        return Err(
            unsupported("unsupported type, alignment, track, duration or native profile").into(),
        );
    }

    let origin = i64::try_from(clock.effective_count)?;
    let cut = row
        .get("cut_frames")
        .and_then(int)
        .ok_or_else(|| unsupported("missing cut"))?;
    let start = str_field(row, "saved_start")
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|s| s.checked_sub(origin))
        .ok_or_else(|| unsupported("invalid saved start"))?;
    if start < 0 || start.checked_add(before) != Some(cut) {
        return Err(unsupported("invalid blend interval").into());
    }
    let outgoing =
        str_field(row, "outgoing").ok_or_else(|| unsupported("missing outgoing edit identity"))?;
    let incoming =
        str_field(row, "incoming").ok_or_else(|| unsupported("missing incoming edit identity"))?;
    let mut at_start = None;
    let mut at_end = None;
    for p in sequence
        .placements
        .iter()
        .filter(|p| if audio { p.video_track.is_none() && p.audio_tracks == [1] } else { p.video_track == Some(1) })
    {
        let first = frames_on(p.record_start, sequence.rate)?;
        let length = frames_on(p.duration, sequence.rate)?;
        let duplicate_out =
            first.checked_add(length) == Some(cut) && at_end.replace((p, length)).is_some();
        let duplicate_in = first == cut && at_start.replace(p).is_some();
        if duplicate_out || duplicate_in {
            return Err(unsupported("ambiguous transition endpoints").into());
        }
    }
    let (left, length) = at_end.ok_or_else(|| unsupported("missing outgoing edit at cut"))?;
    let right = at_start.ok_or_else(|| unsupported("missing incoming edit at cut"))?;
    if left.id != outgoing || right.id != incoming || outgoing == incoming {
        return Err(unsupported("saved endpoint identity disagrees with the cut").into());
    }
    if [left, right].iter().any(|p| {
        p.transform.is_some() || !p.effects.is_empty() || p.speed.is_some() || p.frame_map.is_some()
    }) {
        return Err(unsupported("expected plain edits at the transition").into());
    }
    if length < before
        || frames_on(right.duration, sequence.rate)? < after
        || frames_on(right.source_in, sequence.rate)? < before
    {
        return Err(unsupported("insufficient outgoing frames or incoming source handle").into());
    }
    if after != 0 {
        let check_handle = |edit: &graph::Placement, before, after| -> Result<()> {
            let kind = if audio { StreamKind::Audio } else { StreamKind::Video };
            let item = graph.items.iter().find(|item| item.id == edit.item)
                .ok_or_else(|| unsupported("missing endpoint media"))?;
            let mut streams = item.media.iter().filter_map(|link| {
                let media = graph.media.iter().find(|m| m.id == link.media)?;
                (media.kind == kind).then_some((link, media))
            });
            let (link, media) = streams.next().ok_or_else(|| unsupported("missing endpoint stream"))?;
            let available = frames_on(media.duration, sequence.rate)?;
            let source_in = frames_on(edit.source_in, sequence.rate)?;
            let length = frames_on(edit.duration, sequence.rate)?;
            if streams.next().is_some() || link.source_offset != Some(Rational::ZERO)
                || source_in < before
                || source_in.checked_add(length).and_then(|end| end.checked_add(after))
                    .is_none_or(|end| end > available)
                || (audio && (edit.volume_db.is_some() || edit.gain_db != 0.0
                    || !edit.volume_keys.is_empty() || !edit.grouped_audio.is_empty()
                    || media.sample_rate != Some(48000)))
            {
                return Err(unsupported("insufficient source handle or unsupported stream controls").into());
            }
            Ok(())
        };
        check_handle(left, 0, after)?;
        if audio { check_handle(right, before, 0)?; }
    }
    if !sequence
        .tracks
        .iter()
        .any(|track| track.index == 1 && track.kind == if audio { StreamKind::Audio } else { StreamKind::Video })
    {
        return Err(unsupported("missing transition track owner").into());
    }
    Ok(SourceDissolve {
        prepared: Dissolve { cut, before, duration, id: id(row).into(), style },
        incoming,
        outgoing,
    })
}

pub(crate) fn supported_rate(rate: Rational) -> bool {
    rate == Rational::integer(24) || rate == Rational::new(24000, 1001).unwrap()
}

fn int(value: &Value) -> Option<i64> {
    match value {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

fn str_field<'a>(row: &'a Value, name: &str) -> Option<&'a str> {
    match row.get(name)? {
        Value::Str(value) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_native_video_duration_without_changing_the_rest_of_the_effect() {
        let legacy = video_effect(8).unwrap();
        assert_eq!(hex::encode(&legacy), EFFECT);
        let mut longer = video_effect(304).unwrap();
        assert_eq!(&longer[EFFECT_DURATION], &[48, 1, 0, 0]);
        longer[EFFECT_DURATION].copy_from_slice(&legacy[EFFECT_DURATION]);
        assert_eq!(longer, legacy);
        assert_eq!(Style::Video.offset(12, ALIGNMENT), Some(12));
        assert_eq!(Style::Gain.offset(10, "2"), Some(5));
        assert_eq!(Style::Power.offset(9, "2"), None);
    }

    #[test]
    fn repeated_transitions_require_separate_windows_on_each_lane() {
        let row = |id: &str, cut, audio| SourceDissolve {
            prepared: Dissolve { cut, before: 4, duration: LEGACY_DURATION,
                id: id.into(), style: if audio { Style::Gain } else { Style::Video } },
            incoming: "in",
            outgoing: "out",
        };
        assert!(validate_windows(&[
            row("later", 24, false), row("audio", 17, true), row("earlier", 16, false),
        ]).is_ok());
        let overlap = validate_windows(&[row("a", 16, false), row("b", 23, false)]);
        assert!(overlap.unwrap_err().to_string().contains("a and b overlap"));
        let repeated = validate_windows(&[row("a", 16, false), row("a", 24, true)]);
        assert!(repeated.unwrap_err().to_string().contains("duplicate Resolve transition ID a"));
    }

    #[test]
    fn qualifies_saved_envelope_variants_and_refuses_malformed_compression() {
        let retained = "000000020000001b8012180000002c789c6366606400010e079bb30c0400002d1f0156";
        assert!(native_fields_match(retained));
        assert!(native_fields_match(FIELDS));
        assert!(!native_fields_match(&FIELDS[..FIELDS.len() - 2]));
        let bad_checksum = format!("{}00", &FIELDS[..FIELDS.len() - 2]);
        assert!(!native_fields_match(&bad_checksum));
        let drt = "000000020000001c800a05a80190a30512120000002c789c636660642016000000da0005";
        assert!(timeline_export_fields_match(drt));
        assert!(!native_fields_match(drt));
        assert!(!timeline_export_fields_match(retained));
    }
}
