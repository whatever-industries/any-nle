//! The public entry points (port of `py_premiere/__init__.py` `parse`/`new` and
//! `models/application.py`), plus `Project`'s remaining read-only properties.

use std::path::{Path, PathBuf};

use super::preferences::Preferences;
use super::{Project, SequenceId};
use crate::convert::model::py_str_repr;
use crate::error::Result;
use crate::ids::UuidSource;
use crate::xml::parse_prproj;

/// The Premiere Pro application object (`app` in ExtendScript): holds the project.
#[derive(Clone, Debug)]
pub struct Application {
    project: Project,
}

impl Application {
    /// `Application(project)`.
    pub fn new(project: Project) -> Application {
        Application { project }
    }

    /// `Application.project`.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// `Application.project`, for edits.
    pub fn project_mut(&mut self) -> &mut Project {
        &mut self.project
    }

    /// The project, owned.
    pub fn into_project(self) -> Project {
        self.project
    }

    /// `repr(app)`.
    pub fn repr(&self) -> String {
        format!("Application({})", self.project.repr())
    }
}

/// `py_premiere.parse(path, preferences_path=None)`: open a `.prproj`.
pub fn parse(path: impl AsRef<Path>, preferences_path: Option<&Path>) -> Result<Application> {
    let path = path.as_ref();
    let mut project = Project::from_document(parse_prproj(path)?, path)?;
    project.preferences_path = preferences_path.map(Path::to_path_buf);
    Ok(Application::new(project))
}

/// `py_premiere.new(preferences_path=None)`: a new, empty project (`untitled.prproj`).
pub fn new(preferences_path: Option<&Path>, uuids: &mut dyn UuidSource) -> Result<Application> {
    let mut project = Project::new(uuids)?;
    project.preferences_path = preferences_path.map(Path::to_path_buf);
    Ok(Application::new(project))
}

impl Project {
    /// `Project.active_sequence`: the frontmost sequence, best effort.
    pub fn active_sequence(&self) -> Option<SequenceId> {
        self.active_sequence
    }

    /// `Project.sequence_presets()`: the preset names `add_sequence` accepts, sorted.
    pub fn sequence_presets() -> Vec<String> {
        let mut names: Vec<String> = crate::build::sequence::formats().into_iter().map(|(n, _)| n.to_string()).collect();
        names.sort();
        names
    }

    /// The preferences an import would read: `preferences_path` when set, else the machine's
    /// most recent profile (`None` when Premiere never ran here).
    pub fn preferences(&self) -> Result<Option<Preferences>> {
        match &self.preferences_path {
            Some(p) => Preferences::new(p).map(Some),
            None => Preferences::load_default(),
        }
    }

    /// The preferences file override (`_preferences_path`).
    pub fn preferences_path(&self) -> Option<&PathBuf> {
        self.preferences_path.as_ref()
    }

    /// `repr(project)`.
    pub fn repr(&self) -> String {
        format!("Project(name={}, {} sequence(s))", py_str_repr(&self.name()), self.sequences().len())
    }
}
