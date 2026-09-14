use super::*;
use crate::ui::theme;

fn histogram_open(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| data.get_persisted::<bool>(egui::Id::new("develop-histogram-open")))
        .unwrap_or(true)
}

impl Sidebar {
    pub(super) fn show_histogram_toggle(ui: &mut Ui) {
        let open = histogram_open(ui.ctx());
        if crate::ui::icons::phosphor_icon_toggle_button(
            ui,
            egui_phosphor::regular::CHART_BAR,
            open,
            theme::toolbar_icon_size(),
            if open {
                "Hide RGB + luminance histogram"
            } else {
                "Show RGB + luminance histogram"
            },
        )
        .clicked()
        {
            ui.ctx().data_mut(|data| {
                data.insert_persisted(egui::Id::new("develop-histogram-open"), !open)
            });
        }
    }

    pub(super) fn show_histogram(ui: &mut Ui, app: &mut CalibRawApp) {
        if !histogram_open(ui.ctx()) {
            return;
        }
        theme::content_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Histogram").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (label, color) in [
                        ("L", ui.visuals().text_color()),
                        ("B", theme::CHANNEL_BLUE),
                        ("G", theme::CHANNEL_GREEN),
                        ("R", theme::CHANNEL_RED),
                    ] {
                        ui.colored_label(color, label);
                    }
                });
            });
            let height = if theme::is_compact_portrait(ui) {
                72.0
            } else {
                104.0
            };
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::hover(),
            );
            app.preview.histogram.visible_this_frame |= ui.is_rect_visible(rect);
            response.on_hover_text("RGB and luminance of the developed image, including crop. Shadows are on the left, highlights on the right. Luminance uses linear sRGB weights on a display-encoded axis. The vertical scale is square-root compressed to reveal smaller peaks.");
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
            let plot = rect.shrink(4.0);
            let grid = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
            for fraction in [0.25, 0.5, 0.75] {
                painter.vline(egui::lerp(plot.x_range(), fraction), plot.y_range(), grid);
            }
            if let Some(histogram) = app.current_preview_histogram() {
                let peak = histogram
                    .channels
                    .iter()
                    .flatten()
                    .copied()
                    .max()
                    .unwrap_or(0)
                    .max(1) as f32;
                for (channel, color) in [
                    (3, ui.visuals().text_color()),
                    (0, theme::CHANNEL_RED),
                    (1, theme::CHANNEL_GREEN),
                    (2, theme::CHANNEL_BLUE),
                ] {
                    let points: Vec<_> = histogram.channels[channel]
                        .iter()
                        .enumerate()
                        .map(|(bin, count)| {
                            egui::pos2(
                                egui::lerp(plot.x_range(), bin as f32 / 255.0),
                                plot.bottom() - (*count as f32 / peak).sqrt() * plot.height(),
                            )
                        })
                        .collect();
                    let mut mesh = egui::Mesh::default();
                    let fill = color.gamma_multiply(if channel == 3 { 0.12 } else { 0.18 });
                    for point in &points {
                        mesh.colored_vertex(egui::pos2(point.x, plot.bottom()), fill);
                        mesh.colored_vertex(*point, fill);
                    }
                    for bin in 0..255_u32 {
                        let i = bin * 2;
                        mesh.add_triangle(i, i + 1, i + 2);
                        mesh.add_triangle(i + 1, i + 3, i + 2);
                    }
                    painter.add(egui::Shape::mesh(mesh));
                    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, color)));
                }
            } else {
                let message = if app.preview.gpu_pipeline.is_none() {
                    "Open an image to see its histogram"
                } else if app.preview.histogram.error {
                    "Histogram unavailable"
                } else {
                    "Updating histogram…"
                };
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    egui::FontId::proportional(12.0),
                    ui.visuals().weak_text_color(),
                );
            }
            let (axis, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 12.0), egui::Sense::hover());
            for (value, fraction, align) in [
                ("0", 0.0, egui::Align2::LEFT_CENTER),
                ("128", 0.5, egui::Align2::CENTER_CENTER),
                ("255", 1.0, egui::Align2::RIGHT_CENTER),
            ] {
                ui.painter().text(
                    egui::pos2(egui::lerp(axis.x_range(), fraction), axis.center().y),
                    align,
                    value,
                    egui::FontId::proportional(10.0),
                    ui.visuals().weak_text_color(),
                );
            }
        });
        theme::card_gap(ui);
    }
}
