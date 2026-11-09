//! Audio edit volume from the PVOL control-code 1000 lane (AVB-0005).
use super::*;
use crate::io::Reader;
use crate::source::{Rate, SourceEvent, VolumeAutomation, VolumePoint};

const MAX_KEYS: usize = 4096;

#[derive(Debug)]
pub(super) struct Capture {
    position: i64,
    length: Option<i64>,
    rate: Option<Rate>,
    automation: VolumeAutomation,
}

impl Capture {
    pub(super) fn resolve(self, single: bool, event: &SourceEvent) -> VolumeAutomation {
        if matches!(self.automation, VolumeAutomation::Keys(_))
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
            return VolumeAutomation::Unsupported(
                "volume owner is not one exact same-rate mono edit",
            );
        }
        self.automation
    }
}

pub(super) fn capture(
    ctx: &Ctx<'_>,
    node: &Node,
    tracks: &[&Node],
    position: i64,
    charge: impl FnOnce(usize) -> bool,
) -> Option<Capture> {
    if !tracks.iter().any(|t| int(t, "control_code") == Some(1000)) {
        return None;
    }
    let length = int(node, "length");
    let rate = float(node, "edit_rate").and_then(Rate::from_avid);
    let keys = (|| {
        let [input, control] = tracks else {
            return None;
        };
        if int(input, "control_code").is_some()
            || int(control, "control_code") != Some(1000)
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
        let points = if inline.is_empty() {
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
            subframe(bytes, charge)?
        } else {
            if !matches!(ctrl.get("attributes"), None | Some(Value::Null))
                || !(2..=MAX_KEYS).contains(&inline.len())
                || !charge(inline.len())
            {
                return None;
            }
            let mut points = Vec::with_capacity(inline.len());
            for point in inline {
                points.push(inline_point(point)?);
            }
            points
        };
        let length = length?;
        let valid = points.iter().all(|k| {
            k.frame_denominator > 0
                && k.frame_numerator >= 0
                && k.frame_numerator as i128 <= length as i128 * k.frame_denominator as i128
                && (1..=i32::MAX as i64).contains(&k.gain_q29)
        }) && points.windows(2).all(|w| {
            (w[0].frame_numerator as i128 * w[1].frame_denominator as i128)
                < w[1].frame_numerator as i128 * w[0].frame_denominator as i128
        });
        valid.then_some(points)
    })();
    Some(Capture {
        position,
        length,
        rate,
        automation: keys.map_or(
            VolumeAutomation::Unsupported("unqualified PVOL control layout, clock or key data"),
            VolumeAutomation::Keys,
        ),
    })
}

fn inline_point(value: &Value) -> Option<VolumePoint> {
    let Value::Node(point) = value else {
        return None;
    };
    let Value::List(offset) = point.get("offset")? else {
        return None;
    };
    let [Value::Int(frame_numerator), Value::Int(frame_denominator)] = offset.as_slice() else {
        return None;
    };
    let Value::List(gain) = point.get("value")? else {
        return None;
    };
    let [Value::Int(gain_q29), Value::Int(1)] = gain.as_slice() else {
        return None;
    };
    if int(point, "time_scale") != Some(1)
        || *frame_denominator != 1
        || !matches!(point.get("pp"), Some(Value::List(v)) if v.is_empty())
    {
        return None;
    }
    Some(VolumePoint {
        frame_numerator: *frame_numerator,
        frame_denominator: 1,
        gain_q29: *gain_q29,
    })
}

fn subframe(bytes: &[u8], charge: impl FnOnce(usize) -> bool) -> Option<Vec<VolumePoint>> {
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
        let gain_q29 = i64::from(r.u32().ok()?);
        if r.u32().ok()? != 1 || r.u32().ok()? != 0 || r.u8().ok()? != 1 {
            return None;
        }
        points.push(VolumePoint {
            frame_numerator,
            frame_denominator,
            gain_q29,
        });
    }
    Some(points)
}
