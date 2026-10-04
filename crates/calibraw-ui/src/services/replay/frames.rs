//! Composition of replay video frames from rendered stills: fitting stills
//! to the canvas, transitions, stage titles and the brand outro. Frames are
//! tightly packed 8-bit sRGB `rgb24` rows.

use image::{imageops::FilterType, ImageBuffer, Rgb};

pub(super) const BRAND_TITLE: &str = "CalibRaw";
pub(super) const BRAND_SUBTITLE: &str = "A fast, GPU-accelerated open source RAW editor.";
const BRAND_ICON_PNG: &[u8] = include_bytes!("../../../../../packaging/icons/calibraw-256.png");

/// A developed edit state at replay resolution.
pub(super) struct RenderedStill {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgb: Vec<u8>,
}

pub(super) fn even_dimension(value: u32) -> u32 {
    let value = value.max(2);
    if value.is_multiple_of(2) {
        value
    } else {
        value - 1
    }
}

pub(super) fn replay_canvas_dimensions(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let (maximum_width, maximum_height) = if width >= height {
        (1920.0_f64, 1080.0_f64)
    } else {
        (1080.0_f64, 1920.0_f64)
    };
    let scale = (maximum_width / f64::from(width)).min(maximum_height / f64::from(height));
    let output_width = (f64::from(width) * scale).round().max(2.0) as u32;
    let output_height = (f64::from(height) * scale).round().max(2.0) as u32;
    (even_dimension(output_width), even_dimension(output_height))
}

pub(super) fn fit_to_canvas(
    still: &RenderedStill,
    canvas_width: u32,
    canvas_height: u32,
) -> Vec<u8> {
    let source = ImageBuffer::<Rgb<u8>, &[u8]>::from_raw(still.width, still.height, &still.rgb)
        .expect("rendered replay still has a valid RGB byte count");
    let scale = (canvas_width as f64 / still.width.max(1) as f64)
        .min(canvas_height as f64 / still.height.max(1) as f64);
    let width = (still.width as f64 * scale)
        .round()
        .clamp(1.0, canvas_width as f64) as u32;
    let height = (still.height as f64 * scale)
        .round()
        .clamp(1.0, canvas_height as f64) as u32;
    let resized;
    let pixels: &[u8] = if width == still.width && height == still.height {
        &still.rgb
    } else {
        resized = image::imageops::resize(&source, width, height, FilterType::Lanczos3);
        resized.as_raw()
    };
    let mut canvas = vec![18u8; canvas_width as usize * canvas_height as usize * 3];
    let x0 = (canvas_width - width) / 2;
    let y0 = (canvas_height - height) / 2;
    for y in 0..height {
        let source_start = y as usize * width as usize * 3;
        let destination_start = ((y + y0) as usize * canvas_width as usize + x0 as usize) * 3;
        let bytes = width as usize * 3;
        canvas[destination_start..destination_start + bytes]
            .copy_from_slice(&pixels[source_start..source_start + bytes]);
    }
    canvas
}

pub(super) fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

pub(super) fn crossfade(a: &[u8], b: &[u8], amount: f32, output: &mut [u8]) {
    let amount = amount.clamp(0.0, 1.0);
    let inverse = 1.0 - amount;
    for ((out, left), right) in output.iter_mut().zip(a.iter()).zip(b.iter()) {
        *out = (f32::from(*left) * inverse + f32::from(*right) * amount)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

pub(super) fn split_frame(
    original: &[u8],
    final_edit: &[u8],
    width: u32,
    height: u32,
    divider: f32,
    output: &mut [u8],
) {
    let split_x = (divider.clamp(0.0, 1.0) * width as f32).round() as u32;
    for y in 0..height {
        for x in 0..width {
            let index = (y as usize * width as usize + x as usize) * 3;
            let source = if x < split_x { original } else { final_edit };
            output[index..index + 3].copy_from_slice(&source[index..index + 3]);
        }
    }
    let divider_width = (width / 480).clamp(2, 5);
    let start = split_x.saturating_sub(divider_width / 2);
    let end = split_x.saturating_add(divider_width.div_ceil(2)).min(width);
    for y in 0..height {
        for x in start..end {
            let index = (y as usize * width as usize + x as usize) * 3;
            output[index..index + 3].copy_from_slice(&[238, 238, 238]);
        }
    }
}

fn blend_pixel(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    let alpha = f32::from(alpha) / 255.0;
    let inverse = 1.0 - alpha;
    for channel in 0..3 {
        pixel[channel] = (f32::from(pixel[channel]) * inverse + f32::from(color[channel]) * alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

fn glyph_rows(character: char) -> [u8; 7] {
    match character {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'a' => [0x00, 0x00, 0x0e, 0x01, 0x0f, 0x11, 0x0f],
        'b' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1e],
        'c' => [0x00, 0x00, 0x0e, 0x10, 0x10, 0x11, 0x0e],
        'd' => [0x01, 0x01, 0x0d, 0x13, 0x11, 0x11, 0x0f],
        'e' => [0x00, 0x00, 0x0e, 0x11, 0x1f, 0x10, 0x0e],
        'f' => [0x06, 0x08, 0x1e, 0x08, 0x08, 0x08, 0x08],
        'i' => [0x04, 0x00, 0x0c, 0x04, 0x04, 0x04, 0x0e],
        'l' => [0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0e, 0x11, 0x11, 0x11, 0x0e],
        'p' => [0x00, 0x00, 0x1e, 0x11, 0x1e, 0x10, 0x10],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0f, 0x10, 0x0e, 0x01, 0x1e],
        't' => [0x08, 0x08, 0x1e, 0x08, 0x08, 0x09, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0d],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0a],
        '-' => [0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x06, 0x06, 0x04],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],
        ' ' => [0; 7],
        _ => [0; 7],
    }
}

fn bitmap_text_width(text: &str, scale: i32) -> i32 {
    let count = text.chars().count() as i32;
    if count == 0 {
        return 0;
    }
    count * 5 * scale + (count - 1) * scale
}

/// One line of the built-in bitmap font: where it starts, how large it is and how it blends.
#[derive(Clone, Copy)]
struct BitmapTextRun<'a> {
    text: &'a str,
    x: i32,
    y: i32,
    scale: i32,
    color: [u8; 3],
    alpha: u8,
}

fn draw_bitmap_text(frame: &mut [u8], width: u32, height: u32, run: BitmapTextRun<'_>) {
    let BitmapTextRun {
        text,
        x: start_x,
        y: start_y,
        scale,
        color,
        alpha,
    } = run;
    let mut cursor_x = start_x;
    for character in text.chars() {
        let rows = glyph_rows(character);
        for (row, bits) in rows.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = cursor_x + column * scale + sx;
                        let y = start_y + row as i32 * scale + sy;
                        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                            continue;
                        }
                        let index = (y as usize * width as usize + x as usize) * 3;
                        blend_pixel(&mut frame[index..index + 3], color, alpha);
                    }
                }
            }
        }
        cursor_x += 6 * scale;
    }
}

fn fitted_bitmap_scale(text: &str, max_width: u32, preferred: u32) -> i32 {
    let units = (text.chars().count().max(1) * 6 - 1) as u32;
    (max_width / units).min(preferred).max(1) as i32
}

pub(super) fn brand_outro_frame(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut frame = vec![0u8; width as usize * height as usize * 3];
    let icon = image::load_from_memory(BRAND_ICON_PNG)
        .map_err(|error| format!("Could not decode embedded CalibRaw icon: {error}"))?
        .into_rgb8();
    let short_edge = width.min(height);
    let icon_side = ((short_edge as f32 * 0.18).round() as u32).clamp(96, 256);
    let icon = image::imageops::resize(&icon, icon_side, icon_side, FilterType::Lanczos3);

    let brand_scale =
        fitted_bitmap_scale(BRAND_TITLE, width * 3 / 4, (short_edge / 90).clamp(7, 14));
    let subtitle_scale = fitted_bitmap_scale(
        BRAND_SUBTITLE,
        width * 9 / 10,
        (short_edge / 230).clamp(3, 6),
    );
    let brand_height = 7 * brand_scale;
    let subtitle_height = 7 * subtitle_scale;
    let gap_after_icon = (short_edge as f32 * 0.055).round() as i32;
    let gap_after_title = (short_edge as f32 * 0.035).round() as i32;
    let group_height =
        icon_side as i32 + gap_after_icon + brand_height + gap_after_title + subtitle_height;
    let group_top = ((height as i32 - group_height) / 2).max(0);

    let icon_x = (width.saturating_sub(icon_side) / 2) as usize;
    let icon_y = group_top as usize;
    for y in 0..icon_side as usize {
        let source_start = y * icon_side as usize * 3;
        let destination_start = ((icon_y + y) * width as usize + icon_x) * 3;
        let bytes = icon_side as usize * 3;
        frame[destination_start..destination_start + bytes]
            .copy_from_slice(&icon.as_raw()[source_start..source_start + bytes]);
    }

    let brand_y = group_top + icon_side as i32 + gap_after_icon;
    let brand_x = (width as i32 - bitmap_text_width(BRAND_TITLE, brand_scale)) / 2;
    draw_bitmap_text(
        &mut frame,
        width,
        height,
        BitmapTextRun {
            text: BRAND_TITLE,
            x: brand_x,
            y: brand_y,
            scale: brand_scale,
            color: [255, 255, 255],
            alpha: 255,
        },
    );

    let subtitle_y = brand_y + brand_height + gap_after_title;
    let subtitle_x = (width as i32 - bitmap_text_width(BRAND_SUBTITLE, subtitle_scale)) / 2;
    draw_bitmap_text(
        &mut frame,
        width,
        height,
        BitmapTextRun {
            text: BRAND_SUBTITLE,
            x: subtitle_x,
            y: subtitle_y,
            scale: subtitle_scale,
            color: [205, 205, 205],
            alpha: 255,
        },
    );
    Ok(frame)
}

fn rounded_rect_contains(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
    if x < 0 || y < 0 || x >= width || y >= height {
        return false;
    }
    if x >= radius && x < width - radius || y >= radius && y < height - radius {
        return true;
    }
    let cx = if x < radius {
        radius
    } else {
        width - radius - 1
    };
    let cy = if y < radius {
        radius
    } else {
        height - radius - 1
    };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

pub(super) fn draw_stage_title(frame: &mut [u8], width: u32, height: u32, title: &str, alpha: f32) {
    let title = title.to_ascii_uppercase();
    let scale = (width.min(height) / 280).clamp(3, 6) as i32;
    let glyph_width = 5 * scale;
    let spacing = scale;
    let text_width = title.chars().count() as i32 * (glyph_width + spacing) - spacing;
    let padding_x = 4 * scale;
    let padding_y = 3 * scale;
    let box_width = text_width + padding_x * 2;
    let box_height = 7 * scale + padding_y * 2;
    let box_x = (width as i32 - box_width) / 2;
    let box_y = (height as f32 * 0.045).round() as i32;
    let radius = 3 * scale;
    let alpha = alpha.clamp(0.0, 1.0);
    let background_alpha = (176.0 * alpha).round() as u8;
    let text_alpha = (245.0 * alpha).round() as u8;

    for local_y in 0..box_height {
        for local_x in 0..box_width {
            if !rounded_rect_contains(local_x, local_y, box_width, box_height, radius) {
                continue;
            }
            let x = box_x + local_x;
            let y = box_y + local_y;
            if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                continue;
            }
            let index = (y as usize * width as usize + x as usize) * 3;
            blend_pixel(&mut frame[index..index + 3], [8, 8, 8], background_alpha);
        }
    }

    draw_bitmap_text(
        frame,
        width,
        height,
        BitmapTextRun {
            text: &title,
            x: box_x + padding_x,
            y: box_y + padding_y,
            scale,
            color: [255, 255, 255],
            alpha: text_alpha,
        },
    );
}

pub(super) fn stage_title_alpha(frame: u32, transition_frames: u32, hold_frames: u32) -> f32 {
    let total = transition_frames + hold_frames;
    let fade = 6u32.min(total / 2).max(1);
    if frame < fade {
        smootherstep((frame + 1) as f32 / fade as f32)
    } else if frame + fade >= total {
        smootherstep((total - frame) as f32 / fade as f32)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_dimensions_are_even_for_h264_420() {
        assert_eq!(even_dimension(1920), 1920);
        assert_eq!(even_dimension(1279), 1278);
        assert_eq!(even_dimension(1), 2);
    }

    #[test]
    fn replay_canvas_targets_1080p_in_both_orientations() {
        assert_eq!(replay_canvas_dimensions(6000, 4000), (1620, 1080));
        assert_eq!(replay_canvas_dimensions(4000, 6000), (1080, 1620));
        assert_eq!(replay_canvas_dimensions(4000, 3000), (1440, 1080));
        assert_eq!(replay_canvas_dimensions(3000, 4000), (1080, 1440));
    }

    #[test]
    fn smootherstep_has_stable_endpoints() {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert_eq!(smootherstep(-1.0), 0.0);
        assert_eq!(smootherstep(2.0), 1.0);
    }

    #[test]
    fn bitmap_font_covers_every_stage_title() {
        for title in [
            "EDIT",
            "CROP",
            "ROTATE",
            "TRANSFORM",
            "MASKS",
            "EFFECTS",
            "REMOVE",
        ] {
            for character in title.chars() {
                assert_ne!(glyph_rows(character), [0; 7], "missing glyph {character}");
            }
        }
    }

    #[test]
    fn bitmap_font_covers_brand_outro_copy() {
        for text in [BRAND_TITLE, BRAND_SUBTITLE] {
            for character in text.chars().filter(|character| *character != ' ') {
                assert_ne!(glyph_rows(character), [0; 7], "missing glyph {character}");
            }
        }
    }
}
