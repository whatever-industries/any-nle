use anynle_app::rewrap_size::{Estimate, calculate};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

#[derive(Default)]
pub(crate) struct SizeControl {
    pending: Option<Receiver<Result<Estimate, String>>>,
    result: Option<Result<Estimate, String>>,
}

impl SizeControl {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, recipes: &[Vec<PathBuf>]) {
        if recipes.is_empty() {
            return;
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.result = Some(result);
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.result = Some(Err("The size check stopped. Calculate again.".into()));
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if ui
            .add_enabled(
                self.pending.is_none(),
                egui::Button::new("Calculate rewrap size"),
            )
            .clicked()
        {
            let (send, receive) = mpsc::channel();
            let recipes = recipes.to_vec();
            let ctx = ui.ctx().clone();
            self.pending = Some(receive);
            self.result = None;
            std::thread::spawn(move || {
                let _ = send.send(calculate(&recipes));
                ctx.request_repaint();
            });
        }
        if self.pending.is_some() {
            ui.spinner();
        }
        match &self.result {
            Some(Ok(estimate)) => {
                let bytes = if estimate.bytes >= 1 << 30 {
                    format!("{:.2} GiB", estimate.bytes as f64 / (1u64 << 30) as f64)
                } else if estimate.bytes >= 1 << 20 {
                    format!("{:.2} MiB", estimate.bytes as f64 / (1u64 << 20) as f64)
                } else {
                    format!("{} bytes", estimate.bytes)
                };
                let noun = if estimate.outputs == 1 {
                    "file"
                } else {
                    "files"
                };
                ui.label(format!(
                    "Estimated new media · {bytes} · {} {noun}",
                    estimate.outputs
                ))
                .on_hover_text("Based on the sizes of the source files being rewrapped.");
            }
            Some(Err(error)) => {
                ui.collapsing("Rewrap size unavailable", |ui| {
                    ui.label(error);
                });
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::report_view_tests::{click, text_center};

    #[test]
    fn report_rewrap_size_waits_for_the_button_and_drops_stale_results() {
        let context = egui::Context::default();
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let recipes = vec![vec![file.clone()], vec![file.clone()]];
        let bytes = std::fs::metadata(file).unwrap().len() * 2;
        let label = format!("Estimated new media · {bytes} bytes · 2 files");
        let draw = |control: &mut SizeControl, input| {
            let mut output = context.run_ui(input, |ui| {
                control.show(ui, &recipes);
            });
            output.textures_delta.clear();
            output
        };
        let mut control = SizeControl::default();
        let first = draw(&mut control, egui::RawInput::default());
        assert!(control.result.is_none() && control.pending.is_none());
        let position = text_center(&first, "Calculate rewrap size").unwrap();
        draw(&mut control, click(position));
        assert!(control.pending.is_some());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let frame = draw(&mut control, egui::RawInput::default());
            if text_center(&frame, &label).is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            text_center(
                &draw(&mut SizeControl::default(), egui::RawInput::default()),
                &label
            )
            .is_none()
        );
    }
}
