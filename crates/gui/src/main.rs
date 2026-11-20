#![forbid(unsafe_code)]
#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]
use anynle_gui::model;
mod direction;
mod icons;
mod palette;
#[cfg(target_os = "macos")]
mod relink;
#[cfg(target_os = "macos")]
mod resolve_review;
#[cfg(target_os = "macos")]
mod avid_resolve;
#[cfg(target_os = "macos")]
mod premiere_review;
mod report;
mod rewrap_size;
mod storage;
mod wizard;

fn main() -> eframe::Result {
    #[cfg(feature = "wizard-automation")]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().is_some_and(|arg| arg == "--automation") {
            let Some(path) = args.next().filter(|_| args.next().is_none()) else {
                eprintln!("Usage: any-nle-gui --automation SCENARIO.json");
                std::process::exit(2);
            };
            if let Err(error) = wizard::run_automation(std::path::Path::new(&path)) {
                eprintln!("Wizard automation: {error}");
                std::process::exit(2);
            }
            return Ok(());
        }
    }
    let mut job = None;
    #[cfg(target_os = "macos")]
    let mut resolve_job = None;
    #[cfg(target_os = "macos")]
    let mut premiere_import_job = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => {
                println!("Any NLE {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "Any NLE native wizard\nUsage: any-nle-gui [--job SETTINGS.json]\nThe shared engine reads Avid bins and writes a new Premiere project."
                );
                return Ok(());
            }
            #[cfg(target_os = "macos")]
            "--resolve-job" => {
                let Some(path) = args.next() else {
                    eprintln!("--resolve-job needs a saved Resolve conversion job");
                    std::process::exit(2);
                };
                resolve_job = Some(std::path::PathBuf::from(path));
            }
            #[cfg(target_os = "macos")]
            "--premiere-import-job" => {
                let Some(path) = args.next() else {
                    eprintln!("--premiere-import-job needs a Premiere media linking request");
                    std::process::exit(2);
                };
                premiere_import_job = Some(std::path::PathBuf::from(path));
            }
            "--job" => {
                let Some(path) = args.next() else {
                    eprintln!("--job needs a settings file");
                    std::process::exit(2)
                };
                job = Some(std::path::PathBuf::from(path));
            }
            _ => {
                eprintln!("Unknown option: {arg}");
                std::process::exit(2);
            }
        }
    }
    let image = image::load_from_memory(include_bytes!("../../../assets/icon_v1.jpg"))
        .expect("embedded Any NLE icon")
        .resize(256, 256, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let icon = eframe::egui::IconData {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([880., 620.])
            .with_min_inner_size([760., 520.])
            .with_app_id("industries.whatever.any-nle")
            .with_icon(icon),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    if let Some(path) = premiere_import_job {
        if job.is_some() || resolve_job.is_some() {
            eprintln!("Select one job per wizard window");
            std::process::exit(2);
        }
        return premiere_review::run(path, options);
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = resolve_job {
        if job.is_some() {
            eprintln!("Select one job per wizard window");
            std::process::exit(2);
        }
        return resolve_review::run(path, options);
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = &job {
        if matches!(anynle_app::resolve_job::load_if_avid_resolve(path), Ok(Some(_))) {
            return resolve_review::run(path.clone(), options);
        }
    }
    eframe::run_native(
        "Any NLE",
        options,
        Box::new(move |cc| Ok(Box::new(wizard::App::new(cc, job)))),
    )
}
