//! Avid AniMatte (TrackEffect `EFF_ANIMATTE`) shapes and one-point tracking: a bounded read of
//! the observed 1920x1080 records. Unknown settings, reference layouts and payload
//! revisions are not decoded (`None`), so the caller keeps them as "not converted".
//!
//! Ported from FilmCraft (https://github.com/storytold/filmcraft, PR #1056 at
//! dd9457a978040899dfb171e63d9979c41678ad5c, `crates/interchange/src/avid_bin/animatte.rs`;
//! MIT, Copyright (c) 2026 ArtCraft Team and the FilmCraft contributors; see NOTICE). Changes:
//! reads this crate's [`AvbFile`] raw payloads instead of FilmCraft's `Bin`, returns `None`
//! instead of an error for unreadable references, no serde. Format facts: Avid KB AVB-0015
//! (shapes) and AVB-0016 (tracker), author-only.
//!
//! Layout: the effect's FXPS holds timed shape states (`2 18 ff ff ff ff 0 0 0 0`, u32 LE
//! count, then per state a 106-byte header whose u32 at 1 is the time in 1/2^30 of the effect
//! length, a u32 LE size and a big-endian `BZFX` blob: u32 shape count, per shape a 268-byte
//! header + u32 point count + 62-byte points with f64 BE position, incoming and outgoing
//! handles). The tracker chain TKMN -> TKDS/TKPS -> TKDA ("Point A") / TKPA -> two PRCL curves
//! gives one sample per frame in normalized frame space.

use crate::file::AvbFile;

const DEFAULT_HEADER: [u8; 268] = [
    0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x04, 0x40, 0x59, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x32, 0x00, 0x00, 0x00, 0x00, 0x40, 0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x60, 0xe0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xeb, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xeb, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x64, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00,
    0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0x00,
    0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x7f, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x20, 0x3f, 0xf0,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3f, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const MAX_STATES: usize = 128;
const MAX_SHAPES: usize = 64;
const MAX_POINTS: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct AniMattePoint {
    /// Avid AniMatte units: at 1920x1080, pixel = (960 + x * 1920/720, 540 + y * 1080/243).
    pub xy: [f64; 2],
    /// Handles relative to `xy`, same units.
    pub incoming: [f64; 2],
    pub outgoing: [f64; 2],
    pub corner: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AniMatteShape {
    pub id: u32,
    /// Shape-header Key Out flag. A model matte has one mode for the entire effect.
    pub key_out: bool,
    /// Shape-header UI opacity, 0..100.
    pub opacity: f64,
    /// Two stored feather values, in wire order. H/V assignment and render units are unproved.
    pub feather: [f64; 2],
    pub points: Vec<AniMattePoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AniMatteState {
    /// Frame within the effect (0 = first, last = length - 1).
    pub frame: u32,
    pub shapes: Vec<AniMatteShape>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AniMatteData {
    pub states: Vec<AniMatteState>,
    /// Sampled foreground Point A coordinates in normalized frame space, one per frame.
    pub tracker_xy: Option<Vec<[f64; 2]>>,
    /// Controls whose stored facts survive in the model but whose rendering is not translated.
    pub not_converted: Vec<String>,
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn le64f(data: &[u8], at: usize) -> Option<f64> {
    let v = f64::from_le_bytes(data.get(at..at.checked_add(8)?)?.try_into().ok()?);
    v.is_finite().then_some(v)
}

fn be64f(data: &[u8], at: usize) -> Option<f64> {
    let v = f64::from_be_bytes(data.get(at..at.checked_add(8)?)?.try_into().ok()?);
    (v.is_finite() && v.abs() <= 1_000_000.0).then_some(v)
}

/// Allow only the four observed shape-header fields to differ from the default. Any changed
/// unrecognized header byte refuses the effect; Bias and Anti-alias storage is not isolated.
fn shape_header(data: &[u8]) -> Option<(bool, f64, [f64; 2])> {
    if data.len() != DEFAULT_HEADER.len()
        || !data.iter().enumerate().all(|(i, v)| (4..8).contains(&i) || (23..47).contains(&i) || i == 56 || *v == DEFAULT_HEADER[i])
        || !matches!(data[56], 0 | 1)
    {
        return None;
    }
    let opacity = be64f(data, 23)?;
    let feather = [be64f(data, 31)?, be64f(data, 39)?];
    if !(0.0..=100.0).contains(&opacity) || feather.iter().any(|v| !(0.0..=10_000.0).contains(v)) {
        return None;
    }
    Some((data[56] == 1, opacity, feather))
}

fn shapes(blob: &[u8]) -> Option<Vec<AniMatteShape>> {
    if blob.get(..8)? != b"BZFX\0\0\0\0" || be32(blob, 12)? != 4 {
        return None;
    }
    let count = usize::try_from(be32(blob, 8)?).ok()?;
    if count == 0 || count > MAX_SHAPES {
        return None;
    }
    let mut at = 12usize;
    let mut total_points = 0usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let header = blob.get(at..at.checked_add(268)?)?;
        let (key_out, opacity, feather) = shape_header(header)?;
        let id = be32(blob, at + 4)?;
        let count = usize::try_from(be32(blob, at + 268)?).ok()?;
        total_points = total_points.checked_add(count)?;
        if !(3..=MAX_POINTS).contains(&count) || total_points > MAX_POINTS {
            return None;
        }
        at = at.checked_add(272)?;
        let mut points = Vec::with_capacity(count);
        for _ in 0..count {
            let p = blob.get(at..at.checked_add(62)?)?;
            if p.get(..4)? != [0; 4] || p.get(8) != Some(&1) || !matches!(p.get(9), Some(0 | 1)) || p.get(10..14)? != [0, 0, 0, 2] {
                return None;
            }
            points.push(AniMattePoint {
                xy: [be64f(p, 14)?, be64f(p, 22)?],
                incoming: [be64f(p, 30)?, be64f(p, 38)?],
                outgoing: [be64f(p, 46)?, be64f(p, 54)?],
                corner: p[9] == 1,
            });
            at += 62;
        }
        out.push(AniMatteShape { id, key_out, opacity, feather, points });
    }
    (at == blob.len()).then_some(out)
}

fn states(payload: &[u8], frames: u32) -> Option<Vec<AniMatteState>> {
    if frames < 2 || payload.get(..10)? != [2, 18, 255, 255, 255, 255, 0, 0, 0, 0] || payload.get(payload.len().checked_sub(2)?..)? != [1, 3] {
        return None;
    }
    let count = usize::try_from(le32(payload, 10)?).ok()?;
    if !(2..=MAX_STATES).contains(&count) {
        return None;
    }
    let mut header_at = 17usize;
    let mut out: Vec<AniMatteState> = Vec::with_capacity(count);
    for _ in 0..count {
        let header = payload.get(header_at..header_at.checked_add(106)?)?;
        let timing = le32(header, 1)?;
        if timing > (1 << 30) {
            return None;
        }
        let frame = (u64::from(timing) * u64::from(frames - 1) + (1 << 29)) / (1 << 30);
        let frame = u32::try_from(frame).ok()?;
        if out.last().is_some_and(|prior| frame <= prior.frame) {
            return None;
        }
        let size_at = header_at + 106;
        let size = usize::try_from(le32(payload, size_at)?).ok()?;
        if !(12..=8 * 1024 * 1024).contains(&size) {
            return None;
        }
        let blob_at = size_at.checked_add(4)?;
        let end = blob_at.checked_add(size)?;
        let shape_state = shapes(payload.get(blob_at..end)?)?;
        out.push(AniMatteState { frame, shapes: shape_state });
        header_at = end;
    }
    if header_at != payload.len() - 2 || out.first()?.frame != 0 || out.last()?.frame != frames - 1 {
        return None;
    }
    // Fixed topology: the same shapes with the same point counts in every state.
    let first_ids: Vec<_> = out.first()?.shapes.iter().map(|s| (s.id, s.points.len())).collect();
    if out.iter().any(|state| state.shapes.iter().map(|s| (s.id, s.points.len())).collect::<Vec<_>>() != first_ids) {
        return None;
    }
    Some(out)
}

/// One f64 sample per frame from a PRCL parameter curve (tracker X or Y).
fn curve_samples(payload: &[u8], frames: u32) -> Option<Vec<f64>> {
    let count = usize::try_from(le32(payload, 55)?).ok()?;
    if count != frames as usize
        || count > 8192
        || payload.get(..2)? != [2, 3]
        || payload.len() != 59usize.checked_add(count.checked_mul(22)?)?.checked_add(15)?
    {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for frame in 0..count {
        let at = 59 + frame * 22;
        if le32(payload, at)? != frame as u32
            || le32(payload, at + 4)? != 1
            || payload.get(at + 8..at + 12)? != [1, 0, 0, 0]
            || payload.get(at + 20..at + 22)? != [0, 0]
        {
            return None;
        }
        let value = le64f(payload, at + 12)?;
        if !(-4.0..=4.0).contains(&value) {
            return None;
        }
        out.push(value);
    }
    Some(out)
}

fn object(file: &AvbFile, reference: u32) -> Option<([u8; 4], &[u8])> {
    if reference == 0 {
        return None;
    }
    file.raw(reference)
}

/// The one tracker point ("Point A") assigned to `shape_id`, sampled per frame.
fn point_a_tracking(file: &AvbFile, reference: u32, shape_id: u32, frames: u32) -> Option<Vec<[f64; 2]>> {
    let (class, p) = object(file, reference)?;
    if class != *b"TKMN" || p.len() != 11 || p.get(..2) != Some(&[2, 1]) || p.get(10) != Some(&3) {
        return None;
    }
    let (data_class, d) = object(file, le32(p, 2)?)?;
    let (slots_class, s) = object(file, le32(p, 6)?)?;
    if slots_class != *b"TKPS"
        || s.len() != 47
        || s.get(..6) != Some(&[2, 1, 0x22, 0, 0x49, 0x49])
        || le32(s, 38) != Some(1)
        || s.last() != Some(&3)
        || data_class != *b"TKDS"
        || d.len() != 15
        || d.get(..2) != Some(&[2, 1])
        || le32(d, 2) != Some(1)
        || d.get(10..) != Some(&[1, 1, 0x42, 1, 3])
    {
        return None;
    }
    let (assignment_class, a) = object(file, le32(s, 42)?)?;
    let (point_class, t) = object(file, le32(d, 6)?)?;
    if assignment_class != *b"TKPA"
        || a.len() != 101
        || a.get(..6) != Some(&[2, 1, 0x60, 0, 0x49, 0x49])
        || le32(a, 14) != Some(shape_id)
        || a.get(58..60) != Some(&[8, 0])
        || a.get(60..65) != Some(&[1, 0, 0, 0, 0])
        || a.get(100) != Some(&3)
        || !(1..8).all(|i| a.get(60 + i * 5..65 + i * 5) == Some(&[0, 255, 255, 255, 255]))
        || point_class != *b"TKDA"
        || t.len() != 157
        || t.get(..6) != Some(&[2, 1, 0x38, 0, 0x49, 0x49])
        || le32(t, 10) != Some(0)
        || t.get(18..22) != Some(&[0x80, 7, 0x38, 4])
        || t.get(22..31) != Some(&[7, 0, b'P', b'o', b'i', b'n', b't', b' ', b'A'])
        || t.get(64..66) != Some(&[11, 0])
    {
        return None;
    }
    let (x_class, x) = object(file, le32(t, 66)?)?;
    let (y_class, y) = object(file, le32(t, 70)?)?;
    if x_class != *b"PRCL" || y_class != *b"PRCL" {
        return None;
    }
    let (xs, ys) = (curve_samples(x, frames)?, curve_samples(y, frames)?);
    Some(xs.into_iter().zip(ys).map(|(x, y)| [x, y]).collect())
}

/// Observed 1920x1080 AniMatte records of one TrackEffect (`keyframes` = its FXPS,
/// `manager` = its `trackman` or 0, `frames` = its length). Acceleration is preserved as a
/// reported loss because its exact interpolation law is unknown. Unknown layouts are refused.
pub fn animatte(file: &AvbFile, keyframes: u32, manager: u32, frames: u32, acceleration_raw: u32) -> Option<AniMatteData> {
    if file.big_endian || frames > 8192 || acceleration_raw > (1 << 30) {
        return None;
    }
    let (class, payload) = object(file, keyframes)?;
    if class != *b"FXPS" {
        return None;
    }
    let states = states(payload, frames)?;
    let first = &states.first()?.shapes;
    let mode = first.first()?.key_out;
    // model/1 currently has effect-wide inversion and static per-shape opacity/feather.
    // Refuse mixed modes or animated header controls rather than silently flattening them.
    if states.iter().any(|state| {
        state.shapes.iter().zip(first).any(|(shape, base)| {
            shape.key_out != mode || shape.opacity != base.opacity || shape.feather != base.feather
        })
    }) {
        return None;
    }
    let tracker_xy = if manager == 0 {
        None
    } else {
        // Tracking is only read for one static shape moved by one point.
        if states.len() != 2 || states[0].shapes.len() != 1 || states[0].shapes != states[1].shapes {
            return None;
        }
        Some(point_a_tracking(file, manager, states[0].shapes[0].id, frames)?)
    };
    let mut not_converted = Vec::new();
    if acceleration_raw != 0 {
        not_converted.push(format!("AniMatte Acceleration raw {} (rendered easing not converted)", acceleration_raw));
    }
    if first.iter().any(|shape| shape.feather != [0.0, 0.0]) {
        not_converted.push("AniMatte feather values retained in stored order; H/V assignment and rendering units uncalibrated".into());
    }
    Some(AniMatteData { states, tracker_xy, not_converted })
}

/// The model's neutral matte: per shape, keyframes (frame within the edit) of closed paths;
/// points `[x, y, in_x, in_y, out_x, out_y]` as fractions of the source frame (origin top-left,
/// y down; handles relative to the point). Avid -> fraction at 1920x1080: x / 720 and y / 243
/// from the centre (KB AVB-0015 render fit; FilmCraft PR #1056). One tracked point shifts the
/// static shape by the tracker's movement from its first sample, every frame.
pub fn matte_json(data: &AniMatteData) -> crate::json::Json {
    use crate::json::Json;
    let point = |p: &AniMattePoint, shift: [f64; 2]| {
        let (ix, iy, ox, oy) = if p.corner {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            (p.incoming[0] / 720.0, p.incoming[1] / 243.0, p.outgoing[0] / 720.0, p.outgoing[1] / 243.0)
        };
        Json::Array(
            [0.5 + p.xy[0] / 720.0 + shift[0], 0.5 + p.xy[1] / 243.0 + shift[1], ix, iy, ox, oy].into_iter().map(Json::Float).collect(),
        )
    };
    let key = |frame: u32, shape: &AniMatteShape, shift: [f64; 2]| {
        Json::obj().with("frame", Json::Int(frame as i64)).with("points", Json::Array(shape.points.iter().map(|p| point(p, shift)).collect()))
    };
    let first = &data.states[0];
    let shapes = (0..first.shapes.len())
        .map(|index| {
            let keys: Vec<Json> = match &data.tracker_xy {
                Some(samples) => {
                    let origin = samples[0];
                    samples.iter().enumerate().map(|(f, xy)| key(f as u32, &first.shapes[index], [xy[0] - origin[0], xy[1] - origin[1]])).collect()
                }
                None if data.states.iter().all(|s| s.shapes[index] == first.shapes[index]) => vec![key(0, &first.shapes[index], [0.0, 0.0])],
                None => data.states.iter().map(|s| key(s.frame, &s.shapes[index], [0.0, 0.0])).collect(),
            };
            let shape = &first.shapes[index];
            let mut out = Json::obj().with("keyframes", Json::Array(keys));
            if shape.opacity != 100.0 {
                out.push("opacity", Json::Float(shape.opacity));
            }
            if shape.feather != [0.0, 0.0] {
                out.push("feather", Json::Array(shape.feather.iter().copied().map(Json::Float).collect()));
            }
            out
        })
        .collect();
    let mut out = Json::obj().with("shapes", Json::Array(shapes)).with("inverted", Json::Bool(first.shapes[0].key_out));
    if !data.not_converted.is_empty() {
        out.push("not_converted", Json::Array(data.not_converted.iter().cloned().map(Json::str).collect()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut blob = b"BZFX\0\0\0\0".to_vec();
        blob.extend(1u32.to_be_bytes());
        let mut header = DEFAULT_HEADER;
        header[4..8].copy_from_slice(&42u32.to_be_bytes());
        blob.extend(header);
        blob.extend(4u32.to_be_bytes());
        for id in 0..4u32 {
            blob.extend(0u32.to_be_bytes());
            blob.extend(id.to_be_bytes());
            blob.extend([1, 1, 0, 0, 0, 2]);
            for value in [id as f64, 1.0, 0.0, 0.0, 0.0, 0.0] {
                blob.extend(value.to_be_bytes());
            }
        }
        let mut effect = vec![2, 18, 255, 255, 255, 255, 0, 0, 0, 0];
        effect.extend(2u32.to_le_bytes());
        effect.extend([0; 3]);
        for time in [0u32, 1 << 30] {
            let mut prefix = [0u8; 106];
            prefix[1..5].copy_from_slice(&time.to_le_bytes());
            effect.extend(prefix);
            effect.extend((blob.len() as u32).to_le_bytes());
            effect.extend(&blob);
        }
        effect.extend([1, 3]);
        effect
    }

    #[test]
    fn bounded_default_shape_and_mutations() {
        let valid = sample();
        let parsed = states(&valid, 46).expect("two bounded states");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].shapes[0].points.len(), 4);
        assert_eq!((parsed[0].frame, parsed[1].frame), (0, 45));
        for len in 0..valid.len() {
            let run = std::panic::catch_unwind(|| states(&valid[..len], 46));
            assert!(run.is_ok(), "truncated FXPS panicked at {len}");
            assert!(run.expect("checked").is_none());
        }
        for at in (0..valid.len()).step_by(17) {
            let mut changed = valid.clone();
            changed[at] ^= 0xff;
            assert!(std::panic::catch_unwind(|| states(&changed, 46)).is_ok(), "bit flip panicked at {at}");
        }
    }

    #[test]
    fn observed_header_fields_and_unknowns() {
        let mut header = DEFAULT_HEADER;
        header[4..8].copy_from_slice(&42u32.to_be_bytes());
        assert_eq!(shape_header(&header), Some((false, 100.0, [0.0, 0.0])));
        header[56] = 1;
        header[23..31].copy_from_slice(&46f64.to_be_bytes());
        header[31..39].copy_from_slice(&21f64.to_be_bytes());
        header[39..47].copy_from_slice(&7f64.to_be_bytes());
        assert_eq!(shape_header(&header), Some((true, 46.0, [21.0, 7.0])));
        header[56] = 2;
        assert!(shape_header(&header).is_none());
        header[56] = 1;
        header[23..31].copy_from_slice(&f64::NAN.to_be_bytes());
        assert!(shape_header(&header).is_none());
        header[23..31].copy_from_slice(&46f64.to_be_bytes());
        header[50] ^= 1; // Unmapped header controls must not be silently ignored.
        assert!(shape_header(&header).is_none());
    }
}
