use super::basicadj::sigmoid_contrast_from_percent;
use super::gpu_cache::PersistentGpuPipelineCache;
use super::sigmoid::coefficients as sigmoid_coefficients;
use crate::pipeline::{
    canonical_remove_scene_to_pipeline_scene, effect_params, export_mask_atlas_edge_limit,
    mask_atlas_edge, pipeline_scene_to_working_rec2020, AiDenoisedImage, CfaKind, ExposureParams,
    GeometryTransform, HighlightReconstructionMethod, LensGeometryMap, LoadedRaw, LocalMask,
    MaskEffect, MaskImage, MaskStack, PointColor, PointCurve, ProcessingStage, RawThumbnail,
    RemoveEditState, RemovePatch, SigmoidParams, GLOBAL_TEMPERATURE_LIMIT,
    GLOBAL_TINT_OFFSET_LIMIT, MAX_EFFECT_COMPONENTS, MAX_LOCAL_MASKS, MAX_POINT_COLORS,
    MAX_POINT_CURVE_POINTS,
};
use anyhow::{anyhow, Context, Result};
use bytemuck::{Pod, Zeroable};
use calibraw_core::color_math::{linear_srgb_to_oklab, srgb_decode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use crate::gpu_errors::GpuErrorScopes;

mod builder;
mod clipping;
mod construction;
mod effect_inputs;
mod histogram;
mod mask_layers;
mod readback;
mod resources;
mod scene_depth;
mod scene_surface;
mod shader_manager;
mod shaders;

use builder::*;
pub use clipping::PreviewClippingGpu;
use effect_inputs::{EffectInputs, InputBindingSources, InputBoundPass};
pub use histogram::{PreviewHistogram, PreviewHistogramGpu};
use readback::*;
use resources::*;
use scene_depth::valid_scene_depth;
use scene_surface::SCENE_DEPTH_MIP_LEVELS;
use shader_manager::ShaderManager;

#[cfg(test)]
mod black_tone_tests;
#[cfg(test)]
mod blacks_pipeline_tests;
#[cfg(test)]
mod effect_timing_tests;
#[cfg(test)]
mod existing_effects_tests;
#[cfg(test)]
mod film_effects_tests;
#[cfg(test)]
mod fog_tests;
#[cfg(test)]
mod layout_contract_tests;
#[cfg(test)]
mod light_rays_tests;
#[cfg(test)]
mod photographic_modules_tests;
#[cfg(test)]
mod pixelate_tests;
#[cfg(test)]
mod point_color_tests;
#[cfg(test)]
mod relight_tests;
#[cfg(test)]
mod scene_lights_tests;
#[cfg(test)]
mod tests;

mod effect_lanes;
mod mask_params;
mod params;
mod remove_scene;
mod stage_params;
use mask_params::*;
pub use params::*;
pub use remove_scene::*;
use stage_params::*;

const GPU_PARAMS_ABI_VERSION: u32 = 10;
const MASK_EFFECT_ID_SHIFT: u32 = 8;
pub(super) const LIGHT_RAYS_MASK_ATLAS_EDGE: u32 = if cfg!(target_os = "android") {
    256
} else {
    512
};
const GPU_PARAMS_ABI_SIZE_BYTES: u32 = 1_072;
const CAMERA_UNIFORMS_SIZE_BYTES: u32 = 352;
const SCENE_TONE_UNIFORMS_SIZE_BYTES: u32 = 1_680;
const EFFECTS_UNIFORMS_SIZE_BYTES: u32 = 192;
const GPU_STAGE_UNIFORM_SIZE_BYTES: u32 =
    CAMERA_UNIFORMS_SIZE_BYTES + SCENE_TONE_UNIFORMS_SIZE_BYTES + EFFECTS_UNIFORMS_SIZE_BYTES;
const GPU_STAGE_UNIFORM_ALLOCATION_BYTES: u64 = 512 + SCENE_TONE_UNIFORMS_SIZE_BYTES as u64 + 256;
const MAX_RENDER_MASK_SLOTS: usize =
    MAX_LOCAL_MASKS * (MAX_EFFECT_COMPONENTS + 1) + MAX_EFFECT_COMPONENTS;
const MASK_DATA_SIZE_BYTES: u64 = (std::mem::size_of::<MaskData>() * MAX_RENDER_MASK_SLOTS) as u64;
const WORK_FORMAT_MARKER: &str = "rgba16float /* CALIBRAW_WORK_FORMAT */";
const WORKGROUP_EDGE: u32 = 8;
const TONE_STATS_SIZE_BYTES: u64 = 2 * std::mem::size_of::<[f32; 4]>() as u64;
/// Bins of the scene EV histogram (`ToneHistogram` in `tone_analysis.wgsl`).
const TONE_HISTOGRAM_BIN_COUNT: u32 = 256;
#[cfg(test)]
const DESKTOP_GPU_WORKING_SET_LIMIT_BYTES: u64 = 1_500 * 1024 * 1024;
const ANDROID_GPU_WORKING_SET_LIMIT_BYTES: u64 = 384 * 1024 * 1024;

const COLOR_DENOISE_ENTRY_POINTS: [&str; 6] = [
    "color_denoise_scale_1",
    "color_denoise_scale_2",
    "color_denoise_scale_4",
    "color_denoise_scale_8",
    "color_denoise_scale_16",
    "color_denoise_scale_32",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcessingQuality {
    Preview,
    #[default]
    High,
}

fn expected_pass_count(cfa_kind: CfaKind) -> usize {
    StageIndices::plan(cfa_kind).pass_count
}

#[derive(Clone, Copy, Debug)]
struct UploadedStageUniforms {
    camera: CameraUniforms,
    scene_tone: SceneToneUniforms,
    effects: EffectsUniforms,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RemoveCompositeParams {
    origin: [u32; 2],
    extent: [u32; 2],
}

// Templates share both compiled programs and deferred programs. Keep the explicit
// layouts so reusing a template never compiles an inactive effect just to get its layout.
struct ComputeProgram {
    device: wgpu::Device,
    shader: wgpu::ShaderModule,
    layouts: [wgpu::BindGroupLayout; 3],
    entry: String,
    cache: Option<Arc<PersistentGpuPipelineCache>>,
    compiled: OnceLock<wgpu::ComputePipeline>,
    demosaic_variants: [OnceLock<wgpu::ComputePipeline>; 11],
}

impl ComputeProgram {
    fn get_bind_group_layout(&self, index: u32) -> wgpu::BindGroupLayout {
        self.layouts[index as usize].clone()
    }

    fn get(&self) -> &wgpu::ComputePipeline {
        self.compiled.get_or_init(|| {
            let constants = if self.entry == "bayer_rcd_output" {
                &[
                    ("BAYER_DEMOSAIC_MODE", 0.0),
                    ("BAYER_SENSOR_DENOISE", 0.0),
                    ("BAYER_CA", 0.0),
                ][..]
            } else {
                &[][..]
            };
            self.compile(constants)
        })
    }

    fn for_demosaic_params(&self, camera: &CameraUniforms) -> &wgpu::ComputePipeline {
        if self.entry != "bayer_rcd_output" {
            return self.get();
        }
        let mode = if camera.demosaic_mode >= 1.5 {
            2
        } else {
            usize::from(camera.demosaic_mode >= 0.5)
        };
        let denoise = usize::from(camera.noise_options[0] > 0.0);
        let ca = usize::from(camera.ca_red.abs() > 1e-6 || camera.ca_blue.abs() > 1e-6);
        let variant = mode * 4 + denoise * 2 + ca;
        if variant == 0 {
            return self.get();
        }
        self.demosaic_variants[variant - 1].get_or_init(|| {
            self.compile(&[
                ("BAYER_DEMOSAIC_MODE", mode as f64),
                ("BAYER_SENSOR_DENOISE", denoise as f64),
                ("BAYER_CA", ca as f64),
            ])
        })
    }

    fn compile(&self, constants: &[(&str, f64)]) -> wgpu::ComputePipeline {
        let started = std::time::Instant::now();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&self.entry),
                bind_group_layouts: &self.layouts.each_ref().map(Some),
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(&self.entry),
                layout: Some(&layout),
                module: &self.shader,
                entry_point: Some(&self.entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
                cache: self.cache.as_ref().map(|cache| cache.raw()),
            });
        log::debug!(
            "GPU program {} {constants:?} compiled in {:.3}s",
            self.entry,
            started.elapsed().as_secs_f64()
        );
        pipeline
    }
}

struct Pass {
    pipeline: Arc<ComputeProgram>,
    bindings: PassBindings,
    workgroups: [u32; 3],
}

/// Where a pass takes its group-0 bind group from.
enum PassBindings {
    Fixed(wgpu::BindGroup),
    /// Bound to effect inputs that may be allocated later (`effect_inputs`).
    Inputs(InputBoundPass),
}

impl From<wgpu::BindGroup> for PassBindings {
    fn from(bind_group: wgpu::BindGroup) -> Self {
        Self::Fixed(bind_group)
    }
}

impl From<InputBoundPass> for PassBindings {
    fn from(pass: InputBoundPass) -> Self {
        Self::Inputs(pass)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RawGpuPipelineConfig {
    mask_atlas_edge_override: Option<u32>,
}

/// How to build a [`RawGpuPipeline`].
#[derive(Clone, Copy)]
pub struct PipelineOptions<'a> {
    pub quality: ProcessingQuality,
    /// A fixed mask atlas edge (exports and crops). Without one, interactive
    /// previews size the atlas from the image and reserve every mask layer.
    pub mask_atlas_edge: Option<u32>,
    /// Compiled programs to reuse; they must match the CFA layout and quality.
    pub programs: Option<&'a RawGpuProgramTemplate>,
}

impl<'a> PipelineOptions<'a> {
    pub const fn new(quality: ProcessingQuality) -> Self {
        Self {
            quality,
            mask_atlas_edge: None,
            programs: None,
        }
    }

    #[must_use]
    pub const fn mask_atlas_edge(mut self, edge: u32) -> Self {
        self.mask_atlas_edge = Some(edge);
        self
    }

    #[must_use]
    pub const fn programs(mut self, template: &'a RawGpuProgramTemplate) -> Self {
        self.programs = Some(template);
        self
    }
}

#[derive(Clone)]
pub struct RawGpuProgramTemplate {
    cfa_kind: CfaKind,
    processing_quality: ProcessingQuality,
    pipelines: Vec<Arc<ComputeProgram>>,
    pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
}

#[derive(Default)]
pub struct GpuProgramPrewarm {
    result: Mutex<Option<std::result::Result<Arc<RawGpuProgramTemplate>, String>>>,
    ready: Condvar,
}

impl GpuProgramPrewarm {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, result: std::result::Result<RawGpuProgramTemplate, String>) {
        let Ok(mut slot) = self.result.lock() else {
            return;
        };
        if slot.is_none() {
            *slot = Some(result.map(Arc::new));
            self.ready.notify_all();
        }
    }

    pub fn wait(&self) -> std::result::Result<Arc<RawGpuProgramTemplate>, String> {
        let mut slot = self
            .result
            .lock()
            .map_err(|_| "GPU export prewarm state was poisoned".to_owned())?;
        while slot.is_none() {
            slot = self
                .ready
                .wait(slot)
                .map_err(|_| "GPU export prewarm state was poisoned".to_owned())?;
        }
        slot.as_ref()
            .expect("GPU export prewarm result is present")
            .clone()
    }
}

pub struct RawGpuPipeline {
    output_revision: std::sync::atomic::AtomicU64,
    pub width: u32,
    pub height: u32,
    tone_guide_extent: [u32; 2],
    cfa_kind: CfaKind,
    processing_quality: ProcessingQuality,
    camera_uniforms_buffer: wgpu::Buffer,
    scene_tone_uniforms_buffer: wgpu::Buffer,
    effects_uniforms_buffer: wgpu::Buffer,
    scene_tone_bind_group: wgpu::BindGroup,
    effects_bind_group: wgpu::BindGroup,
    remove_composite_pipeline: wgpu::ComputePipeline,
    remove_composite_bind_group: wgpu::BindGroup,
    remove_composite_params_buffer: wgpu::Buffer,
    uploaded_stage_uniforms: Mutex<UploadedStageUniforms>,
    mask_data_buffer: wgpu::Buffer,
    tone_histogram_buffer: wgpu::Buffer,
    tone_stats_buffer: wgpu::Buffer,
    /// Relight's surface and shadow map and the image-light map, allocated
    /// when an effect first reads them. Cropped and zoomed views copy the
    /// image-light map from the full frame like the tone statistics.
    effect_inputs: EffectInputs,
    /// Whether the image-light map matches the current tone result. The tone
    /// stage builds it only when an effect reads it; otherwise the output
    /// stage builds it once one does (`ensure_image_lights`).
    image_lights_current: AtomicBool,
    /// Where each processing stage starts and ends in `passes`.
    indices: StageIndices,
    post_blur_glow_passes: Vec<Pass>,
    post_blur_pixelate_blocks_pass: Pass,
    post_blur_scene_depth_guide_pass: Pass,
    post_blur_creative_pass: Pass,
    post_blur_render_pass: Pass,
    passes: Vec<Pass>,
    raw_texture: wgpu::Texture,
    color_texture: wgpu::Texture,
    black_texture: wgpu::Texture,
    _reconstructed_raw_texture: wgpu::Texture,
    _highlight_work_a: wgpu::Texture,
    _highlight_work_b: wgpu::Texture,
    _tex1: wgpu::Texture,
    _tex2: wgpu::Texture,
    scene_texture: wgpu::Texture,
    scene_format: wgpu::TextureFormat,
    has_ai_scene: bool,
    has_raster_scene: bool,
    has_ai_cfa: bool,
    display_linear_texture: wgpu::Texture,
    _tone_guide_a: wgpu::Texture,
    _tone_guide_b: wgpu::Texture,
    mask_texture: wgpu::Texture,
    light_rays_mask_texture: wgpu::Texture,
    scene_depth_texture: wgpu::Texture,
    uploaded_scene_depth: Mutex<Option<scene_depth::UploadedSceneDepth>>,
    /// What the Relight shadow map (relight.wgsl) was last built for; `None` after scene depth
    /// changes, so the next output stage rebuilds it.
    relight_shadow_map_key: Mutex<Option<RelightShadowMapKey>>,
    mask_layer_capacity: usize,
    mask_atlas_edge: u32,
    out_texture: wgpu::Texture,
    out_view: wgpu::TextureView,
    pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
    _gpu_budget_reservation: GpuBudgetReservation,
}

#[derive(Clone)]
pub struct GpuOutputSnapshot {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

impl GpuOutputSnapshot {
    pub fn read_thumbnail_blocking(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        maximum_edge: u32,
    ) -> Result<RawThumbnail> {
        if maximum_edge == 0 {
            return Err(anyhow!("thumbnail edge must be non-zero"));
        }
        let rgba = read_rgba8_texture_region_blocking(
            device,
            queue,
            &self.texture,
            TextureReadbackRegion::full(
                self.width,
                self.height,
                "calibraw developed thumbnail readback",
            ),
        )?;
        let image = image::RgbaImage::from_raw(self.width, self.height, rgba)
            .ok_or_else(|| anyhow!("developed thumbnail readback has an invalid byte count"))?;
        let image = calibraw_core::thumbnail_cache::downscale_to_fit(
            image::DynamicImage::ImageRgba8(image),
            maximum_edge,
        )
        .to_rgba8();
        let (width, height) = image.dimensions();
        Ok(RawThumbnail {
            width,
            height,
            rgba: image.into_raw(),
        })
    }
}

struct RawGpuPipelineBuild<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    program_template: Option<&'a RawGpuProgramTemplate>,
    pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
    raw: &'a LoadedRaw,
    params: &'a GpuParams,
    quality: ProcessingQuality,
    config: RawGpuPipelineConfig,
}

impl RawGpuPipeline {
    fn upload_params(&self, queue: &wgpu::Queue, params: &GpuParams) {
        self.prepare_effect_inputs(params);
        self.upload_scene_depth(
            queue,
            params.scene_depth.as_ref(),
            params.needs_relight_surface(),
        );
        match self.uploaded_stage_uniforms.lock() {
            Ok(mut uploaded) => {
                if bytemuck::bytes_of(&uploaded.camera) != params.camera_bytes() {
                    queue.write_buffer(&self.camera_uniforms_buffer, 0, params.camera_bytes());
                    uploaded.camera = params.camera;
                }
                if bytemuck::bytes_of(&uploaded.scene_tone) != params.scene_tone_bytes() {
                    queue.write_buffer(
                        &self.scene_tone_uniforms_buffer,
                        0,
                        params.scene_tone_bytes(),
                    );
                    uploaded.scene_tone = params.scene_tone;
                }
                if bytemuck::bytes_of(&uploaded.effects) != params.effects_bytes() {
                    queue.write_buffer(&self.effects_uniforms_buffer, 0, params.effects_bytes());
                    uploaded.effects = params.effects;
                }
            }
            Err(_) => {
                queue.write_buffer(&self.camera_uniforms_buffer, 0, params.camera_bytes());
                queue.write_buffer(
                    &self.scene_tone_uniforms_buffer,
                    0,
                    params.scene_tone_bytes(),
                );
                queue.write_buffer(&self.effects_uniforms_buffer, 0, params.effects_bytes());
            }
        }
        queue.write_buffer(&self.mask_data_buffer, 0, params.mask_data_bytes());
    }

    pub fn tone_guide_supports_origin(&self, origin_x: i32, origin_y: i32) -> bool {
        let scale = tone_analysis_scale();
        self.tone_guide_extent
            == [
                tone_guide_axis_cell_count(origin_x, self.width, scale),
                tone_guide_axis_cell_count(origin_y, self.height, scale),
            ]
    }

    pub const fn mask_atlas_edge(&self) -> u32 {
        self.mask_atlas_edge
    }

    pub const fn mask_layer_capacity(&self) -> usize {
        self.mask_layer_capacity
    }

    pub const fn immutable_ai_source_matches(&self, cfa_kind: CfaKind, enabled: bool) -> bool {
        match cfa_kind {
            CfaKind::Bayer => self.has_ai_cfa == enabled,
            CfaKind::XTrans => !enabled || self.has_ai_scene,
        }
    }

    /// The final display-referred output (`Rgba8Unorm`, sRGB-encoded values),
    /// which presentation layers may sample, e.g. by registering it as an
    /// egui texture. It stays valid for the pipeline's lifetime.
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.out_view
    }

    pub fn recompute(&self, queue: &wgpu::Queue, device: &wgpu::Device, params: &GpuParams) {
        self.upload_params(queue, params);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw complete recompute encoder"),
        });
        encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
        self.encode_raw_stage(&mut encoder, params);
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_stage_end,
        );
        self.encode_tone_image_lights(&mut encoder, params);
        self.encode_output_stage(&mut encoder, params);
        queue.submit(Some(encoder.finish()));
    }

    pub fn dispatch_stage(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        stage: ProcessingStage,
    ) {
        self.upload_params(queue, params);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(stage.label()),
        });

        match stage {
            ProcessingStage::Raw => self.encode_raw_stage(&mut encoder, params),
            ProcessingStage::Tone => {
                encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
                self.encode_pass_range(
                    &mut encoder,
                    self.indices.tone_prepare_pass_index,
                    self.indices.tone_stage_end,
                );
                self.encode_tone_image_lights(&mut encoder, params);
            }
            ProcessingStage::Output => self.encode_output_stage(&mut encoder, params),
        }

        queue.submit(Some(encoder.finish()));
    }

    pub fn upload_raw_tile(&self, queue: &wgpu::Queue, raw: &LoadedRaw) -> Result<()> {
        if raw.width != self.width || raw.height != self.height {
            return Err(anyhow!(
                "tile dimensions {}x{} do not match reusable pipeline {}x{}",
                raw.width,
                raw.height,
                self.width,
                self.height
            ));
        }
        validate_raw(raw)?;

        if self.has_raster_scene {
            anyhow::ensure!(
                raw.is_pre_demosaiced_raster(),
                "reusable raster pipeline received a sensor RAW tile"
            );
            anyhow::ensure!(
                upload_ai_scene_texture(queue, &self.scene_texture, self.scene_format, raw)?,
                "reusable raster pipeline received a tile without scene-linear RGB"
            );
            return Ok(());
        }

        let ai_image = raw.ai_denoised_image();
        let raw_pixels = if self.has_ai_cfa {
            ai_image
                .as_ref()
                .and_then(AiDenoisedImage::bayer_cfa)
                .context("reusable AI-denoise pipeline received a tile without denoised CFA")?
        } else {
            raw.raw_pixels.as_slice()
        };
        queue.write_texture(
            copy_texture(&self.raw_texture),
            bytemuck::cast_slice(raw_pixels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(raw.width * 2),
                rows_per_image: Some(raw.height),
            },
            texture_size(raw.width, raw.height),
        );
        upload_color_texture(queue, &self.color_texture, raw);
        upload_black_texture(queue, &self.black_texture, raw);
        if self.has_ai_scene {
            anyhow::ensure!(
                upload_ai_scene_texture(queue, &self.scene_texture, self.scene_format, raw)?,
                "reusable AI-denoise pipeline received a tile without derived model output"
            );
        }
        Ok(())
    }

    pub fn begin_export_tone_analysis(&self, queue: &wgpu::Queue, device: &wgpu::Device) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export tone histogram clear"),
        });
        encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
        if let Some((cells, _)) = self.effect_inputs.image_lights() {
            encoder.clear_buffer(&cells, 0, None);
        }
        queue.submit(Some(encoder.finish()));
        self.image_lights_current.store(false, Ordering::Release);
    }

    /// Allocates the effect inputs `params` reads (`effect_inputs`).
    fn prepare_effect_inputs(&self, params: &GpuParams) {
        if params.needs_scene_depth_guide() {
            self.effect_inputs.allocate_scene_depth_guide();
        }
        if params.needs_relight_surface() && self.effect_inputs.allocate_relight() {
            // Scene depth now goes to the Relight surface, which replaced the
            // plain depth texture in every bind group.
            self.forget_uploaded_scene_depth();
        }
        if params.needs_image_lights() {
            self.effect_inputs.allocate_image_lights();
        }
    }

    /// Builds the image-light map from this pipeline's whole image, after the
    /// tone statistics it is resolved against.
    fn encode_image_lights(&self, encoder: &mut wgpu::CommandEncoder) {
        self.effect_inputs.allocate_image_lights();
        if let Some((cells, _)) = self.effect_inputs.image_lights() {
            encoder.clear_buffer(&cells, 0, None);
        }
        self.encode_pass_range(
            encoder,
            self.indices.image_light_accumulate_pass_index,
            self.indices.image_light_end_index,
        );
        self.image_lights_current.store(true, Ordering::Release);
    }

    /// Image lights for a new tone result: built when an effect reads them,
    /// otherwise left for `ensure_image_lights`, which most edits never need.
    fn encode_tone_image_lights(&self, encoder: &mut wgpu::CommandEncoder, params: &GpuParams) {
        if params.needs_image_lights() {
            self.encode_image_lights(encoder);
        } else {
            self.image_lights_current.store(false, Ordering::Release);
        }
    }

    /// Builds the image-light map if the current tone result has none yet.
    /// The scene and tone statistics it reads stay valid until the next raw or
    /// tone stage.
    fn ensure_image_lights(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.image_lights_current.load(Ordering::Acquire) {
            self.encode_image_lights(encoder);
        }
    }

    /// Takes the full frame's image-light map when `params` reads it, so a
    /// cropped or zoomed view sees lights outside its region.
    fn inherit_image_lights(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        full_frame: &Self,
        params: &GpuParams,
    ) {
        if !params.needs_image_lights() {
            self.image_lights_current.store(false, Ordering::Release);
            return;
        }
        full_frame.ensure_image_lights(encoder);
        self.effect_inputs.allocate_image_lights();
        let (Some((_, source)), Some((_, destination))) = (
            full_frame.effect_inputs.image_lights(),
            self.effect_inputs.image_lights(),
        ) else {
            return;
        };
        encoder.copy_texture_to_texture(
            source.as_image_copy(),
            destination.as_image_copy(),
            destination.size(),
        );
        self.image_lights_current.store(true, Ordering::Release);
    }

    pub fn dispatch_stage_with_remove(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        stage: ProcessingStage,
        remove: RemoveSceneContext<'_>,
    ) -> Result<()> {
        self.dispatch_stage(queue, device, params, stage);
        if stage == ProcessingStage::Raw {
            self.upload_remove_scene_patches(queue, device, remove)?;
        }
        Ok(())
    }

    pub fn recompute_with_remove(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        remove: RemoveSceneContext<'_>,
    ) -> Result<()> {
        for stage in [
            ProcessingStage::Raw,
            ProcessingStage::Tone,
            ProcessingStage::Output,
        ] {
            self.dispatch_stage_with_remove(queue, device, params, stage, remove)?;
        }
        Ok(())
    }

    pub fn dispatch_tone_guide_with_inherited_statistics(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        full_frame: &Self,
    ) {
        self.upload_params(queue, params);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw crop tone guide with full-frame statistics"),
        });
        encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
        encoder.copy_buffer_to_buffer(
            &full_frame.tone_stats_buffer,
            0,
            &self.tone_stats_buffer,
            0,
            TONE_STATS_SIZE_BYTES,
        );
        self.inherit_image_lights(&mut encoder, full_frame, params);
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        queue.submit(Some(encoder.finish()));
    }

    pub fn inherit_tone_statistics(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        full_frame: &Self,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw inherit full-frame tone statistics"),
        });
        encoder.copy_buffer_to_buffer(
            &full_frame.tone_stats_buffer,
            0,
            &self.tone_stats_buffer,
            0,
            TONE_STATS_SIZE_BYTES,
        );
        self.inherit_image_lights(&mut encoder, full_frame, params);
        queue.submit(Some(encoder.finish()));
    }

    pub fn accumulate_export_tone_tile_with_remove(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        remove: RemoveSceneContext<'_>,
    ) -> Result<()> {
        self.dispatch_stage_with_remove(queue, device, params, ProcessingStage::Raw, remove)?;
        self.upload_params(queue, params);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw Remove-aware export tone tile"),
        });
        self.encode_pass(&mut encoder, self.indices.tone_prepare_pass_index);
        // Each tile adds its core to the shared image-light grid. Without a
        // receiver the grid stays empty and the tiles skip reading it all again.
        if params.needs_image_lights() {
            self.effect_inputs.allocate_image_lights();
            self.encode_pass(&mut encoder, self.indices.image_light_accumulate_pass_index);
        }
        queue.submit(Some(encoder.finish()));
        Ok(())
    }

    pub fn finish_export_tone_analysis(&self, queue: &wgpu::Queue, device: &wgpu::Device) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export tone histogram reduction"),
        });
        self.encode_pass(&mut encoder, self.indices.tone_reduce_pass_index);
        // Tiles accumulated image lights only if an effect reads them, which
        // allocated the grid.
        let image_lights = self.effect_inputs.image_lights().is_some();
        if image_lights {
            self.encode_pass_range(
                &mut encoder,
                self.indices.image_light_resolve_pass_index,
                self.indices.image_light_end_index,
            );
        }
        queue.submit(Some(encoder.finish()));
        // Resolved from every tile's accumulation; tiles must not rebuild it
        // from their own region.
        self.image_lights_current
            .store(image_lights, Ordering::Release);
    }

    pub fn dispatch_export_tile_with_remove(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        remove: RemoveSceneContext<'_>,
    ) -> Result<()> {
        self.dispatch_stage_with_remove(queue, device, params, ProcessingStage::Raw, remove)?;
        self.upload_params(queue, params);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw Remove-aware tiled export encoder"),
        });
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        self.encode_output_stage(&mut encoder, params);
        queue.submit(Some(encoder.finish()));
        Ok(())
    }

    fn encode_raw_stage(&self, encoder: &mut wgpu::CommandEncoder, params: &GpuParams) {
        if self.has_raster_scene {
            if params.camera.chroma_denoise > 1e-6 {
                self.encode_pass_range(
                    encoder,
                    self.indices.color_denoise_start_index,
                    self.indices.color_denoise_end_index,
                );
            }
            return;
        }
        if self.has_ai_scene && params.uses_ai_denoise() {
            return;
        }
        self.encode_pass(encoder, self.indices.highlight_pass_index);
        self.encode_pass_range(
            encoder,
            self.indices.demosaic_start_index,
            self.indices.demosaic_dual_start_index,
        );
        if params.needs_dual_demosaic_passes() {
            self.encode_pass_range(
                encoder,
                self.indices.demosaic_dual_start_index,
                self.indices.demosaic_dual_end_index,
            );
        }
        let finish = &self.passes[self.indices.demosaic_finish_index];
        dispatch_compute(
            encoder,
            "calibraw demosaic finish",
            finish.pipeline.for_demosaic_params(&params.camera),
            &[
                &self.pass_bind_group(finish),
                &self.scene_tone_bind_group,
                &self.effects_bind_group,
            ],
            finish.workgroups,
        );
        if params.camera.chroma_denoise > 1e-6 {
            self.encode_pass_range(
                encoder,
                self.indices.color_denoise_start_index,
                self.indices.color_denoise_end_index,
            );
        }
    }

    fn encode_output_stage(&self, encoder: &mut wgpu::CommandEncoder, params: &GpuParams) {
        self.output_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if params.needs_image_lights() {
            self.ensure_image_lights(encoder);
        }
        self.encode_relight_shadow_map(encoder, params);
        self.encode_pass(encoder, self.indices.adjustment_prepare_pass_index);
        self.encode_pass(encoder, self.indices.adjustment_tone_pass_index);
        let blur_active = params.needs_blur_passes();
        if params.needs_intermediate_adjustment_passes() {
            self.encode_pass(encoder, self.indices.adjustment_local_tone_pass_index);
            self.encode_pass(encoder, self.indices.adjustment_effects_pass_index);
            self.encode_pass(encoder, self.indices.adjustment_effects_copy_pass_index);
            if blur_active {
                if params.needs_progressive_blur_passes() {
                    self.encode_pass_range(
                        encoder,
                        self.indices.mask_blur_start_index,
                        self.indices.mask_blur_end_index,
                    );
                } else {
                    self.encode_pass(encoder, self.indices.mask_blur_start_index);
                }
            }
            if params.needs_glow_passes() {
                if blur_active {
                    for (index, pass) in self.post_blur_glow_passes.iter().enumerate() {
                        self.encode_bound_pass(
                            encoder,
                            pass,
                            &format!("post-Blur Glow pass {}", index + 1),
                        );
                    }
                } else {
                    self.encode_pass(encoder, self.indices.glow_prepare_pass_index);
                    self.encode_pass_range(
                        encoder,
                        self.indices.glow_blur_start_index,
                        self.indices.glow_blur_end_index,
                    );
                }
            }
            if params.needs_pixelate_block_pass() {
                if blur_active {
                    self.encode_bound_pass(
                        encoder,
                        &self.post_blur_pixelate_blocks_pass,
                        "post-Blur Pixelate block pass",
                    );
                } else {
                    self.encode_pass(encoder, self.indices.pixelate_blocks_pass_index);
                }
            }
            let depth_guide = params.needs_scene_depth_guide();
            if blur_active {
                if depth_guide {
                    self.encode_bound_pass(
                        encoder,
                        &self.post_blur_scene_depth_guide_pass,
                        "post-Blur scene-depth guide pass",
                    );
                }
                self.encode_bound_pass(
                    encoder,
                    &self.post_blur_creative_pass,
                    "post-Blur creative pass",
                );
            } else {
                if depth_guide {
                    self.encode_pass(encoder, self.indices.scene_depth_guide_pass_index);
                }
                self.encode_pass(encoder, self.indices.adjustment_creative_pass_index);
            }
        }
        if blur_active {
            self.encode_bound_pass(
                encoder,
                &self.post_blur_render_pass,
                "post-Blur render pass",
            );
        } else {
            self.encode_pass(encoder, self.indices.adjustment_render_pass_index);
        }
    }

    /// Rebuilds the relight shadow map when a shadowed light or the scene
    /// depth changed since it was last built. Edits that leave both alone,
    /// such as tone or colour changes, reuse it.
    fn encode_relight_shadow_map(&self, encoder: &mut wgpu::CommandEncoder, params: &GpuParams) {
        let Some(key) = params.relight_shadow_map_key() else {
            return;
        };
        let mut built = self
            .relight_shadow_map_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *built == Some(key) {
            return;
        }
        self.encode_pass(encoder, self.indices.relight_shadow_map_pass_index);
        *built = Some(key);
    }

    fn encode_pass(&self, encoder: &mut wgpu::CommandEncoder, index: usize) {
        self.encode_bound_pass(
            encoder,
            &self.passes[index],
            &format!("calibraw pass {}", index + 1),
        );
    }

    fn encode_bound_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pass_record: &Pass,
        label: &str,
    ) {
        let bind_group = self.pass_bind_group(pass_record);
        dispatch_compute(
            encoder,
            label,
            pass_record.pipeline.get(),
            &[
                &bind_group,
                &self.scene_tone_bind_group,
                &self.effects_bind_group,
            ],
            pass_record.workgroups,
        );
    }

    fn pass_bind_group(&self, pass_record: &Pass) -> wgpu::BindGroup {
        match &pass_record.bindings {
            PassBindings::Fixed(bind_group) => bind_group.clone(),
            PassBindings::Inputs(pass) => self.effect_inputs.bind_group(*pass),
        }
    }

    fn encode_pass_range(&self, encoder: &mut wgpu::CommandEncoder, start: usize, end: usize) {
        for index in start..end {
            self.encode_pass(encoder, index);
        }
    }
}
