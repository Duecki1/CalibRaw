//! How large a preview graph may be. A phone or tablet shares one GPU
//! working-set limit between the fitted preview, the zoomed crop, the small
//! navigation graph and an export tile, so each preview graph's edge is
//! bounded against a share of it with the same accounting that admits the
//! allocation (`build_gpu_resource_plan`); and a caller can ask what a graph
//! of any size reserves before building one.

use super::resources::{
    build_gpu_resource_plan, gpu_working_set_limit_bytes, interactive_mask_atlas_edge,
    mobile_preview_limit_bytes, tone_analysis_scale, GpuResourcePlan, GpuResourcePlanInput,
};
use super::{
    ProcessingQuality, RawGpuPipeline, ANDROID_GPU_WORKING_SET_LIMIT_BYTES,
    GPU_STAGE_UNIFORM_ALLOCATION_BYTES, MASK_DATA_SIZE_BYTES,
};
use crate::pipeline::{CfaKind, PixelLayout, MAX_LOCAL_MASKS};
use anyhow::Result;

/// Which preview graph a mobile size bound is for. The fitted graph and the
/// zoomed detail graph are resident together, so each gets a share of the
/// preview budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MobilePreviewRole {
    Fitted,
    Detail,
}

/// The parts of a preview graph's footprint that do not scale with its
/// pixels, which the size bound has to account for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewResourceShape {
    /// The mask atlas edge the graph is built with; `None` sizes it from the
    /// image, as the fitted preview does.
    pub mask_atlas_edge: Option<u32>,
    /// The graph's mask layer capacity (`PipelineOptions::mask_layers`).
    pub mask_layers: usize,
    /// The resident bytes of the fitted graph (`RawGpuPipeline::resident_bytes`),
    /// which the budget of a detail graph leaves out. Zero when there is none
    /// yet, which assumes the fitted graph takes its whole share.
    pub fitted_resident_bytes: u64,
}

/// Mask layers a fitted preview graph is built with beyond the masks it
/// already needs, in whole steps.
const PREVIEW_MASK_LAYER_STEP: usize = 4;

/// A preview graph sized for `mask_count` masks: the count rounded up to a
/// whole step, so adding a mask rebuilds the graph only every few masks.
pub fn preview_mask_layer_capacity(mask_count: usize) -> usize {
    mask_count
        .clamp(1, MAX_LOCAL_MASKS)
        .next_multiple_of(PREVIEW_MASK_LAYER_STEP)
        .min(MAX_LOCAL_MASKS)
}

/// Profile LUTs the preview graph reserves in addition to its processing
/// surfaces.
const PREVIEW_PROFILE_BUFFER_BYTES: u64 = 4 * 1024 * 1024;
/// Resident bytes of an export tile that must still fit beside the previews
/// at the 384 MiB floor of the working-set limit: a 768 px core with a
/// typical halo. Larger limits give the tile the same share
/// ([`mobile_export_tile_bytes`]).
const MOBILE_EXPORT_HEADROOM_BYTES: u64 = 168 * 1024 * 1024;
/// Resident bytes of the small navigation graph.
const MOBILE_NAVIGATION_RESERVE_BYTES: u64 = 40 * 1024 * 1024;
/// Per-graph floors of the preview budget. They are what a fixed 384 MiB
/// device (3 GB of RAM or less) always had, so scaling the budget with RAM
/// never shrinks a preview below it.
const MOBILE_FITTED_FLOOR_BYTES: u64 = 150 * 1024 * 1024;
const MOBILE_DETAIL_FLOOR_BYTES: u64 = 120 * 1024 * 1024;
const MOBILE_FITTED_SHARE_PERCENT: u64 = 55;

/// Resident GPU bytes one preview graph of `role` may reserve when the
/// process may reserve `limit` in total: what is left after the export tile
/// and the navigation graph. The fitted graph takes a fixed share, so it can
/// always be rebuilt. The detail graph takes what the fitted graph that is
/// actually resident leaves (`fitted_resident`, zero if unknown), which is
/// more than a fixed share whenever the fitted graph is smaller than its own:
/// at the fitted view it only needs the screen's pixels.
fn mobile_preview_share_bytes(limit: u64, role: MobilePreviewRole, fitted_resident: u64) -> u64 {
    let pool = limit
        .saturating_sub(mobile_export_tile_bytes(limit))
        .saturating_sub(MOBILE_NAVIGATION_RESERVE_BYTES);
    let fitted_share = (pool * MOBILE_FITTED_SHARE_PERCENT / 100).max(MOBILE_FITTED_FLOOR_BYTES);
    match role {
        MobilePreviewRole::Fitted => fitted_share,
        MobilePreviewRole::Detail => {
            let fitted = if fitted_resident == 0 {
                fitted_share
            } else {
                fitted_resident.min(fitted_share)
            };
            pool.saturating_sub(fitted).max(MOBILE_DETAIL_FLOOR_BYTES)
        }
    }
}

/// Resident GPU bytes one export tile may reserve beside the previews when
/// the process may reserve `limit`: [`MOBILE_EXPORT_HEADROOM_BYTES`] at the
/// 384 MiB floor and the same share (7/16) of a larger limit, so the tile
/// grows with the limit while the previews keep the rest.
fn mobile_export_tile_bytes(limit: u64) -> u64 {
    scale_from_working_set_floor(MOBILE_EXPORT_HEADROOM_BYTES, limit)
}

/// `bytes`, sized for the 384 MiB floor, scaled with `limit`.
fn scale_from_working_set_floor(bytes: u64, limit: u64) -> u64 {
    let scaled =
        u128::from(bytes) * u128::from(limit) / u128::from(ANDROID_GPU_WORKING_SET_LIMIT_BYTES);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Whether this process has a GPU working-set limit: a phone or tablet, whose
/// GPU shares system memory.
pub(crate) fn gpu_working_set_is_limited() -> bool {
    gpu_working_set_limit_bytes() != u64::MAX
}

/// `bytes`, sized for an export at the 384 MiB floor of the GPU working-set
/// limit, scaled with this process's limit like the export tile's share. A
/// process without a limit (a computer) keeps `bytes`.
pub(crate) fn scale_with_mobile_export_tile(bytes: u64) -> u64 {
    match gpu_working_set_limit_bytes() {
        u64::MAX => bytes,
        limit => scale_from_working_set_floor(bytes, limit).max(bytes),
    }
}

/// What a full-quality export tile's graph holds besides its pixels, for
/// [`RawGpuPipeline::mobile_export_padded_edge`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExportTileShape {
    pub(crate) cfa_kind: CfaKind,
    pub(crate) layout: PixelLayout,
    /// Whether the RAW stage blends in AI-denoised sensor samples.
    pub(crate) ai_cfa: bool,
    /// The graph's mask layers.
    pub(crate) mask_layers: usize,
    /// The bytes of the camera profile's tables.
    pub(crate) profile_bytes: u64,
}

impl RawGpuPipeline {
    /// The longest padded export tile edge, up to `maximum`, whose
    /// full-quality graph fits the share of the GPU working-set limit that
    /// mobile previews leave for an export tile, with the same accounting
    /// that admits it. `mask_atlas_edge` gives the tile's mask atlas edge for
    /// a padded edge. `None` when the process has no working-set limit (a
    /// computer) or not even a 64 px tile fits.
    pub(crate) fn mobile_export_padded_edge(
        maximum: u32,
        shape: ExportTileShape,
        mask_atlas_edge: impl Fn(u32) -> u32,
    ) -> Option<u32> {
        let limit = gpu_working_set_limit_bytes();
        if limit == u64::MAX {
            return None;
        }
        let budget = mobile_export_tile_bytes(limit);
        let fits = |edge: u32| {
            build_gpu_resource_plan(GpuResourcePlanInput {
                width: edge,
                height: edge,
                quality: ProcessingQuality::High,
                tone_scale: tone_analysis_scale(),
                mask_atlas_edge: mask_atlas_edge(edge),
                mask_layers: shape.mask_layers.clamp(1, MAX_LOCAL_MASKS) as u32,
                profile_buffer_bytes: shape.profile_bytes,
                stage_uniform_buffer_bytes: GPU_STAGE_UNIFORM_ALLOCATION_BYTES,
                mask_data_buffer_bytes: MASK_DATA_SIZE_BYTES,
                layout: shape.layout,
                ai_cfa: shape.ai_cfa,
                cfa_kind: shape.cfa_kind,
            })
            .is_ok_and(|plan| plan.admitted_gpu_bytes <= budget)
        };
        let (mut low, mut high) = (64, maximum);
        if high < low || !fits(low) {
            return None;
        }
        while low < high {
            let edge = low + (high - low).div_ceil(2);
            if fits(edge) {
                low = edge;
            } else {
                high = edge - 1;
            }
        }
        Some(low)
    }

    /// Bound a mobile preview of `role` using the same resource accounting as
    /// allocation, so the graph that is built fits its share of the
    /// RAM-scaled budget. `shape` is what the graph reserves besides pixels.
    pub fn bounded_mobile_preview_edge(
        width: u32,
        height: u32,
        requested: u32,
        cfa_kind: CfaKind,
        role: MobilePreviewRole,
        shape: PreviewResourceShape,
    ) -> u32 {
        Self::bounded_preview_edge_within(
            width,
            height,
            requested,
            cfa_kind,
            shape,
            mobile_preview_share_bytes(
                mobile_preview_limit_bytes(),
                role,
                shape.fitted_resident_bytes,
            ),
        )
    }

    /// The GPU bytes a preview graph for a `width` by `height` source reserves,
    /// with `mask_layers` mask layers and the safety margin the admission check
    /// adds: what to compare with the memory a device has left before building
    /// one. `u64::MAX` when the size overflows the accounting.
    pub fn preview_admitted_bytes(
        width: u32,
        height: u32,
        cfa_kind: CfaKind,
        mask_layers: usize,
    ) -> u64 {
        build_gpu_resource_plan(preview_plan_input(
            width,
            height,
            cfa_kind,
            interactive_mask_atlas_edge(width, height),
            mask_layers,
        ))
        .map_or(u64::MAX, |plan| plan.admitted_gpu_bytes)
    }

    /// The longest edge up to `requested` whose graph reserves at most
    /// `resident_bytes`.
    fn bounded_preview_edge_within(
        width: u32,
        height: u32,
        requested: u32,
        cfa_kind: CfaKind,
        shape: PreviewResourceShape,
        resident_bytes: u64,
    ) -> u32 {
        let mut low = 1;
        let mut high = requested.min(width.max(height)).max(1);
        while low < high {
            let edge = low + (high - low).div_ceil(2);
            let fits = mobile_preview_resource_plan(width, height, edge, cfa_kind, shape)
                .is_ok_and(|plan| plan.persistent_gpu_bytes <= resident_bytes);
            if fits {
                low = edge;
            } else {
                high = edge - 1;
            }
        }
        low
    }
}

fn mobile_preview_resource_plan(
    width: u32,
    height: u32,
    edge: u32,
    cfa_kind: CfaKind,
    shape: PreviewResourceShape,
) -> Result<GpuResourcePlan> {
    let scale = (f64::from(edge) / f64::from(width.max(height).max(1))).min(1.0);
    let (width, height) = (
        (f64::from(width) * scale).ceil().max(1.0) as u32,
        (f64::from(height) * scale).ceil().max(1.0) as u32,
    );
    // The edge `compute_derived_geometry` picks on Android when the image
    // sizes the atlas.
    let mask_atlas_edge = shape
        .mask_atlas_edge
        .unwrap_or_else(|| crate::pipeline::masks::MASK_ATLAS_EDGE_ANDROID.min(width.max(height)))
        .max(64);
    build_gpu_resource_plan(preview_plan_input(
        width,
        height,
        cfa_kind,
        mask_atlas_edge,
        shape.mask_layers,
    ))
}

/// The resource plan of a preview graph, shared by the size bound and the
/// admission check so both account for the same buffers.
fn preview_plan_input(
    width: u32,
    height: u32,
    cfa_kind: CfaKind,
    mask_atlas_edge: u32,
    mask_layers: usize,
) -> GpuResourcePlanInput {
    GpuResourcePlanInput {
        width,
        height,
        quality: ProcessingQuality::Preview,
        tone_scale: tone_analysis_scale(),
        mask_atlas_edge,
        mask_layers: mask_layers.clamp(1, MAX_LOCAL_MASKS) as u32,
        profile_buffer_bytes: PREVIEW_PROFILE_BUFFER_BYTES,
        stage_uniform_buffer_bytes: GPU_STAGE_UNIFORM_ALLOCATION_BYTES,
        mask_data_buffer_bytes: MASK_DATA_SIZE_BYTES,
        // Sized for mosaics: the RGB source of a binned view or raster, without
        // AI denoise, needs less than the CFA textures it replaces.
        layout: PixelLayout::Mosaic,
        ai_cfa: false,
        cfa_kind,
    }
}
