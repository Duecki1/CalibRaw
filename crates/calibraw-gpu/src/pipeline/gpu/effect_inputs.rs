//! GPU inputs that only some effects read, allocated the first time one of
//! them renders: scene depth and the scene map that every depth-reading effect
//! shares, the scene shadow map, the image-light map that Fog, Smoke and Rain
//! scatter, the contours and the light that Glow and Neon emit into the scene,
//! the haze map Dehaze applies, the LUT the view pass applies, and the scene
//! bake (`scene`), whose working image a pass of this pipeline accumulates.
//!
//! Until then the passes that read them are bound to placeholders of one
//! texel, so a pipeline whose edits never use these effects holds none of their
//! memory. The process
//! budget reserves them with the pipeline (`GpuResourceResidency::OnDemand`),
//! so allocating them later cannot exceed it. Once allocated they stay until
//! the pipeline is dropped.
//!
//! The LUT texture is the exception: its size depends on the LUT, so it takes
//! its own reservation when a LUT is uploaded, and pipelines without one
//! reserve nothing for it. A LUT of another size replaces it.
//!
//! Passes bound to these inputs are tagged with an [`InputBoundPass`]; the
//! pipeline takes their bind groups from [`EffectInputs::bind_group`], which
//! follow the allocation.

use super::scene::{
    air_light_layers, SceneAirLightKey, SceneAirLightRegion, SceneAirLightTarget,
    SceneAirLightUniform, SceneBakePrograms, SceneLightPrograms, SceneShadowLightsUniform,
    AIR_LIGHT_FORMAT,
};
use super::*;

/// The image-light passes: accumulate, resolve, horizontal and vertical blur.
pub(super) const IMAGE_LIGHT_PASSES: usize = 4;
/// The haze passes (haze_analysis.wgsl): accumulate, airlight, transmission
/// and the guided filter's two window passes.
pub(super) const HAZE_PASSES: usize = 5;
/// The scene emission passes (scene_emission.wgsl): the emitter grid, then the
/// light on surfaces and in the volume.
pub(super) const SCENE_EMISSION_PASSES: usize = 3;

/// Coarse maps over the full image that a cropped view or an export tile
/// cannot build from its own region. Each is built from the whole scene with
/// the tone statistics, accumulated by every export tile during the tone
/// prepass, and copied from the full frame by cropped and zoomed views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FullImageMap {
    /// The light sources Fog, Smoke and Rain scatter (tone_analysis.wgsl).
    ImageLights,
    /// The airlight and transmission Dehaze applies (haze_analysis.wgsl).
    Haze,
    /// How much of the photo lies on its contours, for Neon's scene light
    /// (tone_analysis.wgsl). Views copy the grid itself; it has no resolve.
    Contours,
}

impl FullImageMap {
    pub(super) const ALL: [Self; 3] = [Self::ImageLights, Self::Haze, Self::Contours];
}

/// The resources of an allocated [`FullImageMap`].
pub(super) struct FullImageMapResources {
    /// The grid every accumulation pass adds to, cleared before a new image.
    pub(super) cells: wgpu::Buffer,
    /// What a cropped or zoomed view copies from the full frame, in an order
    /// that is the same for every pipeline.
    pub(super) textures: Vec<wgpu::Texture>,
    pub(super) buffers: Vec<wgpu::Buffer>,
}

/// A pass whose bind group follows the allocated effect inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputBoundPass {
    /// The scene preparation, which applies Dehaze from the haze map.
    ScenePreparation,
    /// Wet Surfaces, the creative effects or Rain, reading the first work
    /// texture and writing the second.
    EffectsFromTex1,
    /// The same, reading the second work texture and writing the first.
    EffectsFromTex2,
    /// The view transform and output (`apply_view_node`), which reads the LUT.
    View,
    ViewAfterBlur,
    /// One of the image-light passes, in pass order.
    ImageLights(usize),
    /// One of the haze passes, in pass order.
    Haze(usize),
    /// Accumulation into the contour grid.
    Contours,
    /// One of the scene emission passes, in pass order.
    SceneEmission(usize),
    /// Accumulation into the scene bake's working image.
    SceneBakeAccumulation,
}

/// Bind groups of every [`InputBoundPass`].
struct InputBindGroups {
    scene_preparation: wgpu::BindGroup,
    effects_from_tex1: wgpu::BindGroup,
    effects_from_tex2: wgpu::BindGroup,
    view: wgpu::BindGroup,
    view_after_blur: wgpu::BindGroup,
    image_lights: [wgpu::BindGroup; IMAGE_LIGHT_PASSES],
    haze: [wgpu::BindGroup; HAZE_PASSES],
    contours: wgpu::BindGroup,
    scene_emission: [wgpu::BindGroup; SCENE_EMISSION_PASSES],
    scene_bake_accumulation: wgpu::BindGroup,
}

impl InputBindGroups {
    fn get(&self, pass: InputBoundPass) -> &wgpu::BindGroup {
        match pass {
            InputBoundPass::ScenePreparation => &self.scene_preparation,
            InputBoundPass::EffectsFromTex1 => &self.effects_from_tex1,
            InputBoundPass::EffectsFromTex2 => &self.effects_from_tex2,
            InputBoundPass::View => &self.view,
            InputBoundPass::ViewAfterBlur => &self.view_after_blur,
            InputBoundPass::ImageLights(index) => &self.image_lights[index],
            InputBoundPass::Haze(index) => &self.haze[index],
            InputBoundPass::Contours => &self.contours,
            InputBoundPass::SceneEmission(index) => &self.scene_emission[index],
            InputBoundPass::SceneBakeAccumulation => &self.scene_bake_accumulation,
        }
    }
}

/// The pipeline's own layouts, buffers and views that the input-bound passes
/// also bind. Handles are reference-counted, so keeping them is cheap.
pub(super) struct InputBindingSources {
    pub(super) scene_preparation_layout: wgpu::BindGroupLayout,
    pub(super) creative_layout: wgpu::BindGroupLayout,
    pub(super) view_layout: wgpu::BindGroupLayout,
    pub(super) image_light_layouts: [wgpu::BindGroupLayout; IMAGE_LIGHT_PASSES],
    pub(super) haze_layouts: [wgpu::BindGroupLayout; HAZE_PASSES],
    pub(super) contour_layout: wgpu::BindGroupLayout,
    pub(super) scene_emission_layouts: [wgpu::BindGroupLayout; SCENE_EMISSION_PASSES],
    pub(super) scene_bake_accumulation_layout: wgpu::BindGroupLayout,
    pub(super) camera_uniforms: wgpu::Buffer,
    pub(super) mask_data: wgpu::Buffer,
    pub(super) tone_stats: wgpu::Buffer,
    pub(super) profile: wgpu::Buffer,
    pub(super) raw: wgpu::TextureView,
    pub(super) color: wgpu::TextureView,
    pub(super) black: wgpu::TextureView,
    pub(super) tone_guide: wgpu::TextureView,
    pub(super) scene: wgpu::TextureView,
    pub(super) tex1: wgpu::TextureView,
    pub(super) tex2: wgpu::TextureView,
    pub(super) display_linear: wgpu::TextureView,
    /// The display output.
    pub(super) out: wgpu::TextureView,
    pub(super) mask: wgpu::TextureView,
    pub(super) mask_sampler: wgpu::Sampler,
    pub(super) full_image_mask: wgpu::TextureView,
    pub(super) pixelate_blocks: wgpu::TextureView,
}

/// The image-light grid and its maps (tone_analysis.wgsl).
struct ImageLightInputs {
    cells: wgpu::Buffer,
    /// The resolved and finally blurred map media sample; cropped and zoomed
    /// views copy it from the full frame.
    map: wgpu::Texture,
    map_view: wgpu::TextureView,
    _core: wgpu::Texture,
    core_view: wgpu::TextureView,
    _tail: wgpu::Texture,
    tail_view: wgpu::TextureView,
}

impl ImageLightInputs {
    /// Inputs for a grid of `edge` × `edge` cells.
    fn new(device: &wgpu::Device, edge: u32) -> Self {
        let size = texture_size(edge, edge);
        let usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |usage, label| {
            let texture = create_processing_texture(device, size, IMAGE_LIGHT_FORMAT, usage, label);
            let view = default_texture_view(&texture);
            (texture, view)
        };
        let (map, map_view) = texture(
            usage | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            "calibraw full-image image-light map",
        );
        let (core, core_view) = texture(usage, "calibraw image-light halo core");
        let (tail, tail_view) = texture(usage, "calibraw image-light halo tail");
        Self {
            cells: create_gpu_buffer(
                device,
                "calibraw image-light grid",
                image_light_cells_bytes(edge),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            map,
            map_view,
            _core: core,
            core_view,
            _tail: tail,
            tail_view,
        }
    }
}

/// The contour grid (tone_analysis.wgsl), which the emitter pass reads as it is
/// and cropped and zoomed views copy from the full frame.
struct ContourInputs {
    cells: wgpu::Buffer,
}

impl ContourInputs {
    /// A grid of `edge` × `edge` cells.
    fn new(device: &wgpu::Device, edge: u32) -> Self {
        Self {
            cells: create_gpu_buffer(
                device,
                "calibraw contour grid",
                contour_cells_bytes(edge),
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
        }
    }
}

/// The light Glow and Neon emit into the scene (scene_emission.wgsl): the
/// emitter grid, and the light on surfaces and in the volume media read, which
/// share one texture. Every pipeline builds them from its own parameters in its
/// output stage.
struct SceneEmissionInputs {
    _emitters: wgpu::Texture,
    emitters_view: wgpu::TextureView,
    _light: wgpu::Texture,
    light_view: wgpu::TextureView,
}

impl SceneEmissionInputs {
    /// Textures for grids of `edge` and `surface_edge` cells along the longer
    /// edge; a texel or two for the placeholders.
    fn new(device: &wgpu::Device, edge: u32, surface_edge: u32, medium_cells: u32) -> Self {
        let usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |width, height, label| {
            let texture = create_processing_texture(
                device,
                texture_size(width, height),
                SCENE_EMISSION_FORMAT,
                usage,
                label,
            );
            let view = default_texture_view(&texture);
            (texture, view)
        };
        let (emitters, emitters_view) = texture(edge, 2 * edge, "calibraw scene emission emitters");
        let (light, light_view) = texture(
            surface_edge.max(edge),
            scene_emission_light_rows(edge, surface_edge, medium_cells),
            "calibraw scene emission light",
        );
        Self {
            _emitters: emitters,
            emitters_view,
            _light: light,
            light_view,
        }
    }
}

/// The haze grid, its airlight and its maps (haze_analysis.wgsl).
struct HazeInputs {
    cells: wgpu::Buffer,
    /// The resolved airlight and added veil Dehaze reads; cropped and zoomed
    /// views copy them from the full frame.
    airlight: wgpu::Buffer,
    /// The transmission per cell, then the guided filter's averaged
    /// coefficients and the depth base Dehaze reads; cropped and zoomed views
    /// copy them.
    map: wgpu::Texture,
    map_view: wgpu::TextureView,
    _work: wgpu::Texture,
    /// The guided filter's coefficients of one window, before averaging, and
    /// the depth base.
    work_view: wgpu::TextureView,
}

impl HazeInputs {
    /// Inputs for a grid of `edge` × `edge` cells.
    fn new(device: &wgpu::Device, edge: u32) -> Self {
        let usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |usage, label| {
            let texture = create_processing_texture(
                device,
                texture_size(edge, edge),
                HAZE_MAP_FORMAT,
                usage,
                label,
            );
            let view = default_texture_view(&texture);
            (texture, view)
        };
        let (map, map_view) = texture(
            usage | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            "calibraw full-image haze map",
        );
        let (work, work_view) = texture(usage, "calibraw haze guided-filter coefficients");
        Self {
            cells: create_gpu_buffer(
                device,
                "calibraw haze grid",
                haze_cells_bytes(edge),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            airlight: create_gpu_buffer(
                device,
                "calibraw haze airlight",
                HAZE_AIRLIGHT_BYTES,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            map,
            map_view,
            _work: work,
            work_view,
        }
    }
}

/// The LUT the view pass applies (look_lut.wgsl): `size`³ texels, red along
/// x, green along y and blue along z.
struct LookLutInputs {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: u32,
    /// The LUT the texture holds; `None` until one is written.
    uploaded: Option<LutId>,
    _reservation: Option<GpuBudgetReservation>,
}

impl LookLutInputs {
    fn new(device: &wgpu::Device, size: u32, reservation: Option<GpuBudgetReservation>) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw look LUT"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: LOOK_LUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[LOOK_LUT_FORMAT],
        });
        Self {
            view: default_texture_view(&texture),
            texture,
            size,
            uploaded: None,
            _reservation: reservation,
        }
    }

    /// A texture for `size`³ texels with its budget reservation.
    fn reserved(device: &wgpu::Device, size: u32) -> Result<Self> {
        let reservation = GpuBudgetReservation::acquire_bytes(
            "LUT texture",
            look_lut_texture_bytes(size)?,
            gpu_working_set_limit_bytes(),
        )?;
        Ok(Self::new(device, size, Some(reservation)))
    }

    fn write(&mut self, queue: &wgpu::Queue, lut: &CubeLut) {
        let texels: Vec<[f32; 4]> = lut
            .table()
            .iter()
            .map(|&[red, green, blue]| [red, green, blue, 1.0])
            .collect();
        let row_bytes = self.size * std::mem::size_of::<[f32; 4]>() as u32;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(self.size),
            },
            self.texture.size(),
        );
        self.uploaded = Some(lut.id());
    }
}

/// Views the input-bound passes bind, allocated or placeholder.
struct BoundInputs<'a> {
    image_lights: &'a ImageLightInputs,
    haze: &'a HazeInputs,
    contours: &'a ContourInputs,
    scene_emission: &'a SceneEmissionInputs,
    look_lut: &'a LookLutInputs,
    scene_bake: SceneBakeBindings<'a>,
    air_light: &'a AirLightInputs,
}

/// This pipeline's air light (`scene::lights`, air_light.wgsl): a texture
/// array of `groups` light groups over its region, the uniform that says
/// where it lies, and what it was built from. A placeholder of one texel and
/// an all-zero uniform (no air light) is bound until one is built.
pub(super) struct AirLightInputs {
    pub(super) texture: wgpu::Texture,
    view: wgpu::TextureView,
    pub(super) uniforms: wgpu::Buffer,
    pub(super) size: [u32; 2],
    pub(super) groups: u32,
    pub(super) built: Option<SceneAirLightKey>,
    _reservation: Option<GpuBudgetReservation>,
}

impl AirLightInputs {
    fn new(
        device: &wgpu::Device,
        size: [u32; 2],
        groups: u32,
        reservation: Option<GpuBudgetReservation>,
    ) -> Self {
        let layers = air_light_layers(groups);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw scene air light"),
            size: wgpu::Extent3d {
                width: size[0].max(1),
                height: size[1].max(1),
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: AIR_LIGHT_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene air light"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let uniforms = create_gpu_buffer(
            device,
            "calibraw scene air light uniforms",
            std::mem::size_of::<SceneAirLightUniform>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        Self {
            texture,
            view,
            uniforms,
            size,
            groups,
            built: None,
            _reservation: reservation,
        }
    }

    /// The bytes a texture of `size` and `groups` takes.
    fn bytes(size: [u32; 2], groups: u32) -> u64 {
        u64::from(size[0].max(1))
            * u64::from(size[1].max(1))
            * 8
            * u64::from(air_light_layers(groups))
    }
}

/// What passes bind of the scene bake: the scratch buffer and uniforms the
/// working-image accumulation writes, and the bake's layers, its working image
/// and the light bake that effects read (scene/geometry.wgsl).
#[derive(Clone, Copy)]
pub(super) struct SceneBakeBindings<'a> {
    pub(super) scratch: &'a wgpu::Buffer,
    pub(super) uniforms: &'a wgpu::Buffer,
    pub(super) layers: &'a wgpu::TextureView,
    /// The working image with its mip chain.
    pub(super) working: &'a wgpu::TextureView,
    pub(super) lights: &'a SceneLightBindings,
}

/// What effects read of the light bake: each surface's visibility, the shadow
/// cubes with the lights' uniform (for points in the air) and the sampler
/// that compares with them.
pub(super) struct SceneLightBindings {
    visibility: wgpu::TextureView,
    cubes: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    compare: wgpu::Sampler,
}

impl SceneLightBindings {
    /// The light bake's, or placeholders while it does not exist, which no
    /// light reads (the lights' uniform is zero).
    fn new(device: &wgpu::Device, bake: Option<&SceneBake>, compare: &wgpu::Sampler) -> Self {
        let view = |texture: wgpu::Texture, label: &str| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let placeholder = |format, usage, label: &str| {
            view(
                create_processing_texture(
                    device,
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 2,
                    },
                    format,
                    usage,
                    label,
                ),
                label,
            )
        };
        Self {
            visibility: bake
                .and_then(SceneBake::visibility_texture)
                .map(|texture| view(texture, "calibraw scene visibility"))
                .unwrap_or_else(|| {
                    placeholder(
                        wgpu::TextureFormat::Rgba8Unorm,
                        wgpu::TextureUsages::TEXTURE_BINDING,
                        "calibraw scene visibility placeholder",
                    )
                }),
            cubes: bake
                .and_then(SceneBake::cubes_texture)
                .map(|texture| view(texture, "calibraw scene shadow cubes"))
                .unwrap_or_else(|| {
                    placeholder(
                        wgpu::TextureFormat::Depth32Float,
                        wgpu::TextureUsages::TEXTURE_BINDING,
                        "calibraw scene shadow cubes placeholder",
                    )
                }),
            uniforms: bake
                .and_then(SceneBake::lights_uniforms)
                .unwrap_or_else(|| {
                    create_gpu_buffer(
                        device,
                        "calibraw scene shadow lights placeholder",
                        std::mem::size_of::<SceneShadowLightsUniform>() as u64,
                        wgpu::BufferUsages::UNIFORM,
                    )
                }),
            compare: compare.clone(),
        }
    }
}

/// Bound for the scene bake until it exists. Never read: the accumulation runs
/// only for an allocated bake, and effects read the bake only when its
/// uniforms hold a working size, which these leave at zero.
struct PlaceholderSceneBake {
    scratch: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    layers: wgpu::TextureView,
    working: wgpu::TextureView,
    lights: SceneLightBindings,
}

impl PlaceholderSceneBake {
    fn new(device: &wgpu::Device, compare: &wgpu::Sampler) -> Self {
        let layers = create_processing_texture(
            device,
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: SCENE_BAKE_LAYERS,
            },
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING,
            "calibraw scene bake layers placeholder",
        )
        .create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene bake layers placeholder"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            scratch: create_gpu_buffer(
                device,
                "calibraw scene bake scratch placeholder",
                16,
                wgpu::BufferUsages::STORAGE,
            ),
            uniforms: create_gpu_buffer(
                device,
                "calibraw scene bake uniforms placeholder",
                std::mem::size_of::<SceneBakeUniforms>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            layers,
            working: create_processing_texture(
                device,
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::TEXTURE_BINDING,
                "calibraw scene working image placeholder",
            )
            .create_view(&wgpu::TextureViewDescriptor::default()),
            lights: SceneLightBindings::new(device, None, compare),
        }
    }

    fn bindings(&self) -> SceneBakeBindings<'_> {
        SceneBakeBindings {
            scratch: &self.scratch,
            uniforms: &self.uniforms,
            layers: &self.layers,
            working: &self.working,
            lights: &self.lights,
        }
    }
}

/// The sampler that compares a depth with a shadow cube's: 1 where the
/// reference is at least the stored depth, filtered linearly.
fn scene_compare_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("calibraw scene shadow compare (effects)"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        compare: Some(wgpu::CompareFunction::GreaterEqual),
        ..Default::default()
    })
}

impl InputBindingSources {
    fn bind_groups(&self, device: &wgpu::Device, inputs: &BoundInputs<'_>) -> InputBindGroups {
        let haze = inputs.haze;
        let scene_preparation = create_bind_group(
            device,
            "bg adjustment preparation",
            &self.scene_preparation_layout,
            &[
                buffer_binding(0, &self.camera_uniforms),
                texture_binding(1, &self.raw),
                texture_binding(2, &self.color),
                texture_binding(19, &self.black),
                texture_binding(11, &self.scene),
                texture_binding(21, &self.tex1),
                buffer_binding(16, &self.tone_stats),
                texture_binding(17, &self.tone_guide),
                buffer_binding(20, &self.profile),
                texture_binding(27, &self.mask),
                sampler_binding(28, &self.mask_sampler),
                buffer_binding(33, &self.mask_data),
                buffer_binding(54, &haze.airlight),
                texture_binding(60, &haze.map_view),
            ],
        );
        // Every pass of effects binds the same scene, so an effect may read any
        // of it whichever pass it runs in.
        let effects = |label: &str, input: &wgpu::TextureView, output: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                &self.creative_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(11, &self.scene),
                    texture_binding(24, input),
                    texture_binding(25, output),
                    texture_binding(30, &self.display_linear),
                    texture_binding(27, &self.mask),
                    sampler_binding(28, &self.mask_sampler),
                    buffer_binding(33, &self.mask_data),
                    texture_binding(34, &self.full_image_mask),
                    buffer_binding(16, &self.tone_stats),
                    texture_binding(36, &self.pixelate_blocks),
                    texture_binding(45, &inputs.image_lights.map_view),
                    texture_binding(83, &inputs.scene_emission.light_view),
                    buffer_binding(85, inputs.scene_bake.uniforms),
                    buffer_binding(104, &inputs.scene_bake.lights.uniforms),
                    texture_binding(105, &inputs.scene_bake.lights.cubes),
                    sampler_binding(106, &inputs.scene_bake.lights.compare),
                    texture_binding(112, &inputs.scene_bake.lights.visibility),
                    texture_binding(113, inputs.scene_bake.layers),
                    texture_binding(117, inputs.scene_bake.working),
                    texture_binding(114, &inputs.air_light.view),
                    buffer_binding(115, &inputs.air_light.uniforms),
                    buffer_binding(20, &self.profile),
                ],
            )
        };
        let view = |label: &str, input: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                &self.view_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(12, &self.out),
                    texture_binding(26, input),
                    buffer_binding(16, &self.tone_stats),
                    buffer_binding(20, &self.profile),
                    texture_binding(27, &self.mask),
                    sampler_binding(28, &self.mask_sampler),
                    texture_binding(29, &self.display_linear),
                    buffer_binding(33, &self.mask_data),
                    texture_binding(61, &inputs.look_lut.view),
                ],
            )
        };
        let lights = inputs.image_lights;
        let [accumulate, resolve, horizontal, vertical] = &self.image_light_layouts;
        let image_lights = [
            create_bind_group(
                device,
                "bg image-light accumulation",
                accumulate,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(11, &self.scene),
                    buffer_binding(20, &self.profile),
                    buffer_binding(38, &lights.cells),
                ],
            ),
            create_bind_group(
                device,
                "bg image-light resolve",
                resolve,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(16, &self.tone_stats),
                    buffer_binding(38, &lights.cells),
                    texture_binding(39, &lights.map_view),
                ],
            ),
            create_bind_group(
                device,
                "bg image-light horizontal blur",
                horizontal,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(40, &lights.map_view),
                    texture_binding(41, &lights.core_view),
                    texture_binding(42, &lights.tail_view),
                ],
            ),
            create_bind_group(
                device,
                "bg image-light vertical blur",
                vertical,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(43, &lights.core_view),
                    texture_binding(44, &lights.tail_view),
                    texture_binding(39, &lights.map_view),
                ],
            ),
        ];
        let [accumulate, airlight, transmission, coefficients, smooth] = &self.haze_layouts;
        let haze_groups = [
            create_bind_group(
                device,
                "bg haze accumulation",
                accumulate,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(11, &self.scene),
                    buffer_binding(20, &self.profile),
                    buffer_binding(52, &haze.cells),
                ],
            ),
            create_bind_group(
                device,
                "bg haze airlight",
                airlight,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(16, &self.tone_stats),
                    buffer_binding(52, &haze.cells),
                    buffer_binding(53, &haze.airlight),
                ],
            ),
            create_bind_group(
                device,
                "bg haze transmission",
                transmission,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(52, &haze.cells),
                    buffer_binding(54, &haze.airlight),
                    texture_binding(55, &haze.map_view),
                ],
            ),
            create_bind_group(
                device,
                "bg haze guided-filter coefficients",
                coefficients,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(52, &haze.cells),
                    texture_binding(56, &haze.map_view),
                    texture_binding(57, &haze.work_view),
                ],
            ),
            create_bind_group(
                device,
                "bg haze coefficient smoothing",
                smooth,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(58, &haze.work_view),
                    texture_binding(59, &haze.map_view),
                ],
            ),
        ];
        let contours = create_bind_group(
            device,
            "bg contour accumulation",
            &self.contour_layout,
            &[
                buffer_binding(0, &self.camera_uniforms),
                texture_binding(11, &self.scene),
                buffer_binding(20, &self.profile),
                buffer_binding(78, &inputs.contours.cells),
            ],
        );
        let emission = inputs.scene_emission;
        let [emitters, surfaces, volume] = &self.scene_emission_layouts;
        let scene_emission = [
            create_bind_group(
                device,
                "bg scene emission emitters",
                emitters,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    sampler_binding(28, &self.mask_sampler),
                    buffer_binding(33, &self.mask_data),
                    texture_binding(34, &self.full_image_mask),
                    buffer_binding(85, inputs.scene_bake.uniforms),
                    texture_binding(113, inputs.scene_bake.layers),
                    buffer_binding(78, &inputs.contours.cells),
                    texture_binding(79, &emission.emitters_view),
                ],
            ),
            create_bind_group(
                device,
                "bg scene emission on surfaces",
                surfaces,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(16, &self.tone_stats),
                    buffer_binding(85, inputs.scene_bake.uniforms),
                    texture_binding(113, inputs.scene_bake.layers),
                    texture_binding(80, &emission.emitters_view),
                    texture_binding(81, &emission.light_view),
                ],
            ),
            create_bind_group(
                device,
                "bg scene emission volume",
                volume,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(16, &self.tone_stats),
                    buffer_binding(85, inputs.scene_bake.uniforms),
                    texture_binding(80, &emission.emitters_view),
                    texture_binding(81, &emission.light_view),
                ],
            ),
        ];
        InputBindGroups {
            scene_preparation,
            effects_from_tex1: effects("bg scene effects from tex1", &self.tex1, &self.tex2),
            effects_from_tex2: effects("bg scene effects from tex2", &self.tex2, &self.tex1),
            view: view("bg scene look view and output", &self.tex2),
            view_after_blur: view("bg scene look view and output after mask Blur", &self.tex1),
            image_lights,
            haze: haze_groups,
            contours,
            scene_emission,
            scene_bake_accumulation: create_bind_group(
                device,
                "bg scene bake accumulation",
                &self.scene_bake_accumulation_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(11, &self.scene),
                    buffer_binding(20, &self.profile),
                    buffer_binding(84, inputs.scene_bake.scratch),
                    buffer_binding(85, inputs.scene_bake.uniforms),
                ],
            ),
        }
    }
}

struct Allocated {
    image_lights: Option<ImageLightInputs>,
    haze: Option<HazeInputs>,
    contours: Option<ContourInputs>,
    scene_emission: Option<SceneEmissionInputs>,
    look_lut: Option<LookLutInputs>,
    /// The LUT that last failed to upload and why, so it is not retried for
    /// every render.
    look_lut_failure: Option<(LutId, String)>,
    /// This pipeline's scene bake, or the whole image's that a view of part
    /// of it shares.
    scene_bake: Option<Arc<SceneBake>>,
    /// The bake's light revision the bind groups hold (`SceneBake::lights_revision`).
    bound_lights_revision: u64,
    /// This pipeline's air light, when an effect reads it.
    air_light: Option<AirLightInputs>,
    bind_groups: InputBindGroups,
}

pub(super) struct EffectInputs {
    device: wgpu::Device,
    sources: InputBindingSources,
    placeholder_image_lights: ImageLightInputs,
    placeholder_haze: HazeInputs,
    placeholder_contours: ContourInputs,
    placeholder_scene_emission: SceneEmissionInputs,
    placeholder_look_lut: LookLutInputs,
    placeholder_scene_bake: PlaceholderSceneBake,
    placeholder_air_light: AirLightInputs,
    /// Compares with the shadow cubes (`SceneLightBindings`).
    scene_compare: wgpu::Sampler,
    /// The scene bake's and light bake's programs, compiled on this
    /// pipeline's first bake. Held here because `SceneBakePrograms::shared`
    /// keeps only weak references: without an owner every render compiled
    /// them again (about 45 ms each on a desktop GPU).
    scene_programs: Mutex<Option<Arc<SceneBakePrograms>>>,
    allocated: Mutex<Allocated>,
}

impl EffectInputs {
    /// Inputs with nothing allocated yet: passes bind the placeholders.
    pub(super) fn new(device: &wgpu::Device, sources: InputBindingSources) -> Self {
        let placeholder_image_lights = ImageLightInputs::new(device, 1);
        let placeholder_haze = HazeInputs::new(device, 1);
        let placeholder_contours = ContourInputs::new(device, 1);
        // Never read: receivers skip the light unless an effect emits it.
        let placeholder_scene_emission = SceneEmissionInputs::new(device, 1, 1, 1);
        // Never read: the view pass skips the LUT unless one was uploaded.
        let placeholder_look_lut = LookLutInputs::new(device, 1, None);
        let scene_compare = scene_compare_sampler(device);
        let placeholder_scene_bake = PlaceholderSceneBake::new(device, &scene_compare);
        let placeholder_air_light = AirLightInputs::new(device, [1, 1], 1, None);
        let bind_groups = sources.bind_groups(
            device,
            &BoundInputs {
                image_lights: &placeholder_image_lights,
                haze: &placeholder_haze,
                contours: &placeholder_contours,
                scene_emission: &placeholder_scene_emission,
                look_lut: &placeholder_look_lut,
                scene_bake: placeholder_scene_bake.bindings(),
                air_light: &placeholder_air_light,
            },
        );
        Self {
            device: device.clone(),
            sources,
            placeholder_image_lights,
            placeholder_haze,
            placeholder_contours,
            placeholder_scene_emission,
            placeholder_look_lut,
            placeholder_scene_bake,
            placeholder_air_light,
            scene_compare,
            scene_programs: Mutex::new(None),
            allocated: Mutex::new(Allocated {
                image_lights: None,
                haze: None,
                contours: None,
                scene_emission: None,
                look_lut: None,
                look_lut_failure: None,
                scene_bake: None,
                bound_lights_revision: 0,
                air_light: None,
                bind_groups,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Allocated> {
        self.allocated
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The current bind group of `pass`.
    /// `pass`'s bind group. The light bake allocates its visibility within the
    /// scene bake, which views of part of the image share, so the bind groups
    /// follow it here.
    pub(super) fn bind_group(&self, pass: InputBoundPass) -> wgpu::BindGroup {
        let mut allocated = self.lock();
        let revision = allocated
            .scene_bake
            .as_ref()
            .map_or(0, |bake| bake.lights_revision());
        if revision != allocated.bound_lights_revision {
            self.rebind(&mut allocated);
        }
        allocated.bind_groups.get(pass).clone()
    }

    /// Allocates the scene emission textures unless they exist.
    pub(super) fn allocate_scene_emission(&self) {
        let mut allocated = self.lock();
        if allocated.scene_emission.is_some() {
            return;
        }
        allocated.scene_emission = Some(SceneEmissionInputs::new(
            &self.device,
            SCENE_EMISSION_GRID_LONG,
            SCENE_EMISSION_SURFACE_GRID_LONG,
            MEDIUM_CELLS,
        ));
        self.rebind(&mut allocated);
    }

    /// Allocates `map`'s grid and outputs unless they exist.
    pub(super) fn allocate_full_image_map(&self, map: FullImageMap) {
        let mut allocated = self.lock();
        match map {
            FullImageMap::ImageLights => {
                if allocated.image_lights.is_some() {
                    return;
                }
                allocated.image_lights =
                    Some(ImageLightInputs::new(&self.device, IMAGE_LIGHT_GRID_LONG));
            }
            FullImageMap::Haze => {
                if allocated.haze.is_some() {
                    return;
                }
                allocated.haze = Some(HazeInputs::new(&self.device, HAZE_GRID_LONG));
            }
            FullImageMap::Contours => {
                if allocated.contours.is_some() {
                    return;
                }
                allocated.contours = Some(ContourInputs::new(&self.device, CONTOUR_GRID_LONG));
            }
        }
        self.rebind(&mut allocated);
    }

    /// Writes `lut` to the LUT texture unless it already holds it, first
    /// allocating a texture of the LUT's size when the current one differs.
    pub(super) fn upload_look_lut(&self, queue: &wgpu::Queue, lut: &CubeLut) -> Result<()> {
        let mut allocated = self.lock();
        if allocated
            .look_lut
            .as_ref()
            .is_some_and(|inputs| inputs.uploaded == Some(lut.id()))
        {
            return Ok(());
        }
        if let Some((id, message)) = &allocated.look_lut_failure {
            if *id == lut.id() {
                return Err(anyhow!("{message}"));
            }
        }
        if allocated
            .look_lut
            .as_ref()
            .is_none_or(|inputs| inputs.size != lut.size())
        {
            // Release the previous texture's reservation before taking one
            // for the new size.
            allocated.look_lut = None;
            let replacement = LookLutInputs::reserved(&self.device, lut.size());
            let failure = replacement.as_ref().err().map(|error| format!("{error:#}"));
            allocated.look_lut = replacement.ok();
            self.rebind(&mut allocated);
            if let Some(message) = failure {
                allocated.look_lut_failure = Some((lut.id(), message.clone()));
                return Err(anyhow!(message));
            }
        }
        if let Some(inputs) = allocated.look_lut.as_mut() {
            inputs.write(queue, lut);
        }
        allocated.look_lut_failure = None;
        Ok(())
    }

    fn rebind(&self, allocated: &mut Allocated) {
        let revision = allocated
            .scene_bake
            .as_ref()
            .map_or(0, |bake| bake.lights_revision());
        let scene_bake = allocated.scene_bake.clone().map(|bake| {
            let lights = SceneLightBindings::new(&self.device, Some(&bake), &self.scene_compare);
            (bake, lights)
        });
        allocated.bind_groups = self.sources.bind_groups(
            &self.device,
            &BoundInputs {
                image_lights: allocated
                    .image_lights
                    .as_ref()
                    .unwrap_or(&self.placeholder_image_lights),
                haze: allocated.haze.as_ref().unwrap_or(&self.placeholder_haze),
                contours: allocated
                    .contours
                    .as_ref()
                    .unwrap_or(&self.placeholder_contours),
                scene_emission: allocated
                    .scene_emission
                    .as_ref()
                    .unwrap_or(&self.placeholder_scene_emission),
                look_lut: allocated
                    .look_lut
                    .as_ref()
                    .unwrap_or(&self.placeholder_look_lut),
                scene_bake: match &scene_bake {
                    Some((bake, lights)) => bake.bindings(lights),
                    None => self.placeholder_scene_bake.bindings(),
                },
                air_light: allocated
                    .air_light
                    .as_ref()
                    .unwrap_or(&self.placeholder_air_light),
            },
        );
        allocated.bound_lights_revision = revision;
    }

    /// The scene bake of `size`, allocated unless one of that size exists.
    /// `None` when it does not fit the GPU budget.
    pub(super) fn allocate_scene_bake(&self, size: [u32; 2]) -> Result<Arc<SceneBake>> {
        let mut allocated = self.lock();
        if let Some(bake) = allocated
            .scene_bake
            .as_ref()
            .filter(|bake| bake.size() == size)
        {
            return Ok(Arc::clone(bake));
        }
        // Release the previous bake's reservation before taking one for the
        // new size.
        allocated.scene_bake = None;
        let bake = SceneBake::new(&self.device, size).map(Arc::new);
        allocated.scene_bake = bake.as_ref().ok().cloned();
        self.rebind(&mut allocated);
        bake
    }

    /// Encodes the air light of `region` for `groups` light groups into this
    /// pipeline's texture, allocating or growing it first (the process budget
    /// may refuse, which leaves the placeholder bound and no air light).
    pub(super) fn encode_air_light(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bake: &SceneBake,
        programs: &SceneLightPrograms,
        region: SceneAirLightRegion,
    ) {
        let groups = bake.air_light_groups();
        if groups == 0 {
            return;
        }
        let mut allocated = self.lock();
        let fits = allocated
            .air_light
            .as_ref()
            .is_some_and(|air| air.size == region.size && air.groups == groups);
        if !fits {
            allocated.air_light = None;
            let reservation = GpuBudgetReservation::acquire_bytes(
                "scene air light",
                AirLightInputs::bytes(region.size, groups),
                gpu_working_set_limit_bytes(),
            );
            match reservation {
                Ok(reservation) => {
                    allocated.air_light = Some(AirLightInputs::new(
                        &self.device,
                        region.size,
                        groups,
                        Some(reservation),
                    ));
                }
                Err(error) => log::error!("scene air light not allocated: {error:#}"),
            }
            self.rebind(&mut allocated);
        }
        let Some(air) = allocated.air_light.as_mut() else {
            return;
        };
        let target = SceneAirLightTarget {
            texture: &air.texture,
            uniforms: &air.uniforms,
        };
        let built =
            bake.encode_air_light(&self.device, encoder, programs, &target, region, air.built);
        air.built = built;
    }

    /// Shares `bake`, the whole image's, with this view of part of it.
    pub(super) fn share_scene_bake(&self, bake: &Arc<SceneBake>) {
        let mut allocated = self.lock();
        if allocated
            .scene_bake
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, bake))
        {
            return;
        }
        allocated.scene_bake = Some(Arc::clone(bake));
        self.rebind(&mut allocated);
    }

    pub(super) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The scene programs of this pipeline's device, compiled on first use
    /// and kept for the pipeline's lifetime.
    pub(super) fn scene_programs(&self) -> Result<Arc<SceneBakePrograms>> {
        let mut programs = self
            .scene_programs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(programs) = programs.as_ref() {
            return Ok(Arc::clone(programs));
        }
        let shared = SceneBakePrograms::shared(&self.device)?;
        *programs = Some(Arc::clone(&shared));
        Ok(shared)
    }

    /// The scene bake, once allocated or shared.
    pub(super) fn scene_bake(&self) -> Option<Arc<SceneBake>> {
        self.lock().scene_bake.clone()
    }

    /// `map`'s grid and outputs, once allocated.
    pub(super) fn full_image_map(&self, map: FullImageMap) -> Option<FullImageMapResources> {
        let allocated = self.lock();
        match map {
            FullImageMap::ImageLights => {
                allocated
                    .image_lights
                    .as_ref()
                    .map(|lights| FullImageMapResources {
                        cells: lights.cells.clone(),
                        textures: vec![lights.map.clone()],
                        buffers: Vec::new(),
                    })
            }
            FullImageMap::Haze => allocated.haze.as_ref().map(|haze| FullImageMapResources {
                cells: haze.cells.clone(),
                textures: vec![haze.map.clone()],
                buffers: vec![haze.airlight.clone()],
            }),
            FullImageMap::Contours => {
                allocated
                    .contours
                    .as_ref()
                    .map(|contours| FullImageMapResources {
                        cells: contours.cells.clone(),
                        textures: Vec::new(),
                        buffers: vec![contours.cells.clone()],
                    })
            }
        }
    }
}
