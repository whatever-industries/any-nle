//! Review for the internal graph-to-Premiere linking route.
use anynle_app::premiere_import::{self, Link, Mode, Report, WorkerState};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

const BUNDLE: &str = "Adobe Premiere Pro 2027/Adobe Premiere Pro 2027.app";

pub(crate) fn run(job: PathBuf, options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native(
        "Any NLE — Premiere Review",
        options,
        Box::new(|cc| {
            let (send, receive) = mpsc::channel();
            let mut review = Review {
                job,
                installed: installed_premiere(),
                running: premiere_running(),
                state: WorkerState::Unavailable,
                next_check: Instant::now(),
                busy: false,
                result: None,
                issue: None,
                send,
                receive,
            };
            review.start(Mode::Review, &cc.egui_ctx);
            Ok(Box::new(review))
        }),
    )
}

struct Review {
    job: PathBuf,
    installed: Option<PathBuf>,
    running: bool,
    state: WorkerState,
    next_check: Instant,
    busy: bool,
    result: Option<Report>,
    issue: Option<String>,
    send: mpsc::Sender<Result<Report, String>>,
    receive: mpsc::Receiver<Result<Report, String>>,
}

impl Review {
    fn start(&mut self, mode: Mode, ctx: &egui::Context) {
        self.busy = true;
        self.issue = None;
        let job = self.job.clone();
        let destination = self.send.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = destination.send(premiere_import::run(&job, mode));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for Review {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.receive.try_recv() {
            self.busy = false;
            match event {
                Ok(report) => self.result = Some(report),
                Err(error) => self.issue = Some(error),
            }
        }
        if self.next_check <= Instant::now() {
            self.state = premiere_import::worker_status();
            if self
                .result
                .as_ref()
                .is_some_and(|report| report.worker.is_some())
            {
                self.running = self.state != WorkerState::Unavailable || premiere_running();
            }
            self.next_check = Instant::now() + Duration::from_secs(1);
        }
        ui.ctx()
            .request_repaint_after(self.next_check.saturating_duration_since(Instant::now()));

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Review & convert");
            crate::icons::route(ui, "Graph/2", "Adobe Premiere Pro", egui::TextStyle::Body);
            ui.add_space(16.0);
            ui.group(|ui| {
                ui.heading("Status");
                ui.strong("Media linking");
                if let Some(report) = &self.result {
                    let direct = report
                        .files
                        .iter()
                        .filter(|file| file.link == Link::Direct)
                        .count();
                    let pending = report
                        .files
                        .iter()
                        .filter(|file| file.link == Link::PremiereRequired)
                        .count();
                    let donors = report.files.len() - direct - pending;
                    let (message, launch) = status_message(
                        direct,
                        donors,
                        pending,
                        self.state,
                        self.installed.is_some(),
                        self.running,
                    );
                    ui.label(message);
                    if launch && ui.button("Launch Premiere").clicked() {
                        if let Some(bundle) = &self.installed {
                            match Command::new("/usr/bin/open").arg("-a").arg(bundle).status() {
                                Ok(status) if status.success() => {}
                                _ => {
                                    self.issue = Some("Could not launch Adobe Premiere Pro.".into())
                                }
                            }
                        }
                    }
                    egui::CollapsingHeader::new("Media details").show(ui, |ui| {
                        for file in &report.files {
                            let link = match file.link {
                                Link::Direct => "Direct",
                                Link::PremiereRequired => "Premiere required",
                                Link::PremiereDonor => "Premiere media record",
                            };
                            ui.label(format!("{link} · {}", file.path));
                        }
                    });
                } else if self.busy {
                    ui.label("Checking the saved graph and original media.");
                }
                if let Some(error) = &self.issue {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            });
            ui.add_space(12.0);
            let published = self.result.as_ref().is_some_and(|report| report.published);
            if published {
                ui.strong("Premiere project created");
                return;
            }
            let ready = self
                .result
                .as_ref()
                .is_some_and(|report| report.worker.is_none() || self.state == WorkerState::Ready);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Check again"))
                    .clicked()
                {
                    self.result = None;
                    self.next_check = Instant::now();
                    self.installed = installed_premiere();
                    self.start(Mode::Review, ui.ctx());
                }
                if ui
                    .add_enabled(!self.busy && ready, egui::Button::new("Convert"))
                    .clicked()
                {
                    self.start(Mode::Convert, ui.ctx());
                }
                if self.busy {
                    ui.spinner();
                }
            });
        });
    }
}

/// The normal wizard refreshes only the helper's readiness after preflight.
pub(crate) struct WizardStatus {
    worker: WorkerState,
    next_check: Instant,
    launch: Option<PathBuf>,
    message: String,
}

impl Default for WizardStatus {
    fn default() -> Self {
        Self {
            worker: WorkerState::Unavailable,
            next_check: Instant::now(),
            launch: None,
            message: String::new(),
        }
    }
}

impl WizardStatus {
    pub(crate) fn refresh(&mut self, ctx: &egui::Context, media: &anynle_app::job::MediaLinking) {
        if self.next_check <= Instant::now() || self.message.is_empty() {
            self.worker = premiere_import::worker_status();
            let running = self.worker != WorkerState::Unavailable || premiere_running();
            let installed = (!running).then(installed_premiere).flatten();
            let (message, launch) = status_message(
                media.direct_files, 0, media.premiere_files.len(),
                self.worker, installed.is_some(), running,
            );
            self.message = message;
            self.launch = if launch { installed } else { None };
            self.next_check = Instant::now() + Duration::from_secs(1);
        }
        ctx.request_repaint_after(self.next_check.saturating_duration_since(Instant::now()));
    }

    pub(crate) fn ready(&self) -> bool {
        self.worker == WorkerState::Ready
    }

    pub(crate) fn show(&self, ui: &mut egui::Ui) -> Result<(), String> {
        ui.label(&self.message);
        if let Some(bundle) = &self.launch {
            if ui.button("Launch Premiere").clicked() {
                let status = Command::new("/usr/bin/open").arg("-a").arg(bundle).status();
                if !status.is_ok_and(|result| result.success()) {
                    return Err("Could not launch Adobe Premiere Pro.".into());
                }
            }
        }
        Ok(())
    }
}

fn installed_premiere() -> Option<PathBuf> {
    [
        Some(PathBuf::from("/Applications")),
        dirs::home_dir().map(|home| home.join("Applications")),
    ]
    .into_iter()
    .flatten()
    .map(|root| root.join(BUNDLE))
    .find(|path| is_premiere_bundle(path))
}

fn is_premiere_bundle(path: &Path) -> bool {
    path.join("Contents/MacOS/Adobe Premiere Pro 2027")
        .is_file()
        && Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
            .arg(path.join("Contents/Info.plist"))
            .output()
            .is_ok_and(|output| {
                output.status.success() && output.stdout.trim_ascii() == b"com.adobe.PremierePro.27"
            })
}

fn premiere_running() -> bool {
    Command::new("/usr/bin/pgrep")
        .args([
            "-qf",
            "/Adobe Premiere Pro 2027[.]app/Contents/MacOS/Adobe Premiere Pro 2027$",
        ])
        .status()
        .is_ok_and(|status| status.success())
}

fn originals(count: usize) -> String {
    format!(
        "{count} {}",
        if count == 1 { "original" } else { "originals" }
    )
}

fn status_message(
    direct: usize,
    donors: usize,
    pending: usize,
    worker: WorkerState,
    installed: bool,
    running: bool,
) -> (String, bool) {
    if pending == 0 {
        return if donors == 0 {
            (
                format!("Ready · All {} link directly.", originals(direct)),
                false,
            )
        } else {
            (
                format!("{direct} linked directly · {donors} use saved Premiere media records."),
                false,
            )
        };
    }
    if worker == WorkerState::Ready {
        let message = if direct == 0 {
            format!("Ready · {} will use Premiere.", originals(pending))
        } else {
            format!("Ready · {direct} link directly. {pending} will use Premiere.")
        };
        return (message, false);
    }
    let need = if pending == 1 { "needs" } else { "need" };
    let reason = match worker {
        WorkerState::ProjectOpen => "Close the open project in Premiere to free the helper.",
        WorkerState::Busy => "Premiere is processing another assisted import.",
        WorkerState::Unavailable if running => {
            "The Any NLE media helper is unavailable. Check its installation in Creative Cloud."
        }
        WorkerState::Unavailable if installed => "Launch Premiere to check the helper.",
        WorkerState::Unavailable => "Premiere 27 was not found in Applications.",
        WorkerState::Ready => unreachable!(),
    };
    (
        format!(
            "{} {need} Premiere for media linking. {reason}",
            originals(pending)
        ),
        worker == WorkerState::Unavailable && installed && !running,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_offline_premiere_with_required_media_offers_launch() {
        assert_eq!(
            status_message(3, 0, 0, WorkerState::Unavailable, true, false),
            ("Ready · All 3 originals link directly.".into(), false)
        );
        assert!(status_message(2, 0, 1, WorkerState::Unavailable, true, false).1);
        assert!(!status_message(2, 0, 1, WorkerState::Unavailable, true, true).1);
        for state in [
            WorkerState::ProjectOpen,
            WorkerState::Ready,
            WorkerState::Busy,
        ] {
            assert!(!status_message(2, 0, 1, state, true, true).1);
        }
        assert!(
            status_message(2, 0, 1, WorkerState::ProjectOpen, true, true)
                .0
                .contains("open project")
        );
    }
}
