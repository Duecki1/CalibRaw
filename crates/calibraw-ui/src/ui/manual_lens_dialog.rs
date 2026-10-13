//! The manual lens-correction dialog. Its preview redraws the current preview
//! texture through the draft correction on every change; applying the draft
//! runs the full correction.
//!
//! The preview texture holds the uncorrected geometry (the canvas applies lens
//! maps when it draws) and the vignetting of the correction already applied,
//! so the dialog warps it with the draft distortion and shades it by the
//! difference between the draft and applied vignetting.

use crate::app::{CalibRawApp, ManualLensDialog};
use crate::pipeline::{ManualLensCorrection, ManualLensModel};
use crate::ui::components::adjustment_slider::AdjustmentSlider;
use eframe::egui::{self, epaint::Vertex, Color32, Mesh, Pos2, Rect, Sense, Shape, Ui};

/// Mesh cells along the preview's longer edge.
const MESH_CELLS: u32 = 48;
/// Guide lines along the preview's longer edge, for judging straight lines.
const GUIDE_CELLS: u32 = 8;
/// The display encoding the preview texture is blended in; vertex colours
/// multiply encoded values, so a linear gain `g` becomes `g^(1/2.2)`.
const DISPLAY_GAMMA: f32 = 2.2;

pub(crate) fn show(ctx: &egui::Context, app: &mut CalibRawApp) {
    let Some(mut dialog) = app.develop_ui.manual_lens_dialog else {
        return;
    };
    let texture = app.preview_base_pipeline().map(|pipeline| {
        (
            pipeline.texture(),
            [pipeline.gpu().width, pipeline.gpu().height],
        )
    });
    let mut action = moduwu_design::DialogAction::None;
    moduwu_design::dialog_window(
        "Manual lens correction",
        ctx,
        moduwu_design::DIALOG_WIDTH_LARGE,
    )
    .show(ctx, |ui| {
        if let Some((texture, size)) = texture {
            ui.vertical_centered(|ui| paint_preview(ui, texture, size, dialog));
            ui.add_space(moduwu_design::SPACE_SM);
        }
        AdjustmentSlider::new(
            "Distortion",
            &mut dialog.draft.distortion,
            ManualLensCorrection::RANGE,
        )
        .decimals(0)
        .step(1.0)
        .hover_text("Positive straightens barrel distortion, negative pincushion.")
        .show(ui);
        AdjustmentSlider::new(
            "Vignetting",
            &mut dialog.draft.vignetting,
            ManualLensCorrection::RANGE,
        )
        .decimals(0)
        .step(1.0)
        .hover_text("Positive brightens the corners, negative darkens them.")
        .show(ui);
        action = moduwu_design::dialog_confirmation_buttons(
            ui,
            "Cancel",
            "Apply",
            true,
            false,
            moduwu_design::DialogKeyboard::CLOSE_ONLY,
        );
    });
    match action {
        moduwu_design::DialogAction::Confirm => app.apply_manual_lens_correction(dialog.draft),
        moduwu_design::DialogAction::Cancel => app.develop_ui.manual_lens_dialog = None,
        moduwu_design::DialogAction::None => app.develop_ui.manual_lens_dialog = Some(dialog),
    }
}

/// Paints the whole frame of `texture` (`size` pixels) as `dialog`'s draft
/// would correct it, fitted to the dialog width and half the screen height.
fn paint_preview(ui: &mut Ui, texture: egui::TextureId, size: [u32; 2], dialog: ManualLensDialog) {
    let [width, height] = size.map(|extent| extent.max(1));
    let aspect = height as f32 / width as f32;
    let max_height = ui.ctx().content_rect().height() * 0.5;
    let display_width = ui.available_width().min(max_height / aspect).max(1.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(display_width, display_width * aspect),
        Sense::hover(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Image, true, "Corrected preview")
    });

    let draft = ManualLensModel::new(dialog.draft, width, height);
    let shown = ManualLensModel::new(dialog.shown, width, height);
    let extent = [
        width.saturating_sub(1).max(1) as f32,
        height.saturating_sub(1).max(1) as f32,
    ];
    let cells = grid_cells(width, height, MESH_CELLS);
    // Per vertex: screen position, texture position and display-encoded gain.
    let vertices: Vec<(Pos2, Pos2, f32)> = (0..=cells[1])
        .flat_map(|row| (0..=cells[0]).map(move |column| (column, row)))
        .map(|(column, row)| {
            let uv = [
                column as f32 / cells[0] as f32,
                row as f32 / cells[1] as f32,
            ];
            let source = draft.source_position([uv[0] * extent[0], uv[1] * extent[1]]);
            let stops = draft.vignetting_stops(source) - shown.vignetting_stops(source);
            (
                rect.lerp_inside(egui::vec2(uv[0], uv[1])),
                Pos2::new(source[0] / extent[0], source[1] / extent[1]),
                (stops / DISPLAY_GAMMA).exp2(),
            )
        })
        .collect();

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    // Gains up to 1 tint the texture; each further unit of gain adds another
    // copy of it with additive (zero-alpha, premultiplied) vertex colours.
    let max_gain = vertices.iter().map(|vertex| vertex.2).fold(1.0, f32::max);
    let additive_layers = (max_gain - 1.0).ceil() as u32;
    for layer in 0..=additive_layers {
        let colour = |gain: f32| {
            let level = (gain - layer as f32).clamp(0.0, 1.0);
            let level = (level * 255.0).round() as u8;
            if layer == 0 {
                Color32::from_gray(level)
            } else {
                Color32::from_rgba_premultiplied(level, level, level, 0)
            }
        };
        let mut mesh = Mesh::with_texture(texture);
        mesh.vertices
            .extend(vertices.iter().map(|&(pos, uv, gain)| Vertex {
                pos,
                uv,
                color: colour(gain),
            }));
        let stride = cells[0] + 1;
        for row in 0..cells[1] {
            for column in 0..cells[0] {
                let a = row * stride + column;
                let (b, c) = (a + 1, a + stride);
                mesh.indices.extend_from_slice(&[a, b, c + 1, a, c + 1, c]);
            }
        }
        painter.add(Shape::mesh(mesh));
    }
    paint_guides(&painter, rect, cells_for_rect(rect));
}

/// Mesh cells per axis for a `width`×`height` image, `longer` along the
/// longer edge and roughly square.
fn grid_cells(width: u32, height: u32, longer: u32) -> [u32; 2] {
    let shorter = |long: u32, short: u32| {
        ((longer as f32 * short as f32 / long as f32).round() as u32).max(2)
    };
    if width >= height {
        [longer, shorter(width, height)]
    } else {
        [shorter(height, width), longer]
    }
}

fn cells_for_rect(rect: Rect) -> [u32; 2] {
    grid_cells(
        rect.width().round().max(1.0) as u32,
        rect.height().round().max(1.0) as u32,
        GUIDE_CELLS,
    )
}

fn paint_guides(painter: &egui::Painter, rect: Rect, cells: [u32; 2]) {
    let stroke = egui::Stroke::new(1.0, Color32::from_white_alpha(70));
    for column in 1..cells[0] {
        let x = rect.left() + rect.width() * column as f32 / cells[0] as f32;
        painter.vline(x, rect.y_range(), stroke);
    }
    for row in 1..cells[1] {
        let y = rect.top() + rect.height() * row as f32 / cells[1] as f32;
        painter.hline(rect.x_range(), y, stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_cells_follow_the_aspect_ratio() {
        assert_eq!(grid_cells(3000, 2000, 48), [48, 32]);
        assert_eq!(grid_cells(2000, 3000, 48), [32, 48]);
        assert_eq!(grid_cells(10_000, 10, 48), [48, 2]);
    }
}
