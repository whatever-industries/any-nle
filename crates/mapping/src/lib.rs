//! Pair policies: the choices that depend on the source *and* target program, kept out of the
//! graph (docs/GRAPH-MODEL.md). Today one pair, Avid -> Premiere, holding what model/1 carries
//! as `options`: bin mode, Avid's default pan, marker conflicts and Avid info, and the colour
//! mapper's tables (ROADMAP "Import options UI").
//!
//! The policy decides; the Premiere writer applies it. Default tables are the writer's own
//! (`prproj::convert::colors`), so the colour preview shows exactly what will be written.

use graph::json::{self, Value};
use graph::{Graph, GraphError, Loss};
use prproj::convert::colors::{marker_color, nearest_label, rgb16_hex, LABEL_NAMES, MARKER_NAMES};
use prproj::convert::model::Options;

pub type R<T> = Result<T, GraphError>;

fn fail<T>(msg: impl Into<String>) -> R<T> {
    Err(GraphError(msg.into()))
}

pub const POLICY_SCHEMA: &str = "anynle.policy.avid-premiere/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinMode {
    /// Picture and each audio file as linked source clips.
    Separate,
    /// One grouped build per clip.
    Grouped,
    /// One merged clip per clip.
    Merged,
}

impl BinMode {
    pub fn as_str(self) -> &'static str {
        match self {
            BinMode::Separate => "separate",
            BinMode::Grouped => "grouped",
            BinMode::Merged => "merged",
        }
    }

    pub fn parse(s: &str) -> Option<BinMode> {
        match s {
            "separate" => Some(BinMode::Separate),
            "grouped" => Some(BinMode::Grouped),
            "merged" => Some(BinMode::Merged),
            _ => None,
        }
    }
}

/// Avid -> Premiere choices. `Default` is model/1's default options.
#[derive(Clone, Debug, PartialEq)]
pub struct AvidToPremiere {
    pub bin_mode: BinMode,
    /// Pan mono edits odd-left/even-right like Avid does when the edit has no pan of its own.
    pub match_avid_default_pan: bool,
    /// Where an edit marker and a source marker share a frame, the source marker wins.
    pub source_marker_wins: bool,
    /// The losing marker of such a conflict moves to the sequence instead of being dropped.
    pub conflict_loser_to_sequence: bool,
    /// Append "Avid: <user> · <color>" to Premiere marker comments.
    pub marker_avid_info: bool,
    /// Avid marker colour name -> Premiere marker colour index (0..7), over the defaults.
    pub marker_colors: Vec<(String, i64)>,
    /// Avid clip colour `#RRGGBB` -> Premiere label index (0..15), over nearest-colour.
    pub clip_labels: Vec<(String, i64)>,
}

impl Default for AvidToPremiere {
    fn default() -> Self {
        AvidToPremiere::from_options(&Options::default()).expect("model/1 defaults are valid")
    }
}

impl AvidToPremiere {
    pub fn from_options(o: &Options) -> R<AvidToPremiere> {
        let Some(bin_mode) = BinMode::parse(&o.bin_mode) else {
            return fail(format!("bin mode {:?}", o.bin_mode));
        };
        let source_marker_wins = match o.marker_conflict_winner.as_str() {
            "edit" => false,
            "source" => true,
            other => return fail(format!("marker conflict winner {other:?}")),
        };
        Ok(AvidToPremiere {
            bin_mode,
            match_avid_default_pan: o.match_avid_default_pan,
            source_marker_wins,
            conflict_loser_to_sequence: o.marker_conflict_loser_to_sequence,
            marker_avid_info: o.marker_avid_info,
            marker_colors: o.marker_colors.clone(),
            clip_labels: o.clip_labels.clone(),
        })
    }

    pub fn to_options(&self) -> Options {
        Options {
            bin_mode: self.bin_mode.as_str().to_string(),
            match_avid_default_pan: self.match_avid_default_pan,
            marker_conflict_winner: if self.source_marker_wins {
                "source"
            } else {
                "edit"
            }
            .to_string(),
            marker_conflict_loser_to_sequence: self.conflict_loser_to_sequence,
            marker_avid_info: self.marker_avid_info,
            marker_colors: self.marker_colors.clone(),
            clip_labels: self.clip_labels.clone(),
        }
    }

    /// Map an Avid marker colour to a Premiere marker colour (0 Green .. 7 Cyan).
    pub fn set_marker_color(&mut self, avid_name: &str, index: i64) -> R<()> {
        if !(0..=7).contains(&index) || avid_name.is_empty() {
            return fail(format!(
                "marker colour {avid_name:?} -> {index}: index 0..7"
            ));
        }
        self.marker_colors.retain(|(k, _)| k != avid_name);
        self.marker_colors.push((avid_name.to_string(), index));
        Ok(())
    }

    /// Map an Avid clip colour (`#RRGGBB`) to a Premiere label (0 Violet .. 15 Yellow).
    pub fn set_clip_label(&mut self, hex: &str, index: i64) -> R<()> {
        let ok = hex.len() == 7
            && hex.starts_with('#')
            && hex[1..]
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c));
        if !ok || !(0..=15).contains(&index) {
            return fail(format!(
                "clip label {hex:?} -> {index}: expected #RRGGBB (upper case) and index 0..15"
            ));
        }
        self.clip_labels.retain(|(k, _)| k != hex);
        self.clip_labels.push((hex.to_string(), index));
        Ok(())
    }

    /// What every colour in the graph becomes in Premiere, in order of first use: the colour
    /// mapper's table. Marker colours from another palette are not Avid names and are skipped.
    pub fn colour_preview(&self, g: &Graph) -> ColourPreview {
        self.preview_facts(
            g.items
                .iter()
                .flat_map(|i| &i.markers)
                .chain(g.sequences.iter().flat_map(|s| &s.markers))
                .filter_map(|m| m.color.as_ref()),
            g.items.iter().filter_map(|i| i.color_rgb16),
        )
    }

    /// Palette facts are independent of successful timeline preparation.
    pub fn colour_preview_source(&self, g: &graph::source::SourceGraph) -> ColourPreview {
        self.preview_facts(
            g.items
                .iter()
                .flat_map(|i| &i.markers)
                .chain(g.sequences.iter().flat_map(|s| &s.markers))
                .filter_map(|m| m.color.as_ref()),
            g.items.iter().filter_map(|i| i.color_rgb16),
        )
    }

    fn preview_facts<'a>(
        &self,
        labels: impl Iterator<Item = &'a graph::Label>,
        colors: impl Iterator<Item = [i64; 3]>,
    ) -> ColourPreview {
        let mut preview = ColourPreview::default();
        for label in labels.filter(|l| l.palette == "avid") {
            match preview.markers.iter_mut().find(|e| e.avid == label.name) {
                Some(e) => e.uses += 1,
                None => {
                    let index = marker_color(&label.name, &self.marker_colors);
                    preview.markers.push(MarkerColour {
                        avid: label.name.clone(),
                        premiere: index.map(|i| (i, name_of(&MARKER_NAMES, i))),
                        uses: 1,
                    })
                }
            }
        }
        for rgb in colors {
            let hex = rgb16_hex(rgb);
            match preview.labels.iter_mut().find(|e| e.avid == hex) {
                Some(e) => e.uses += 1,
                None => {
                    let index = nearest_label(rgb, &self.clip_labels);
                    preview.labels.push(LabelColour {
                        avid: hex,
                        premiere: (index, name_of(&LABEL_NAMES, index)),
                        uses: 1,
                    })
                }
            }
        }
        preview
    }

    /// What this policy cannot place, as losses: marker colours that are not Avid palette
    /// names keep Premiere's default colour (the writer notes the same per marker).
    pub fn losses(&self, g: &Graph) -> Vec<Loss> {
        self.losses_facts(
            g.items
                .iter()
                .map(|i| (&i.id, &i.markers))
                .chain(g.sequences.iter().map(|s| (&s.id, &s.markers)))
                .flat_map(|(id, markers)| {
                    markers.iter().filter_map(move |m| {
                        m.color.as_ref().map(|c| (id.as_str(), m.name.as_str(), c))
                    })
                }),
        )
    }

    pub fn losses_source(&self, g: &graph::source::SourceGraph) -> Vec<Loss> {
        self.losses_facts(
            g.items
                .iter()
                .map(|i| (&i.id, &i.markers))
                .chain(g.sequences.iter().map(|s| (&s.id, &s.markers)))
                .flat_map(|(id, markers)| {
                    markers.iter().filter_map(move |m| {
                        m.color.as_ref().map(|c| (id.as_str(), m.name.as_str(), c))
                    })
                }),
        )
    }

    fn losses_facts<'a>(
        &self,
        facts: impl Iterator<Item = (&'a str, &'a str, &'a graph::Label)>,
    ) -> Vec<Loss> {
        facts.filter_map(|(id,name,label)|{
            let mapped=label.palette=="avid"&&marker_color(&label.name,&self.marker_colors).is_some();
            (!mapped).then(||Loss{object:id.into(),aspect:"marker_color".into(),reason:format!("marker {name:?}: colour {:?} ({}) has no Premiere colour; Premiere's default kept",label.name,label.palette)})
        }).collect()
    }

    pub fn to_json_text(&self) -> String {
        // One entry per key, the later winning as the writer reads the tables (JSON keys are unique).
        let table = |t: &[(String, i64)]| {
            Value::Obj(
                t.iter()
                    .enumerate()
                    .filter(|(i, (k, _))| !t[i + 1..].iter().any(|(later, _)| later == k))
                    .map(|(_, (k, v))| (k.clone(), Value::Int(*v)))
                    .collect(),
            )
        };
        json::write(&Value::Obj(vec![
            ("schema".into(), Value::Str(POLICY_SCHEMA.into())),
            ("bin_mode".into(), Value::Str(self.bin_mode.as_str().into())),
            (
                "match_avid_default_pan".into(),
                Value::Bool(self.match_avid_default_pan),
            ),
            (
                "source_marker_wins".into(),
                Value::Bool(self.source_marker_wins),
            ),
            (
                "conflict_loser_to_sequence".into(),
                Value::Bool(self.conflict_loser_to_sequence),
            ),
            (
                "marker_avid_info".into(),
                Value::Bool(self.marker_avid_info),
            ),
            ("marker_colors".into(), table(&self.marker_colors)),
            ("clip_labels".into(), table(&self.clip_labels)),
        ]))
    }

    pub fn from_json_text(text: &str) -> R<AvidToPremiere> {
        let v = json::parse(text).map_err(GraphError)?;
        let Value::Obj(pairs) = &v else {
            return fail("policy: expected an object");
        };
        let known = [
            "schema",
            "bin_mode",
            "match_avid_default_pan",
            "source_marker_wins",
            "conflict_loser_to_sequence",
            "marker_avid_info",
            "marker_colors",
            "clip_labels",
        ];
        if let Some((k, _)) = pairs.iter().find(|(k, _)| !known.contains(&k.as_str())) {
            return fail(format!("policy: unknown key {k:?}"));
        }
        if v.get("schema") != Some(&Value::Str(POLICY_SCHEMA.into())) {
            return fail(format!("policy.schema: expected {POLICY_SCHEMA}"));
        }
        let mut p = AvidToPremiere::default();
        if let Some(m) = v.get("bin_mode") {
            p.bin_mode = match m {
                Value::Str(s) => BinMode::parse(s),
                _ => None,
            }
            .ok_or_else(|| GraphError("policy.bin_mode: separate, grouped or merged".into()))?;
        }
        for (key, slot) in [
            ("match_avid_default_pan", &mut p.match_avid_default_pan),
            ("source_marker_wins", &mut p.source_marker_wins),
            (
                "conflict_loser_to_sequence",
                &mut p.conflict_loser_to_sequence,
            ),
            ("marker_avid_info", &mut p.marker_avid_info),
        ] {
            match v.get(key) {
                None => {}
                Some(Value::Bool(b)) => *slot = *b,
                Some(_) => return fail(format!("policy.{key}: expected true or false")),
            }
        }
        let table = |key: &str| -> R<Vec<(String, i64)>> {
            match v.get(key) {
                None => Ok(Vec::new()),
                Some(Value::Obj(t)) => t
                    .iter()
                    .map(|(k, x)| match x {
                        Value::Int(n) => Ok((k.clone(), *n)),
                        _ => fail(format!("policy.{key}.{k}: expected an index")),
                    })
                    .collect(),
                Some(_) => fail(format!("policy.{key}: expected an object")),
            }
        };
        // Model/1's rules (any 1..64-character key, index in range), not the stricter setters:
        // whatever model/1 options hold must reload from the policy file.
        for (key, high, slot) in [
            ("marker_colors", 7, &mut p.marker_colors),
            ("clip_labels", 15, &mut p.clip_labels),
        ] {
            for (k, i) in table(key)? {
                if k.is_empty() || k.chars().count() > 64 || !(0..=high).contains(&i) {
                    return fail(format!(
                        "policy.{key}.{k}: expected a 1..64-character name and index 0..{high}"
                    ));
                }
                slot.push((k, i));
            }
        }
        Ok(p)
    }
}

/// A palette entry's name; an index outside the palette (a table entry set directly rather than
/// through the setters) shows as such instead of panicking.
fn name_of(names: &[&str], index: i64) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i))
        .map_or_else(
            || format!("index {index} (outside the palette)"),
            |n| n.to_string(),
        )
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColourPreview {
    pub markers: Vec<MarkerColour>,
    pub labels: Vec<LabelColour>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarkerColour {
    pub avid: String,
    /// `None` = not an Avid palette name; Premiere's default colour is kept.
    pub premiere: Option<(i64, String)>,
    pub uses: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LabelColour {
    /// `#RRGGBB`.
    pub avid: String,
    pub premiere: (i64, String),
    pub uses: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_model1_defaults() {
        assert_eq!(AvidToPremiere::default().to_options(), Options::default());
    }

    #[test]
    fn policy_json_round_trips_and_validates() {
        let mut p = AvidToPremiere {
            bin_mode: BinMode::Merged,
            source_marker_wins: true,
            ..Default::default()
        };
        p.set_marker_color("Purple", 2).unwrap();
        p.set_clip_label("#7F00FF", 0).unwrap();
        p.set_marker_color("Purple", 6).unwrap(); // re-mapping replaces
        let back = AvidToPremiere::from_json_text(&p.to_json_text()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.marker_colors, vec![("Purple".to_string(), 6)]);
        assert!(p.clone().set_marker_color("Red", 8).is_err());
        assert!(p.clone().set_clip_label("#7f00ff", 1).is_err());
        assert!(AvidToPremiere::from_json_text("{\"schema\": \"x\"}").is_err());
        // Keys model/1 accepts (here lower-case hex) survive a save and reload.
        let mut loose = AvidToPremiere::default();
        loose.clip_labels.push(("#7f00ff".into(), 3));
        loose
            .marker_colors
            .extend([("Red".into(), 1), ("Red".into(), 4)]);
        let reloaded = AvidToPremiere::from_json_text(&loose.to_json_text()).unwrap();
        assert_eq!(reloaded.clip_labels, loose.clip_labels);
        assert_eq!(reloaded.marker_colors, vec![("Red".to_string(), 4)]); // the later entry wins
        assert_eq!(name_of(&MARKER_NAMES, 9), "index 9 (outside the palette)");
    }
}
