//! Mono audio edit volume and pan from PVOL control codes 1000 and 2000 (AVB-0005).
use super::*;
use crate::io::Reader;
use crate::source::{Automation, PanPoint, Rate, SourceEvent, SourceSubclipGain, VolumePoint};

const MAX_KEYS: usize = 4096;
const VOLUME: i64 = 1000;
const PAN: i64 = 2000;

/// A saved subclip PVOL with one full-span sound input. This establishes the
/// source route and saved scalar controls, without inferring nested mixing.
pub(super) fn subclip_source<'a>(
    ctx: &Ctx<'a>,
    node: &Node,
) -> Option<(&'a Node, Option<SourceSubclipGain>)> {
    gain_source(ctx, node, GainInput::Clip)
}

pub(super) fn equalizer_gain_source<'a>(
    ctx: &Ctx<'a>,
    node: &Node,
) -> Option<(&'a Node, SourceSubclipGain)> {
    let (input, gain) = gain_source(ctx, node, GainInput::AafSequence)?;
    let gain = gain?;
    gain.pan_set.then_some((input, gain))
}

pub(super) fn stacked_neutral_source<'a>(ctx: &Ctx<'a>, node: &Node) -> Option<&'a Node> {
    let (input, None) = gain_source(ctx, node, GainInput::NeutralWrapper)? else { return None };
    Some(input)
}

/// A neutral wrapper and a single source segment with matching saved AAF routing.
pub(super) fn neutral_aaf_source<'a>(ctx: &Ctx<'a>, node: &Node) -> Option<&'a Node> {
    let (input, None) = gain_source(ctx, node, GainInput::NeutralAafSequence)? else { return None };
    Some(input)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GainInput {
    Clip,
    AafSequence,
    NeutralAafSequence,
    NeutralWrapper,
}

fn gain_source<'a>(
    ctx: &Ctx<'a>,
    node: &Node,
    shape: GainInput,
) -> Option<(&'a Node, Option<SourceSubclipGain>)> {
    let unset = |node: &Node, key| matches!(node.get(key), None | Some(Value::Null | Value::Ref(0)));
    if node.class != *b"PVOL"
        || node.get("effect_id")?.as_str() != Some("Audio Pan Volume")
        || int(node, "media_kind_id") != Some(KIND_SOUND)
        || int(node, "mc_mode") != Some(0)
        || int(node, "num_scalars") != Some(0)
        || int(node, "info_is_reversed") != Some(0)
        || !matches!(int(node, "is_trim_gain_effect"), None | Some(0))
        || int(node, "pan") != Some(16_384)
        || ["left_bob", "right_bob", "session_attrs", "precomputed", "param_list", "keyframes", "trackman"]
            .iter().any(|&key| !unset(node, key))
    {
        return None;
    }
    let &Value::Bool(pan_set) = node.get("pan_set")? else { return None };
    let level_q29 = i32::try_from(int(node, "level")?).ok()?;
    let gain = match node.get("level_set") {
        Some(Value::Bool(false)) if level_q29 == 1 << 29 && pan_set => None,
        Some(Value::Bool(true)) if level_q29 > 0 => Some(SourceSubclipGain { level_q29, pan_set }),
        _ => return None,
    };
    let Value::List(tracks) = node.get("tracks")? else { return None };
    let [Value::Node(track)] = tracks.as_slice() else { return None };
    // A sole saved input can omit its local slot label. The SCLP's source
    // track label still selects the channel on the referenced master.
    match track.get("index") {
        Some(Value::Int(1)) => {}
        None if int(track, "lock_number") == Some(0) => {}
        _ => return None,
    }
    if track.fields.iter().any(|(key, value)| {
        !matches!((*key, value), ("index" | "component", _) | ("lock_number", Value::Int(0)))
    }) {
        return None;
    }
    let rate = Rate::from_avid(float(node, "edit_rate")?)?;
    let length = int(node, "length").filter(|length| *length > 0)?;
    let mut input = ctx.deref(track.get("component"))?;
    let neutral_aaf = shape == GainInput::NeutralAafSequence;
    let aaf_sequence = neutral_aaf || shape == GainInput::AafSequence;
    let mut sequence_slot = None;
    let mut routing = None;
    if shape == GainInput::NeutralWrapper {
        input = subclip_source(ctx, input)
            .and_then(|(leaf, gain)| gain.is_none().then_some(leaf))?;
    }
    if aaf_sequence {
        let unset_fields = ["left_bob", "right_bob", "param_list", "precomputed", "session_attrs"];
        let no_attrs = |node: &Node| unset(node, "attributes") ||
            ctx.deref(node.get("attributes")).is_some_and(|attrs| {
                matches!(attrs.get("entries"), Some(Value::Attrs(values)) if values.is_empty())
            });
        if int(track, "index") != Some(1) || input.class != *b"SEQU"
            || int(input, "media_kind_id") != Some(KIND_SOUND)
            || float(input, "edit_rate").and_then(Rate::from_avid) != Some(rate)
            || int(input, "length").is_some_and(|saved| saved != length)
            || !unset_fields.iter().all(|field| unset(input, field))
        {
            return None;
        }
        if neutral_aaf || !no_attrs(input) {
            let attrs = ctx.deref(input.get("attributes"))?;
            let Value::Attrs(values) = attrs.get("entries")? else { return None };
            if neutral_aaf {
                routing = Some(values.as_slice());
            } else {
                let [(key, slot)] = values.as_slice() else { return None };
                if key != "_SAVED_AAF_SLOTID" {
                    return None;
                }
                sequence_slot = Some(slot.as_int()?);
            }
        }
        let Value::RefList(parts) = input.get("components")? else { return None };
        let [first, clip, last] = parts.as_slice() else { return None };
        for part in [first, last] {
            let filler = ctx.deref(Some(part))?;
            if filler.class != *b"FILL" || int(filler, "length") != Some(0)
                || int(filler, "media_kind_id") != Some(KIND_SOUND)
                || float(filler, "edit_rate").and_then(Rate::from_avid) != Some(rate)
                || !unset_fields.iter().all(|field| unset(filler, field))
                || !no_attrs(filler)
            {
                return None;
            }
        }
        input = ctx.deref(Some(clip))?;
    }
    let source_track = int(input, "track_id").filter(|track| *track > 0)?;
    if sequence_slot.is_some_and(|slot| slot != source_track) {
        return None;
    }
    let attrs = match ctx.deref(node.get("attributes")) {
        Some(attributes) => match attributes.get("entries")? {
            Value::Attrs(attrs) => attrs.as_slice(),
            _ => return None,
        },
        None if !pan_set && unset(node, "attributes") => &[],
        None => return None,
    };
    let attributes_qualified = if pan_set {
        qualified_center_pan_attrs(attrs, source_track, aaf_sequence, neutral_aaf)
    } else {
        attrs.is_empty()
    };
    if !attributes_qualified {
        return None;
    }
    if aaf_sequence && !attrs.iter().any(|(key, _)| key == "_SAVED_AAF_SLOTID") {
        return None;
    }
    if let Some(routing) = routing {
        let mut wrapper_routing = attrs.iter().filter(|(key, _)| is_aaf_route(key));
        let count = wrapper_routing.clone().count();
        // Neutral saves carry either the slot alone or all three route fields.
        if !matches!(count, 1 | 3) || count != routing.len()
            || !wrapper_routing.all(|entry| routing.contains(entry))
        {
            return None;
        }
    }
    (input.class == *b"SCLP"
        && int(input, "media_kind_id") == Some(KIND_SOUND)
        && int(input, "length") == Some(length)
        && float(input, "edit_rate").and_then(Rate::from_avid) == Some(rate)
        && int(input, "start_time").is_some_and(|time| time >= 0)
        && mob_id(input).is_some()).then_some((input, gain))
}

fn is_aaf_route(key: &str) -> bool {
    matches!(key, "_SAVED_AAF_SLOTID" | "_SAVED_AAF_CHANNELID" | "_SAVED_AAF_SOURCESLOTID")
}

fn qualified_center_pan_attrs(
    attrs: &[(String, Value)], source_track: i64, aaf_sequence: bool, neutral_aaf: bool,
) -> bool {
    let extra = attrs.iter().filter(|(key, _)| key == "_SAVED_AAF_PHYSICAL_TRACK_NUMBER"
        || aaf_sequence && key == "_SAVED_AAF_SLOTID"
        || neutral_aaf && is_aaf_route(key)).count();
    (2..=17).contains(&(attrs.len() - extra))
        && attrs.iter().any(|(key, _)| key == "_NEW_STYLE_CLIP_PAN")
        && attrs.iter().any(|(key, _)| key == "_PAN_1_1")
        && !attrs.iter().enumerate().any(|(index, (key, value))| {
            let expected = match key.as_bytes() {
                b"_NEW_STYLE_CLIP_PAN" => 1,
                // This optional AAF value only corroborates the saved SCLP.
                b"_SAVED_AAF_PHYSICAL_TRACK_NUMBER" => source_track,
                b"_SAVED_AAF_SLOTID" if aaf_sequence => source_track,
                b"_SAVED_AAF_CHANNELID" if neutral_aaf => source_track,
                b"_SAVED_AAF_SOURCESLOTID" if neutral_aaf => 1,
                b"_IGNORE_TRACKING" | b"_PAN_LFE_GAIN" => 0,
                b"_PAN_2_0" => 16_384,
                b"_PAN_1_1" | [b'_', b'P', b'A', b'N', b'_', b'2'..=b'8', b'_', b'1']
                | [b'_', b'P', b'A', b'N', b'_', b'3'..=b'8', b'_', b'0'] => 32_768,
                _ => return true,
            };
            value.as_int() != Some(expected) || attrs[..index].iter().any(|(previous, _)| previous == key)
        })
}

#[derive(Debug)]
pub(super) struct Capture<P> {
    position: i64,
    length: Option<i64>,
    rate: Option<Rate>,
    automation: Automation<P>,
}

impl<P> Capture<P> {
    pub(super) fn resolve(
        self, single: bool, event: &SourceEvent, owner_error: &'static str,
    ) -> Automation<P> {
        if matches!(self.automation, Automation::Keys(_))
            && (!single
                || event.event.combined.is_some()
                || event.event.speed.is_some()
                || event.range.as_ref().is_none_or(|r| {
                    r.record_start != self.position
                        || Some(r.length) != self.length
                        || Some(r.record_rate) != self.rate
                        || (r.source_rate != r.record_rate && !(event.motion_adapter_phase.is_some()
                            && r.length.checked_add(1) == Some(r.source_length)
                            && Rate::automatic_adapter_span(r.source_rate, r.record_rate) == Some((1001, 1000))))
                }))
        {
            return Automation::Unsupported(owner_error);
        }
        self.automation
    }
}

pub(super) fn capture_volume(
    ctx: &Ctx<'_>,
    node: &Node,
    tracks: &[&Node],
    position: i64,
    charge: impl FnOnce(usize) -> bool,
) -> Option<Capture<VolumePoint>> {
    capture(ctx, node, tracks, position, VOLUME, charge, |frame_numerator, frame_denominator, gain_q29| {
        (1..=i32::MAX as i64).contains(&gain_q29)
            .then_some(VolumePoint { frame_numerator, frame_denominator, gain_q29 })
    })
}

pub(super) fn capture_pan(
    ctx: &Ctx<'_>,
    node: &Node,
    tracks: &[&Node],
    position: i64,
    charge: impl FnOnce(usize) -> bool,
) -> Option<Capture<PanPoint>> {
    capture(ctx, node, tracks, position, PAN, charge, |frame_numerator, frame_denominator, raw_pan| {
        (0..=32768).contains(&raw_pan)
            .then_some(PanPoint { frame_numerator, frame_denominator, raw_pan })
    })
}

fn capture<P>(
    ctx: &Ctx<'_>,
    node: &Node,
    tracks: &[&Node],
    position: i64,
    code: i64,
    charge: impl FnOnce(usize) -> bool,
    make_point: impl Fn(i64, i64, i64) -> Option<P>,
) -> Option<Capture<P>> {
    if !tracks.iter().any(|t| int(t, "control_code") == Some(code)) {
        return None;
    }
    let length = int(node, "length");
    let rate = float(node, "edit_rate").and_then(Rate::from_avid);
    let keys = (|| {
        let length_frames = length?;
        let mut previous: Option<(i64, i64)> = None;
        let mut point = |num: i64, den: i64, raw| {
            if den <= 0 || num < 0 || i128::from(num) > i128::from(length_frames) * i128::from(den)
                || previous.is_some_and(|(n, d)| i128::from(n) * i128::from(den) >= i128::from(num) * i128::from(d))
            {
                return None;
            }
            previous = Some((num, den));
            make_point(num, den, raw)
        };
        let [input, control] = tracks else {
            return None;
        };
        if int(input, "control_code").is_some()
            || int(control, "control_code") != Some(code)
            || control.get("control_sub_code").is_some()
            || int(node, "is_trim_gain_effect") != Some(0)
        {
            return None;
        }
        let input = ctx.deref(input.get("component"))?;
        let ctrl = ctx.deref(control.get("component"))?;
        let audio_adapter = code == VOLUME && input.class == *b"SPED"
            && super::source_capture::automatic_adapter_attrs(ctx, input).is_some_and(|attrs| {
                super::source_capture::qualified_automatic_adapter(ctx, input, attrs, KIND_SOUND, length_frames)
                    .is_some_and(|(_, _, _, _, span, _, ratio)| ratio == (1001, 1000) && span == length_frames + 1)
            });
        if (input.class != *b"SCLP" && !audio_adapter)
            || ctrl.class != *b"CTRL"
            || int(input, "length") != length
            || int(ctrl, "length") != length
            || float(input, "edit_rate").and_then(Rate::from_avid) != rate
            || float(ctrl, "edit_rate").and_then(Rate::from_avid) != rate
            || int(ctrl, "media_kind_id") != Some(10)
            || int(ctrl, "interp_kind") != Some(3)
            || rate.is_none()
        {
            return None;
        }
        let Some(Value::List(inline)) = ctrl.get("control_points") else {
            return None;
        };
        if inline.is_empty() {
            let attrs = ctx.deref(ctrl.get("attributes"))?;
            let Some(Value::Attrs(entries)) = attrs.get("entries") else {
                return None;
            };
            let [(key, Value::Bytes(bytes))] = entries.as_slice() else {
                return None;
            };
            if key != "SUBFRAME_AUDIO_CTLPTS" {
                return None;
            }
            subframe(bytes, charge, point)
        } else {
            // The managed-audio adapter can carry one explicit point on
            // the record clock. Destination preparation reports the
            // unresolved Avid envelope behavior.
            let min_keys = if audio_adapter { 1 } else { 2 };
            if !matches!(ctrl.get("attributes"), None | Some(Value::Null))
                || !(min_keys..=MAX_KEYS).contains(&inline.len())
                || !charge(inline.len())
            {
                return None;
            }
            let mut points = Vec::with_capacity(inline.len());
            for value in inline {
                let (num, den, raw) = inline_point(value)?;
                points.push(point(num, den, raw)?);
            }
            Some(points)
        }
    })();
    Some(Capture {
        position,
        length,
        rate,
        automation: keys.map_or(
            Automation::Unsupported("unqualified PVOL control layout, clock or key data"),
            Automation::Keys,
        ),
    })
}

fn inline_point(value: &Value) -> Option<(i64, i64, i64)> {
    let Value::Node(point) = value else {
        return None;
    };
    let Value::List(offset) = point.get("offset")? else {
        return None;
    };
    let [Value::Int(frame_numerator), Value::Int(frame_denominator)] = offset.as_slice() else {
        return None;
    };
    let Value::List(value) = point.get("value")? else {
        return None;
    };
    let [Value::Int(raw), Value::Int(1)] = value.as_slice() else {
        return None;
    };
    if int(point, "time_scale") != Some(1)
        || *frame_denominator != 1
        || !matches!(point.get("pp"), Some(Value::List(v)) if v.is_empty())
    {
        return None;
    }
    Some((*frame_numerator, 1, *raw))
}

fn subframe<P>(
    bytes: &[u8], charge: impl FnOnce(usize) -> bool,
    mut point: impl FnMut(i64, i64, i64) -> Option<P>,
) -> Option<Vec<P>> {
    // This payload uses little-endian integers and literal tags independently of AVB framing.
    let mut r = Reader::new(bytes, 0, false);
    if r.bytes(4).ok()? != b"LsPt" || r.u32().ok()? != 1 {
        return None;
    }
    let count = r.u32().ok()? as usize;
    if !(2..=MAX_KEYS).contains(&count) || bytes.len() != 12 + 29 * count || !charge(count) {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        if r.bytes(4).ok()? != b"SgPt" || r.u32().ok()? != 21 {
            return None;
        }
        let frame_numerator = i64::from(r.s32().ok()?);
        let frame_denominator = i64::from(r.u32().ok()?);
        let raw = i64::from(r.u32().ok()?);
        if r.u32().ok()? != 1 || r.u32().ok()? != 0 || r.u8().ok()? != 1 {
            return None;
        }
        points.push(point(frame_numerator, frame_denominator, raw)?);
    }
    Some(points)
}
