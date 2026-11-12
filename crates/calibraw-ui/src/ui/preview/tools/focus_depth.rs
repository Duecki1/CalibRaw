//! Lens Blur's depth overlay: while a handle of its focus range is held, the
//! photo tinted by where each surface lies against the range, nearer, inside
//! or farther, in the colours of the range's track. It is drawn from the
//! edit's scene depth at the depth result's size, so its outlines are the
//! depth model's; the render's are the photo's own (scene/geometry.wgsl).

use super::super::*;
use crate::pipeline::{DisparityMap, SceneDepth};
use crate::ui::theme;

/// The tint's opacity, as the mask overlay's.
const OVERLAY_ALPHA: u8 = 110;
/// Sky probability from which a pixel counts as sky, the farthest surface,
/// as the scene bake takes it.
const SKY: u8 = 128;

/// The overlay's texture and what it was drawn from. Kept in egui's memory
/// while a handle is held; the disparity is held so its address stays its
/// identity.
#[derive(Clone)]
struct FocusOverlay {
    disparity: Arc<[u16]>,
    /// Each disparity pixel's control depth, the sky at the farthest.
    control: Arc<[f32]>,
    range: [f32; 2],
    texture: egui::TextureHandle,
}

fn overlay_id() -> egui::Id {
    egui::Id::new("calibraw-lens-blur-depth-overlay")
}

/// Each pixel's control depth (`SceneCamera::control_depth`) at the
/// disparity's size, the sky at the farthest, as Lens Blur's shader places
/// surfaces against its focus range.
fn control_depths(depth: &SceneDepth) -> Arc<[f32]> {
    let disparity = depth.disparity();
    let sky = depth.sky();
    let camera = depth.camera();
    let width = disparity.width().max(1) as usize;
    let height = disparity.height().max(1) as usize;
    let (sky_width, sky_height) = (sky.width as usize, sky.height as usize);
    disparity
        .values()
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let sx = ((index % width) * sky_width / width).min(sky_width - 1);
            let sy = ((index / width) * sky_height / height).min(sky_height - 1);
            if sky.pixels[sy * sky_width + sx] >= SKY {
                1.0
            } else {
                camera.control_depth(DisparityMap::decode(*value))
            }
        })
        .collect()
}

/// The overlay's pixels: each control depth tinted as nearer than the focus
/// range `[near, far]`, inside it or farther.
fn overlay_image(size: [usize; 2], control: &[f32], [near, far]: [f32; 2]) -> egui::ColorImage {
    let rgba: Vec<u8> = control
        .iter()
        .flat_map(|&control| {
            let color = if control < near {
                theme::FOCUS_NEAR
            } else if control > far {
                theme::FOCUS_FAR
            } else {
                theme::FOCUS_IN
            };
            [color.r(), color.g(), color.b(), OVERLAY_ALPHA]
        })
        .collect();
    egui::ColorImage::from_rgba_unmultiplied(size, &rgba)
}

impl Preview {
    /// Paints the depth overlay while Lens Blur's focus range is being
    /// adjusted and the edit has scene depth, and lets it go otherwise.
    pub(in crate::ui::preview) fn paint_focus_depth_overlay(
        ui: &Ui,
        app: &CalibRawApp,
        layout: PreviewLayout,
    ) {
        let ctx = ui.ctx();
        let (Some(range), Some(depth)) = (
            crate::ui::sidebar::focus_range_adjusting(ctx),
            app.masks.stack.scene_depth.as_ref(),
        ) else {
            ctx.data_mut(|data| data.remove::<FocusOverlay>(overlay_id()));
            return;
        };
        let disparity = depth.disparity();
        let size = [disparity.width() as usize, disparity.height() as usize];
        let kept = ctx
            .data(|data| data.get_temp::<FocusOverlay>(overlay_id()))
            .filter(|overlay| Arc::ptr_eq(&overlay.disparity, disparity.values()));
        let overlay = match kept {
            Some(overlay) if overlay.range == range => overlay,
            Some(mut overlay) => {
                let image = overlay_image(size, &overlay.control, range);
                overlay.texture.set(image, egui::TextureOptions::LINEAR);
                overlay.range = range;
                ctx.data_mut(|data| data.insert_temp(overlay_id(), overlay.clone()));
                overlay
            }
            None => {
                let control = control_depths(depth);
                let image = overlay_image(size, &control, range);
                let overlay = FocusOverlay {
                    disparity: Arc::clone(disparity.values()),
                    control,
                    range,
                    texture: ctx.load_texture(
                        "lens-blur-depth-overlay",
                        image,
                        egui::TextureOptions::LINEAR,
                    ),
                };
                ctx.data_mut(|data| data.insert_temp(overlay_id(), overlay.clone()));
                overlay
            }
        };
        // One more pass once the handle is let go, so the overlay goes.
        ctx.request_repaint();
        let lens_geometry = loaded_lens_geometry(app).map(Arc::as_ref);
        paint_final_geometry_overlay_texture(
            ui,
            overlay.texture.id(),
            layout.projection(app.develop.geometry, lens_geometry),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            [0.0, 0.0, 1.0, 1.0],
        );
    }
}
