use super::geometry::GeometryInverseMap;
use super::{
    build_region_proxy, export_mask_atlas_edge, extract_padded_tile, extract_padded_tile_into,
    mask_atlas_edge, mask_region_texture_extent, mask_source_region_uv, required_export_tile_halo,
    CfaKind, ExposureParams, GeometryTransform, GpuParams, GpuProgramPrewarm, LensGeometryMap,
    LoadedRaw, MaskStack, NativeRect, PipelineOptions, ProcessingQuality, ProcessingStage,
    ProxySpec, RawGpuPipeline, RawGpuProgramTemplate, RemoveEditState, RemoveSceneContext,
    SrgbOutputTransform, TilePlan, TileSpec, EXPORT_TILE_HALO, MAX_LOCAL_MASKS,
    MIN_EXPORT_TILE_HALO, TONE_GUIDE_CELL_SIZE,
};
use anyhow::{Context, Result};
use calibraw_core::file_ops::{replace_file, sync_parent_directory};
use rayon::prelude::*;
use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod crop;
mod destination;
mod encoders;
mod geometry_output;
mod resize;
mod tiff;
pub use crop::*;
use destination::*;
use encoders::*;
use geometry_output::*;
use resize::*;
use tiff::*;

mod rows;
mod settings;
mod streaming;
mod worker;
use rows::*;
pub use settings::*;
use streaming::*;
pub use worker::*;

const EXPORT_CPU_ROW_BATCH: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    #[default]
    Jpeg,
    Tiff,
    JpegXl,
}

/// `width` and `height` describe `pixels`; the pixels still cover the complete
/// native crop supplied to [`render_remove_scene_crop_resized`].
pub struct ResizedRemoveSceneCrop {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<f32>,
}

const STALE_EXPORT_PART_AGE: Duration = Duration::from_secs(24 * 60 * 60);
static NEXT_EXPORT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

pub struct TiledExportJob {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub raw: Arc<LoadedRaw>,
    pub geometry: GeometryTransform,
    pub exposure: ExposureParams,
    pub masks: MaskStack,
    pub remove: RemoveEditState,
    pub target: ExportTarget,
    pub tile_spec: TileSpec,
    pub settings: ExportSettings,
    pub metadata: ExportMetadata,
    pub cancellation: Arc<AtomicBool>,
    pub program_prewarm: Option<Arc<GpuProgramPrewarm>>,
}

fn tone_grid_aligned_crop_tile(crop: NativeRect, halo: u32) -> Result<crate::pipeline::ExportTile> {
    let alignment = i64::from(TONE_GUIDE_CELL_SIZE.max(1));
    let align_down = |value: i64| value.div_euclid(alignment) * alignment;
    let align_up = |value: i64| -(-value).div_euclid(alignment) * alignment;

    let core_x = i64::from(crop.x);
    let core_y = i64::from(crop.y);
    let core_right = core_x
        .checked_add(i64::from(crop.width))
        .context("crop right edge overflow")?;
    let core_bottom = core_y
        .checked_add(i64::from(crop.height))
        .context("crop bottom edge overflow")?;
    let halo = i64::from(halo);
    let origin_x = align_down(core_x - halo);
    let origin_y = align_down(core_y - halo);
    let padded_right = align_up(core_right + halo);
    let padded_bottom = align_up(core_bottom + halo);

    Ok(crate::pipeline::ExportTile {
        core_x: crop.x,
        core_y: crop.y,
        core_width: crop.width,
        core_height: crop.height,
        local_core_x: u32::try_from(core_x - origin_x).context("crop x offset overflow")?,
        local_core_y: u32::try_from(core_y - origin_y).context("crop y offset overflow")?,
        padded_width: u32::try_from(padded_right - origin_x)
            .context("aligned crop width overflow")?,
        padded_height: u32::try_from(padded_bottom - origin_y)
            .context("aligned crop height overflow")?,
        global_origin_x: i32::try_from(origin_x).context("aligned crop x origin overflow")?,
        global_origin_y: i32::try_from(origin_y).context("aligned crop y origin overflow")?,
    })
}

#[derive(Clone, Copy)]
struct ExportContext<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    events: &'a mpsc::Sender<ExportEvent>,
    cancellation: &'a AtomicBool,
    program_template: Option<&'a RawGpuProgramTemplate>,
}

#[derive(Clone, Copy)]
struct ExportRequest<'a> {
    raw: &'a LoadedRaw,
    exposure: &'a ExposureParams,
    masks: &'a MaskStack,
    remove: &'a RemoveEditState,
    output: ExportOutput<'a>,
    tile_spec: TileSpec,
    output_width: u32,
    output_height: u32,
    keep_metadata: bool,
    metadata: &'a ExportMetadata,
    geometry: GeometryTransform,
    bit_depth: ExportBitDepth,
    color: &'a ResolvedExportColor,
}

mod color;
pub(super) use color::linear_rec2020_icc;
use color::{built_in_srgb_icc, resolve_export_color, ResolvedExportColor};

mod metadata;
use metadata::{add_png_text_metadata, build_exif_payload};

#[cfg(test)]
mod tests;
