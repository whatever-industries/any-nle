//! Developer-only scenarios. All wizard actions run in an isolated session.
use super::{App, BinPath, BinSelection, Page, Tree, egui, job, storage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "automation/screenshot.rs"]
mod screenshot;

const SCHEMA: &str = "anynle.wizard-automation/1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    schema: String,
    actions: Vec<Action>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    LoadJob { path: PathBuf },
    FindBins,
    SetFolderExpanded { path: PathBuf, expanded: bool },
    SetEntireProject { enabled: bool },
    SetBinSelected { path: PathBuf, selected: bool },
    OpenMedia,
    RunReview,
    Snapshot,
    CaptureProjectLayout,
    CaptureProjectScreenshot { path: PathBuf },
    CaptureScreenshot { path: PathBuf },
}

#[derive(Serialize)]
struct Response {
    schema: &'static str,
    snapshots: Vec<State>,
    layouts: Vec<ProjectLayout>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    screenshots: Vec<Screenshot>,
}

#[derive(Serialize)]
struct State {
    page: &'static str,
    revision: u64,
    catalog: Option<Catalog>,
    selected_bins: Vec<PathBuf>,
    job: job::JobSpec,
    review: Option<Value>,
}

#[derive(Serialize)]
struct Catalog {
    bins: Vec<CatalogBin>,
    folders: Vec<CatalogFolder>,
}

#[derive(Serialize)]
struct CatalogBin {
    path: PathBuf,
    relative: PathBuf,
}

#[derive(Serialize)]
struct CatalogFolder {
    path: PathBuf,
    expanded: bool,
}

#[derive(Serialize)]
struct ProjectLayout {
    viewport: [f32; 2],
    source_list: [f32; 4],
    destination_label: [f32; 4],
    continue_label: [f32; 4],
}

#[derive(Serialize)]
struct Screenshot {
    path: PathBuf,
    width: u32,
    height: u32,
}

pub(crate) fn run(path: &Path) -> Result<(), String> {
    let data = storage::read_settings(path)?;
    let scenario = serde_json::from_str(&data).map_err(|err| format!("Invalid scenario: {err}"))?;
    let response = execute(scenario)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer(&mut out, &response).map_err(|err| err.to_string())?;
    out.write_all(b"\n")
        .and_then(|()| out.flush())
        .map_err(|err| err.to_string())
}

fn execute(scenario: Scenario) -> Result<Response, String> {
    if scenario.schema != SCHEMA {
        return Err("Unsupported scenario version.".into());
    }
    let ctx = egui::Context::default();
    let mut app = App::session(&ctx);
    let mut response = Response {
        schema: SCHEMA,
        snapshots: Vec::new(),
        layouts: Vec::new(),
        screenshots: Vec::new(),
    };
    for (index, action) in scenario.actions.into_iter().enumerate() {
        apply(action, &mut app, &ctx, &mut response)
            .map_err(|err| format!("Action {}: {err}", index + 1))?;
    }
    Ok(response)
}

fn apply(
    action: Action,
    app: &mut App,
    ctx: &egui::Context,
    response: &mut Response,
) -> Result<(), String> {
    match action {
        Action::LoadJob { path } => {
            app.load_job(&path);
            if let Some(error) = app.error.take() {
                return Err(error);
            }
        }
        Action::FindBins => {
            app.find_bins(ctx);
            wait_for_worker(app, ctx)?;
        }
        Action::SetFolderExpanded { path, expanded } => {
            let catalog = catalog(app)?;
            let tree = Tree::build(catalog);
            let mut node = &tree;
            if path.as_os_str().is_empty() {
                return Err("Choose a catalog folder by its project-relative path.".into());
            }
            for part in path.components() {
                node = node
                    .folders
                    .get(part.as_os_str())
                    .ok_or_else(|| format!("Unknown catalog folder: {}", path.display()))?;
            }
            app.expansion.set_open(&node.relative, false, expanded);
        }
        Action::SetEntireProject { enabled } => {
            let before = app.wizard.snapshot();
            app.wizard.inputs.entire_project = enabled;
            app.apply_settings_changes(&before);
        }
        Action::SetBinSelected { path, selected } => {
            if app.wizard.inputs.entire_project {
                return Err(
                    "Turn off entire-project selection before selecting individual bins.".into(),
                );
            }
            let path = select_bin(catalog(app)?, &path)?.path.clone();
            let before = app.wizard.snapshot();
            BinSelection::new(&mut app.wizard.inputs.bins).set(std::iter::once(&path), selected);
            app.apply_settings_changes(&before);
        }
        Action::OpenMedia | Action::RunReview => {
            if let Err(required) = app.wizard.inputs.check_required_paths() {
                return Err(required.message().into());
            }
            if matches!(action, Action::OpenMedia) {
                app.open_step(Page::Clips, ctx);
            } else {
                app.wizard.page = Page::Review;
                app.prepare(ctx);
                wait_for_worker(app, ctx)?;
            }
        }
        Action::Snapshot => response.snapshots.push(snapshot(app)?),
        Action::CaptureProjectLayout => response.layouts.push(project_layout(app, ctx)?),
        Action::CaptureProjectScreenshot { path } => {
            require_project_picker(app, "Capture a screenshot")?;
            response.screenshots.push(capture(app, path)?);
        }
        Action::CaptureScreenshot { path } => {
            match app.wizard.page {
                Page::Direction => require_project_picker(app, "Capture a screenshot")?,
                Page::Clips | Page::Review => {}
                _ => return Err("Capture a screenshot on Project, Media, or Review.".into()),
            }
            response.screenshots.push(capture(app, path)?);
        }
    }
    Ok(())
}

fn catalog(app: &App) -> Result<&[BinPath], String> {
    app.catalog
        .as_deref()
        .ok_or_else(|| "Run find_bins first.".into())
}

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

fn wait_for_worker(app: &mut App, ctx: &egui::Context) -> Result<(), String> {
    while app.busy {
        let event = app.rx.recv().map_err(|err| err.to_string())?;
        app.receive_events(ctx, Some(event));
    }
    app.error.take().map_or(Ok(()), Err)
}

fn snapshot(app: &App) -> Result<State, String> {
    let spec = app.spec();
    let review = if let Some(prepared) = &app.prepared {
        serde_json::to_value(prepared.preflight_report()).map(Some)
    } else if let Some(blocker) = &app.review_blocker {
        serde_json::to_value(job::blocked_preflight_report(&spec, blocker)).map(Some)
    } else {
        Ok(None)
    }
    .map_err(|err| err.to_string())?;
    let catalog = app.catalog.as_ref().map(|bins| {
        let mut folders = Vec::new();
        let tree = Tree::build(bins);
        list_folders(&tree, &mut folders);
        Catalog {
            bins: bins
                .iter()
                .map(|bin| CatalogBin {
                    path: bin.path.clone(),
                    relative: bin.relative.clone(),
                })
                .collect(),
            folders: folders
                .into_iter()
                .map(|path| CatalogFolder {
                    expanded: app.expansion.is_open(&path, bins.len(), false),
                    path,
                })
                .collect(),
        }
    });
    let selected_bins = if spec.entire_project {
        app.catalog
            .iter()
            .flatten()
            .map(|bin| bin.path.clone())
            .collect()
    } else {
        spec.bins.clone()
    };
    Ok(State {
        page: app.wizard.page.label(),
        revision: app.wizard.revision(),
        catalog,
        selected_bins,
        job: spec,
        review,
    })
}

fn list_folders(tree: &Tree, folders: &mut Vec<PathBuf>) {
    for folder in tree.folders.values() {
        folders.push(folder.relative.clone());
        list_folders(folder, folders);
    }
}

fn wizard_frame(
    app: &mut App,
    ctx: &egui::Context,
    screen: egui::Rect,
    pixels_per_point: f32,
) -> egui::FullOutput {
    let mut frame = eframe::Frame::_new_kittest();
    let mut draw = |time| {
        let mut input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(screen),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(pixels_per_point);
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
    };
    let mut output = draw(0.);
    // The settled frame still needs the fonts and icons allocated by the first frame.
    output.append(draw(0.25));
    output
}

fn require_project_picker(app: &App, operation: &str) -> Result<(), String> {
    if app.wizard.page != Page::Direction || catalog(app)?.is_empty() {
        return Err(format!("{operation} on Project after finding source bins."));
    }
    Ok(())
}

fn project_layout(app: &mut App, ctx: &egui::Context) -> Result<ProjectLayout, String> {
    require_project_picker(app, "Capture layout")?;
    let viewport = egui::vec2(760., 520.);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, viewport);
    let mut output = wizard_frame(app, ctx, screen, 1.);
    let shapes = std::mem::take(&mut output.shapes);
    output.drop_without_applying_deltas();
    let label = |name| {
        shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == name => {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    shape.clip_rect.contains_rect(rect).then_some(rect)
                }
                _ => None,
            })
            .ok_or_else(|| format!("Project layout has no fully visible {name} label."))
    };
    let source = app
        .source_viewport
        .ok_or("No source list in Project layout.")?;
    let destination = label("Destination")?;
    let next = label("Continue")?;
    if [source, destination, next]
        .iter()
        .any(|rect| !rect.is_positive() || !screen.contains_rect(*rect))
        || next.bottom() > source.top()
        || source.bottom() > destination.top()
    {
        return Err(format!(
            "Project layout does not fit at the minimum window size. Continue: {next:?}, bin list: {source:?}, Destination: {destination:?}."
        ));
    }
    let coords = |rect: egui::Rect| [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
    Ok(ProjectLayout {
        viewport: [viewport.x, viewport.y],
        source_list: coords(source),
        destination_label: coords(destination),
        continue_label: coords(next),
    })
}

fn capture(app: &mut App, path: PathBuf) -> Result<Screenshot, String> {
    #[cfg(target_os = "macos")]
    return screenshot::capture(app, path);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("Wizard screenshots require a macOS developer build.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    const EMPTY_BIN: &[u8] = include_bytes!("../../../avid-reader/tests/fixtures/pyavb_empty.avb");

    #[test]
    fn review_scenario_selects_whole_bins_and_invalidates_stale_review() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let project = root.join("project");
        let media = root.join("media");
        fs::create_dir(&media).unwrap();
        for path in ["Editorial/Reels/Selects.avb", "Sound/Selects.avb"] {
            let path = project.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, EMPTY_BIN).unwrap();
        }
        let output = root.join("destination.prproj");
        let settings = root.join("job.json");
        let spec = job::JobSpec {
            project_folder: Some(project.clone()),
            output: output.clone(),
            media_roots: vec![media],
            mode: job::ClipMode::Separate,
            include_dependencies: false,
            ffmpeg: Some("unused-ffmpeg".into()),
            ffprobe: Some("unused-ffprobe".into()),
            ..Default::default()
        };
        fs::write(&settings, serde_json::to_vec(&spec).unwrap()).unwrap();
        let run = || {
            execute(serde_json::from_value(json!({
                "schema": SCHEMA,
                "actions": [
                    {"action": "load_job", "path": settings},
                    {"action": "find_bins"},
                    {"action": "set_folder_expanded", "path": "Sound", "expanded": false},
                    {"action": "set_bin_selected", "path": "Editorial/Reels/Selects.avb", "selected": true},
                    {"action": "capture_project_layout"},
                    {"action": "open_media"},
                    {"action": "snapshot"},
                    {"action": "run_review"},
                    {"action": "snapshot"},
                    {"action": "set_bin_selected", "path": "Editorial/Reels/Selects.avb", "selected": false},
                    {"action": "snapshot"}
                ]
            })).unwrap()).unwrap()
        };
        let response = run();
        let media = &response.snapshots[0];
        assert_eq!(media.page, Page::Clips.label());
        assert!(media.review.is_none());
        let before = &response.snapshots[1];
        let after = &response.snapshots[2];
        assert_eq!(response.layouts.len(), 1);
        assert_eq!(before.catalog.as_ref().unwrap().bins.len(), 2);
        assert!(
            before
                .catalog
                .as_ref()
                .unwrap()
                .folders
                .iter()
                .any(|folder| { folder.path == Path::new("Sound") && !folder.expanded })
        );
        let selected = project.join("Editorial/Reels/Selects.avb");
        assert_eq!(before.selected_bins, [selected.clone()]);
        assert_eq!(before.job.bins, before.selected_bins);
        assert_eq!(before.review.as_ref().unwrap()["status"], "ready");
        assert!(after.job.bins.is_empty() && after.review.is_none());
        assert!(after.revision > before.revision);
        assert!(!output.exists());

        fs::write(&output, b"incumbent").unwrap();
        let blocked = run();
        let report = blocked.snapshots[1].review.as_ref().unwrap();
        assert_eq!(report["status"], "blocked");
        assert_eq!(report["blocker"]["kind"], "project_output");
        assert_eq!(fs::read(output).unwrap(), b"incumbent");
        assert_eq!(fs::read(selected).unwrap(), EMPTY_BIN);
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
        assert!(serde_json::from_value::<Action>(json!({"action": "convert"})).is_err());
    }
}
