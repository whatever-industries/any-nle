//! Premiere's on-disk preferences (port of `py_premiere/models/preferences.py`). Read-only.
//!
//! `~/Documents/Adobe/Premiere Pro/<version>/Profile-<name>/Adobe Premiere Pro Prefs` is an
//! uncompressed PremiereData XML file whose `Preferences` object holds a flat `Properties` bag;
//! the `BE.Prefs.*` keys back the label palette and the still-import defaults.

use std::path::{Path, PathBuf};

use crate::build::time::TICKS_PER_SECOND;
use crate::convert::model::py_str_repr;
use crate::error::{Error, Result};
use crate::xml::{NodeId, Tree};

/// `LABEL_COUNT`: size of Premiere's label palette.
pub const LABEL_COUNT: usize = 16;

/// `LABEL_KINDS`: the media-kind keys of `BE.Prefs.LabelDefaults.*`.
pub const LABEL_KINDS: [&str; 8] = ["Bin", "Sequence", "Video", "Audio", "AV", "Still", "DynamicLink", "Captions"];

/// A parsed `Adobe Premiere Pro Prefs` file.
#[derive(Clone, Debug)]
pub struct Preferences {
    path: PathBuf,
    tree: Tree,
    bag: NodeId,
}

/// `_default_prefs_path`: the most recently modified profile's prefs file, if any.
pub fn default_prefs_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let base = Path::new(&home).join("Documents").join("Adobe").join("Premiere Pro");
    if !base.is_dir() {
        return None;
    }
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for version_dir in std::fs::read_dir(&base).ok()?.flatten() {
        let Ok(profiles) = std::fs::read_dir(version_dir.path()) else { continue };
        for profile in profiles.flatten() {
            if !profile.file_name().to_string_lossy().starts_with("Profile-") {
                continue;
            }
            let prefs = profile.path().join("Adobe Premiere Pro Prefs");
            if prefs.is_file() {
                let mtime = std::fs::metadata(&prefs).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                candidates.push((mtime, prefs));
            }
        }
    }
    // Python's `max(..., key=mtime)` keeps the FIRST of equal maxima.
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for candidate in candidates {
        if best.as_ref().is_none_or(|b| candidate.0 > b.0) {
            best = Some(candidate);
        }
    }
    best.map(|b| b.1)
}

impl Preferences {
    /// `Preferences(path)`: parse the file; refuses one that is not well-formed XML or has no
    /// `Preferences` object (with a non-empty `ObjectID`) holding a `Properties` bag.
    pub fn new(path: impl AsRef<Path>) -> Result<Preferences> {
        let path = path.as_ref().to_path_buf();
        let data = std::fs::read(&path)?;
        let mut tree = Tree::new();
        let root = match crate::xml::parser::parse_bytes_into(&mut tree, &data) {
            Ok(root) => root,
            Err(Error::Parse(m)) => return Err(Error::Value(format!("not a Premiere preferences file: {m}"))),
            Err(e) => return Err(e),
        };
        let mut bag = None;
        for &element in tree.children(root) {
            if tree.tag(element) == "Preferences" && tree.get(element, "ObjectID").is_some_and(|v| !v.is_empty()) {
                bag = tree.child_by_tag(element, "Properties");
                break;
            }
        }
        let Some(bag) = bag else { return Err(Error::Value("not a Premiere preferences file".into())) };
        Ok(Preferences { path: crate::model::project::py_path(&path.to_string_lossy()), tree, bag })
    }

    /// `Preferences.load_default()`: the machine's own prefs, or `None` when Premiere never ran.
    pub fn load_default() -> Result<Option<Preferences>> {
        default_prefs_path().map(Preferences::new).transpose()
    }

    /// `path`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Point the record at another path (only what `path` and the repr report).
    pub fn set_path(&mut self, path: impl AsRef<Path>) {
        self.path = path.as_ref().to_path_buf();
    }

    /// `get(key)`: the raw text of a key, or `None`.
    pub fn get(&self, key: &str) -> Option<String> {
        self.tree.child_by_tag(self.bag, key).map(|e| self.tree[e].text_or_empty().to_string())
    }

    /// `get_int(key)`: the integer value when the text is an optionally negative run of digits
    /// (a run of signs like `--5` passes Python's digit test and then raises in `int()`; so
    /// does it here).
    pub fn get_int(&self, key: &str) -> Result<Option<i64>> {
        let Some(text) = self.get(key) else { return Ok(None) };
        let digits = text.trim_start_matches('-');
        if text.is_empty() || digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Ok(None);
        }
        crate::model::project::py_int(&text).map(Some)
    }

    /// `get_bool(key)`: `None` when absent, else whether the text is `"true"`.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).map(|t| t == "true")
    }

    /// `label_names`: the 16 `BE.Prefs.LabelNames.N`.
    pub fn label_names(&self) -> Vec<String> {
        (0..LABEL_COUNT).map(|i| self.get(&format!("BE.Prefs.LabelNames.{i}")).unwrap_or_default()).collect()
    }

    /// `label_colors`: the 16 packed `BE.Prefs.LabelColors.N` (0 where absent).
    pub fn label_colors(&self) -> Result<Vec<i64>> {
        (0..LABEL_COUNT).map(|i| Ok(self.get_int(&format!("BE.Prefs.LabelColors.{i}"))?.unwrap_or(0))).collect()
    }

    /// `label_default(kind)`: the default label index of a media kind; `None` when unset or
    /// outside the palette.
    pub fn label_default(&self, kind: &str) -> Result<Option<usize>> {
        if !LABEL_KINDS.contains(&kind) {
            return Err(Error::Value(format!("unknown label kind {}", py_str_repr(kind))));
        }
        Ok(self
            .get_int(&format!("BE.Prefs.LabelDefaults.{kind}"))?
            .filter(|&i| (0..LABEL_COUNT as i64).contains(&i))
            .map(|i| i as usize))
    }

    /// `still_frame_rate`: ticks per frame for imported stills.
    pub fn still_frame_rate(&self) -> Result<Option<i64>> {
        self.get_int("BE.Prefs.StillImages.DefaultFramerate")
    }

    /// `still_default_out_ticks`: whole seconds floored to whole frames, in ticks.
    pub fn still_default_out_ticks(&self) -> Result<Option<i64>> {
        let seconds = self.get_int("BE.Prefs.StillImages.DurationInSeconds")?;
        let frame = self.still_frame_rate()?;
        let (Some(seconds), Some(frame)) = (seconds, frame.filter(|&f| f != 0)) else { return Ok(None) };
        let total = (seconds as i128) * (TICKS_PER_SECOND as i128);
        let frames = total.div_euclid(frame as i128);
        i64::try_from(frames * frame as i128).map(Some).map_err(|_| Error::Value("integer out of range".into()))
    }

    /// `repr(preferences)`.
    pub fn repr(&self) -> String {
        format!("Preferences(path={})", py_str_repr(&self.path.to_string_lossy()))
    }
}
