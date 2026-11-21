//! Review layout and report rendering for the Avid to Premiere wizard.
use super::{
    AMBER, App, ERROR_INK, ERROR_SURFACE, GOOD, INK, MUTED, MediaJump, Pick, ReviewAction, counted,
    review_path, semibold,
};
use crate::model::{Choices, ClipMode, Inputs, Page};
use crate::report::{Filter, PAGE_SIZE, ReportBrowser};
use crate::rewrap_size::SizeControl;
use anynle_app::job::{self, BlockerKind, ReviewBlocker};
use eframe::egui::{self, Color32, RichText};
use mapping::AvidToPremiere;
use std::collections::HashMap;
use std::path::Path;

pub(super) enum Panel {
    Findings,
    Output,
}

pub(super) fn layout(ui: &mut egui::Ui, mut draw: impl FnMut(&mut egui::Ui, Panel)) {
    let width = ui.available_width();
    if width >= 600. {
        let output_width = 196.;
        ui.horizontal_top(|ui| {
            let findings_width = width - output_width - ui.spacing().item_spacing.x;
            column(ui, findings_width, |ui| draw(ui, Panel::Findings));
            column(ui, output_width, |ui| draw(ui, Panel::Output));
        });
    } else {
        draw(ui, Panel::Findings);
        ui.add_space(10.);
        draw(ui, Panel::Output);
    }
}

pub(super) struct Findings<'a> {
    pub(super) busy: bool,
    pub(super) choices: &'a Choices,
    pub(super) issue: Option<&'a ReviewBlocker>,
    pub(super) prepared: Option<&'a job::Review>,
    pub(super) media_ready: bool,
    pub(super) browser: &'a mut ReportBrowser,
    pub(super) why: &'a mut Option<String>,
    pub(super) rewrap_size: &'a mut SizeControl,
    #[cfg(target_os = "macos")]
    pub(super) premiere_status: &'a crate::premiere_review::WizardStatus,
}

impl Findings<'_> {
    pub(super) fn show(self, ui: &mut egui::Ui) -> Vec<ReviewAction> {
        let Self {
            busy,
            choices,
            issue,
            prepared,
            media_ready,
            browser,
            why,
            rewrap_size,
            #[cfg(target_os = "macos")]
            premiere_status,
        } = self;
        if let Some(issue) = issue {
            if let Some(action) = blocker_card(ui, issue, browser) {
                return vec![action];
            }
            rewrap_size.show(ui, &issue.rewrap_sources);
        }
        let creates_media = prepared.is_some_and(|review| review.estimated_bytes.is_some());
        let mut actions = Vec::new();
        let mut save_report = false;
        let mut choose_rewrap = false;
        let mut clear_rewrap = false;
        if issue.is_none() {
            ui.vertical(|ui| {
                if let Some(review) = prepared {
                    let inv = &review.inventory;
                    let changes = browser.changes;
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
                            format!(
                                " · {}",
                                counted(inv.ume_originals, "qualified original", "qualified originals")
                            )
                        }
                    ));
                    if inv.ume_originals > 0 && inv.native_media == 0 {
                        ui.label("Links to available originals in place.");
                    }
                    if let Some(media) = &review.media_linking {
                        ui.add_space(6.);
                        ui.strong("Media linking");
                        if !media.rewrap_available.is_empty() {
                            ui.colored_label(
                                AMBER,
                                format!("Rewrap available for {} managed clips.", media.rewrap_available.len()),
                            );
                            ui.collapsing("Affected clips", |ui| {
                                for clip in &media.rewrap_available {
                                    ui.label(clip);
                                }
                            });
                            choose_rewrap = ui.button("Choose folder to rewrap affected clips…").clicked();
                        }
                        if !media.rewrapped_clips.is_empty() {
                            ui.label(format!(
                                "{} managed clips will be rewrapped.",
                                media.rewrapped_clips.len()
                            ));
                            clear_rewrap = ui.button("Continue without rewrapping").clicked();
                        }
                        if !media.conversion_pending.is_empty() {
                            ui.colored_label(
                                AMBER,
                                format!("Conversion pending for {} recognized clips.", media.conversion_pending.len()),
                            );
                            ui.collapsing("Show clips with pending conversion", |ui| {
                                for clip in &media.conversion_pending {
                                    ui.label(clip);
                                }
                            });
                        }
                        if media.premiere_files.is_empty() {
                            ui.label(format!(
                                "{} link directly in place.",
                                counted(media.direct_files, "verified file", "verified files")
                            ));
                        } else {
                            #[cfg(target_os = "macos")]
                            if let Err(error) = premiere_status.show(ui) {
                                actions.push(ReviewAction::ShowError(error));
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
                        ui.colored_label(
                            AMBER,
                            RichText::new(counted(changes, "translation change", "translation changes"))
                                .strong(),
                        );
                        egui::CollapsingHeader::new("Affected edits")
                            .id_salt("preflight-affected-edits")
                            .show(ui, |ui| {
                                for &index in &browser.change_examples {
                                    let note = &review.notes[index];
                                    if let Some(name) = review.subjects.get(&note.object) {
                                        ui.strong(name);
                                    }
                                    ui.label(&note.message);
                                }
                            });
                    }
                    if browser.warnings > 0 {
                        ui.colored_label(AMBER, counted(browser.warnings, "warning", "warnings"));
                    }
                    let label = format!("Full preflight report · {}", counted(review.notes.len(), "note", "notes"));
                    egui::CollapsingHeader::new(label)
                        .id_salt("preflight-report")
                        .show(ui, |ui| {
                            report(
                                ui, &review.notes, Some(&review.subjects),
                                &review.timeline_groups, &review.source_groups, browser, why,
                            );
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
                        .add_enabled(!busy, egui::Button::new("Save preflight report"))
                        .clicked();
                } else {
                    ui.strong(if busy { "Preflight in progress" } else { "Preflight" });
                    ui.label(
                        RichText::new("Checks media, translation limits and disk space before creating any output.")
                            .color(MUTED),
                    );
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
        if let Some(review) = prepared {
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
            actions.push(ReviewAction::Choose(Pick::RewrapNeeded));
        }
        if clear_rewrap {
            actions.push(ReviewAction::ClearRewrap);
        }
        if save_report {
            actions.push(ReviewAction::Choose(Pick::SavePreflightReport));
        }
        actions
    }
}

pub(super) fn output(
    ui: &mut egui::Ui,
    inputs: &Inputs,
    choices: &Choices,
    review: Option<&job::Review>,
    rewrap_size: &mut SizeControl,
) {
    let rewrap = choices.mode == ClipMode::Rewrap;
    let creates_media = review.is_some_and(|review| review.estimated_bytes.is_some());
    App::card(ui, |ui| {
        ui.strong("Output");
        if inputs.output.as_os_str().is_empty() {
            ui.label(RichText::new("Choose a project file on the Project step.").color(MUTED));
        } else {
            review_path(ui, "Premiere project", &inputs.output);
        }
        ui.small("Clips");
        ui.label(choices.mode.label());
        ui.small("Source");
        ui.label(if inputs.entire_project {
            "Entire project".into()
        } else {
            counted(inputs.bins.len(), "bin", "bins")
        });
        if !rewrap {
            ui.label(if choices.uses_camera_originals() {
                "Primary media · Prefer camera originals for picture"
            } else {
                "Primary media · Use the media in the Avid edit"
            });
            if choices.attaches_avid_proxies() {
                ui.label(if inputs.proxy_rewrap_dir.is_some() {
                    "Proxies · Attach matching Avid media and allow stereo rewrap"
                } else {
                    "Proxies · Link matching Avid media in place"
                });
            }
        }
        if creates_media {
            ui.colored_label(AMBER, RichText::new("Creates new media.").strong());
            if let Some(path) = if rewrap {
                inputs.new_media.as_ref()
            } else {
                inputs
                    .rewrap_needed
                    .as_ref()
                    .or(inputs.proxy_rewrap_dir.as_ref())
            } {
                review_path(ui, "Media folder", path);
            }
        } else if rewrap && review.is_none() {
            ui.label(
                RichText::new("A new media folder is needed for native OP-Atom rewraps.")
                    .color(MUTED),
            );
            if let Some(path) = &inputs.new_media {
                review_path(ui, "Media folder", path);
            }
        }
        if let Some(review) = review {
            rewrap_size.show(ui, &review.rewrap_sources);
        }
    });
}

pub(super) fn blocker_card(
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
    App::card_with_fill(ui, ERROR_SURFACE, |ui| {
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
                blocker_details(ui, &issue.affected, &issue.timeline_groups, browser);
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
    let changed = ui
        .horizontal_wrapped(|ui| {
            let changed = report_group_filter(
                ui,
                timelines,
                &mut browser.timeline,
                "report-timeline",
                "All timelines",
            );
            ui.add(
                egui::TextEdit::singleline(&mut browser.search)
                    .hint_text("Search affected details")
                    .desired_width(220.),
            )
            .changed()
                || changed
        })
        .inner;
    if changed {
        browser.refilter_details(details, timelines);
    }
    if browser.count(details.len()) == 0 {
        ui.label("No affected details match your search.");
        return;
    }
    let page_changed = report_pages(ui, browser, details.len(), "Details");
    let mut rows = egui::ScrollArea::vertical()
        .id_salt("blocker-details")
        .max_height(260.);
    if changed || page_changed {
        rows = rows.vertical_scroll_offset(0.);
    }
    rows.show(ui, |ui| {
        for index in browser.page_indices(details.len()) {
            ui.label(&details[index]);
        }
    });
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
    let selected = selection
        .and_then(|index| groups.get(index))
        .map_or_else(|| all.to_owned(), label);
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .wrap_mode(egui::TextWrapMode::Truncate)
        .height(240.)
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(selection, None, all).changed();
            for (index, group) in groups.iter().enumerate() {
                changed |= ui
                    .selectable_value(selection, Some(index), label(group))
                    .on_hover_text(&group.id)
                    .changed();
            }
        });
    changed
}
pub(super) fn report(
    ui: &mut egui::Ui,
    notes: &[job::ReportNote],
    subjects: Option<&HashMap<String, String>>,
    timelines: &[job::ReviewGroup],
    sources: &[job::ReviewGroup],
    browser: &mut ReportBrowser,
    why: &mut Option<String>,
) {
    let changed = ui
        .horizontal_wrapped(|ui| {
            let filters = [
                (Filter::All, format!("All · {}", notes.len())),
                (Filter::Changes, format!("Changes · {}", browser.changes)),
                (Filter::Warnings, format!("Warnings · {}", browser.warnings)),
            ];
            let mut changed = false;
            for (filter, label) in filters {
                changed |= ui
                    .selectable_value(&mut browser.filter, filter, label)
                    .changed();
            }
            changed |= report_group_filter(
                ui,
                timelines,
                &mut browser.timeline,
                "report-timeline",
                "All timelines",
            );
            changed |= report_group_filter(
                ui,
                sources,
                &mut browser.source_clip,
                "report-source-clip",
                "All source clips",
            );
            let search_changed = ui
                .add(
                    egui::TextEdit::singleline(&mut browser.search)
                        .hint_text("Search report")
                        .desired_width(180.),
                )
                .changed();
            changed || search_changed
        })
        .inner;
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
    let page_changed = report_pages(ui, browser, notes.len(), "Notes");
    let mut rows = egui::ScrollArea::vertical()
        .id_salt("report-rows")
        .max_height(360.);
    if changed || page_changed {
        rows = rows.vertical_scroll_offset(0.);
    }
    rows.show(ui, |ui| {
        for index in browser.page_indices(notes.len()) {
            let n = &notes[index];
            let subject = subjects
                .and_then(|names| names.get(&n.object))
                .map(String::as_str);
            App::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&n.severity).small().color(
                        match n.severity.to_lowercase().as_str() {
                            "error" | "failure" => Color32::LIGHT_RED,
                            "loss" | "warning" => AMBER,
                            _ => MUTED,
                        },
                    ));
                    ui.strong(subject.unwrap_or(&n.object))
                        .on_hover_text(&n.object);
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
fn report_pages(ui: &mut egui::Ui, browser: &mut ReportBrowser, total: usize, label: &str) -> bool {
    let previous_page = browser.page;
    let count = browser.count(total);
    let page_count = count.div_ceil(PAGE_SIZE);
    ui.horizontal_wrapped(|ui| {
        if page_count > 1 {
            if ui
                .add_enabled(browser.page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                browser.page -= 1;
            }
            let mut page_number = browser.page + 1;
            ui.label("Page");
            ui.add(egui::DragValue::new(&mut page_number).range(1..=page_count));
            browser.page = page_number - 1;
            ui.label(format!("of {page_count}"));
            if ui
                .add_enabled(browser.page + 1 < page_count, egui::Button::new("Next"))
                .clicked()
            {
                browser.page += 1;
            }
        }
        let range = browser.page_range(total);
        ui.small(format!(
            "{label} {}–{} of {count}",
            range.start + 1,
            range.end
        ));
    });
    browser.page != previous_page
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

fn bytes(value: Option<u64>) -> String {
    match value {
        None => "not available".into(),
        Some(v) if v >= 1_073_741_824 => format!("{:.1} GiB", v as f64 / 1_073_741_824.),
        Some(v) if v >= 1_048_576 => format!("{:.1} MiB", v as f64 / 1_048_576.),
        Some(v) => format!("{v} bytes"),
    }
}
fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}
