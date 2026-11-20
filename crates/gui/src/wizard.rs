use crate::model::{Choices, ClipMode, Page, Preset, RequiredInput, Snapshot, Wizard};
use crate::report::{Filter, PAGE_SIZE, ReportBrowser};
use crate::{direction, icons::Glyph, palette, storage};
use anynle_app::job::{
    self, BinPath, BlockerKind, CancelToken, Completion, CompletionStatus, Inventory, JobSpec, PreparedJob,
    Progress, ReviewBlocker,
};
use eframe::egui::{self, Color32, RichText};
use mapping::{AvidToPremiere, BinMode};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

#[cfg(feature = "wizard-automation")]
#[path = "automation.rs"]
mod automation;
#[cfg(feature = "wizard-automation")]
pub(super) use automation::run as run_automation;

const CANVAS: Color32 = Color32::from_rgb(27, 28, 39);
const RAIL: Color32 = Color32::from_rgb(37, 37, 52);
const SURFACE: Color32 = Color32::from_rgb(40, 41, 56);
const CONTROL: Color32 = Color32::from_rgb(52, 53, 71);
const BORDER: Color32 = Color32::from_rgb(60, 61, 80);
const INK: Color32 = Color32::from_rgb(241, 239, 251);
const MUTED: Color32 = Color32::from_rgb(176, 175, 195);
const ACCENT: Color32 = Color32::from_rgb(196, 186, 255);
const SELECTED: Color32 = Color32::from_rgb(59, 54, 89);
const PRIMARY: Color32 = Color32::from_rgb(201, 190, 255);
const PRIMARY_INK: Color32 = Color32::from_rgb(35, 29, 69);
const GOOD: Color32 = Color32::from_rgb(133, 222, 190);
const AMBER: Color32 = Color32::from_rgb(244, 199, 137);
const ERROR_SURFACE: Color32 = Color32::from_rgb(61, 41, 54);
const ERROR_INK: Color32 = Color32::from_rgb(255, 187, 201);
const SOURCE_LIST_HEIGHT: f32 = 196.;
const PROJECT_TAIL_SPACE: f32 = 156.;
const CONVERTING: &str = "Converting project";
const SAVING_PREFLIGHT: &str = "Saving preflight report";
const WHY: &str = include_str!("../../../docs/DECISIONS-EXPLAINED.md");
#[derive(Clone, Copy)]
enum Pick {
    Bins,
    Project,
    Output,
    Media,
    MappedOriginalFolder,
    NewMedia,
    RewrapNeeded,
    ProxyMedia,
    LoadPreset,
    SavePreset,
    SaveJob,
    SavePreflightReport,
    LoadJob,
    Ffmpeg,
    Ffprobe,
}
enum Event {
    Progress(Progress),
    Catalog(u64, Result<Vec<BinPath>, String>),
    Inventory(u64, Result<Inventory, String>),
    Prepared(u64, Result<Box<PreparedJob>, ReviewBlocker>),
    PreflightReportSaved(PreflightReportSource, PathBuf, Result<(), String>),
    Finished(Completion),
    Picked(Pick, Option<Vec<PathBuf>>),
    WorkerFailed(String),
}
enum PreflightReportSource {
    Ready(Box<PreparedJob>),
    Blocked(ReviewBlocker),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaJump {
    Locations,
    OriginalMappings,
    Tools,
}
#[derive(Clone, Copy)]
enum ReviewAction {
    Step(Page),
    Choose(Pick),
    Media(MediaJump),
    SwitchToSeparate { enable_offline_media: bool },
    Recheck,
}
#[derive(Default)]
struct Tree {
    relative: PathBuf,
    folders: BTreeMap<OsString, Tree>,
    files: Vec<usize>,
}

#[derive(Default)]
struct FolderExpansion {
    browse: BTreeMap<PathBuf, bool>,
    search: BTreeMap<PathBuf, bool>,
}

impl FolderExpansion {
    fn is_open(&self, path: &Path, bin_count: usize, filtering: bool) -> bool {
        let state = if filtering { &self.search } else { &self.browse };
        state.get(path).copied().unwrap_or(filtering || bin_count <= 16)
    }

    fn set_open(&mut self, path: &Path, filtering: bool, open: bool) {
        let state = if filtering { &mut self.search } else { &mut self.browse };
        state.insert(path.to_path_buf(), open);
    }
}

struct BinSelection<'a> {
    paths: &'a mut Vec<PathBuf>,
    checked: HashSet<PathBuf>,
}

impl<'a> BinSelection<'a> {
    fn new(paths: &'a mut Vec<PathBuf>) -> Self {
        let checked = paths.iter().cloned().collect();
        Self { paths, checked }
    }

    fn set<'p>(&mut self, bins: impl Iterator<Item = &'p PathBuf>, checked: bool) {
        if checked {
            for bin in bins {
                if self.checked.insert(bin.clone()) {
                    self.paths.push(bin.clone());
                }
            }
        } else {
            for bin in bins {
                self.checked.remove(bin);
            }
            self.paths.retain(|path| self.checked.contains(path));
        }
    }
}

impl Tree {
    fn build(bins: &[BinPath]) -> Self {
        let mut tree = Self::default();
        for (i, bin) in bins.iter().enumerate() {
            let relative = &bin.relative;
            let mut node = &mut tree;
            if let Some(parent) = relative.parent() {
                for part in parent.components() {
                    let name = part.as_os_str().to_owned();
                    node = node.folders.entry(name.clone()).or_insert_with(|| Self {
                        relative: node.relative.join(name),
                        ..Default::default()
                    });
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
        catalog: &[BinPath],
        selected: &mut BinSelection<'_>,
        matches: Option<&[bool]>,
        entire: bool,
        expansion: &mut FolderExpansion,
    ) {
        for (name, node) in &self.folders {
            let indices = node.indices();
            if matches.is_some_and(|matches| !indices.iter().any(|&i| matches[i])) {
                continue;
            }
            let filtering = matches.is_some();
            let id = ui.make_persistent_id(("bin-folder", name, filtering));
            let open = expansion.is_open(&node.relative, catalog.len(), filtering);
            let mut state =
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open);
            state.set_open(open);
            let header = state
                .show_header(ui, |ui| {
                    let bins = || indices.iter().map(|&i| &catalog[i].path);
                    let mut all = entire || bins().all(|path| selected.checked.contains(path));
                    let partial = !all && bins().any(|path| selected.checked.contains(path));
                    let checkbox = egui::Checkbox::new(&mut all, name.to_string_lossy())
                        .indeterminate(partial);
                    if ui
                        .add_enabled(!entire, checkbox)
                        .on_hover_text("Select every bin in this folder and its subfolders")
                        .changed()
                    {
                        selected.set(bins(), all);
                    }
                    ui.label(
                        RichText::new(counted(indices.len(), "bin", "bins"))
                            .small()
                            .color(MUTED),
                    );
                });
            let now_open = header.is_open();
            if now_open != open {
                expansion.set_open(&node.relative, filtering, now_open);
            }
            let (toggle, _, _) = header.body(|ui| {
                node.show(ui, catalog, selected, matches, entire, expansion);
            });
            let action = if now_open { "Collapse" } else { "Expand" };
            let label = format!("{action} {}", name.to_string_lossy());
            toggle.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            toggle.on_hover_text(label);
        }
        for i in &self.files {
            let bin = &catalog[*i];
            if matches.is_some_and(|matches| !matches[*i]) {
                continue;
            }
            ui.push_id(&bin.path, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().indent);
                    let mut value = entire || selected.checked.contains(&bin.path);
                    if ui
                        .add_enabled(
                            !entire,
                            egui::Checkbox::new(&mut value, path_name(&bin.path)),
                        )
                        .on_hover_text(bin.path.display().to_string())
                        .changed()
                    {
                        selected.set(std::iter::once(&bin.path), value);
                    }
                });
            });
        }
    }
}

#[derive(Default)]
struct Activity {
    errors: VecDeque<String>,
    unread: bool,
    open: bool,
    critical: bool,
}

impl Activity {
    fn record(&mut self, error: String, critical: bool) {
        if self.errors.back() != Some(&error) {
            if self.errors.len() == 64 {
                self.errors.pop_front();
            }
            self.errors.push_back(error);
        }
        self.unread = !self.open;
        self.critical |= critical;
    }

    fn button(&mut self, ui: &mut egui::Ui) {
        let color = if self.unread { ERROR_INK } else { MUTED };
        let icon = Glyph::Activity.image(ui, 15., color);
        let text = if self.unread { "Activity · Error" } else { "Activity" };
        let mut button = egui::Button::image_and_text(icon, RichText::new(text).color(color))
            .right_text("")
            .min_size(egui::vec2(ui.available_width(), 31.))
            .corner_radius(8)
            .frame_when_inactive(false);
        if self.unread {
            button = button.fill(ERROR_SURFACE).frame_when_inactive(true);
        }
        if ui.add(button).clicked() {
            self.open = true;
            self.unread = false;
        }
    }

    fn show(&mut self, ctx: &egui::Context) {
        egui::Window::new("Activity")
            .open(&mut self.open)
            .default_width(480.)
            .show(ctx, |ui| {
                if self.errors.is_empty() {
                    ui.label("No errors in this session.");
                    return;
                }
                if ui.button("Copy error log").clicked() {
                    ctx.copy_text(self.errors.iter().rev().map(String::as_str).collect::<Vec<_>>().join("\n\n"));
                }
                egui::ScrollArea::vertical().max_height(380.).show(ui, |ui| {
                    for error in self.errors.iter().rev() {
                        ui.add(egui::Label::new(error).wrap().selectable(true));
                        ui.add_space(8.);
                    }
                });
            });
        if self.critical {
            let response = egui::Modal::new(egui::Id::unique("critical-conversion-error"))
                .backdrop_color(Color32::from_black_alpha(150))
                .show(ctx, |ui| {
                    ui.set_min_width(285.);
                    ui.label(RichText::new("Conversion stopped").font(semibold(19.)));
                    ui.label("See Activity for the error details.");
                    ui.add_space(6.);
                    ui.horizontal(|ui| {
                        (ui.button("Open Activity").clicked(), ui.button("Close").clicked())
                    }).inner
                });
            let (open, close) = response.inner;
            if open {
                self.open = true;
                self.unread = false;
                ctx.request_repaint();
            }
            self.critical = !(open || close || response.should_close());
        }
    }
}

fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("Inter Semibold".into()))
}

pub struct App {
    wizard: Wizard,
    remember_choices: bool,
    catalog: Option<Vec<BinPath>>,
    expansion: FolderExpansion,
    #[cfg(feature = "wizard-automation")]
    source_viewport: Option<egui::Rect>,
    inventory: Option<Inventory>,
    prepared: Option<PreparedJob>,
    rewrap_size: crate::rewrap_size::SizeControl,
    review_blocker: Option<ReviewBlocker>,
    media_jump: Option<MediaJump>,
    completion: Option<Completion>,
    completion_subjects: HashMap<String, String>,
    tx: SyncSender<Event>,
    rx: Receiver<Event>,
    cancel: CancelToken,
    busy: bool,
    task: String,
    progress: Option<Progress>,
    error: Option<String>,
    activity: Activity,
    highlight_required_input: bool,
    notice: Option<String>,
    search: String,
    report: ReportBrowser,
    custom_color: String,
    why: Option<String>,
    preset_name: String,
    show_colors: bool,
    mapping_saved: String,
    mapping_actual: PathBuf,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    close_when_idle: bool,
    close_confirmation: bool,
    #[cfg(target_os = "macos")]
    relink: crate::relink::Relink,
    #[cfg(target_os = "macos")]
    premiere_status: crate::premiere_review::WizardStatus,
    #[cfg(target_os = "macos")]
    resolve_wizard: Option<crate::avid_resolve::Wizard>,
    #[cfg(target_os = "macos")]
    resolve_templates: Option<anynle_resolve_adapter::turnover::Templates>,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, replay: Option<PathBuf>) -> Self {
        let mut app = Self::session(&cc.egui_ctx);
        app.remember_choices = true;
        match storage::load() {
            Ok(Some(c)) => app.wizard.choices = c,
            Ok(None) => {}
            Err(e) => app.error = Some(e),
        }
        #[cfg(target_os = "macos")]
        {
            app.resolve_templates =
                anynle_app::resolve_job::local_templates().unwrap_or_else(|error| {
                    eprintln!("Resolve templates: {error}");
                    None
                });
        }
        if let Some(path) = replay {
            app.load_job(&path);
        }
        app
    }
    fn session(ctx: &egui::Context) -> Self {
        Self::theme(ctx);
        let (tx, rx) = mpsc::sync_channel(32);
        Self {
            wizard: Wizard::default(),
            remember_choices: false,
            catalog: None,
            expansion: FolderExpansion::default(),
            #[cfg(feature = "wizard-automation")]
            source_viewport: None,
            inventory: None,
            prepared: None,
            rewrap_size: Default::default(),
            #[cfg(target_os = "macos")]
            resolve_wizard: None,
            #[cfg(target_os = "macos")]
            resolve_templates: None,
            review_blocker: None,
            media_jump: None,
            completion: None,
            completion_subjects: HashMap::new(),
            tx,
            rx,
            cancel: CancelToken::default(),
            busy: false,
            task: String::new(),
            progress: None,
            error: None,
            activity: Default::default(),
            highlight_required_input: false,
            notice: None,
            search: String::new(),
            report: ReportBrowser::default(),
            custom_color: String::new(),
            why: None,
            preset_name: "My preset".into(),
            show_colors: false,
            mapping_saved: String::new(),
            mapping_actual: PathBuf::new(),
            ffmpeg: None,
            ffprobe: None,
            close_when_idle: false,
            close_confirmation: false,
            #[cfg(target_os = "macos")]
            relink: Default::default(),
            #[cfg(target_os = "macos")]
            premiere_status: Default::default(),
        }
    }
    fn theme(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        for (name, data) in [
            ("Inter", include_bytes!("../resources/inter/Inter-Regular.ttf").as_slice()),
            ("Inter Semibold", include_bytes!("../resources/inter/Inter-SemiBold.ttf").as_slice()),
        ] {
            fonts.font_data.insert(name.into(), egui::FontData::from_static(data).into());
        }
        let regular = fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap();
        regular.insert(0, "Inter".into());
        let mut bold = regular.clone();
        bold[0] = "Inter Semibold".into();
        fonts.families.insert(semibold(13.).family, bold);
        ctx.set_fonts(fonts);
        let mut style = egui::Style::default();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = CANVAS;
        style.visuals.window_fill = SURFACE;
        style.visuals.window_stroke = egui::Stroke::new(1., BORDER);
        style.visuals.extreme_bg_color = Color32::from_rgb(22, 23, 33);
        style.visuals.faint_bg_color = SURFACE;
        style.visuals.selection.bg_fill = SELECTED;
        style.visuals.selection.stroke = egui::Stroke::new(1., ACCENT);
        style.visuals.hyperlink_color = ACCENT;
        let widgets = &mut style.visuals.widgets;
        widgets.noninteractive.fg_stroke.color = INK;
        widgets.noninteractive.bg_stroke.color = BORDER;
        widgets.inactive.weak_bg_fill = CONTROL;
        widgets.inactive.bg_fill = CONTROL;
        widgets.inactive.bg_stroke = egui::Stroke::NONE;
        widgets.inactive.fg_stroke.color = INK;
        widgets.hovered.weak_bg_fill = BORDER;
        widgets.hovered.bg_fill = BORDER;
        widgets.hovered.bg_stroke.color = MUTED;
        widgets.active.weak_bg_fill = SELECTED;
        widgets.active.bg_fill = SELECTED;
        widgets.active.bg_stroke.color = ACCENT;
        style.spacing.item_spacing = egui::vec2(8., 7.);
        style.spacing.button_padding = egui::vec2(8., 4.);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, semibold(30.));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(11.));
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
            allow_offline_media: c.mode != ClipMode::Rewrap && c.allow_offline_media,
            skip_unused_bin_items: false,
            prefer_source_media: c.uses_camera_originals(),
            attach_avid_proxies: c.attaches_avid_proxies(),
            proxy_rewrap_dir: if c.attaches_avid_proxies() {
                i.proxy_rewrap_dir.clone()
            } else { None },
            new_media: if c.mode == ClipMode::Rewrap {
                i.new_media.clone()
            } else {
                None
            },
            rewrap_needed: if c.mode == ClipMode::Rewrap { None } else { i.rewrap_needed.clone() },
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
        self.rewrap_size = Default::default();
        self.review_blocker = None;
        self.completion = None;
        self.completion_subjects = HashMap::new();
        self.report = ReportBrowser::default();
    }
    fn apply_settings_changes(&mut self, before: &Snapshot) -> bool {
        if !self.wizard.changed_since(before) {
            return false;
        }
        self.invalidate();
        if before.choices.include_trash_attic != self.wizard.choices.include_trash_attic {
            self.catalog = None;
        }
        true
    }
    fn start(&mut self, label: &str) {
        self.busy = true;
        self.task = label.into();
        self.error = None;
        self.notice = None;
        self.progress = None;
        self.cancel = CancelToken::default();
    }
    fn find_bins(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        let spec = self.spec();
        let revision = self.wizard.revision();
        self.start("Finding source bins");
        let (tx, cancel, ctx) = (self.tx.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(|| job::bin_catalog(&spec, &cancel))
                .unwrap_or_else(|_| Err("The bin catalog worker stopped unexpectedly.".into()));
            let _ = tx.send(Event::Catalog(revision, result));
            ctx.request_repaint();
        });
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
        if self.wizard.inputs.check_required_paths().is_err() {
            return;
        }
        let spec = self.spec();
        let revision = self.wizard.revision();
        self.start("Checking conversion");
        self.invalidate();
        let (tx, cancel, ctx) = (self.tx.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut progress = |p| {
                let _ = tx.try_send(Event::Progress(p));
                ctx.request_repaint();
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                job::prepare_for_review(spec, &cancel, &mut progress)
            }))
            .unwrap_or_else(|_| Err(ReviewBlocker::new(
                BlockerKind::Retry,
                "The preflight worker stopped unexpectedly.",
            )))
            .map(Box::new);
            let _ = tx.send(Event::Prepared(revision, result));
            ctx.request_repaint();
        });
    }
    fn media_linking_ready(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.prepared.as_ref().and_then(|job| job.review().media_linking.as_ref())
            .is_some_and(|media| !media.premiere_files.is_empty())
        {
            return self.premiere_status.ready();
        }
        true
    }

    fn convert(&mut self, ctx: &egui::Context) {
        if !self.media_linking_ready() {
            return;
        }
        if self.busy || !self.wizard.can_convert() {
            return;
        }
        let Some(mut prepared) = self.prepared.take() else {
            return;
        };
        self.completion_subjects = prepared.take_review_subjects();
        self.report = ReportBrowser::default();
        self.start(CONVERTING);
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
    fn save_preflight_report(&mut self, path: PathBuf, ctx: &egui::Context) {
        let source = if let Some(prepared) = self.prepared.take() {
            PreflightReportSource::Ready(Box::new(prepared))
        } else if let Some(blocker) = self.review_blocker.take() {
            PreflightReportSource::Blocked(blocker)
        } else {
            return;
        };
        let spec = self.spec();
        self.start(SAVING_PREFLIGHT);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &source {
                PreflightReportSource::Ready(prepared) => prepared.save_preflight_report(&path),
                PreflightReportSource::Blocked(blocker) => {
                    job::save_blocked_preflight_report(&spec, blocker, &path)
                }
            }))
            .unwrap_or_else(|_| Err("The preflight report could not be saved.".into()));
            let _ = tx.send(Event::PreflightReportSaved(source, path, result));
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
                Pick::MappedOriginalFolder => d
                    .set_title("Choose the current folder for the saved path")
                    .pick_folder()
                    .map(|p| vec![p]),
                Pick::Output => d
                    .set_title("New Premiere project")
                    .add_filter("Premiere project", &["prproj"])
                    .set_file_name("Converted.prproj")
                    .save_file()
                    .map(|p| vec![p]),
                Pick::ProxyMedia => d
                    .set_title("Choose a folder for rewrapped proxies")
                    .pick_folder().map(|p| vec![p]),
                Pick::NewMedia | Pick::RewrapNeeded => d
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
                Pick::SavePreflightReport => d
                    .set_title("Save preflight report")
                    .add_filter("JSON report", &["json"])
                    .set_file_name("Any NLE Preflight.json")
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
                    rewrap_needed: s.rewrap_needed,
                    proxy_rewrap_dir: s.proxy_rewrap_dir,
                };
                self.wizard.choices = Choices {
                    mode: match s.mode {
                        job::ClipMode::RewrapNative => ClipMode::Rewrap,
                        job::ClipMode::Grouped => ClipMode::Grouped,
                        job::ClipMode::Merged => ClipMode::Merged,
                        job::ClipMode::Separate => ClipMode::Separate,
                    },
                    allow_offline_media: s.allow_offline_media,
                    prefer_source_media: s.prefer_source_media || s.attach_avid_proxies,
                    attach_avid_proxies: s.attach_avid_proxies,
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
                self.catalog = None;
                self.expansion = FolderExpansion::default();
                self.inventory = None;
                self.notice = Some(
                    "Job settings loaded. Check the paths, then run a fresh pre-flight.".into(),
                );
            }
            Err(e) => self.error = Some(format!("Couldn't load job: {e}")),
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        self.receive_events(ctx, None);
    }
    fn receive_events(&mut self, ctx: &egui::Context, mut first: Option<Event>) {
        for _ in 0..64 {
            let Some(event) = first.take().or_else(|| self.rx.try_recv().ok()) else {
                break;
            };
            match event {
                Event::WorkerFailed(message) => {
                    self.close_when_idle = false;
                    self.busy = false;
                    self.wizard.invalidate();
                    self.invalidate();
                    self.activity.record(message, true);
                }
                Event::Progress(p) => self.progress = Some(p),
                Event::Catalog(revision, result) => {
                    self.busy = false;
                    if self.cancel.is_cancelled() {
                        self.notice = Some("Cancelled.".into());
                    } else if revision == self.wizard.revision() {
                        match result {
                            Ok(catalog) => self.catalog = Some(catalog),
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
                Event::Inventory(revision, result) => {
                    self.busy = false;
                    if self.cancel.is_cancelled() {
                        self.notice = Some("Cancelled.".into());
                        continue;
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
                        continue;
                    }
                    if revision != self.wizard.revision() {
                        self.notice = Some("Settings changed; check again before converting.".into());
                        continue;
                    }
                    match result {
                        Ok(p) => {
                            self.wizard.accept_review(revision);
                            self.report = ReportBrowser::new(&p.review().notes);
                            self.inventory = Some(p.review().inventory.clone());
                            self.prepared = Some(*p);
                        }
                        Err(issue) => self.review_blocker = Some(issue),
                    }
                }
                Event::Finished(c) => {
                    self.busy = false;
                    self.wizard.invalidate();
                    self.report = ReportBrowser::new(&c.report);
                    if c.status == CompletionStatus::Failed {
                        self.activity.record(c.message.clone(), true);
                    }
                    self.completion = Some(c);
                }
                Event::PreflightReportSaved(source, path, result) => {
                    self.busy = false;
                    match source {
                        PreflightReportSource::Ready(prepared) => self.prepared = Some(*prepared),
                        PreflightReportSource::Blocked(issue) => self.review_blocker = Some(issue),
                    }
                    match result {
                        Ok(()) => {
                            self.notice =
                                Some(format!("Preflight report saved to {}", path.display()))
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Event::Picked(kind, paths) => {
                    self.busy = false;
                    let before = self.wizard.snapshot();
                    let mut find_bins = false;
                    if let Some(paths) = paths {
                        if let Some(path) = paths.first() {
                            match kind {
                                Pick::Bins => {
                                    self.wizard.inputs.project_folder = None;
                                    self.wizard.inputs.entire_project = false;
                                    self.wizard.inputs.add_bins(paths.clone());
                                    self.catalog = None;
                                    self.expansion = FolderExpansion::default();
                                    self.search.clear();
                                    self.inventory = None;
                                }
                                Pick::Project => {
                                    self.wizard.inputs.project_folder = Some(path.clone());
                                    self.wizard.inputs.entire_project = true;
                                    self.wizard.inputs.bins.clear();
                                    self.catalog = None;
                                    self.expansion = FolderExpansion::default();
                                    self.search.clear();
                                    self.inventory = None;
                                    find_bins = true;
                                }
                                Pick::Output => self.wizard.inputs.output = path.clone(),
                                Pick::Media => {
                                    if !self.wizard.inputs.media_roots.contains(path) {
                                        self.wizard.inputs.media_roots.push(path.clone());
                                    }
                                }
                                Pick::MappedOriginalFolder => self.mapping_actual = path.clone(),
                                Pick::ProxyMedia => self.wizard.inputs.proxy_rewrap_dir = Some(path.clone()),
                                Pick::NewMedia => {
                                    self.wizard.inputs.new_media = Some(path.join(new_media_name()))
                                }
                                Pick::RewrapNeeded => {
                                    self.wizard.inputs.rewrap_needed = Some(path.join(new_media_name()))
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
                                Pick::SavePreflightReport => {
                                    self.save_preflight_report(path.clone(), ctx);
                                }
                                Pick::LoadJob => {
                                    #[cfg(target_os = "macos")]
                                    match anynle_app::resolve_job::load_if_avid_resolve(path) {
                                        Ok(Some(job)) => {
                                            self.resolve_wizard = Some(crate::avid_resolve::Wizard::from_job(job, ctx));
                                            continue;
                                        }
                                        Err(error) => {
                                            self.error = Some(error);
                                            continue;
                                        }
                                        Ok(None) => {}
                                    }
                                    self.load_job(path);
                                }
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
                    if self.apply_settings_changes(&before) && self.remember_choices {
                        let _ = storage::save(&self.wizard.choices);
                    }
                    if find_bins {
                        self.find_bins(ctx);
                    }
                }
            }
        }
        if self.close_when_idle && !self.busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    fn why_button(&mut self, ui: &mut egui::Ui, title: &str) {
        if ui.link("Why?").clicked() {
            self.why = Some(title.into());
        }
    }
    fn title(ui: &mut egui::Ui, title: &str, subtitle: &str) {
        if !subtitle.is_empty() {
            ui.label(RichText::new(subtitle).color(MUTED).small());
            ui.add_space(3.);
        }
        ui.heading(RichText::new(title).strong());
        ui.add_space(5.);
    }
    fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
        Self::card_with_fill(ui, SURFACE, content);
    }
    fn card_with_fill(ui: &mut egui::Ui, fill: Color32, content: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(fill)
            .inner_margin(12.)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                content(ui);
            });
    }
    fn require_project_paths(&mut self) -> bool {
        let ready = self.wizard.inputs.check_required_paths().is_ok();
        self.highlight_required_input = !ready;
        if !ready {
            self.wizard.page = Page::Direction;
        }
        ready
    }
    fn direction(&mut self, ui: &mut egui::Ui) {
        #[cfg(feature = "wizard-automation")]
        {
            self.source_viewport = None;
        }
        let required_input = if self.highlight_required_input {
            self.wizard.inputs.check_required_paths().err()
        } else {
            None
        };
        self.highlight_required_input = required_input.is_some();
        let (source_fill, destination_fill) = match required_input {
            Some(RequiredInput::Bins | RequiredInput::BinSelection) => (ERROR_SURFACE, SURFACE),
            Some(RequiredInput::Output | RequiredInput::OutputExtension) => (SURFACE, ERROR_SURFACE),
            None => (SURFACE, SURFACE),
        };
        Self::card_with_fill(ui, source_fill, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Source bins").font(semibold(14.)));
                if ui.button("Choose project folder…").clicked() {
                    self.pick(Pick::Project, ui.ctx());
                }
                if ui.button("Choose bins…").clicked() {
                    self.pick(Pick::Bins, ui.ctx());
                }
                ui.menu_button("Source options", |ui| {
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
                });
            });
            if let Some(path) = &self.wizard.inputs.project_folder {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(path_name(path)).small().color(MUTED))
                        .on_hover_text(path.display().to_string());
                    ui.checkbox(
                        &mut self.wizard.inputs.entire_project,
                        "Import entire project",
                    );
                });
            } else if self.catalog.is_none() && !self.wizard.inputs.bins.is_empty() {
                ui.label(
                    RichText::new(format!("{} selected", counted(self.wizard.inputs.bins.len(), "bin", "bins")))
                        .color(MUTED),
                );
            }
            if self.wizard.inputs.project_folder.is_some() || !self.wizard.inputs.bins.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    let action = if self.catalog.is_some() { "Refresh bins" } else { "Find bins" };
                    if ui.button(action).clicked() {
                        self.find_bins(ui.ctx());
                    }
                    if self.catalog.is_some() {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Search bins and folders")
                                .desired_width(220.),
                        );
                    }
                });
            }
            if let Some(catalog) = &self.catalog {
                let inputs = &mut self.wizard.inputs;
                let search = self.search.trim().to_lowercase();
                let matches: Option<Vec<_>> = (!search.is_empty()).then(|| {
                    catalog.iter()
                        .map(|bin| {
                            bin.relative
                                .to_string_lossy()
                                .to_lowercase()
                                .contains(&search)
                        })
                        .collect()
                });
                if catalog.is_empty() {
                    ui.small("No bins found.");
                } else if matches
                    .as_ref()
                    .is_none_or(|matches| matches.iter().any(|&matched| matched))
                {
                    let height = ui.available_height() - PROJECT_TAIL_SPACE;
                    let _list = source_list(ui, ("bin-tree", &inputs.project_folder), Some(height), |ui| {
                        ui.spacing_mut().indent = 14.;
                        ui.visuals_mut().indent_has_left_vline = false;
                        let mut selected = BinSelection::new(&mut inputs.bins);
                        Tree::build(catalog).show(
                            ui,
                            catalog,
                            &mut selected,
                            matches.as_deref(),
                            inputs.entire_project,
                            &mut self.expansion,
                        );
                    });
                    #[cfg(feature = "wizard-automation")]
                    {
                        let mut visible = _list.inner_rect;
                        visible.set_height(visible.height().min(_list.content_size.y));
                        self.source_viewport = Some(visible);
                    }
                } else {
                    ui.small("No bins match your search.");
                }
                ui.small(format!(
                    "{} bins available · {} selected",
                    catalog.len(),
                    if self.wizard.inputs.entire_project {
                        catalog.len()
                    } else {
                        self.wizard.inputs.bins.len()
                    }
                ));
            } else if !self.wizard.inputs.bins.is_empty() {
                let mut remove = None;
                let height = ui.available_height() - PROJECT_TAIL_SPACE;
                source_list(ui, "picked-bins", Some(height), |ui| {
                    for (i, p) in self.wizard.inputs.bins.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                            ui.add(egui::Label::new(path_name(p)).truncate())
                                .on_hover_text(p.display().to_string());
                        });
                    }
                });
                if let Some(i) = remove {
                    self.wizard.inputs.bins.remove(i);
                }
            }

        });
        ui.add_space(8.);
        Self::card_with_fill(ui, destination_fill, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Destination").font(semibold(14.)));
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

        });
    }
    fn saved_path_mappings(&mut self, ui: &mut egui::Ui) {
        ui.small("Enter the folder prefix saved by the source editor, then choose its current folder.");
        ui.add(egui::TextEdit::singleline(&mut self.mapping_saved).hint_text("Saved folder prefix"));
        path_edit(ui, &mut self.mapping_actual, "Current folder");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Browse current folder…").clicked() {
                self.pick(Pick::MappedOriginalFolder, ui.ctx());
            }
            let saved = self.mapping_saved.trim();
            if ui.add_enabled(!saved.is_empty() && self.mapping_actual.is_absolute(), egui::Button::new("Add mapping")).clicked() {
                let paths = &mut self.wizard.inputs.path_mappings;
                let prefix = PathBuf::from(saved);
                let current = std::mem::take(&mut self.mapping_actual);
                if let Some((_, root)) = paths.iter_mut().find(|(old, _)| *old == prefix) {
                    *root = current;
                } else {
                    paths.push((prefix, current));
                }
                self.mapping_saved.clear();
            }
        });
        let mut remove = None;
        for (i, (saved, current)) in self.wizard.inputs.path_mappings.iter().enumerate() {
            ui.push_id(i, |ui| {
                ui.separator();
                review_path(ui, "Saved prefix", saved);
                review_path(ui, "Current folder", current);
                if ui.small_button("Remove mapping").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.wizard.inputs.path_mappings.remove(i);
        }
    }

    fn clips(&mut self, ui: &mut egui::Ui) {
        let jump = self.media_jump.take();
        if let Some(inv) = &self.inventory {
            if inv.native_media == 0 && inv.ume_originals > 0 {
                Self::card(ui, |ui| {
                    ui.colored_label(GOOD, "Linked-original candidates found.");
                    ui.label("Pre-flight checks these original-file links. If all qualify, no new media folder is needed. Your native-media choice is remembered for other projects.");
                });
            }
        }
        Self::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Clip layout").font(semibold(14.)));
                if self.wizard.choices.mode != ClipMode::Rewrap
                    && ui.link("Use recommended mode").clicked()
                {
                    self.wizard.choices.mode = ClipMode::Rewrap;
                }
                self.why_button(ui, "How clips are built");
            });
            for mode in ClipMode::ALL {
                let selected = self.wizard.choices.mode == mode;
                egui::Frame::new()
                    .fill(if selected { SELECTED } else { SURFACE })
                    .corner_radius(7)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            let choice = ui.radio_value(&mut self.wizard.choices.mode, mode, mode.label());
                            let (summary, limitation) = mode.explanation();
                            choice.on_hover_text(format!("{summary} {limitation}"));
                            if mode == ClipMode::Rewrap {
                                ui.label(RichText::new("Recommended").small().color(ACCENT));
                            }
                        });
                    });
            }
            let mode = self.wizard.choices.mode;
            let (summary, limitation) = mode.explanation();
            ui.label(RichText::new(summary).small());
            ui.label(RichText::new(limitation).small().color(MUTED));
            if mode != ClipMode::Rewrap {
                ui.checkbox(&mut self.wizard.choices.allow_offline_media, "Allow offline media");
            }
        });
        if self.wizard.choices.mode != ClipMode::Rewrap {
            Self::card(ui, |ui| {
                ui.strong("Primary media");
                ui.radio_value(&mut self.wizard.choices.prefer_source_media, false, "Use the media in the Avid edit");
                ui.radio_value(&mut self.wizard.choices.prefer_source_media, true, "Prefer camera originals for picture");
                if self.wizard.choices.uses_camera_originals() {
                    ui.small("Try the saved camera path or a saved-path mapping. Keep the Avid media when the match cannot be verified.");
                    ui.checkbox(&mut self.wizard.choices.attach_avid_proxies, "Attach matching Avid media as proxies");
                }
                if self.wizard.choices.attaches_avid_proxies() {
                    ui.small("Verified picture-only DNxHD can attach in place. A matching stereo original can use a rewrapped proxy after both audio channels are verified.");
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Rewrap proxy for compatibility…").on_hover_text("Creates new media.").clicked() {
                            self.pick(Pick::ProxyMedia, ui.ctx());
                        }
                        if self.wizard.inputs.proxy_rewrap_dir.is_some() && ui.button("Link existing proxies only").clicked() {
                            self.wizard.inputs.proxy_rewrap_dir = None;
                        }
                    });
                    if let Some(path) = &self.wizard.inputs.proxy_rewrap_dir {
                        ui.colored_label(AMBER, "Creates new media.");
                        review_path(ui, "Proxy folder", path);
                    }
                }
            });
        }
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
            ui.colored_label(AMBER, "Some ProRes OP-Atom files need rewrapping for Premiere.");
        }
        let locations = matches!(jump, Some(MediaJump::Locations | MediaJump::OriginalMappings));
        let locations_panel = egui::CollapsingHeader::new("Media locations & linking")
            .default_open(true)
            .open(locations.then_some(true))
            .show(ui,|ui|{
            ui.label("Media stays in place. Pre-flight checks file presence, identity and routing.");
            ui.horizontal(|ui|{if ui.button("Add Avid media folder…").clicked(){self.pick(Pick::Media,ui.ctx());}if ui.button("Refresh source references").clicked(){self.scan(ui.ctx());}});
            path_list(ui,"Avid media",&mut self.wizard.inputs.media_roots);
            if let Some(inv)=&self.inventory {ui.colored_label(if inv.found_media==inv.total_media{GOOD}else{AMBER},media_status(inv));}
            let mappings = egui::CollapsingHeader::new("Saved-path mappings")
                .open((jump == Some(MediaJump::OriginalMappings)).then_some(true)).show(ui,|ui|{
                self.saved_path_mappings(ui);
            });
            if jump == Some(MediaJump::OriginalMappings) {
                mappings.header_response.scroll_to_me(Some(egui::Align::TOP));
            }
        });
        if jump == Some(MediaJump::Locations) {
            locations_panel.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
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
        let tools = egui::CollapsingHeader::new("External media tools")
            .open((jump == Some(MediaJump::Tools)).then_some(true))
            .show(ui, |ui| {
                ui.small("The packaged verified FFmpeg tools are used when available. Choose explicit tools for a local installation.");
                for (label, pick, path) in [
                    ("FFmpeg…", Pick::Ffmpeg, self.ffmpeg.clone()),
                    ("ffprobe…", Pick::Ffprobe, self.ffprobe.clone()),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(label).clicked() {
                            self.pick(pick, ui.ctx());
                        }
                        ui.label(path.map_or("Bundled tools".into(), |p| p.display().to_string()));
                    });
                }
                if (self.ffmpeg.is_some() || self.ffprobe.is_some()) && ui.button("Use bundled tools").clicked() {
                    self.ffmpeg = None;
                    self.ffprobe = None;
                    self.wizard.invalidate();
                    self.invalidate();
                }
            });
        if jump == Some(MediaJump::Tools) {
            tools.header_response.scroll_to_me(Some(egui::Align::TOP));
        }
    }
    fn markers(&mut self, ui: &mut egui::Ui) {
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
        if !self.wizard.choices.effects.is_empty()
            && ui.button("Clear saved effect choices").clicked()
        {
            self.wizard.choices.effects.clear();
        }
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
    fn blocker_card(
        ui: &mut egui::Ui,
        issue: &ReviewBlocker,
        browser: &mut ReportBrowser,
    ) -> Option<ReviewAction> {
        use BlockerKind::*;
        let (title, next, (label, target)) = match issue.kind {
            ProjectSource => (
                "Check the Avid source",
                "Check the selected bins or project folder on Project, then return here to run preflight.",
                ("Go to Project", ReviewAction::Step(Page::Direction)),
            ),
            ProjectOutput => (
                "Choose a project destination",
                "Select a new .prproj file in an available folder, then check again.",
                ("Choose project file…", ReviewAction::Choose(Pick::Output)),
            ),
            AvidMedia => (
                "Check Avid media",
                "Add or correct the Avid media folders. If there are duplicate copies, select the intended source folders. Then return to Review.",
                (
                    "Open media locations",
                    ReviewAction::Media(MediaJump::Locations),
                ),
            ),
            RewrapMissingMedia => (
                "Required media is missing",
                "Locate the source media to rewrap, or switch to Separate files to preserve the edits as offline clips.",
                ("Locate media", ReviewAction::Media(MediaJump::Locations)),
            ),
            RewrapMissingOriginal => (
                "Required linked original is missing",
                "Reconnect the original drive or map the saved path to its current folder. You can switch to Separate files to review offline links.",
                (
                    "Open saved-path mappings",
                    ReviewAction::Media(MediaJump::OriginalMappings),
                ),
            ),
            LegacyMxfPackage => (
                "Check legacy MXF packages",
                "Use the package IDs and lanes below to check the files in the selected Avid media folders. If several copies claim the same package ID, select only the intended source folders.",
                (
                    "Open media locations",
                    ReviewAction::Media(MediaJump::Locations),
                ),
            ),
            OriginalLocator => (
                "Locate the linked original",
                "Reconnect the original drive or map the saved path prefix to its current folder. Then return to Review.",
                (
                    "Open saved-path mappings",
                    ReviewAction::Media(MediaJump::OriginalMappings),
                ),
            ),
            OriginalMedia => (
                "Linked original could not be used",
                "Check the file and the reason below. If its format is unsupported, select a bin with compatible originals or prepare compatible media in Avid.",
                ("Review selected bins", ReviewAction::Step(Page::Direction)),
            ),
            NewMedia => (
                "Choose a new media folder",
                "Choose a parent folder with room for the new media, then check again.",
                ("Choose media parent…", ReviewAction::Choose(Pick::NewMedia)),
            ),
            ProxyMedia => (
                "Check the proxy output folder",
                "Choose an existing folder with space for the rewrapped proxies, or use the existing proxies only.",
                (
                    "Choose proxy folder…",
                    ReviewAction::Choose(Pick::ProxyMedia),
                ),
            ),
            MediaTools => (
                "Check the media tools",
                "Review the FFmpeg and ffprobe paths, or reset them to the bundled tools. Then return to Review.",
                ("Open media tools", ReviewAction::Media(MediaJump::Tools)),
            ),
            Effects => (
                "Check saved effect choices",
                "Clear saved effect choices to use the supported settings for these bins, then return to Review.",
                ("Go to Effects", ReviewAction::Step(Page::Effects)),
            ),
            Rewrap => (
                "Media preparation blocked",
                "Check the reason below. Review the clip mode and new media folder, or change the selected bins if the source is unsupported.",
                ("Go to Clips & media", ReviewAction::Step(Page::Clips)),
            ),
            GroupClipMode => (
                "Use Separate files for editable Avid Groups",
                "Switch clip mode, then return to Review to check the Group and its original media.",
                (
                    "Switch to Separate files",
                    ReviewAction::SwitchToSeparate {
                        enable_offline_media: false,
                    },
                ),
            ),
            SourceContent => (
                "Selected content could not be converted",
                "Check the affected source below. Adjust the source in Avid or narrow the bin selection, then run preflight again.",
                ("Review selected bins", ReviewAction::Step(Page::Direction)),
            ),
            SequenceTiming => (
                "Sequence timing could not be translated",
                "In a copy of the Avid sequence, replace the affected edits with media at the sequence frame rate. You can also select bins that do not contain the affected sequence.",
                ("Review selected bins", ReviewAction::Step(Page::Direction)),
            ),
            Retry => (
                "Run preflight again",
                "Wait for any changes to finish before checking again.",
                ("Check again", ReviewAction::Recheck),
            ),
        };
        let mut action = None;
        Self::card_with_fill(ui, ERROR_SURFACE, |ui| {
            let description = |ui: &mut egui::Ui| {
                ui.colored_label(ERROR_INK, RichText::new(title).font(semibold(14.)));
                ui.small(next);
            };
            if recovery_button(ui, label, description) {
                action = Some(target);
            }
            if matches!(issue.kind, RewrapMissingMedia | RewrapMissingOriginal)
                && ui.button("Switch to Separate files").clicked()
            {
                action = Some(ReviewAction::SwitchToSeparate {
                    enable_offline_media: true,
                });
            }
            let details_label = if issue.affected.is_empty() {
                "Blocker details".into()
            } else {
                format!("Affected details · {}", issue.affected.len())
            };
            egui::CollapsingHeader::new(details_label)
                .id_salt(("blocker-details", title))
                .show(ui, |ui| {
                    ui.label(&issue.message);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Copy details").clicked() {
                            let mut details = vec![issue.message.as_str()];
                            details.extend(issue.affected.iter().map(String::as_str));
                            ui.ctx().copy_text(details.join("\n"));
                        }
                        if ui.button("Save preflight report").clicked() {
                            action = Some(ReviewAction::Choose(Pick::SavePreflightReport));
                        }
                    });
                    Self::blocker_details(ui, &issue.affected, &issue.timeline_groups, browser);
                });
        });
        action
    }
    fn blocker_details(
        ui: &mut egui::Ui,
        details: &[String],
        timelines: &[job::ReviewGroup],
        browser: &mut ReportBrowser,
    ) {
        if details.len() <= 8 && timelines.is_empty() {
            for detail in details {
                ui.label(detail);
            }
            return;
        }
        let changed = ui.horizontal_wrapped(|ui| {
            let changed = Self::report_group_filter(ui, timelines, &mut browser.timeline,
                "report-timeline", "All timelines");
            ui.add(
                egui::TextEdit::singleline(&mut browser.search)
                    .hint_text("Search affected details")
                    .desired_width(220.),
            ).changed() || changed
        }).inner;
        if changed {
            browser.refilter_details(details, timelines);
        }
        if browser.count(details.len()) == 0 {
            ui.label("No affected details match your search.");
            return;
        }
        let page_changed = Self::report_pages(ui, browser, details.len(), "Details");
        let mut rows = egui::ScrollArea::vertical().id_salt("blocker-details").max_height(260.);
        if changed || page_changed {
            rows = rows.vertical_scroll_offset(0.);
        }
        rows.show(ui, |ui| {
            for index in browser.page_indices(details.len()) {
                ui.label(&details[index]);
            }
        });
    }
    fn take_review_action(&mut self, action: ReviewAction, ctx: &egui::Context) {
        match action {
            ReviewAction::Step(page) => self.wizard.page = page,
            ReviewAction::Choose(pick) => self.pick(pick, ctx),
            ReviewAction::SwitchToSeparate { enable_offline_media } => {
                self.wizard.choices.mode = ClipMode::Separate;
                if enable_offline_media {
                    self.wizard.choices.allow_offline_media = true;
                }
                self.invalidate();
                self.wizard.page = Page::Clips;
            }
            ReviewAction::Media(jump) => {
                self.media_jump = Some(jump);
                self.wizard.page = Page::Clips;
            }
            ReviewAction::Recheck => self.prepare(ctx),
        }
    }
    fn review(&mut self, ui: &mut egui::Ui) {
        if self.completion.is_some() {
            self.finish(ui);
            return;
        }
        if self.busy && matches!(self.task.as_str(), CONVERTING | SAVING_PREFLIGHT) {
            return;
        }
        let width = ui.available_width();
        if width >= 600. {
            let output_width = 196.;
            ui.horizontal_top(|ui| {
                let findings_width = width - output_width - ui.spacing().item_spacing.x;
                column(ui, findings_width, |ui| self.review_findings(ui));
                column(ui, output_width, |ui| self.review_output(ui));
            });
        } else {
            self.review_findings(ui);
            ui.add_space(10.);
            self.review_output(ui);
        }
    }
    fn review_output(&mut self, ui: &mut egui::Ui) {
        let i = &self.wizard.inputs;
        let choices = &self.wizard.choices;
        let rewrap = choices.mode == ClipMode::Rewrap;
        let review = self.prepared.as_ref().map(PreparedJob::review);
        let creates_media = review.is_some_and(|review| review.estimated_bytes.is_some());
        Self::card(ui, |ui| {
            ui.strong("Output");
            if i.output.as_os_str().is_empty() {
                ui.label(RichText::new("Choose a project file on the Project step.").color(MUTED));
            } else {
                review_path(ui, "Premiere project", &i.output);
            }
            ui.small("Clips");
            ui.label(choices.mode.label());
            ui.small("Source");
            ui.label(if i.entire_project {
                "Entire project".into()
            } else {
                counted(i.bins.len(), "bin", "bins")
            });
            if !rewrap {
                ui.label(if choices.uses_camera_originals() {
                    "Primary media · Prefer camera originals for picture"
                } else {
                    "Primary media · Use the media in the Avid edit"
                });
                if choices.attaches_avid_proxies() {
                    ui.label(if i.proxy_rewrap_dir.is_some() {
                        "Proxies · Attach matching Avid media and allow stereo rewrap"
                    } else {
                        "Proxies · Link matching Avid media in place"
                    });
                }
            }
            if creates_media {
                ui.colored_label(AMBER, RichText::new("Creates new media.").strong());
                if let Some(path) = if rewrap { i.new_media.as_ref() } else {
                    i.rewrap_needed.as_ref().or(i.proxy_rewrap_dir.as_ref())
                } {
                    review_path(ui, "Media folder", path);
                }
            } else if rewrap && review.is_none() {
                ui.label(RichText::new("A new media folder is needed for native OP-Atom rewraps.").color(MUTED));
                if let Some(path) = &i.new_media {
                    review_path(ui, "Media folder", path);
                }
            }
            if let Some(review) = review {
                self.rewrap_size.show(ui, &review.rewrap_sources);
            }
        });
    }
    fn review_findings(&mut self, ui: &mut egui::Ui) {
        let blocked = self.review_blocker.is_some();
        if let Some(issue) = &self.review_blocker {
            if let Some(action) = Self::blocker_card(ui, issue, &mut self.report) {
                self.take_review_action(action, ui.ctx());
                return;
            }
            self.rewrap_size.show(ui, &issue.rewrap_sources);
        }
        let media_ready = self.media_linking_ready();
        let choices = &self.wizard.choices;
        let review = self.prepared.as_ref().map(PreparedJob::review);
        let creates_media = review.is_some_and(|review| review.estimated_bytes.is_some());

        let mut save_report = false;
        let mut choose_rewrap = false;
        let mut clear_rewrap = false;
        if !blocked {
            ui.vertical(|ui| {
                if let Some(review) = review {
                    let inv = &review.inventory;
                    let changes = self.report.changes;
                    let (status, color) = if !media_ready {
                        ("Waiting for Premiere", AMBER)
                    } else if changes == 0 {
                        ("Ready to convert", GOOD)
                    } else {
                        ("Ready to convert with changes", AMBER)
                    };
                    ui.colored_label(color, RichText::new(status).strong());
                    ui.label(format!(
                        "{} checked · {} of {} found{}",
                        counted(inv.bins.len(), "bin", "bins"),
                        inv.found_media,
                        counted(inv.total_media, "media file", "media files"),
                        if inv.ume_originals == 0 {
                            String::new()
                        } else {
                            format!(" · {}", counted(inv.ume_originals, "qualified original", "qualified originals"))
                        }
                    ));
                    if inv.ume_originals > 0 && inv.native_media == 0 {
                        ui.label("Links to available originals in place.");
                    }
                    if let Some(media) = &review.media_linking {
                        ui.add_space(6.);
                        ui.strong("Media linking");
                        if !media.rewrap_available.is_empty() {
                            ui.colored_label(AMBER, format!("Rewrap available for {} managed clips.", media.rewrap_available.len()));
                            ui.collapsing("Affected clips", |ui| {
                                for clip in &media.rewrap_available { ui.label(clip); }
                            });
                            choose_rewrap = ui.button("Choose folder to rewrap affected clips…").clicked();
                        }
                        if !media.rewrapped_clips.is_empty() {
                            ui.label(format!("{} managed clips will be rewrapped.", media.rewrapped_clips.len()));
                            clear_rewrap = ui.button("Continue without rewrapping").clicked();
                        }
                        if !media.conversion_pending.is_empty() {
                            ui.colored_label(AMBER, format!("Conversion pending for {} recognized clips.", media.conversion_pending.len()));
                            ui.collapsing("Show clips with pending conversion", |ui| {
                                for clip in &media.conversion_pending { ui.label(clip); }
                            });
                        }
                        if media.premiere_files.is_empty() {
                            ui.label(format!("{} link directly in place.",
                                counted(media.direct_files, "verified file", "verified files")));
                        } else {
                            #[cfg(target_os = "macos")]
                            if let Err(error) = self.premiere_status.show(ui) {
                                self.error = Some(error);
                            }
                            egui::CollapsingHeader::new("Files that need Premiere").show(ui, |ui| {
                                for path in &media.premiere_files {
                                    review_path(ui, "Original", Path::new(path));
                                }
                            });
                        }
                    }
                    if creates_media {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Space reserved for preparation").color(MUTED));
                            ui.strong(bytes(review.estimated_bytes));
                            ui.label(RichText::new("· Available").color(MUTED));
                            ui.label(bytes(review.available_bytes));
                        });
                    }
                    if choices.uses_camera_originals() {
                        if review.camera_originals.is_empty() {
                            ui.label("No camera-original matches. Using the media in the Avid edits.");
                        } else {
                            let count = review.camera_originals.len();
                            egui::CollapsingHeader::new(format!("Camera-original matches · {count}")).show(ui, |ui| {
                                for link in &review.camera_originals {
                                    ui.strong(&link.name);
                                    review_path(ui, "Picture", &link.picture);
                                    if !link.audio_channels.is_empty() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.small("Audio from camera file");
                                            for (track, channel) in &link.audio_channels {
                                                ui.small(format!("· A{track}"));
                                                ui.add(crate::icons::arrow(ui, 11., "to"));
                                                ui.small(format!("channel {channel}"));
                                            }
                                        });
                                    }
                                    if let Some(proxy) = &link.proxy {
                                        review_path(ui, "Proxy", proxy);
                                    }
                                    ui.add_space(5.);
                                }
                            });
                        }
                    }
                    if changes > 0 {
                        ui.add_space(7.);
                        ui.colored_label(AMBER, RichText::new(
                            counted(changes, "translation change", "translation changes")
                        ).strong());
                        egui::CollapsingHeader::new("Affected edits")
                            .id_salt("preflight-affected-edits")
                            .show(ui, |ui| {
                                for &index in &self.report.change_examples {
                                    let note = &review.notes[index];
                                    if let Some(name) = review.subjects.get(&note.object) {
                                        ui.strong(name);
                                    }
                                    ui.label(&note.message);
                                }
                            });
                    }
                    if self.report.warnings > 0 {
                        ui.colored_label(AMBER, counted(self.report.warnings, "warning", "warnings"));
                    }
                    let label = format!("Full preflight report · {}", counted(review.notes.len(), "note", "notes"));
                    egui::CollapsingHeader::new(label)
                        .id_salt("preflight-report")
                        .show(ui, |ui| {
                            Self::report(ui, &review.notes, Some(&review.subjects),
                                &review.timeline_groups, &review.source_groups, &mut self.report, &mut self.why);
                            if !inv.notes.is_empty() {
                                egui::CollapsingHeader::new(format!("Inventory notes · {}", inv.notes.len()))
                                    .show(ui, |ui| {
                                        egui::ScrollArea::vertical().max_height(180.).show(ui, |ui| {
                                            for note in &inv.notes {
                                                ui.label(RichText::new(note).color(MUTED));
                                            }
                                        });
                                    });
                            }
                        });
                    save_report = ui
                        .add_enabled(!self.busy, egui::Button::new("Save preflight report"))
                        .clicked();
                } else {
                    ui.strong(if self.busy { "Preflight in progress" } else { "Preflight" });
                    ui.label(RichText::new("Checks media, translation limits and disk space before creating any output.").color(MUTED));
                }
            });
        }

        egui::CollapsingHeader::new("Conversion settings").show(ui, |ui| {
            let p = AvidToPremiere::from_json_text(&choices.policy_json).unwrap_or_default();
            ui.label(format!(
                "Dependencies {} · Trash / Attic {}",
                on_off(choices.include_dependencies),
                on_off(choices.include_trash_attic),
            ));
            ui.label(format!(
                "Marker priority: {} · move losing marker to sequence {} · Avid info {} · default pan {}",
                if p.source_marker_wins { "source" } else { "edit" },
                on_off(p.conflict_loser_to_sequence),
                on_off(p.marker_avid_info),
                on_off(p.match_avid_default_pan)
            ));
        });
        if let Some(review) = review {
            egui::CollapsingHeader::new("Command-line replay").show(ui, |ui| {
                if review.cli_equivalent.is_empty() {
                    ui.label("Use Save job settings and run any-nle run --job SETTINGS.json.");
                } else {
                    let mut command = review.cli_equivalent.as_str();
                    ui.add(
                        egui::TextEdit::multiline(&mut command)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(3),
                    );
                    if ui.button("Copy CLI command").clicked() {
                        ui.ctx().copy_text(review.cli_equivalent.clone());
                    }
                }
            });
        }
        if choose_rewrap {
            self.pick(Pick::RewrapNeeded, ui.ctx());
        }
        if clear_rewrap {
            self.wizard.inputs.rewrap_needed = None;
            self.invalidate();
        }
        if save_report {
            self.pick(Pick::SavePreflightReport, ui.ctx());
        }
    }
    fn report_group_filter(
        ui: &mut egui::Ui,
        groups: &[job::ReviewGroup],
        selection: &mut Option<usize>,
        id: &str,
        all: &str,
    ) -> bool {
        if groups.is_empty() {
            return false;
        }
        let label = |group: &job::ReviewGroup| match &group.bin {
            Some(bin) => format!("{} · {}", group.name, bin.display()),
            None => group.name.clone(),
        };
        let selected = selection.and_then(|index| groups.get(index))
            .map_or_else(|| all.to_owned(), label);
        let mut changed = false;
        egui::ComboBox::from_id_salt(id)
            .selected_text(selected)
            .wrap_mode(egui::TextWrapMode::Truncate)
            .height(240.)
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(selection, None, all).changed();
                for (index, group) in groups.iter().enumerate() {
                    changed |= ui.selectable_value(selection, Some(index), label(group))
                        .on_hover_text(&group.id)
                        .changed();
                }
            });
        changed
    }
    fn report(
        ui: &mut egui::Ui,
        notes: &[job::ReportNote],
        subjects: Option<&HashMap<String, String>>,
        timelines: &[job::ReviewGroup],
        sources: &[job::ReviewGroup],
        browser: &mut ReportBrowser,
        why: &mut Option<String>,
    ) {
        let changed = ui.horizontal_wrapped(|ui| {
            let filters = [
                (Filter::All, format!("All · {}", notes.len())),
                (Filter::Changes, format!("Changes · {}", browser.changes)),
                (Filter::Warnings, format!("Warnings · {}", browser.warnings)),
            ];
            let mut changed = false;
            for (filter, label) in filters {
                changed |= ui.selectable_value(&mut browser.filter, filter, label).changed();
            }
            changed |= Self::report_group_filter(ui, timelines, &mut browser.timeline,
                "report-timeline", "All timelines");
            changed |= Self::report_group_filter(ui, sources, &mut browser.source_clip,
                "report-source-clip", "All source clips");
            let search_changed = ui.add(
                egui::TextEdit::singleline(&mut browser.search)
                    .hint_text("Search report")
                    .desired_width(180.),
            ).changed();
            changed || search_changed
        }).inner;
        if changed {
            browser.refilter(notes, subjects, timelines, sources);
        }
        let count = browser.count(notes.len());
        if count == 0 {
            ui.label(if notes.is_empty() {
                "No translation notes reported."
            } else {
                "No notes match these filters."
            });
            return;
        }
        let page_changed = Self::report_pages(ui, browser, notes.len(), "Notes");
        let mut rows = egui::ScrollArea::vertical().id_salt("report-rows").max_height(360.);
        if changed || page_changed {
            rows = rows.vertical_scroll_offset(0.);
        }
        rows.show(ui, |ui| {
            for index in browser.page_indices(notes.len()) {
                let n = &notes[index];
                let subject = subjects.and_then(|names| names.get(&n.object)).map(String::as_str);
                Self::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&n.severity).small().color(
                            match n.severity.to_lowercase().as_str() {
                                "error" | "failure" => Color32::LIGHT_RED,
                                "loss" | "warning" => AMBER,
                                _ => MUTED,
                            },
                        ));
                        ui.strong(subject.unwrap_or(&n.object)).on_hover_text(&n.object);
                        if ui.link("Why?").clicked() {
                            *why = Some(if n.why.is_empty() {
                                n.message.clone()
                            } else {
                                n.why.clone()
                            });
                        }
                    });
                    ui.label(&n.message);
                });
            }
        });
    }
    fn report_pages(
        ui: &mut egui::Ui,
        browser: &mut ReportBrowser,
        total: usize,
        label: &str,
    ) -> bool {
        let previous_page = browser.page;
        let count = browser.count(total);
        let page_count = count.div_ceil(PAGE_SIZE);
        ui.horizontal_wrapped(|ui| {
            if page_count > 1 {
                if ui.add_enabled(browser.page > 0, egui::Button::new("Previous")).clicked() {
                    browser.page -= 1;
                }
                let mut page_number = browser.page + 1;
                ui.label("Page");
                ui.add(egui::DragValue::new(&mut page_number).range(1..=page_count));
                browser.page = page_number - 1;
                ui.label(format!("of {page_count}"));
                if ui.add_enabled(browser.page + 1 < page_count, egui::Button::new("Next")).clicked() {
                    browser.page += 1;
                }
            }
            let range = browser.page_range(total);
            ui.small(format!("{label} {}–{} of {count}", range.start + 1, range.end));
        });
        browser.page != previous_page
    }
    fn finish(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.completion.as_ref() else {
            return;
        };
        let (title, color) = match c.status {
            CompletionStatus::Complete => ("Conversion complete", GOOD),
            CompletionStatus::Cancelled => ("Conversion cancelled", AMBER),
            CompletionStatus::Failed => ("Conversion stopped", Color32::LIGHT_RED),
        };
        Self::title(
            ui,
            title,
            "Review the project, retained files and conversion report.",
        );
        Self::card(ui, |ui| {
            if c.status == CompletionStatus::Failed {
                ui.colored_label(color, "See Activity for details.");
            } else {
                ui.colored_label(color, &c.message);
            }
            if let Some(path) = &c.output {
                review_path(ui, "Premiere project", path);
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
                review_path(ui, "Receipt", receipt);
            }
            if !c.retained_paths.is_empty() {
                egui::CollapsingHeader::new(format!("Retained files · {}", c.retained_paths.len()))
                    .show(ui, |ui| {
                        for path in &c.retained_paths {
                            review_path(ui, "Path", path);
                            let folder = if path.is_dir() {
                                path.as_path()
                            } else {
                                path.parent().unwrap_or(path)
                            };
                            if ui.small_button("Open folder").clicked() {
                                if let Err(e) = open_folder(folder) {
                                    self.error = Some(e);
                                }
                            }
                        }
                    });
            }
        });
        egui::CollapsingHeader::new(format!(
            "Conversion report · {}",
            counted(c.report.len(), "note", "notes")
        ))
        .show(ui, |ui| Self::report(ui, &c.report, Some(&self.completion_subjects), &[], &[],
            &mut self.report, &mut self.why));
        if ui.button("Convert another project").clicked() {
            self.invalidate();
            self.wizard.page = Page::Direction;
            self.wizard.invalidate();
        }
    }
    fn page_header(&mut self, ui: &mut egui::Ui) {
        let page = self.wizard.page;
        let (title, subtitle) = match page {
            Page::Direction => ("Choose what to transfer", ""),
            Page::Clips => ("Choose how to link media", "Avid Media Composer to Premiere Pro"),
            Page::Markers => (
                "Markers & colors",
                "Choose marker conflict rules and map Avid colors to Premiere's palettes.",
            ),
            Page::Effects => ("Effects", "See how effects in the selected Avid bins will translate."),
            Page::Review => ("Review conversion", "Avid Media Composer to Premiere Pro"),
        };
        if page == Page::Direction {
            #[cfg(target_os = "macos")]
            let resolve_available = self.resolve_templates.is_some() && !self.busy;
            #[cfg(not(target_os = "macos"))]
            let resolve_available = false;
            let _resolve_selected = direction::show(ui, resolve_available);
            #[cfg(target_os = "macos")]
            if _resolve_selected && let Some(templates) = &self.resolve_templates {
                self.resolve_wizard = Some(crate::avid_resolve::Wizard::new(
                    templates.clone(),
                    &self.wizard.inputs,
                    self.ffmpeg.clone(),
                    self.ffprobe.clone(),
                ));
            }
        } else {
            ui.label(RichText::new(subtitle).color(MUTED).small());
        }
        ui.add_space(3.);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 32.),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                self.navigation(ui, page);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(title).heading().strong()).truncate());
                });
            },
        );
        if self.busy {
            self.show_progress(ui);
        }
        ui.add_space(5.);
    }

    fn show_progress(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.strong(&self.task);
        });
        if let Some(p) = &self.progress {
            if !p.message.is_empty() {
                let message = p.current_bin.as_ref().map_or_else(
                    || p.message.clone(),
                    |path| p.message.replace(&*path.to_string_lossy(), &path_name(path)),
                );
                ui.add(egui::Label::new(RichText::new(message).small().color(MUTED)).truncate())
                    .on_hover_text(&p.message);
            }
            if p.total > 0 {
                let fraction = (p.completed as f32 / p.total as f32).clamp(0., 1.);
                ui.add(egui::ProgressBar::new(fraction).fill(ACCENT).desired_height(3.))
                    .on_hover_text(format!("{} · {} / {}", p.stage, p.completed, p.total));
            }
        }
    }

    fn navigation(&mut self, ui: &mut egui::Ui, page: Page) {
        if self.busy {
            if !self.task.starts_with("Choosing")
                && self.task != SAVING_PREFLIGHT
                && ui.add_enabled(!self.cancel.is_cancelled(), egui::Button::new("Cancel")).clicked()
            {
                self.cancel.cancel();
                self.notice = Some("Cancellation requested. Waiting for the current safe boundary.".into());
            }
            return;
        }
        let primary = |label| {
            egui::Button::new(RichText::new(label).strong().color(PRIMARY_INK))
                .fill(PRIMARY)
                .min_size(egui::vec2(94., 32.))
                .corner_radius(9)
        };
        if page == Page::Review {
            if self.wizard.can_convert() && self.prepared.is_some() {
                if ui.add_enabled(self.media_linking_ready(), primary("Convert")).clicked() {
                    self.convert(ui.ctx());
                }
                if !self.busy && ui.button("Check again").clicked() {
                    self.prepare(ui.ctx());
                }
            } else if ui
                .add_enabled(
                    self.wizard.inputs.check_required_paths().is_ok(),
                    primary(if self.review_blocker.is_some() { "Check again" } else { "Run review" }),
                )
                .clicked()
            {
                self.prepare(ui.ctx());
            }
        } else {
            let (label, next) = if page == Page::Direction {
                ("Continue", Page::Clips)
            } else {
                ("Run review", Page::Review)
            };
            if ui.add(primary(label)).clicked() {
                self.open_step(next, ui.ctx());
            }
        }
        if !self.busy && page != Page::Direction && ui.button("Back").clicked() {
            self.wizard.page = if page == Page::Clips { Page::Direction } else { Page::Clips };
        }
    }

    fn open_step(&mut self, page: Page, ctx: &egui::Context) {
        if page == Page::Direction || self.require_project_paths() {
            self.wizard.page = page;
            if page == Page::Review && self.prepared.is_none() && self.review_blocker.is_none() {
                self.prepare(ctx);
            }
        }
    }

    fn step(&mut self, ui: &mut egui::Ui, page: Page, name: &str, icon: Option<Glyph>) {
        let active = self.wizard.page == page;
        let color = if active { ACCENT } else { MUTED };
        let label = RichText::new(name).color(color);
        let label = if active { label.font(semibold(13.)) } else { label };
        let button = match icon {
            Some(icon) => egui::Button::image_and_text(icon.image(ui, 16., color), label),
            None => egui::Button::new(label),
        };
        if ui
            .add_enabled(
                !self.busy && self.completion.is_none(),
                button.selected(active)
                    .frame_when_inactive(active)
                    .right_text("")
                    .min_size(egui::vec2(ui.available_width(), 31.))
                    .corner_radius(8),
            )
            .clicked()
        {
            self.open_step(page, ui.ctx());
        }
    }

    fn steps(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            egui::Frame::new().fill(SELECTED).corner_radius(7).inner_margin(6.).show(ui, |ui| {
                ui.add(Glyph::Layers.image(ui, 15., ACCENT));
            });
            ui.label(RichText::new("Any NLE").font(semibold(14.)));
        });
        ui.add_space(13.);
        ui.spacing_mut().item_spacing.y = 4.;
        self.step(ui, Page::Direction, "Project", Some(Glyph::Project));
        self.step(ui, Page::Clips, "Media", Some(Glyph::Media));
        self.step(ui, Page::Review, "Review", Some(Glyph::Review));

        ui.add_space(10.);
        let advanced = matches!(self.wizard.page, Page::Markers | Page::Effects);
        egui::CollapsingHeader::new("Advanced settings")
            .open(advanced.then_some(true))
            .show_unindented(ui, |ui| {
                self.step(ui, Page::Markers, Page::Markers.label(), None);
                self.step(ui, Page::Effects, Page::Effects.label(), None);
            });
        ui.add_space(6.);
        ui.add_enabled_ui(!self.busy, |ui| {
            egui::CollapsingHeader::new("Saved settings").show(ui, |ui| {
                if ui.button("Load preset…").clicked() {
                    self.pick(Pick::LoadPreset, ui.ctx());
                }
                ui.add(egui::TextEdit::singleline(&mut self.preset_name)
                    .hint_text("Preset name")
                    .desired_width(ui.available_width()));
                if ui.button("Save preset…").clicked() {
                    self.pick(Pick::SavePreset, ui.ctx());
                }
                ui.add_space(6.);
                if ui.button("Save job settings…").clicked() {
                    self.pick(Pick::SaveJob, ui.ctx());
                }
                if ui.button("Load saved job…").clicked() {
                    self.pick(Pick::LoadJob, ui.ctx());
                }
            });
            #[cfg(target_os = "macos")]
            if ui
                .link("Relink media…")
                .on_hover_text("Relink media in an existing project")
                .clicked()
            {
                self.relink.open = true;
            }
        });
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
                ui.horizontal(|ui|{if ui.button("Keep working").clicked(){self.close_confirmation=false;}if ui.button("Cancel job & close when safe").clicked(){self.cancel.cancel();self.close_when_idle=true;self.close_confirmation=false;}});
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
        #[cfg(target_os = "macos")]
        if let Some(resolve) = &mut self.resolve_wizard {
            if resolve.show(ui, true) {
                self.resolve_wizard = None;
            }
            return;
        }
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && self.busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.task == SAVING_PREFLIGHT {
                self.close_when_idle = true;
            } else if !self.close_when_idle {
                self.close_confirmation = true;
            }
        }
        self.poll(&ctx);
        if let Some(error) = self.error.take() {
            self.activity.record(error, false);
        }
        if self.wizard.page == Page::Review {
            self.require_project_paths();
            #[cfg(target_os = "macos")]
            if let Some(media) = self.prepared.as_ref().and_then(|job| job.review().media_linking.as_ref()) {
                if !media.premiere_files.is_empty() {
                    self.premiere_status.refresh(&ctx, media);
                }
            }
        }
        egui::Panel::left("steps")
            .resizable(false)
            .exact_size(162.)
            .frame(
                egui::Frame::new()
                    .fill(RAIL)
                    .inner_margin(egui::Margin::symmetric(11, 18)),
            )
            .show_separator_line(false)
            .show(ui, |ui| {
                egui::Panel::bottom("activity-footer")
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(RAIL))
                    .show(ui, |ui| self.activity.button(ui));
                egui::ScrollArea::vertical()
                    .id_salt("step-scroll")
                    .show(ui, |ui| self.steps(ui));
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(CANVAS)
                    .inner_margin(egui::Margin::symmetric(25, 16)),
            )
            .show(ui, |ui| {
                if let Some(notice) = &self.notice {
                    ui.label(RichText::new(notice).color(AMBER));
                }
                let before = self.wizard.snapshot();
                if self.completion.is_none() || self.busy {
                    self.page_header(ui);
                }
                if self.wizard.page == Page::Direction {
                    ui.add_enabled_ui(!self.busy, |ui| self.direction(ui));
                } else {
                    egui::ScrollArea::vertical()
                    .id_salt((
                        "page-scroll",
                        self.wizard.page.index(),
                        self.completion.as_ref().map(|_| self.wizard.revision()),
                    ))
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!self.busy, |ui| match self.wizard.page {
                            Page::Direction => unreachable!(),
                            Page::Clips => self.clips(ui),
                            Page::Markers => self.markers(ui),
                            Page::Effects => self.effects(ui),
                            Page::Review => self.review(ui),
                        });
                    });
                }
                if self.apply_settings_changes(&before)
                    && self.remember_choices
                    && before.choices != self.wizard.choices
                    && let Err(e) = storage::save(&self.wizard.choices)
                {
                    self.error = Some(e);
                }
            });
        if let Some(error) = self.error.take() {
            self.activity.record(error, false);
            ctx.request_repaint();
        }
        self.activity.show(&ctx);
        self.help_window(&ctx);
        #[cfg(target_os = "macos")]
        self.relink.show(&ctx, self.ffmpeg.as_deref(), self.ffprobe.as_deref());
    }
}
fn column(ui: &mut egui::Ui, width: f32, content: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            content(ui);
        },
    );
}

fn recovery_button(
    ui: &mut egui::Ui,
    label: &str,
    description: impl FnOnce(&mut egui::Ui),
) -> bool {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_width = ui.fonts_mut(|fonts| fonts.layout_no_wrap(label.into(), font, INK).size().x);
    let button_width = text_width + 2. * ui.spacing().button_padding.x + 2.;
    let body_width = ui.available_width() - button_width - ui.spacing().item_spacing.x;
    if body_width >= 160. {
        ui.horizontal_top(|ui| {
            column(ui, body_width, description);
            ui.button(label).clicked()
        })
        .inner
    } else {
        description(ui);
        ui.button(label).clicked()
    }
}

pub(crate) fn source_list<R>(
    ui: &mut egui::Ui,
    id: impl egui::AsIdSalt,
    height: Option<f32>,
    rows: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    ui.scope(|ui| {
        let spacing = ui.spacing_mut();
        spacing.item_spacing.y = 2.;
        spacing.interact_size.y = 18.;
        spacing.scroll.floating = false;
        spacing.scroll.bar_width = 7.;
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(height.map_or(SOURCE_LIST_HEIGHT, |height| height.clamp(48., 300.)))
            .auto_shrink([false, true])
            .show(ui, rows)
    })
    .inner
}

fn counted(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}
fn path_name(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
}
fn review_path(ui: &mut egui::Ui, label: &str, path: &Path) {
    let show_path = |ui: &mut egui::Ui| {
        ui.label(RichText::new(label).color(MUTED));
        let full = path.display().to_string();
        ui.add(egui::Label::new(path_name(path)).truncate())
            .on_hover_text(&full)
            .context_menu(|ui| {
                if ui.button("Copy full path").clicked() {
                    ui.ctx().copy_text(full);
                    ui.close();
                }
            });
    };
    if ui.available_width() < 220. {
        ui.vertical(show_path);
    } else {
        ui.horizontal(show_path);
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let full = p.display().to_string();
                    ui.add(egui::Label::new(&full).truncate()).on_hover_text(full);
                });
            });
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

#[cfg(test)]
pub(crate) mod report_view_tests {
    use super::*;

    pub(crate) fn text_center(output: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(shape) if shape.galley.job.text == text => {
                Some(shape.pos + shape.galley.rect.center().to_vec2())
            }
            _ => None,
        })
    }

    pub(crate) fn click(pos: egui::Pos2) -> egui::RawInput {
        let pointer = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        egui::RawInput {
            events: vec![egui::Event::PointerMoved(pos), pointer(true), pointer(false)],
            ..Default::default()
        }
    }

    #[test]
    fn missing_project_paths_highlight_only_the_blocking_section() {
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        app.wizard = Wizard::default();
        app.error = None;
        let mut frame = eframe::Frame::_new_kittest();
        let mut draw = |app: &mut App, input: egui::RawInput| {
            let mut output = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
            output.textures_delta.clear();
            output
        };
        let highlighted = |output: &egui::FullOutput, label| {
            let text = text_center(output, label).expect("Project section heading");
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == ERROR_SURFACE && rect.rect.contains(text))
            })
        };
        let initial = draw(&mut app, egui::RawInput::default());
        assert!(!highlighted(&initial, "Source bins"));
        assert!(!highlighted(&initial, "Destination"));

        for button in ["Continue", "Review"] {
            app.highlight_required_input = false;
            let current = draw(&mut app, egui::RawInput::default());
            let blocked = draw(&mut app, click(text_center(&current, button).unwrap()));
            assert_eq!(app.wizard.page, Page::Direction);
            assert!(app.error.is_none());
            assert!(highlighted(&blocked, "Source bins"), "{button}");
            assert!(!highlighted(&blocked, "Destination"), "{button}");
            assert!(text_center(&blocked, RequiredInput::Bins.message()).is_none());
        }

        app.wizard.inputs.bins.push(PathBuf::from("source.avb"));
        let missing_output = draw(&mut app, egui::RawInput::default());
        assert!(!highlighted(&missing_output, "Source bins"));
        assert!(highlighted(&missing_output, "Destination"));
        assert!(text_center(&missing_output, RequiredInput::Output.message()).is_none());

        app.wizard.inputs.output = PathBuf::from("output.mov");
        let wrong_extension = draw(&mut app, egui::RawInput::default());
        assert!(highlighted(&wrong_extension, "Destination"));
        app.wizard.inputs.output.set_extension("prproj");
        let ready = draw(&mut app, egui::RawInput::default());
        assert!(!highlighted(&ready, "Source bins"));
        assert!(!highlighted(&ready, "Destination"));
        draw(&mut app, click(text_center(&ready, "Continue").unwrap()));
        assert_eq!(app.wizard.page, Page::Clips);

        let media = draw(&mut app, egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760., 520.))),
            ..Default::default()
        });
        let title = text_center(&media, "Choose how to link media").unwrap();
        let back = text_center(&media, "Back").unwrap();
        let review = text_center(&media, "Run review").unwrap();
        assert!(title.x < back.x && back.x < review.x && (title.y - review.y).abs() < 6.);
    }

    #[test]
    fn preflight_recovery_opens_the_correct_media_panel() {
        for (kind, button, expected) in [
            (BlockerKind::RewrapMissingMedia, "Locate media", MediaJump::Locations),
            (BlockerKind::RewrapMissingOriginal, "Open saved-path mappings", MediaJump::OriginalMappings),
            (BlockerKind::LegacyMxfPackage, "Open media locations", MediaJump::Locations),
            (BlockerKind::OriginalLocator, "Open saved-path mappings", MediaJump::OriginalMappings),
        ] {
            let issue = ReviewBlocker::new(kind, "Synthetic preflight issue");
            let ctx = egui::Context::default();
            let mut browser = ReportBrowser::default();
            let mut initial = ctx.run_ui(egui::RawInput::default(), |ui| {
                App::blocker_card(ui, &issue, &mut browser);
            });
            initial.textures_delta.clear();
            let can_switch = initial.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(shape)
                    if shape.galley.job.text == "Switch to Separate files")
            });
            assert_eq!(
                can_switch,
                matches!(kind, BlockerKind::RewrapMissingMedia | BlockerKind::RewrapMissingOriginal)
            );
            let button_center = text_center(&initial, button).expect("recovery button");
            assert!(text_center(&initial, "Synthetic preflight issue").is_none());
            assert!(initial.shapes.iter().any(|shape| matches!(
                &shape.shape, egui::Shape::Rect(rect)
                    if rect.fill == ERROR_SURFACE && rect.rect.contains(button_center)
            )), "{kind:?}");
            let mut selected = None;
            let mut output = ctx.run_ui(click(button_center), |ui| {
                selected = App::blocker_card(ui, &issue, &mut browser);
            });
            output.textures_delta.clear();
            assert!(matches!(selected, Some(ReviewAction::Media(jump)) if jump == expected), "{kind:?}");
        }
    }

    #[test]
    fn source_bin_tree_indents_and_selects_duplicate_names_by_path() {
        let relative = ["Editorial/Sequences/Selects.avb", "Sound/Selects.avb"];
        let catalog: Vec<_> = relative
                .iter()
                .map(|path| BinPath {
                    path: Path::new("project").join(path),
                    relative: path.into(),
                })
                .collect();
        let ctx = egui::Context::default();
        App::theme(&ctx);
        let tree = Tree::build(&catalog);
        let mut expansion = FolderExpansion::default();
        let mut render = |raw, paths: &mut Vec<PathBuf>| {
            let mut frame = ctx.run_ui(raw, |ui| {
                let mut selected = BinSelection::new(paths);
                source_list(ui, "test-tree", None, |ui| {
                    tree.show(ui, &catalog, &mut selected, None, false, &mut expansion);
                });
            });
            frame.textures_delta.clear();
            frame
        };
        let mut bins = vec![];
        let first = render(egui::RawInput::default(), &mut bins);
        let editorial = text_center(&first, "Editorial").unwrap();
        let duplicate_labels: Vec<_> = first
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(shape) if shape.galley.job.text == "Selects.avb" => {
                    Some(shape.pos + shape.galley.rect.center().to_vec2())
                }
                _ => None,
            })
            .collect();
        assert_eq!(duplicate_labels.len(), 2);
        assert!(duplicate_labels[0].x > duplicate_labels[1].x);
        render(click(duplicate_labels[1]), &mut bins);
        assert_eq!(bins, [catalog[1].path.clone()]);
        render(click(editorial), &mut bins);
        assert_eq!(
            bins,
            [
                catalog[1].path.clone(),
                catalog[0].path.clone()
            ]
        );
        render(click(editorial), &mut bins);
        assert_eq!(bins, [catalog[1].path.clone()]);
    }

    #[test]
    fn large_bin_tree_preserves_space_and_can_scroll_filtered_results() {
        let catalog: Vec<_> = (0..40)
                .map(|i| {
                    let relative = PathBuf::from(format!("Editorial/Reels/Bin {i:02}.avb"));
                    BinPath {
                        path: Path::new("project").join(&relative),
                        relative,
                    }
                })
                .collect();
        let ctx = egui::Context::default();
        App::theme(&ctx);
        let tree = Tree::build(&catalog);
        let mut bins = vec![];
        let mut layout = None;
        let mut expansion = FolderExpansion::default();
        let matching = [true; 40];
        let mut draw = |input, matches| {
            let mut frame = ctx.run_ui(input, |ui| {
                let mut selected = BinSelection::new(&mut bins);
                layout = Some(source_list(ui, "test-scroll-tree", None, |ui| {
                    tree.show(ui, &catalog, &mut selected, matches, false, &mut expansion);
                }));
            });
            frame.textures_delta.clear();
            (frame, layout.take().unwrap())
        };
        let (folded, short) = draw(egui::RawInput::default(), None);
        assert!(text_center(&folded, "Editorial").is_some());
        assert!(text_center(&folded, "Bin 00.avb").is_none());
        assert!(short.content_size.y <= short.inner_rect.height());
        let (expanded, long) = draw(egui::RawInput::default(), Some(matching.as_slice()));
        assert!(text_center(&expanded, "Bin 00.avb").is_some());
        assert!(long.inner_rect.height() <= SOURCE_LIST_HEIGHT);
        assert!(long.content_size.y > SOURCE_LIST_HEIGHT);
        let (_, scrolled) = draw(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(long.inner_rect.center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0., -100.),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            Some(matching.as_slice()),
        );
        assert!(scrolled.state.clamped_offset().y > 0.);
    }

    #[test]
    fn review_places_output_beside_findings_when_the_window_has_room() {
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        app.wizard = Wizard::default();
        app.wizard.page = Page::Review;
        app.wizard.inputs.bins = vec!["Source.avb".into()];
        app.wizard.inputs.output = "Converted.prproj".into();
        app.review_blocker = Some(ReviewBlocker::new(
            BlockerKind::RewrapMissingMedia,
            "The source file is missing.",
        ));
        app.error = None;
        let mut frame = eframe::Frame::_new_kittest();
        for (width, side_by_side) in [(880., true), (760., false)] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 620.),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            output.textures_delta.clear();
            let finding = text_center(&output, "Required media is missing").unwrap();
            let summary = text_center(&output, "Output").unwrap();
            let fix = text_center(&output, "Locate media").unwrap();
            let title = text_center(&output, "Review conversion").unwrap();
            let recheck = text_center(&output, "Check again").unwrap();
            assert_eq!(summary.x > finding.x, side_by_side);
            if side_by_side {
                assert!((summary.y - finding.y).abs() < 4.);
            } else {
                assert!(summary.y > finding.y);
            }
            assert!(fix.x > finding.x && (fix.y - finding.y).abs() < 6.);
            assert!(recheck.x > title.x && (recheck.y - title.y).abs() < 6.);
            assert!(recheck.y < finding.y);
        }
    }

    #[test]
    fn populated_project_fits_destination_at_minimum_window_height() {
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        app.wizard = Wizard::default();
        app.wizard.inputs.project_folder = Some("project".into());
        app.catalog = Some((0..40).map(|i| {
            let relative = PathBuf::from(format!("Reels {i:02}/Source.avb"));
            BinPath { path: Path::new("project").join(&relative), relative }
        }).collect());
        app.error = None;
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760., 520.))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        output.textures_delta.clear();
        let destination = text_center(&output, "Project name (optional)").unwrap();
        let heading = text_center(&output, "Choose what to transfer").unwrap();
        let next = text_center(&output, "Continue").unwrap();
        assert!(next.x > heading.x && (next.y - heading.y).abs() < 6.);
        assert!(destination.y > next.y + 22. && destination.y < 500.);
        assert!(text_center(&output, "Use recommended mode").is_none());
    }

    #[test]
    fn cancellation_skips_error_log_and_only_critical_failure_opens_modal() {
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        app.error = None;
        app.cancel.cancel();
        app.tx.send(Event::Inventory(app.wizard.revision(), Err("private path: cancelled".into()))).unwrap();
        app.poll(&ctx);
        assert_eq!(app.notice.as_deref(), Some("Cancelled."));
        assert!(app.error.is_none() && app.activity.errors.is_empty());
        app.activity.record("Ordinary failure".into(), false);
        assert!(app.activity.unread && !app.activity.critical);
        app.tx.send(Event::WorkerFailed("Critical failure".into())).unwrap();
        app.poll(&ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let mut draw = |time| {
            let input = egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(880., 620.))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            output.textures_delta.clear();
            output
        };
        draw(0.);
        let output = draw(0.25);
        assert!(app.activity.critical);
        assert!(text_center(&output, "Conversion stopped").is_some());
        assert!(text_center(&output, "Critical failure").is_none());
    }

    #[test]
    fn blocked_preflight_report_searches_all_pages_and_copies_complete_details() {
        let mut issue = ReviewBlocker::new(BlockerKind::AvidMedia, "Resolve missing media");
        issue.affected = (1..=53).map(|index| format!("Missing file {index}")).collect();
        let target = "MXF Café slate package 52";
        issue.affected[51] = target.into();
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.);
        let mut browser = ReportBrowser::default();
        let mut frame = |raw| {
            let mut output = ctx.run_ui(raw, |ui| {
                App::blocker_card(ui, &issue, &mut browser);
            });
            output.textures_delta.clear();
            output
        };
        let initial = frame(egui::RawInput::default());
        assert!(text_center(&initial, "Details 1–25 of 53").is_none());
        assert!(text_center(&initial, target).is_none());
        let expanded = frame(click(
            text_center(&initial, "Affected details · 53").unwrap(),
        ));
        assert!(text_center(&expanded, "Save preflight report").is_some());
        assert!(text_center(&expanded, "Details 1–25 of 53").is_some());
        let second = frame(click(text_center(&expanded, "Next").unwrap()));
        assert!(text_center(&second, "Details 26–50 of 53").is_some());

        frame(click(text_center(&second, "Search affected details").unwrap()));
        let filtered = frame(egui::RawInput {
            events: vec![egui::Event::Text("  CAFÉ SLATE  ".into())],
            ..Default::default()
        });
        assert!(text_center(&filtered, "Details 1–1 of 1").is_some());
        assert!(text_center(&filtered, target).is_some());
        assert!(text_center(&filtered, "Missing file 1").is_none());
        let copied = frame(click(text_center(&filtered, "Copy details").unwrap()));
        let full = format!("{}\n{}", issue.message, issue.affected.join("\n"));
        assert!(
            copied.platform_output.commands.iter().any(|command| {
                matches!(command, egui::OutputCommand::CopyText(text) if text == &full)
            }),
            "clipboard commands: {:?}",
            copied.platform_output.commands
        );
    }

    #[test]
    fn blocked_timing_report_filters_short_lists_and_copies_every_detail() {
        let mut issue = ReviewBlocker::new(BlockerKind::SequenceTiming, "Unsupported timing");
        issue.affected = vec!["First edit · V1 · frame 12".into(), "Second edit · V2 · frame 18".into()];
        issue.timeline_groups = ["First/Scenes.avb", "Second/Scenes.avb"].into_iter().enumerate()
            .map(|(index, bin)| job::ReviewGroup {
                id: bin.into(),
                name: "Assembly".into(),
                bin: Some(bin.into()),
                note_indices: vec![index],
            }).collect();
        let mut browser = ReportBrowser::default();
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.);
        let mut frame = |input| {
            let mut output = ctx.run_ui(input, |ui| {
                App::blocker_card(ui, &issue, &mut browser);
            });
            output.textures_delta.clear();
            output
        };
        let initial = frame(egui::RawInput::default());
        frame(click(text_center(&initial, "Affected details · 2").unwrap()));
        let expanded = frame(egui::RawInput::default());
        frame(click(text_center(&expanded, "All timelines").unwrap()));
        let menu = frame(egui::RawInput::default());
        assert!(text_center(&menu, "Assembly · First/Scenes.avb").is_some());
        frame(click(text_center(&menu, "Assembly · Second/Scenes.avb").unwrap()));
        let filtered = frame(egui::RawInput::default());
        assert!(text_center(&filtered, "First edit · V1 · frame 12").is_none());
        assert!(text_center(&filtered, "Second edit · V2 · frame 18").is_some());
        let copied = frame(click(text_center(&filtered, "Copy details").unwrap()));
        let expected = format!("{}\n{}", issue.message, issue.affected.join("\n"));
        assert!(copied.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == &expected)
        }));
    }

    #[test]
    fn report_view_draws_only_the_selected_page() {
        let notes: Vec<_> = (1..=53).map(|index| job::ReportNote {
            severity: "loss".into(),
            object: format!("Edit {index}"),
            message: format!("Report message {index}"),
            why: String::new(),
        }).collect();
        let mut report = ReportBrowser::new(&notes);
        let ctx = egui::Context::default();
        let mut why = None;
        for (page, expected, visible, hidden) in [
            (0, "Notes 1–25 of 53", "Report message 1", "Report message 53"),
            (2, "Notes 51–53 of 53", "Report message 53", "Report message 1"),
        ] {
            report.page = page;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                App::report(ui, &notes, None, &[], &[], &mut report, &mut why);
            });
            output.textures_delta.clear();
            let shown = |text| output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(shape) if shape.galley.job.text == text)
            });
            assert!(shown(expected));
            assert!(shown(visible));
            assert!(!shown(hidden));
        }
    }

    #[test]
    fn preflight_report_timeline_menu_distinguishes_matching_names() {
        let notes = ["First timeline finding", "Second timeline finding"].map(|message| job::ReportNote {
            severity: "loss".into(),
            object: message.into(),
            message: message.into(),
            why: String::new(),
        });
        let timelines = ["First/Scenes.avb", "Second/Scenes.avb"].map(|bin| job::ReviewGroup {
            id: bin.into(),
            name: "Assembly".into(),
            bin: Some(bin.into()),
            note_indices: vec![usize::from(bin.starts_with("Second"))],
        });
        let sources = [job::ReviewGroup {
            id: "source".into(), name: "Camera A".into(), bin: None, note_indices: vec![0],
        }];
        let mut browser = ReportBrowser::new(&notes);
        let mut why = None;
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.);
        let mut frame = |input| {
            let mut output = ctx.run_ui(input, |ui| {
                App::report(ui, &notes, None, &timelines, &sources, &mut browser, &mut why);
            });
            output.textures_delta.clear();
            output
        };
        let initial = frame(egui::RawInput::default());
        frame(click(text_center(&initial, "All timelines").unwrap()));
        let menu = frame(egui::RawInput::default());
        assert!(text_center(&menu, "Assembly · First/Scenes.avb").is_some());
        frame(click(text_center(&menu, "Assembly · Second/Scenes.avb").unwrap()));
        let filtered = frame(egui::RawInput::default());
        assert!(text_center(&filtered, "First timeline finding").is_none());
        assert!(text_center(&filtered, "Second timeline finding").is_some());
        frame(click(text_center(&filtered, "All source clips").unwrap()));
        let source_menu = frame(egui::RawInput::default());
        frame(click(text_center(&source_menu, "Camera A").unwrap()));
        let combined = frame(egui::RawInput::default());
        assert!(text_center(&combined, "No notes match these filters.").is_some());
    }
}

#[cfg(all(test, target_os = "macos", feature = "live-premiere-tests"))]
mod installed_helper_tests {
    use super::*;
    use super::report_view_tests::{click, text_center};

    #[test]
    fn qualify_installed_premiere_helper_from_wizard() {
        let request = PathBuf::from(std::env::var_os("ANYNLE_WIZARD_ASSISTED_JOB")
            .expect("set ANYNLE_WIZARD_ASSISTED_JOB to a disposable job"));
        assert_eq!(anynle_app::premiere_import::worker_status(),
            anynle_app::premiere_import::WorkerState::Ready);
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), Some(request));
        let mut frame = eframe::Frame::_new_kittest();
        let mut draw = |app: &mut App, mut input: egui::RawInput| {
            input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100., 850.)));
            let mut output = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
            output.textures_delta.clear();
            output
        };
        app.wizard.page = Page::Review;
        app.prepare(&ctx);
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while app.busy {
            assert!(std::time::Instant::now() < deadline, "preflight timed out");
            draw(&mut app, egui::RawInput::default());
            std::thread::sleep(Duration::from_millis(10));
        }
        let normal = draw(&mut app, egui::RawInput::default());
        assert!(text_center(&normal, "Media linking").is_some());
        assert!(text_center(&normal, "Ready · 1 original will use Premiere.").is_some());
        let preflight = app.prepared.as_ref().expect("normal wizard preflight");
        preflight.save_preflight_report(&app.wizard.inputs.output.with_extension("preflight.json")).unwrap();
        assert!(!app.wizard.inputs.output.exists());
        assert!(app.media_linking_ready());
        draw(&mut app, click(text_center(&normal, "Convert").expect("Convert button")));
        assert!(app.busy);
        while app.busy {
            assert!(std::time::Instant::now() < deadline, "conversion timed out");
            draw(&mut app, egui::RawInput::default());
            std::thread::sleep(Duration::from_millis(10));
        }
        let completion = app.completion.expect("normal wizard conversion");
        assert_eq!(completion.status, CompletionStatus::Complete, "{}", completion.message);
        assert!(completion.report.iter().any(|note| note.message.starts_with("Used Premiere to link ")));
    }
}
