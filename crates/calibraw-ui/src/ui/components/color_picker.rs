use eframe::egui::{
    self, Color32, FontId, Mesh, Pos2, Response, Sense, Shape, Stroke, StrokeKind, Ui,
};

const PICKER_HEIGHT: f32 = 112.0;
const HUE_STRIP_WIDTH: f32 = 22.0;
const PICKER_GAP: f32 = 8.0;
const PLANE_SEGMENTS_X: usize = 24;
const PLANE_SEGMENTS_Y: usize = 12;
const HUE_SEGMENTS: usize = 36;
const SWATCH_WIDTH: f32 = 108.0;

#[derive(Clone, Copy)]
struct PickerState {
    expanded: bool,
    hue: f32,
}

/// A compact, reusable RGB color picker designed to live directly in sidebar
/// cards. The swatch toggles an inline saturation/value plane and hue strip;
/// no popup or modal is created.
pub(crate) fn sidebar_color_picker(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    color: &mut [f32; 3],
    label: &str,
    tooltip: &str,
) -> bool {
    let picker_id = ui.make_persistent_id(id_salt);
    let (color_hue, mut saturation, mut value) = rgb_to_hsv(*color);
    let mut state = ui
        .data(|data| data.get_temp::<PickerState>(picker_id))
        .unwrap_or(PickerState {
            expanded: false,
            hue: color_hue,
        });

    // Hue is undefined for grayscale colors. Preserve the last meaningful hue
    // so increasing saturation from gray produces a predictable color.
    if saturation > 1e-5 {
        state.hue = color_hue;
    }

    let swatch = crate::ui::theme::property_row(ui, label, |ui| {
        color_swatch_button(ui, display_color(*color), state.expanded)
    });
    swatch.response.on_hover_text(tooltip);
    if swatch.inner.clicked() {
        state.expanded = !state.expanded;
    }

    let mut changed = false;
    if state.expanded {
        ui.push_id(picker_id.with("inline-picker"), |ui| {
            let frame = egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .inner_margin(egui::Margin::same(crate::ui::theme::SPACE_SM as i8))
                .corner_radius(6.0)
                .stroke(Stroke::new(
                    1.0,
                    ui.visuals().widgets.noninteractive.bg_stroke.color,
                ));

            frame.show(ui, |ui| {
                let width = ui.available_width().max(1.0);
                let strip_width = HUE_STRIP_WIDTH.min((width * 0.12).max(1.0));
                let gap = PICKER_GAP.min(width * 0.06);
                let plane_width = (width - strip_width - gap).max(1.0);

                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, PICKER_HEIGHT), Sense::hover());
                let plane =
                    egui::Rect::from_min_size(rect.min, egui::vec2(plane_width, rect.height()));
                let hue_strip = egui::Rect::from_min_max(
                    egui::pos2(rect.right() - strip_width, rect.top()),
                    rect.max,
                );

                let plane_response = ui
                    .interact(
                        plane,
                        ui.id().with("saturation-value"),
                        Sense::click_and_drag(),
                    )
                    .on_hover_text("Drag to choose saturation and brightness");
                if pointer_is_operating(&plane_response) {
                    if let Some(pointer) = plane_response.interact_pointer_pos() {
                        let next_saturation =
                            ((pointer.x - plane.left()) / plane.width()).clamp(0.0, 1.0);
                        let next_value =
                            (1.0 - (pointer.y - plane.top()) / plane.height()).clamp(0.0, 1.0);
                        if (saturation - next_saturation).abs() > f32::EPSILON
                            || (value - next_value).abs() > f32::EPSILON
                        {
                            saturation = next_saturation;
                            value = next_value;
                            changed = true;
                        }
                    }
                }

                let hue_response = ui
                    .interact(hue_strip, ui.id().with("hue"), Sense::click_and_drag())
                    .on_hover_text("Drag to choose hue");
                if pointer_is_operating(&hue_response) {
                    if let Some(pointer) = hue_response.interact_pointer_pos() {
                        let next_hue =
                            ((pointer.y - hue_strip.top()) / hue_strip.height()).clamp(0.0, 1.0);
                        if (state.hue - next_hue).abs() > f32::EPSILON {
                            state.hue = next_hue;
                            changed = true;
                        }
                    }
                }

                paint_saturation_value_plane(ui, plane, state.hue, saturation, value);
                paint_hue_strip(ui, hue_strip, state.hue);
            });
        });
    }

    if changed {
        *color = hsv_to_rgb(state.hue, saturation, value);
    }
    ui.data_mut(|data| data.insert_temp(picker_id, state));
    changed
}

fn color_swatch_button(ui: &mut Ui, color: Color32, expanded: bool) -> Response {
    let desired_size = egui::vec2(SWATCH_WIDTH, crate::ui::theme::CONTROL_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
    let interaction = crate::ui::theme::interaction_visuals(ui, &response, expanded);
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 5.0, interaction.weak_fill);
    painter.rect_stroke(rect, 5.0, interaction.stroke, StrokeKind::Inside);

    let circle_center = egui::pos2(rect.left() + 15.0, rect.center().y);
    painter.circle_filled(circle_center, 7.0, color);
    painter.circle_stroke(
        circle_center,
        7.0,
        Stroke::new(1.0, Color32::from_white_alpha(110)),
    );

    painter.text(
        egui::pos2(rect.left() + 29.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        color_hex(color),
        FontId::monospace(11.0),
        interaction.foreground,
    );
    painter.text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        if expanded {
            egui_phosphor::regular::CARET_UP
        } else {
            egui_phosphor::regular::CARET_DOWN
        },
        FontId::proportional(12.0),
        interaction.foreground,
    );
    response
}

fn paint_saturation_value_plane(ui: &Ui, rect: egui::Rect, hue: f32, saturation: f32, value: f32) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    painter.add(Shape::mesh(saturation_value_mesh(rect, hue)));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );

    let marker = egui::pos2(
        egui::lerp(rect.left()..=rect.right(), saturation),
        egui::lerp(rect.bottom()..=rect.top(), value),
    );
    painter.circle_stroke(marker, 6.0, Stroke::new(3.0, Color32::BLACK));
    painter.circle_stroke(marker, 6.0, Stroke::new(1.5, Color32::WHITE));
}

fn paint_hue_strip(ui: &Ui, rect: egui::Rect, hue: f32) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    painter.add(Shape::mesh(hue_mesh(rect)));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );

    let y = egui::lerp(rect.top()..=rect.bottom(), hue);
    let marker = [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)];
    painter.line_segment(marker, Stroke::new(4.0, Color32::BLACK));
    painter.line_segment(marker, Stroke::new(2.0, Color32::WHITE));
}

fn pointer_is_operating(response: &Response) -> bool {
    response.clicked() || response.dragged() || response.is_pointer_button_down_on()
}

fn saturation_value_mesh(rect: egui::Rect, hue: f32) -> Mesh {
    let mut mesh = Mesh::default();
    for row in 0..=PLANE_SEGMENTS_Y {
        let y = row as f32 / PLANE_SEGMENTS_Y as f32;
        let value = 1.0 - y;
        for column in 0..=PLANE_SEGMENTS_X {
            let saturation = column as f32 / PLANE_SEGMENTS_X as f32;
            mesh.colored_vertex(
                Pos2::new(
                    egui::lerp(rect.left()..=rect.right(), saturation),
                    egui::lerp(rect.top()..=rect.bottom(), y),
                ),
                hsv_display_color(hue, saturation, value),
            );
        }
    }

    let stride = PLANE_SEGMENTS_X + 1;
    for row in 0..PLANE_SEGMENTS_Y {
        for column in 0..PLANE_SEGMENTS_X {
            let a = (row * stride + column) as u32;
            let b = a + 1;
            let c = a + stride as u32;
            let d = c + 1;
            mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    mesh
}

fn hue_mesh(rect: egui::Rect) -> Mesh {
    let mut mesh = Mesh::default();
    for segment in 0..=HUE_SEGMENTS {
        let hue = segment as f32 / HUE_SEGMENTS as f32;
        let y = egui::lerp(rect.top()..=rect.bottom(), hue);
        let color = hsv_display_color(hue, 1.0, 1.0);
        mesh.colored_vertex(Pos2::new(rect.left(), y), color);
        mesh.colored_vertex(Pos2::new(rect.right(), y), color);
        if segment < HUE_SEGMENTS {
            let index = (segment * 2) as u32;
            mesh.indices.extend_from_slice(&[
                index,
                index + 1,
                index + 2,
                index + 1,
                index + 3,
                index + 2,
            ]);
        }
    }
    mesh
}

// Mask-effect colors are stored as sRGB-encoded 0..1 values. Keep the
// picker in that same domain: egui::ecolor::hsv_from_rgb/rgb_from_hsv operate
// on linear RGB, which would apply an unintended extra transfer function when
// the shader later converts the picker color from sRGB to working Rec.2020.
fn rgb_to_hsv(rgb: [f32; 3]) -> (f32, f32, f32) {
    let r = rgb[0].clamp(0.0, 1.0);
    let g = rgb[1].clamp(0.0, 1.0);
    let b = rgb[2].clamp(0.0, 1.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / delta / 6.0).rem_euclid(1.0)
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let hue = hue.rem_euclid(1.0) * 6.0;
    let saturation = saturation.clamp(0.0, 1.0);
    let value = value.clamp(0.0, 1.0);
    let chroma = value * saturation;
    let x = chroma * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hue.floor() as i32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    [r1 + m, g1 + m, b1 + m]
}

fn hsv_display_color(hue: f32, saturation: f32, value: f32) -> Color32 {
    display_color(hsv_to_rgb(hue, saturation, value))
}

fn display_color(rgb: [f32; 3]) -> Color32 {
    fn channel(value: f32) -> u8 {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    }
    Color32::from_rgb(channel(rgb[0]), channel(rgb[1]), channel(rgb[2]))
}

fn color_hex(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

#[cfg(test)]
mod tests {
    use super::{color_hex, display_color, hsv_to_rgb, rgb_to_hsv, sidebar_color_picker};

    #[test]
    fn rgb_and_hsv_round_trip_for_sidebar_colors() {
        for original in [
            [1.0, 0.0, 0.0],
            [0.12, 0.73, 0.41],
            [0.5, 0.5, 0.5],
            [0.03, 0.05, 0.92],
        ] {
            let (hue, saturation, value) = rgb_to_hsv(original);
            let round_trip = hsv_to_rgb(hue, saturation, value);
            for channel in 0..3 {
                assert!((round_trip[channel] - original[channel]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn picker_hex_is_the_stored_srgb_value() {
        let blue = [0.0, 112.0 / 255.0, 1.0];
        assert_eq!(color_hex(display_color(blue)), "#0070FF");

        let (hue, saturation, value) = rgb_to_hsv(blue);
        let round_trip = hsv_to_rgb(hue, saturation, value);
        for channel in 0..3 {
            assert!((round_trip[channel] - blue[channel]).abs() < 1e-5);
        }
    }

    #[test]
    fn collapsed_picker_fits_a_narrow_sidebar_without_changing_color() {
        let ctx = eframe::egui::Context::default();
        let mut color = [0.2, 0.4, 0.8];
        let before = color;
        let _ = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            ui.set_width(220.0);
            let left = ui.cursor().left();
            assert!(!sidebar_color_picker(
                ui,
                "test-picker",
                &mut color,
                "Color",
                "Pick a color",
            ));
            assert!(ui.min_rect().right() <= left + 220.1);
        });
        assert_eq!(before, color);
    }
}
