//! Local choices only. Presets never include source names, media paths, or output destinations.
use crate::model::{Choices, Preset};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("Any NLE").join("wizard-settings.json"))
}
pub fn load() -> Result<Option<Choices>, String> {
    let Some(path) = settings_path() else {
        return Ok(None);
    };
    match read_settings(&path) {
        Ok(text) => Preset::from_json(&text).map(|p| Some(p.choices)),
        Err(_) if !path.exists() => Ok(None),
        Err(e) => Err(format!("Couldn't read remembered settings: {e}")),
    }
}
pub fn save(choices: &Choices) -> Result<(), String> {
    let path = settings_path().ok_or("No local configuration folder is available.")?;
    let parent = path.parent().ok_or("No settings parent folder.")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let text = Preset::new("Last used".into(), choices.clone()).to_json()?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary
        .write_all(text.as_bytes())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| format!("Couldn't save settings: {e}"))?;
    temporary
        .persist(&path)
        .map_err(|e| format!("Couldn't publish settings: {e}"))?;
    Ok(())
}
pub fn write_new(path: &Path, text: &str) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_settings(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.len() > 1_048_576 {
        return Err("Choose a settings file no larger than 1 MiB.".into());
    }
    use std::io::Read;
    let mut text=String::new();std::fs::File::open(path).map_err(|e|e.to_string())?.take(1_048_577).read_to_string(&mut text).map_err(|e|e.to_string())?;
    if text.len()>1_048_576{return Err("Settings grew beyond 1 MiB while reading".into())}Ok(text)
}
