use super::*;

impl Sidebar {
    /// Shows user edits, then requests content results those edits newly
    /// depend on, such as scene depth once depth fog is added. Loads, pastes
    /// and undo do not pass through here, so they only mark results stale.
    pub(super) fn requesting_new_content<R>(
        app: &mut CalibRawApp,
        frame: &eframe::Frame,
        show: impl FnOnce(&mut CalibRawApp) -> R,
    ) -> R {
        let before = app.masks.stack.content_dependencies();
        let shown = show(app);
        app.request_new_content_dependencies(&before, frame);
        shown
    }

    /// Offers the AI update wherever results derived from image content are
    /// edited: masks and depth effects alike.
    pub(super) fn show_ai_update_card(ui: &mut Ui, app: &mut CalibRawApp, frame: &eframe::Frame) {
        if !app.ai_update_needed() || app.ai_update_busy() {
            return;
        }
        crate::ui::theme::section_card(ui, "AI results need updating", |ui| {
            ui.label(
                "The image changed since AI masks or scene depth were made. Update them to match it without changing your edits.",
            );
            ui.add_space(crate::ui::theme::SPACE_XS);
            if crate::ui::theme::secondary_button(ui, "Update AI results").clicked() {
                app.request_ai_update(frame);
            }
        });
        crate::ui::theme::card_gap(ui);
    }
}
