//! The Export panel's watermark card: the switch, the image and where the
//! watermark sits. The preview shows it while the panel is open
//! (`ui::preview::tools::watermark`).

use crate::app::{WatermarkAction, WatermarkView};
use crate::pipeline::{WatermarkAnchor, WatermarkPlacement};
use crate::ui::components::adjustment_slider::AdjustmentSlider;
use eframe::egui::{self, Ui};
use egui_phosphor::regular;

pub(super) fn watermark_card(ui: &mut Ui, view: WatermarkView) -> Option<WatermarkAction> {
    let mut action = None;
    moduwu_design::section_card(ui, "Watermark", |ui| {
        let mut enabled = view.enabled;
        ui.add_enabled_ui(!view.loading, |ui| {
            if moduwu_design::toggle_row(ui, &mut enabled, "Add watermark").changed() {
                action = Some(WatermarkAction::SetEnabled(enabled));
            }
        });
        if view.loading {
            ui.add(egui::Spinner::new());
        }
        let Some((texture, size)) = view.shown_image() else {
            return;
        };
        ui.horizontal(|ui| {
            // Fitted into a 2:1 box twice the control height.
            let bounds = egui::vec2(4.0, 2.0) * moduwu_design::CONTROL_HEIGHT;
            let image = egui::vec2(size[0].max(1) as f32, size[1].max(1) as f32);
            let fitted = image * (bounds.x / image.x).min(bounds.y / image.y);
            ui.add(egui::Image::new((texture, fitted)).alt_text("Watermark image"));
            if moduwu_design::secondary_button_enabled(ui, !view.loading, "Change image…").clicked()
            {
                action = Some(WatermarkAction::ChooseImage);
            }
        });

        let placement = view.placement;
        let (mut anchor, mut margin, mut size) =
            (placement.anchor(), placement.margin(), placement.size());
        ui.label("Position");
        if let Some(chosen) = anchor_grid(ui, anchor) {
            anchor = chosen;
        }
        AdjustmentSlider::new("Size (%)", &mut size, WatermarkPlacement::SIZE_RANGE)
            .decimals(0)
            .step(1.0)
            .reset_to(WatermarkPlacement::DEFAULT_SIZE)
            .show(ui);
        // A centered watermark keeps no distance from the edges.
        if anchor != WatermarkAnchor::Center {
            AdjustmentSlider::new("Margin (%)", &mut margin, WatermarkPlacement::MARGIN_RANGE)
                .decimals(1)
                .step(0.5)
                .reset_to(WatermarkPlacement::DEFAULT_MARGIN)
                .show(ui);
        }
        let edited = WatermarkPlacement::new(anchor, margin, size);
        if edited != placement {
            action = Some(WatermarkAction::Place(edited));
        }
    });
    action
}

/// The nine anchors as a 3×3 grid of arrows pointing to their side or
/// corner; returns the one clicked.
fn anchor_grid(ui: &mut Ui, current: WatermarkAnchor) -> Option<WatermarkAnchor> {
    let edge = egui::Vec2::splat(moduwu_design::CONTROL_HEIGHT);
    let enabled = ui.is_enabled();
    let mut chosen = None;
    for row in WatermarkAnchor::ALL.chunks_exact(3) {
        ui.horizontal(|ui| {
            for &anchor in row {
                let selected = anchor == current;
                let response = moduwu_design::icon_toggle_button(
                    ui,
                    anchor_glyph(anchor),
                    selected,
                    edge,
                    anchor.label(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::RadioButton,
                        enabled,
                        selected,
                        anchor.label(),
                    )
                });
                if response.clicked() {
                    chosen = Some(anchor);
                }
            }
        });
    }
    chosen
}

fn anchor_glyph(anchor: WatermarkAnchor) -> &'static str {
    match anchor {
        WatermarkAnchor::TopLeft => regular::ARROW_UP_LEFT,
        WatermarkAnchor::Top => regular::ARROW_UP,
        WatermarkAnchor::TopRight => regular::ARROW_UP_RIGHT,
        WatermarkAnchor::Left => regular::ARROW_LEFT,
        WatermarkAnchor::Center => regular::DOT_OUTLINE,
        WatermarkAnchor::Right => regular::ARROW_RIGHT,
        WatermarkAnchor::BottomLeft => regular::ARROW_DOWN_LEFT,
        WatermarkAnchor::Bottom => regular::ARROW_DOWN,
        WatermarkAnchor::BottomRight => regular::ARROW_DOWN_RIGHT,
    }
}
