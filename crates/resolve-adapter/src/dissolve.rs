//! The native left-aligned V1 Cross Dissolve on the qualified 23.976 project profile.
use crate::{binary::unpack, prepare::frames_on, Result};
use flate2::bufread::ZlibDecoder;
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Rational, StreamKind, Value,
};
use std::io::Read;

pub(crate) const ADAPTER: &str = "resolve.transitions";
pub(crate) const INTERPRETATION: &str = "resolve.left_cross_dissolve_8";
pub(crate) const CENTER_INTERPRETATION: &str = "resolve.center_cross_dissolve_8";
pub(crate) const AUDIO_INTERPRETATION: &str = "resolve.center_cross_fade_0db_8";
pub(crate) const AUDIO_TYPE: &str = "Cross Fade 0DB";
pub(crate) const AUDIO_FIELDS: &str =
    "00000002000000158012120000002c789c636660642016000000da0005";
const AUDIO_FIELDS_SHA: &str =
    "bc87b3394116f6be254b620a0fdfaec66a3130602aad452bef2dc8ef6d02ddc1";
pub(crate) const DURATION: i64 = 8;
pub(crate) const TYPE: &str = "Cross Dissolve";
pub(crate) const ALIGNMENT: &str = "3";
pub(crate) const POSITION: &str = "2";
pub(crate) const LOSS: &str =
    "Resolve transition fields retained; cut and handle mapping are unqualified";
pub(crate) const TIMELINE_EXPORT_LOSS: &str =
    "Resolve Cross Dissolve retained in its native sidecar; destination mapping required";
pub(crate) const EFFECT: &str = "000000020000003a800a37081238024a004a004a004a004a29088b015224ffffffe00000000000000c0000000000000000000800000000000c00000000000000f03f";
const EFFECT_SHA: &str = "8293817214186b6015e1ba95399050f8f7553ad91e9685183970fb58e33c2cae";
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Dissolve {
    pub(crate) cut: i64,
    pub(crate) before: i64,
    id: String,
    pub(crate) audio: bool,
}

impl Dissolve {
    pub(crate) fn type_label(&self) -> &'static str {
        if self.audio { AUDIO_TYPE } else { TYPE }
    }

    pub(crate) fn alignment(&self) -> &'static str {
        if self.before == DURATION { ALIGNMENT } else { "2" }
    }

    pub(crate) fn window(&self) -> std::ops::Range<i64> {
        let start = self.cut - self.before;
        start..start + DURATION
    }
}

pub(crate) struct SourceDissolve<'a> {
    pub(crate) prepared: Dissolve,
    pub(crate) incoming: &'a str,
    pub(crate) outgoing: &'a str,
}

/// The common graph stores source handles as overlapping placements. Resolve
/// stores the incoming preroll in its transition and abuts the visible edits.
pub(crate) fn from_source(sequence: &SourceSequence) -> Result<Option<SourceDissolve<'_>>> {
    let transition = match sequence.transitions.as_slice() {
        [] => return Ok(None),
        [transition] => transition,
        _ => return Err("Resolve source dissolve profile requires one transition".into()),
    };
    let refuse = |why| format!("Resolve transition {}: {why}", transition.id);
    let before = frames_on(transition.cut_offset, sequence.rate)?;
    if !supported_rate(sequence.rate)
        || transition.track != 1
        || transition.kind != StreamKind::Video
        || !matches!(transition.style.as_str(), "avid.EFF2_BLEND_DISSOLVE" | "premiere.ADBE Cross Dissolve")
        || frames_on(transition.duration, sequence.rate)? != DURATION
        || !matches!(before, 4 | DURATION)
        || transition.outgoing.reason.is_some()
        || transition.incoming.reason.is_some()
    {
        return Err(refuse("expected an eight-frame left-aligned or centered V1 dissolve").into());
    }
    let ([out], [input]) = (
        transition.outgoing.placements.as_slice(), transition.incoming.placements.as_slice()
    ) else {
        return Err(refuse("ambiguous endpoints").into());
    };
    let find = |id: &str| {
        sequence.placements.iter().find(|p| p.id == id && p.video_track == Some(1))
            .ok_or_else(|| refuse("missing V1 endpoint"))
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
        })
    {
        return Err(refuse("expected plain endpoints spanning the saved interval and a visible incoming edit").into());
    }
    Ok(Some(SourceDissolve {
        prepared: Dissolve { cut, before, id: transition.id.clone(), audio: false },
        incoming: input,
        outgoing: out,
    }))
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
    let Some(data) = graph.sidecar(ADAPTER, &sequence.id) else {
        return Ok(None);
    };
    let Value::Arr(rows) = data else {
        return Err("invalid Resolve transition receipt".into());
    };
    let id = |row| str_field(row, "id").unwrap_or("<missing ID>");
    let [row] = rows.as_slice() else {
        let first = rows.first().map(id).unwrap_or("<missing ID>");
        return Err(format!(
            "Resolve transition {first}: expected one transition on the selected timeline"
        )
        .into());
    };
    let unsupported = |reason| format!("Resolve transition {}: {reason}", id(row));
    let clock = sequence
        .clock
        .as_ref()
        .ok_or_else(|| unsupported("missing timeline clock"))?;
    let audio = str_field(row, "interpretation") == Some(AUDIO_INTERPRETATION);
    let before = match (str_field(row, "alignment_type"), str_field(row, "interpretation")) {
        (Some(ALIGNMENT), Some(INTERPRETATION)) => DURATION,
        (Some("2"), Some(CENTER_INTERPRETATION | AUDIO_INTERPRETATION)) => 4,
        _ => return Err(unsupported("unqualified Resolve dissolve alignment").into()),
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
        || str_field(row, "type_label") != Some(if audio { AUDIO_TYPE } else { TYPE })
        || str_field(row, "position") != Some(POSITION)
        || str_field(row, "saved_duration").and_then(|s| s.parse().ok()) != Some(DURATION)
        || str_field(row, "flags") != Some("0")
        || (if audio {
            row.get("effect_filters_sha256").is_some()
                || str_field(row, "fields_blob_sha256") != Some(AUDIO_FIELDS_SHA)
        } else {
            str_field(row, "effect_filters_sha256") != Some(EFFECT_SHA)
        })
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
        || frames_on(right.duration, sequence.rate)? < DURATION - before
        || frames_on(right.source_in, sequence.rate)? < before
    {
        return Err(unsupported("insufficient outgoing frames or incoming source handle").into());
    }
    if before != DURATION {
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
        check_handle(left, 0, DURATION - before)?;
        if audio { check_handle(right, before, 0)?; }
    }
    if !sequence
        .tracks
        .iter()
        .any(|track| track.index == 1 && track.kind == if audio { StreamKind::Audio } else { StreamKind::Video })
    {
        return Err(unsupported("missing transition track owner").into());
    }
    Ok(Some(Dissolve {
        cut,
        before,
        id: id(row).into(),
        audio,
    }))
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
