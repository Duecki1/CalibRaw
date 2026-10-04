//! Rasterizing remove brush strokes into masks.

use super::*;

pub fn adaptive_remove_dilation(stroke: &[RemoveBrushPoint]) -> u32 {
    if stroke.is_empty() {
        return 0;
    }
    let average_radius = stroke
        .iter()
        .map(|point| point.radius.max(0.0))
        .sum::<f32>()
        / stroke.len() as f32;
    (average_radius * 0.06).round().clamp(1.0, 12.0) as u32
}

pub fn rasterize_remove_brush(
    image_width: u32,
    image_height: u32,
    brush: &RemoveBrushStroke,
) -> Option<RemoveMask> {
    if image_width == 0 || image_height == 0 || brush.points.is_empty() {
        return None;
    }
    let dilation = brush.dilation_radius as f32;
    let mut x0 = image_width as f32;
    let mut y0 = image_height as f32;
    let mut x1 = 0.0f32;
    let mut y1 = 0.0f32;
    for point in &brush.points {
        if !point.x.is_finite() || !point.y.is_finite() || !point.radius.is_finite() {
            continue;
        }
        let radius = point.radius.max(0.5) + dilation;
        x0 = x0.min(point.x - radius - 1.0);
        y0 = y0.min(point.y - radius - 1.0);
        x1 = x1.max(point.x + radius + 1.0);
        y1 = y1.max(point.y + radius + 1.0);
    }
    let left = x0.floor().max(0.0).min(image_width as f32) as u32;
    let top = y0.floor().max(0.0).min(image_height as f32) as u32;
    let right = x1.ceil().max(0.0).min(image_width as f32) as u32;
    let bottom = y1.ceil().max(0.0).min(image_height as f32) as u32;
    if right <= left || bottom <= top {
        return None;
    }
    let bounds = NativeRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };
    let mut pixels = vec![0u8; bounds.width as usize * bounds.height as usize];

    for point in &brush.points {
        let radius = point.radius.max(0.5) + dilation;
        paint_disc(&mut pixels, bounds, point.x, point.y, radius);
    }

    Some(RemoveMask { bounds, pixels })
}

fn paint_disc(pixels: &mut [u8], bounds: NativeRect, center_x: f32, center_y: f32, radius: f32) {
    let y_start = (center_y - radius)
        .floor()
        .max(bounds.y as f32)
        .min(bounds.bottom() as f32) as u32;
    let y_end = (center_y + radius)
        .ceil()
        .max(bounds.y as f32)
        .min(bounds.bottom() as f32) as u32;
    let radius_sq = radius * radius;
    for y in y_start..y_end {
        let dy = y as f32 + 0.5 - center_y;
        let remaining = (radius_sq - dy * dy).max(0.0).sqrt();
        let x_start = (center_x - remaining)
            .floor()
            .max(bounds.x as f32)
            .min(bounds.right() as f32) as u32;
        let x_end = (center_x + remaining)
            .ceil()
            .max(bounds.x as f32)
            .min(bounds.right() as f32) as u32;
        let row = (y - bounds.y) as usize * bounds.width as usize;
        for x in x_start..x_end {
            pixels[row + (x - bounds.x) as usize] = 255;
        }
    }
}

/// Native pixels a Big-LaMa pass sends to the model: exactly its fixed input,
/// so a pass of this size is inferred without any resampling.
pub(super) const NATIVE_PASS_EDGE: u32 = BIG_LAMA_INPUT_EDGE;
/// Context kept around a target before it no longer fits a native pass.
pub(super) const MIN_CONTEXT_MARGIN: u32 = 64;
/// Thin strokes too long for one native pass are filled in tiles of this edge.
pub(super) const REMOVE_TILE_EDGE: u32 = 320;
/// Only strokes at most this many native pixels from edge to centre are thin
/// enough to tile: every tile then still sees background right beside the
/// stroke. Thicker objects are filled whole so no tile cuts through them.
pub(super) const MAX_TILED_HALF_WIDTH: f32 = 40.0;
/// Beyond this many tiles a stroke is filled in fewer, downscaled passes;
/// each pass costs about a second and a half of CPU inference.
pub(super) const MAX_TILED_PASSES: usize = 32;
/// Largest share of a crop that may be masked; the rest is the context the
/// model fills from.
pub(super) const MAX_MASKED_FRACTION: f32 = 0.4;
/// Context edge of a downscaled pass relative to its target's edge.
pub(super) const DOWNSCALED_CONTEXT_FACTOR: f32 = 1.75;

/// One Big-LaMa inference within a Remove stroke.
///
/// `target` holds the mask pixels this pass fills. Stroke pixels inside `crop`
/// that later passes fill stay masked, so the model never copies the object
/// being removed, while pixels filled by earlier passes are real context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovePass {
    pub crop: NativeRect,
    pub target: RemoveMask,
}
