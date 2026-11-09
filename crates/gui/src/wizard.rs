use crate::model::{Choices, ClipMode, Page, Preset, Wizard};
use crate::{palette, storage};
use anynle_app::job::{
    self, CancelToken, Completion, CompletionStatus, Inventory, JobSpec, PreparedJob, Progress,
    Review,
};
use eframe::egui::{self, Color32, RichText};
use mapping::{AvidToPremiere, BinMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

const ACCENT: Color32 = Color32::from_rgb(190, 164, 255);
const MUTED: Color32 = Color32::from_rgb(164, 170, 190);
const GOOD: Color32 = Color32::from_rgb(133, 222, 190);
const AMBER: Color32 = Color32::from_rgb(244, 199, 137);
const WHY: &str = include_str!("../../../docs/DECISIONS-EXPLAINED.md");
#[derive(Clone, Copy)]
enum Pick {
    Bins,
    Project,
    Output,
    Media,

    NewMedia,
    LoadPreset,
    SavePreset,
    SaveJob,
    LoadJob,
    Ffmpeg,
    Ffprobe,
}
enum Event {
    Progress(Progress),
    Inventory(u64, Result<Inventory, String>),
    Prepared(u64, Result<Box<PreparedJob>, String>),
    Finished(Completion),
    Picked(Pick, Option<Vec<PathBuf>>),
    WorkerFailed(String),
}
#[derive(Default)]
struct Tree {
    folders: BTreeMap<String, Tree>,
    files: Vec<usize>,
}
impl Tree {
    fn build(inv: &Inventory) -> Self {
        let mut tree = Self::default();
        for (i, bin) in inv.bins.iter().enumerate() {
            let relative = &bin.relative;
            let mut node = &mut tree;
            if let Some(parent) = relative.parent() {
                for part in parent.components() {
                    let name = part.as_os_str().to_string_lossy().into_owned();
                    node = node.folders.entry(name).or_default();
                }
            }
            node.files.push(i);
        }
        tree
    }
    fn indices(&self) -> Vec<usize> {
        self.files
            .iter()
            .copied()
            .chain(self.folders.values().flat_map(Self::indices))
            .collect()
    }
    fn show(
        &self,
        ui: &mut egui::Ui,
        inventory: &Inventory,
        selected: &mut Vec<PathBuf>,
        search: &str,
        entire: bool,
        prefix: &str,
    ) {
        for (name, node) in &self.folders {
            let indices = node.indices();
            if !indices.iter().any(|i| {
                inventory.bins[*i]
                    .relative
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(search)
            }) {
                continue;
            }
            let key = format!("{prefix}/{name}");
            ui.horizontal(|ui| {
                let mut all = entire
                    || indices
                        .iter()
                        .all(|i| selected.contains(&inventory.bins[*i].path));
                if ui
                    .checkbox(&mut all, "")
                    .on_hover_text("Select this folder's bins")
                    .changed()
                {
                    for i in &indices {
                        let path = &inventory.bins[*i].path;
                        if all {
                            if !selected.contains(path) {
                                selected.push(path.clone());
                            }
                        } else {
                            selected.retain(|p| p != path);
                        }
                    }
                }
                egui::CollapsingHeader::new(format!("{name}  ·  {} bins", indices.len()))
                    .id_salt(&key)
                    .default_open(true)
                    .show(ui, |ui| {
                        node.show(ui, inventory, selected, search, entire, &key)
                    });
            });
        }
        for i in &self.files {
            let bin = &inventory.bins[*i];
            if !bin
                .relative
                .to_string_lossy()
                .to_lowercase()
                .contains(search)
            {
                continue;
            }
            ui.horizontal(|ui| {
                let mut value = entire || selected.contains(&bin.path);
                if ui
                    .checkbox(
                        &mut value,
                        bin.path.file_name().unwrap_or_default().to_string_lossy(),
                    )
                    .changed()
                {
                    if value {
                        selected.push(bin.path.clone());
                    } else {
                        selected.retain(|p| p != &bin.path);
                    }
                }
                ui.label(
                    RichText::new(format!(
                        "{} clips  ·  {} sequences{}",
                        bin.clips,
                        bin.sequences,
                        if bin.dependency {
                            "  ·  dependency"
                        } else {
                            ""
                        }
                    ))
                    .small()
                    .color(MUTED),
                );
            });
        }
    }
}

pub struct App {
    wizard: Wizard,
    inventory: Option<Inventory>,
    prepared: Option<PreparedJob>,
    review: Option<Review>,
    completion: Option<Completion>,
    tx: SyncSender<Event>,
    rx: Receiver<Event>,
    cancel: CancelToken,
    busy: bool,
    task: String,
    progress: Option<Progress>,
    error: Option<String>,
    notice: Option<String>,
    search: String,
    report_search: String,
    custom_color: String,
    why: Option<String>,
    preset_name: String,
    show_colors: bool,
    mapping_saved: String,
    mapping_actual: String,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    close_after_cancel: bool,
    close_confirmation: bool,
    logo: egui::TextureHandle,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, replay: Option<PathBuf>) -> Self {
        Self::theme(&cc.egui_ctx);
        let source = image::load_from_memory(include_bytes!("../../../assets/icon_v1.jpg"))
            .expect("embedded brand")
            .resize(160, 160, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [source.width() as usize, source.height() as usize],
            &source,
        );
        let logo = cc
            .egui_ctx
            .load_texture("Any NLE", pixels, egui::TextureOptions::LINEAR);
        let (tx, rx) = mpsc::sync_channel(32);
        let mut wizard = Wizard::default();
        let mut error = None;
        match storage::load() {
            Ok(Some(c)) => wizard.choices = c,
            Ok(None) => {}
            Err(e) => error = Some(e),
        }
        let mut app = Self {
            wizard,
            inventory: None,
            prepared: None,
            review: None,
            completion: None,
            tx,
            rx,
            cancel: CancelToken::default(),
            busy: false,
            task: String::new(),
            progress: None,
            error,
            notice: None,
            search: String::new(),
            report_search: String::new(),
            custom_color: String::new(),
            why: None,
            preset_name: "My preset".into(),
            show_colors: false,
            mapping_saved: String::new(),
            mapping_actual: String::new(),
            ffmpeg: None,
            ffprobe: None,
            close_after_cancel: false,
            close_confirmation: false,
            logo,
        };
        if let Some(path) = replay {
            app.load_job(&path);
        }
        app
    }
    fn theme(ctx: &egui::Context) {
        let mut style = egui::Style::default();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(24, 26, 37);
        style.visuals.window_fill = Color32::from_rgb(33, 35, 49);
        style.visuals.extreme_bg_color = Color32::from_rgb(18, 20, 29);
        style.visuals.faint_bg_color = Color32::from_rgb(31, 34, 46);
        style.visuals.override_text_color = Some(Color32::from_rgb(235, 235, 244));
        style.visuals.selection.bg_fill = Color32::from_rgb(78, 61, 120);
        style.visuals.selection.stroke = egui::Stroke::new(1., ACCENT);
        style.visuals.hyperlink_color = ACCENT;
        style.spacing.item_spacing = egui::vec2(12., 10.);
        style.spacing.button_padding = egui::vec2(14., 9.);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(28.));
        ctx.set_theme(egui::Theme::Dark);
        ctx.set_style_of(egui::Theme::Dark, style);
    }
    fn spec(&self) -> JobSpec {
        let i = &self.wizard.inputs;
        let c = &self.wizard.choices;
        let mut p = AvidToPremiere::from_json_text(&c.policy_json).unwrap_or_default();
        p.bin_mode = match c.mode {
            ClipMode::Grouped => BinMode::Grouped,
            ClipMode::Merged => BinMode::Merged,
            _ => BinMode::Separate,
        };
        JobSpec {
            bins: i.bins.clone(),
            project_folder: i.project_folder.clone(),
            entire_project: i.entire_project,
            include_trash_attic: c.include_trash_attic,
            include_dependencies: c.include_dependencies,
            output: i.output.clone(),
            project_name: i.project_name.clone(),
            media_roots: i.media_roots.clone(),
            original_roots: i.original_roots.clone(),
            path_mappings: i
                .path_mappings
                .iter()
                .map(|(a, b)| job::PathMapping {
                    saved_prefix: a.clone(),
                    actual_root: b.clone(),
                })
                .collect(),
            mode: match c.mode {
                ClipMode::Rewrap => job::ClipMode::RewrapNative,
                ClipMode::Grouped => job::ClipMode::Grouped,
                ClipMode::Merged => job::ClipMode::Merged,
                ClipMode::Separate => job::ClipMode::Separate,
            },
            new_media: if c.mode == ClipMode::Rewrap {
                i.new_media.clone()
            } else {
                None
            },
            policy_json: p.to_json_text(),
            effects: self
                .wizard
                .choices
                .effects
                .iter()
                .filter_map(|(key, mode)| {
                    Some(job::EffectChoice {
                        key: key.clone(),
                        mode: match mode.as_str() {
                            "convert" => job::EffectMode::Convert,
                            "approximate" => job::EffectMode::Approximate,
                            "skip" => job::EffectMode::Skip,
                            _ => return None,
                        },
                    })
                })
                .collect(),
            ffmpeg: self.ffmpeg.clone(),
            ffprobe: self.ffprobe.clone(),
            ..Default::default()
        }
    }
    fn invalidate(&mut self) {
        self.prepared = None;
        self.review = None;
        self.completion = None;
    }
    fn start(&mut self, label: &str) {
        self.busy = true;
        self.task = label.into();
        self.error = None;
        self.notice = None;
        self.progress = None;
        self.cancel = CancelToken::default();
    }
    fn scan(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        let spec = self.spec();
        let revision = self.wizard.revision();
        self.start("Reading project inventory");
        let (tx, cancel, ctx) = (self.tx.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut progress = |p| {
                let _ = tx.try_send(Event::Progress(p));
                ctx.request_repaint();
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                job::inventory(&spec, &cancel, &mut progress)
            }))
            .unwrap_or_else(|_| {
                Err(
                    "The inventory worker stopped unexpectedly. No conversion output was created."
                        .into(),
                )
            });
            let _ = tx.send(Event::Inventory(revision, result));
            ctx.request_repaint();
        });
    }
    fn prepare(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        if let Err(e) = self.wizard.inputs.required_paths() {
            self.error = Some(e);
            return;
        }
        let spec = self.spec();
        let revision = self.wizard.revision();
        self.start("Checking conversion");
        self.prepared = None;
        self.review = None;
        let (tx, cancel, ctx) = (self.tx.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut progress = |p| {
                let _ = tx.try_send(Event::Progress(p));
                ctx.request_repaint();
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||job::prepare(spec,&cancel,&mut progress))).unwrap_or_else(|_|Err("The pre-flight worker stopped unexpectedly. Run the check again before converting.".into())).map(Box::new);
            let _ = tx.send(Event::Prepared(revision, result));
            ctx.request_repaint();
        });
    }
    fn convert(&mut self, ctx: &egui::Context) {
        if self.busy || !self.wizard.can_convert() {
            return;
        }
        let Some(prepared) = self.prepared.take() else {
            return;
        };
        self.start("Converting");
        let (tx, cancel, ctx) = (self.tx.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut progress = |p| {
                let _ = tx.try_send(Event::Progress(p));
                ctx.request_repaint();
            };
            let completion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                job::execute(prepared, &cancel, &mut progress)
            }));
            let event=match completion{Ok(c)=>Event::Finished(c),Err(_)=>Event::WorkerFailed("The conversion worker stopped unexpectedly. Completion is unknown; inspect the destination and any engine receipts before retrying. Existing files will not be replaced.".into())};
            let _ = tx.send(event);
            ctx.request_repaint();
        });
    }
    fn pick(&mut self, kind: Pick, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.start("Choosing a location");
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let d = rfd::FileDialog::new();
            let picked = match kind {
                Pick::Bins => d
                    .set_title("Choose Avid bins")
                    .add_filter("Avid bins", &["avb", "AVB"])
                    .pick_files(),
                Pick::Project | Pick::Media => d
                    .set_title("Choose a folder")
                    .pick_folder()
                    .map(|p| vec![p]),
                Pick::Output => d
                    .set_title("New Premiere project")
                    .add_filter("Premiere project", &["prproj"])
                    .set_file_name("Converted.prproj")
                    .save_file()
                    .map(|p| vec![p]),
                Pick::NewMedia => d
                    .set_title("Choose a parent for NEW media folder")
                    .pick_folder()
                    .map(|p| vec![p]),
                Pick::LoadPreset | Pick::LoadJob => d
                    .add_filter("Any NLE settings", &["json"])
                    .pick_file()
                    .map(|p| vec![p]),
                Pick::SaveJob => d
                    .set_title("Save a NEW job settings file")
                    .add_filter("Any NLE job", &["json"])
                    .set_file_name("Any NLE job.json")
                    .save_file()
                    .map(|p| vec![p]),
                Pick::SavePreset => d
                    .set_title("Save a NEW preset")
                    .add_filter("Any NLE preset", &["json"])
                    .set_file_name("Any NLE preset.json")
                    .save_file()
                    .map(|p| vec![p]),
                Pick::Ffmpeg | Pick::Ffprobe => d
                    .set_title("Choose the external media tool")
                    .pick_file()
                    .map(|p| vec![p]),
            };
            let _ = tx.send(Event::Picked(kind, picked));
            ctx.request_repaint();
        });
    }
    fn load_job(&mut self, path: &Path) {
        match storage::read_settings(path)
            .and_then(|text| serde_json::from_str::<JobSpec>(&text).map_err(|e| e.to_string()))
        {
            Ok(s) => {
                if s.schema != "anynle.job/1" {
                    self.error = Some("Unsupported job settings version.".into());
                    return;
                }
                if !s.policy_json.is_empty() {
                    if let Err(e) = AvidToPremiere::from_json_text(&s.policy_json) {
                        self.error = Some(format!("Invalid saved policy: {e}"));
                        return;
                    }
                }
                self.wizard.inputs = crate::model::Inputs {
                    project_folder: s.project_folder,
                    bins: s.bins,
                    entire_project: s.entire_project,
                    output: s.output,
                    project_name: s.project_name,
                    media_roots: s.media_roots,
                    original_roots: s.original_roots,
                    path_mappings: s
                        .path_mappings
                        .into_iter()
                        .map(|m| (m.saved_prefix, m.actual_root))
                        .collect(),
                    new_media: s.new_media,
                };
                self.wizard.choices = Choices {
                    mode: match s.mode {
                        job::ClipMode::RewrapNative => ClipMode::Rewrap,
                        job::ClipMode::Grouped => ClipMode::Grouped,
                        job::ClipMode::Merged => ClipMode::Merged,
                        job::ClipMode::Separate => ClipMode::Separate,
                    },
                    include_dependencies: s.include_dependencies,
                    include_trash_attic: s.include_trash_attic,
                    policy_json: if s.policy_json.is_empty() {
                        AvidToPremiere::default().to_json_text()
                    } else {
                        s.policy_json
                    },
                    effects: s
                        .effects
                        .into_iter()
                        .map(|e| {
                            (
                                e.key,
                                match e.mode {
                                    job::EffectMode::Convert => "convert",
                                    job::EffectMode::Approximate => "approximate",
                                    job::EffectMode::Skip => "skip",
                                }
                                .into(),
                            )
                        })
                        .collect(),
                };
                self.ffmpeg = s.ffmpeg;
                self.ffprobe = s.ffprobe;
                self.wizard.invalidate();
                self.invalidate();
                self.inventory = None;
                self.notice = Some(
                    "Job settings loaded. Check the paths, then run a fresh pre-flight.".into(),
                );
            }
            Err(e) => self.error = Some(format!("Couldn't load job: {e}")),
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        for _ in 0..64 {
            let Ok(event) = self.rx.try_recv() else {
                break;
            };
            match event {
                Event::WorkerFailed(message) => {
                    self.close_after_cancel = false;
                    self.busy = false;
                    self.wizard.invalidate();
                    self.prepared = None;
                    self.error = Some(message);
                }
                Event::Progress(p) => self.progress = Some(p),
                Event::Inventory(revision, result) => {
                    self.busy = false;
                    if self.cancel.is_cancelled() {
                        self.notice = Some("Cancelled; no project was published.".into());
                    }
                    match result {
                        Ok(i) if revision == self.wizard.revision() => {
                            self.inventory = Some(i);
                        }
                        Ok(_) => self.notice = Some("Settings changed; scan again.".into()),
                        Err(e) => self.error = Some(e),
                    }
                }
                Event::Prepared(revision, result) => {
                    self.busy = false;
                    if self.cancel.is_cancelled() {
                        self.notice = Some("Cancelled; no project was published.".into());
                    }
                    match result {
                        Ok(p) if self.wizard.accept_review(revision) => {
                            self.review = Some(p.review().clone());
                            self.inventory = Some(p.review().inventory.clone());
                            self.prepared = Some(*p);
                        }
                        Ok(_) => {
                            self.notice =
                                Some("Settings changed; check again before converting.".into())
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Event::Finished(c) => {
                    self.busy = false;
                    self.wizard.invalidate();
                    self.completion = Some(c);
                }
                Event::Picked(kind, paths) => {
                    self.busy = false;
                    let before = self.wizard.snapshot();
                    if let Some(paths) = paths {
                        if let Some(path) = paths.first() {
                            match kind {
                                Pick::Bins => {
                                    self.wizard.inputs.project_folder = None;
                                    self.wizard.inputs.entire_project = false;
                                    self.wizard.inputs.add_bins(paths.clone());
                                    self.inventory = None;
                                }
                                Pick::Project => {
                                    self.wizard.inputs.project_folder = Some(path.clone());
                                    self.wizard.inputs.entire_project = true;
                                    self.wizard.inputs.bins.clear();
                                    self.inventory = None;
                                }
                                Pick::Output => self.wizard.inputs.output = path.clone(),
                                Pick::Media => {
                                    if !self.wizard.inputs.media_roots.contains(path) {
                                        self.wizard.inputs.media_roots.push(path.clone());
                                    }
                                }
                                Pick::NewMedia => {
                                    self.wizard.inputs.new_media = Some(path.join(new_media_name()))
                                }
                                Pick::LoadPreset => match storage::read_settings(path)
                                    .and_then(|t| Preset::from_json(&t))
                                {
                                    Ok(p) => {
                                        self.wizard.apply_preset(p);
                                        self.invalidate();
                                    }
                                    Err(e) => self.error = Some(e),
                                },
                                Pick::SavePreset => {
                                    let result = Preset::new(
                                        self.preset_name.clone(),
                                        self.wizard.choices.clone(),
                                    )
                                    .to_json()
                                    .and_then(|t| storage::write_new(path, &t));
                                    match result {
                                        Ok(()) => {
                                            self.notice =
                                                Some(format!("Saved preset: {}", path.display()))
                                        }
                                        Err(e) => self.error = Some(e),
                                    }
                                }
                                Pick::SaveJob => {
                                    let result = if path
                                        .extension()
                                        .is_none_or(|x| !x.eq_ignore_ascii_case("json"))
                                    {
                                        Err("Choose a .json job settings file".into())
                                    } else {
                                        serde_json::to_string_pretty(&self.spec())
                                            .map_err(|e| e.to_string())
                                            .and_then(|text| {
                                                if text.len() > 1 << 20 {
                                                    Err("Job settings exceed 1 MiB".into())
                                                } else {
                                                    storage::write_new(path, &text)
                                                }
                                            })
                                    };
                                    match result {
                                        Ok(()) => {
                                            self.notice = Some(format!(
                                                "Saved job settings: {}",
                                                path.display()
                                            ))
                                        }
                                        Err(e) => self.error = Some(e),
                                    }
                                }
                                Pick::LoadJob => self.load_job(path),
                                Pick::Ffmpeg => {
                                    self.ffmpeg = Some(path.clone());
                                    self.wizard.invalidate();
                                    self.invalidate();
                                }
                                Pick::Ffprobe => {
                                    self.ffprobe = Some(path.clone());
                                    self.wizard.invalidate();
                                    self.invalidate();
                                }
                            }
                        }
                    }
                    if self.wizard.changed_since(&before) {
                        self.invalidate();
                        let _ = storage::save(&self.wizard.choices);
                    }
                }
            }
        }
        if self.close_after_cancel && !self.busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn why_button(&mut self, ui: &mut egui::Ui, title: &str) {
        if ui.link("Why?").clicked() {
            self.why = Some(title.into());
        }
    }
    fn title(ui: &mut egui::Ui, kicker: &str, title: &str, subtitle: &str) {
        ui.label(RichText::new(kicker).small().color(ACCENT));
        ui.heading(title);
        ui.label(RichText::new(subtitle).color(MUTED));
        ui.add_space(12.);
    }
    fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(31, 34, 47))
            .inner_margin(16.)
            .corner_radius(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                content(ui);
            });
    }
    fn direction(&mut self, ui: &mut egui::Ui) {
        Self::title(
            ui,
            "01 / PROJECT",
            "Bring your edit with you",
            "Choose your Avid bins and a new home for the Premiere project.",
        );
        Self::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("From");
                egui::ComboBox::from_id_salt("from")
                    .selected_text("Avid Media Composer")
                    .show_ui(ui, |ui| {
                        ui.selectable_label(true, "Avid Media Composer").clicked();
                    });
                ui.label(RichText::new(">").size(24.).color(ACCENT));
                ui.label("To");
                egui::ComboBox::from_id_salt("to")
                    .selected_text("Premiere Pro")
                    .show_ui(ui, |ui| {
                        ui.selectable_label(true, "Premiere Pro").clicked();
                    });
            });
            ui.small("Every editing app handles clips, audio and markers its own way. The next steps decide how Any NLE translates them.");
        });
        ui.add_space(8.);
        Self::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Source");
                if ui.button("Choose bins…").clicked() {
                    self.pick(Pick::Bins, ui.ctx());
                }
                if ui.button("Project folder…").clicked() {
                    self.pick(Pick::Project, ui.ctx());
                }
            });
            if let Some(path) = &self.wizard.inputs.project_folder {
                ui.label(path.display().to_string());
                ui.checkbox(
                    &mut self.wizard.inputs.entire_project,
                    "Import entire project",
                );
            } else {
                ui.label(
                    RichText::new(format!("{} bins selected", self.wizard.inputs.bins.len()))
                        .color(MUTED),
                );
            }
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut self.wizard.choices.include_trash_attic,
                    "Include Trash / Attic",
                );
                self.why_button(ui, "Project-folder import");
            });
            ui.checkbox(
                &mut self.wizard.choices.include_dependencies,
                "Also import clips these sequences use",
            );
            ui.horizontal(|ui| {
                if ui.button("Read bin tree & counts").clicked() {
                    self.scan(ui.ctx());
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search bins and folders")
                        .desired_width(260.),
                );
            });
            if let Some(inv) = &self.inventory {
                let tree = Tree::build(inv);
                let search = self.search.to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("bin-tree-scroll")
                    .max_height(220.)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!self.wizard.inputs.entire_project, |ui| {
                            tree.show(
                                ui,
                                inv,
                                &mut self.wizard.inputs.bins,
                                &search,
                                self.wizard.inputs.entire_project,
                                "root",
                            )
                        });
                    });
                ui.small(format!(
                    "{} bins available · {} selected",
                    inv.bins.len(),
                    if self.wizard.inputs.entire_project {
                        inv.bins.len()
                    } else {
                        self.wizard.inputs.bins.len()
                    }
                ));
            } else if !self.wizard.inputs.bins.is_empty() {
                let mut remove = None;
                egui::ScrollArea::vertical()
                    .max_height(160.)
                    .show(ui, |ui| {
                        for (i, p) in self.wizard.inputs.bins.iter().enumerate() {
                            ui.horizontal(|ui| {
                                if ui.small_button("×").clicked() {
                                    remove = Some(i);
                                }
                                ui.label(p.display().to_string());
                            });
                        }
                    });
                if let Some(i) = remove {
                    self.wizard.inputs.bins.remove(i);
                }
            }
        });
        ui.add_space(8.);
        Self::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Destination");
                if ui.button("New Premiere project…").clicked() {
                    self.pick(Pick::Output, ui.ctx());
                }
            });
            path_edit(
                ui,
                &mut self.wizard.inputs.output,
                "Choose a NEW .prproj file",
            );
            let mut name = self.wizard.inputs.project_name.clone().unwrap_or_default();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut name)
                        .hint_text("Project name (optional)")
                        .desired_width(320.),
                )
                .changed()
            {
                self.wizard.inputs.project_name = if name.trim().is_empty() {
                    None
                } else {
                    Some(name)
                };
            }

            ui.colored_label(GOOD, "Your Avid project is only read, never changed.");
            ui.small("Existing destination files and media folders are never replaced.");
        });
    }
    fn clips(&mut self, ui: &mut egui::Ui) {
        Self::title(
            ui,
            "02 / CLIPS & MEDIA",
            "Choose how clips arrive",
            "Your timeline follows the same construction as your bin clips.",
        );
        if let Some(inv) = &self.inventory {
            if inv.native_media == 0 && inv.ume_originals > 0 {
                Self::card(ui, |ui| {
                    ui.colored_label(GOOD, "Linked-original candidates found.");
                    ui.label("Pre-flight checks these original-file links. If all qualify, no new media folder is needed. Your native-media choice is remembered for other projects.");
                });
            }
        }
        for mode in ClipMode::ALL {
            let selected = self.wizard.choices.mode == mode;
            egui::Frame::group(ui.style())
                .fill(if selected {
                    Color32::from_rgb(52, 43, 76)
                } else {
                    Color32::from_rgb(31, 34, 47)
                })
                .stroke(egui::Stroke::new(
                    1.,
                    if selected {
                        ACCENT
                    } else {
                        Color32::from_rgb(59, 63, 80)
                    },
                ))
                .inner_margin(13.)
                .corner_radius(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut self.wizard.choices.mode,
                            mode,
                            RichText::new(mode.label()).strong(),
                        );
                        if mode == ClipMode::Rewrap {
                            ui.label(RichText::new("RECOMMENDED").small().color(ACCENT));
                        }
                    });
                    let (pro, con) = mode.explanation();
                    ui.label(pro);
                    ui.label(
                        RichText::new(con)
                            .small()
                            .color(if mode == ClipMode::Merged {
                                AMBER
                            } else {
                                MUTED
                            }),
                    );
                });
        }
        ui.horizontal(|ui| {
            ui.small(
                "Avid MultiGroups and qualified linked originals follow their supported routes.",
            );
            self.why_button(ui, "How clips are built");
        });
        let native_needed = self.inventory.as_ref().is_none_or(|i| i.native_media > 0);
        if self.wizard.choices.mode == ClipMode::Rewrap && native_needed {
            Self::card(ui, |ui| {
                ui.colored_label(AMBER, RichText::new("Creates new media.").strong());
                ui.label("Choose a parent folder. Any NLE proposes a new subfolder for stream-copied media.");
                ui.horizontal(|ui| {
                    if ui.button("Choose media parent…").clicked() {
                        self.pick(Pick::NewMedia, ui.ctx());
                    }
                    if let Some(p) = &mut self.wizard.inputs.new_media {
                        path_edit(ui, p, "New media folder");
                    }
                });
                if self.wizard.inputs.new_media.is_none() {
                    ui.small(
                        "A new destination is required if the scan finds native OP-Atom media.",
                    );
                }
            });
        }
        if self.wizard.choices.mode != ClipMode::Rewrap {
            ui.colored_label(AMBER,"Direct OP-Atom options have limited native qualification. ProRes OP-Atom has known Premiere import failures; rewrapping is recommended.");
        }
        egui::CollapsingHeader::new("Media locations & linking").default_open(true).show(ui,|ui|{
            ui.label("Media stays in place. Pre-flight checks file presence, identity and routing.");
            ui.horizontal(|ui|{if ui.button("Add Avid media folder…").clicked(){self.pick(Pick::Media,ui.ctx());}if ui.button("Refresh source references").clicked(){self.scan(ui.ctx());}});
            path_list(ui,"Avid media",&mut self.wizard.inputs.media_roots);
            if let Some(inv)=&self.inventory {ui.colored_label(if inv.found_media==inv.total_media{GOOD}else{AMBER},media_status(inv));}
            ui.small("Originals are selected only by their exact saved paths or explicit saved-path mappings. Proxy attachment and audio-to-source relinking have no additional supported setting in this release.");
            egui::CollapsingHeader::new("Saved-path mappings").show(ui,|ui|{
                ui.label("Map an original saved path prefix to its current root; filenames alone never establish identity.");
                ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut self.mapping_saved).hint_text("Saved prefix").desired_width(220.));ui.label(">");ui.add(egui::TextEdit::singleline(&mut self.mapping_actual).hint_text("Current root").desired_width(220.));if ui.button("Add").clicked(){if !self.mapping_saved.trim().is_empty()&&!self.mapping_actual.trim().is_empty(){self.wizard.inputs.path_mappings.push((PathBuf::from(self.mapping_saved.trim()),PathBuf::from(self.mapping_actual.trim())));self.mapping_saved.clear();self.mapping_actual.clear();}}});
                let mut remove=None;for(i,(a,b))in self.wizard.inputs.path_mappings.iter().enumerate(){ui.horizontal(|ui|{ui.label(format!("{} > {}",a.display(),b.display()));if ui.small_button("Remove").clicked(){remove=Some(i);}});}if let Some(i)=remove{self.wizard.inputs.path_mappings.remove(i);}
            });
        });
        let mut policy =
            AvidToPremiere::from_json_text(&self.wizard.choices.policy_json).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut policy.match_avid_default_pan,
                "Match Avid's default panning",
            );
            self.why_button(ui, "Pan");
        });
        self.wizard.choices.policy_json = policy.to_json_text();
        egui::CollapsingHeader::new("External media tools").show(ui,|ui|{ui.small("The packaged verified FFmpeg tools are used when available. Choose explicit tools for a local installation.");ui.horizontal(|ui|{if ui.button("FFmpeg…").clicked(){self.pick(Pick::Ffmpeg,ui.ctx());}ui.label(self.ffmpeg.as_ref().map_or("Bundled tools".into(),|p|p.display().to_string()));});ui.horizontal(|ui|{if ui.button("ffprobe…").clicked(){self.pick(Pick::Ffprobe,ui.ctx());}ui.label(self.ffprobe.as_ref().map_or("Bundled tools".into(),|p|p.display().to_string()));});});
    }
    fn markers(&mut self, ui: &mut egui::Ui) {
        Self::title(
            ui,
            "03 / MARKERS & COLORS",
            "Keep the useful details",
            "Choose marker conflict rules and map Avid colors to Premiere's palettes.",
        );
        let mut policy =
            AvidToPremiere::from_json_text(&self.wizard.choices.policy_json).unwrap_or_default();
        Self::card(ui, |ui| {
            ui.strong("When source and edit markers share a position");
            ui.horizontal(|ui| {
                ui.radio_value(&mut policy.source_marker_wins, false, "Edit marker wins");
                ui.radio_value(&mut policy.source_marker_wins, true, "Source marker wins");
                self.why_button(ui, "Where markers go");
            });
            ui.checkbox(
                &mut policy.conflict_loser_to_sequence,
                "Move the losing marker to the sequence",
            );
            ui.checkbox(
                &mut policy.marker_avid_info,
                "Append Avid user and original color to marker comments",
            );
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.show_colors, false, "Marker colors");
            ui.selectable_value(&mut self.show_colors, true, "Clip labels");
            self.why_button(
                ui,
                if self.show_colors {
                    "Clip color"
                } else {
                    "Marker color"
                },
            );
        });
        Self::card(ui, |ui| {
            if self.show_colors {
                palette::clips(ui, &mut policy, &mut self.custom_color, &mut self.error);
            } else {
                palette::markers(ui, &mut policy);
            }
        });
        if ui.button("Reset color mappings to recommended").clicked() {
            policy.marker_colors.clear();
            policy.clip_labels.clear();
        }
        self.wizard.choices.policy_json = policy.to_json_text();
    }
    fn effects(&mut self, ui: &mut egui::Ui) {
        Self::title(
            ui,
            "04 / EFFECTS",
            "Know what comes through",
            "Only translations supported by the shared engine are offered.",
        );
        if self.inventory.is_none() {
            ui.label("Read the project inventory to see the effect types present.");
            if ui.button("Read effect inventory").clicked() {
                self.scan(ui.ctx());
            }
            return;
        }
        let effects = self
            .inventory
            .as_ref()
            .map(|i| i.effects.clone())
            .unwrap_or_default();
        if effects.is_empty() {
            Self::card(ui, |ui| {
                ui.colored_label(GOOD, "No effect types reported in this selection.");
                ui.small("The final pre-flight still checks every retained source fact and reports unsupported details.");
            });
        }
        for e in effects {
            Self::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(&e.label);
                    ui.label(
                        RichText::new(format!("{} occurrences", e.count))
                            .small()
                            .color(MUTED),
                    );
                });
                ui.label(&e.explanation);
                ui.horizontal(|ui| {
                    let current = self
                        .wizard
                        .choices
                        .effects
                        .iter()
                        .find(|(key, _)| key == &e.key)
                        .map(|(_, mode)| mode.clone());
                    for mode in &e.modes {
                        let (key, text) = match mode {
                            job::EffectMode::Convert => ("convert", "Convert"),
                            job::EffectMode::Approximate => {
                                ("approximate", "Convert approximately · reported")
                            }
                            job::EffectMode::Skip => ("skip", "Skip · reported"),
                        };
                        if e.modes.len() > 1 {
                            let selected = current.as_deref().unwrap_or(match e.modes.first() {
                                Some(job::EffectMode::Convert) => "convert",
                                Some(job::EffectMode::Approximate) => "approximate",
                                _ => "skip",
                            }) == key;
                            if ui.radio(selected, text).clicked() {
                                self.wizard.choices.effects.retain(|(k, _)| k != &e.key);
                                self.wizard
                                    .choices
                                    .effects
                                    .push((e.key.clone(), key.into()));
                            }
                        } else {
                            ui.label(RichText::new(text).color(
                                if matches!(mode, job::EffectMode::Approximate) {
                                    AMBER
                                } else {
                                    MUTED
                                },
                            ));
                        }
                    }
                    self.why_button(ui, &e.label);
                });
            });
        }
        ui.small("Current effect handling is fixed by the qualified translation. Choices appear only when the engine supports alternatives; all approximations and skips are listed in Review.");
    }
    fn review(&mut self, ui: &mut egui::Ui) {
        if self.completion.is_some() {
            self.finish(ui);
            return;
        }
        Self::title(
            ui,
            "05 / REVIEW",
            "Ready when you are",
            "Check your choices and pre-flight notes before any output is created.",
        );
        Self::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Avid Media Composer");
                ui.label(">");
                ui.strong("Premiere Pro");
            });
            ui.label(format!("Project: {}", self.wizard.inputs.output.display()));
            ui.label(format!("Clips: {}", self.wizard.choices.mode.label()));
            ui.label(format!(
                "Selection: {} · dependencies {} · Trash / Attic {}",
                if self.wizard.inputs.entire_project {
                    "entire project".into()
                } else {
                    format!("{} bins", self.wizard.inputs.bins.len())
                },
                on_off(self.wizard.choices.include_dependencies),
                on_off(self.wizard.choices.include_trash_attic)
            ));
            if self.wizard.choices.mode == ClipMode::Rewrap
                && self.inventory.as_ref().is_none_or(|i| i.native_media > 0)
            {
                ui.colored_label(AMBER, "Creates new media.");
                ui.label(format!(
                    "New media: {}",
                    self.wizard
                        .inputs
                        .new_media
                        .as_ref()
                        .map_or("Choose a new folder on Clips & media".into(), |p| p
                            .display()
                            .to_string())
                ));
            }
            let p = AvidToPremiere::from_json_text(&self.wizard.choices.policy_json)
                .unwrap_or_default();
            ui.small(format!(
                "Marker winner: {} · losing marker to sequence {} · Avid info {} · default pan {}",
                if p.source_marker_wins {
                    "source"
                } else {
                    "edit"
                },
                on_off(p.conflict_loser_to_sequence),
                on_off(p.marker_avid_info),
                on_off(p.match_avid_default_pan)
            ));
        });
        ui.horizontal(|ui| {
            if ui
                .button(if self.review.is_some() {
                    "Check again"
                } else {
                    "Run pre-flight check"
                })
                .clicked()
            {
                self.prepare(ui.ctx());
            }
            ui.label(
                RichText::new("Reads and prepares only. Creates no output.")
                    .small()
                    .color(MUTED),
            );
        });
        if let Some(review) = self.review.clone() {
            Self::card(ui, |ui| {
                ui.colored_label(GOOD, RichText::new("Pre-flight complete").strong());
                ui.label(format!(
                    "{} bins · {} of {} media found · {} qualified UME originals",
                    review.inventory.bins.len(),
                    review.inventory.found_media,
                    review.inventory.total_media,
                    review.inventory.ume_originals
                ));
                if self.wizard.choices.mode == ClipMode::Rewrap && review.inventory.native_media > 0
                {
                    ui.label(format!(
                        "Estimated new media: {} · available: {}",
                        bytes(review.estimated_bytes),
                        bytes(review.available_bytes)
                    ));
                }
                for note in &review.inventory.notes {
                    ui.label(RichText::new(note).small().color(MUTED));
                }
            });
            self.report(ui, &review.notes);
            egui::CollapsingHeader::new("Replay & command-line equivalent").show(ui,|ui|{
                ui.label("The shared engine saves the exact job settings and completion receipt beside the new project. Changing inputs requires a fresh check.");
                ui.add(egui::TextEdit::multiline(&mut review.cli_equivalent.clone()).font(egui::TextStyle::Monospace).desired_rows(3).interactive(false));
                if ui.add_enabled(!review.cli_equivalent.is_empty(),egui::Button::new("Copy CLI command")).clicked(){ui.ctx().copy_text(review.cli_equivalent.clone());}if review.cli_equivalent.is_empty(){ui.label("This job is too large for a portable command. Save job settings and run any-nle run --job SETTINGS.json instead.");}
            });
        } else {
            ui.label(RichText::new("Run pre-flight to see missing media, translation losses, effect handling and disk-space review.").color(MUTED));
        }
        ui.small("Native application compatibility remains limited to the qualified profiles. The report records unsupported details and approximations.");
    }
    fn report(&mut self, ui: &mut egui::Ui, notes: &[job::ReportNote]) {
        ui.horizontal(|ui| {
            ui.strong(format!("Report · {} notes", notes.len()));
            ui.add(
                egui::TextEdit::singleline(&mut self.report_search)
                    .hint_text("Search report")
                    .desired_width(260.),
            );
        });
        let query = self.report_search.to_lowercase();
        let mut visible = 0;
        for n in notes {
            if !format!("{} {} {}", n.severity, n.object, n.message)
                .to_lowercase()
                .contains(&query)
            {
                continue;
            }
            visible += 1;
            Self::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&n.severity).small().color(
                        match n.severity.to_lowercase().as_str() {
                            "error" | "failure" => Color32::LIGHT_RED,
                            "loss" | "warning" => AMBER,
                            _ => MUTED,
                        },
                    ));
                    ui.strong(&n.object);
                    if ui.link("Why?").clicked() {
                        self.why = Some(if n.why.is_empty() {
                            n.message.clone()
                        } else {
                            n.why.clone()
                        });
                    }
                });
                ui.label(&n.message);
            });
        }
        if visible == 0 {
            ui.label(if notes.is_empty() {
                "No translation notes reported."
            } else {
                "No notes match your search."
            });
        }
    }
    fn finish(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.completion.clone() else {
            return;
        };
        let (title, color) = match c.status {
            CompletionStatus::Complete => ("Your project is ready", GOOD),
            CompletionStatus::Cancelled => ("Conversion cancelled", AMBER),
            CompletionStatus::Failed => ("Conversion stopped", Color32::LIGHT_RED),
        };
        Self::title(
            ui,
            "CONVERSION REPORT",
            title,
            "Review the actual result and any files retained by the engine.",
        );
        Self::card(ui, |ui| {
            ui.colored_label(color, &c.message);
            if let Some(path) = &c.output {
                ui.label(path.display().to_string());
                ui.horizontal(|ui| {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if let Some(app) = premiere_app() {
                        if ui.button("Open in Premiere").clicked() {
                            if let Err(e) = open_premiere(&app, path) {
                                self.error = Some(e);
                            }
                        }
                    }
                    if ui
                        .button(if cfg!(target_os = "macos") {
                            "Show in Finder"
                        } else {
                            "Open folder"
                        })
                        .clicked()
                    {
                        if let Some(parent) = path.parent() {
                            if let Err(e) = open_folder(parent) {
                                self.error = Some(e);
                            }
                        }
                    }
                });
            }
            #[cfg(target_os="linux")]
            ui.small("Premiere Pro is unavailable on Linux. Transfer the project and linked media to a supported editing system.");
            if let Some(receipt) = &c.receipt {
                ui.label(format!("Receipt: {}", receipt.display()));
            }
            if !c.retained_paths.is_empty() {
                ui.strong("Retained files");
                for path in &c.retained_paths {
                    ui.horizontal(|ui| {
                        ui.label(path.display().to_string());
                        if ui.small_button("Open folder").clicked() {
                            let folder = if path.is_dir() {
                                path.as_path()
                            } else {
                                path.parent().unwrap_or(path)
                            };
                            if let Err(e) = open_folder(folder) {
                                self.error = Some(e);
                            }
                        }
                    });
                }
            }
        });
        self.report(ui, &c.report);
        if ui.button("Convert another project").clicked() {
            self.completion = None;
            self.review = None;
            self.prepared = None;
            self.wizard.page = Page::Direction;
            self.wizard.invalidate();
        }
    }
    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui|{
            if self.wizard.page!=Page::Direction&&self.completion.is_none()&&ui.add_enabled(!self.busy,egui::Button::new("< Back")).clicked(){self.wizard.page=self.wizard.page.back();}
            if self.busy {
                ui.spinner();ui.label(&self.task);
                if !self.task.starts_with("Choosing")&&ui.button("Cancel").clicked(){self.cancel.cancel();self.notice=Some("Cancellation requested. Waiting for the current safe boundary; any retained files will be reported.".into());}
            }else if self.completion.is_none(){
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{
                    if self.wizard.page==Page::Review {
                        if ui.add_enabled(self.wizard.can_convert()&&self.prepared.is_some(),egui::Button::new(RichText::new("Convert >").strong()).fill(Color32::from_rgb(87,66,135))).clicked(){self.convert(ui.ctx());}
                    }else if ui.button("Next >").clicked(){
                        match self.wizard.inputs.required_paths(){Ok(())=>{self.wizard.page=self.wizard.page.next();if self.wizard.page==Page::Review{self.prepare(ui.ctx());}},Err(e)=>self.error=Some(e)}
                    }
                    if self.wizard.page!=Page::Review&&ui.link("Use recommended settings").clicked(){
                        match self.wizard.inputs.required_paths(){Ok(())=>{self.wizard.recommended();if let Err(e)=storage::save(&self.wizard.choices){self.error=Some(e);}self.invalidate();self.prepare(ui.ctx());},Err(e)=>self.error=Some(e)}
                    }
                });
            }
        });
        if let Some(p) = &self.progress {
            let f = if p.total == 0 {
                0.
            } else {
                (p.completed as f32 / p.total as f32).clamp(0., 1.)
            };
            ui.add(
                egui::ProgressBar::new(f)
                    .animate(self.busy)
                    .text(format!("{} · {} / {}", p.stage, p.completed, p.total)),
            );
            if let Some(bin) = &p.current_bin {
                ui.small(bin.display().to_string());
            }
            if !p.message.is_empty() {
                ui.small(&p.message);
            }
        }
    }
    fn help_window(&mut self, ctx: &egui::Context) {
        if let Some(title) = self.why.clone() {
            let mut open = true;
            egui::Window::new(format!("Why? · {title}")).open(&mut open).default_width(640.).vscroll(true).show(ctx,|ui|{
                let key=title.to_lowercase();
                let section=WHY.split("\n### ").find(|s|s.lines().next().is_some_and(|h|h.to_lowercase().contains(&key)));
                if let Some(section)=section{for line in section.lines(){if line.starts_with("## "){break;}ui.label(line);}}
                else{ui.label(&title);ui.separator();ui.label("Any NLE uses the qualified translation for Avid > Premiere. Source facts, unsupported details and approximations are kept in the conversion report. Only current supported choices are offered.");}
            });
            if !open {
                self.why = None;
            }
        }
        if self.close_confirmation {
            egui::Window::new("Conversion in progress").collapsible(false).resizable(false).show(ctx,|ui|{
                ui.label("Cancel the current job before closing? Any retained output will be recorded by the engine.");
                ui.horizontal(|ui|{if ui.button("Keep working").clicked(){self.close_confirmation=false;}if ui.button("Cancel job & close when safe").clicked(){self.cancel.cancel();self.close_after_cancel=true;self.close_confirmation=false;}});
            });
        }
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if self.busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && self.busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.close_after_cancel {
                self.close_confirmation = true;
            }
        }
        self.poll(&ctx);
        egui::Panel::top("brand")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(20, 21, 31))
                    .inner_margin(16.),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&self.logo)
                            .fit_to_exact_size(egui::vec2(44., 44.))
                            .corner_radius(8),
                    );
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Any NLE").size(23.).strong());
                        ui.label(RichText::new("Your edit. A new home.").small().color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("ALPHA  {}", env!("CARGO_PKG_VERSION")))
                                .small()
                                .color(ACCENT),
                        );
                    });
                });
            });
        egui::Panel::bottom("navigation")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(20, 21, 31))
                    .inner_margin(16.),
            )
            .show(ui, |ui| self.navigation(ui));
        egui::Panel::left("steps")
            .resizable(false)
            .exact_size(210.)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(25, 27, 39))
                    .inner_margin(18.),
            )
            .show(ui, |ui| {
                ui.add_space(12.);
                for p in Page::ALL {
                    let active = self.wizard.page == p;
                    let text = RichText::new(format!("{:02}   {}", p.index() + 1, p.label()))
                        .color(if active { ACCENT } else { MUTED });
                    if ui
                        .add_enabled(
                            !self.busy,
                            egui::Button::new(text)
                                .selected(active)
                                .min_size(egui::vec2(174., 42.)),
                        )
                        .clicked()
                    {
                        if p == Page::Direction || self.wizard.inputs.required_paths().is_ok() {
                            self.wizard.page = p;
                        } else {
                            self.error = self.wizard.inputs.required_paths().err();
                        }
                    }
                }
                ui.add_space(24.);
                ui.separator();
                ui.label(RichText::new("YOUR SETTINGS").small().color(MUTED));
                ui.add_enabled_ui(!self.busy, |ui| {
                    if ui.button("Load preset…").clicked() {
                        self.pick(Pick::LoadPreset, ui.ctx());
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.preset_name)
                            .hint_text("Preset name")
                            .desired_width(160.),
                    );
                    if ui.button("Save preset…").clicked() {
                        self.pick(Pick::SavePreset, ui.ctx());
                    }
                    if ui.button("Save job settings…").clicked() {
                        self.pick(Pick::SaveJob, ui.ctx());
                    }
                    if ui.button("Load saved job…").clicked() {
                        self.pick(Pick::LoadJob, ui.ctx());
                    }
                });
                ui.add_space(14.);
                ui.label(
                    RichText::new("Choices remembered locally.\nNo telemetry. No updater.")
                        .small()
                        .color(MUTED),
                );
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(24, 26, 37))
                    .inner_margin(24.),
            )
            .show(ui, |ui| {
                if let Some(error) = self.error.clone() {
                    egui::Frame::group(ui.style())
                        .fill(Color32::from_rgb(63, 35, 43))
                        .inner_margin(12.)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(Color32::LIGHT_RED, &error);
                                if ui.small_button("Dismiss").clicked() {
                                    self.error = None;
                                }
                            });
                        });
                }
                if let Some(notice) = &self.notice {
                    ui.label(RichText::new(notice).color(AMBER));
                }
                let before = self.wizard.snapshot();
                egui::ScrollArea::vertical()
                    .id_salt("page-scroll")
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!self.busy, |ui| match self.wizard.page {
                            Page::Direction => self.direction(ui),
                            Page::Clips => self.clips(ui),
                            Page::Markers => self.markers(ui),
                            Page::Effects => self.effects(ui),
                            Page::Review => self.review(ui),
                        });
                    });
                if self.wizard.changed_since(&before) {
                    self.invalidate();
                    if let Err(e) = storage::save(&self.wizard.choices) {
                        self.error = Some(e);
                    }
                }
            });
        self.help_window(&ctx);
    }
}
fn path_edit(ui: &mut egui::Ui, path: &mut PathBuf, hint: &str) {
    let mut text = path.to_string_lossy().into_owned();
    if ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(hint)
                .desired_width(ui.available_width().max(120.)),
        )
        .changed()
    {
        *path = PathBuf::from(text);
    }
}
fn path_list(ui: &mut egui::Ui, label: &str, paths: &mut Vec<PathBuf>) {
    let mut remove = None;
    for (i, p) in paths.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).small().color(MUTED));
            ui.label(p.display().to_string());
            if ui.small_button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        paths.remove(i);
    }
}
fn bytes(value: Option<u64>) -> String {
    match value {
        None => "not available".into(),
        Some(v) if v >= 1_073_741_824 => format!("{:.1} GiB", v as f64 / 1_073_741_824.),
        Some(v) if v >= 1_048_576 => format!("{:.1} MiB", v as f64 / 1_048_576.),
        Some(v) => format!("{v} bytes"),
    }
}
fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}
fn new_media_name() -> String {
    format!(
        "Any NLE Media {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    )
}
fn open_folder(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe").arg(path).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();
    result
        .map(|_| ())
        .map_err(|e| format!("Couldn't open folder: {e}"))
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn premiere_app() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let root = PathBuf::from("/Applications");
    #[cfg(target_os = "windows")]
    let root = PathBuf::from(std::env::var_os("ProgramFiles")?).join("Adobe");
    let mut dirs = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("Adobe Premiere Pro"))
        })
        .collect::<Vec<_>>();
    dirs.sort();
    #[cfg(target_os = "macos")]
    {
        for p in dirs.into_iter().rev() {
            if p.extension().is_some_and(|e| e == "app") {
                return Some(p);
            }
            if let Ok(entries) = std::fs::read_dir(&p) {
                if let Some(app) = entries.filter_map(Result::ok).map(|e| e.path()).find(|p| {
                    p.extension().is_some_and(|e| e == "app")
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("Adobe Premiere Pro"))
                }) {
                    return Some(app);
                }
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        dirs.into_iter()
            .rev()
            .map(|p| p.join("Adobe Premiere Pro.exe"))
            .find(|p| p.is_file())
    }
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn open_premiere(app: &Path, path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-a")
        .arg(app)
        .arg(path)
        .spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new(app).arg(path).spawn();
    result
        .map(|_| ())
        .map_err(|e| format!("Couldn't open Premiere: {e}"))
}

fn media_status(inv: &Inventory) -> String {
    if inv
        .notes
        .iter()
        .any(|n| n == "Media presence is not checked by the inventory; run pre-flight.")
    {
        format!(
            "Last scan: {} media references · {} native candidates · {} linked-original candidates · presence not checked",
            inv.total_media, inv.native_media, inv.ume_originals
        )
    } else {
        format!(
            "Last pre-flight: {} of {} media files found · {} native · {} qualified original links",
            inv.found_media, inv.total_media, inv.native_media, inv.ume_originals
        )
    }
}
