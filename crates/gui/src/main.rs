#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use anynle_gui::model;
mod direction;
mod palette;
mod storage;
mod wizard;

fn main() -> eframe::Result {
    let mut job = None;
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
    eframe::run_native(
        "Any NLE",
        options,
        Box::new(move |cc| Ok(Box::new(wizard::App::new(cc, job)))),
    )
}
