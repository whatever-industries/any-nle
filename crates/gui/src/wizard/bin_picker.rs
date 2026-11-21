//! Project bin browsing and selection. The wizard owns job inputs and discovery.
use super::{MUTED, counted, path_name, source_list};
use crate::model::Inputs;
use anynle_app::job::BinPath;
use eframe::egui::{self, RichText};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const PROJECT_TAIL_SPACE: f32 = 156.;

#[derive(Default)]
pub(super) struct BinPicker {
    catalog: Option<Vec<BinPath>>,
    expansion: FolderExpansion,
    search: String,
    #[cfg(feature = "wizard-automation")]
    pub(super) source_viewport: Option<egui::Rect>,
}

impl BinPicker {
    pub(super) fn catalog(&self) -> Option<&[BinPath]> {
        self.catalog.as_deref()
    }

    pub(super) fn set_catalog(&mut self, catalog: Vec<BinPath>) {
        self.catalog = Some(catalog);
    }

    pub(super) fn clear_catalog(&mut self) {
        self.catalog = None;
    }

    pub(super) fn reset_expansion(&mut self) {
        self.expansion = FolderExpansion::default();
    }

    pub(super) fn search_field(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search bins and folders")
                .desired_width(220.),
        );
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, inputs: &mut Inputs) {
        #[cfg(feature = "wizard-automation")]
        {
            self.source_viewport = None;
        }
        if let Some(catalog) = &self.catalog {
            let search = self.search.trim().to_lowercase();
            let matches: Option<Vec<_>> = (!search.is_empty()).then(|| {
                catalog
                    .iter()
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
                let _list = source_list(
                    ui,
                    ("bin-tree", &inputs.project_folder),
                    Some(height),
                    |ui| {
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
                    },
                );
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
                if inputs.entire_project {
                    catalog.len()
                } else {
                    inputs.bins.len()
                }
            ));
        } else if !inputs.bins.is_empty() {
            let mut remove = None;
            let height = ui.available_height() - PROJECT_TAIL_SPACE;
            source_list(ui, "picked-bins", Some(height), |ui| {
                for (i, p) in inputs.bins.iter().enumerate() {
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
                inputs.bins.remove(i);
            }
        }
    }

    #[cfg(feature = "wizard-automation")]
    pub(super) fn found_bins(&self) -> Result<&[BinPath], String> {
        self.catalog().ok_or_else(|| "Run find_bins first.".into())
    }

    #[cfg(feature = "wizard-automation")]
    pub(super) fn set_folder_expanded(
        &mut self,
        path: &Path,
        expanded: bool,
    ) -> Result<(), String> {
        let tree = Tree::build(self.found_bins()?);
        if path.as_os_str().is_empty() {
            return Err("Choose a catalog folder by its project-relative path.".into());
        }
        let mut node = &tree;
        for part in path.components() {
            node = node
                .folders
                .get(part.as_os_str())
                .ok_or_else(|| format!("Unknown catalog folder: {}", path.display()))?;
        }
        self.expansion.set_open(&node.relative, false, expanded);
        Ok(())
    }

    #[cfg(feature = "wizard-automation")]
    pub(super) fn set_bin_selected(
        &self,
        inputs: &mut Inputs,
        path: &Path,
        selected: bool,
    ) -> Result<(), String> {
        if inputs.entire_project {
            return Err(
                "Turn off entire-project selection before selecting individual bins.".into(),
            );
        }
        let bin = select_bin(self.found_bins()?, path)?;
        BinSelection::new(&mut inputs.bins).set(std::iter::once(&bin.path), selected);
        Ok(())
    }

    #[cfg(feature = "wizard-automation")]
    pub(super) fn folders(&self, bins: &[BinPath]) -> Vec<(PathBuf, bool)> {
        fn visit(tree: &Tree, folders: &mut Vec<PathBuf>) {
            for folder in tree.folders.values() {
                folders.push(folder.relative.clone());
                visit(folder, folders);
            }
        }
        let mut folders = Vec::new();
        visit(&Tree::build(bins), &mut folders);
        folders
            .into_iter()
            .map(|path| {
                let expanded = self.expansion.is_open(&path, bins.len(), false);
                (path, expanded)
            })
            .collect()
    }
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
        let state = if filtering {
            &self.search
        } else {
            &self.browse
        };
        state
            .get(path)
            .copied()
            .unwrap_or(filtering || bin_count <= 16)
    }

    fn set_open(&mut self, path: &Path, filtering: bool, open: bool) {
        let state = if filtering {
            &mut self.search
        } else {
            &mut self.browse
        };
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
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                open,
            );
            state.set_open(open);
            let header = state.show_header(ui, |ui| {
                let bins = || indices.iter().map(|&i| &catalog[i].path);
                let mut all = entire || bins().all(|path| selected.checked.contains(path));
                let partial = !all && bins().any(|path| selected.checked.contains(path));
                let checkbox =
                    egui::Checkbox::new(&mut all, name.to_string_lossy()).indeterminate(partial);
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

#[cfg(any(test, feature = "wizard-automation"))]
fn select_bin<'a>(catalog: &'a [BinPath], path: &Path) -> Result<&'a BinPath, String> {
    let mut matches = catalog.iter().filter(|bin| {
        if path.is_absolute() {
            bin.path == path
        } else {
            bin.relative == path
        }
    });
    let bin = matches
        .next()
        .ok_or_else(|| format!("Unknown catalog bin: {}", path.display()))?;
    if matches.next().is_some() {
        return Err(format!(
            "Ambiguous catalog bin: {}. Use its full path.",
            path.display()
        ));
    }
    Ok(bin)
}

#[cfg(test)]
mod tests {
    use super::super::{
        App, SOURCE_LIST_HEIGHT,
        report_view_tests::{click, text_center},
    };
    use super::*;

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
        assert_eq!(bins, [catalog[1].path.clone(), catalog[0].path.clone()]);
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
    fn bin_selectors_require_an_exact_unique_catalog_entry() {
        let catalog = ["Editorial", "Sound"].map(|folder| BinPath {
            path: Path::new("project").join(folder).join("Selects.avb"),
            relative: "Selects.avb".into(),
        });
        assert!(
            select_bin(&catalog, Path::new("Selects.avb"))
                .unwrap_err()
                .contains("Ambiguous")
        );
        assert!(
            select_bin(&catalog, Path::new("Other.avb"))
                .unwrap_err()
                .contains("Unknown")
        );
    }

    #[test]
    fn search_keeps_browse_folder_expansion() {
        let context = egui::Context::default();
        App::theme(&context);
        let folder = Path::new("Editorial");
        let mut inputs = Inputs::default();
        let mut picker = BinPicker::default();
        picker.set_catalog(
            ["Editorial/Selects.avb", "Editorial/Assembly.avb"]
                .map(|path| BinPath {
                    path: Path::new("project").join(path),
                    relative: path.into(),
                })
                .to_vec(),
        );
        picker.expansion.set_open(folder, false, false);
        let mut draw = |picker: &mut BinPicker| {
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                picker.show(ui, &mut inputs);
            });
            output.textures_delta.clear();
            output
        };

        picker.search = "SELECTS".into();
        let filtered = draw(&mut picker);
        assert!(text_center(&filtered, "Selects.avb").is_some());
        assert!(text_center(&filtered, "Assembly.avb").is_none());
        picker.search.clear();
        let unfiltered = draw(&mut picker);
        assert!(text_center(&unfiltered, "Editorial").is_some());
        assert!(text_center(&unfiltered, "Selects.avb").is_none());
    }
}
