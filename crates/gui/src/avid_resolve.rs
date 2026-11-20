//! Avid source and media choices for the gated Resolve destination.
use crate::{model::Inputs, resolve_review::Review};
use anynle_app::{
    backend_bundle,
    resolve_job::{self, SequenceRow},
};
use anynle_resolve_adapter::turnover::{JobSchema, Templates, TurnoverJob, TurnoverSource};
use eframe::egui;
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
    op1a_rewrap_dir: Option<PathBuf>,
    camera: bool,
    require_camera_originals: bool,
    proxies: bool,
    proxy_rewrap_dir: Option<PathBuf>,
    catalog: Option<Catalog>,
    pending: Option<Receiver<Result<Catalog, String>>>,
    ordered: Vec<String>,
    review: Option<Review>,
    open_mappings: bool,
    issue: Option<String>,
}

impl Wizard {
    pub(crate) fn from_job(job: TurnoverJob, ctx: &egui::Context) -> Self {
        let TurnoverSource::AvidToResolve {
            bins,
            avid_project_root,
            media_roots,
            mappings,
            sequences,
            prefer_source_media,
            require_camera_originals,
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
        wizard.op1a_rewrap_dir = op1a_rewrap_dir.clone();
        wizard.ordered.clone_from(sequences);
        wizard.camera = *prefer_source_media || *require_camera_originals || *attach_avid_proxies;
        wizard.require_camera_originals = *require_camera_originals;
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
            op1a_rewrap_dir: None,
            camera: false,
            require_camera_originals: false,
            proxies: false,
            proxy_rewrap_dir: None,
            catalog: None,
            pending: None,
            ordered: vec![],
            review: None,
            open_mappings: false,
            issue: None,
        }
    }

    fn invalidate_catalog(&mut self) {
        self.catalog = None;
        self.ordered.clear();
        self.issue = None;
    }

    fn scan(&mut self, ctx: &egui::Context) {
        self.catalog = None;
        self.issue = None;
        let (bins, root) = (self.bins.clone(), self.root.clone());
        let (send, receive) = mpsc::channel();
        let ctx = ctx.clone();
        self.pending = Some(receive);
        std::thread::spawn(move || {
            let _ = send.send(resolve_job::discover(&bins, root.as_deref()));
            ctx.request_repaint();
        });
    }

    fn set_catalog(&mut self, catalog: Catalog) {
        let mut unavailable = 0;
        self.ordered.retain_mut(|selection| {
            let rows = &catalog.1;
            if rows.iter().any(|row| &row.id == selection) {
                return true;
            }
            let mut matches = rows.iter().filter(|row| &row.name == selection);
            if let (Some(row), None) = (matches.next(), matches.next()) {
                selection.clone_from(&row.id);
                true
            } else {
                unavailable += 1;
                false
            }
        });
        if unavailable > 0 {
            self.ordered.clear();
            self.issue = Some(format!(
                "{unavailable} saved timeline selection(s) could not be resolved. Select the timelines again."
            ));
        }
        self.catalog = Some(catalog);
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
                require_camera_originals: self.camera && self.require_camera_originals,
                attach_avid_proxies: self.camera && self.proxies,
                proxy_rewrap_dir: if self.camera && self.proxies {
                    self.proxy_rewrap_dir.clone()
                } else {
                    None
                },
                op1a_rewrap_dir: self.op1a_rewrap_dir.clone().filter(|_| self.rewrap),
                ffmpeg: Some(tools.ffmpeg),
            },
            templates: self.templates.clone(),
            ffprobe: tools.ffprobe,
            output: self.output.clone(),
        })
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, embedded: bool) -> bool {
        let mut open_mappings = false;
        if let Some(review) = &mut self.review {
            let mut back = false;
            egui::Panel::top("avid-resolve-back").show(ui, |ui| {
                back = ui
                    .add_enabled(!review.busy(), egui::Button::new("Back to Avid source"))
                    .clicked();
            });
            if !back {
                open_mappings = review.show(ui);
            }
            if !back && !open_mappings {
                return false;
            }
            self.op1a_rewrap_dir = review.op1a_rewrap_dir().map(ToOwned::to_owned);
            self.review = None;
            if self.catalog.is_none() {
                self.scan(ui.ctx());
            }
            // The Review panel has already used this frame's central layout.
            if open_mappings {
                self.open_mappings = true;
                ui.ctx().request_repaint();
                return false;
            }
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(catalog) => self.set_catalog(catalog),
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
        let open_mappings = std::mem::take(&mut self.open_mappings);
        let mut exit = false;
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("avid-resolve-setup").show(ui, |ui| {
                if embedded && ui.button("Back to conversion routes").clicked() {
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
                                self.scan(ui.ctx());
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
                        crate::wizard::source_list(ui, "resolve-source-timelines", None, |ui| {
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
                        });
                        if self.ordered.len() > 1 {
                            ui.add_space(6.);
                            ui.strong("Timeline order");
                            crate::wizard::source_list(ui, "resolve-timeline-order", None, |ui| {
                                for index in 0..self.ordered.len() {
                                    ui.horizontal(|ui| {
                                        let up = crate::icons::arrow(ui, 14., "Move timeline up")
                                            .rotate(
                                                -std::f32::consts::FRAC_PI_2,
                                                egui::vec2(0.5, 0.5),
                                            );
                                        if ui
                                            .add_enabled(index > 0, egui::Button::image(up))
                                            .clicked()
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
                            });
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
                    let mappings = egui::CollapsingHeader::new("Original locations and proxies")
                        .open(open_mappings.then_some(true));
                    let response = mappings.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.mapping_prefix)
                                .hint_text("Saved folder prefix"));
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
                            ui.checkbox(&mut self.require_camera_originals, "Require camera originals")
                                .on_hover_text("Block conversion if any picture edit or editable Group camera lacks a verified original.");
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
                    if open_mappings {
                        response.header_response.scroll_to_me(Some(egui::Align::Center));
                    }
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

impl eframe::App for Wizard {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::report_view_tests::{click, text_center};

    #[test]
    fn strict_review_opens_mappings_and_preserves_timeline_selection() {
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.);
        let templates = Templates {
            layout: "layout.drp".into(),
            profile: "profile.drp".into(),
            atom: None,
            group: None,
            fairlight: None,
            nested: None,
        };
        let inputs = Inputs { output: "result.drp".into(), ..Default::default() };
        let mut wizard = Wizard::new(templates, &inputs, Some("ffmpeg".into()), Some("ffprobe".into()));
        let bin = PathBuf::from("control.avb");
        wizard.ordered = vec!["Timeline B".into(), "timeline-a-id".into()];
        wizard.set_catalog((vec![bin.clone()], vec![
            SequenceRow { id: "timeline-a-id".into(), name: "Timeline A".into(), bin: bin.clone() },
            SequenceRow { id: "timeline-b-id".into(), name: "Timeline B".into(), bin },
        ]));
        wizard.camera = true;
        wizard.require_camera_originals = true;
        wizard.rewrap = true;
        wizard.op1a_rewrap_dir = Some("OP1a Media".into());
        let review = Review::with_rewrap(wizard.settings(), true);
        review.report_camera_blocker_for_test();
        wizard.review = Some(review);

        let mut draw = |input| {
            let mut output = ctx.run_ui(input, |ui| { wizard.show(ui, false); });
            output.textures_delta.clear();
            output
        };
        let blocked = draw(egui::RawInput::default());
        assert!(text_center(&blocked, "Camera originals couldn’t be verified").is_some());
        assert!(text_center(&blocked, "Convert").is_none());
        draw(click(text_center(&blocked, "Open saved-path mappings").unwrap()));
        let edit = draw(egui::RawInput::default());
        assert!(text_center(&edit, "Map to folder…").is_some());
        assert!(text_center(&edit, "Require camera originals").is_some());

        let job = serde_json::to_value(wizard.settings().unwrap()).unwrap();
        assert!(wizard.review.is_none());
        assert_eq!(job["source"]["sequences"], serde_json::json!(["timeline-b-id", "timeline-a-id"]));
        assert_eq!(job["source"]["require_camera_originals"], true);
        assert_eq!(job["source"]["op1a_rewrap_dir"], "OP1a Media");
        wizard.camera = false;
        assert_eq!(serde_json::to_value(wizard.settings().unwrap()).unwrap()["source"]["require_camera_originals"], false);
    }
}
