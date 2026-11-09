//! Avid Color Correction (TrackEffect `EFF2_RGB_COLOR_CORRECTION`) parameters, interpreted from
//! the Avid KB (AVB-0012; parameter table in docs/avid-coverage.md, FXE-073 rows). Each
//! parameter is a PRIT whose GUID, written as its 16 file-order bytes in hex, is the KB's ID.
//!
//! Converted today: the Curves tab's Master/Red/Green/Blue curves (AVUP payload of 16 f64
//! (x, y) pairs stored as 256 + 2 x display value, i.e. 0..255 -> 256..766; point count from
//! its own parameter, absent = 2), Saturation, and stored Hue/Brightness/Contrast/Gain/Gamma/Setup
//! values. The writer applies a provisional measured model for those six tone controls; this
//! does not establish Avid's exact processing equations or complete color fidelity. Other
//! controls remain explicitly unconverted.

use crate::io::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Int(i64),
    Float(f64),
    Bytes(Vec<u8>),
    Other,
}

/// One Color Correction effect's parameters: (file-order hex ID, value), in file order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColorCorrection {
    pub params: Vec<(String, ParamValue)>,
}

pub fn file_order_hex(u: &Uuid) -> String {
    let mut out = format!("{:02x}{:02x}{:02x}{:02x}", u.data1 & 0xff, (u.data1 >> 8) & 0xff, (u.data1 >> 16) & 0xff, u.data1 >> 24);
    out += &format!("{:02x}{:02x}{:02x}{:02x}", u.data2 & 0xff, u.data2 >> 8, u.data3 & 0xff, u.data3 >> 8);
    for b in u.data4 {
        out += &format!("{:02x}", b);
    }
    out
}

/// KB names of known parameters (by 8-hex prefix for the HSL controls the KB cites short).
const NAMES: &[(&str, &str)] = &[
    ("6a10e399", "Hue"),
    ("d0728153", "Saturation"),
    ("022d5a68", "Brightness"),
    ("d55210f9", "Contrast"),
    ("6c30edfa", "Gain"),
    ("cfcdd0fe", "Gamma"),
    ("0da179f1", "Setup"),
    ("b6b58d55", "Clip Low"),
    ("e552a557", "Clip High"),
    ("a2dcb949", "Clip enable"),
    ("1b45b5fb", "Invert Luma"),
    ("b5a94d51", "Invert Chroma"),
];

/// Curves: (name, points ID, enable ID, count ID).
const CURVES: &[(&str, &str, &str, Option<&str>)] = &[
    ("master", "35b75f22cb4ac745b9b6deacdaf96354", "23437fd83cba2a4e92adf0afe57ba965", Some("aa7109d7e874cc4e9214f0040fc48987")),
    ("red", "201e0d1d18aa6c44af28e99784c1b19e", "798d7477872c7a409818f233d031ee07", Some("110507eb3fb5f1408a1feb2e07b74635")),
    ("green", "23f11d2c1135f741b4713ee88dc485ec", "4d74649c518cbf41bf1ccea6a4ffbcfb", Some("c6a40fd63f214d49af870a134d4b1271")),
    ("blue", "826abb97f91a3944bf066e64dc62167f", "ba0c50f829fe1d4a8a63066c14162f75", Some("aeab4abe0d84c44d921976d8127ab4f4")),
];

/// Present in every Color Correction effect (KB: shared items, meanings not established).
const STRUCTURAL: &[&str] = &["ea10d291", "dca5f78a", "3dc627eb", "f7745810", "41f5116e", "065d3b3b", "335b4302", "f1401374"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Interpreted {
    pub saturation: Option<f64>,
    /// HSL Controls values as stored (Hue in degrees 0..360, Brightness, Contrast, Gain, Gamma, Setup); the writer
    /// models their effect from the provisional Phase 52 measurements (Codex intake, 2026-10-15).
    pub tone: Vec<(&'static str, f64)>,
    /// (curve name, points normalized so Avid 0..255 display -> 0..1; may leave 0..1).
    pub curves: Vec<(&'static str, Vec<(f64, f64)>)>,
    /// Human-readable notes: controls present but not converted, assumptions made.
    pub not_converted: Vec<String>,
    pub notes: Vec<String>,
}

impl ColorCorrection {
    fn get(&self, id: &str) -> Option<&ParamValue> {
        self.params.iter().find(|(k, _)| k == id || (id.len() == 8 && k.starts_with(id))).map(|(_, v)| v)
    }

    pub fn interpret(&self) -> Interpreted {
        let mut out = Interpreted::default();
        let mut used: Vec<&str> = Vec::new();
        for (name, points_id, enable_id, count_id) in CURVES {
            used.extend([*points_id, *enable_id]);
            if let Some(c) = count_id {
                used.push(c);
            }
            let enabled = matches!(self.get(enable_id), Some(ParamValue::Int(1)));
            let Some(ParamValue::Bytes(data)) = self.get(points_id) else {
                continue;
            };
            if !enabled {
                out.notes.push(format!("{} curve is switched off in Avid (points kept, not applied)", name));
                continue;
            }
            let count = match count_id.and_then(|c| self.get(c)) {
                Some(ParamValue::Int(n)) => *n,
                _ => {
                    if count_id.is_none() {
                        out.notes.push(format!("{} curve: point count assumed 2 or from stored points (its count parameter is not in the KB)", name));
                    }
                    2
                }
            };
            let pairs: Vec<(f64, f64)> = data
                .chunks_exact(16)
                .take(16)
                .map(|c| (f64::from_le_bytes(c[0..8].try_into().unwrap()), f64::from_le_bytes(c[8..16].try_into().unwrap())))
                .collect();
            let count = if count_id.is_none() {
                // Without a count parameter, keep the leading non-padding pairs.
                pairs.iter().take_while(|(x, y)| !(*x == 0.0 && *y == 0.0)).count().max(2) as i64
            } else {
                count
            };
            if count < 2 || count as usize > pairs.len() {
                out.notes.push(format!("{} curve: point count {} out of range; not converted", name, count));
                continue;
            }
            let points = pairs[..count as usize].iter().map(|(x, y)| ((x - 256.0) / 510.0, (y - 256.0) / 510.0)).collect();
            out.curves.push((name, points));
        }
        for (id, v) in &self.params {
            if used.contains(&id.as_str()) || STRUCTURAL.iter().any(|s| id.starts_with(s)) {
                continue;
            }
            let name = NAMES.iter().find(|(p, _)| id.starts_with(p)).map(|(_, n)| *n);
            match (name, v) {
                (Some("Saturation"), ParamValue::Float(f)) => out.saturation = Some(*f),
                (Some(n @ ("Hue" | "Brightness" | "Contrast" | "Gain" | "Gamma" | "Setup")), ParamValue::Float(f)) => {
                    // Explicit defaults are stored too (Phase 52: brightness 0, contrast 0).
                    let default = if n == "Gain" { 100.0 } else if n == "Gamma" { 1.0 } else { 0.0 };
                    if (f - default).abs() > 1e-9 {
                        out.tone.push((n, *f));
                    }
                }
                (Some(n), ParamValue::Float(f)) => out.not_converted.push(format!("{} {}", n, trim(*f))),
                (Some(n), ParamValue::Int(i)) => out.not_converted.push(format!("{} {}", n, i)),
                (Some(n), _) => out.not_converted.push(n.to_string()),
                (None, ParamValue::Float(f)) => out.not_converted.push(format!("unknown control {} = {}", &id[..8], trim(*f))),
                (None, ParamValue::Int(i)) => out.not_converted.push(format!("unknown control {} = {}", &id[..8], i)),
                (None, _) => out.not_converted.push(format!("unknown control {}", &id[..8])),
            }
        }
        out
    }
}

fn trim(f: f64) -> String {
    let s = format!("{:.4}", f);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[(f64, f64)]) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..16 {
            let (x, y) = points.get(i).copied().unwrap_or((0.0, 0.0));
            out.extend(x.to_le_bytes());
            out.extend(y.to_le_bytes());
        }
        out
    }

    #[test]
    fn phase45_curves_and_saturation() {
        // Phase 45 "color 2.avb": green 3 points (KB 43,180 middle point), red 34,301 with no
        // count parameter, master disabled.
        let cc = ColorCorrection {
            params: vec![
                ("4d74649c518cbf41bf1ccea6a4ffbcfb".into(), ParamValue::Int(1)),
                ("c6a40fd63f214d49af870a134d4b1271".into(), ParamValue::Int(3)),
                ("23f11d2c1135f741b4713ee88dc485ec".into(), ParamValue::Bytes(curve(&[(256.0, 256.0), (342.0, 616.0), (766.0, 766.0)]))),
                ("798d7477872c7a409818f233d031ee07".into(), ParamValue::Int(1)),
                ("201e0d1d18aa6c44af28e99784c1b19e".into(), ParamValue::Bytes(curve(&[(324.0, 858.0), (766.0, 766.0)]))),
                ("23437fd83cba2a4e92adf0afe57ba965".into(), ParamValue::Int(0)),
                ("35b75f22cb4ac745b9b6deacdaf96354".into(), ParamValue::Bytes(curve(&[(256.0, 256.0), (766.0, 766.0)]))),
                ("538172d0".to_string() + "dd774061b25da0be5f1930af", ParamValue::Float(1.0)), // not the KB ID: unknown
                ("d072815377dd6140b25da0be5f1930af".into(), ParamValue::Float(205.56)),
                ("6c30edfaef0cd346a32132f0f19e0069".into(), ParamValue::Float(177.78)),
                ("065d3b3b00000000000000000000000".into(), ParamValue::Int(5)),
            ],
        };
        let i = cc.interpret();
        assert_eq!(i.saturation, Some(205.56));
        let green = &i.curves.iter().find(|(n, _)| *n == "green").unwrap().1;
        assert_eq!(green, &vec![(0.0, 0.0), (86.0 / 510.0, 360.0 / 510.0), (1.0, 1.0)]);
        let red = &i.curves.iter().find(|(n, _)| *n == "red").unwrap().1;
        assert_eq!(red, &vec![(68.0 / 510.0, 602.0 / 510.0), (1.0, 1.0)]);
        assert!(i.curves.iter().all(|(n, _)| *n != "master"));
        assert!(i.notes.iter().any(|n| n.starts_with("master curve is switched off")));
        assert!(i.tone.contains(&("Gain", 177.78)));
        assert!(!i.not_converted.iter().any(|n| n.starts_with("Gain")));
        assert!(i.not_converted.iter().any(|n| n.starts_with("unknown control 538172d0")));
    }

    #[test]
    fn file_order_hex_matches_kb_ids() {
        // pyavb UUID 538172d0-dd77-4061-b25d-a0be5f1930af = KB d072815377dd6140b25da0be5f1930af.
        let u = Uuid { data1: 0x538172d0, data2: 0xdd77, data3: 0x4061, data4: [0xb2, 0x5d, 0xa0, 0xbe, 0x5f, 0x19, 0x30, 0xaf] };
        assert_eq!(file_order_hex(&u), "d072815377dd6140b25da0be5f1930af");
    }
}
