//! Applying geometry edits to library thumbnails.

use super::*;

pub fn transform_thumbnail_geometry(
    thumbnail: &RawThumbnail,
    geometry: GeometryTransform,
) -> RawThumbnail {
    transform_thumbnail_geometry_with_lens(thumbnail, geometry, None)
}

pub fn transform_thumbnail_geometry_with_lens(
    thumbnail: &RawThumbnail,
    geometry: GeometryTransform,
    lens_geometry: Option<&LensGeometryMap>,
) -> RawThumbnail {
    let geometry = geometry.sanitized();
    if (geometry.is_identity() && lens_geometry.is_none())
        || thumbnail.width == 0
        || thumbnail.height == 0
    {
        return thumbnail.clone();
    }
    let (output_width, output_height) =
        geometry.crop_pixel_dimensions(thumbnail.width, thumbnail.height);
    let inverse_map = GeometryInverseMap::new_with_lens(
        geometry,
        lens_geometry,
        thumbnail.width,
        thumbnail.height,
        output_width,
        output_height,
    );
    let mut rgba = vec![0u8; output_width as usize * output_height as usize * 4];
    for output_y in 0..output_height {
        for output_x in 0..output_width {
            let [source_x, source_y] =
                inverse_map.source_position(output_x as f32, output_y as f32);
            let pixel = sample_thumbnail_rgba_bilinear(
                &thumbnail.rgba,
                thumbnail.width,
                thumbnail.height,
                source_x,
                source_y,
            );
            let index = (output_y as usize * output_width as usize + output_x as usize) * 4;
            rgba[index..index + 4].copy_from_slice(&pixel);
        }
    }
    RawThumbnail {
        width: output_width,
        height: output_height,
        rgba,
    }
}

fn sample_thumbnail_rgba_bilinear(
    source: &[u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
) -> [u8; 4] {
    if !x.is_finite()
        || !y.is_finite()
        || x < -0.5
        || y < -0.5
        || x > width as f32 - 0.5
        || y > height as f32 - 0.5
    {
        return [0, 0, 0, 255];
    }
    let x = x.clamp(0.0, width.saturating_sub(1) as f32);
    let y = y.clamp(0.0, height.saturating_sub(1) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(width.saturating_sub(1));
    let y1 = (y0 + 1).min(height.saturating_sub(1));
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let load = |px: u32, py: u32, channel: usize| -> f32 {
        let index = ((py as usize * width as usize + px as usize) * 4) + channel;
        source
            .get(index)
            .copied()
            .unwrap_or(if channel == 3 { 255 } else { 0 }) as f32
    };
    let mut result = [0u8; 4];
    for (channel, value) in result.iter_mut().enumerate() {
        let top = load(x0, y0, channel) * (1.0 - tx) + load(x1, y0, channel) * tx;
        let bottom = load(x0, y1, channel) * (1.0 - tx) + load(x1, y1, channel) * tx;
        *value = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
    }
    result
}
