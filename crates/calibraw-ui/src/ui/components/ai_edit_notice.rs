//! The banner that the open photo's exports are marked as AI-edited
//! (`pipeline::is_ai_edited`), shown in the Remove and Export panels.

use crate::i18n::tr;
use eframe::egui::{self, Response, RichText, Sense, Stroke, Ui};

/// A full-width banner in the theme's warning colour; its tooltip says why
/// the photo is marked and what the mark is.
pub(crate) fn ai_edit_notice(ui: &mut Ui) -> Response {
    let warn = ui.visuals().warn_fg_color;
    let title = tr("Marked as AI-edited");
    let response = moduwu_design::card_frame(ui)
        .fill(warn.gamma_multiply(0.14))
        .stroke(Stroke::new(1.5, warn))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(egui_phosphor::regular::SPARKLE)
                        .size(moduwu_design::toolbar_icon_size().y)
                        .color(warn),
                );
                ui.label(RichText::new(title).strong().color(warn));
            });
        })
        .response
        .interact(Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, title));
    response.on_hover_text(tr(
        "Remove changed a large part of this photo, so its exports are tagged as AI-edited in their metadata, as the EU AI Act requires. Nothing visible is added. Small fixes such as dust spots stay untagged, and Clone and Heal never count.",
    ))
}
