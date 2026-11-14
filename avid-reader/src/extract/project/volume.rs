//! Mono audio edit volume and pan from PVOL control codes 1000 and 2000 (AVB-0005).
use super::*;
use crate::io::Reader;
use crate::source::{Automation, PanPoint, Rate, SourceEvent, VolumePoint};

const MAX_KEYS: usize = 4096;
const VOLUME: i64 = 1000;
const PAN: i64 = 2000;

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
                        || r.source_rate != r.record_rate
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
        if input.class != *b"SCLP"
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
            if !matches!(ctrl.get("attributes"), None | Some(Value::Null))
                || !(2..=MAX_KEYS).contains(&inline.len())
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
