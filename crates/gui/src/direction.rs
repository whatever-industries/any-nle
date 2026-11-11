use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Editor {
    AvidMediaComposer,
    Premiere,
    Resolve,
    ProTools,
    AfterEffects,
}

impl Editor {
    const ALL: [Self; 5] = [
        Self::AvidMediaComposer,
        Self::Premiere,
        Self::Resolve,
        Self::ProTools,
        Self::AfterEffects,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::AvidMediaComposer => "Avid Media Composer",
            Self::Premiere => "Premiere",
            Self::Resolve => "Resolve",
            Self::ProTools => "Pro Tools",
            Self::AfterEffects => "After Effects",
        }
    }
}

// This describes the native wizard's available routes. Job settings still refer
// only to the existing Avid-to-Premiere engine.
const AVAILABLE_ROUTES: [(Editor, Editor); 1] = [(Editor::AvidMediaComposer, Editor::Premiere)];

pub(super) fn show(ui: &mut egui::Ui) {
    let (source, destination) = AVAILABLE_ROUTES[0];
    let available = format!(
        "Only {} to {} is available in this version.",
        source.label(),
        destination.label(),
    );
    egui::Grid::new("translation-direction").show(ui, |ui| {
        dropdown(ui, "Translate from", source, &available, |editor| {
            AVAILABLE_ROUTES.contains(&(editor, destination))
        });
        ui.end_row();
        dropdown(ui, "Translate to", destination, &available, |editor| {
            AVAILABLE_ROUTES.contains(&(source, editor))
        });
        ui.end_row();
    });
    ui.small(available);
}

fn dropdown(
    ui: &mut egui::Ui,
    name: &str,
    current: Editor,
    explanation: &str,
    is_available: impl Fn(Editor) -> bool,
) {
    let label = ui.label(name);
    egui::ComboBox::from_id_salt(name)
        .selected_text(current.label())
        .width(176.)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            for editor in Editor::ALL {
                if ui
                    .add_enabled(
                        is_available(editor),
                        egui::Button::selectable(editor == current, editor.label()),
                    )
                    .on_disabled_hover_text(explanation)
                    .clicked()
                {
                    ui.close();
                }
            }
        })
        .response
        .labelled_by(label.id)
        .on_hover_text(explanation);
}
