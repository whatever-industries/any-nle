//! Pure wizard state: paths stay local; a review is valid only for its exact settings revision.
use mapping::AvidToPremiere;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PRESET_SCHEMA: &str = "anynle.gui-preset/1";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipMode {
    #[default]
    Rewrap,
    Grouped,
    Merged,
    Separate,
}
impl ClipMode {
    pub const ALL: [Self; 4] = [Self::Rewrap, Self::Grouped, Self::Merged, Self::Separate];
    pub fn label(self) -> &'static str {
        match self {
            Self::Rewrap => "Rewrap to native clips",
            Self::Grouped => "Grouped",
            Self::Merged => "Merged",
            Self::Separate => "Separate files",
        }
    }
    pub fn explanation(self) -> (&'static str, &'static str) {
        match self {
            Self::Rewrap => (
                "One native clip with the declared audio groups. Stream copy keeps the essence.",
                "Creates new media. Qualified native sources only; needs additional space.",
            ),
            Self::Grouped => (
                "Keeps source parts together and retains their metadata.",
                "Premiere uses a grouped construction rather than one file-backed clip.",
            ),
            Self::Merged => (
                "Presents picture and sound as one merged build.",
                "Merged clips lose some source metadata. Stereo uses a grouped build.",
            ),
            Self::Separate => (
                "Keeps picture and audio parts separately accessible, with links.",
                "More project items to manage. Timeline construction follows this choice.",
            ),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choices {
    pub mode: ClipMode,
    pub include_dependencies: bool,
    pub include_trash_attic: bool,
    pub policy_json: String,
    #[serde(default)]
    pub effects: Vec<(String, String)>,
}
impl Default for Choices {
    fn default() -> Self {
        Self {
            mode: ClipMode::Rewrap,
            include_dependencies: true,
            include_trash_attic: false,
            policy_json: AvidToPremiere::default().to_json_text(),
            effects: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub schema: String,
    pub name: String,
    pub choices: Choices,
}
impl Preset {
    pub fn new(name: String, choices: Choices) -> Self {
        Self {
            schema: PRESET_SCHEMA.into(),
            name,
            choices,
        }
    }
    pub fn from_json(text: &str) -> Result<Self, String> {
        if text.len() > 1_048_576 {
            return Err("Preset is larger than 1 MiB.".into());
        }
        let p: Self = serde_json::from_str(text).map_err(|e| format!("Invalid preset: {e}"))?;
        if p.schema != PRESET_SCHEMA {
            return Err("This preset version is not supported.".into());
        }
        AvidToPremiere::from_json_text(&p.choices.policy_json).map_err(|e| e.to_string())?;
        if p.choices.effects.len() > 1024
            || p.choices.effects.iter().any(|(key, mode)| {
                key.is_empty()
                    || key.len() > 256
                    || !matches!(mode.as_str(), "convert" | "approximate" | "skip")
            })
        {
            return Err("Preset contains an invalid effect choice.".into());
        }
        let mut keys = std::collections::HashSet::new();
        if p.choices.effects.iter().any(|(k, _)| !keys.insert(k)) {
            return Err("Preset repeats an effect choice.".into());
        }
        Ok(p)
    }
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Direction,
    Clips,
    Markers,
    Effects,
    Review,
}
impl Page {
    pub const ALL: [Self; 5] = [
        Self::Direction,
        Self::Clips,
        Self::Markers,
        Self::Effects,
        Self::Review,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Direction => "Project",
            Self::Clips => "Clips & media",
            Self::Markers => "Markers & colors",
            Self::Effects => "Effects",
            Self::Review => "Review & convert",
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1).min(4)]
    }
    pub fn back(self) -> Self {
        Self::ALL[self.index().saturating_sub(1)]
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub project_folder: Option<PathBuf>,
    pub bins: Vec<PathBuf>,
    pub entire_project: bool,
    pub output: PathBuf,
    pub project_name: Option<String>,
    pub media_roots: Vec<PathBuf>,
    pub original_roots: Vec<PathBuf>,
    pub path_mappings: Vec<(PathBuf, PathBuf)>,
    pub new_media: Option<PathBuf>,
}
#[derive(Clone, Copy)]
pub enum RequiredInput {
    Bins,
    BinSelection,
    Output,
    OutputExtension,
}
impl RequiredInput {
    pub fn message(self) -> &'static str {
        match self {
            Self::Bins => "Choose Avid bins or a project folder.",
            Self::BinSelection => "Select at least one bin.",
            Self::Output => "Choose a new Premiere project destination.",
            Self::OutputExtension => "The destination must end with .prproj.",
        }
    }
}
impl Inputs {
    pub fn add_bins(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            if is_bin(&path) && !self.bins.contains(&path) {
                self.bins.push(path);
            }
        }
        self.bins.sort();
    }
    pub fn required_paths(&self) -> Result<(), String> {
        self.check_required_paths().map_err(|input| input.message().into())
    }
    pub fn check_required_paths(&self) -> Result<(), RequiredInput> {
        if self.project_folder.is_none() && self.bins.is_empty() {
            return Err(RequiredInput::Bins);
        }
        if !self.entire_project && self.bins.is_empty() {
            return Err(RequiredInput::BinSelection);
        }
        if self.output.as_os_str().is_empty() {
            return Err(RequiredInput::Output);
        }
        if self
            .output
            .extension()
            .is_none_or(|e| !e.eq_ignore_ascii_case("prproj"))
        {
            return Err(RequiredInput::OutputExtension);
        }
        Ok(())
    }
}
pub fn is_bin(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("avb"))
}
pub fn excluded_folder(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("trash") || s.eq_ignore_ascii_case("attic"))
    })
}
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub inputs: Inputs,
    pub choices: Choices,
}
pub struct Wizard {
    pub page: Page,
    pub inputs: Inputs,
    pub choices: Choices,
    revision: u64,
    reviewed_revision: Option<u64>,
}
impl Default for Wizard {
    fn default() -> Self {
        Self {
            page: Page::Direction,
            inputs: Inputs::default(),
            choices: Choices::default(),
            revision: 0,
            reviewed_revision: None,
        }
    }
}
impl Wizard {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            inputs: self.inputs.clone(),
            choices: self.choices.clone(),
        }
    }
    pub fn changed_since(&mut self, before: &Snapshot) -> bool {
        if *before != self.snapshot() {
            self.invalidate();
            true
        } else {
            false
        }
    }
    pub fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.reviewed_revision = None;
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn accept_review(&mut self, revision: u64) -> bool {
        if revision == self.revision {
            self.reviewed_revision = Some(revision);
            true
        } else {
            false
        }
    }
    pub fn can_convert(&self) -> bool {
        self.reviewed_revision == Some(self.revision)
    }
    pub fn apply_preset(&mut self, preset: Preset) {
        self.choices = preset.choices;
        self.invalidate();
    }
    pub fn recommended(&mut self) {
        self.choices = Choices::default();
        self.invalidate();
        self.page = Page::Review;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_default_and_explicit_preset_are_distinct() {
        let mut w = Wizard::default();
        assert_eq!(w.choices.mode, ClipMode::Rewrap);
        let p = Preset::new(
            "Separate".into(),
            Choices {
                mode: ClipMode::Separate,
                ..Default::default()
            },
        );
        let p = Preset::from_json(&p.to_json().unwrap()).unwrap();
        w.apply_preset(p);
        assert_eq!(w.choices.mode, ClipMode::Separate);
    }
    #[test]
    fn review_is_invalidated_only_by_settings_changes_and_stale_result_is_rejected() {
        let mut w = Wizard::default();
        let s = w.snapshot();
        assert!(w.accept_review(0));
        assert!(!w.changed_since(&s));
        assert!(w.can_convert());
        w.inputs.output = "new.prproj".into();
        assert!(w.changed_since(&s));
        assert!(!w.can_convert());
        assert!(!w.accept_review(0));
        assert!(w.accept_review(1));
    }
    #[test]
    fn bin_selection_is_deduplicated_case_insensitive_and_never_includes_locks() {
        let mut i = Inputs::default();
        i.add_bins(["b.AVB", "a.avb", "b.AVB", "c.avb.lck"].map(PathBuf::from));
        assert_eq!(i.bins, vec![PathBuf::from("a.avb"), PathBuf::from("b.AVB")]);
        assert!(excluded_folder(Path::new("project/Attic/a.avb")));
        assert!(!excluded_folder(Path::new("project/Attic notes/a.avb")));
    }
    #[test]
    fn paths_required_before_shortcut_and_navigation_is_bounded() {
        let mut w = Wizard::default();
        assert!(w.inputs.required_paths().is_err());
        w.inputs.add_bins([PathBuf::from("synthetic.avb")]);
        w.inputs.output = "synthetic.prproj".into();
        w.inputs.required_paths().unwrap();
        w.recommended();
        assert_eq!(w.page, Page::Review);
        assert_eq!(Page::Direction.back(), Page::Direction);
        assert_eq!(Page::Review.next(), Page::Review);
    }
    #[test]
    fn preset_carries_explicit_effect_choices_and_rejects_unknown_modes() {
        let mut c = Choices::default();
        c.effects.push(("synthetic".into(), "skip".into()));
        let p = Preset::new("Effects".into(), c);
        assert_eq!(Preset::from_json(&p.to_json().unwrap()).unwrap(), p);
        assert!(Preset::from_json(&p.to_json().unwrap().replace("skip", "invented")).is_err());
    }
    #[test]
    fn malformed_and_future_presets_refuse_without_changing_settings() {
        let p = Preset::new("Example".into(), Choices::default());
        assert!(
            Preset::from_json(&p.to_json().unwrap().replace(PRESET_SCHEMA, "future/8")).is_err()
        );
        assert!(Preset::from_json("{}").is_err());
        assert!(Preset::from_json(&(" ".repeat(1_048_577))).is_err());
    }
}
