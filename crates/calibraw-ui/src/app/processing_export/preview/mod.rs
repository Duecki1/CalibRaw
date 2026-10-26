use super::*;
use crate::pipeline::TONE_GUIDE_CELL_SIZE;
pub(in crate::app) use crate::pipeline::{mask_region_texture_extent, mask_source_region_uv};

pub(in crate::app) fn aligned_detail_axis(
    min_uv: f32,
    max_uv: f32,
    extent: u32,
    cfa_period: u32,
    viewport_pixels: u32,
    pixel_scale: f32,
    processing_halo: u32,
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

    let visible_detail_pixels = (viewport_pixels.max(1) as f32 * pixel_scale.max(0.1)).max(1.0);
    let support_padding =
        (visible_len as f32 * processing_halo as f32 / visible_detail_pixels).ceil() as u32;
    // Reserve a small reusable border in addition to processing support.
    let padding = support_padding
        .max(processing_halo)
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
    processing_halo: u32,
) -> PreviewUvRect {
    let mut display = visible;
    for (axis, extent) in texture_size.into_iter().enumerate() {
        let halo =
            processing_halo as f32 / extent.max(1) as f32 * (crop.max[axis] - crop.min[axis]);
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

/// The long edge of the crop's texture. `canvas_pixels` is the physical size of
/// the canvas, which bounds it; `visible_pixels` is the physical size of the
/// part of the image in view along each source axis, which can be smaller than
/// the canvas (a photo that does not fill it), and sets the density.
pub(in crate::app) fn requested_detail_edge(
    quality: PreviewQuality,
    canvas_pixels: [u32; 2],
    visible_pixels: [u32; 2],
    visible: PreviewUvRect,
    crop_size: [u32; 2],
    full_size: [u32; 2],
    processing_halo: u32,
) -> u32 {
    let [crop_width, crop_height] = crop_size;
    let [full_width, full_height] = full_size;
    let visible_source_width =
        ((visible.max[0] - visible.min[0]).max(1.0 / full_width.max(1) as f32) * full_width as f32)
            .max(1.0);
    let visible_source_height = ((visible.max[1] - visible.min[1])
        .max(1.0 / full_height.max(1) as f32)
        * full_height as f32)
        .max(1.0);
    let padded_width_pixels =
        visible_pixels[0].max(1) as f32 * crop_width as f32 / visible_source_width;
    let padded_height_pixels =
        visible_pixels[1].max(1) as f32 * crop_height as f32 / visible_source_height;
    (padded_width_pixels.max(padded_height_pixels) * quality.pixel_scale())
        .ceil()
        .clamp(
            256.0,
            quality
                .detail_edge_for_viewport(canvas_pixels)
                .saturating_add(processing_halo.saturating_mul(2)) as f32,
        ) as u32
}

/// What the zoomed crop's graph is sized for besides its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct DetailGraph {
    /// The masks it renders, which size its atlas.
    pub mask_count: usize,
    /// What the fitted graph has reserved, which the crop's budget leaves out.
    pub fitted_resident_bytes: u64,
}

/// How the zoomed crop's graph is built: the atlas and the mask layers that
/// `DetailGraph::preview_graph` sizes it for.
fn detail_pipeline_options<'a>(mask_count: usize) -> PipelineOptions<'a> {
    PipelineOptions::new(ProcessingQuality::Preview)
        .mask_atlas_edge(detail_mask_edge())
        .mask_layers(detail_mask_layers(mask_count))
}

impl DetailGraph {
    fn preview_graph(self) -> PreviewGraph {
        PreviewGraph::Detail {
            mask_count: self.mask_count,
            mask_atlas_edge: detail_mask_edge(),
            fitted_resident_bytes: self.fitted_resident_bytes,
        }
    }
}

/// The screen the detail crop is rendered for, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct DetailView {
    /// The canvas, which bounds the crop's size.
    pub canvas_pixels: [u32; 2],
    /// The part of the image in view along each source axis, which sets the
    /// crop's density. See `PreviewState::detail_view`.
    pub visible_pixels: [u32; 2],
}

/// The next crop and its density are planned independently of the cached crop.
/// Both preparation and cache validation must use this same request geometry.
struct PreviewDetailPlan {
    origin: [u32; 2],
    size: [u32; 2],
    edge: u32,
    graph: DetailGraph,
}

impl PreviewDetailPlan {
    fn new(
        full_size: [u32; 2],
        cfa_kind: crate::pipeline::CfaKind,
        visible: PreviewUvRect,
        view: DetailView,
        quality: PreviewQuality,
        processing_halo: u32,
        graph: DetailGraph,
    ) -> Self {
        let period = match cfa_kind {
            crate::pipeline::CfaKind::Bayer => 2,
            crate::pipeline::CfaKind::XTrans => 6,
        };
        let axes = [0, 1].map(|axis| {
            aligned_detail_axis(
                visible.min[axis],
                visible.max[axis],
                full_size[axis],
                period,
                view.visible_pixels[axis],
                quality.pixel_scale(),
                processing_halo,
            )
        });
        let origin = axes.map(|(start, _)| start);
        let size = axes.map(|(start, end)| end - start);
        let edge = requested_detail_edge(
            quality,
            view.canvas_pixels,
            view.visible_pixels,
            visible,
            size,
            full_size,
            processing_halo,
        );
        let edge = PreviewQuality::bounded_source_edge(
            size[0],
            size[1],
            edge,
            cfa_kind,
            graph.preview_graph(),
        );
        Self {
            origin,
            size,
            edge,
            graph,
        }
    }

    fn sampling_scale(&self) -> f64 {
        (f64::from(self.edge) / f64::from(self.size[0].max(self.size[1]).max(1))).min(1.0)
    }
}

pub(in crate::app) fn navigation_proxy_edge() -> u32 {
    if PlatformCaps::CURRENT.constrained_memory() {
        384
    } else {
        512
    }
}

pub(in crate::app) fn navigation_mask_edge() -> u32 {
    if PlatformCaps::CURRENT.constrained_memory() {
        256
    } else {
        384
    }
}

pub(in crate::app) fn detail_mask_edge() -> u32 {
    if PlatformCaps::CURRENT.constrained_memory() {
        1024
    } else {
        2048
    }
}

/// The mask atlas of the reduced zoomed crop: half the sharp crop's, matching
/// its half-size graph.
pub(in crate::app) fn interactive_detail_mask_edge() -> u32 {
    detail_mask_edge() / 2
}

pub(in crate::app) fn detail_mask_source_region(
    masks: &MaskStack,
    source_origin: [u32; 2],
    source_size: [u32; 2],
    full_width: u32,
    full_height: u32,
) -> [u32; 4] {
    mask_region_with_margin(
        source_origin,
        source_size,
        [full_width, full_height],
        masks.raster_margin_pixels(full_width, full_height),
    )
}

fn mask_region_with_margin(
    source_origin: [u32; 2],
    source_size: [u32; 2],
    full_size: [u32; 2],
    margin: u32,
) -> [u32; 4] {
    let [full_width, full_height] = full_size;
    let full_width = full_width.max(1);
    let full_height = full_height.max(1);
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

fn detail_mask_update_region(
    masks: &MaskStack,
    source_origin: [u32; 2],
    source_size: [u32; 2],
    full_size: [u32; 2],
    cached_region: Option<[u32; 4]>,
    interactive: bool,
) -> [u32; 4] {
    let required = if interactive {
        // Reserve the entire grow/feather slider range once. Changing the crop
        // every tick invalidates distance caches and reuploads every mask layer.
        let slider_margin = (full_size[0].min(full_size[1]) as f32 * 0.095 + 2.0).ceil() as u32;
        let margin = masks
            .raster_margin_pixels(full_size[0], full_size[1])
            .max(slider_margin);
        mask_region_with_margin(source_origin, source_size, full_size, margin)
    } else {
        detail_mask_source_region(
            masks,
            source_origin,
            source_size,
            full_size[0],
            full_size[1],
        )
    };
    cached_region
        .filter(|cached| {
            (0..2).all(|axis| {
                cached[axis] <= required[axis]
                    && cached[axis].saturating_add(cached[axis + 2])
                        >= required[axis].saturating_add(required[axis + 2])
            })
        })
        .unwrap_or(required)
}

fn detail_mask_texture_extent(region: [u32; 4], atlas_edge: u32, interactive: bool) -> [u32; 2] {
    let edge = if interactive {
        atlas_edge.min(if PlatformCaps::CURRENT.constrained_memory() {
            512
        } else {
            768
        })
    } else {
        atlas_edge
    };
    mask_region_texture_extent(region, edge)
}

pub(super) const DETAIL_ZOOM_START: f32 = 1.0005;

pub(in crate::app) fn zoom_detail_idle_delay() -> Duration {
    Duration::from_millis(if PlatformCaps::CURRENT.constrained_memory() {
        220
    } else {
        140
    })
}

mod detail;
mod interactive_crop;
mod navigation;
mod processing;
mod rebuild;
mod state;
use detail::{detail_virtual_frame, DetailPlacement};
use processing::dispatch_detail_stage;
use state::full_frame_tone_pipeline;

#[cfg(test)]
mod mask_interaction_tests;

// Compare samples per source pixel, not the fraction of the cached texture
// currently on screen. Using that fraction to lower the budget lets a capped
// texture pass forever as the user zooms farther into it.
fn detail_covers_view(
    coverage: PreviewUvRect,
    texture_size: [u32; 2],
    source_size: [u32; 2],
    visible: PreviewUvRect,
    requested: &PreviewDetailPlan,
) -> bool {
    (0..2).all(|axis| {
        let contains = coverage.min[axis] <= visible.min[axis] + 1e-6
            && coverage.max[axis] >= visible.max[axis] - 1e-6;
        let required = requested.sampling_scale() * f64::from(source_size[axis]);
        // CFA dimensions are rounded down to a complete mosaic period. Allow
        // that rounding in the NEW crop, scaled to the cached source extent.
        let phase_guard = if requested.sampling_scale() < 1.0 {
            6.0 * f64::from(source_size[axis]) / f64::from(requested.size[axis].max(1))
        } else {
            0.0
        };
        contains && (f64::from(texture_size[axis]) + phase_guard >= required)
    })
}
