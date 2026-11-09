//! Receipt encoding, exclusive writes and project completion/failure records.
use super::{Plan, R};
use graph::json::{self, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub(super) fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::Obj(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub(super) fn text(s: impl Into<String>) -> Value {
    Value::Str(s.into())
}
pub(super) fn strings(xs: &[String]) -> Value {
    Value::Arr(xs.iter().cloned().map(text).collect())
}
impl Plan {
    pub(super) fn write_receipt(&self, name: &str, value: Value) -> R<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.destination.join(name))
            .map_err(|e| e.to_string())?;
        f.write_all((json::write(&value) + "\n").as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string())
    }
    pub fn record_failure(&self, error: &str) {
        if let Err(receipt_error) = self.write_receipt(
            "conversion-failed.json",
            obj(vec![
                (
                    "status",
                    text("project conversion failed; verified/partial media retained"),
                ),
                ("error", text(error)),
            ]),
        ) {
            eprintln!("conversion failure receipt could not be written: {receipt_error}; retained outputs: {}", self.destination.display());
        }
    }
    #[cfg(test)]
    pub fn record_project(&self, path: &Path) -> R<()> {
        self.record_project_with_mode(path, "existing bin mode")
    }
    pub fn record_project_with_mode(&self, path: &Path, mode: &str) -> R<()> {
        let result = self.write_receipt(
            "conversion.json",
            obj(vec![
                (
                    "status",
                    text("project written; Premiere native acceptance pending"),
                ),
                ("project", text(path.to_string_lossy())),
                ("notice", text("Creates new media.")),
                ("clip_construction", text(mode)),
            ]),
        );
        if let Err(error) = result {
            let message = format!("project published at {}; prepared media retained at {}; completion receipt could not be written: {error}", path.display(), self.destination.display());
            if let Err(receipt_error) = self.write_receipt("conversion-failed.json", obj(vec![
                ("status", text("project published; completion receipt failed; media and project retained")),
                ("stage", text("completion_receipt")),
                ("project_published", Value::Bool(true)),
                ("project", text(path.to_string_lossy())),
                ("media_directory", text(self.destination.to_string_lossy())),
                ("error", text(&error)),
            ])) {
                eprintln!("{message}; failure receipt also could not be written: {receipt_error}");
            }
            return Err(message);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_receipt_failure_reports_existing_project_and_preserves_outputs() {
        let root=std::env::temp_dir().join(format!("anynle-completion-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        let project=root.join("published.prproj");std::fs::write(&project,b"published project").unwrap();
        let occupied=root.join("conversion.json");std::fs::write(&occupied,b"incumbent receipt").unwrap();
        let plan=Plan { destination:root.clone(),inputs:vec![],jobs:vec![],
            ffmpeg:"ffmpeg".into(),ffprobe:"ffprobe".into(),versions:vec![],estimate:0,free:0 };
        let error=plan.record_project_with_mode(&project,"test").unwrap_err();
        assert!(error.contains("project published") && error.contains("media retained"));
        assert_eq!(std::fs::read(&project).unwrap(),b"published project");
        assert_eq!(std::fs::read(&occupied).unwrap(),b"incumbent receipt");
        let failure=std::fs::read_to_string(root.join("conversion-failed.json")).unwrap();
        assert_eq!(json::parse(&failure).unwrap().get("project_published"), Some(&Value::Bool(true)));
        std::fs::remove_dir_all(root).unwrap();
    }
}
