use super::Sidebar;
use crate::app::CalibRawApp;
use crate::i18n::tr;
use crate::ui::icons::{icon_toggle_button, UiIcon};
use crate::ui::theme;
use eframe::egui::{self, Ui};

impl Sidebar {
    pub(super) fn show_clipping_toggles(ui: &mut Ui, app: &mut CalibRawApp) {
        for (enabled, icon, tooltip) in [
            (
                &mut app.preview.clipping.highlights,
                egui_phosphor::regular::SUN,
                tr("Highlight clipping: mark pixels red when any processed RGB channel reaches 255"),
            ),
            (
                &mut app.preview.clipping.shadows,
                egui_phosphor::regular::MOON,
                tr("Shadow clipping: mark pixels blue when all processed RGB channels reach 0"),
            ),
        ] {
            if moduwu_design::ghost_icon_toggle_button(
                ui,
                icon,
                *enabled,
                moduwu_design::toolbar_icon_size(),
                tooltip,
            )
            .clicked()
            {
                *enabled = !*enabled;
            }
        }
    }

    pub(super) fn show_histogram_toggle(ui: &mut Ui, app: &mut CalibRawApp) {
        let open = app.develop_ui.histogram_open;
        if moduwu_design::ghost_icon_toggle_button(
            ui,
            egui_phosphor::regular::CHART_BAR,
            open,
            moduwu_design::toolbar_icon_size(),
            if open {
                tr("Hide RGB + luminance histogram")
            } else {
                tr("Show RGB + luminance histogram")
            },
        )
        .clicked()
        {
            app.develop_ui.histogram_open = !open;
            app.persist_performance_settings();
        }
    }

    /// The channel the histogram is limited to (0 red, 1 green, 2 blue,
    /// 3 luminance), or `None` for all of them.
    fn histogram_channel(ctx: &egui::Context) -> Option<usize> {
        ctx.data(|data| data.get_temp::<Option<usize>>(egui::Id::new("develop-histogram-channel")))
            .flatten()
    }

    pub(super) fn show_histogram(ui: &mut Ui, app: &mut CalibRawApp) {
        if !app.develop_ui.histogram_open {
            return;
        }
        Self::show_histogram_controls(ui, app);
        if !app.develop_ui.histogram_overlay {
            let height = if moduwu_design::is_compact_portrait(ui) {
                72.0
            } else {
                104.0
            };
            let size = egui::vec2(ui.available_width(), height);
            Self::show_histogram_plot(ui, app, size, false);
        }
    }

    /// The channel buttons and the placement toggle stay in the panel.
    pub(super) fn show_histogram_controls(ui: &mut Ui, app: &mut CalibRawApp) {
        let selected = Self::histogram_channel(ui.ctx());
        let spacing = ui.spacing().item_spacing.x;
        let icon_size = moduwu_design::toolbar_icon_size();
        let button = egui::vec2(
            ((ui.available_width() - icon_size.x - spacing * 5.0) / 5.0).max(24.0),
            30.0,
        );
        ui.horizontal(|ui| {
            for (channel, label, name, color) in [
                (
                    None,
                    tr("All"),
                    tr("All channels"),
                    ui.visuals().text_color(),
                ),
                (Some(0), "R", tr("Red"), theme::CHANNEL_RED),
                (Some(1), "G", tr("Green"), theme::CHANNEL_GREEN),
                (Some(2), "B", tr("Blue"), theme::CHANNEL_BLUE),
                (Some(3), "L", tr("Luminance"), ui.visuals().text_color()),
            ] {
                let active = selected == channel;
                // Dim in the channel's colour at rest, bright when selected.
                let color = if active {
                    color.lerp_to_gamma(egui::Color32::WHITE, 0.6)
                } else {
                    color.gamma_multiply(0.6)
                };
                if ui
                    .add_sized(
                        button,
                        egui::Button::selectable(active, egui::RichText::new(label).color(color)),
                    )
                    .on_hover_text(name)
                    .clicked()
                {
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new("develop-histogram-channel"), channel)
                    });
                }
            }
            if icon_toggle_button(
                ui,
                UiIcon::PhotoOverlay,
                app.develop_ui.histogram_overlay,
                icon_size,
                tr("Histogram over the photo"),
            )
            .clicked()
            {
                app.develop_ui.histogram_overlay = !app.develop_ui.histogram_overlay;
                app.persist_performance_settings();
                ui.ctx().request_repaint();
            }
        });
    }

    /// The histogram overlay's rect as last placed, while it is shown.
    fn histogram_overlay_rect(ctx: &egui::Context) -> Option<egui::Rect> {
        ctx.data(|data| data.get_temp(egui::Id::new("develop-histogram-overlay-rect")))
    }

    /// The part of the preview overlays from the panel may take (the tone
    /// curve, the mask quick controls): the exposed canvas below the
    /// histogram overlay, as last laid out.
    pub(super) fn photo_overlay_region(ctx: &egui::Context) -> Option<egui::Rect> {
        let mut region = crate::ui::preview::exposed_canvas(ctx)?;
        if let Some(histogram) = Self::histogram_overlay_rect(ctx) {
            region.min.y = region.min.y.max(histogram.bottom());
        }
        Some(region)
    }

    /// Where a canvas label at the top left of `rect` starts: below the
    /// histogram overlay while it covers that corner.
    pub(crate) fn canvas_label_origin(ctx: &egui::Context, rect: egui::Rect) -> egui::Pos2 {
        let origin = rect.left_top() + egui::vec2(12.0, 12.0);
        match Self::histogram_overlay_rect(ctx) {
            Some(overlay)
                if overlay.x_range().contains(origin.x) && overlay.bottom() > origin.y =>
            {
                egui::pos2(origin.x, overlay.bottom() + 8.0)
            }
            _ => origin,
        }
    }

    /// The histogram plot alone, translucent over the top left of the
    /// preview `canvas`, while it is switched on.
    pub(crate) fn show_histogram_overlay(
        ctx: &egui::Context,
        app: &mut CalibRawApp,
        canvas: egui::Rect,
    ) {
        let placed = egui::Id::new("develop-histogram-overlay-rect");
        ctx.data_mut(|data| data.remove::<egui::Rect>(placed));
        if !app.develop_ui.histogram_open
            || !app.develop_ui.histogram_overlay
            || canvas.width() < 120.0
            || canvas.height() < 120.0
        {
            return;
        }
        let size = egui::vec2((canvas.width() - 16.0).clamp(100.0, 220.0), 64.0);
        let origin = canvas.left_top() + egui::vec2(8.0, 8.0);
        ctx.data_mut(|data| data.insert_temp(placed, egui::Rect::from_min_size(origin, size)));
        egui::Area::new(egui::Id::new("develop-histogram-overlay"))
            .order(egui::Order::Middle)
            .fixed_pos(origin)
            .movable(false)
            .constrain(false)
            .interactable(false)
            .show(ctx, |ui| Self::show_histogram_plot(ui, app, size, true));
    }

    fn show_histogram_plot(ui: &mut Ui, app: &mut CalibRawApp, size: egui::Vec2, overlay: bool) {
        let selected = Self::histogram_channel(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        app.preview.histogram.visible_this_frame |= ui.is_rect_visible(rect);
        let painter = ui.painter_at(rect);
        let background = ui.visuals().extreme_bg_color;
        let background = if overlay {
            background.gamma_multiply(0.45)
        } else {
            background
        };
        painter.rect_filled(rect, 4.0, background);
        let plot = rect.shrink(4.0);
        let grid = egui::Stroke::new(
            1.0,
            ui.visuals()
                .widgets
                .noninteractive
                .bg_stroke
                .color
                .gamma_multiply(0.6),
        );
        for fraction in [0.25, 0.5, 0.75] {
            painter.vline(egui::lerp(plot.x_range(), fraction), plot.y_range(), grid);
        }
        let Some(histogram) = app.current_preview_histogram() else {
            return;
        };
        // Scaled to the channels drawn, so a selected one fills the plot.
        let peak = histogram
            .channels
            .iter()
            .enumerate()
            .filter(|(channel, _)| selected.is_none_or(|selected| selected == *channel))
            .flat_map(|(_, bins)| bins)
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
            if selected.is_some_and(|selected| selected != channel) {
                continue;
            }
            let points: Vec<_> = histogram.channels[channel]
                .iter()
                .enumerate()
                .map(|(bin, &count)| {
                    // Any occupied bin stays visible, so a few clipped
                    // pixels still reach the end of the plot.
                    let height = if count == 0 {
                        0.0
                    } else {
                        ((count as f32 / peak).sqrt() * plot.height()).max(1.5)
                    };
                    egui::pos2(
                        egui::lerp(plot.x_range(), bin as f32 / 255.0),
                        plot.bottom() - height,
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
    }
}
