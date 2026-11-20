use eframe::egui;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Editor {
    AvidMediaComposer,
    Premiere,
    Resolve,
}

impl Editor {
    const ALL: [Self; 3] = [Self::AvidMediaComposer, Self::Premiere, Self::Resolve];

    fn label(self) -> &'static str {
        match self {
            Self::AvidMediaComposer => "Avid Media Composer",
            Self::Premiere => "Premiere",
            Self::Resolve => "Resolve",
        }
    }
}

// This describes the native wizard's available routes. Job settings still refer
// only to the existing Avid-to-Premiere engine.
const AVAILABLE_ROUTES: [(Editor, Editor); 1] = [(Editor::AvidMediaComposer, Editor::Premiere)];

pub(super) fn show(ui: &mut egui::Ui, avid_resolve: bool) -> bool {
    let (source, destination) = AVAILABLE_ROUTES[0];
    let available = if avid_resolve {
        "Available routes are Avid Media Composer to Premiere or Resolve.".to_owned()
    } else {
        format!(
            "Only {} to {} is available in this version.",
            source.label(),
            destination.label(),
        )
    };
    let mut selected_resolve = false;
    egui::Grid::new("translation-direction").show(ui, |ui| {
        dropdown(ui, "Translate from", source, &available, |editor| {
            AVAILABLE_ROUTES.contains(&(editor, destination))
        });
        ui.end_row();
        selected_resolve = dropdown(ui, "Translate to", destination, &available, |editor| {
            AVAILABLE_ROUTES.contains(&(source, editor)) || (avid_resolve && editor == Editor::Resolve)
        }) == Some(Editor::Resolve);
        ui.end_row();
    });
    if !avid_resolve { ui.small(available); }
    selected_resolve
}

fn dropdown(
    ui: &mut egui::Ui,
    name: &str,
    current: Editor,
    explanation: &str,
    is_available: impl Fn(Editor) -> bool,
) -> Option<Editor> {
    let label = ui.label(name);
    let mut selected = None;
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
                    selected = Some(editor);
                    ui.close();
                }
            }
        })
        .response
        .labelled_by(label.id)
        .on_hover_text(explanation);
    selected
}
