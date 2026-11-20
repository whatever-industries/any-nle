//! Local file exchange with the installed Premiere UXP helper.
use super::{bounded_file, Result};
use prproj::{RandomUuids, UuidSource};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SCHEMA: &str = "anynle.premiere-worker/2";
const MAX_MESSAGE: usize = 64 << 10;
const REQUEST_LIFETIME: u64 = 30_000;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Unavailable,
    Ready,
    ProjectOpen,
    Busy,
}

#[derive(Deserialize)]
struct Status {
    schema: String,
    timestamp: u64,
    state: WorkerState,
}

#[derive(Serialize)]
struct Request<'a> {
    schema: &'static str,
    id: &'a str,
    expires: u64,
    originals: &'a [String],
}

#[derive(Deserialize)]
struct Response {
    schema: String,
    id: String,
    #[serde(default)]
    files: Vec<ImportedFile>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ImportedFile {
    path: String,
    online: bool,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as u64)
}

fn mailbox() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if !home.is_absolute() {
        return None;
    }
    Some(home.join(
        "Library/Application Support/Adobe/UXP/PluginsStorage/PPRO/27/External/\
         work.whatever-industries.anynle.media-link/PluginData/worker-v1",
    ))
}

pub fn status() -> WorkerState {
    mailbox().map_or(WorkerState::Unavailable, |folder| status_at(&folder))
}

fn status_at(folder: &Path) -> WorkerState {
    if !fs::symlink_metadata(folder).is_ok_and(|meta| meta.is_dir()) {
        return WorkerState::Unavailable;
    }
    let status: Status = match read(&folder.join("status.json")) {
        Some(status) => status,
        None => return WorkerState::Unavailable,
    };
    let now = now_millis();
    let max_age = if status.state == WorkerState::Busy {
        45_000
    } else {
        5_000
    };
    if status.schema != SCHEMA || status.timestamp > now || now - status.timestamp > max_age {
        return WorkerState::Unavailable;
    }
    status.state
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    serde_json::from_str(&bounded_file(path, MAX_MESSAGE).ok()?).ok()
}

pub(super) struct Donor {
    job: PathBuf,
    pub path: PathBuf,
}

impl Drop for Donor {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.job);
    }
}

pub(super) fn capture(originals: &[String]) -> Result<Donor> {
    for path in originals {
        let file = Path::new(path);
        if !file.is_absolute() || !file.is_file() {
            return Err(format!("{path}: Premiere import needs an online absolute file"));
        }
    }
    let folder = mailbox().ok_or("Premiere-assisted linking is unavailable on this platform")?;
    capture_at(&folder, originals)
}

fn capture_at(folder: &Path, originals: &[String]) -> Result<Donor> {
    match status_at(folder) {
        WorkerState::Ready => {}
        WorkerState::ProjectOpen => {
            return Err("Close the open Premiere project before using assisted linking".into());
        }
        WorkerState::Busy => {
            return Err("Premiere is already running an assisted import. Wait and recheck".into());
        }
        WorkerState::Unavailable => {
            return Err("The Any NLE media helper is unavailable in Premiere. Install or launch it, then recheck".into());
        }
    }
    let id = RandomUuids::new().uuid4().to_string();
    let expires = now_millis().saturating_add(REQUEST_LIFETIME);
    let request = serde_json::to_vec(&Request {
        schema: SCHEMA,
        id: &id,
        expires,
        originals,
    })
    .map_err(|error| error.to_string())?;
    if request.len() > MAX_MESSAGE {
        return Err("Premiere-assisted import request is too large".into());
    }
    // The helper serializes requests. An exclusive folder and atomic rename keep
    // concurrent clients and partially written requests from sharing a scratch.
    let job = folder.join(format!("job-{id}"));
    create_job(&job).map_err(|error| error.to_string())?;
    let staging = job.join("request.tmp");
    let publish =
        fs::write(&staging, request).and_then(|()| fs::rename(&staging, job.join("request.json")));
    if let Err(error) = publish {
        let _ = fs::remove_dir_all(&job);
        return Err(format!(
            "Could not submit the Premiere media import: {error}"
        ));
    }
    let result = job.join("result.json");
    let deadline = std::time::Instant::now() + Duration::from_secs(65);
    loop {
        if let Some(response) = read::<Response>(&result) {
            if response.schema != SCHEMA || response.id != id {
                return Err("Premiere returned an invalid import receipt".into());
            }
            if let Some(error) = response.error {
                // The helper may still own a scratch file if close failed. Keep
                // this job's files available for diagnosis and recovery.
                return Err(error);
            }
            let path = job.join(format!("Any NLE Import {id}/Donor.prproj"));
            let donor = Donor { job, path };
            if response.files.len() != originals.len()
                || response
                    .files
                    .iter()
                    .zip(originals)
                    .any(|(file, expected)| !file.online || &file.path != expected)
                || !fs::symlink_metadata(&donor.path).is_ok_and(|meta| meta.is_file())
            {
                return Err("Premiere did not save a donor for the exact online originals".into());
            }
            return Ok(donor);
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "Premiere did not finish assisted media import. Check its worker status and retry"
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn create_job(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(test: impl FnOnce(&Path)) {
        let folder =
            std::env::temp_dir().join(format!("anynle-test-{}", RandomUuids::new().uuid4()));
        fs::create_dir(&folder).unwrap();
        let cleanup = Donor {
            job: folder.clone(),
            path: folder.clone(),
        };
        test(&folder);
        drop(cleanup);
    }

    fn write_status(folder: &Path, state: &str, timestamp: u64) {
        fs::write(
            folder.join("status.json"),
            serde_json::json!({
                "schema": SCHEMA, "timestamp": timestamp, "state": state
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn stale_and_busy_heartbeats_do_not_submit_requests() {
        sandbox(|folder| {
            let media = ["/tmp/control.mov".to_string()];
            assert_eq!(status_at(folder), WorkerState::Unavailable);
            write_status(folder, "ready", now_millis() - 6_000);
            assert_eq!(status_at(folder), WorkerState::Unavailable);
            for (value, expected) in [
                ("project_open", WorkerState::ProjectOpen),
                ("busy", WorkerState::Busy),
            ] {
                write_status(folder, value, now_millis());
                assert_eq!(status_at(folder), expected);
                assert!(capture_at(folder, &media).is_err());
                assert_eq!(fs::read_dir(folder).unwrap().count(), 1);
            }
        });
    }

    #[test]
    fn matches_the_exact_online_result_and_cleans_completed_scratch() {
        sandbox(|folder| {
            let media = ["/tmp/control.mov".to_string()];
            write_status(folder, "ready", now_millis());
            std::thread::scope(|scope| {
                let paths = &media;
                scope.spawn(move || {
                    for _ in 0..200 {
                        for entry in fs::read_dir(folder).unwrap().flatten() {
                            let path = entry.path();
                            let Some(request): Option<serde_json::Value> = read(&path.join("request.json")) else {
                                continue;
                            };
                            assert_eq!(request["originals"], serde_json::json!(paths));
                            let id = request["id"].as_str().unwrap();
                            let result = path.join(format!("Any NLE Import {id}"));
                            fs::create_dir(&result).unwrap();
                            fs::write(result.join("Donor.prproj"), b"test receipt only").unwrap();
                            fs::write(path.join("result.tmp"), serde_json::json!({
                                "schema": SCHEMA, "id": id, "files": [{ "path": &paths[0], "online": true }]
                            }).to_string()).unwrap();
                            fs::rename(path.join("result.tmp"), path.join("result.json")).unwrap();
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    panic!("No request arrived");
                });
                let donor = capture_at(folder, &media).unwrap();
                assert!(donor.path.is_file());
                drop(donor);
                assert_eq!(fs::read_dir(folder).unwrap().count(), 1);
            });
        });
    }
}
