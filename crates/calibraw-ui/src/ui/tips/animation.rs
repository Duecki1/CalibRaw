//! Plays a tip's bundled WebP clip, decoding one frame at a time.

use eframe::egui;
use image_webp::{DecodingError, WebPDecoder};
use std::io::Cursor;
use std::time::Duration;

/// Shortest frame duration honoured. Encoders may store 0 ms frames, which
/// would otherwise repaint every frame.
const MIN_FRAME_SECONDS: f64 = 0.02;

/// A looping clip shown in a tip. Only the current frame is decoded and
/// uploaded, so memory stays at one frame however long the clip is.
pub(crate) struct TipAnimation {
    decoder: WebPDecoder<Cursor<&'static [u8]>>,
    /// The current frame: RGBA when the clip has alpha, RGB otherwise.
    pixels: Vec<u8>,
    texture: egui::TextureHandle,
    /// The egui time, in seconds, when the next frame is due; `None` for a
    /// still image.
    next_frame_at: Option<f64>,
}

impl TipAnimation {
    /// Decodes the first frame of `bytes` and uploads it as texture `name`.
    pub(crate) fn new(
        ctx: &egui::Context,
        name: &str,
        bytes: &'static [u8],
    ) -> Result<Self, DecodingError> {
        let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
        let mut pixels = vec![
            0;
            decoder
                .output_buffer_size()
                .ok_or(DecodingError::ImageTooLarge)?
        ];
        let next_frame_at = if decoder.is_animated() {
            let duration_ms = decoder.read_frame(&mut pixels)?;
            Some(ctx.input(|input| input.time) + frame_seconds(duration_ms))
        } else {
            decoder.read_image(&mut pixels)?;
            None
        };
        let texture = ctx.load_texture(
            name,
            color_image(&decoder, &pixels),
            egui::TextureOptions::LINEAR,
        );
        Ok(Self {
            decoder,
            pixels,
            texture,
            next_frame_at,
        })
    }

    /// Shows the frame due now, loops at the end of the clip and schedules a
    /// repaint for the next frame.
    pub(crate) fn advance(&mut self, ctx: &egui::Context) -> Result<(), DecodingError> {
        let Some(due) = self.next_frame_at else {
            return Ok(());
        };
        let now = ctx.input(|input| input.time);
        let next = if now < due {
            due
        } else {
            let duration_ms = match self.decoder.read_frame(&mut self.pixels) {
                Err(DecodingError::NoMoreFrames) => {
                    self.decoder.reset_animation();
                    self.decoder.read_frame(&mut self.pixels)?
                }
                result => result?,
            };
            self.texture.set(
                color_image(&self.decoder, &self.pixels),
                egui::TextureOptions::LINEAR,
            );
            // Timed from now, so a stalled UI resumes at normal speed instead
            // of racing through the frames it missed.
            now + frame_seconds(duration_ms)
        };
        self.next_frame_at = Some(next);
        ctx.request_repaint_after(Duration::from_secs_f64(next - now));
        Ok(())
    }

    pub(crate) fn texture(&self) -> &egui::TextureHandle {
        &self.texture
    }

    /// The clip's size in pixels.
    pub(crate) fn size(&self) -> egui::Vec2 {
        let (width, height) = self.decoder.dimensions();
        egui::vec2(width as f32, height as f32)
    }
}

fn frame_seconds(duration_ms: u32) -> f64 {
    (f64::from(duration_ms) / 1000.0).max(MIN_FRAME_SECONDS)
}

fn color_image(decoder: &WebPDecoder<Cursor<&'static [u8]>>, pixels: &[u8]) -> egui::ColorImage {
    let (width, height) = decoder.dimensions();
    let size = [width as usize, height as usize];
    if decoder.has_alpha() {
        egui::ColorImage::from_rgba_unmultiplied(size, pixels)
    } else {
        egui::ColorImage::from_rgb(size, pixels)
    }
}
