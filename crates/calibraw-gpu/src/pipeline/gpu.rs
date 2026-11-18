use super::basicadj::sigmoid_contrast_from_percent;
use super::gpu_cache::PersistentGpuPipelineCache;
use super::sigmoid::{coefficients as sigmoid_coefficients, SigmoidCoefficients};
use crate::pipeline::{
    canonical_remove_scene_to_pipeline_scene, effect_params, export_mask_atlas_edge_limit,
    mask_atlas_edge, AiDenoisedImage, BinnedSensor, CfaKind, ExposureParams, FilmSource, FilmStock,
    GeometryMapping, GeometryTransform, HighlightModel, HighlightReconstructionMethod,
    LateralChannel, LensGeometryMap, LoadedRaw, LocalMask, MaskEffect, MaskStack, PixelLayout,
    PointColor, PointCurve, ProcessingStage, ProfileColor, RawThumbnail, RemoveEditState,
    RemovePatch, SceneDepth, SensorSaturation, SigmoidParams, GLOBAL_TEMPERATURE_LIMIT,
    GLOBAL_TINT_OFFSET_LIMIT, MAX_EFFECT_COMPONENTS, MAX_LOCAL_MASKS, MAX_POINT_COLORS,
    MAX_POINT_CURVE_POINTS,
};
use anyhow::{anyhow, Context, Result};
use bytemuck::{Pod, Zeroable};
use calibraw_core::color_math::{linear_srgb_to_oklab, srgb_decode};
use calibraw_core::luts::{CubeLut, LutId, ResolvedLook};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::gpu_errors::GpuErrorScopes;
use crate::stage_timing;

mod builder;
mod clipping;
mod construction;
mod diagnostics;
mod effect_inputs;
mod effect_programs;
mod geometry_resample;
mod histogram;
mod mask_layers;
mod output_parts;
mod preview_budget;
mod readback;
mod resources;
mod scene;
mod unrendered;
use scene::{
    SceneBake, SceneBakeUniforms, SceneLightKind, SceneLightSet, SceneShadowLight,
    SCENE_BAKE_LAYERS, SCENE_SHADOW_LIGHTS, SCENE_WORKING_LONG_EDGE_MAX,
};
mod defocus;
mod shader_manager;
mod shaders;
mod styled_glow;
use defocus::{defocus_layout_entries, DEFOCUS_ENTRIES, DEFOCUS_LAYOUT_LABELS, DEFOCUS_PASSES};
use styled_glow::{
    styled_glow_layout_entries, STYLED_GLOW_ENTRIES, STYLED_GLOW_LAYOUT_LABELS, STYLED_GLOW_PASSES,
};
mod wait;
mod work;
pub(crate) use work::in_flight_summary;
use work::{dispatch_band_entry, DispatchBands, PassDispatch, PassWork, WorkEncoder};

use builder::*;
pub use clipping::PreviewClippingGpu;
use effect_inputs::{
    EffectInputs, FullImageMap, InputBindingSources, InputBoundPass, HAZE_PASSES,
    SCENE_EMISSION_PASSES,
};
use effect_programs::{EffectModules, EffectPrograms, EffectSet, Readiness};
pub(crate) use geometry_resample::{resample_geometry, GeometryResampleJob};
pub use histogram::{PreviewHistogram, PreviewHistogramGpu};
use output_parts::{
    BuildRevision, OutputInputs, OutputPart, OutputResume, OutputStart, OutputTextures,
    SharedMapBuilds,
};
pub(crate) use preview_budget::{
    gpu_working_set_is_limited, scale_with_mobile_export_tile, ExportTileShape,
};
pub use preview_budget::{preview_mask_layer_capacity, MobilePreviewRole, PreviewResourceShape};
pub(crate) use readback::NonFiniteReadback;
pub(crate) use readback::PendingSceneReadback;
use readback::*;
use resources::*;
use shader_manager::ShaderManager;
pub use unrendered::UnrenderedEffect;
use unrendered::{EffectResource, UnrenderedEffects};
pub use wait::GpuWait;
pub(crate) use wait::SLOW_GPU_WAIT;
use wait::{wait_for_submission, GpuWaitError};

#[cfg(test)]
mod layout_contract_tests;
#[cfg(test)]
mod tests;

mod effect_wgsl;
mod mask_params;
mod params;
mod remove_scene;
mod stage_params;
use mask_params::*;
pub use params::*;
pub use remove_scene::*;
use stage_params::*;

const GPU_PARAMS_ABI_VERSION: u32 = 16;
const MASK_EFFECT_ID_SHIFT: u32 = 8;
/// The flag bits of an effect slot's `metadata[3]` that hold its light bake
/// channel plus one (`assign_scene_bake_lights`).
const MASK_SCENE_BAKE_LIGHT_BITS: u32 = 0xf;
pub(super) const FULL_IMAGE_MASK_ATLAS_EDGE: u32 = if cfg!(target_os = "android") {
    256
} else {
    512
};
const GPU_PARAMS_ABI_SIZE_BYTES: u32 = 1_120;
const CAMERA_UNIFORMS_SIZE_BYTES: u32 = 432;
const SCENE_TONE_UNIFORMS_SIZE_BYTES: u32 = 1_776;
const EFFECTS_UNIFORMS_SIZE_BYTES: u32 = 224;
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
/// Android budget that previews are sized against, and the floor of the
/// Android admission cap (see `gpu_working_set_limit_bytes`).
const ANDROID_GPU_WORKING_SET_LIMIT_BYTES: u64 = 384 * 1024 * 1024;
/// Upper bound of the RAM-scaled Android admission cap.
#[cfg(target_os = "android")]
const ANDROID_GPU_WORKING_SET_CEILING_BYTES: u64 = 1_536 * 1024 * 1024;
/// Share of device RAM, as a divisor, of the Android working-set budget
/// previews and export tiles are sized against.
#[cfg(target_os = "android")]
const ANDROID_GPU_WORKING_SET_RAM_DIVISOR: u64 = 8;
/// Share of device RAM, as a divisor, GPU pipelines may reserve together on
/// Android (`gpu_admission_limit_bytes`): 3 GiB of a 12 GiB phone, 1 GiB of a
/// 4 GiB tablet.
#[cfg(target_os = "android")]
const ANDROID_GPU_ADMISSION_RAM_DIVISOR: u64 = 4;
/// Upper bound of the Android admission limit.
#[cfg(target_os = "android")]
const ANDROID_GPU_ADMISSION_CEILING_BYTES: u64 = 3 * 1024 * 1024 * 1024;

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

/// Specializations of `bayer_rcd_output`: demosaic mode (3) × sensor denoise
/// (2), indexed by [`demosaic_variant`].
const DEMOSAIC_VARIANT_COUNT: usize = 6;

const fn demosaic_variant(mode: usize, denoise: bool) -> usize {
    mode * 2 + denoise as usize
}

fn demosaic_variant_for(camera: &CameraUniforms) -> usize {
    let mode = if camera.demosaic_mode >= 1.5 {
        2
    } else {
        usize::from(camera.demosaic_mode >= 0.5)
    };
    demosaic_variant(mode, camera.noise_options[0] > 0.0)
}

fn demosaic_variant_constants(variant: usize) -> [(&'static str, f64); 2] {
    [
        ("BAYER_DEMOSAIC_MODE", (variant / 2) as f64),
        ("BAYER_SENSOR_DENOISE", (variant % 2) as f64),
    ]
}

// Templates share both compiled programs and deferred programs. Keep the explicit
// layouts so reusing a template never compiles an inactive effect just to get its layout.
struct ComputeProgram {
    device: wgpu::Device,
    shader: wgpu::ShaderModule,
    layouts: [wgpu::BindGroupLayout; 3],
    entry: String,
    /// Override constants every compilation takes, before its variant's.
    constants: Vec<(&'static str, f64)>,
    /// How the program's dispatches are measured and cut (`work`).
    work: PassWork,
    cache: Option<Arc<PersistentGpuPipelineCache>>,
    compiled: OnceLock<wgpu::ComputePipeline>,
    demosaic_variants: [OnceLock<wgpu::ComputePipeline>; DEMOSAIC_VARIANT_COUNT - 1],
    /// The program per effect set, for a program that dispatches effects
    /// (`effect_programs`); `compiled` holds every effect.
    effects: Option<EffectPrograms>,
}

impl ComputeProgram {
    fn get_bind_group_layout(&self, index: u32) -> wgpu::BindGroupLayout {
        self.layouts[index as usize].clone()
    }

    fn get(&self) -> &wgpu::ComputePipeline {
        self.compiled.get_or_init(|| {
            if self.is_bayer_demosaic() {
                self.compile(&demosaic_variant_constants(0))
            } else {
                self.compile(&[])
            }
        })
    }

    fn is_bayer_demosaic(&self) -> bool {
        self.entry == "bayer_rcd_output"
    }

    fn for_demosaic_params(&self, camera: &CameraUniforms) -> &wgpu::ComputePipeline {
        if !self.is_bayer_demosaic() {
            return self.get();
        }
        self.demosaic_variant(demosaic_variant_for(camera))
    }

    fn demosaic_variant(&self, variant: usize) -> &wgpu::ComputePipeline {
        if variant == 0 {
            return self.get();
        }
        self.demosaic_variants[variant - 1]
            .get_or_init(|| self.compile(&demosaic_variant_constants(variant)))
    }

    /// Compiles the program with its own constants and `variant`'s.
    fn compile(&self, variant: &[(&str, f64)]) -> wgpu::ComputePipeline {
        let started = std::time::Instant::now();
        let constants: Vec<(&str, f64)> = self
            .constants
            .iter()
            .copied()
            .chain(variant.iter().copied())
            .collect();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&self.entry),
                bind_group_layouts: &self.layouts.each_ref().map(Some),
                immediate_size: 0,
            });
        let pipeline = compile_compute_pipeline(
            &self.device,
            &wgpu::ComputePipelineDescriptor {
                label: Some(&self.entry),
                layout: Some(&layout),
                module: &self.shader,
                entry_point: Some(&self.entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                cache: self.cache.as_ref().map(|cache| cache.raw()),
            },
        );
        if let Some(cache) = &self.cache {
            cache.note_compiled();
        }
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
    mask_layer_capacity_override: Option<usize>,
    profile_capacity_bytes: u64,
}

/// Room for the tables of a camera profile previewed on a graph built for
/// another ([`PipelineOptions::profile_preview`]). The preview budget
/// already reserves it for every preview graph (`preview_budget`); common
/// DCPs need a few hundred kilobytes.
pub const PREVIEWED_PROFILE_BYTES: u64 = 4 * 1024 * 1024;

/// How to build a [`RawGpuPipeline`].
#[derive(Clone, Copy)]
pub struct PipelineOptions<'a> {
    pub quality: ProcessingQuality,
    /// A fixed mask atlas edge (exports and crops). Without one, interactive
    /// previews size the atlas from the image and reserve every mask layer.
    pub mask_atlas_edge: Option<u32>,
    /// A fixed number of mask layers for an interactive preview that sizes
    /// its atlas from the image. A graph with more masks than this must be
    /// rebuilt; see [`RawGpuPipeline::mask_layer_capacity`].
    pub mask_layers: Option<usize>,
    /// Compiled programs to reuse; they must match the CFA layout and quality.
    pub programs: Option<&'a RawGpuProgramTemplate>,
    /// The persistent cache programs compile with when no `programs` are
    /// reused, whose cache is used instead.
    pub pipeline_cache: Option<&'a Arc<PersistentGpuPipelineCache>>,
    /// Reserve [`PREVIEWED_PROFILE_BYTES`] for the tables of other camera
    /// profiles, so parameters with another profile of the same photo
    /// ([`GpuParams::new_with_profile_color`]) render without a rebuild.
    pub profile_preview: bool,
}

impl<'a> PipelineOptions<'a> {
    pub const fn new(quality: ProcessingQuality) -> Self {
        Self {
            quality,
            mask_atlas_edge: None,
            mask_layers: None,
            programs: None,
            pipeline_cache: None,
            profile_preview: false,
        }
    }

    #[must_use]
    pub const fn mask_atlas_edge(mut self, edge: u32) -> Self {
        self.mask_atlas_edge = Some(edge);
        self
    }

    #[must_use]
    pub const fn mask_layers(mut self, layers: usize) -> Self {
        self.mask_layers = Some(layers);
        self
    }

    #[must_use]
    pub const fn programs(mut self, template: &'a RawGpuProgramTemplate) -> Self {
        self.programs = Some(template);
        self
    }

    #[must_use]
    pub const fn pipeline_cache(mut self, cache: &'a Arc<PersistentGpuPipelineCache>) -> Self {
        self.pipeline_cache = Some(cache);
        self
    }

    #[must_use]
    pub const fn profile_preview(mut self, enabled: bool) -> Self {
        self.profile_preview = enabled;
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

/// Templates share compiled and deferred programs. Construction warms the
/// active passes; optional effects compile only when dispatched. Preparing a
/// selected demosaic variant can take seconds, so call [`Self::compile_for_params`]
/// off the UI thread.
impl RawGpuProgramTemplate {
    /// Whether a pipeline of `quality` for `raw` can be built from these
    /// programs ([`PipelineOptions::programs`]).
    pub fn fits(&self, raw: &LoadedRaw, quality: ProcessingQuality) -> bool {
        self.cfa_kind == raw.cfa_kind
            && self.processing_quality == quality
            && self.pipelines.len() == expected_pass_count(raw.cfa_kind)
    }

    /// Compiles the demosaic variant that rendering `params` selects.
    pub fn compile_for_params(&self, params: &GpuParams) {
        for program in self.bayer_demosaic_programs() {
            program.for_demosaic_params(&params.camera);
        }
    }

    /// Saves the programs compiled so far to the persistent pipeline cache the
    /// template compiles with, if it has one and a program compiled since its
    /// last save, so the next launch loads them.
    pub fn persist_pipeline_cache(&self) -> Result<()> {
        if let Some(cache) = &self.pipeline_cache {
            cache.persist_if_changed()?;
        }
        Ok(())
    }

    fn bayer_demosaic_programs(&self) -> impl Iterator<Item = &ComputeProgram> {
        self.pipelines
            .iter()
            .filter(|program| program.is_bayer_demosaic())
            .map(Arc::as_ref)
    }
}

/// Programs shared by photo workers, keyed by sensor layout and render
/// quality and kept from the first pipeline built for each combination
/// ([`RawGpuPipeline::new_reusing`]).
#[derive(Default)]
pub struct GpuProgramPrewarm {
    result: Mutex<Option<std::result::Result<Arc<RawGpuProgramTemplate>, String>>>,
    ready: Condvar,
    /// The persistent cache of a prewarm without startup programs.
    pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
    /// The UI opens the disk cache on the first photo worker.
    cache_directory: Option<std::path::PathBuf>,
    disk_cache: OnceLock<Option<Arc<PersistentGpuPipelineCache>>>,
    /// Templates the startup one does not fit.
    kept: Mutex<Vec<Arc<RawGpuProgramTemplate>>>,
    /// Held while a pipeline is built without a template, so two jobs that
    /// start together compile its programs once.
    building: Mutex<()>,
}

impl GpuProgramPrewarm {
    pub fn new() -> Self {
        Self::default()
    }

    /// A prewarm without startup programs, whose pipelines compile with `cache`.
    pub fn without_startup_programs(cache: Option<Arc<PersistentGpuPipelineCache>>) -> Self {
        let prewarm = Self {
            pipeline_cache: cache,
            ..Self::default()
        };
        if let Ok(mut slot) = prewarm.result.lock() {
            *slot = Some(Err("no programs were compiled at startup".to_owned()));
        }
        prewarm
    }

    /// Share programs compiled for actual photos. Opening the library alone
    /// performs no driver compilation or disk-cache initialization.
    pub fn on_demand(cache_directory: Option<std::path::PathBuf>) -> Self {
        Self {
            cache_directory,
            ..Self::without_startup_programs(None)
        }
    }

    fn load_pipeline_cache(
        &self,
        device: &wgpu::Device,
    ) -> Option<Arc<PersistentGpuPipelineCache>> {
        if let Some(root) = &self.cache_directory {
            return self
                .disk_cache
                .get_or_init(|| {
                    match PersistentGpuPipelineCache::load_or_create(
                        device,
                        &device.adapter_info(),
                        root,
                    ) {
                        Ok(cache) => cache.map(|(cache, _)| cache),
                        Err(error) => {
                            log::warn!("could not load GPU pipeline cache: {error:#}");
                            None
                        }
                    }
                })
                .clone();
        }
        self.pipeline_cache()
    }

    /// The programs a pipeline of `quality` for `raw` can be built from, once
    /// the startup programs are published.
    fn template_for(
        &self,
        raw: &LoadedRaw,
        quality: ProcessingQuality,
    ) -> Option<Arc<RawGpuProgramTemplate>> {
        if let Some(template) = self
            .wait()
            .ok()
            .filter(|template| template.fits(raw, quality))
        {
            return Some(template);
        }
        self.kept
            .lock()
            .ok()?
            .iter()
            .find(|template| template.fits(raw, quality))
            .cloned()
    }

    /// The persistent cache pipelines built without a template compile with.
    fn pipeline_cache(&self) -> Option<Arc<PersistentGpuPipelineCache>> {
        self.pipeline_cache.clone().or_else(|| {
            self.wait()
                .ok()
                .and_then(|template| template.pipeline_cache.clone())
        })
    }

    /// Saves the programs compiled so far to the persistent pipeline cache, if
    /// one compiled since its last save.
    pub fn persist_pipeline_cache(&self) -> Result<()> {
        if let Some(cache) = self
            .disk_cache
            .get()
            .cloned()
            .flatten()
            .or_else(|| self.pipeline_cache())
        {
            cache.persist_if_changed()?;
        }
        Ok(())
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
            .map_err(|_| "GPU prewarm state was poisoned".to_owned())?;
        while slot.is_none() {
            slot = self
                .ready
                .wait(slot)
                .map_err(|_| "GPU prewarm state was poisoned".to_owned())?;
        }
        slot.as_ref()
            .expect("the loop above waits until a prewarm result is present")
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
    /// The band origins bound with the scene-tone uniforms (`work`).
    dispatch_bands: DispatchBands,
    /// Whether this pipeline's stages wait for the GPU between chunks
    /// ([`Self::pace_submissions`]).
    paced_submissions: AtomicBool,
    /// Fences for this graph's submitted processing chunks, oldest first.
    pending_submissions: Mutex<VecDeque<wgpu::SubmissionIndex>>,
    effects_bind_group: wgpu::BindGroup,
    remove_composite_pipeline: wgpu::ComputePipeline,
    remove_composite_bind_group: wgpu::BindGroup,
    remove_composite_params_buffer: wgpu::Buffer,
    uploaded_stage_uniforms: Mutex<UploadedStageUniforms>,
    /// The effects of the active slots of the parameters last uploaded
    /// ([`EffectSet::bits`]), which choose the programs of the passes that
    /// dispatch effects (`effect_programs`).
    uploaded_effects: std::sync::atomic::AtomicU32,
    /// Whether a render waits for the program of its effect set. A preview
    /// takes it once a worker has compiled it, so no edit stalls on a
    /// compile. A full-quality graph, which an export renders every tile
    /// with, compiles it while it is built.
    effect_readiness: Readiness,
    mask_data_buffer: wgpu::Buffer,
    /// The slots at the start of `mask_data_buffer`, as last uploaded.
    uploaded_mask_slots: Mutex<Vec<MaskData>>,
    tone_histogram_buffer: wgpu::Buffer,
    tone_stats_buffer: wgpu::Buffer,
    /// The camera profile's tables (`ProfileGpuData`), and room for more.
    profile_buffer: wgpu::Buffer,
    /// Words `profile_buffer` holds.
    profile_capacity_words: usize,
    /// The camera profile whose tables `profile_buffer` holds.
    uploaded_profile: Mutex<Arc<ProfileColor>>,
    /// Relight's surface, the scene shadow map and the full-image maps, allocated
    /// when an effect first reads them. Cropped and zoomed views copy the
    /// full-image maps from the full frame like the tone statistics.
    effect_inputs: EffectInputs,
    /// Whether each full-image map matches the current tone result, indexed
    /// by `FullImageMap`. The tone stage builds a map only when an effect
    /// reads it; otherwise the output stage builds it once one does
    /// (`ensure_full_image_map`).
    full_image_maps_current: [AtomicBool; FullImageMap::ALL.len()],
    /// Where each processing stage starts and ends in `passes`.
    indices: StageIndices,
    post_blur_glow_passes: Vec<Pass>,
    post_blur_pixelate_blocks_pass: Pass,
    post_blur_render_pass: Pass,
    /// Whether the last output stage left the image in the second work
    /// texture, which the render pass then read; otherwise it read the first
    /// (`post_blur_render_pass`). Point colour samples render from the same one.
    output_image_in_tex2: AtomicBool,
    /// The last preview render of the output stage, which the next starts
    /// after the parts it shares with (`output_parts`). Every method that
    /// writes the work textures outside the output stage, or changes what it
    /// reads other than through its parameters, forgets it.
    output_resume: Mutex<OutputResume>,
    /// Effects whose resources could not be allocated (`unrendered`).
    unrendered: Mutex<UnrenderedEffects>,
    /// This pipeline's tone statistics and full-image maps, built again by
    /// each tone stage; crops that inherit them compare it.
    tone_build: BuildRevision,
    /// The full frame's `tone_build` this pipeline last inherited; 0 before
    /// any.
    inherited_tone: std::sync::atomic::AtomicU64,
    /// The raster finish program reading camera RGB from highlight work A: a
    /// binned source's reconstruction, or an X-Trans photo's AI-denoised scene.
    binned_finish_pass: Pass,
    passes: Vec<Pass>,
    raw_texture: wgpu::Texture,
    color_texture: wgpu::Texture,
    black_texture: wgpu::Texture,
    lens_shading_texture: wgpu::Texture,
    highlight_model_texture: wgpu::Texture,
    /// The highlight model `highlight_model_texture` holds.
    uploaded_highlight_model: Mutex<Option<Arc<HighlightModel>>>,
    _reconstructed_raw_texture: wgpu::Texture,
    /// Highlight reconstruction scratch. Pipelines with an AI-denoised scene
    /// (X-Trans), which skips reconstruction and demosaicing, keep that scene
    /// here for every RAW stage to start from.
    highlight_work_a: wgpu::Texture,
    /// Whether `highlight_work_a` still holds the AI-denoised scene: a RAW
    /// stage that demosaics, with AI denoise off, overwrites it.
    ai_scene_intact: AtomicBool,
    _highlight_work_b: wgpu::Texture,
    _tex1: wgpu::Texture,
    _tex2: wgpu::Texture,
    _xtrans_homogeneity: Option<wgpu::Texture>,
    _xtrans_complete: Option<wgpu::Texture>,
    rgb_source_texture: wgpu::Texture,
    binned_ai_texture: wgpu::Texture,
    binned_clipping_texture: wgpu::Texture,
    scene_texture: wgpu::Texture,
    scene_format: wgpu::TextureFormat,
    has_ai_scene: bool,
    /// The layout of the source the graph was built for; uploaded tiles must
    /// share it.
    layout: PixelLayout,
    /// Whether the RAW stage blends in AI-denoised sensor samples: a Bayer
    /// mosaic's in the raw texture, or a binned view's in its own texture.
    has_ai_cfa: bool,
    display_linear_texture: wgpu::Texture,
    _tone_guide_a: wgpu::Texture,
    _tone_guide_b: wgpu::Texture,
    mask_texture: wgpu::Texture,
    full_image_mask_texture: wgpu::Texture,
    mask_layer_capacity: usize,
    mask_atlas_edge: u32,
    out_texture: wgpu::Texture,
    out_view: wgpu::TextureView,
    pipeline_cache: Option<Arc<PersistentGpuPipelineCache>>,
    setup_times: PipelineSetupTimes,
    _gpu_budget_reservation: GpuBudgetReservation,
}

/// Where [`RawGpuPipeline::new`] spent its time, on the CPU.
#[derive(Clone, Copy, Debug, Default)]
pub struct PipelineSetupTimes {
    /// Textures, buffers, bind group layouts and bind groups.
    pub resources: Duration,
    /// Composing the WGSL and creating the shader modules; none when the
    /// programs come from a template.
    pub shader_modules: Duration,
    /// Compiling the programs these edits use, and the Remove composite.
    /// Programs a template already compiled cost nothing here.
    pub programs: Duration,
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
    /// Prepares what the render tier sizes, for a render of `params` from
    /// `start`: Lens Blur's textures and map when it runs the Scene part,
    /// the scene lights when it runs a part that may read them (the Scene or
    /// the Creative part). A part a render leaves as it is keeps what it was
    /// rendered from, in its own tier (`output_parts`). Call after
    /// `upload_params`.
    fn prepare_tiered_effects(&self, queue: &wgpu::Queue, params: &GpuParams, start: OutputPart) {
        if start == OutputPart::Scene {
            self.prepare_defocus(queue, params);
        }
        if start <= OutputPart::Creative {
            self.prepare_scene_lights(queue, params);
        }
    }

    /// Uploads `params` and returns the uniforms as uploaded, which the
    /// pipeline adjusts to what it holds (`effects_for_upload`,
    /// `scene_tone_for_upload`, `without_highlight_model`). What the render
    /// tier sizes is prepared by `prepare_tiered_effects`.
    fn upload_params(&self, queue: &wgpu::Queue, params: &GpuParams) -> UploadedStageUniforms {
        self.upload_profile_tables(queue, &params.profile_color);
        self.prepare_effect_inputs(params);
        let (styled_glow_near, styled_glow_map) = self.prepare_styled_glow(params);
        let effects = params.effects_for_upload(styled_glow_near, styled_glow_map);
        let look_lut_uploaded = match self.prepare_look_lut(queue, params.look_lut()) {
            Ok(()) => true,
            Err(error) => {
                // Callers that must not render without the LUT report this
                // through `prepare_look_lut` before dispatching.
                log::error!("rendering without the LUT: {error:#}");
                false
            }
        };
        let mut camera = params.camera_for_raw_texture(self.has_ai_cfa);
        if !self.upload_highlight_model(queue, params.highlight_model.as_ref()) {
            camera = GpuParams::without_highlight_model(camera);
        }
        let scene_tone = params.scene_tone_for_upload(look_lut_uploaded);
        match self.uploaded_stage_uniforms.lock() {
            Ok(mut uploaded) => {
                if bytemuck::bytes_of(&uploaded.camera) != bytemuck::bytes_of(&camera) {
                    queue.write_buffer(
                        &self.camera_uniforms_buffer,
                        0,
                        bytemuck::bytes_of(&camera),
                    );
                    uploaded.camera = camera;
                }
                if bytemuck::bytes_of(&uploaded.scene_tone) != bytemuck::bytes_of(&scene_tone) {
                    queue.write_buffer(
                        &self.scene_tone_uniforms_buffer,
                        0,
                        bytemuck::bytes_of(&scene_tone),
                    );
                    uploaded.scene_tone = scene_tone;
                }
                if bytemuck::bytes_of(&uploaded.effects) != bytemuck::bytes_of(&effects) {
                    queue.write_buffer(
                        &self.effects_uniforms_buffer,
                        0,
                        bytemuck::bytes_of(&effects),
                    );
                    uploaded.effects = effects;
                }
            }
            Err(_) => {
                queue.write_buffer(&self.camera_uniforms_buffer, 0, bytemuck::bytes_of(&camera));
                queue.write_buffer(
                    &self.scene_tone_uniforms_buffer,
                    0,
                    bytemuck::bytes_of(&scene_tone),
                );
                queue.write_buffer(
                    &self.effects_uniforms_buffer,
                    0,
                    bytemuck::bytes_of(&effects),
                );
            }
        }
        self.upload_mask_slots(queue, params.mask_slots());
        self.uploaded_effects.store(
            EffectSet::of_slots(params.mask_slots()).bits(),
            Ordering::Release,
        );
        // After the camera uniforms, which tell whether this pipeline renders
        // the whole image.
        self.upload_scene_bake_depth(queue, params);
        UploadedStageUniforms {
            camera,
            scene_tone,
            effects,
        }
    }

    /// Uploads `slots`, the mask slots in use, unless the buffer already
    /// starts with them. The shaders read only the slots in use
    /// (`mask_slot_count` in common.wgsl), so the rest of the table stays as
    /// it was.
    fn upload_mask_slots(&self, queue: &wgpu::Queue, slots: &[MaskData]) {
        if slots.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(slots);
        // The record is replaced in one step below, so a panic elsewhere
        // cannot leave it half written.
        let mut uploaded = self
            .uploaded_mask_slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if uploaded
            .get(..slots.len())
            .is_some_and(|held| bytemuck::cast_slice::<MaskData, u8>(held) == bytes)
        {
            return;
        }
        queue.write_buffer(&self.mask_data_buffer, 0, bytes);
        if uploaded.len() < slots.len() {
            uploaded.resize(slots.len(), MaskData::zeroed());
        }
        uploaded[..slots.len()].copy_from_slice(slots);
    }

    pub fn tone_guide_supports_origin(&self, origin_x: i32, origin_y: i32) -> bool {
        let scale = tone_analysis_scale();
        self.tone_guide_extent
            == [
                tone_guide_axis_cell_count(origin_x, self.width, scale),
                tone_guide_axis_cell_count(origin_y, self.height, scale),
            ]
    }

    /// Where building this pipeline spent its time.
    pub const fn setup_times(&self) -> PipelineSetupTimes {
        self.setup_times
    }

    pub const fn mask_atlas_edge(&self) -> u32 {
        self.mask_atlas_edge
    }

    /// The resident GPU bytes this graph reserved against the process budget.
    pub fn resident_bytes(&self) -> u64 {
        self._gpu_budget_reservation.bytes()
    }

    pub const fn mask_layer_capacity(&self) -> usize {
        self.mask_layer_capacity
    }

    /// The layout of the source the graph was built for, which
    /// [`Self::upload_raw_tile`] requires of every tile.
    pub const fn pixel_layout(&self) -> PixelLayout {
        self.layout
    }

    pub fn immutable_ai_source_matches(&self, cfa_kind: CfaKind, enabled: bool) -> bool {
        match cfa_kind {
            CfaKind::Bayer => self.has_ai_cfa == enabled,
            CfaKind::XTrans => {
                !enabled || (self.has_ai_scene && self.ai_scene_intact.load(Ordering::Acquire))
            }
        }
    }

    /// The final display-referred output (`Rgba8Unorm`, sRGB-encoded values),
    /// which presentation layers may sample, e.g. by registering it as an
    /// egui texture. It stays valid for the pipeline's lifetime.
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.out_view
    }

    /// Copies the output ([`Self::output_view`]) into `destination`, an
    /// `Rgba8Unorm` texture of this pipeline's size made with `COPY_DST`: a
    /// presentation layer that shows only completed renders keeps the copy
    /// while the next render runs. Queued after the work submitted so far.
    pub fn copy_output_to(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        destination: &wgpu::Texture,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw copy output to presentation"),
        });
        encoder.copy_texture_to_texture(
            self.out_texture.as_image_copy(),
            destination.as_image_copy(),
            self.out_texture.size(),
        );
        queue.submit(Some(encoder.finish()));
    }

    pub fn recompute(&self, queue: &wgpu::Queue, device: &wgpu::Device, params: &GpuParams) {
        self.upload_params(queue, params);
        self.prepare_tiered_effects(queue, params, OutputPart::Scene);
        let mut encoder = self.work(device, queue, "calibraw complete recompute encoder");
        let timing = stage_timing::begin(device, || self.timed_stage_name("Recompute"));
        encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
        self.encode_raw_stage(&mut encoder, params);
        self.encode_tone_stage(&mut encoder, params);
        self.encode_output_stage(&mut encoder, params, OutputStart::Scene);
        encoder.submit(timing);
    }

    /// Renders `stage` of `params`. The output stage starts at the first of
    /// its parts that what changed since this pipeline's last output render
    /// reaches (`output_parts`), so a pipeline must be given every stage an
    /// edit affects, as `affected_stage` names them.
    pub fn dispatch_stage(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        stage: ProcessingStage,
    ) {
        let uploaded = self.upload_params(queue, params);
        let mut encoder = self.work(device, queue, stage.label());

        match stage {
            ProcessingStage::Raw | ProcessingStage::Tone => {
                let timing =
                    stage_timing::begin(device, || self.timed_stage_name(&format!("{stage:?}")));
                if stage == ProcessingStage::Raw {
                    self.encode_raw_stage(&mut encoder, params);
                } else {
                    encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
                    self.encode_tone_stage(&mut encoder, params);
                }
                encoder.submit(timing);
            }
            ProcessingStage::Output => {
                let inputs = OutputInputs::new(params, &uploaded, self.shared_map_builds());
                let mut start = self.lock_output_resume().start(&inputs);
                self.prepare_tiered_effects(queue, params, start.part());
                // An allocation whose outcome changed forgets the last
                // render, which then starts earlier and prepares more.
                let restart = self.lock_output_resume().start(&inputs);
                if restart.part() < start.part() {
                    self.prepare_tiered_effects(queue, params, restart.part());
                    start = restart;
                }
                let timing =
                    stage_timing::begin(device, || self.timed_stage_name(start.timing_label()));
                let textures = self.encode_output_stage(&mut encoder, params, start);
                // After encoding, which rebuilt the maps this pipeline owns.
                let inputs = inputs.with_map_builds(self.shared_map_builds());
                self.lock_output_resume().record(inputs, start, textures);
                encoder.submit(timing);
            }
        }
    }

    /// Encodes the tone stage: the tone statistics and guide, and the
    /// full-image maps `params` reads. Every later stage reads them anew.
    fn encode_tone_stage(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        self.forget_output();
        self.tone_build.bump();
        self.encode_pass_range(
            encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_stage_end,
        );
        self.encode_tone_full_image_maps(encoder, params);
    }

    fn lock_output_resume(&self) -> std::sync::MutexGuard<'_, OutputResume> {
        // The state is replaced whole by each method, so a panic elsewhere
        // cannot leave it half written.
        self.output_resume
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Forgets the last output render, whose results something other than
    /// the output stage overwrote or outdated: the next starts at the Scene
    /// part.
    fn forget_output(&self) {
        self.lock_output_resume().forget();
    }

    /// Notes an upload of mask `layer` (`mask_layers`), which the next output
    /// render reads from the first part that reads it.
    fn note_mask_layer_upload(&self, layer: usize) {
        self.lock_output_resume().note_mask_layer(layer);
    }

    /// The builds of the Lens Blur and Glow maps this pipeline reads.
    fn shared_map_builds(&self) -> SharedMapBuilds {
        SharedMapBuilds {
            defocus: self.effect_inputs.defocus_map_build(),
            styled_glow: self.effect_inputs.styled_glow_map_build(),
        }
    }

    /// How `stage_timing` names a stage of this pipeline: "Output 652x978".
    fn timed_stage_name(&self, stage: &str) -> String {
        format!("{stage} {}x{}", self.width, self.height)
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
        self.forget_output();
        anyhow::ensure!(
            raw.pixel_layout() == self.layout,
            "reusable {:?} pipeline received a {:?} tile",
            self.layout,
            raw.pixel_layout()
        );

        if let Some(rgb) = rgb_source_pixels(raw) {
            upload_rgb_texture(queue, &self.rgb_source_texture, self.scene_format, raw, rgb)?;
        }
        match self.layout {
            PixelLayout::Raster => return Ok(()),
            PixelLayout::Binned => {
                // Without clipped photosites the texture keeps the last
                // view's content, which `highlight_options.w` disables.
                if let Some(clipping) = binned_clipping(raw) {
                    upload_binned_clipping_texture(
                        queue,
                        &self.binned_clipping_texture,
                        raw,
                        clipping,
                    )?;
                }
                if self.has_ai_cfa {
                    let ai_rgb = raw
                        .binned_sensor
                        .as_ref()
                        .and_then(BinnedSensor::ai_rgb)
                        .context(
                            "reusable AI-denoise pipeline received a tile without denoised RGB",
                        )?;
                    upload_rgb_texture(
                        queue,
                        &self.binned_ai_texture,
                        self.scene_format,
                        raw,
                        ai_rgb,
                    )?;
                }
            }
            PixelLayout::Mosaic => {
                let ai_image = raw.ai_denoised_image();
                let ai_cfa = if self.has_ai_cfa {
                    Some(
                        ai_image
                            .as_ref()
                            .and_then(AiDenoisedImage::bayer_cfa)
                            .context(
                                "reusable AI-denoise pipeline received a tile without denoised CFA",
                            )?,
                    )
                } else {
                    None
                };
                upload_raw_texture(queue, &self.raw_texture, raw, ai_cfa)?;
                upload_color_texture(queue, &self.color_texture, raw);
                upload_black_texture(queue, &self.black_texture, raw);
            }
        }
        upload_lens_shading_texture(queue, &self.lens_shading_texture, raw);
        if self.has_ai_scene {
            anyhow::ensure!(
                upload_ai_scene_texture(queue, &self.highlight_work_a, self.scene_format, raw)?,
                "reusable AI-denoise pipeline received a tile without derived model output"
            );
            self.ai_scene_intact.store(true, Ordering::Release);
        }
        Ok(())
    }

    pub fn begin_export_tone_analysis(&self, queue: &wgpu::Queue, device: &wgpu::Device) {
        self.forget_output();
        let mut encoder = self.work(device, queue, "calibraw export tone histogram clear");
        encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
        for map in FullImageMap::ALL {
            if let Some(resources) = self.effect_inputs.full_image_map(map) {
                encoder.clear_buffer(&resources.cells, 0, None);
            }
        }
        if let Some(bake) = self.effect_inputs.scene_bake() {
            bake.outdate_scene();
        }
        encoder.submit(None);
        for map in FullImageMap::ALL {
            self.full_image_map_current(map)
                .store(false, Ordering::Release);
        }
    }

    /// Uploads `lut` for the look LUT pass unless this pipeline already holds
    /// it. The texture is allocated, and reserved against the process GPU
    /// budget, only when a LUT is first uploaded. Parameters with a LUT
    /// upload it themselves; call this first to learn whether it fits.
    pub fn prepare_look_lut(&self, queue: &wgpu::Queue, lut: Option<&CubeLut>) -> Result<()> {
        match lut {
            Some(lut) => self.effect_inputs.upload_look_lut(queue, lut),
            None => Ok(()),
        }
    }

    /// Whether the tables of `color`, a camera profile of this graph's photo,
    /// fit its profile buffer, so parameters with it render correctly.
    pub fn holds_profile_tables(&self, color: &ProfileColor) -> bool {
        color.camera_profile.gpu_data().words.len() <= self.profile_capacity_words
    }

    /// Uploads the tables of `color` unless the profile buffer holds them.
    /// Tables that do not fit are not uploaded; callers check
    /// [`Self::holds_profile_tables`] before rendering another profile.
    fn upload_profile_tables(&self, queue: &wgpu::Queue, color: &Arc<ProfileColor>) {
        let Ok(mut uploaded) = self.uploaded_profile.lock() else {
            return;
        };
        if Arc::ptr_eq(&uploaded, color) {
            return;
        }
        let data = color.camera_profile.gpu_data();
        if let Err(error) = data.validate() {
            log::error!("keeping the previous camera profile tables: {error:#}");
            return;
        }
        if data.words.len() > self.profile_capacity_words {
            log::error!(
                "camera profile tables of {} words exceed the graph's {}",
                data.words.len(),
                self.profile_capacity_words
            );
            return;
        }
        queue.write_buffer(&self.profile_buffer, 0, bytemuck::cast_slice(&data.words));
        *uploaded = Arc::clone(color);
    }

    /// Uploads `model` unless the pipeline holds it already. Returns whether the highlight
    /// pass may read it: false without a model, and for one that does not fit the texture.
    fn upload_highlight_model(
        &self,
        queue: &wgpu::Queue,
        model: Option<&Arc<HighlightModel>>,
    ) -> bool {
        let Some(model) = model else {
            return false;
        };
        let Ok(mut uploaded) = self.uploaded_highlight_model.lock() else {
            return false;
        };
        if uploaded
            .as_ref()
            .is_some_and(|uploaded| Arc::ptr_eq(uploaded, model))
        {
            return true;
        }
        if !upload_highlight_model_texture(queue, &self.highlight_model_texture, model) {
            log::error!(
                "highlight model of {:?} cells does not fit the pipeline's {}x{} texture",
                model.cells(),
                self.highlight_model_texture.width(),
                self.highlight_model_texture.height()
            );
            return false;
        }
        *uploaded = Some(Arc::clone(model));
        true
    }

    /// Allocates the effect inputs `params` reads (`effect_inputs`).
    fn prepare_effect_inputs(&self, params: &GpuParams) {
        if params.needs_scene_emission() {
            self.effect_inputs.allocate_scene_emission();
        }
        for map in FullImageMap::ALL {
            if params.needs_full_image_map(map) {
                self.effect_inputs.allocate_full_image_map(map);
            }
        }
    }

    fn full_image_map_current(&self, map: FullImageMap) -> &AtomicBool {
        &self.full_image_maps_current[map as usize]
    }

    /// Builds `map` from this pipeline's whole image, after the tone
    /// statistics it is resolved against.
    fn encode_full_image_map(&self, encoder: &mut WorkEncoder<'_>, map: FullImageMap) {
        self.effect_inputs.allocate_full_image_map(map);
        if let Some(resources) = self.effect_inputs.full_image_map(map) {
            encoder.clear_buffer(&resources.cells, 0, None);
        }
        let passes = self.indices.full_image_map_passes(map);
        self.encode_pass(encoder, passes.accumulate);
        self.encode_pass_range(encoder, passes.resolve.start, passes.resolve.end);
        self.full_image_map_current(map)
            .store(true, Ordering::Release);
    }

    /// Full-image maps for a new tone result: built when an effect reads
    /// them, otherwise left for `ensure_full_image_map`, which most edits
    /// never need.
    fn encode_tone_full_image_maps(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        self.outdate_scene_bake();
        for map in FullImageMap::ALL {
            if params.needs_full_image_map(map) {
                self.encode_full_image_map(encoder, map);
            } else {
                self.full_image_map_current(map)
                    .store(false, Ordering::Release);
            }
        }
    }

    /// Builds `map` if the current tone result has none yet. The scene and
    /// tone statistics it reads stay valid until the next raw or tone stage.
    fn ensure_full_image_map(&self, encoder: &mut WorkEncoder<'_>, map: FullImageMap) {
        if !self.full_image_map_current(map).load(Ordering::Acquire) {
            self.encode_full_image_map(encoder, map);
        }
    }

    /// Takes the full frame's maps that `params` reads, so a cropped or
    /// zoomed view sees the lights and haze outside its region.
    fn inherit_full_image_maps(
        &self,
        encoder: &mut WorkEncoder<'_>,
        full_frame: &Self,
        params: &GpuParams,
    ) {
        for map in FullImageMap::ALL {
            if !params.needs_full_image_map(map) {
                self.full_image_map_current(map)
                    .store(false, Ordering::Release);
                continue;
            }
            full_frame.ensure_full_image_map(encoder, map);
            self.effect_inputs.allocate_full_image_map(map);
            let (Some(source), Some(destination)) = (
                full_frame.effect_inputs.full_image_map(map),
                self.effect_inputs.full_image_map(map),
            ) else {
                continue;
            };
            for (source, destination) in source.textures.iter().zip(&destination.textures) {
                encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    destination.as_image_copy(),
                    destination.size(),
                );
            }
            for (source, destination) in source.buffers.iter().zip(&destination.buffers) {
                encoder.copy_buffer_to_buffer(source, 0, destination, 0, destination.size());
            }
            self.full_image_map_current(map)
                .store(true, Ordering::Release);
        }
        self.inherit_scene_bake(encoder, full_frame, params);
        // The full frame builds its Glow map in its output stage whenever a
        // Glow is active, whatever its spread.
        if params.styled_glow_slots() > 0 {
            if let Some(map) = full_frame.effect_inputs.styled_glow_map() {
                self.effect_inputs.share_styled_glow_map(&map);
            }
        }
        // And its Lens Blur map, whenever a Lens Blur reaches it.
        if params.needs_defocus_map() {
            if let Some(map) = full_frame.effect_inputs.defocus_map() {
                self.effect_inputs.share_defocus_map(&map);
            }
        }
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
        self.inherit_full_frame_tone(queue, device, params, full_frame);
        self.dispatch_inherited_tone_guide(queue, device);
    }

    /// The first half of [`Self::dispatch_tone_guide_with_inherited_statistics`]:
    /// copies `full_frame`'s tone statistics and full-image maps into this
    /// crop, building a map `full_frame` lacks. It reads and may encode work on
    /// `full_frame`, so it runs on the thread that renders `full_frame`. Call
    /// after this crop's RAW stage, which writes none of what it copies.
    pub fn inherit_full_frame_tone(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        full_frame: &Self,
    ) {
        self.upload_params(queue, params);
        self.forget_output();
        self.inherited_tone
            .store(full_frame.tone_build.get(), Ordering::Release);
        let mut encoder = self.work(
            device,
            queue,
            "calibraw crop inherits full-frame statistics",
        );
        encoder.clear_buffer(&self.tone_histogram_buffer, 0, None);
        encoder.copy_buffer_to_buffer(
            &full_frame.tone_stats_buffer,
            0,
            &self.tone_stats_buffer,
            0,
            TONE_STATS_SIZE_BYTES,
        );
        self.inherit_full_image_maps(&mut encoder, full_frame, params);
        encoder.submit(None);
    }

    /// The second half of [`Self::dispatch_tone_guide_with_inherited_statistics`]:
    /// the tone guide passes, which read what [`Self::inherit_full_frame_tone`]
    /// copied and the parameters it uploaded. Call it next, with nothing
    /// encoded on this crop in between; it uploads nothing, because uploading
    /// again after the inheritance could upload other Glow inputs than the
    /// passes read when they ran in one go. Touches only this crop, so a
    /// worker thread may run it.
    pub fn dispatch_inherited_tone_guide(&self, queue: &wgpu::Queue, device: &wgpu::Device) {
        let mut encoder = self.work(
            device,
            queue,
            "calibraw crop tone guide with full-frame statistics",
        );
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        encoder.submit(None);
    }

    pub fn inherit_tone_statistics(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
        full_frame: &Self,
    ) {
        // New scene depth outdates the scene map, so it is uploaded before
        // the map is inherited rather than after.
        self.upload_params(queue, params);
        // Statistics and maps of a tone stage the full frame rendered since
        // the last inheritance change what every part of the output reads.
        let tone = full_frame.tone_build.get();
        if self.inherited_tone.swap(tone, Ordering::AcqRel) != tone {
            self.forget_output();
        }
        let mut encoder = self.work(device, queue, "calibraw inherit full-frame tone statistics");
        encoder.copy_buffer_to_buffer(
            &full_frame.tone_stats_buffer,
            0,
            &self.tone_stats_buffer,
            0,
            TONE_STATS_SIZE_BYTES,
        );
        self.inherit_full_image_maps(&mut encoder, full_frame, params);
        encoder.submit(None);
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
        let mut encoder = self.work(device, queue, "calibraw Remove-aware export tone tile");
        let timing = stage_timing::begin(device, || self.timed_stage_name("Export tone"));
        self.encode_pass(&mut encoder, self.indices.tone_prepare_pass_index);
        // Each tile adds its core to the shared full-image maps. Without a
        // receiver a map stays empty and the tiles skip reading it all again.
        for map in FullImageMap::ALL {
            if params.needs_full_image_map(map) {
                self.effect_inputs.allocate_full_image_map(map);
                self.encode_pass(
                    &mut encoder,
                    self.indices.full_image_map_passes(map).accumulate,
                );
            }
        }
        if params.needs_scene_bake() {
            if let Some(bake) = self.own_scene_bake(queue, params) {
                self.accumulate_scene_working_image(&mut encoder, &bake);
            }
        }
        encoder.submit(timing);
        Ok(())
    }

    pub fn finish_export_tone_analysis(&self, queue: &wgpu::Queue, device: &wgpu::Device) {
        self.forget_output();
        self.tone_build.bump();
        let mut encoder = self.work(device, queue, "calibraw export tone histogram reduction");
        let timing = stage_timing::begin(device, || self.timed_stage_name("Export tone reduce"));
        self.encode_pass(&mut encoder, self.indices.tone_reduce_pass_index);
        // Tiles accumulated a map only if an effect reads it, which allocated
        // its grid.
        let allocated =
            FullImageMap::ALL.map(|map| self.effect_inputs.full_image_map(map).is_some());
        for (map, allocated) in FullImageMap::ALL.into_iter().zip(allocated) {
            if allocated {
                let resolve = self.indices.full_image_map_passes(map).resolve;
                self.encode_pass_range(&mut encoder, resolve.start, resolve.end);
            }
        }
        // Tiles accumulated the bake's working image only if an effect reads
        // the bake, which allocated it.
        if let Some(bake) = self.effect_inputs.scene_bake() {
            self.encode_scene_bake(&mut encoder, &bake);
        }
        encoder.submit(timing);
        // Resolved from every tile's accumulation; tiles must not rebuild them
        // from their own region.
        for (map, allocated) in FullImageMap::ALL.into_iter().zip(allocated) {
            self.full_image_map_current(map)
                .store(allocated, Ordering::Release);
        }
    }

    /// Bytes of one scene texel, in which [`Self::begin_scene_region_readback`]
    /// reads and [`Self::write_scene_region`] writes.
    pub fn scene_texel_bytes(&self) -> u32 {
        self.scene_format.block_copy_size(None).unwrap_or(16)
    }

    /// Writes scene texels a RAW stage of this export left
    /// ([`Self::begin_scene_region_readback`]) into the region at `origin`
    /// (pixels of this graph) of `extent`, so that
    /// [`Self::dispatch_export_tile_from_scene`] renders the tile without its
    /// RAW stage. `texels` holds the region's rows, each starting
    /// `bytes_per_row` bytes after the previous.
    pub fn write_scene_region(
        &self,
        queue: &wgpu::Queue,
        texels: &[u8],
        bytes_per_row: u32,
        origin: [u32; 2],
        extent: [u32; 2],
    ) -> Result<()> {
        let [width, height] = extent;
        anyhow::ensure!(
            width > 0
                && height > 0
                && origin[0]
                    .checked_add(width)
                    .is_some_and(|right| right <= self.width)
                && origin[1]
                    .checked_add(height)
                    .is_some_and(|bottom| bottom <= self.height),
            "scene region {width}x{height} at {origin:?} lies outside the {}x{} graph",
            self.width,
            self.height
        );
        self.forget_output();
        let row_bytes = u64::from(width) * u64::from(self.scene_texel_bytes());
        let required = u64::from(bytes_per_row) * u64::from(height - 1) + row_bytes;
        anyhow::ensure!(
            u64::from(bytes_per_row) >= row_bytes && texels.len() as u64 >= required,
            "scene region texels are shorter than {width}x{height}"
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.scene_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Renders the tone and output stages of an export tile whose scene the
    /// RAW stage, or [`Self::write_scene_region`], already holds.
    pub fn dispatch_export_tile_from_scene(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
    ) {
        self.upload_params(queue, params);
        self.prepare_tiered_effects(queue, params, OutputPart::Scene);
        let mut encoder = self.work(device, queue, "calibraw Remove-aware tiled export encoder");
        let timing = stage_timing::begin(device, || self.timed_stage_name("Export tile"));
        self.forget_output();
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        self.encode_output_stage(&mut encoder, params, OutputStart::Scene);
        encoder.submit(timing);
    }

    fn encode_raw_stage(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        self.forget_output();
        if self.layout == PixelLayout::Raster {
            self.encode_pass(encoder, self.indices.raster_finish_pass_index);
            self.encode_chromatic_aberration(encoder, params);
            self.encode_color_denoise(encoder, params);
            return;
        }
        if self.has_ai_scene && params.uses_ai_denoise() {
            // A source overwritten by demosaicing leaves the scene as it is;
            // `immutable_ai_source_matches` asks for a new pipeline then.
            if self.ai_scene_intact.load(Ordering::Acquire) {
                // The model's demosaiced camera RGB in highlight work A takes
                // the binned finish: the profiled luminance denoise on the way
                // into the scene, a plain copy at zero. It only reads
                // highlight work A, which stays intact for the next render.
                self.encode_bound_pass(encoder, &self.binned_finish_pass, "AI scene finish");
                self.encode_chromatic_aberration(encoder, params);
                self.encode_color_denoise(encoder, params);
            }
            return;
        }
        // Highlight reconstruction and demosaicing write highlight work A.
        self.ai_scene_intact.store(false, Ordering::Release);
        if self.layout == PixelLayout::Binned {
            self.encode_pass(encoder, self.indices.binned_highlight_pass_index);
            self.encode_bound_pass(encoder, &self.binned_finish_pass, "binned finish");
        } else {
            self.encode_demosaic(encoder, params);
        }
        self.encode_chromatic_aberration(encoder, params);
        self.encode_color_denoise(encoder, params);
    }

    /// Reconstructs a mosaic's highlights and demosaics it into the scene.
    fn encode_demosaic(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        self.encode_pass(encoder, self.indices.highlight_pass_index);
        self.encode_pass_range(
            encoder,
            self.indices.demosaic_start_index,
            self.indices.demosaic_dual_start_index,
        );
        if matches!(self.cfa_kind, CfaKind::Bayer) && stores_rcd_reference(self.scene_format) {
            self.encode_pass(encoder, self.indices.rcd_reference_pass_index);
        }
        if params.needs_dual_demosaic_passes() {
            self.encode_pass_range(
                encoder,
                self.indices.demosaic_dual_start_index,
                self.indices.demosaic_dual_end_index,
            );
        }
        let finish = &self.passes[self.indices.demosaic_finish_index];
        self.dispatch_pass(
            encoder,
            "calibraw demosaic finish",
            finish.pipeline.for_demosaic_params(&params.camera),
            &self.pass_bind_group(finish),
            finish.workgroups,
            finish.pipeline.work,
            demosaic_variant_for(&params.camera) as u64,
        );
    }

    /// Moves red and blue onto green and removes colour fringes in the scene
    /// when Chromatic Aberration is on.
    fn encode_chromatic_aberration(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        if params.needs_chromatic_aberration_passes() {
            self.encode_pass(
                encoder,
                self.indices.chromatic_aberration_prepare_pass_index,
            );
            self.encode_pass(encoder, self.indices.chromatic_aberration_fringe_pass_index);
        }
    }

    /// Filters the scene's colour noise in place when Color denoise is on.
    fn encode_color_denoise(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        if params.camera.chroma_denoise > 1e-6 {
            self.encode_pass_range(
                encoder,
                self.indices.color_denoise_start_index,
                self.indices.color_denoise_end_index,
            );
        }
    }

    /// Encodes the output stage from `start` ([`OutputStart`]): the parts
    /// before it are left as the last render made them. Returns where each
    /// part's input is afterwards.
    fn encode_output_stage(
        &self,
        encoder: &mut WorkEncoder<'_>,
        params: &GpuParams,
        start: OutputStart,
    ) -> OutputTextures {
        self.output_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.ensure_output_inputs(encoder, params);
        let textures = match start {
            OutputStart::View {
                input_in_tex2,
                creative_input_in_tex2,
            } => OutputTextures {
                creative_input_in_tex2,
                view_input_in_tex2: input_in_tex2,
            },
            OutputStart::Scene | OutputStart::Creative { .. } => {
                if params.needs_scene_emission() {
                    // Rebuilt for every render that reaches the Creative part:
                    // it follows the emitters' settings and masks, which change
                    // without a new tone result. Wet Surfaces, in the Scene
                    // part, reads it too.
                    let start = self.indices.scene_emission_pass_index;
                    self.encode_pass_range(encoder, start, start + SCENE_EMISSION_PASSES);
                }
                let creative_input_in_tex2 = match start {
                    OutputStart::Creative { input_in_tex2 } => input_in_tex2,
                    _ => self.encode_creative_input(encoder, params),
                };
                self.encode_creative_part(encoder, params, creative_input_in_tex2)
            }
        };
        self.output_image_in_tex2
            .store(textures.view_input_in_tex2, Ordering::Release);
        self.encode_render_pass(encoder, textures.view_input_in_tex2);
        textures
    }

    /// The Creative part of the output stage from the creative input, in the
    /// second work texture when `creative_input_in_tex2`: the Glow passes,
    /// Pixelate's blocks, the creative pass and the pass after it. Each pass
    /// that transforms the image reads it from one work texture and writes
    /// the other; the Glow passes write the other and display_linear.
    fn encode_creative_part(
        &self,
        encoder: &mut WorkEncoder<'_>,
        params: &GpuParams,
        creative_input_in_tex2: bool,
    ) -> OutputTextures {
        let mut image_in_tex2 = creative_input_in_tex2;
        let mut creative_input_intact = true;
        if params.needs_intermediate_adjustment_passes() {
            if params.styled_glow_slots() > 0 {
                self.encode_styled_glow(encoder, params, image_in_tex2);
            }
            if params.needs_glow_passes() {
                if image_in_tex2 {
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
                if image_in_tex2 {
                    self.encode_bound_pass(
                        encoder,
                        &self.post_blur_pixelate_blocks_pass,
                        "post-Blur Pixelate block pass",
                    );
                } else {
                    self.encode_pass(encoder, self.indices.pixelate_blocks_pass_index);
                }
            }
            for (needed, index) in [
                (
                    params.needs_creative_pass(),
                    self.indices.adjustment_creative_pass_index,
                ),
                (
                    params.needs_weather_air_pass(),
                    self.indices.weather_air_pass_index,
                ),
            ] {
                if needed {
                    self.encode_effects_pass(encoder, index, &mut image_in_tex2);
                    // The pass wrote the texture the image is now in.
                    creative_input_intact &= image_in_tex2 != creative_input_in_tex2;
                }
            }
        }
        OutputTextures {
            creative_input_in_tex2: creative_input_intact.then_some(creative_input_in_tex2),
            view_input_in_tex2: image_in_tex2,
        }
    }

    /// The full-image maps and the scene bake the output stage reads.
    fn ensure_output_inputs(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) {
        for map in FullImageMap::ALL {
            if params.needs_full_image_map(map) {
                self.ensure_full_image_map(encoder, map);
            }
        }
        self.ensure_scene_bake(encoder, params);
    }

    /// The output stage up to the creative pass's input: the scene
    /// adjustments, and with intermediate passes the scene effects, the blurs
    /// and Wet Surfaces. Returns whether the image ends in the second work
    /// texture rather than the first.
    fn encode_creative_input(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) -> bool {
        let mut image_in_tex2 = self.encode_scene_adjustments(encoder, params);
        if !params.needs_intermediate_adjustment_passes() {
            return image_in_tex2;
        }
        if params.needs_blur_passes() {
            self.encode_blur_stage_input(encoder, image_in_tex2);
            if params.defocus_slots() > 0 {
                self.encode_defocus(encoder, params);
            }
            if params.needs_progressive_blur_passes() {
                self.encode_pass_range(
                    encoder,
                    self.indices.mask_blur_start_index,
                    self.indices.mask_blur_end_index,
                );
            } else {
                self.encode_pass(encoder, self.indices.mask_blur_start_index);
            }
            image_in_tex2 = true;
        }
        if params.needs_wet_surfaces_pass() {
            self.encode_effects_pass(
                encoder,
                self.indices.wet_surfaces_pass_index,
                &mut image_in_tex2,
            );
        }
        image_in_tex2
    }

    /// The output stage up to the scene effects: scene preparation and tone
    /// into the second work texture, then only the passes `params` give work
    /// to. The mask tone pass moves the image to the first work texture and
    /// the scene effects pass, which reads it there, back to the second. A
    /// pass left out would have copied its input unchanged, so the image is
    /// the same wherever it ends. Returns whether that is the second work
    /// texture.
    fn encode_scene_adjustments(&self, encoder: &mut WorkEncoder<'_>, params: &GpuParams) -> bool {
        self.encode_pass(encoder, self.indices.adjustment_prepare_pass_index);
        self.encode_pass(encoder, self.indices.adjustment_tone_pass_index);
        if !params.needs_intermediate_adjustment_passes() {
            return true;
        }
        let scene_effects = params.needs_scene_effects_pass();
        if !scene_effects && !params.needs_local_tone_pass() {
            return true;
        }
        // Without mask tone it copies, which the scene effects pass needs:
        // it and the detail pyramid read the first work texture.
        self.encode_pass(encoder, self.indices.adjustment_local_tone_pass_index);
        if !scene_effects {
            return false;
        }
        if let Some(levels) = params.detail_pyramid_levels() {
            let start = self.indices.detail_pyramid_start_index;
            self.encode_pass_range(encoder, start, start + levels);
        }
        self.encode_pass(encoder, self.indices.adjustment_effects_pass_index);
        true
    }

    /// Puts the image in the first work texture, where the first Blur stage
    /// and Lens Blur's passes read it.
    fn encode_blur_stage_input(&self, encoder: &mut WorkEncoder<'_>, image_in_tex2: bool) {
        if image_in_tex2 {
            self.encode_pass(encoder, self.indices.adjustment_effects_copy_pass_index);
        }
    }

    /// Encodes the display transform from the work texture holding the image.
    fn encode_render_pass(&self, encoder: &mut WorkEncoder<'_>, image_in_tex2: bool) {
        if image_in_tex2 {
            self.encode_pass(encoder, self.indices.adjustment_render_pass_index);
        } else {
            self.encode_bound_pass(
                encoder,
                &self.post_blur_render_pass,
                "post-Blur render pass",
            );
        }
    }

    /// Encodes the pass of effects at `index` from the work texture holding the
    /// image into the other.
    fn encode_effects_pass(
        &self,
        encoder: &mut WorkEncoder<'_>,
        index: usize,
        image_in_tex2: &mut bool,
    ) {
        let pass = &self.passes[index];
        let input = if *image_in_tex2 {
            InputBoundPass::EffectsFromTex2
        } else {
            InputBoundPass::EffectsFromTex1
        };
        self.dispatch_program(
            encoder,
            pass,
            &pass.pipeline.entry,
            &self.effect_inputs.bind_group(input),
        );
        *image_in_tex2 = !*image_in_tex2;
    }

    fn encode_pass(&self, encoder: &mut WorkEncoder<'_>, index: usize) {
        let pass = &self.passes[index];
        self.encode_bound_pass(encoder, pass, &pass.pipeline.entry);
    }

    fn encode_bound_pass(&self, encoder: &mut WorkEncoder<'_>, pass_record: &Pass, label: &str) {
        let bind_group = self.pass_bind_group(pass_record);
        self.dispatch_program(encoder, pass_record, label, &bind_group);
    }

    /// Encodes `pass` over `bind_group` with the program for the effects
    /// last uploaded ([`Self::effect_program`]).
    fn dispatch_program(
        &self,
        encoder: &mut WorkEncoder<'_>,
        pass: &Pass,
        label: &str,
        bind_group: &wgpu::BindGroup,
    ) {
        let effect_program = self.effect_program(pass);
        // Each program has its own cost: 1 + the effects it holds, 0 for the
        // program with every effect.
        let variant = effect_program
            .as_ref()
            .map_or(0, |(_, held)| 1 + u64::from(held.bits()));
        self.dispatch_pass(
            encoder,
            label,
            effect_program
                .as_ref()
                .map_or_else(|| pass.pipeline.get(), |(program, _)| program),
            bind_group,
            pass.workgroups,
            pass.pipeline.work,
            variant,
        );
    }

    /// Encodes one pass of this pipeline: `bind_group` in group 0, the
    /// scene-tone uniforms with the dispatch band in 1, the effects uniforms
    /// in 2.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_pass(
        &self,
        encoder: &mut WorkEncoder<'_>,
        label: &str,
        pipeline: &wgpu::ComputePipeline,
        bind_group: &wgpu::BindGroup,
        workgroups: [u32; 3],
        work: PassWork,
        variant: u64,
    ) {
        encoder.dispatch(PassDispatch {
            label,
            pipeline,
            bind_groups: [
                bind_group,
                &self.scene_tone_bind_group,
                &self.effects_bind_group,
            ],
            workgroups,
            bands: &self.dispatch_bands,
            work,
            variant,
        });
    }

    /// A stage's work, whose chunks go to `queue` as they close and the last
    /// one with `WorkEncoder::submit`.
    fn work<'a>(
        &'a self,
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        label: &'static str,
    ) -> WorkEncoder<'a> {
        WorkEncoder::new(
            device,
            queue,
            label,
            &self.pending_submissions,
            self.paced_submissions.load(Ordering::Acquire),
        )
    }

    /// Makes this pipeline's stages wait for the GPU whenever a tenth of a
    /// second of their work is outstanding, so that the UI's frames reach
    /// the GPU in between. For pipelines rendered on a worker thread (an
    /// export's tiles); the UI thread must not wait.
    pub fn pace_submissions(&self, paced: bool) {
        self.paced_submissions.store(paced, Ordering::Release);
    }

    /// The program of `pass` that holds only the effects of the parameters
    /// last uploaded, for a pass that dispatches effects, with the effects it
    /// holds; `None` to encode the pass's own program, which holds every
    /// effect (`effect_programs`). Both compute the same image.
    fn effect_program(&self, pass: &Pass) -> Option<(wgpu::ComputePipeline, EffectSet)> {
        let effects = pass.pipeline.effects.as_ref()?;
        let set = EffectSet::from_bits(self.uploaded_effects.load(Ordering::Acquire));
        // The asynchronous path needs an already usable fallback. Compiling
        // every effect just to cover the first specialized compile has crashed
        // native drivers and blocks the preview longer than the smaller program.
        let readiness = if pass.pipeline.compiled.get().is_some() {
            self.effect_readiness
        } else {
            Readiness::Wait
        };
        effects.program(set, readiness)
    }

    fn pass_bind_group(&self, pass_record: &Pass) -> wgpu::BindGroup {
        match &pass_record.bindings {
            PassBindings::Fixed(bind_group) => bind_group.clone(),
            PassBindings::Inputs(pass) => self.effect_inputs.bind_group(*pass),
        }
    }

    fn encode_pass_range(&self, encoder: &mut WorkEncoder<'_>, start: usize, end: usize) {
        for index in start..end {
            self.encode_pass(encoder, index);
        }
    }
}
