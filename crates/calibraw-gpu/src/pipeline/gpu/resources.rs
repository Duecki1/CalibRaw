use super::*;
use crate::pipeline::{LensShadingMap, TONE_GUIDE_CELL_SIZE};
use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use wgpu::util::DeviceExt;

pub(super) const SCENE_DEPTH_EDGE: u32 = 1024;
/// Stored scene depth, which Fog and Smoke read.
pub(super) const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// Relight's surface: stored depth, smooth depth and its two gradients, with
/// a mip chain (`scene_surface`). It replaces the plain depth once allocated.
pub(super) const RELIGHT_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Image lights (tone_analysis.wgsl): grid cells along the full image's
/// longer edge. Textures and the cell buffer are sized for a square grid.
pub(super) const IMAGE_LIGHT_GRID_LONG: u32 = 160;
/// One-stop brightness bands summed per image-light grid cell.
pub(super) const IMAGE_LIGHT_BANDS: u32 = 16;
/// Bytes of one grid cell: an RGBA f32 sum per band.
pub(super) const IMAGE_LIGHT_CELL_BYTES: u64 = IMAGE_LIGHT_BANDS as u64 * 16;
pub(super) const IMAGE_LIGHT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Dehaze (haze.wgsl): grid cells along the full image's longer edge. The
/// cell buffer and textures are sized for a square grid.
pub(super) const HAZE_GRID_LONG: u32 = 160;
/// Bytes of one haze grid cell (`HazeCell` in haze_analysis.wgsl): two RGBA
/// f32 vectors.
pub(super) const HAZE_CELL_BYTES: u64 = 2 * 16;
/// Bytes of the resolved airlight and added veil (`HazeAirlight` in
/// haze.wgsl): two RGBA f32 vectors.
pub(super) const HAZE_AIRLIGHT_BYTES: u64 = 2 * 16;
/// Per haze grid cell, the transmission and then the guided filter's
/// coefficients. They stay f32: a slope of several units times the guide
/// nearly cancels its offset.
pub(super) const HAZE_MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// Per scene-depth texel, the joint-upsampling guide colour and the stored
/// depth (scene_depth.wgsl).
pub(super) const SCENE_DEPTH_GUIDE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Relight shadows of up to four lights per scene-depth texel (relight.wgsl).
pub(super) const RELIGHT_SHADOW_MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Bytes of an image-light grid of `edge` × `edge` cells.
pub(super) const fn image_light_cells_bytes(edge: u32) -> u64 {
    edge as u64 * edge as u64 * IMAGE_LIGHT_CELL_BYTES
}

/// Bytes of a haze grid of `edge` × `edge` cells.
pub(super) const fn haze_cells_bytes(edge: u32) -> u64 {
    edge as u64 * edge as u64 * HAZE_CELL_BYTES
}

const MAX_UPLOAD_SCRATCH_BYTES: usize = 8 * 1024 * 1024;

std::thread_local! {
    static BLACK_UPLOAD_SCRATCH: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
    static COLOR_UPLOAD_SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

pub(super) const fn tone_analysis_scale() -> u32 {
    TONE_GUIDE_CELL_SIZE
}

pub(super) fn tone_guide_format() -> wgpu::TextureFormat {
    wgpu::TextureFormat::R32Float
}

pub(super) fn interactive_mask_atlas_edge(width: u32, height: u32) -> u32 {
    mask_atlas_edge()
        .min(width.max(height))
        .clamp(64, export_mask_atlas_edge_limit())
}

const GPU_ALLOCATION_ALIGNMENT_BYTES: u64 = 256;
const GPU_SAFETY_MARGIN_NUMERATOR: u64 = 1;
const GPU_SAFETY_MARGIN_DENOMINATOR: u64 = 5;
static RESERVED_GPU_BYTES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GpuResourceResidency {
    Persistent,
    /// Reserved with the pipeline like `Persistent`, but allocated only when
    /// an effect first reads it (`effect_inputs`).
    OnDemand,
    Transient,
    HostPeak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GpuResourceAccountingEntry {
    pub name: &'static str,
    pub residency: GpuResourceResidency,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GpuResourcePlan {
    pub entries: Vec<GpuResourceAccountingEntry>,
    pub persistent_gpu_bytes: u64,
    pub transient_gpu_peak_bytes: u64,
    pub host_peak_bytes: u64,
    pub safety_margin_bytes: u64,
    pub admitted_gpu_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GpuResourcePlanInput {
    pub width: u32,
    pub height: u32,
    pub quality: ProcessingQuality,
    pub tone_scale: u32,
    pub mask_atlas_edge: u32,
    pub mask_layers: u32,
    pub profile_buffer_bytes: u64,
    pub stage_uniform_buffer_bytes: u64,
    pub mask_data_buffer_bytes: u64,
    /// Whether the raw texture also carries the AI-denoised Bayer mosaic.
    pub ai_cfa: bool,
    /// Whether the source is a pre-demosaiced raster, held in an RGB source
    /// texture while the CFA textures shrink to placeholders.
    pub raster_source: bool,
    /// X-Trans pipelines also hold Markesteijn's homogeneity map.
    pub cfa_kind: CfaKind,
}

#[derive(Debug)]
pub(super) struct GpuBudgetReservation {
    bytes: u64,
}

impl GpuBudgetReservation {
    pub(super) fn acquire(plan: &GpuResourcePlan, limit: u64) -> Result<Self> {
        validate_gpu_resource_plan(plan, limit)?;
        let bytes = plan.persistent_gpu_bytes;
        reserve_gpu_bytes(&RESERVED_GPU_BYTES, limit, bytes).map_err(|used| {
            anyhow!(
                "GPU pipelines already reserve {:.1} MiB of resident resources; this pipeline needs another {:.1} MiB, exceeding the {:.1} MiB process budget; close optional detail/navigation previews, reduce proxy size or mask capacity, or use tiled export",
                used as f64 / (1024.0 * 1024.0),
                bytes as f64 / (1024.0 * 1024.0),
                limit as f64 / (1024.0 * 1024.0),
            )
        })?;
        Ok(Self { bytes })
    }
}

impl GpuBudgetReservation {
    /// Reserves `bytes` for `resource`, which is allocated after its pipeline
    /// was built and is not part of the pipeline's plan.
    pub(super) fn acquire_bytes(resource: &str, bytes: u64, limit: u64) -> Result<Self> {
        reserve_gpu_bytes(&RESERVED_GPU_BYTES, limit, bytes).map_err(|used| {
            anyhow!(
                "GPU pipelines already reserve {:.1} MiB of resident resources; the {resource} needs another {:.1} MiB, exceeding the {:.1} MiB process budget",
                used as f64 / (1024.0 * 1024.0),
                bytes as f64 / (1024.0 * 1024.0),
                limit as f64 / (1024.0 * 1024.0),
            )
        })?;
        Ok(Self { bytes })
    }

    pub(super) fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for GpuBudgetReservation {
    fn drop(&mut self) {
        RESERVED_GPU_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn reserve_gpu_bytes(used: &AtomicU64, limit: u64, bytes: u64) -> std::result::Result<(), u64> {
    let mut current = used.load(Ordering::Acquire);
    loop {
        let Some(next) = current.checked_add(bytes) else {
            return Err(current);
        };
        if next > limit {
            return Err(current);
        }
        match used.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn checked_align_up(value: u64, alignment: u64, context: &'static str) -> Result<u64> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(anyhow!("invalid {context} alignment {alignment}"));
    }
    let mask = alignment - 1;
    value
        .checked_add(mask)
        .map(|expanded| expanded & !mask)
        .ok_or_else(|| anyhow!("{context} alignment overflows"))
}

fn texture_format_bytes_per_texel(format: wgpu::TextureFormat) -> Result<u64> {
    match format {
        wgpu::TextureFormat::R8Uint => Ok(1),
        wgpu::TextureFormat::R16Uint | wgpu::TextureFormat::R16Float => Ok(2),
        wgpu::TextureFormat::R32Float
        | wgpu::TextureFormat::R32Uint
        | wgpu::TextureFormat::Rg16Uint
        | wgpu::TextureFormat::Rgba8Unorm => Ok(4),
        wgpu::TextureFormat::Rgba16Float => Ok(8),
        wgpu::TextureFormat::Rgba32Float => Ok(16),
        _ => Err(anyhow!("unaccounted GPU texture format {format:?}")),
    }
}

/// The texel format of the look LUT (look_lut.wgsl): f32, so the lattice
/// keeps the precision of the `.cube` file.
pub(super) const LOOK_LUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// Bytes of a look LUT texture with `size`³ texels.
pub(super) fn look_lut_texture_bytes(size: u32) -> Result<u64> {
    texture_allocation_bytes(size, size, size, 1, LOOK_LUT_FORMAT)
}

fn texture_allocation_bytes(
    width: u32,
    height: u32,
    depth_or_layers: u32,
    mip_levels: u32,
    format: wgpu::TextureFormat,
) -> Result<u64> {
    if width == 0 || height == 0 || depth_or_layers == 0 || mip_levels == 0 {
        return Err(anyhow!(
            "GPU texture dimensions, layers, and mip levels must be non-zero"
        ));
    }
    let bytes_per_texel = texture_format_bytes_per_texel(format)?;
    let mut total = 0u64;
    let mut mip_width = width;
    let mut mip_height = height;
    for _ in 0..mip_levels {
        let mip_bytes = u64::from(mip_width)
            .checked_mul(u64::from(mip_height))
            .and_then(|value| value.checked_mul(u64::from(depth_or_layers)))
            .and_then(|value| value.checked_mul(bytes_per_texel))
            .ok_or_else(|| anyhow!("GPU texture byte calculation overflows"))?;
        total = total
            .checked_add(checked_align_up(
                mip_bytes,
                GPU_ALLOCATION_ALIGNMENT_BYTES,
                "GPU texture",
            )?)
            .ok_or_else(|| anyhow!("GPU texture mip total overflows"))?;
        mip_width = (mip_width / 2).max(1);
        mip_height = (mip_height / 2).max(1);
    }
    Ok(total)
}

fn aligned_buffer_bytes(bytes: u64) -> Result<u64> {
    checked_align_up(bytes.max(1), GPU_ALLOCATION_ALIGNMENT_BYTES, "GPU buffer")
}

fn aligned_copy_buffer_bytes(width: u32, height: u32, bytes_per_texel: u64) -> Result<u64> {
    let row_bytes = u64::from(width)
        .checked_mul(bytes_per_texel)
        .ok_or_else(|| anyhow!("GPU copy row byte calculation overflows"))?;
    let padded_row = checked_align_up(row_bytes, 256, "GPU copy row")?;
    padded_row
        .checked_mul(u64::from(height))
        .ok_or_else(|| anyhow!("GPU copy buffer byte calculation overflows"))
}

fn push_entry(
    entries: &mut Vec<GpuResourceAccountingEntry>,
    name: &'static str,
    residency: GpuResourceResidency,
    bytes: u64,
) {
    entries.push(GpuResourceAccountingEntry {
        name,
        residency,
        bytes,
    });
}

pub(super) fn build_gpu_resource_plan(input: GpuResourcePlanInput) -> Result<GpuResourcePlan> {
    if input.tone_scale == 0 {
        return Err(anyhow!("GPU tone-analysis scale must be non-zero"));
    }
    let mut entries = Vec::new();
    let work_format = processing_work_format(input.quality);
    let full = |format| texture_allocation_bytes(input.width, input.height, 1, 1, format);

    if input.raster_source {
        push_entry(
            &mut entries,
            "raster source texture",
            GpuResourceResidency::Persistent,
            full(work_format)?,
        );
    } else {
        push_entry(
            &mut entries,
            "raw CFA texture",
            GpuResourceResidency::Persistent,
            full(raw_texture_format(input.ai_cfa))?,
        );
        push_entry(
            &mut entries,
            "CFA color-index texture",
            GpuResourceResidency::Persistent,
            full(wgpu::TextureFormat::R8Uint)?,
        );
        push_entry(
            &mut entries,
            "black-level texture",
            GpuResourceResidency::Persistent,
            full(wgpu::TextureFormat::R32Float)?,
        );
    }
    push_entry(
        &mut entries,
        "reconstructed raw texture",
        GpuResourceResidency::Persistent,
        full(wgpu::TextureFormat::R32Float)?,
    );
    for name in [
        "highlight work A",
        "highlight work B",
        "scene texture",
        "display-linear texture",
        "demosaic scratch 1",
        "demosaic scratch 2",
    ] {
        push_entry(
            &mut entries,
            name,
            GpuResourceResidency::Persistent,
            full(work_format)?,
        );
    }
    push_entry(
        &mut entries,
        "encoded output texture",
        GpuResourceResidency::Persistent,
        full(wgpu::TextureFormat::Rgba8Unorm)?,
    );
    if input.cfa_kind == CfaKind::XTrans {
        push_entry(
            &mut entries,
            "X-Trans homogeneity map",
            GpuResourceResidency::Persistent,
            full(XTRANS_HOMOGENEITY_FORMAT)?,
        );
    }

    let tone_width = input.width.div_ceil(input.tone_scale);
    let tone_height = input.height.div_ceil(input.tone_scale);
    let tone_bytes = texture_allocation_bytes(tone_width, tone_height, 1, 1, tone_guide_format())?;
    push_entry(
        &mut entries,
        "tone guide A",
        GpuResourceResidency::Persistent,
        tone_bytes,
    );
    push_entry(
        &mut entries,
        "tone guide B",
        GpuResourceResidency::Persistent,
        tone_bytes,
    );

    push_entry(
        &mut entries,
        "scene depth texture",
        GpuResourceResidency::Persistent,
        texture_allocation_bytes(SCENE_DEPTH_EDGE, SCENE_DEPTH_EDGE, 1, 1, SCENE_DEPTH_FORMAT)?,
    );
    push_entry(
        &mut entries,
        "Relight scene-depth surface",
        GpuResourceResidency::OnDemand,
        texture_allocation_bytes(
            SCENE_DEPTH_EDGE,
            SCENE_DEPTH_EDGE,
            1,
            SCENE_DEPTH_MIP_LEVELS,
            RELIGHT_SURFACE_FORMAT,
        )?,
    );
    let mask_bytes = texture_allocation_bytes(
        input.mask_atlas_edge,
        input.mask_atlas_edge,
        input.mask_layers,
        1,
        wgpu::TextureFormat::R16Float,
    )?;
    push_entry(
        &mut entries,
        "local-mask atlas",
        GpuResourceResidency::Persistent,
        mask_bytes,
    );
    push_entry(
        &mut entries,
        "Light Rays emission atlas",
        GpuResourceResidency::Persistent,
        texture_allocation_bytes(
            LIGHT_RAYS_MASK_ATLAS_EDGE,
            LIGHT_RAYS_MASK_ATLAS_EDGE,
            input.mask_layers,
            1,
            wgpu::TextureFormat::R16Float,
        )?,
    );
    push_entry(
        &mut entries,
        "camera/output profile buffer",
        GpuResourceResidency::Persistent,
        aligned_buffer_bytes(input.profile_buffer_bytes)?,
    );
    push_entry(
        &mut entries,
        "stage uniform buffers",
        GpuResourceResidency::Persistent,
        aligned_buffer_bytes(input.stage_uniform_buffer_bytes)?,
    );
    push_entry(
        &mut entries,
        "local-mask data buffer",
        GpuResourceResidency::Persistent,
        aligned_buffer_bytes(input.mask_data_buffer_bytes)?,
    );
    let histogram_bytes = u64::try_from(std::mem::size_of::<u32>())
        .ok()
        .and_then(|word_bytes| word_bytes.checked_mul(256))
        .ok_or_else(|| anyhow!("tone histogram buffer byte calculation overflows"))?;
    push_entry(
        &mut entries,
        "tone histogram buffer",
        GpuResourceResidency::Persistent,
        aligned_buffer_bytes(histogram_bytes)?,
    );
    push_entry(
        &mut entries,
        "tone statistics buffer",
        GpuResourceResidency::Persistent,
        aligned_buffer_bytes(TONE_STATS_SIZE_BYTES)?,
    );
    push_entry(
        &mut entries,
        "scene-depth guide",
        GpuResourceResidency::OnDemand,
        texture_allocation_bytes(
            SCENE_DEPTH_EDGE,
            SCENE_DEPTH_EDGE,
            1,
            1,
            SCENE_DEPTH_GUIDE_FORMAT,
        )?,
    );
    push_entry(
        &mut entries,
        "Relight shadow map",
        GpuResourceResidency::OnDemand,
        texture_allocation_bytes(
            SCENE_DEPTH_EDGE,
            SCENE_DEPTH_EDGE,
            1,
            1,
            RELIGHT_SHADOW_MAP_FORMAT,
        )?,
    );
    push_entry(
        &mut entries,
        "image-light grid buffer",
        GpuResourceResidency::OnDemand,
        aligned_buffer_bytes(image_light_cells_bytes(IMAGE_LIGHT_GRID_LONG))?,
    );
    push_entry(
        &mut entries,
        "image-light textures",
        GpuResourceResidency::OnDemand,
        3 * texture_allocation_bytes(
            IMAGE_LIGHT_GRID_LONG,
            IMAGE_LIGHT_GRID_LONG,
            1,
            1,
            IMAGE_LIGHT_FORMAT,
        )?,
    );
    push_entry(
        &mut entries,
        "haze grid and airlight buffers",
        GpuResourceResidency::OnDemand,
        aligned_buffer_bytes(haze_cells_bytes(HAZE_GRID_LONG))?
            .checked_add(aligned_buffer_bytes(HAZE_AIRLIGHT_BYTES)?)
            .ok_or_else(|| anyhow!("haze buffer size overflows"))?,
    );
    push_entry(
        &mut entries,
        "haze textures",
        GpuResourceResidency::OnDemand,
        2 * texture_allocation_bytes(HAZE_GRID_LONG, HAZE_GRID_LONG, 1, 1, HAZE_MAP_FORMAT)?,
    );

    let rgba8_readback = aligned_copy_buffer_bytes(input.width, input.height, 4)?;
    // Every RGBA32F readback (display-linear regions, the scene texture, the
    // camera scene) is refused unless the graph is `ProcessingQuality::High`,
    // so a preview graph only ever reads back its RGBA8 output.
    let readback_peak = match input.quality {
        ProcessingQuality::Preview => rgba8_readback,
        ProcessingQuality::High => aligned_copy_buffer_bytes(input.width, input.height, 16)?
            .min(MAX_RGBA32_READBACK_CHUNK_BYTES)
            .max(rgba8_readback),
    };
    push_entry(
        &mut entries,
        "readback buffer peak",
        GpuResourceResidency::Transient,
        readback_peak,
    );

    let upload_scratch_bytes = u64::try_from(MAX_UPLOAD_SCRATCH_BYTES)
        .map_err(|_| anyhow!("upload scratch size does not fit in u64"))?;
    push_entry(
        &mut entries,
        "black upload scratch",
        GpuResourceResidency::HostPeak,
        upload_scratch_bytes,
    );
    push_entry(
        &mut entries,
        "color upload scratch",
        GpuResourceResidency::HostPeak,
        upload_scratch_bytes,
    );

    let sum = |residency| -> Result<u64> {
        entries
            .iter()
            .filter(|entry| entry.residency == residency)
            .try_fold(0u64, |total, entry| {
                total
                    .checked_add(entry.bytes)
                    .ok_or_else(|| anyhow!("GPU resource-plan total overflows"))
            })
    };
    // On-demand inputs are reserved up front, so allocating them later never
    // exceeds the budget.
    let persistent_gpu_bytes = sum(GpuResourceResidency::Persistent)?
        .checked_add(sum(GpuResourceResidency::OnDemand)?)
        .ok_or_else(|| anyhow!("GPU resource-plan total overflows"))?;
    let transient_gpu_peak_bytes = sum(GpuResourceResidency::Transient)?;
    let host_peak_bytes = sum(GpuResourceResidency::HostPeak)?;
    let gpu_before_margin = persistent_gpu_bytes
        .checked_add(transient_gpu_peak_bytes)
        .ok_or_else(|| anyhow!("GPU resource-plan peak overflows"))?;
    let safety_margin_bytes = gpu_before_margin
        .checked_mul(GPU_SAFETY_MARGIN_NUMERATOR)
        .and_then(|value| value.checked_div(GPU_SAFETY_MARGIN_DENOMINATOR))
        .ok_or_else(|| anyhow!("GPU safety-margin calculation overflows"))?;
    let admitted_gpu_bytes = gpu_before_margin
        .checked_add(safety_margin_bytes)
        .ok_or_else(|| anyhow!("GPU admitted working set overflows"))?;

    Ok(GpuResourcePlan {
        entries,
        persistent_gpu_bytes,
        transient_gpu_peak_bytes,
        host_peak_bytes,
        safety_margin_bytes,
        admitted_gpu_bytes,
    })
}

pub(super) fn validate_gpu_resource_plan(plan: &GpuResourcePlan, limit: u64) -> Result<()> {
    if plan.admitted_gpu_bytes > limit {
        let mask = plan
            .entries
            .iter()
            .find(|entry| entry.name == "local-mask atlas")
            .map_or(0, |entry| entry.bytes);
        return Err(anyhow!(
            "GPU resource plan requires {:.1} MiB including a {:.1} MiB safety margin (mask atlas {:.1} MiB), above the {:.1} MiB budget; reduce proxy size or mask-atlas capacity, or use tiled export",
            plan.admitted_gpu_bytes as f64 / (1024.0 * 1024.0),
            plan.safety_margin_bytes as f64 / (1024.0 * 1024.0),
            mask as f64 / (1024.0 * 1024.0),
            limit as f64 / (1024.0 * 1024.0),
        ));
    }
    Ok(())
}

#[cfg(target_os = "android")]
pub(super) fn gpu_working_set_limit_bytes() -> u64 {
    // Previews are sized against the fixed 384 MiB budget and together reserve
    // up to about 262 MiB; a full-quality export tile needs about 168 MiB more.
    // Scaling with device RAM lets the export coexist with open previews.
    static LIMIT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        let total_ram = calibraw_core::system_memory::total_memory_bytes();
        let limit = android_working_set_limit_bytes(total_ram);
        log::info!(
            "GPU working-set budget {:.1} MiB (device RAM {})",
            limit as f64 / (1024.0 * 1024.0),
            total_ram.map_or_else(
                || "unknown".to_owned(),
                |bytes| format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
            ),
        );
        limit
    })
}

#[cfg(not(target_os = "android"))]
pub(super) fn gpu_working_set_limit_bytes() -> u64 {
    // Desktop VRAM varies widely. The fixed estimate is useful in tests, but
    // must not reject a valid allocation before the driver has a chance to
    // allocate it. Real OOM is reported by the GPU error scopes.
    u64::MAX
}

/// The process reservation limit mobile previews are sized against. An
/// unlimited admission budget keeps the mobile floor for preview sizing;
/// Android's finite, RAM-scaled limit is shared with allocation admission.
pub(super) fn mobile_preview_limit_bytes() -> u64 {
    match gpu_working_set_limit_bytes() {
        u64::MAX => ANDROID_GPU_WORKING_SET_LIMIT_BYTES,
        limit => limit,
    }
}

/// The Android admission cap for `total_ram_bytes` of device RAM: a fixed
/// share, never below the preview budget or above the ceiling. Unknown RAM
/// keeps the preview budget.
#[cfg(target_os = "android")]
pub(super) fn android_working_set_limit_bytes(total_ram_bytes: Option<u64>) -> u64 {
    total_ram_bytes.map_or(ANDROID_GPU_WORKING_SET_LIMIT_BYTES, |ram| {
        (ram / ANDROID_GPU_WORKING_SET_RAM_DIVISOR).clamp(
            ANDROID_GPU_WORKING_SET_LIMIT_BYTES,
            ANDROID_GPU_WORKING_SET_CEILING_BYTES,
        )
    })
}

pub(super) fn processing_work_format(quality: ProcessingQuality) -> wgpu::TextureFormat {
    match quality {
        ProcessingQuality::Preview => wgpu::TextureFormat::Rgba16Float,
        ProcessingQuality::High => wgpu::TextureFormat::Rgba32Float,
    }
}

pub(super) fn work_shader_source(
    source: &str,
    format: wgpu::TextureFormat,
) -> Result<Cow<'_, str>> {
    let marker_count = source.matches(WORK_FORMAT_MARKER).count();
    if marker_count == 0 {
        return Err(anyhow!(
            "format-specialized shader is missing the CalibRaw work-format marker"
        ));
    }
    match format {
        wgpu::TextureFormat::Rgba16Float => Ok(Cow::Borrowed(source)),
        wgpu::TextureFormat::Rgba32Float => Ok(Cow::Owned(
            source.replace(WORK_FORMAT_MARKER, "rgba32float"),
        )),
        _ => Err(anyhow!(
            "unsupported CalibRaw work texture format: {format:?}"
        )),
    }
}

pub(super) fn create_demosaic_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::Texture {
    create_processing_texture(
        device,
        size,
        format,
        wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        label,
    )
}

/// The source of a pre-demosaiced raster, which the raster finish pass turns
/// into the scene each RAW stage. It stays untouched so denoise and Remove
/// re-run from the original pixels. Sensor sources get a 1x1 placeholder, as
/// rasters do for the CFA textures.
pub(super) fn create_raster_source_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
    format: wgpu::TextureFormat,
) -> Result<wgpu::Texture> {
    let raster = raw.is_pre_demosaiced_raster();
    let size = if raster {
        texture_size(raw.width, raw.height)
    } else {
        texture_size(1, 1)
    };
    let texture = create_processing_texture(
        device,
        size,
        format,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "calibraw raster source",
    );
    if raster {
        upload_raster_source_texture(queue, &texture, format, raw)?;
    }
    Ok(texture)
}

/// Uploads a raster's scene-linear RGB pixels into its source texture.
pub(super) fn upload_raster_source_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    raw: &LoadedRaw,
) -> Result<()> {
    let rgb = raw
        .scene_linear_raster()
        .context("raster source is missing scene-linear RGB pixels")?;
    let expected = raw.width as usize * raw.height as usize * 3;
    anyhow::ensure!(
        rgb.len() == expected,
        "scene-linear raster has {} values, expected {expected}",
        rgb.len()
    );
    upload_rgba_texture_chunks(
        queue,
        texture,
        format,
        raw,
        |index| {
            [
                half::f16::from_f32(rgb[index * 3]).to_bits(),
                half::f16::from_f32(rgb[index * 3 + 1]).to_bits(),
                half::f16::from_f32(rgb[index * 3 + 2]).to_bits(),
            ]
        },
        |index| [rgb[index * 3], rgb[index * 3 + 1], rgb[index * 3 + 2]],
    )
}

fn upload_rgba_texture_chunks(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    raw: &LoadedRaw,
    pixel_f16: impl Fn(usize) -> [u16; 3],
    pixel_f32: impl Fn(usize) -> [f32; 3],
) -> Result<()> {
    match format {
        wgpu::TextureFormat::Rgba16Float => write_packed_rows(queue, texture, raw, 8, |source| {
            pack_rgba16_pixel(pixel_f16(source))
        }),
        wgpu::TextureFormat::Rgba32Float => write_packed_rows(queue, texture, raw, 16, |source| {
            pack_rgba32_pixel(pixel_f32(source))
        }),
        _ => Err(anyhow!("unsupported scene texture format {format:?}")),
    }
}

/// Uploads all rows of `raw` in chunks of at most `MAX_UPLOAD_SCRATCH_BYTES`,
/// packing each texel with `pixel(source)`, where `source` is the row-major
/// pixel index in `raw`.
fn write_packed_rows<T: bytemuck::Pod + Default, const CHANNELS: usize>(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    raw: &LoadedRaw,
    bytes_per_texel: u32,
    pixel: impl Fn(usize) -> [T; CHANNELS],
) -> Result<()> {
    let bytes_per_row = raw
        .width
        .checked_mul(bytes_per_texel)
        .ok_or_else(|| anyhow!("scene texture upload row byte count overflows"))?;
    let rows_per_chunk = (MAX_UPLOAD_SCRATCH_BYTES / bytes_per_row as usize).max(1) as u32;
    for first_row in (0..raw.height).step_by(rows_per_chunk as usize) {
        let row_count = rows_per_chunk.min(raw.height - first_row);
        let pixels = row_count as usize * raw.width as usize;
        let mut rgba = vec![T::default(); pixels * CHANNELS];
        for pixel_index in 0..pixels {
            let source = first_row as usize * raw.width as usize + pixel_index;
            let destination = pixel_index * CHANNELS;
            rgba[destination..destination + CHANNELS].copy_from_slice(&pixel(source));
        }
        write_texture_chunk(
            queue,
            texture,
            first_row,
            row_count,
            raw.width,
            bytes_per_row,
            &rgba,
        );
    }
    Ok(())
}

/// Decodes the `index`th RGB triplet of packed IEEE half-float bits to `f32`.
pub(super) fn decode_f16_rgb(rgb16f: &[u16], index: usize) -> [f32; 3] {
    [
        half::f16::from_bits(rgb16f[index * 3]).to_f32(),
        half::f16::from_bits(rgb16f[index * 3 + 1]).to_f32(),
        half::f16::from_bits(rgb16f[index * 3 + 2]).to_f32(),
    ]
}

fn pack_rgba16_pixel([red, green, blue]: [u16; 3]) -> [u16; 4] {
    [red, green, blue, half::f16::ONE.to_bits()]
}

fn pack_rgba32_pixel([red, green, blue]: [f32; 3]) -> [f32; 4] {
    [red, green, blue, 1.0]
}

fn write_texture_chunk<T: bytemuck::Pod>(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    first_row: u32,
    row_count: u32,
    width: u32,
    bytes_per_row: u32,
    data: &[T],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: first_row,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(row_count),
        },
        wgpu::Extent3d {
            width,
            height: row_count,
            depth_or_array_layers: 1,
        },
    );
}

/// Uploads `raw`'s demosaiced AI-denoise result into the scene texture, if it
/// has one, and returns whether it did.
pub(super) fn upload_ai_scene_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    raw: &LoadedRaw,
) -> Result<bool> {
    let Some(image) = raw.ai_denoised_image() else {
        return Ok(false);
    };
    let Some(rgb16f) = image.camera_rgb16f() else {
        return Ok(false);
    };
    anyhow::ensure!(
        image.is_valid_for(raw.width, raw.height),
        "AI-denoise texture dimensions do not match the RAW"
    );
    upload_rgba_texture_chunks(
        queue,
        texture,
        format,
        raw,
        |index| {
            [
                rgb16f[index * 3],
                rgb16f[index * 3 + 1],
                rgb16f[index * 3 + 2],
            ]
        },
        |index| decode_f16_rgb(rgb16f, index),
    )?;
    Ok(true)
}

pub(super) fn create_tone_guide_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::Texture {
    create_processing_texture(
        device,
        size,
        format,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        label,
    )
}

pub(super) fn create_float_work_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::Texture {
    create_processing_texture(
        device,
        size,
        format,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        label,
    )
}

pub(super) fn create_processing_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[format],
    })
}

pub(super) fn create_processing_texture_array(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    layers: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &str,
) -> wgpu::Texture {
    create_processing_texture(
        device,
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layers,
        },
        format,
        usage,
        label,
    )
}

pub(super) fn default_texture_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(super) fn create_initialized_buffer(
    device: &wgpu::Device,
    label: &str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage,
    })
}

pub(super) fn create_gpu_buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

pub(super) fn create_bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    entries: &[wgpu::BindGroupEntry<'_>],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries,
    })
}

pub(super) fn create_bind_group_layout(
    device: &wgpu::Device,
    label: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    })
}

pub(super) fn create_compute_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    entry_point: &str,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        module,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache,
    })
}

pub(super) fn dispatch_compute(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::ComputePipeline,
    bind_groups: &[&wgpu::BindGroup],
    workgroups: [u32; 3],
) {
    let timestamps = crate::stage_timing::next_pass(label);
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: timestamps
            .as_ref()
            .map(crate::stage_timing::PassTimestamps::writes),
    });
    pass.set_pipeline(pipeline);
    for (index, bind_group) in bind_groups.iter().enumerate() {
        pass.set_bind_group(index as u32, *bind_group, &[]);
    }
    pass.dispatch_workgroups(workgroups[0], workgroups[1], workgroups[2]);
}

pub(super) fn buffer_binding(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

pub(super) fn texture_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

pub(super) fn sampler_binding(binding: u32, sampler: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Sampler(sampler),
    }
}

pub(super) fn buffer_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(super) fn storage_buffer_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(super) fn storage_texture_entry(
    binding: u32,
    format: wgpu::TextureFormat,
    access: wgpu::StorageTextureAccess,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

pub(super) fn validate_raw(raw: &LoadedRaw) -> Result<()> {
    let pixels = crate::pipeline::raw_loader::validate_raw_dimensions(raw.width, raw.height)?;
    if raw.is_pre_demosaiced_raster() {
        let expected = pixels
            .checked_mul(3)
            .ok_or_else(|| anyhow!("raster RGB element count overflows"))?;
        let rgb = raw
            .scene_linear_raster()
            .ok_or_else(|| anyhow!("raster source is missing scene-linear RGB pixels"))?;
        if rgb.len() != expected || rgb.iter().any(|value| !value.is_finite()) {
            return Err(anyhow!("scene-linear raster payload is invalid"));
        }
    } else if raw.raw_pixels.len() != pixels {
        return Err(anyhow!(
            "raw pixel count mismatch: got {}, expected {}",
            raw.raw_pixels.len(),
            pixels
        ));
    }
    if raw.color_indices.len() != pixels {
        return Err(anyhow!(
            "CFA index count mismatch: got {}, expected {}",
            raw.color_indices.len(),
            pixels
        ));
    }
    if raw.black_levels_per_pixel.len() != pixels {
        return Err(anyhow!(
            "black-level map count mismatch: got {}, expected {}",
            raw.black_levels_per_pixel.len(),
            pixels
        ));
    }
    if raw
        .color_indices
        .storage_slice()
        .iter()
        .any(|channel| *channel > 3)
    {
        return Err(anyhow!("CFA index map contains a channel above 3"));
    }
    if raw
        .wb_coeffs
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(anyhow!(
            "white-balance coefficients must be finite and positive"
        ));
    }
    if raw
        .profile_color
        .cam_to_srgb
        .iter()
        .flatten()
        .any(|value| !value.is_finite())
    {
        return Err(anyhow!(
            "camera-to-working matrix contains a non-finite value"
        ));
    }
    if raw
        .profile_color
        .cam_to_srgb
        .iter()
        .flatten()
        .all(|value| value.abs() <= 1e-12)
    {
        return Err(anyhow!("camera-to-working matrix is empty"));
    }
    if raw.black_levels.iter().any(|value| !value.is_finite())
        || raw.white_levels.iter().any(|value| !value.is_finite())
    {
        return Err(anyhow!(
            "black/white calibration contains a non-finite value"
        ));
    }

    let period_width = joint_period(
        raw.color_indices.storage_width(),
        raw.black_levels_per_pixel.storage_width(),
        raw.width,
    );
    let period_height = joint_period(
        raw.color_indices.storage_height(),
        raw.black_levels_per_pixel.storage_height(),
        raw.height,
    );
    for y in 0..period_height {
        for x in 0..period_width {
            let index = (y * raw.width + x) as usize;
            let black = raw.black_levels_per_pixel[index];
            let channel = raw.color_indices[index];
            let white = raw.white_levels[channel as usize];
            if !black.is_finite() || white <= black {
                return Err(anyhow!(
                    "invalid black/white range at pixel {index}: black={black}, white={white}"
                ));
            }
        }
    }
    Ok(())
}

fn joint_period(left: u32, right: u32, logical: u32) -> u32 {
    fn gcd(mut a: u64, mut b: u64) -> u64 {
        while b != 0 {
            let remainder = a % b;
            a = b;
            b = remainder;
        }
        a.max(1)
    }

    let left = u64::from(left.max(1));
    let right = u64::from(right.max(1));
    let lcm = left
        .checked_div(gcd(left, right))
        .and_then(|value| value.checked_mul(right))
        .unwrap_or(u64::from(logical));
    lcm.min(u64::from(logical)).max(1) as u32
}

pub(super) fn texture_array_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub(super) fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// An unfiltered 3D float texture, read with `textureLoad`.
pub(super) fn texture_3d_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D3,
            multisampled: false,
        },
        count: None,
    }
}

/// Markesteijn's homogeneity counts, 4 bits for each of 8 candidates
/// (xtrans_demosaic.wgsl).
pub(super) const XTRANS_HOMOGENEITY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;

/// The sensor mosaic format: `R16Uint` with the original mosaic, or
/// `Rg16Uint` with the original in R and the AI-denoised mosaic in G, which
/// `raw_sampling.wgsl` blends by `CameraUniforms::ai_cfa_weight`.
pub(super) const fn raw_texture_format(has_ai_cfa: bool) -> wgpu::TextureFormat {
    if has_ai_cfa {
        wgpu::TextureFormat::Rg16Uint
    } else {
        wgpu::TextureFormat::R16Uint
    }
}

pub(super) fn create_raw_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
    ai_cfa: Option<&[u16]>,
) -> Result<wgpu::Texture> {
    let sensor_size = if raw.is_pre_demosaiced_raster() {
        texture_size(1, 1)
    } else {
        texture_size(raw.width, raw.height)
    };
    let format = raw_texture_format(ai_cfa.is_some());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("calibraw raw mosaic"),
        size: sensor_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[format],
    });
    upload_raw_texture(queue, &texture, raw, ai_cfa)?;
    Ok(texture)
}

/// Uploads `raw`'s mosaic into a texture made by [`create_raw_texture`] with
/// the same `ai_cfa` presence.
pub(super) fn upload_raw_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    raw: &LoadedRaw,
    ai_cfa: Option<&[u16]>,
) -> Result<()> {
    if raw.raw_pixels.is_empty() {
        return Ok(());
    }
    let Some(ai_cfa) = ai_cfa else {
        queue.write_texture(
            copy_texture(texture),
            bytemuck::cast_slice(&raw.raw_pixels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raw.width * 2),
                rows_per_image: Some(raw.height),
            },
            texture_size(raw.width, raw.height),
        );
        return Ok(());
    };
    anyhow::ensure!(
        ai_cfa.len() == raw.raw_pixels.len(),
        "AI-denoised mosaic has {} values, expected {}",
        ai_cfa.len(),
        raw.raw_pixels.len()
    );
    write_packed_rows(queue, texture, raw, 4, |source| {
        [raw.raw_pixels[source], ai_cfa[source]]
    })
}

pub(super) fn create_black_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
) -> wgpu::Texture {
    let texture = create_per_pixel_texture(
        device,
        raw,
        wgpu::TextureFormat::R32Float,
        "calibraw per-pixel black levels",
    );
    if !raw.is_pre_demosaiced_raster() {
        upload_black_texture(queue, &texture, raw);
    }
    texture
}

pub(super) fn upload_black_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, raw: &LoadedRaw) {
    if raw.black_levels_per_pixel.storage_width() == raw.width
        && raw.black_levels_per_pixel.storage_height() == raw.height
    {
        queue.write_texture(
            copy_texture(texture),
            bytemuck::cast_slice(raw.black_levels_per_pixel.storage_slice()),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raw.width * 4),
                rows_per_image: Some(raw.height),
            },
            texture_size(raw.width, raw.height),
        );
        return;
    }

    const MAX_EXPANSION_BYTES: usize = MAX_UPLOAD_SCRATCH_BYTES;
    let width = raw.width as usize;
    let row_bytes = width.saturating_mul(std::mem::size_of::<f32>()).max(1);
    let rows_per_chunk = (MAX_EXPANSION_BYTES / row_bytes)
        .max(1)
        .min(raw.height as usize);
    BLACK_UPLOAD_SCRATCH.with(|scratch| {
        let mut values = scratch.borrow_mut();
        values.clear();
        let required_capacity = width.saturating_mul(rows_per_chunk);
        if values.capacity() < required_capacity {
            let additional = required_capacity - values.capacity();
            values.reserve(additional);
        }
        let mut row_start = 0u32;
        while row_start < raw.height {
            let rows = (rows_per_chunk as u32).min(raw.height - row_start);
            values.clear();
            for y in row_start..row_start + rows {
                raw.black_levels_per_pixel.append_row_to(y, &mut *values);
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: row_start,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(values.as_slice()),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(raw.width * 4),
                    rows_per_image: Some(rows),
                },
                texture_size(raw.width, rows),
            );
            row_start += rows;
        }
    });
}

pub(super) fn create_color_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
) -> wgpu::Texture {
    let texture = create_per_pixel_texture(
        device,
        raw,
        wgpu::TextureFormat::R8Uint,
        "calibraw CFA color indices",
    );
    if !raw.is_pre_demosaiced_raster() {
        upload_color_texture(queue, &texture, raw);
    }
    texture
}

/// The RGBA32F texture of `raw`'s lens shading gains, which highlight
/// reconstruction applies to its output. It is always
/// [`LensShadingMap::EDGE`] square, so a reused tile pipeline refills it.
pub(super) fn create_lens_shading_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
) -> wgpu::Texture {
    let edge = LensShadingMap::EDGE;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("calibraw lens shading gains"),
        size: texture_size(edge, edge),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    upload_lens_shading_texture(queue, &texture, raw);
    texture
}

/// Uploads `raw`'s lens shading gains, or unit gains when it has none.
pub(super) fn upload_lens_shading_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    raw: &LoadedRaw,
) {
    let edge = LensShadingMap::EDGE;
    let texels: Vec<[f32; 4]> = match &raw.lens_shading {
        Some(map) => map
            .gains()
            .iter()
            .map(|&[red, green, blue]| [red, green, blue, 1.0])
            .collect(),
        None => vec![[1.0; 4]; (edge * edge) as usize],
    };
    queue.write_texture(
        copy_texture(texture),
        bytemuck::cast_slice(&texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(edge * 16),
            rows_per_image: Some(edge),
        },
        texture_size(edge, edge),
    );
}

fn create_per_pixel_texture(
    device: &wgpu::Device,
    raw: &LoadedRaw,
    format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::Texture {
    let sensor_size = if raw.is_pre_demosaiced_raster() {
        texture_size(1, 1)
    } else {
        texture_size(raw.width, raw.height)
    };
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: sensor_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[format],
    })
}

pub(super) fn upload_color_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, raw: &LoadedRaw) {
    if raw.color_indices.storage_width() == raw.width
        && raw.color_indices.storage_height() == raw.height
    {
        queue.write_texture(
            copy_texture(texture),
            raw.color_indices.storage_slice(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raw.width),
                rows_per_image: Some(raw.height),
            },
            texture_size(raw.width, raw.height),
        );
        return;
    }

    const MAX_EXPANSION_BYTES: usize = MAX_UPLOAD_SCRATCH_BYTES;
    let width = raw.width as usize;
    let rows_per_chunk = (MAX_EXPANSION_BYTES / width.max(1))
        .max(1)
        .min(raw.height as usize);
    COLOR_UPLOAD_SCRATCH.with(|scratch| {
        let mut values = scratch.borrow_mut();
        values.clear();
        let required_capacity = width.saturating_mul(rows_per_chunk);
        if values.capacity() < required_capacity {
            let additional = required_capacity - values.capacity();
            values.reserve(additional);
        }
        let mut row_start = 0u32;
        while row_start < raw.height {
            let rows = (rows_per_chunk as u32).min(raw.height - row_start);
            values.clear();
            for y in row_start..row_start + rows {
                raw.color_indices.append_row_to(y, &mut *values);
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: row_start,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                values.as_slice(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(raw.width),
                    rows_per_image: Some(rows),
                },
                texture_size(raw.width, rows),
            );
            row_start += rows;
        }
    });
}

pub(super) fn copy_texture(texture: &wgpu::Texture) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    }
}

pub(super) fn texture_size(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}
