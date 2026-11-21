//! Shared Avid destination preview. Route exposure waits for native bin writing.
use anynle_app::avid_media::{self, AudioProfile, DrivePlan, PictureProfile};
use eframe::egui::{self, RichText};
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub(super) enum Source {
    Premiere,
    Resolve,
}

impl Source {
    pub(super) fn parse(source: &str) -> Option<Self> {
        match source {
            "premiere" => Some(Self::Premiere),
            "resolve" => Some(Self::Resolve),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Premiere => "Adobe Premiere Pro",
            Self::Resolve => "DaVinci Resolve",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Premiere => "prproj",
            Self::Resolve => "drp",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Media {
    Link,
    Fallback,
    Managed,
}

impl Media {
    fn label(self) -> &'static str {
        match self {
            Self::Link => "Link originals",
            Self::Fallback => "Create Avid media for unsupported originals",
            Self::Managed => "Create Avid media for all selected originals",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Project,
    Media,
    Review,
}

struct Preview {
    source: Source,
    path: Option<PathBuf>,
    project_folder: Option<PathBuf>,
    page: Page,
    media: Media,
    picture: PictureProfile,
    audio: AudioProfile,
    drive: Option<PathBuf>,
    plan: Option<Result<DrivePlan, String>>,
}

pub(super) fn run(source: Source, options: eframe::NativeOptions) -> eframe::Result {
    eframe::run_native(
        "Any NLE · Avid destination preview",
        options,
        Box::new(move |cc| {
            crate::wizard::App::theme(&cc.egui_ctx);
            Ok(Box::new(Preview {
                source,
                path: None,
                project_folder: None,
                page: Page::Project,
                media: Media::Link,
                picture: PictureProfile::default(),
                audio: AudioProfile::default(),
                drive: None,
                plan: None,
            }))
        }),
    )
}

impl eframe::App for Preview {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("avid-destination-steps")
            .resizable(false)
            .default_size(158.)
            .show(ui, |ui| {
                ui.add_space(12.);
                ui.label(RichText::new("Any NLE").size(17.));
                ui.add_space(14.);
                for (page, title) in [
                    (Page::Project, "Project"),
                    (Page::Media, "Media"),
                    (Page::Review, "Review"),
                ] {
                    if ui.selectable_label(self.page == page, title).clicked() {
                        self.page = page;
                    }
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.small(self.source.name());
                ui.add(crate::icons::arrow(ui, 12., "to"));
                ui.small("Avid Media Composer");
            });
            ui.horizontal(|ui| {
                ui.heading(match self.page {
                    Page::Project => "Choose what to transfer",
                    Page::Media => "Choose how to handle media",
                    Page::Review => "Review Avid setup",
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.page != Page::Review && ui.button("Continue").clicked() {
                        self.page = if self.page == Page::Project {
                            Page::Media
                        } else {
                            self.check_drive();
                            Page::Review
                        };
                    }
                });
            });
            ui.add_space(8.);
            match self.page {
                Page::Project => self.project(ui),
                Page::Media => self.media(ui),
                Page::Review => self.review(ui),
            }
        });
    }
}

impl Preview {
    fn project(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            if ui.button("Choose source project…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(self.source.name(), &[self.source.extension()])
                    .pick_file()
                {
                    self.path = Some(path);
                }
            }
            if let Some(path) = &self.path {
                ui.label(path.to_string_lossy());
            }
        });
        ui.add_space(5.);
        card(ui, |ui| {
            if ui.button("Choose Avid project folder…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.project_folder = Some(path);
                }
            }
            if let Some(path) = &self.project_folder {
                ui.label(path.to_string_lossy());
            }
        });
    }

    fn media(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            for mode in [Media::Link, Media::Fallback, Media::Managed] {
                ui.radio_value(&mut self.media, mode, mode.label());
            }
        });
        if self.media == Media::Link {
            return;
        }
        ui.add_space(5.);
        card(ui, |ui| {
            if ui.button("Choose media drive…").clicked() {
                let picker =
                    rfd::FileDialog::new().set_title("Select the root of the Avid media drive");
                #[cfg(target_os = "macos")]
                let picker = picker.set_directory("/Volumes");
                if let Some(drive) = picker.pick_folder() {
                    self.drive = Some(drive);
                    self.check_drive();
                }
            }
            self.drive_details(ui);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("avid-picture-encoding")
                    .selected_text(self.picture.label())
                    .show_ui(ui, |ui| {
                        for profile in PictureProfile::ALL {
                            ui.selectable_value(&mut self.picture, profile, profile.label());
                        }
                    });
                egui::ComboBox::from_id_salt("avid-audio-encoding")
                    .selected_text(self.audio.label())
                    .show_ui(ui, |ui| {
                        for profile in AudioProfile::ALL {
                            ui.selectable_value(&mut self.audio, profile, profile.label());
                        }
                    });
            });
        });
    }

    fn check_drive(&mut self) {
        self.plan = self
            .drive
            .as_ref()
            .map(|drive| avid_media::plan_drive(drive).map_err(|error| error.to_string()));
    }

    fn drive_details(&self, ui: &mut egui::Ui) {
        match &self.plan {
            Some(Ok(plan)) => {
                ui.label(plan.mxf_folder.to_string_lossy());
                ui.small(format!(
                    "{:.1} GB available",
                    plan.available_bytes as f64 / 1e9
                ));
            }
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            None => {}
        }
    }

    fn review(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(self.media.label());
            if self.media != Media::Link {
                ui.label(format!("{} · {}", self.picture.label(), self.audio.label()));
                self.drive_details(ui);
                if self.drive.is_none() {
                    ui.small("Choose a media drive.");
                }
                ui.small("Media Composer creates the media databases when it scans the folder.");
            }
            ui.small("Project and bin translation is pending.");
        });
        if self.media != Media::Link
            && !matches!(self.plan, Some(Ok(_)))
            && ui.button("Media settings").clicked()
        {
            self.page = Page::Media;
        }
    }
}

fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(12)
        .corner_radius(9)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            contents(ui);
        });
}
