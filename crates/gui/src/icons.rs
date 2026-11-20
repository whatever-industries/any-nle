use eframe::egui;

const ARROW_SIZE: usize = 64;
const ARROW_RGBA: &[u8; ARROW_SIZE * ARROW_SIZE * 4] =
    include_bytes!(concat!(env!("OUT_DIR"), "/arrow-right.rgba"));

pub(super) fn arrow(ui: &egui::Ui, size: f32, accessible_label: &str) -> egui::Image<'static> {
    let id = egui::Id::unique("any-nle-arrow-right");
    let ctx = ui.ctx();
    let cached = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id));
    let texture = cached.unwrap_or_else(|| {
        let image = egui::ColorImage::from_rgba_premultiplied([ARROW_SIZE, ARROW_SIZE], ARROW_RGBA);
        let texture = ctx.load_texture("arrow-right", image, egui::TextureOptions::LINEAR);
        ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    egui::Image::from_texture(&texture)
        .fit_to_exact_size(egui::Vec2::splat(size))
        .tint(ui.visuals().text_color())
        .alt_text(accessible_label)
}

#[cfg(target_os = "macos")]
pub(super) fn route(ui: &mut egui::Ui, source: &str, destination: &str, style: egui::TextStyle) {
    let size = style.resolve(ui.style()).size;
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(source).text_style(style.clone()));
        ui.add(arrow(ui, size, "to"));
        ui.label(egui::RichText::new(destination).text_style(style));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_arrow_has_ink_and_a_transparent_background() {
        let alpha = |x, y| ARROW_RGBA[(y * ARROW_SIZE + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(ARROW_SIZE / 2, ARROW_SIZE / 2), 255);
    }
}
