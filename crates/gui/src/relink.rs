use anynle_resolve_adapter::relink::{self, Move};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

struct Row {
    original: String,
    destination: Option<String>,
    capture: bool,
}

#[derive(Default)]
pub(crate) struct Relink {
    pub(crate) open: bool,
    project: Option<PathBuf>,
    rows: Vec<Row>,
    search: String,
    checksum: bool,
    manifest: Option<PathBuf>,
    result: Option<Result<String, String>>,
    worker: Option<Receiver<Result<String, String>>>,
}

impl Relink {
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        ffmpeg: Option<&Path>,
        ffprobe: Option<&Path>,
    ) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.worker {
            match rx.try_recv() {
                Ok(result) => {
                    self.result = Some(result);
                    self.worker = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.result = Some(Err(
                        "The relink operation stopped before reporting a result.".into(),
                    ));
                    self.worker = None;
                }
            }
        }
        let modal =
            egui::Modal::new(egui::Id::unique("anynle-relink-existing-project")).show(ctx, |ui| {
                ui.set_width(540.);
                ui.heading("Relink media");
                ui.label("Premiere 27 and Resolve Studio 21.2 · linked MOV originals");
                ui.add_enabled_ui(self.worker.is_none(), |ui| self.form(ui, ffmpeg, ffprobe));
                if self.worker.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking files and writing output…");
                    });
                }
                if let Some(result) = &self.result {
                    match result {
                        Ok(message) => {
                            ui.label(message);
                        }
                        Err(error) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, error);
                        }
                    }
                }
            });
        if modal.should_close() && self.worker.is_none() {
            self.open = false;
        }
    }

    fn form(&mut self, ui: &mut egui::Ui, ffmpeg: Option<&Path>, ffprobe: Option<&Path>) {
        if ui.button("Choose project…").clicked()
            && let Some(project) = rfd::FileDialog::new()
                .add_filter("Native project", &["prproj", "drp"])
                .pick_file()
        {
            self.result = None;
            match relink::originals(&project) {
                Ok(paths) => {
                    self.project = Some(project);
                    self.rows = paths
                        .into_iter()
                        .map(|original| Row {
                            original,
                            destination: None,
                            capture: false,
                        })
                        .collect();
                    self.search.clear();
                    self.checksum = false;
                    self.manifest = None;
                }
                Err(error) => self.result = Some(Err(error.to_string())),
            }
        }
        let Some(project) = self.project.clone() else {
            if ui.button("Close").clicked() {
                self.open = false;
            }
            return;
        };
        ui.label(project.display().to_string());
        ui.checkbox(&mut self.checksum, "Verify file contents with SHA-256");
        if self.checksum {
            ui.colored_label(
                egui::Color32::from_rgb(244, 199, 137),
                "Leave off for routine moves. This reads every byte of the selected media and requires checksums from before the move.",
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Choose checksum manifest…").clicked() {
                    self.manifest = rfd::FileDialog::new()
                        .add_filter("Checksum manifest", &["json"])
                        .pick_file();
                }
                let count = self.rows.iter().filter(|row| row.capture).count();
                if ui
                    .add_enabled(count != 0, egui::Button::new("Capture checked originals…"))
                    .clicked()
                    && let Some(output) = rfd::FileDialog::new()
                        .add_filter("Checksum manifest", &["json"])
                        .set_file_name("original-checksums.json")
                        .save_file()
                {
                    let selected: Vec<_> = self
                        .rows
                        .iter()
                        .filter(|row| row.capture)
                        .map(|row| row.original.clone())
                        .collect();
                    let source = project.clone();
                    self.run(ui.ctx(), move || {
                        let published = relink::capture(&source, Some(&selected), &output)
                            .map_err(|error| error.to_string())?;
                        Ok(published
                            .cleanup_warning
                            .unwrap_or_else(|| format!("Saved {}", output.display())))
                    });
                }
            });
            if let Some(manifest) = &self.manifest {
                ui.label(manifest.display().to_string());
            }
        }
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Filter by saved path"));
        let filter = self.search.to_lowercase();
        let mut invalid_path = false;
        egui::ScrollArea::vertical()
            .id_salt("relink-originals")
            .max_height(200.)
            .show(ui, |ui| {
                for row in self
                    .rows
                    .iter_mut()
                    .filter(|row| row.original.to_lowercase().contains(&filter))
                {
                    ui.push_id(&row.original, |ui| {
                        ui.horizontal(|ui| {
                            if self.checksum {
                                ui.checkbox(&mut row.capture, "")
                                    .on_hover_text("Include in checksum capture");
                            }
                            ui.add(egui::Label::new(&row.original).truncate())
                                .on_hover_text(&row.original);
                        });
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Choose moved file…").clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .add_filter("QuickTime movie", &["mov"])
                                    .pick_file()
                            {
                                if let Some(path) = path.to_str() {
                                    row.destination = Some(path.into());
                                } else {
                                    invalid_path = true;
                                }
                            }
                            if let Some(path) = &row.destination {
                                if ui.small_button("Clear").clicked() {
                                    row.destination = None;
                                } else {
                                    ui.add(egui::Label::new(path).truncate())
                                        .on_hover_text(path);
                                }
                            }
                        });
                    });
                }
            });
        if invalid_path {
            self.result = Some(Err(
                "Choose a media path that can be represented as UTF-8.".into()
            ));
        }
        let moves: Vec<_> = self
            .rows
            .iter()
            .filter_map(|row| {
                Some(Move {
                    original: row.original.clone(),
                    destination: row.destination.clone()?,
                })
            })
            .collect();
        ui.horizontal(|ui| {
            if ui.button("Close").clicked() {
                self.open = false;
            }
            let can_save = self.worker.is_none()
                && !moves.is_empty()
                && (!self.checksum || self.manifest.is_some());
            if ui
                .add_enabled(
                    can_save,
                    egui::Button::new(format!("Save relinked project ({} moved)…", moves.len())),
                )
                .clicked()
            {
                let ext = project
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or_default();
                if let Some(output) = rfd::FileDialog::new()
                    .add_filter("Native project", &[ext])
                    .set_file_name(format!("Relinked.{ext}"))
                    .save_file()
                {
                    let manifest = self.manifest.clone().filter(|_| self.checksum);
                    let ffmpeg = ffmpeg.map(Path::to_owned);
                    let ffprobe = ffprobe.map(Path::to_owned);
                    self.run(ui.ctx(), move || {
                        let tools = anynle_app::backend_bundle::resolve(
                            ffmpeg.as_deref(),
                            ffprobe.as_deref(),
                            false,
                        )?;
                        let published = relink::apply(
                            &project,
                            &moves,
                            &output,
                            &tools.ffprobe,
                            manifest.as_deref(),
                        )
                        .map_err(|error| error.to_string())?;
                        Ok(published
                            .cleanup_warning
                            .unwrap_or_else(|| format!("Saved {}", output.display())))
                    });
                }
            }
        });
    }

    fn run(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.result = None;
        self.worker = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }
}
