//! Shimmer wave painted over thumbnails that are still loading.
//!
//! The wave is one textured rounded rect per tile: a 64×64 alpha texture
//! holding a diagonal highlight band, sampled with repeat wrapping and scrolled
//! through its UVs. It adds no render pass, shader or per-frame upload, and
//! tessellates like the plain placeholder fill it sits on.

use std::f32::consts::PI;
use std::time::Duration;

use eframe::egui::{
    self, pos2, Color32, ColorImage, CornerRadius, Id, Rect, TextureHandle, TextureOptions, Ui,
};

/// Texels per side of the tileable band texture.
const TEXTURE_SIZE: usize = 64;
/// Half width of the highlight band, as a fraction of one wave period.
const BAND_HALF_WIDTH: f32 = 0.18;
/// Screen distance between two wave crests, in points.
const WAVE_PERIOD_POINTS: f32 = 360.0;
/// Horizontal travel speed of the wave, in points per second.
const WAVE_SPEED_POINTS_PER_SECOND: f32 = 240.0;
/// Strength of the highlight relative to the weak text colour.
const WAVE_INTENSITY: f32 = 0.22;
/// Repaint cadence while a wave is visible; the wave is slow and soft, so
/// 30 fps reads as smooth and halves the idle redraw cost on mobile.
const WAVE_FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// Paints the loading wave over `rect` and schedules the next wave frame.
///
/// The phase comes from absolute screen position, so neighbouring loading
/// tiles show one continuous wave travelling across the grid.
pub(crate) fn paint_loading_wave(ui: &Ui, rect: Rect, corner_radius: impl Into<CornerRadius>) {
    let texture = wave_texture(ui.ctx());
    let period = WAVE_PERIOD_POINTS;
    // Wrapping keeps the f64 → f32 conversion precise in long sessions.
    let shift = (ui.input(|input| input.time) * f64::from(WAVE_SPEED_POINTS_PER_SECOND))
        .rem_euclid(f64::from(period)) as f32;
    let uv = Rect::from_min_max(
        pos2((rect.min.x - shift) / period, rect.min.y / period),
        pos2((rect.max.x - shift) / period, rect.max.y / period),
    );
    let tint = ui
        .visuals()
        .weak_text_color()
        .gamma_multiply(WAVE_INTENSITY);
    ui.painter_at(rect).add(
        egui::epaint::RectShape::filled(rect, corner_radius, tint).with_texture(texture.id(), uv),
    );
    ui.ctx().request_repaint_after(WAVE_FRAME_INTERVAL);
}

/// The band texture, uploaded once per context and kept in temporary memory.
fn wave_texture(ctx: &egui::Context) -> TextureHandle {
    let id = Id::new("calibraw-loading-wave-texture");
    if let Some(texture) = ctx.data(|data| data.get_temp::<TextureHandle>(id)) {
        return texture;
    }
    let texture = ctx.load_texture(
        "loading-wave",
        wave_band_image(),
        TextureOptions::LINEAR_REPEAT,
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// White texels whose alpha follows a raised-cosine band along the `x + y`
/// diagonal. The band depends only on `(x + y) mod TEXTURE_SIZE`, so the image
/// tiles seamlessly in both directions under repeat wrapping.
fn wave_band_image() -> ColorImage {
    let pixels = (0..TEXTURE_SIZE * TEXTURE_SIZE)
        .map(|texel| {
            let (x, y) = (texel % TEXTURE_SIZE, texel / TEXTURE_SIZE);
            let phase = ((x + y) % TEXTURE_SIZE) as f32 / TEXTURE_SIZE as f32;
            let offset = (phase - 0.5).abs();
            let alpha = if offset < BAND_HALF_WIDTH {
                0.5 + 0.5 * (PI * offset / BAND_HALF_WIDTH).cos()
            } else {
                0.0
            };
            Color32::from_white_alpha((alpha * 255.0).round() as u8)
        })
        .collect();
    ColorImage::new([TEXTURE_SIZE, TEXTURE_SIZE], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_peaks_mid_period_and_is_clear_at_the_wrap_seam() {
        let image = wave_band_image();
        let alpha = |x: usize| image.pixels[x].a();
        assert_eq!(alpha(TEXTURE_SIZE / 2), 255);
        assert!(alpha(TEXTURE_SIZE / 2 - 4) > alpha(TEXTURE_SIZE / 2 - 8));
        // Linear filtering across the repeat seam blends these two texels.
        assert_eq!(alpha(0), 0);
        assert_eq!(alpha(TEXTURE_SIZE - 1), 0);
    }
}
