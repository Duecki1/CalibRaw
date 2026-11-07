//! The scene bake (docs/rework/DEPTH_EFFECTS_PLAN.md §4.3): what depth effects
//! read of the scene, built once per depth result and unedited scene, so that
//! each pixel can be shaded as up to two surfaces with their own depth,
//! normal and colour instead of one blended depth.
//!
//! Layout and passes are documented in `shaders/scene/bake_common.wgsl`. Here:
//! - [`SceneBake`] owns the textures, the scratch buffer, the uniforms and the
//!   uploaded depth result, and knows whether they are current;
//! - `bake.rs` compiles the passes once per device and encodes them;
//! - `camera.rs` and `ground.rs` prepare the uniforms (B8 runs on the CPU).
//!
//! Ownership. A pipeline that renders the whole image (the fitted preview, an
//! export) owns its bake and fills its working image itself: the preview from
//! its whole frame, an export from every tile's core during the tone
//! pre-pass. The working-image accumulation is a pass of the processing
//! pipeline (`accumulate_scene_working_image`), as it reads the pipeline's
//! scene source; everything after it belongs to the bake. The bake is held in
//! an `Arc` so that views of part of the image can share the whole image's.

mod bake;
pub(super) mod camera;
mod ground;
pub(super) mod lights;

use super::{
    create_gpu_buffer, create_initialized_buffer, create_processing_texture_array,
    gpu_working_set_limit_bytes, texture_size, GpuBudgetReservation,
};
use anyhow::Result;
use calibraw_core::pipeline::{DisparityMap, MaskImage, SceneCamera, SceneDepth};
use camera::SceneCameraUniform;
use std::sync::{Arc, Mutex};

pub(super) use bake::SceneBakePrograms;
pub use lights::SceneLightQuality;
pub(super) use lights::{
    air_light_layers, SceneAirLightKey, SceneAirLightRegion, SceneAirLightTarget,
    SceneAirLightUniform, SceneLightKind, SceneLightPrograms, SceneLightSet, SceneShadowLight,
    SceneShadowLightsUniform, AIR_LIGHT_FORMAT, SCENE_SHADOW_LIGHTS,
};

/// The largest working size's longer edge, which the accumulation pass's
/// dispatch covers.
pub(in crate::pipeline::gpu) const SCENE_WORKING_LONG_EDGE_MAX: u32 = 1540;

/// The working size's longer edge: the depth model's own (Distill at 1540),
/// or less where the GPU budget is under [`FULL_BAKE_BUDGET`] (§10 of the
/// plan; the bake needs about 110 MiB at 1540 and 60 MiB at 1152).
fn working_long_edge() -> u32 {
    if gpu_working_set_limit_bytes() >= FULL_BAKE_BUDGET {
        SCENE_WORKING_LONG_EDGE_MAX
    } else {
        1152
    }
}

const FULL_BAKE_BUDGET: u64 = 1 << 30;

/// The longer edge of a bake that holds only the working image, for effects
/// that read the scene's colour with a wide footprint (Rain's surroundings)
/// on a photo without a depth result.
const IMAGE_ONLY_LONG_EDGE: u32 = 640;

/// Layers of the bake array (`SCENE_LAYER_*` in bake_common.wgsl).
pub(super) const SCENE_BAKE_LAYERS: u32 = 5;
const SCENE_BAKE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The lens blur buffer (`SceneFocus` in bake_common.wgsl): the fit, then
/// `SCENE_FOCUS_DEPTH_BINS × SCENE_FOCUS_BLUR_BINS` u32 counts.
pub(super) const SCENE_FOCUS_DEPTH_BINS: u32 = 32;
pub(super) const SCENE_FOCUS_BLUR_BINS: u32 = 24;
pub(super) const SCENE_FOCUS_BYTES: u64 =
    16 + 4 * (SCENE_FOCUS_DEPTH_BINS * SCENE_FOCUS_BLUR_BINS) as u64;
/// Levels of the layers' push-pull pyramid (`SCENE_LAYER_LEVELS`).
pub(super) const SCENE_LAYER_LEVELS: u32 = 5;
/// Bytes of the scratch buffer per working texel: one vec4.
const SCRATCH_TEXEL_BYTES: u64 = 16;

/// `SceneBakeUniforms` in bake_common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct SceneBakeUniforms {
    pub(super) size: [u32; 4],
    pub(super) input_size: [u32; 4],
    pub(super) camera: SceneCameraUniform,
    pub(super) ground_plane: [f32; 4],
    pub(super) ground_horizon: [f32; 4],
    /// The lens blur's fit, copied from the buffer after B7 (`focus_fit`).
    pub(super) focus_fit: [f32; 4],
}

/// The working size for `depth` on a pipeline whose whole image is
/// `full_width` × `full_height`: the full image's aspect, its longer edge the
/// least of [`working_long_edge`], the depth's and the image's own.
pub(super) fn scene_working_size(
    depth: &SceneDepth,
    full_width: u32,
    full_height: u32,
) -> [u32; 2] {
    let disparity = depth.disparity();
    let long = working_long_edge()
        .min(disparity.width().max(disparity.height()))
        .min(full_width.max(full_height))
        .max(1);
    let (full_width, full_height) = (full_width.max(1), full_height.max(1));
    let short = |short_edge: u32, long_edge: u32| {
        ((long as f64 * f64::from(short_edge) / f64::from(long_edge)).round() as u32).clamp(1, long)
    };
    if full_width >= full_height {
        [long, short(full_height, full_width)]
    } else {
        [short(full_width, full_height), long]
    }
}

/// The working size of a bake without a depth result: the full image's
/// aspect, its longer edge at most [`IMAGE_ONLY_LONG_EDGE`].
pub(super) fn scene_image_size(full_width: u32, full_height: u32) -> [u32; 2] {
    let long = IMAGE_ONLY_LONG_EDGE.min(full_width.max(full_height)).max(1);
    let (full_width, full_height) = (full_width.max(1), full_height.max(1));
    let short = |short_edge: u32, long_edge: u32| {
        ((long as f64 * f64::from(short_edge) / f64::from(long_edge)).round() as u32).clamp(1, long)
    };
    if full_width >= full_height {
        [long, short(full_height, full_width)]
    } else {
        [short(full_width, full_height), long]
    }
}

/// Mip levels of the working image: down to 1 texel on the longer edge.
fn working_mip_levels([width, height]: [u32; 2]) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// The scene bake of one working size (module docs).
pub(super) struct SceneBake {
    size: [u32; 2],
    layers: wgpu::Texture,
    /// All layers, as effects read them (geometry.wgsl).
    layers_view: wgpu::TextureView,
    working: wgpu::Texture,
    /// The working image with its whole mip chain, as effects read it.
    working_view_all: wgpu::TextureView,
    scratch: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    focus: wgpu::Buffer,
    /// Per level of the layers' pyramid from 1: the level (`scene_layer_level`).
    pyramid_uniforms: Vec<wgpu::Buffer>,
    /// Per mip level of the working image from 1: the level and the finer
    /// level's size (`scene_mip_level`).
    mip_uniforms: Vec<wgpu::Buffer>,
    state: Mutex<SceneBakeState>,
    /// The light bake (`lights.rs`), allocated once a light casts shadows.
    lights: Mutex<Option<lights::SceneLightBake>>,
    /// Counts the light bake's allocations, so the pipelines that bind its
    /// visibility bind the current one.
    lights_revision: std::sync::atomic::AtomicU64,
    _reservation: GpuBudgetReservation,
}

struct SceneBakeState {
    /// The uniforms hold the working size without a depth result's values:
    /// effects see no geometry (`SceneBake::clear_depth`).
    image_only: bool,
    scene: WorkingScene,
    /// The depth result uploaded, if any.
    inputs: Option<SceneBakeInputs>,
    /// The layers are baked from the current working image and `inputs`.
    baked: bool,
    /// Counts the bakes, so what is built from the layers knows when they
    /// changed.
    generation: u64,
}

/// What the working image holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkingScene {
    /// An earlier scene, or nothing yet.
    Outdated,
    /// Nothing usable: accumulation passes are adding the current scene to
    /// the scratch buffer.
    Accumulating,
    /// The current scene.
    Current,
}

/// A depth result's inputs on the GPU, each at its own size.
struct SceneBakeInputs {
    /// The result uploaded, to recognise it again; it shares the edit's
    /// pixels.
    depth: SceneDepth,
    disparity: wgpu::Texture,
    sky: wgpu::Texture,
    normals: wgpu::Texture,
    _reservation: GpuBudgetReservation,
}

impl SceneBake {
    /// A bake of `size` texels with its budget reservation.
    pub(super) fn new(device: &wgpu::Device, size: [u32; 2]) -> Result<Self> {
        let [width, height] = size;
        let texels = u64::from(width) * u64::from(height);
        let mips = working_mip_levels(size);
        let bytes = texels * 8 * u64::from(SCENE_BAKE_LAYERS)
            + texels * 8 * 4 / 3
            + texels * SCRATCH_TEXEL_BYTES
            + SCENE_FOCUS_BYTES;
        let reservation = GpuBudgetReservation::acquire_bytes(
            "scene bake",
            bytes,
            gpu_working_set_limit_bytes(),
        )?;
        let layers = create_processing_texture_array(
            device,
            width,
            height,
            SCENE_BAKE_LAYERS,
            SCENE_BAKE_FORMAT,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            "calibraw scene bake layers",
        );
        let working = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw scene working image"),
            size: texture_size(width, height),
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_BAKE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[SCENE_BAKE_FORMAT],
        });
        let scratch = create_gpu_buffer(
            device,
            "calibraw scene bake scratch",
            texels * SCRATCH_TEXEL_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        );
        let uniforms = create_gpu_buffer(
            device,
            "calibraw scene bake uniforms",
            std::mem::size_of::<SceneBakeUniforms>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let focus = create_gpu_buffer(
            device,
            "calibraw scene lens blur",
            SCENE_FOCUS_BYTES,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        let level_uniform = |values: [u32; 4]| {
            create_initialized_buffer(
                device,
                "calibraw scene bake level",
                bytemuck::bytes_of(&values),
                wgpu::BufferUsages::UNIFORM,
            )
        };
        let pyramid_uniforms = (1..=SCENE_LAYER_LEVELS)
            .map(|level| level_uniform([level, 0, 0, 0]))
            .collect();
        let mip_uniforms = (1..mips)
            .map(|level| {
                let finer = level - 1;
                level_uniform([level, (width >> finer).max(1), (height >> finer).max(1), 0])
            })
            .collect();
        let working_view_all = working.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene working image"),
            ..Default::default()
        });
        let layers_view = layers.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene bake layers"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Ok(Self {
            size,
            layers,
            layers_view,
            working,
            working_view_all,
            scratch,
            uniforms,
            focus,
            pyramid_uniforms,
            mip_uniforms,
            state: Mutex::new(SceneBakeState {
                image_only: false,
                scene: WorkingScene::Outdated,
                inputs: None,
                baked: false,
                generation: 0,
            }),
            lights: Mutex::new(None),
            lights_revision: std::sync::atomic::AtomicU64::new(1),
            _reservation: reservation,
        })
    }

    pub(super) fn size(&self) -> [u32; 2] {
        self.size
    }

    /// What passes bind of the bake, with the light bake's `visibility`.
    pub(super) fn bindings<'a>(
        &'a self,
        lights: &'a super::effect_inputs::SceneLightBindings,
    ) -> super::effect_inputs::SceneBakeBindings<'a> {
        super::effect_inputs::SceneBakeBindings {
            scratch: &self.scratch,
            uniforms: &self.uniforms,
            layers: &self.layers_view,
            working: &self.working_view_all,
            lights,
        }
    }

    /// Changes whenever the light bake's visibility texture is allocated.
    pub(super) fn lights_revision(&self) -> u64 {
        self.lights_revision
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SceneBakeState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Uploads `depth` unless it is the one uploaded, which outdates the
    /// layers but not the working image.
    pub(super) fn upload_depth(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: &SceneDepth,
    ) -> Result<()> {
        let mut state = self.lock();
        if state
            .inputs
            .as_ref()
            .is_some_and(|inputs| same_depth(&inputs.depth, depth))
        {
            return Ok(());
        }
        state.inputs = None;
        state.baked = false;
        let inputs = SceneBakeInputs::upload(device, queue, depth, self.size)?;
        let (ground_plane, ground_horizon) = ground::scene_ground(depth);
        let uniforms = SceneBakeUniforms {
            size: [
                self.size[0],
                self.size[1],
                inputs.disparity.width(),
                inputs.disparity.height(),
            ],
            input_size: [
                inputs.sky.width(),
                inputs.sky.height(),
                inputs.normals.width(),
                inputs.normals.height(),
            ],
            camera: SceneCameraUniform::new(depth.camera()),
            ground_plane,
            ground_horizon,
            focus_fit: [0.0; 4],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        state.inputs = Some(inputs);
        state.image_only = false;
        Ok(())
    }

    /// Drops the depth result, if any: effects read the working image only.
    pub(super) fn clear_depth(&self, queue: &wgpu::Queue) {
        let mut state = self.lock();
        if state.image_only {
            return;
        }
        state.inputs = None;
        state.baked = false;
        state.image_only = true;
        let uniforms = SceneBakeUniforms {
            size: [self.size[0], self.size[1], 0, 0],
            ..Default::default()
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Marks the working image outdated: the scene changed.
    pub(super) fn outdate_scene(&self) {
        let mut state = self.lock();
        state.scene = WorkingScene::Outdated;
        state.baked = false;
    }

    /// Whether the working image holds the current scene.
    pub(super) fn scene_current(&self) -> bool {
        self.lock().scene == WorkingScene::Current
    }

    /// The uploaded depth result's camera.
    fn camera(&self) -> Option<SceneCamera> {
        self.lock()
            .inputs
            .as_ref()
            .map(|inputs| *inputs.depth.camera())
    }

    /// The current bake's generation, once the layers are baked.
    fn baked_generation(&self) -> Option<u64> {
        let state = self.lock();
        state.baked.then_some(state.generation)
    }

    /// Prepares the scratch buffer for an accumulation pass encoded after
    /// this: the first one since the scene changed clears it, later ones (the
    /// next export tiles) add to it.
    pub(super) fn begin_accumulation(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut state = self.lock();
        if state.scene != WorkingScene::Accumulating {
            encoder.clear_buffer(&self.scratch, 0, None);
            state.scene = WorkingScene::Accumulating;
            state.baked = false;
        }
    }

    /// Bakes what is outdated: after an accumulation, the working image and
    /// then the layers; after a new depth result, the layers. Without a depth
    /// result it bakes the working image alone. It encodes nothing without a
    /// scene.
    pub(super) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        programs: &SceneBakePrograms,
    ) {
        let mut state = self.lock();
        let accumulated = match state.scene {
            WorkingScene::Outdated => return,
            WorkingScene::Accumulating => true,
            WorkingScene::Current if state.baked => return,
            WorkingScene::Current => false,
        };
        match state.inputs.as_ref() {
            Some(inputs) => programs.encode(device, encoder, self, inputs, accumulated),
            None if state.image_only => programs.encode_image(device, encoder, self, accumulated),
            // Neither a depth result nor the uniforms of a bake without one:
            // nothing says what to bake.
            None => return,
        }
        state.scene = WorkingScene::Current;
        state.baked = true;
        state.generation += 1;
    }
}

/// Whether two depth results are the same one: the same pixel allocations.
fn same_depth(a: &SceneDepth, b: &SceneDepth) -> bool {
    Arc::ptr_eq(a.disparity().values(), b.disparity().values())
        && Arc::ptr_eq(&a.sky().pixels, &b.sky().pixels)
        && Arc::ptr_eq(&a.normals().x().pixels, &b.normals().x().pixels)
        && a.camera() == b.camera()
}

impl SceneBakeInputs {
    /// Uploads `depth`'s parts. A part more than half again as large as the
    /// working size is first averaged down by a whole factor, as the bake
    /// samples it near the working size (old results are stored at up to
    /// 4096 texels).
    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth: &SceneDepth,
        working: [u32; 2],
    ) -> Result<Self> {
        let disparity_map = depth.disparity();
        let disparity = reduced(
            disparity_map.width(),
            disparity_map.height(),
            working,
            |x, y| {
                DisparityMap::decode(disparity_map.values()[y * disparity_map.width() as usize + x])
            },
        );
        let sky = reduced_mask(depth.sky(), working);
        let normal_x = reduced_mask(depth.normals().x(), working);
        let normal_y = reduced_mask(depth.normals().y(), working);
        let normals: Vec<f32> = normal_x
            .values
            .iter()
            .zip(&normal_y.values)
            .flat_map(|(&x, &y)| [x, y])
            .collect();
        let bytes =
            disparity.values.len() as u64 * 4 + sky.values.len() as u64 + normals.len() as u64;
        let reservation = GpuBudgetReservation::acquire_bytes(
            "scene depth inputs",
            bytes,
            gpu_working_set_limit_bytes(),
        )?;
        let texture =
            |[width, height]: [u32; 2], format, label, bytes_per_texel: u32, data: &[u8]| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: texture_size(width, height),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[format],
                });
                queue.write_texture(
                    texture.as_image_copy(),
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * bytes_per_texel),
                        rows_per_image: Some(height),
                    },
                    texture_size(width, height),
                );
                texture
            };
        // Back to 8 bits: averaging down only moved them between levels.
        let as_bytes = |values: &[f32]| -> Vec<u8> {
            values
                .iter()
                .map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8)
                .collect()
        };
        Ok(Self {
            depth: depth.clone(),
            disparity: texture(
                [disparity.width, disparity.height],
                wgpu::TextureFormat::R32Float,
                "calibraw scene disparity",
                4,
                bytemuck::cast_slice(&disparity.values),
            ),
            sky: texture(
                [sky.width, sky.height],
                wgpu::TextureFormat::R8Unorm,
                "calibraw scene sky",
                1,
                &as_bytes(&sky.values),
            ),
            normals: texture(
                [normal_x.width, normal_x.height],
                wgpu::TextureFormat::Rg8Unorm,
                "calibraw scene normals",
                2,
                &as_bytes(&normals),
            ),
            _reservation: reservation,
        })
    }
}

/// An input part, possibly averaged down.
struct Reduced {
    width: u32,
    height: u32,
    values: Vec<f32>,
}

/// `width` × `height` values from `at`, averaged down by the whole factor
/// that brings them nearest the working size if they exceed it by half
/// again, else as they are.
fn reduced(
    width: u32,
    height: u32,
    working: [u32; 2],
    at: impl Fn(usize, usize) -> f32,
) -> Reduced {
    let ratio = width.max(height) as f32 / working[0].max(working[1]).max(1) as f32;
    let factor = if ratio >= 1.5 {
        ratio.floor() as u32
    } else {
        1
    };
    let (out_width, out_height) = ((width / factor).max(1), (height / factor).max(1));
    let mut values = Vec::with_capacity((out_width * out_height) as usize);
    for y in 0..out_height {
        for x in 0..out_width {
            let mut sum = 0.0;
            for dy in 0..factor {
                for dx in 0..factor {
                    sum += at(
                        ((x * factor + dx).min(width - 1)) as usize,
                        ((y * factor + dy).min(height - 1)) as usize,
                    );
                }
            }
            values.push(sum / (factor * factor) as f32);
        }
    }
    Reduced {
        width: out_width,
        height: out_height,
        values,
    }
}

/// An 8-bit image's values in [0, 1], averaged down as [`reduced`] does.
fn reduced_mask(image: &MaskImage, working: [u32; 2]) -> Reduced {
    let width = image.width as usize;
    reduced(image.width, image.height, working, |x, y| {
        f32::from(image.pixels[y * width + x]) / 255.0
    })
}

impl SceneBake {
    /// A layer of the bake array as a 2D view, for reading or writing it.
    fn layer_view(&self, layer: u32) -> wgpu::TextureView {
        self.layers.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene bake layer"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    }

    fn working_view(&self, level: u32) -> wgpu::TextureView {
        self.working.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene working image level"),
            base_mip_level: level,
            mip_level_count: Some(1),
            ..Default::default()
        })
    }
}

impl super::RawGpuPipeline {
    /// Whether this pipeline's last uploaded parameters render the whole
    /// image in one tile, so it can fill a bake's working image itself.
    fn renders_whole_image(&self) -> bool {
        let uploaded = self
            .uploaded_stage_uniforms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let camera = &uploaded.camera;
        camera.tile_origin_x == 0
            && camera.tile_origin_y == 0
            && camera.width == camera.full_width
            && camera.height == camera.full_height
    }

    /// Allocates the bake `params` needs at the image's working size and
    /// uploads its depth result, if its effects read the geometry: for a
    /// pipeline that fills the bake itself (the whole image, or an export's
    /// tiles). Otherwise the bake holds the working image alone.
    pub(super) fn own_scene_bake(
        &self,
        queue: &wgpu::Queue,
        params: &super::GpuParams,
    ) -> Option<Arc<SceneBake>> {
        let (full_width, full_height) = (params.camera.full_width, params.camera.full_height);
        let depth = params
            .depth_result
            .as_ref()
            .filter(|_| params.needs_scene_geometry());
        let size = match depth {
            Some(depth) => scene_working_size(depth, full_width, full_height),
            None => scene_image_size(full_width, full_height),
        };
        let bake = self
            .effect_inputs
            .allocate_scene_bake(size)
            .inspect_err(|error| log::error!("scene bake not allocated: {error:#}"))
            .ok()?;
        match depth {
            Some(depth) => bake
                .upload_depth(self.effect_inputs.device(), queue, depth)
                .inspect_err(|error| log::error!("scene depth not uploaded: {error:#}"))
                .ok()?,
            None => bake.clear_depth(queue),
        }
        Some(bake)
    }

    /// Uploads `params`' depth result to the bake: one this pipeline owns for
    /// the whole image, or the one a view of part of it shares (allocated by
    /// the whole image's pipeline, `share_scene_bake`).
    pub(super) fn upload_scene_bake_depth(&self, queue: &wgpu::Queue, params: &super::GpuParams) {
        if !params.needs_scene_bake() {
            return;
        }
        let device = self.effect_inputs.device();
        let bake = if self.renders_whole_image() {
            self.own_scene_bake(queue, params)
        } else {
            let Some(bake) = self.effect_inputs.scene_bake() else {
                return;
            };
            if let Some(depth) = params
                .depth_result
                .as_ref()
                .filter(|_| params.needs_scene_geometry())
            {
                if let Err(error) = bake.upload_depth(device, queue, depth) {
                    log::error!("scene depth not uploaded: {error:#}");
                }
            }
            Some(bake)
        };
        if let Some(bake) = bake {
            if params.needs_light_bake() {
                bake.prepare_lights(device, queue, &params.scene_light_set());
            }
        }
    }

    /// A new scene: the whole image's working image no longer holds it.
    pub(super) fn outdate_scene_bake(&self) {
        if let Some(bake) = self.effect_inputs.scene_bake() {
            if self.renders_whole_image() {
                bake.outdate_scene();
            }
        }
    }

    /// Brings the bake up to date before effects read it. A pipeline that
    /// renders the whole image first accumulates its scene if the working
    /// image is outdated; others bake only what their bake's owner filled.
    pub(super) fn ensure_scene_bake(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        params: &super::GpuParams,
    ) {
        if !params.needs_scene_bake() {
            return;
        }
        let Some(bake) = self.effect_inputs.scene_bake() else {
            return;
        };
        if !bake.scene_current() && self.renders_whole_image() {
            self.accumulate_scene_working_image(encoder, &bake);
        }
        self.encode_scene_bake(encoder, &bake);
        if params.needs_light_bake() {
            let device = self.effect_inputs.device();
            match self.effect_inputs.scene_programs() {
                Ok(programs) => {
                    bake.encode_lights(device, encoder, programs.lights());
                    if params.needs_air_light() {
                        self.effect_inputs.encode_air_light(
                            encoder,
                            &bake,
                            programs.lights(),
                            self.air_light_region(params.scene_light_quality()),
                        );
                    }
                }
                Err(error) => log::error!("scene bake programs did not compile: {error:#}"),
            }
        }
    }

    /// The air light grid this pipeline needs: the texels its pixels read.
    fn air_light_region(&self, quality: SceneLightQuality) -> SceneAirLightRegion {
        let uploaded = self
            .uploaded_stage_uniforms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let camera = &uploaded.camera;
        SceneAirLightRegion::covering(
            [
                camera.tile_origin_x.max(0) as u32,
                camera.tile_origin_y.max(0) as u32,
            ],
            [camera.width, camera.height],
            [camera.full_width, camera.full_height],
            quality,
        )
    }

    /// Adds this pipeline's tile core (or whole image) to the bake's working
    /// image.
    pub(super) fn accumulate_scene_working_image(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bake: &SceneBake,
    ) {
        bake.begin_accumulation(encoder);
        self.encode_pass(encoder, self.indices.scene_bake_accumulation_pass_index);
    }

    pub(super) fn encode_scene_bake(&self, encoder: &mut wgpu::CommandEncoder, bake: &SceneBake) {
        let device = self.effect_inputs.device();
        match self.effect_inputs.scene_programs() {
            Ok(programs) => bake.encode(device, encoder, &programs),
            Err(error) => log::error!("scene bake programs did not compile: {error:#}"),
        }
    }

    /// Shares the whole image's bake (`full_frame`'s) with this view of part
    /// of it, first bringing it up to date.
    pub(super) fn inherit_scene_bake(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        full_frame: &Self,
        params: &super::GpuParams,
    ) {
        if !params.needs_scene_bake() {
            return;
        }
        full_frame.ensure_scene_bake(encoder, params);
        if let Some(bake) = full_frame.effect_inputs.scene_bake() {
            self.effect_inputs.share_scene_bake(&bake);
        }
    }
}
