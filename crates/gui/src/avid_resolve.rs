//! Avid source and media choices for the gated Resolve destination.
use crate::{model::Inputs, resolve_review::Review};
use anynle_app::{
    backend_bundle,
    resolve_job::{self, SequenceRow},
};
use anynle_resolve_adapter::turnover::{JobSchema, Templates, TurnoverJob, TurnoverSource};
use eframe::{App as _, egui};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

type Catalog = (Vec<PathBuf>, Vec<SequenceRow>);

pub(crate) struct Wizard {
    templates: Templates,
    root: Option<PathBuf>,
    bins: Vec<PathBuf>,
    output: PathBuf,
    media_roots: Vec<PathBuf>,
    mappings: Vec<[PathBuf; 2]>,
    mapping_prefix: String,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    rewrap: bool,
    camera: bool,
    proxies: bool,
    proxy_rewrap_dir: Option<PathBuf>,
    catalog: Option<Catalog>,
    pending: Option<Receiver<Result<Catalog, String>>>,
    ordered: Vec<String>,
    review: Option<Review>,
    issue: Option<String>,
}

impl Wizard {
    pub(crate) fn from_job(job: TurnoverJob, ctx: &egui::Context) -> Self {
        let TurnoverSource::AvidToResolve {
            bins,
            avid_project_root,
            media_roots,
            mappings,
            prefer_source_media,
            attach_avid_proxies,
            proxy_rewrap_dir,
            op1a_rewrap_dir,
            ffmpeg,
            ..
        } = &job.source
        else {
            unreachable!("only Avid jobs enter this wizard")
        };
        let inputs = Inputs {
            project_folder: avid_project_root.clone(),
            bins: bins.clone(),
            output: job.output.clone(),
            media_roots: media_roots.clone(),
            path_mappings: mappings
                .iter()
                .map(|[from, to]| (from.clone(), to.clone()))
                .collect(),
            ..Default::default()
        };
        let mut wizard = Self::new(
            job.templates.clone(),
            &inputs,
            ffmpeg.clone(),
            Some(job.ffprobe.clone()),
        );
        wizard.rewrap = op1a_rewrap_dir.is_some();
        wizard.camera = *prefer_source_media;
        wizard.proxies = *attach_avid_proxies;
        wizard.proxy_rewrap_dir = proxy_rewrap_dir.clone();
        let mut review = Review::with_rewrap(Ok(job), wizard.rewrap);
        review.check_media(ctx);
        wizard.review = Some(review);
        wizard
    }

    pub(crate) fn new(
        templates: Templates,
        inputs: &Inputs,
        ffmpeg: Option<PathBuf>,
        ffprobe: Option<PathBuf>,
    ) -> Self {
        Self {
            templates,
            root: inputs.project_folder.clone(),
            bins: inputs.bins.clone(),
            output: if inputs.output.as_os_str().is_empty() {
                PathBuf::new()
            } else {
                inputs.output.with_extension("drp")
            },
            media_roots: inputs.media_roots.clone(),
            mappings: inputs
                .path_mappings
                .iter()
                .map(|(saved, actual)| [saved.into(), actual.clone()])
                .collect(),
            mapping_prefix: String::new(),
            ffmpeg,
            ffprobe,
            rewrap: false,
            camera: false,
            proxies: false,
            proxy_rewrap_dir: None,
            catalog: None,
            pending: None,
            ordered: vec![],
            review: None,
            issue: None,
        }
    }

    fn invalidate_catalog(&mut self) {
        self.catalog = None;
        self.ordered.clear();
        self.issue = None;
    }

    fn scan(&mut self, ui: &egui::Ui) {
        self.invalidate_catalog();
        let (bins, root) = (self.bins.clone(), self.root.clone());
        let (send, receive) = mpsc::channel();
        let ctx = ui.ctx().clone();
        self.pending = Some(receive);
        std::thread::spawn(move || {
            let _ = send.send(resolve_job::discover(&bins, root.as_deref()));
            ctx.request_repaint();
        });
    }

    fn settings(&self) -> Result<TurnoverJob, String> {
        let (bins, _) = self
            .catalog
            .as_ref()
            .ok_or("Read the Avid timelines first.")?;
        if !(1..=64).contains(&self.ordered.len()) {
            return Err("Select between 1 and 64 timelines.".into());
        }
        let tools =
            backend_bundle::resolve(self.ffmpeg.as_deref(), self.ffprobe.as_deref(), false)?;
        Ok(TurnoverJob {
            schema: JobSchema::V2,
            source: TurnoverSource::AvidToResolve {
                bins: bins.clone(),
                sequences: self.ordered.clone(),
                avid_project_root: self.root.clone(),
                media_roots: self.media_roots.clone(),
                mappings: self.mappings.clone(),
                prefer_source_media: self.camera,
                require_camera_originals: false,
                attach_avid_proxies: self.camera && self.proxies,
                proxy_rewrap_dir: if self.camera && self.proxies {
                    self.proxy_rewrap_dir.clone()
                } else {
                    None
                },
                op1a_rewrap_dir: None,
                ffmpeg: Some(tools.ffmpeg),
            },
            templates: self.templates.clone(),
            ffprobe: tools.ffprobe,
            output: self.output.clone(),
        })
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) -> bool {
        if let Some(review) = &mut self.review {
            let mut back = false;
            egui::Panel::top("avid-resolve-back").show(ui, |ui| {
                back = ui
                    .add_enabled(!review.busy(), egui::Button::new("Back to Avid source"))
                    .clicked();
            });
            if back {
                self.review = None;
            } else {
                review.ui(ui, frame);
                return false;
            }
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(catalog) => self.catalog = Some(catalog),
                        Err(error) => self.issue = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.issue = Some("Timeline inventory stopped. Read timelines again.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut exit = false;
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if ui.button("Back to conversion routes").clicked() {
                    exit = true;
                    return;
                }
                crate::icons::route(
                    ui,
                    "Avid Media Composer",
                    "DaVinci Resolve",
                    egui::TextStyle::Heading,
                );
                ui.group(|ui| {
                    ui.strong("Source");
                    ui.add_enabled_ui(self.pending.is_none(), |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Choose project folder…").clicked() {
                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                    self.root = Some(path);
                                    self.bins.clear();
                                    self.invalidate_catalog();
                                }
                            }
                            if ui.button("Choose bins…").clicked() {
                                if let Some(bins) = rfd::FileDialog::new()
                                    .add_filter("Avid bins", &["avb"])
                                    .pick_files()
                                {
                                    self.bins = bins;
                                    if self.root.as_ref().is_some_and(|root| {
                                        self.bins.iter().any(|bin| !bin.starts_with(root))
                                    }) {
                                        self.root = None;
                                    }
                                    self.invalidate_catalog();
                                }
                            }
                            if ui
                                .add_enabled(
                                    self.root.is_some() || !self.bins.is_empty(),
                                    egui::Button::new("Read timelines"),
                                )
                                .clicked()
                            {
                                self.scan(ui);
                            }
                        });
                    });
                    if let Some(root) = &self.root {
                        ui.label(root.display().to_string());
                    }
                    for path in &self.bins {
                        ui.label(path.display().to_string());
                    }
                    if self.pending.is_some() {
                        ui.spinner();
                    }
                    if let Some((_, rows)) = &self.catalog {
                        if rows.is_empty() {
                            ui.label("No timelines found in the selected bins.");
                        }
                        for row in rows {
                            let mut selected = self.ordered.contains(&row.id);
                            let bin = self
                                .root
                                .as_ref()
                                .and_then(|root| row.bin.strip_prefix(root).ok())
                                .unwrap_or(&row.bin);
                            if ui
                                .checkbox(
                                    &mut selected,
                                    format!("{} · {}", row.name, bin.display()),
                                )
                                .on_hover_text(&row.id)
                                .changed()
                            {
                                if selected {
                                    self.ordered.push(row.id.clone());
                                } else {
                                    self.ordered.retain(|id| id != &row.id);
                                }
                            }
                        }
                        if self.ordered.len() > 1 {
                            ui.separator();
                            ui.strong("Timeline order");
                            for index in 0..self.ordered.len() {
                                ui.horizontal(|ui| {
                                    let up = crate::icons::arrow(ui, 14., "Move timeline up")
                                        .rotate(-std::f32::consts::FRAC_PI_2, egui::vec2(0.5, 0.5));
                                    if ui.add_enabled(index > 0, egui::Button::image(up)).clicked()
                                    {
                                        self.ordered.swap(index, index - 1);
                                    }
                                    if let Some(row) =
                                        rows.iter().find(|row| row.id == self.ordered[index])
                                    {
                                        ui.label(&row.name)
                                            .on_hover_text(row.bin.display().to_string());
                                    }
                                });
                            }
                        }
                    }
                });
                ui.group(|ui| {
                    ui.strong("Media");
                    ui.radio_value(&mut self.rewrap, false, "Link to originals");
                    ui.radio_value(&mut self.rewrap, true, "Rewrap managed media to OP1a");
                    if self.rewrap {
                        ui.label("Creates new media.");
                    }
                    if ui.button("Add Avid media folder…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            if !self.media_roots.contains(&path) {
                                self.media_roots.push(path);
                            }
                        }
                    }
                    let mut remove = None;
                    for (index, root) in self.media_roots.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(root.display().to_string());
                            if ui.small_button("Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                    }
                    if let Some(index) = remove {
                        self.media_roots.remove(index);
                    }
                    ui.collapsing("Original locations and proxies", |ui| {
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut self.mapping_prefix)
                                .on_hover_text("Saved path or folder prefix");
                            if ui
                                .add_enabled(
                                    !self.mapping_prefix.trim().is_empty(),
                                    egui::Button::new("Map to folder…"),
                                )
                                .clicked()
                            {
                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                    self.mappings
                                        .push([PathBuf::from(self.mapping_prefix.trim()), path]);
                                    self.mapping_prefix.clear();
                                }
                            }
                        });
                        let mut remove = None;
                        for (index, [saved, actual]) in self.mappings.iter().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(saved.display().to_string());
                                ui.add(crate::icons::arrow(ui, 13., "to"));
                                ui.label(actual.display().to_string());
                                if ui.small_button("Remove").clicked() {
                                    remove = Some(index);
                                }
                            });
                        }
                        if let Some(index) = remove {
                            self.mappings.remove(index);
                        }
                        ui.checkbox(&mut self.camera, "Prefer camera originals for picture");
                        if self.camera {
                            ui.checkbox(&mut self.proxies, "Attach matching Avid media as proxies");
                            if self.proxies
                                && ui.button("Choose folder for proxy rewrap…").clicked()
                            {
                                self.proxy_rewrap_dir = rfd::FileDialog::new().pick_folder();
                            }
                            if self.proxies {
                                if let Some(dir) = &self.proxy_rewrap_dir {
                                    ui.label(dir.display().to_string());
                                    if ui.button("Link existing proxies only").clicked() {
                                        self.proxy_rewrap_dir = None;
                                    }
                                }
                            }
                        }
                    });
                });
                ui.group(|ui| {
                    ui.strong("Destination");
                    if ui.button("Choose Resolve project…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Resolve project", &["drp"])
                            .save_file()
                        {
                            self.output = if path.extension().is_some() {
                                path
                            } else {
                                path.with_extension("drp")
                            };
                        }
                    }
                    ui.label(self.output.display().to_string());
                });
                if let Some(issue) = &self.issue {
                    ui.colored_label(egui::Color32::LIGHT_RED, issue);
                }
                let ready = self.pending.is_none()
                    && (1..=64).contains(&self.ordered.len())
                    && self
                        .output
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("drp"));
                if ui.add_enabled(ready, egui::Button::new("Review")).clicked() {
                    let mut review = Review::with_rewrap(self.settings(), self.rewrap);
                    review.check_media(ui.ctx());
                    self.review = Some(review);
                }
            });
        });
        exit
    }
}
