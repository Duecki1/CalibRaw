use super::*;
use crate::pipeline::TONE_GUIDE_CELL_SIZE;

pub(in crate::app) fn aligned_detail_axis(
    min_uv: f32,
    max_uv: f32,
    extent: u32,
    cfa_period: u32,
    viewport_pixels: u32,
    detail_pixel_scale: f32,
) -> (u32, u32) {
    let extent = extent.max(1);
    let cfa_period = cfa_period.max(1);
    let gcd = |mut a: u32, mut b: u32| {
        while b != 0 {
            let remainder = a % b;
            a = b;
            b = remainder;
        }
        a.max(1)
    };
    let period = cfa_period
        .checked_div(gcd(cfa_period, TONE_GUIDE_CELL_SIZE))
        .and_then(|value| value.checked_mul(TONE_GUIDE_CELL_SIZE))
        .unwrap_or(cfa_period.max(TONE_GUIDE_CELL_SIZE));
    let visible_start =
        ((min_uv.clamp(0.0, 1.0) * extent as f32).floor() as u32).min(extent.saturating_sub(1));
    let visible_end =
        ((max_uv.clamp(0.0, 1.0) * extent as f32).ceil() as u32).clamp(visible_start + 1, extent);
    let visible_len = visible_end - visible_start;

    let visible_detail_pixels =
        (viewport_pixels.max(1) as f32 * detail_pixel_scale.max(0.1)).max(1.0);
    let support_padding =
        (visible_len as f32 * EXPORT_TILE_HALO as f32 / visible_detail_pixels).ceil() as u32;
    // Reserve a small reusable border in addition to processing support.
    let padding = support_padding
        .max(EXPORT_TILE_HALO)
        .saturating_add((visible_len as f32 * 0.08).ceil() as u32);
    let padded_start = visible_start.saturating_sub(padding);
    let padded_end = visible_end.saturating_add(padding).min(extent);
    let aligned_start = (padded_start / period) * period;
    let aligned_end = padded_end
        .div_ceil(period)
        .saturating_mul(period)
        .min(extent)
        .max(aligned_start + 1);
    (aligned_start, aligned_end)
}

pub(in crate::app) fn detail_texture_uv(
    visible: PreviewUvRect,
    crop: PreviewUvRect,
) -> PreviewUvRect {
    let crop_width = (crop.max[0] - crop.min[0]).max(f32::EPSILON);
    let crop_height = (crop.max[1] - crop.min[1]).max(f32::EPSILON);
    PreviewUvRect {
        min: [
            ((visible.min[0] - crop.min[0]) / crop_width).clamp(0.0, 1.0),
            ((visible.min[1] - crop.min[1]) / crop_height).clamp(0.0, 1.0),
        ],
        max: [
            ((visible.max[0] - crop.min[0]) / crop_width).clamp(0.0, 1.0),
            ((visible.max[1] - crop.min[1]) / crop_height).clamp(0.0, 1.0),
        ],
    }
}

fn detail_display_uv(
    visible: PreviewUvRect,
    crop: PreviewUvRect,
    texture_size: [u32; 2],
) -> PreviewUvRect {
    let mut display = visible;
    for (axis, extent) in texture_size.into_iter().enumerate() {
        let halo =
            EXPORT_TILE_HALO as f32 / extent.max(1) as f32 * (crop.max[axis] - crop.min[axis]);
        let safe_min = if crop.min[axis] <= 0.0 {
            0.0
        } else {
            crop.min[axis] + halo
        };
        let safe_max = if crop.max[axis] >= 1.0 {
            1.0
        } else {
            crop.max[axis] - halo
        };
        display.min[axis] = visible.min[axis].min(safe_min).max(crop.min[axis]);
        display.max[axis] = visible.max[axis].max(safe_max).min(crop.max[axis]);
    }
    display
}

pub(in crate::app) fn requested_detail_edge(
    quality: PreviewQuality,
    viewport_pixels: [u32; 2],
    visible: PreviewUvRect,
    crop_width: u32,
    crop_height: u32,
    full_width: u32,
    full_height: u32,
) -> u32 {
    let visible_source_width =
        ((visible.max[0] - visible.min[0]).max(1.0 / full_width.max(1) as f32) * full_width as f32)
            .max(1.0);
    let visible_source_height = ((visible.max[1] - visible.min[1])
        .max(1.0 / full_height.max(1) as f32)
        * full_height as f32)
        .max(1.0);
    let padded_width_pixels =
        viewport_pixels[0].max(1) as f32 * crop_width as f32 / visible_source_width;
    let padded_height_pixels =
        viewport_pixels[1].max(1) as f32 * crop_height as f32 / visible_source_height;
    (padded_width_pixels.max(padded_height_pixels) * quality.detail_pixel_scale())
        .ceil()
        .clamp(
            256.0,
            quality.detail_edge_for_viewport(viewport_pixels) as f32,
        ) as u32
}

pub(in crate::app) fn navigation_proxy_edge() -> u32 {
    if cfg!(target_os = "android") {
        384
    } else {
        512
    }
}

pub(in crate::app) fn navigation_mask_edge() -> u32 {
    if cfg!(target_os = "android") {
        256
    } else {
        384
    }
}

pub(in crate::app) fn detail_mask_edge() -> u32 {
    if cfg!(target_os = "android") {
        1024
    } else {
        2048
    }
}

pub(in crate::app) fn detail_uses_opposed_chroma(
    raw: &LoadedRaw,
    exposure: &ExposureParams,
) -> bool {
    raw.uses_opposed_chroma(exposure)
}

pub(in crate::app) fn detail_mask_source_region(
    masks: &MaskStack,
    source_origin: [u32; 2],
    source_size: [u32; 2],
    full_width: u32,
    full_height: u32,
) -> [u32; 4] {
    let full_width = full_width.max(1);
    let full_height = full_height.max(1);
    let margin = masks.raster_margin_pixels(full_width, full_height);
    let x0 = source_origin[0].min(full_width - 1).saturating_sub(margin);
    let y0 = source_origin[1].min(full_height - 1).saturating_sub(margin);
    let x1 = source_origin[0]
        .saturating_add(source_size[0])
        .saturating_add(margin)
        .clamp(x0 + 1, full_width);
    let y1 = source_origin[1]
        .saturating_add(source_size[1])
        .saturating_add(margin)
        .clamp(y0 + 1, full_height);
    [x0, y0, x1 - x0, y1 - y0]
}

pub(in crate::app) fn mask_source_region_uv(
    region: [u32; 4],
    full_width: u32,
    full_height: u32,
) -> [f32; 4] {
    let width = full_width.max(1) as f32;
    let height = full_height.max(1) as f32;
    [
        region[0] as f32 / width,
        region[1] as f32 / height,
        region[0].saturating_add(region[2]) as f32 / width,
        region[1].saturating_add(region[3]) as f32 / height,
    ]
}

pub(in crate::app) fn mask_region_texture_extent(region: [u32; 4], max_edge: u32) -> [u32; 2] {
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

pub(super) const DETAIL_ZOOM_START: f32 = 1.0005;

pub(in crate::app) fn zoom_detail_idle_delay() -> Duration {
    Duration::from_millis(if cfg!(target_os = "android") {
        220
    } else {
        140
    })
}

mod detail;
mod navigation;
mod processing;
mod rebuild;
mod state;

// Check coverage and sampling density independently of navigation. A native
// crop never needs upsampling on the GPU, even when displayed beyond 100%.
fn detail_covers_view(
    coverage: PreviewUvRect,
    texture_uv: PreviewUvRect,
    texture_size: [u32; 2],
    source_size: [u32; 2],
    visible: PreviewUvRect,
    viewport: [u32; 2],
    quality: PreviewQuality,
) -> bool {
    (0..2).all(|axis| {
        let contains = coverage.min[axis] <= visible.min[axis] + 1e-6
            && coverage.max[axis] >= visible.max[axis] - 1e-6;
        let native = texture_size[axis] >= source_size[axis];
        let coverage_span = (coverage.max[axis] - coverage.min[axis]).max(f32::EPSILON);
        let visible_span = (visible.max[axis] - visible.min[axis]).max(f32::EPSILON);
        let samples = texture_size[axis] as f32
            * (texture_uv.max[axis] - texture_uv.min[axis])
            * visible_span
            / coverage_span;
        // Respect the same padded-working-edge budget as preparation. Otherwise
        // an aspect-mismatched/rotated view can rebuild forever at the cap.
        let budget_samples = quality.detail_edge_for_viewport(viewport) as f32
            * texture_size[axis] as f32
            / texture_size[0].max(texture_size[1]).max(1) as f32
            * (texture_uv.max[axis] - texture_uv.min[axis])
            * visible_span
            / coverage_span;
        let required = (viewport[axis] as f32 * quality.detail_pixel_scale()).min(budget_samples);
        // CFA proxy dimensions are rounded down to a complete mosaic period.
        contains && (native || samples + 6.0 >= required)
    })
}

#[cfg(test)]
mod detail_resolution_tests {
    use super::*;

    fn uv(min: f32, max: f32) -> PreviewUvRect {
        PreviewUvRect {
            min: [min; 2],
            max: [max; 2],
        }
    }

    #[test]
    fn native_detail_is_reusable_when_zooming_further_into_its_coverage() {
        assert!(detail_covers_view(
            uv(0.2, 0.8),
            uv(0.1, 0.9),
            [1200; 2],
            [1200; 2],
            uv(0.4, 0.6),
            [2400; 2],
            PreviewQuality::Max
        ));
    }

    #[test]
    fn cached_detail_must_cover_every_visible_edge() {
        for visible in [uv(0.19, 0.7), uv(0.3, 0.81), uv(0.0, 1.0)] {
            assert!(!detail_covers_view(
                uv(0.2, 0.8),
                uv(0.0, 1.0),
                [1200; 2],
                [1200; 2],
                visible,
                [1000; 2],
                PreviewQuality::Medium
            ));
        }
    }

    #[test]
    fn proxy_detail_refreshes_on_zoom_dpi_or_quality_increase() {
        let coverage = uv(0.2, 0.8);
        let texture = uv(0.0, 1.0);
        assert!(detail_covers_view(
            coverage,
            texture,
            [1200; 2],
            [3000; 2],
            coverage,
            [1200; 2],
            PreviewQuality::Medium
        ));
        assert!(!detail_covers_view(
            coverage,
            texture,
            [1200; 2],
            [3000; 2],
            uv(0.4, 0.6),
            [1200; 2],
            PreviewQuality::Medium
        ));
        assert!(!detail_covers_view(
            coverage,
            texture,
            [1200; 2],
            [3000; 2],
            coverage,
            [2400; 2],
            PreviewQuality::Medium
        ));
        assert!(!detail_covers_view(
            coverage,
            texture,
            [1200; 2],
            [3000; 2],
            coverage,
            [1200; 2],
            PreviewQuality::Max
        ));
    }

    #[test]
    fn working_edge_cap_and_cfa_rounding_do_not_trigger_endless_rebuilds() {
        let viewport = [1000, 1800];
        let edge = PreviewQuality::Medium.detail_edge_for_viewport(viewport);
        assert!(detail_covers_view(
            uv(0.2, 0.8),
            uv(0.1, 0.9),
            [edge - 5, (edge - 5) / 2],
            [6000, 3000],
            uv(0.2, 0.8),
            viewport,
            PreviewQuality::Medium
        ));
    }

    #[test]
    fn reusable_border_excludes_processing_halo_and_includes_the_visible_region() {
        let crop = uv(0.1, 0.9);
        let visible = uv(0.3, 0.7);
        let edge = EXPORT_TILE_HALO * 10;
        let display = detail_display_uv(visible, crop, [edge; 2]);
        assert!((display.min[0] - 0.18).abs() < 1e-6);
        assert!((display.max[0] - 0.82).abs() < 1e-6);
        assert!(display.min[0] <= visible.min[0] && display.max[0] >= visible.max[0]);
        let full = detail_display_uv(visible, uv(0.0, 1.0), [edge; 2]);
        assert_eq!(full.min, [0.0; 2]);
        assert_eq!(full.max, [1.0; 2]);
    }

    #[test]
    fn detail_crop_origin_uses_the_shared_cfa_and_tone_grid() {
        let extent = 6_017;
        let (bayer_start, bayer_end) = aligned_detail_axis(0.173, 0.481, extent, 2, 1_600, 1.0);
        let (xtrans_start, xtrans_end) = aligned_detail_axis(0.173, 0.481, extent, 6, 1_600, 1.0);

        let gcd = |mut a: u32, mut b: u32| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a.max(1)
        };
        let lcm = |a: u32, b: u32| a / gcd(a, b) * b;

        assert_eq!(bayer_start % lcm(2, TONE_GUIDE_CELL_SIZE), 0);
        assert_eq!(xtrans_start % lcm(6, TONE_GUIDE_CELL_SIZE), 0);
        assert!(bayer_end > bayer_start);
        assert!(xtrans_end > xtrans_start);

        let visible_start = (0.173_f32 * extent as f32).floor() as u32;
        let visible_end = (0.481_f32 * extent as f32).ceil() as u32;
        assert!(bayer_start <= visible_start && bayer_end >= visible_end);
        assert!(xtrans_start <= visible_start && xtrans_end >= visible_end);
    }

    #[test]
    fn medium_zoom_detail_matches_the_physical_viewport_density() {
        let visible = PreviewUvRect {
            min: [0.25, 0.25],
            max: [0.75, 0.75],
        };
        let medium = requested_detail_edge(
            PreviewQuality::Medium,
            [3_000, 2_000],
            visible,
            3_500,
            2_344,
            7_000,
            4_688,
        );
        let low = requested_detail_edge(
            PreviewQuality::Low,
            [3_000, 2_000],
            visible,
            3_500,
            2_344,
            7_000,
            4_688,
        );

        assert_eq!(medium, 3_000);
        assert!(low < medium);
    }
}
