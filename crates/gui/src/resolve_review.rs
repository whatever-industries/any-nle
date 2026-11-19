//! Internal Review screen for a saved Premiere to Resolve job.
use anynle_resolve_adapter::{
    MediaLinkReview,
    native_import::{self, WorkerStatus},
    turnover::{Turnover, TurnoverJob, TurnoverSource},
};
use eframe::egui::{self, Color32};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    time::Duration,
};

enum Event {
    Media(Result<MediaLinkReview, String>),
    Worker(bool, WorkerStatus),
    Progress(String),
    Prepared(Result<Box<Turnover>, String>),
    Published(Result<Option<String>, String>),
}

fn is_resolve_bundle(path: &Path) -> bool {
    path.join("Contents/MacOS/Resolve").is_file()
        && Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
            .arg(path.join("Contents/Info.plist"))
            .output()
            .is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout).trim()
                        == "com.blackmagic-design.DaVinciResolve"
            })
}

fn installed_resolve() -> Option<PathBuf> {
    let relative = [
        "DaVinci Resolve/DaVinci Resolve.app",
        "DaVinci Resolve/DaVinci Resolve Studio.app",
        "DaVinci Resolve.app",
        "DaVinci Resolve Studio.app",
    ];
    [
        Some(PathBuf::from("/Applications")),
        dirs::home_dir().map(|home| home.join("Applications")),
    ]
    .into_iter()
    .flatten()
    .flat_map(|root| relative.map(|app| root.join(app)))
    .find(|app| is_resolve_bundle(app))
}

fn running() -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-xq", "Resolve"])
        .status()
        .is_ok_and(|status| status.success())
}

fn read_job(path: &Path) -> Result<TurnoverJob, String> {
    let mut json = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take((2 << 20) + 1).read_to_end(&mut json))
        .map_err(|error| error.to_string())?;
    if json.len() > 2 << 20 {
        return Err("Resolve job exceeds its size limit".into());
    }
    let job: TurnoverJob = serde_json::from_slice(&json).map_err(|error| error.to_string())?;
    if !matches!(job.source, TurnoverSource::PremiereToResolve { .. }) {
        return Err("This Review screen requires a Premiere to Resolve job.".into());
    }
    Ok(job)
}

pub(crate) fn run(path: PathBuf, options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native(
        "Any NLE — Resolve Review",
        options,
        Box::new(move |cc| {
            let mut review = Review::new(read_job(&path));
            review.check_media(&cc.egui_ctx);
            Ok(Box::new(review))
        }),
    )
}

struct Review {
    job: Option<Arc<TurnoverJob>>,
    installed: Option<PathBuf>,
    worker: WorkerStatus,
    running: bool,
    checking_worker: bool,
    media: Option<MediaLinkReview>,
    prepared: Option<Box<Turnover>>,
    accepting_losses: bool,
    busy: bool,
    progress: String,
    done: bool,
    issue: Option<String>,
    warning: Option<String>,
    send: mpsc::Sender<Event>,
    receive: mpsc::Receiver<Event>,
}

impl Review {
    fn new(job: Result<TurnoverJob, String>) -> Self {
        let (send, receive) = mpsc::channel();
        let (job, issue) = match job {
            Ok(job) => (Some(Arc::new(job)), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            job,
            issue,
            send,
            receive,
            installed: installed_resolve(),
            worker: WorkerStatus::Disconnected,
            running: false,
            checking_worker: false,
            media: None,
            prepared: None,
            accepting_losses: false,
            busy: false,
            progress: String::new(),
            done: false,
            warning: None,
        }
    }

    fn check_media(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.clone() else { return };
        self.issue = None;
        self.prepared = None;
        self.media = None;
        self.accepting_losses = false;
        self.busy = true;
        self.progress = "Checking saved media links…".into();
        let (send, ctx) = (self.send.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = job.review_media_links().map_err(|error| error.to_string());
            let _ = send.send(Event::Media(result));
            ctx.request_repaint();
        });
    }

    fn check_worker(&mut self, ctx: &egui::Context) {
        if self.checking_worker {
            return;
        }
        self.checking_worker = true;
        let (send, ctx) = (self.send.clone(), ctx.clone());
        std::thread::spawn(move || {
            let running = running();
            let status = if running {
                native_import::worker_status()
            } else {
                WorkerStatus::Disconnected
            };
            let _ = send.send(Event::Worker(running, status));
            ctx.request_repaint();
        });
    }

    fn launch(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.installed else { return };
        match Command::new("/usr/bin/open").arg(path).status() {
            Ok(status) if status.success() => {
                self.issue = None;
                self.checking_worker = true;
                let (send, ctx) = (self.send.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let deadline = std::time::Instant::now() + Duration::from_secs(20);
                    loop {
                        let running = running();
                        let status = if running {
                            native_import::worker_status()
                        } else {
                            WorkerStatus::Disconnected
                        };
                        if status != WorkerStatus::Disconnected
                            || std::time::Instant::now() >= deadline
                        {
                            let _ = send.send(Event::Worker(running, status));
                            ctx.request_repaint();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(750));
                    }
                });
            }
            _ => self.issue = Some("DaVinci Resolve could not launch.".into()),
        }
    }

    fn prepare(&mut self, ctx: &egui::Context) {
        let Some(job) = self.job.clone() else { return };
        self.busy = true;
        self.issue = None;
        self.progress = "Preparing linked edit…".into();
        self.accepting_losses = false;
        let (send, ctx) = (self.send.clone(), ctx.clone());
        std::thread::spawn(move || {
            let progress = &mut |count, path: &Path| {
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                let _ = send.send(Event::Progress(format!(
                    "Checked {count} originals · {name}"
                )));
                ctx.request_repaint();
            };
            let result = (|| {
                let conversion = job.prepare_with_media_progress(progress)?;
                conversion.review(&job.templates, &job.output)?;
                Ok::<_, Box<dyn std::error::Error>>(Box::new(conversion))
            })()
            .map_err(|error| error.to_string());
            let _ = send.send(Event::Prepared(result));
            ctx.request_repaint();
        });
    }

    fn publish(&mut self, ctx: &egui::Context) {
        let (Some(job), Some(prepared)) = (self.job.clone(), self.prepared.take()) else {
            return;
        };
        self.busy = true;
        self.progress = "Writing the Resolve project…".into();
        let accept = self.accepting_losses;
        let (send, ctx) = (self.send.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = prepared
                .publish(&job.templates, &job.output, accept)
                .map(|project| project.cleanup_warning)
                .map_err(|error| error.to_string());
            let _ = send.send(Event::Published(result));
            ctx.request_repaint();
        });
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Launch,
    Locate,
    Check,
    None,
}

fn status_message(
    direct: usize,
    assisted: usize,
    blocked: bool,
    installed: bool,
    running: bool,
    state: &WorkerStatus,
) -> (String, Action) {
    if blocked {
        return (
            "Media linking is blocked. Check Media details, then recheck the source.".into(),
            Action::None,
        );
    }
    let linking = format!(
        "{assisted} {} assisted media linking.",
        if assisted == 1 {
            "original needs"
        } else {
            "originals need"
        }
    );
    if assisted == 0 {
        return (
            format!(
                "Ready · {direct} {} directly.",
                if direct == 1 {
                    "original links"
                } else {
                    "originals link"
                }
            ),
            Action::None,
        );
    }
    let (message, action) = match state {
        WorkerStatus::Ready => (
            format!("Ready · {direct} link directly. {assisted} will use Resolve Studio."),
            Action::None,
        ),
        WorkerStatus::Free => (
            format!("DaVinci Resolve Free is installed. {linking} This requires Studio."),
            Action::Check,
        ),
        WorkerStatus::UnsupportedBuild(version) => (
            format!(
                "Resolve Studio {version} detected. Assisted linking requires the qualified build 21.2.0.00006."
            ),
            Action::Check,
        ),
        WorkerStatus::OpenWorker(name) => (
            format!(
                "Open the dedicated scratch project “{name}” in Resolve Studio, then check again."
            ),
            Action::Check,
        ),
        WorkerStatus::Busy => (
            "Another Resolve-assisted import is running. Wait for it to finish, then check again."
                .into(),
            Action::Check,
        ),
        WorkerStatus::Rendering => (
            "The Resolve scratch project is rendering. Wait for the render to finish, then check again."
                .into(),
            Action::Check,
        ),
        WorkerStatus::ContainsContent(name) => (
            format!(
                "The scratch project “{name}” contains media, bins, or timelines. Return it to an empty state, then check again."
            ),
            Action::Check,
        ),
        WorkerStatus::Setup(instructions) => (
            format!("{instructions} Then check again."),
            Action::Check,
        ),
        WorkerStatus::Unconfigured => (
            "Configure the dedicated Resolve import worker, then check again.".into(),
            Action::Check,
        ),
        WorkerStatus::Disconnected if running => (
            format!(
                "Can’t connect to Resolve Studio. {linking} Check Preferences → System → General → External scripting using → Local."
            ),
            Action::Check,
        ),
        WorkerStatus::Disconnected if installed => (
            format!("{linking} Launch DaVinci Resolve Studio."),
            Action::Launch,
        ),
        WorkerStatus::Disconnected => (
            format!("Resolve Studio wasn’t found on this Mac. {linking}"),
            Action::Locate,
        ),
    };
    (message, action)
}

impl eframe::App for Review {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(event) = self.receive.try_recv() {
            match event {
                Event::Media(result) => {
                    self.busy = false;
                    match result {
                        Ok(review) => {
                            if review.blockers.is_empty() && !review.assist.is_empty() {
                                self.check_worker(&ctx);
                            }
                            self.media = Some(review);
                        }
                        Err(error) => self.issue = Some(error),
                    }
                }
                Event::Worker(running, status) => {
                    self.checking_worker = false;
                    self.running = running;
                    self.worker = status;
                }
                Event::Progress(progress) => self.progress = progress,
                Event::Prepared(result) => {
                    self.busy = false;
                    match result {
                        Ok(prepared) => self.prepared = Some(prepared),
                        Err(error) => {
                            self.issue = Some(error);
                            self.check_worker(&ctx);
                        }
                    }
                }
                Event::Published(result) => {
                    self.busy = false;
                    match result {
                        Ok(warning) => {
                            self.done = true;
                            self.warning = warning;
                        }
                        Err(error) => self.issue = Some(error),
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Review & convert");
                ui.label("Adobe Premiere Pro → DaVinci Resolve");
                ui.separator();
                if let Some(job) = &self.job {
                    ui.label(job.output.display().to_string());
                }
                ui.heading("Status");
                if self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&self.progress);
                    });
                }
                if self.done {
                    ui.label("Conversion complete.");
                    if let Some(warning) = &self.warning {
                        ui.label(warning);
                    }
                    return;
                }

                let mut action = Action::None;
                let worker_ready = self.worker == WorkerStatus::Ready;
                let mut can_prepare = false;
                if let Some(media) = &self.media {
                    ui.strong("Media linking");
                    let blocked = !media.blockers.is_empty();
                    let needs_worker =
                        !blocked && !media.assist.is_empty() && self.prepared.is_none();
                    let (message, next_action) = if self.checking_worker && needs_worker {
                        ("Checking Resolve Studio…".into(), Action::None)
                    } else {
                        let (direct, assisted) = self
                            .prepared
                            .as_ref()
                            .map_or((media.direct, media.assist.len()), |p| p.media_counts());
                        status_message(
                            direct,
                            assisted,
                            blocked,
                            self.installed.is_some(),
                            needs_worker && self.running,
                            &self.worker,
                        )
                    };
                    let waiting = needs_worker && !worker_ready;
                    can_prepare = !blocked && !waiting && !self.checking_worker;
                    if waiting || blocked {
                        let fill = if blocked {
                            Color32::from_rgb(114, 35, 36)
                        } else {
                            Color32::from_rgb(95, 67, 25)
                        };
                        egui::Frame::new()
                            .fill(fill)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.label(message);
                            });
                    } else {
                        ui.label(message);
                    }
                    action = next_action;
                    if !media.assist.is_empty() || blocked {
                        ui.collapsing("Media details", |ui| {
                            ui.label(format!("{} direct links", media.direct));
                            for affected in &media.assist {
                                ui.label(affected);
                            }
                            for issue in &media.blockers {
                                ui.colored_label(Color32::LIGHT_RED, issue);
                            }
                        });
                    }
                }

                if let Some(issue) = &self.issue {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(114, 35, 36))
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.label(issue);
                        });
                }
                if !self.busy && self.prepared.is_none() {
                    if action == Action::Launch && ui.button("Launch Resolve Studio").clicked() {
                        self.launch(&ctx);
                    }
                    if action == Action::Locate && ui.button("Locate Resolve Studio…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_directory("/Applications")
                            .add_filter("macOS application", &["app"])
                            .pick_file()
                        {
                            if is_resolve_bundle(&path) {
                                self.installed = Some(path);
                                self.issue = None;
                                self.check_worker(&ctx);
                            } else {
                                self.issue =
                                    Some("Choose the DaVinci Resolve application bundle.".into());
                            }
                        }
                    }
                    if action != Action::None && ui.button("Check again").clicked() {
                        if self.installed.is_none() {
                            self.installed = installed_resolve();
                        }
                        self.check_worker(&ctx);
                    }
                    if self.job.is_some() && ui.button("Recheck source").clicked() {
                        self.check_media(&ctx);
                    }
                    if ui
                        .add_enabled(can_prepare, egui::Button::new("Prepare project"))
                        .clicked()
                    {
                        self.prepare(&ctx);
                    }
                }

                if let Some(prepared) = &self.prepared {
                    let needs_approval = prepared.reviewable_losses().next().is_some()
                        || !prepared.omitted_markers().is_empty();
                    if needs_approval {
                        ui.collapsing("Conversion notes", |ui| {
                            for loss in prepared.reviewable_losses() {
                                ui.colored_label(
                                    Color32::YELLOW,
                                    format!("{} · {}", loss.object, loss.reason),
                                );
                            }
                            for marker in prepared.omitted_markers() {
                                ui.colored_label(
                                    Color32::YELLOW,
                                    format!("Omitted marker {marker}"),
                                );
                            }
                        });
                        ui.checkbox(
                            &mut self.accepting_losses,
                            "Accept these omissions for this conversion",
                        );
                    }
                    if ui
                        .add_enabled(
                            !needs_approval || self.accepting_losses,
                            egui::Button::new("Convert"),
                        )
                        .clicked()
                    {
                        self.publish(&ctx);
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_depends_on_current_media_and_observed_worker_state() {
        let status = |assisted, installed, running, state| {
            status_message(9, assisted, false, installed, running, &state)
        };
        assert_eq!(
            status(0, false, false, WorkerStatus::Disconnected).1,
            Action::None
        );
        assert_eq!(
            status(3, false, false, WorkerStatus::Disconnected).1,
            Action::Locate
        );
        assert_eq!(
            status(3, true, false, WorkerStatus::Disconnected).1,
            Action::Launch
        );
        assert_eq!(
            status(3, true, true, WorkerStatus::Disconnected).1,
            Action::Check
        );
        assert!(status(3, true, true, WorkerStatus::Free).0.contains("Free"));
        assert!(
            status(3, true, true, WorkerStatus::UnsupportedBuild("21.0".into()))
                .0
                .contains("21.0")
        );
        assert!(
            status(3, true, true, WorkerStatus::OpenWorker("scratch".into()))
                .0
                .contains("scratch")
        );
        assert_eq!(status(3, true, true, WorkerStatus::Ready).1, Action::None);
    }

    #[test]
    fn scratch_worker_messages_offer_state_specific_recovery() {
        for (state, instructions) in [
            (WorkerStatus::Busy, "assisted import is running"),
            (WorkerStatus::Rendering, "rendering"),
            (
                WorkerStatus::ContainsContent("Test worker".into()),
                "“Test worker” contains media, bins, or timelines",
            ),
            (
                WorkerStatus::Setup("Select the Media Pool root folder.".into()),
                "Select the Media Pool root folder.",
            ),
        ] {
            let (message, action) = status_message(1, 1, false, true, true, &state);
            assert!(message.contains(instructions), "{message}");
            assert_eq!(action, Action::Check);
        }
    }

    #[test]
    fn blocked_media_suppresses_ready_and_launch() {
        for assisted in [0, 3] {
            let (message, action) =
                status_message(9, assisted, true, true, false, &WorkerStatus::Disconnected);
            assert!(message.starts_with("Media linking is blocked."));
            assert_eq!(action, Action::None);
        }
    }
}
