//! Avid Resize (TrackEffect `EFF2_BLEND_RESIZE`) and 3D Warp (`EFF2_SBLEND`) static position,
//! scale, rotation and crop, interpreted from the Avid KB (AVB-0009, pinned in
//! research-dependencies.json; all author-only, pending independent review). Each parameter is a
//! PRIT whose file-order GUID is `<prefix>568d7e84d511935a50f857c10000`; values are Avid UI units.
//!
//! Output is the model's neutral transform: fractions of the frame measured from its centre,
//! x right and y down. At 1920x1080 the KB's render calibration gives (S15 warp-transform
//! calibration, resize-render-results):
//! - Warp Position / Axis: 1920/999 px per unit right, 1080/999 px per unit UP (Avid +Y is up);
//!   Rotation Z degrees clockwise about the Axis; Scale percent about the frame centre.
//! - Warp Crop: edges at centre + (1920 x / 1998, -1080 y / 1998), in source space before the
//!   transforms (-999 / 999 / 999 / -999 = no crop).
//! - Resize -> Warp (exact on rendered pairs): X r, Y -r, Scale r, Crop L 2r-999, R 2r+999,
//!   T 999-2r, B -999-2r; so Resize Position Y is DOWN and Resize crops are r/999 of the frame.
//!
//! Whether the 999 normalisation holds for other frame sizes is not established (ledger D).

use crate::color::{ColorCorrection, ParamValue};

const SUFFIX: &str = "568d7e84d511935a50f857c10000";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Resize,
    Warp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PictureEffect {
    pub kind: Kind,
    pub params: ColorCorrection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transform {
    pub position: (f64, f64),
    pub scale: (f64, f64),
    pub rotation: f64,
    pub anchor: (f64, f64),
    /// left, top, right, bottom
    pub crop: [f64; 4],
    pub not_converted: Vec<String>,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { position: (0.0, 0.0), scale: (100.0, 100.0), rotation: 0.0, anchor: (0.0, 0.0), crop: [0.0; 4], not_converted: Vec::new() }
    }
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        self.position == (0.0, 0.0) && self.scale == (100.0, 100.0) && self.rotation == 0.0 && self.anchor == (0.0, 0.0) && self.crop == [0.0; 4]
    }
}

/// (prefix, name) of the controls interpreted below.
const RESIZE: &[(&str, &str)] = &[
    ("2581", "Position X"),
    ("2681", "Position Y"),
    ("2981", "Scaling X"),
    ("2a81", "Scaling Y"),
    ("2b81", "Fixed Aspect"),
    ("2e81", "Crop Lft"),
    ("2f81", "Crop Rgt"),
    ("3081", "Crop Top"),
    ("3181", "Crop Bot"),
    ("4d81", "unknown 4d81"),
    ("5381", "unknown 5381"),
];

const WARP: &[(&str, &str)] = &[
    ("a981", "Position"),
    ("aa81", "Position X"),
    ("ab81", "Position Y"),
    ("ac81", "Position Z"),
    ("b281", "Scaling"),
    ("b381", "Scale X"),
    ("b481", "Scale Y"),
    ("2b81", "Fixed Aspect"),
    ("b781", "Rotation"),
    ("b881", "Rotation X"),
    ("b981", "Rotation Y"),
    ("ba81", "Rotation Z"),
    ("bc81", "Axis"),
    ("bd81", "Axis X"),
    ("be81", "Axis Y"),
    ("bf81", "Axis Z"),
    ("cb81", "Crop"),
    ("cc81", "Crop L"),
    ("cd81", "Crop R"),
    ("ce81", "Crop T"),
    ("cf81", "Crop B"),
    ("0682", "Background"),
    ("4d81", "unknown 4d81"),
    ("5381", "unknown 5381"),
];

fn trim(f: f64) -> String {
    let s = format!("{:.4}", f);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl PictureEffect {
    fn number(&self, prefix: &str) -> Option<f64> {
        let id = format!("{}{}", prefix, SUFFIX);
        self.params.params.iter().find(|(k, _)| *k == id).and_then(|(_, v)| match v {
            ParamValue::Float(f) => Some(*f),
            ParamValue::Int(i) => Some(*i as f64),
            _ => None,
        })
    }

    fn on(&self, prefix: &str) -> bool {
        self.number(prefix) == Some(1.0)
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::Resize => "Resize",
            Kind::Warp => "3D Warp",
        }
    }

    pub fn interpret(&self) -> Transform {
        let mut out = Transform::default();
        let known = match self.kind {
            Kind::Resize => RESIZE,
            Kind::Warp => WARP,
        };
        for (id, v) in &self.params.params {
            let name = id.strip_suffix(SUFFIX).and_then(|p| known.iter().find(|(k, _)| *k == p));
            if name.is_none() {
                out.not_converted.push(match v {
                    ParamValue::Float(f) => format!("{} control {} = {}", self.name(), &id[..8], trim(*f)),
                    ParamValue::Int(i) => format!("{} control {} = {}", self.name(), &id[..8], i),
                    _ => format!("{} control {}", self.name(), &id[..8]),
                });
            }
        }
        let n = |p: &str, default: f64| self.number(p).unwrap_or(default);
        for unknown in ["4d81", "5381"] {
            if n(unknown, 0.0) != 0.0 {
                out.not_converted.push(format!("{} control {} = {}", self.name(), unknown, trim(n(unknown, 0.0))));
            }
        }
        match self.kind {
            Kind::Resize => {
                if n("2b81", 0.0) != 0.0 {
                    out.not_converted.push("Resize Fixed Aspect on (not observed in the KB)".into());
                }
                out.position = (n("2581", 0.0) / 999.0, n("2681", 0.0) / 999.0);
                out.scale = (n("2981", 100.0), n("2a81", 100.0));
                out.crop = [n("2e81", 0.0) / 999.0, n("3081", 0.0) / 999.0, 0.0 - n("2f81", 0.0) / 999.0, 0.0 - n("3181", 0.0) / 999.0];
            }
            Kind::Warp => {
                let mut t = (0.0, 0.0);
                if self.on("a981") {
                    t = (n("aa81", 0.0) / 999.0, 0.0 - n("ab81", 0.0) / 999.0);
                    if n("ac81", 0.0) != 0.0 {
                        out.not_converted.push(format!("3D Warp Position Z = {} (perspective)", trim(n("ac81", 0.0))));
                    }
                }
                if self.on("b281") {
                    let sx = n("b381", 100.0);
                    out.scale = (sx, n("b481", sx));
                }
                if self.on("b781") {
                    for (p, axis) in [("b881", "X"), ("b981", "Y")] {
                        if n(p, 0.0) != 0.0 {
                            out.not_converted.push(format!("3D Warp Rotation {} = {} (3D)", axis, trim(n(p, 0.0))));
                        }
                    }
                    out.rotation = n("ba81", 0.0);
                }
                let mut a = (0.0, 0.0);
                if self.on("bc81") {
                    a = (n("bd81", 0.0) / 999.0, 0.0 - n("be81", 0.0) / 999.0);
                    if n("bf81", 0.0) != 0.0 {
                        out.not_converted.push(format!("3D Warp Axis Z = {} (3D)", trim(n("bf81", 0.0))));
                    }
                }
                // Avid: q = R S (p - a) + S a + t, so the axis lands at S a + t.
                out.anchor = a;
                out.position = (out.scale.0 / 100.0 * a.0 + t.0, out.scale.1 / 100.0 * a.1 + t.1);
                if self.on("cb81") {
                    out.crop = [
                        (n("cc81", -999.0) + 999.0) / 1998.0,
                        (999.0 - n("ce81", 999.0)) / 1998.0,
                        (999.0 - n("cd81", 999.0)) / 1998.0,
                        (n("cf81", -999.0) + 999.0) / 1998.0,
                    ];
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(kind: Kind, values: &[(&str, f64)]) -> PictureEffect {
        PictureEffect {
            kind,
            params: ColorCorrection { params: values.iter().map(|(p, v)| (format!("{}{}", p, SUFFIX), ParamValue::Float(*v))).collect() },
        }
    }

    #[test]
    fn resize_and_its_promoted_warp_agree() {
        // RESIZE_COMPARE_READY pairs (KB resize-render-results: rendered TIFFs identical).
        let pairs: &[(&[(&str, f64)], &[(&str, f64)])] = &[
            (&[("2981", 60.0), ("2a81", 90.0)], &[("a981", 1.0), ("b281", 1.0), ("b381", 60.0), ("b481", 90.0), ("cb81", 1.0), ("cc81", -999.0), ("cd81", 999.0), ("ce81", 999.0), ("cf81", -999.0)]),
            (&[("2581", 120.0), ("2681", -80.0)], &[("a981", 1.0), ("aa81", 120.0), ("ab81", 80.0), ("b281", 1.0), ("cb81", 1.0)]),
            (
                &[("2e81", 100.0), ("2f81", -150.0), ("3081", 80.0), ("3181", -120.0)],
                &[("a981", 1.0), ("b281", 1.0), ("cb81", 1.0), ("cc81", -799.0), ("cd81", 699.0), ("ce81", 839.0), ("cf81", -759.0)],
            ),
        ];
        for (r, w) in pairs {
            let a = effect(Kind::Resize, r).interpret();
            let b = effect(Kind::Warp, w).interpret();
            assert!((a.position.0 - b.position.0).abs() < 1e-12 && (a.position.1 - b.position.1).abs() < 1e-12, "{:?} {:?}", a, b);
            assert_eq!(a.scale, b.scale);
            for i in 0..4 {
                assert!((a.crop[i] - b.crop[i]).abs() < 1e-12, "{:?} {:?}", a.crop, b.crop);
            }
            assert!(a.not_converted.is_empty() && b.not_converted.is_empty());
        }
    }

    #[test]
    fn warp_axis_and_3d_controls() {
        let t = effect(Kind::Warp, &[("a981", 1.0), ("aa81", 99.9), ("b281", 1.0), ("b381", 50.0), ("b781", 1.0), ("ba81", 30.0), ("b881", 10.0), ("bc81", 1.0), ("bd81", 99.9), ("be81", 99.9)])
            .interpret();
        assert_eq!(t.scale, (50.0, 50.0));
        assert_eq!(t.rotation, 30.0);
        assert!((t.anchor.0 - 0.1).abs() < 1e-12 && (t.anchor.1 + 0.1).abs() < 1e-12);
        assert!((t.position.0 - 0.15).abs() < 1e-12 && (t.position.1 + 0.05).abs() < 1e-12);
        assert_eq!(t.not_converted, vec!["3D Warp Rotation X = 10 (3D)".to_string()]);
    }
}
