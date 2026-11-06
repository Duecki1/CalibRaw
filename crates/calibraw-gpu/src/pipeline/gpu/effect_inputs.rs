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

use super::*;

/// The image-light passes: accumulate, resolve, horizontal and vertical blur.
pub(super) const IMAGE_LIGHT_PASSES: usize = 4;
/// The haze passes (haze_analysis.wgsl): accumulate, airlight, transmission
/// and the guided filter's two window passes.
pub(super) const HAZE_PASSES: usize = 5;
/// The scene map passes (scene_analysis.wgsl): accumulate, resolve,
/// coefficient smoothing, the environment blur, the ground and the lens blur.
pub(super) const SCENE_MAP_PASSES: usize = 6;
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
    /// Edge-accurate depth, the photo's environment, the ground and the lens
    /// blur that every effect reading the scene shares (scene_analysis.wgsl).
    Scene,
    /// How much of the photo lies on its contours, for Neon's scene light
    /// (tone_analysis.wgsl). Views copy the grid itself; it has no resolve.
    Contours,
}

impl FullImageMap {
    pub(super) const ALL: [Self; 4] = [Self::ImageLights, Self::Haze, Self::Scene, Self::Contours];
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
    SceneShadowMap,
    /// One of the image-light passes, in pass order.
    ImageLights(usize),
    /// One of the haze passes, in pass order.
    Haze(usize),
    /// One of the scene map passes, in pass order.
    SceneMap(usize),
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
    scene_shadow_map: wgpu::BindGroup,
    image_lights: [wgpu::BindGroup; IMAGE_LIGHT_PASSES],
    haze: [wgpu::BindGroup; HAZE_PASSES],
    scene_map: [wgpu::BindGroup; SCENE_MAP_PASSES],
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
            InputBoundPass::SceneShadowMap => &self.scene_shadow_map,
            InputBoundPass::ImageLights(index) => &self.image_lights[index],
            InputBoundPass::Haze(index) => &self.haze[index],
            InputBoundPass::SceneMap(index) => &self.scene_map[index],
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
    pub(super) scene_shadow_map_layout: wgpu::BindGroupLayout,
    pub(super) image_light_layouts: [wgpu::BindGroupLayout; IMAGE_LIGHT_PASSES],
    pub(super) haze_layouts: [wgpu::BindGroupLayout; HAZE_PASSES],
    pub(super) scene_map_layouts: [wgpu::BindGroupLayout; SCENE_MAP_PASSES],
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

/// Scene depth with its mip chain (`scene_surface`), of `edge` × `edge` texels
/// and `mip_levels` levels.
struct SceneDepthInputs {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl SceneDepthInputs {
    fn new(device: &wgpu::Device, edge: u32, mip_levels: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw full-image scene depth"),
            size: texture_size(edge, edge),
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[SCENE_DEPTH_FORMAT],
        });
        Self {
            view: default_texture_view(&texture),
            texture,
        }
    }
}

/// The scene shadow map (scene_shadows.wgsl): the surface on the level-0
/// scene-depth grid of `edge` × `edge` texels, and below it the air slices
/// (`scene_shadow_map_height`).
struct SceneShadowMapInputs {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl SceneShadowMapInputs {
    fn new(device: &wgpu::Device, edge: u32) -> Self {
        let texture = create_processing_texture(
            device,
            texture_size(edge, scene_shadow_map_height(edge)),
            SCENE_SHADOW_MAP_FORMAT,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            "calibraw full-image scene shadow map",
        );
        Self {
            view: default_texture_view(&texture),
            _texture: texture,
        }
    }
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

/// The scene map's grid and its maps (scene_analysis.wgsl).
struct SceneMapInputs {
    cells: wgpu::Buffer,
    /// The coarse photo, its wide blur, depth's coefficients with the sky's
    /// luminance, depth's range, the ground and the lens blur table effects
    /// read; cropped and zoomed views copy them.
    environment: wgpu::Texture,
    environment_view: wgpu::TextureView,
    environment_blur: wgpu::Texture,
    environment_blur_view: wgpu::TextureView,
    // depth-rework-bridge: delete in P7. Depth's coefficients and sky
    // luminance, depth's range, the ground and the lens blur table, as layers
    // of one array (`SCENE_MAP_*_LAYER` in scene_depth.wgsl), which effects
    // read through `maps_view` and the scene map passes write through one view
    // per layer.
    maps: wgpu::Texture,
    maps_view: wgpu::TextureView,
    depth_map_view: wgpu::TextureView,
    depth_range_view: wgpu::TextureView,
    ground_view: wgpu::TextureView,
    focus_view: wgpu::TextureView,
    _coefficients: wgpu::Texture,
    /// The guided filter's coefficients of one window, before averaging.
    coefficients_view: wgpu::TextureView,
}

impl SceneMapInputs {
    /// Inputs for a grid of `edge` × `edge` cells and a blur of `blur_edge`.
    fn new(device: &wgpu::Device, edge: u32, blur_edge: u32) -> Self {
        let usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
        let copied = usage | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        let blocks = |edge, blocks, format, usage, label| {
            let texture = create_processing_texture(
                device,
                texture_size(edge, edge * blocks),
                format,
                usage,
                label,
            );
            let view = default_texture_view(&texture);
            (texture, view)
        };
        let texture = |edge, format, usage, label| blocks(edge, 1, format, usage, label);
        let (environment, environment_view) = texture(
            edge,
            SCENE_ENVIRONMENT_FORMAT,
            copied,
            "calibraw full-image scene environment",
        );
        let (environment_blur, environment_blur_view) = texture(
            blur_edge,
            SCENE_ENVIRONMENT_FORMAT,
            copied,
            "calibraw full-image scene environment blur",
        );
        // The largest map, the depth map's blocks, sets the layers' size; the
        // lens blur needs SCENE_FOCUS_BINS texels of a row.
        let maps = create_processing_texture(
            device,
            wgpu::Extent3d {
                width: edge.max(SCENE_FOCUS_BINS),
                height: edge * SCENE_MAP_COEFFICIENT_BLOCKS,
                depth_or_array_layers: SCENE_MAP_LAYERS,
            },
            SCENE_COEFFICIENT_FORMAT,
            copied,
            "calibraw full-image scene maps",
        );
        let maps_view = maps.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw full-image scene maps"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_view = |layer: u32| {
            maps.create_view(&wgpu::TextureViewDescriptor {
                label: Some("calibraw full-image scene map layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let [depth_map_view, depth_range_view, ground_view, focus_view] =
            [0, 1, 2, 3].map(layer_view);
        let (coefficients, coefficients_view) = blocks(
            edge,
            SCENE_MAP_COEFFICIENT_BLOCKS,
            SCENE_COEFFICIENT_FORMAT,
            usage,
            "calibraw scene map guided-filter coefficients",
        );
        Self {
            cells: create_gpu_buffer(
                device,
                "calibraw scene map grid",
                scene_map_cells_bytes(edge),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            environment,
            environment_view,
            environment_blur,
            environment_blur_view,
            maps,
            maps_view,
            depth_map_view,
            depth_range_view,
            ground_view,
            focus_view,
            _coefficients: coefficients,
            coefficients_view,
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
    scene_depth: &'a SceneDepthInputs,
    scene_map: &'a SceneMapInputs,
    shadow_map: &'a SceneShadowMapInputs,
    image_lights: &'a ImageLightInputs,
    haze: &'a HazeInputs,
    contours: &'a ContourInputs,
    scene_emission: &'a SceneEmissionInputs,
    look_lut: &'a LookLutInputs,
    scene_bake: SceneBakeBindings<'a>,
}

/// What passes bind of the scene bake: the scratch buffer and uniforms the
/// working-image accumulation writes, and the bake's layers and the light
/// bake's visibility that effects read (scene/geometry.wgsl).
#[derive(Clone, Copy)]
pub(super) struct SceneBakeBindings<'a> {
    pub(super) scratch: &'a wgpu::Buffer,
    pub(super) uniforms: &'a wgpu::Buffer,
    pub(super) layers: &'a wgpu::TextureView,
    pub(super) visibility: &'a wgpu::TextureView,
}

/// Bound for the scene bake until it exists. Never read: the accumulation runs
/// only for an allocated bake, and effects read the bake only when its
/// uniforms hold a working size, which these leave at zero.
struct PlaceholderSceneBake {
    scratch: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    layers: wgpu::TextureView,
    visibility: wgpu::TextureView,
}

impl PlaceholderSceneBake {
    fn new(device: &wgpu::Device) -> Self {
        let array = |format, layers, label| {
            let texture = create_processing_texture(
                device,
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: layers,
                },
                format,
                wgpu::TextureUsages::TEXTURE_BINDING,
                label,
            );
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
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
            layers: array(
                wgpu::TextureFormat::Rgba16Float,
                SCENE_BAKE_LAYERS,
                "calibraw scene bake layers placeholder",
            ),
            visibility: array(
                wgpu::TextureFormat::Rgba8Unorm,
                2,
                "calibraw scene visibility placeholder",
            ),
        }
    }

    fn bindings(&self) -> SceneBakeBindings<'_> {
        SceneBakeBindings {
            scratch: &self.scratch,
            uniforms: &self.uniforms,
            layers: &self.layers,
            visibility: &self.visibility,
        }
    }
}

impl InputBindingSources {
    fn bind_groups(&self, device: &wgpu::Device, inputs: &BoundInputs<'_>) -> InputBindGroups {
        let haze = inputs.haze;
        let scene_depth = &inputs.scene_depth.view;
        let scene_map = inputs.scene_map;
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
                    texture_binding(35, scene_depth),
                    buffer_binding(16, &self.tone_stats),
                    texture_binding(36, &self.pixelate_blocks),
                    texture_binding(45, &inputs.image_lights.map_view),
                    texture_binding(46, &inputs.shadow_map.view),
                    texture_binding(69, &scene_map.environment_view),
                    texture_binding(70, &scene_map.environment_blur_view),
                    // depth-rework-bridge: delete in P7.
                    texture_binding(71, &scene_map.maps_view),
                    texture_binding(83, &inputs.scene_emission.light_view),
                    buffer_binding(85, inputs.scene_bake.uniforms),
                    texture_binding(112, inputs.scene_bake.visibility),
                    texture_binding(113, inputs.scene_bake.layers),
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
        let [accumulate, resolve, smooth, blur, ground, focus] = &self.scene_map_layouts;
        let scene_map_groups = [
            create_bind_group(
                device,
                "bg scene map accumulation",
                accumulate,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(11, &self.scene),
                    buffer_binding(20, &self.profile),
                    texture_binding(35, scene_depth),
                    buffer_binding(62, &scene_map.cells),
                ],
            ),
            create_bind_group(
                device,
                "bg scene map resolve",
                resolve,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(62, &scene_map.cells),
                    texture_binding(63, &scene_map.environment_view),
                    texture_binding(64, &scene_map.coefficients_view),
                    texture_binding(76, &scene_map.depth_range_view),
                ],
            ),
            create_bind_group(
                device,
                "bg scene map coefficient smoothing",
                smooth,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(65, &scene_map.coefficients_view),
                    texture_binding(66, &scene_map.depth_map_view),
                ],
            ),
            create_bind_group(
                device,
                "bg scene map environment blur",
                blur,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(67, &scene_map.environment_view),
                    texture_binding(68, &scene_map.environment_blur_view),
                ],
            ),
            create_bind_group(
                device,
                "bg scene map ground",
                ground,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    sampler_binding(28, &self.mask_sampler),
                    texture_binding(35, scene_depth),
                    texture_binding(67, &scene_map.environment_view),
                    texture_binding(72, &scene_map.ground_view),
                ],
            ),
            create_bind_group(
                device,
                "bg scene map lens blur",
                focus,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(62, &scene_map.cells),
                    texture_binding(74, &scene_map.focus_view),
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
                    texture_binding(35, scene_depth),
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
                    sampler_binding(28, &self.mask_sampler),
                    texture_binding(35, scene_depth),
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
            scene_shadow_map: create_bind_group(
                device,
                "bg scene shadow map",
                &self.scene_shadow_map_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(33, &self.mask_data),
                    texture_binding(35, scene_depth),
                    texture_binding(47, &inputs.shadow_map.view),
                ],
            ),
            image_lights,
            haze: haze_groups,
            scene_map: scene_map_groups,
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
    scene_depth: Option<SceneDepthInputs>,
    scene_map: Option<SceneMapInputs>,
    scene_shadow_map: Option<SceneShadowMapInputs>,
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
    bind_groups: InputBindGroups,
}

pub(super) struct EffectInputs {
    device: wgpu::Device,
    sources: InputBindingSources,
    placeholder_scene_depth: SceneDepthInputs,
    placeholder_scene_map: SceneMapInputs,
    placeholder_shadow_map: SceneShadowMapInputs,
    placeholder_image_lights: ImageLightInputs,
    placeholder_haze: HazeInputs,
    placeholder_contours: ContourInputs,
    placeholder_scene_emission: SceneEmissionInputs,
    placeholder_look_lut: LookLutInputs,
    placeholder_scene_bake: PlaceholderSceneBake,
    allocated: Mutex<Allocated>,
}

impl EffectInputs {
    /// Inputs with nothing allocated yet: passes bind the placeholders.
    pub(super) fn new(device: &wgpu::Device, sources: InputBindingSources) -> Self {
        let placeholder_scene_depth = SceneDepthInputs::new(device, 1, 1);
        let placeholder_scene_map = SceneMapInputs::new(device, 1, 1);
        let placeholder_shadow_map = SceneShadowMapInputs::new(device, 1);
        let placeholder_image_lights = ImageLightInputs::new(device, 1);
        let placeholder_haze = HazeInputs::new(device, 1);
        let placeholder_contours = ContourInputs::new(device, 1);
        // Never read: receivers skip the light unless an effect emits it.
        let placeholder_scene_emission = SceneEmissionInputs::new(device, 1, 1, 1);
        // Never read: the view pass skips the LUT unless one was uploaded.
        let placeholder_look_lut = LookLutInputs::new(device, 1, None);
        let placeholder_scene_bake = PlaceholderSceneBake::new(device);
        let bind_groups = sources.bind_groups(
            device,
            &BoundInputs {
                scene_depth: &placeholder_scene_depth,
                scene_map: &placeholder_scene_map,
                shadow_map: &placeholder_shadow_map,
                image_lights: &placeholder_image_lights,
                haze: &placeholder_haze,
                contours: &placeholder_contours,
                scene_emission: &placeholder_scene_emission,
                look_lut: &placeholder_look_lut,
                scene_bake: placeholder_scene_bake.bindings(),
            },
        );
        Self {
            device: device.clone(),
            sources,
            placeholder_scene_depth,
            placeholder_scene_map,
            placeholder_shadow_map,
            placeholder_image_lights,
            placeholder_haze,
            placeholder_contours,
            placeholder_scene_emission,
            placeholder_look_lut,
            placeholder_scene_bake,
            allocated: Mutex::new(Allocated {
                scene_depth: None,
                scene_map: None,
                scene_shadow_map: None,
                image_lights: None,
                haze: None,
                contours: None,
                scene_emission: None,
                look_lut: None,
                look_lut_failure: None,
                scene_bake: None,
                bound_lights_revision: 0,
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

    /// Allocates scene depth unless it exists. Nothing is uploaded to it before
    /// that, so `upload_scene_depth` fills it on the first upload after this.
    pub(super) fn allocate_scene_depth(&self) {
        let mut allocated = self.lock();
        if allocated.scene_depth.is_some() {
            return;
        }
        allocated.scene_depth = Some(SceneDepthInputs::new(
            &self.device,
            SCENE_DEPTH_EDGE,
            SCENE_DEPTH_MIP_LEVELS,
        ));
        self.rebind(&mut allocated);
    }

    /// Allocates the scene shadow map unless it exists.
    pub(super) fn allocate_scene_shadow_map(&self) {
        let mut allocated = self.lock();
        if allocated.scene_shadow_map.is_some() {
            return;
        }
        allocated.scene_shadow_map =
            Some(SceneShadowMapInputs::new(&self.device, SCENE_DEPTH_EDGE));
        self.rebind(&mut allocated);
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
            FullImageMap::Scene => {
                if allocated.scene_map.is_some() {
                    return;
                }
                allocated.scene_map = Some(SceneMapInputs::new(
                    &self.device,
                    SCENE_MAP_GRID_LONG,
                    SCENE_MAP_BLUR_GRID_LONG,
                ));
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
        let placeholder = &self.placeholder_scene_bake;
        let revision = allocated
            .scene_bake
            .as_ref()
            .map_or(0, |bake| bake.lights_revision());
        let scene_bake = allocated.scene_bake.clone().map(|bake| {
            let visibility = bake.visibility_texture().map(|texture| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("calibraw scene visibility"),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
            });
            (bake, visibility)
        });
        allocated.bind_groups = self.sources.bind_groups(
            &self.device,
            &BoundInputs {
                scene_depth: allocated
                    .scene_depth
                    .as_ref()
                    .unwrap_or(&self.placeholder_scene_depth),
                scene_map: allocated
                    .scene_map
                    .as_ref()
                    .unwrap_or(&self.placeholder_scene_map),
                shadow_map: allocated
                    .scene_shadow_map
                    .as_ref()
                    .unwrap_or(&self.placeholder_shadow_map),
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
                    Some((bake, visibility)) => {
                        bake.bindings(visibility.as_ref().unwrap_or(&placeholder.visibility))
                    }
                    None => placeholder.bindings(),
                },
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

    /// The scene bake, once allocated or shared.
    pub(super) fn scene_bake(&self) -> Option<Arc<SceneBake>> {
        self.lock().scene_bake.clone()
    }

    /// The scene-depth texture, once allocated.
    pub(super) fn scene_depth_texture(&self) -> Option<wgpu::Texture> {
        self.lock()
            .scene_depth
            .as_ref()
            .map(|inputs| inputs.texture.clone())
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
            FullImageMap::Scene => {
                allocated
                    .scene_map
                    .as_ref()
                    .map(|scene_map| FullImageMapResources {
                        cells: scene_map.cells.clone(),
                        textures: vec![
                            scene_map.environment.clone(),
                            scene_map.environment_blur.clone(),
                            scene_map.maps.clone(),
                        ],
                        buffers: Vec::new(),
                    })
            }
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
