/// Map a native mask region to normalized source coordinates.
/// Saturating end coordinates preserve regions near the integer limit.
pub fn mask_source_region_uv(region: [u32; 4], full_width: u32, full_height: u32) -> [f32; 4] {
    let width = full_width.max(1) as f32;
    let height = full_height.max(1) as f32;
    [
        region[0] as f32 / width,
        region[1] as f32 / height,
        region[0].saturating_add(region[2]) as f32 / width,
        region[1].saturating_add(region[3]) as f32 / height,
    ]
}

/// Fit a mask region in the atlas while preserving its aspect ratio.
/// Empty regions and a zero edge limit retain a minimum one-pixel extent.
pub fn mask_region_texture_extent(region: [u32; 4], max_edge: u32) -> [u32; 2] {
    let width = region[2].max(1);
    let height = region[3].max(1);
    let longest = width.max(height);
    if longest <= max_edge {
        return [width, height];
    }
    let scale = max_edge.max(1) as f64 / longest as f64;
    [
        ((width as f64 * scale).round() as u32).clamp(1, max_edge.max(1)),
        ((height as f64 * scale).round() as u32).clamp(1, max_edge.max(1)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_region_preserves_full_frame_and_offset_coordinates() {
        assert_eq!(
            mask_source_region_uv([0, 0, 400, 200], 400, 200),
            [0.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(
            mask_source_region_uv([100, 50, 200, 100], 400, 200),
            [0.25, 0.25, 0.75, 0.75]
        );
        assert_eq!(
            mask_source_region_uv([u32::MAX, u32::MAX, 2, 2], u32::MAX, u32::MAX),
            [1.0; 4]
        );
        assert_eq!(mask_source_region_uv([0, 0, 0, 0], 0, 0), [0.0; 4]);
    }

    #[test]
    fn texture_extent_preserves_aspect_ratio_and_atlas_limits() {
        assert_eq!(
            mask_region_texture_extent([5, 8, 600, 400], 1024),
            [600, 400]
        );
        assert_eq!(
            mask_region_texture_extent([5, 8, 600, 400], 300),
            [300, 200]
        );
        assert_eq!(
            mask_region_texture_extent([5, 8, 400, 600], 300),
            [200, 300]
        );
        assert_eq!(
            mask_region_texture_extent([0, 0, 1, u32::MAX], 512),
            [1, 512]
        );
        assert_eq!(mask_region_texture_extent([0; 4], 0), [1, 1]);
    }
}
