//! Display-only palettes. Translation uses the shared mapping policy (CONV-0024).
//! Avid 32 RGB16 swatches: snapshots/2026-10-13/phases33-34-review/files/palette.json
//! SHA256: 0f641a1b0dd26c637abac9ca7aac1b8ac876895238eba5f04f402dea64c23576
//! Premiere marker colors: vendor/py_premiere/data/marker_colors.py. Labels use prproj constants.
use eframe::egui::{self, Color32, RichText};
use mapping::AvidToPremiere;
use prproj::convert::colors::{
    self, AVID_MARKER_DEFAULTS, LABEL_NAMES, LABEL_PACKED, MARKER_NAMES,
};

pub const AVID_SWATCHES: [[i64; 3]; 32] = [
    [64000, 48640, 48640],
    [56832, 25600, 29696],
    [65280, 0, 29440],
    [48896, 0, 26112],
    [32256, 12544, 26880],
    [49408, 19200, 41216],
    [61440, 12800, 58880],
    [36608, 0, 45824],
    [58880, 48640, 65280],
    [22528, 17920, 58624],
    [14592, 11776, 38144],
    [15104, 25344, 37888],
    [23040, 38912, 58112],
    [16896, 54272, 62464],
    [17920, 39168, 36864],
    [43520, 65280, 49920],
    [25856, 50432, 21248],
    [16896, 32768, 13824],
    [61952, 65280, 16384],
    [32256, 32768, 14336],
    [49408, 50176, 22016],
    [56064, 55296, 47104],
    [58368, 50688, 0],
    [48896, 43264, 36608],
    [65280, 50176, 32768],
    [62720, 33280, 12544],
    [32256, 20992, 13568],
    [49664, 32256, 20992],
    [48896, 48896, 48896],
    [22784, 22784, 22784],
    [32768, 9216, 9216],
    [51200, 14592, 14592],
];
const MARKER_PACKED: [i64; 8] = [
    4281828977, 4281740498, 4289825711, 4280578025, 4281049552, 4294967295, 4294741314, 4292277273,
];
fn packed(v: i64) -> Color32 {
    let (r, g, b) = colors::unpack_bgr(v);
    Color32::from_rgb(r as u8, g as u8, b as u8)
}
fn hex_color(s: &str) -> Option<Color32> {
    let n = u32::from_str_radix(s.strip_prefix('#')?, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}
fn swatch(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(18., 18.), egui::Sense::hover());
    ui.painter().rect_filled(r, 4., c);
}
fn selector(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    index: &mut i64,
    names: &[&str],
    values: &[i64],
) {
    let safe = (*index).clamp(0, names.len() as i64 - 1) as usize;
    egui::ComboBox::from_id_salt(id)
        .selected_text(names[safe])
        .width(140.)
        .show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                ui.horizontal(|ui| {
                    swatch(ui, packed(values[i]));
                    ui.selectable_value(index, i as i64, *name);
                });
            }
        });
    swatch(ui, packed(values[safe]));
}
pub fn markers(ui: &mut egui::Ui, p: &mut AvidToPremiere) {
    egui::Grid::new("marker-palette")
        .num_columns(4)
        .striped(true)
        .spacing([18., 7.])
        .show(ui, |ui| {
            for (name, default) in AVID_MARKER_DEFAULTS {
                ui.label(name);
                ui.label("→");
                let mut index = p
                    .marker_colors
                    .iter()
                    .rev()
                    .find(|(n, _)| n == name)
                    .map_or(default, |(_, i)| *i);
                let old = index;
                selector(
                    ui,
                    ("marker", name),
                    &mut index,
                    &MARKER_NAMES,
                    &MARKER_PACKED,
                );
                if old != index {
                    let _ = p.set_marker_color(name, index);
                }
                ui.end_row();
            }
        });
}
pub fn clips(
    ui: &mut egui::Ui,
    p: &mut AvidToPremiere,
    custom: &mut String,
    error: &mut Option<String>,
) {
    ui.label(
        "32 Avid palette swatches → 16 Premiere labels. Custom RGB colors use the same mapping.",
    );
    egui::Grid::new("clip-palette")
        .num_columns(5)
        .striped(true)
        .spacing([12., 7.])
        .show(ui, |ui| {
            let mut all = AVID_SWATCHES
                .iter()
                .map(|rgb| colors::rgb16_hex(*rgb))
                .collect::<Vec<_>>();
            for (hex, _) in &p.clip_labels {
                if !all.contains(hex) {
                    all.push(hex.clone());
                }
            }
            for hex in all {
                let Some(color) = hex_color(&hex) else {
                    continue;
                };
                swatch(ui, color);
                ui.label(RichText::new(&hex).monospace());
                ui.label("→");
                let [r, g, b, _] = color.to_array();
                let rgb = [r as i64 * 257, g as i64 * 257, b as i64 * 257];
                let mut index = colors::nearest_label(rgb, &p.clip_labels);
                let old = index;
                selector(ui, ("clip", &hex), &mut index, &LABEL_NAMES, &LABEL_PACKED);
                if old != index {
                    let _ = p.set_clip_label(&hex, index);
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        ui.label("Custom color");
        ui.add(
            egui::TextEdit::singleline(custom)
                .hint_text("#RRGGBB")
                .desired_width(110.),
        );
        if ui.button("Add mapping").clicked() {
            let hex = custom.trim().to_ascii_uppercase();
            if hex.len() == 7 && hex_color(&hex).is_some() {
                let c = hex_color(&hex).unwrap();
                let [r, g, b, _] = c.to_array();
                let i =
                    colors::nearest_label([r as i64 * 257, g as i64 * 257, b as i64 * 257], &[]);
                if let Err(e) = p.set_clip_label(&hex, i) {
                    *error = Some(e.to_string());
                } else {
                    custom.clear();
                }
            } else {
                *error = Some("Enter a color as #RRGGBB.".into());
            }
        }
    });
    ui.small(
        "Premiere labels shown with factory colors; its Preferences may change their appearance.",
    );
}
