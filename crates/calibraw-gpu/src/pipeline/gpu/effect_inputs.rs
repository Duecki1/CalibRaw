//! GPU inputs that only some effects read, allocated the first time one of
//! them renders: the joint-upsampling guide of scene depth that Fog, Smoke and
//! Relight read, Relight's scene-depth surface and shadow map, and the
//! image-light map that Fog and Smoke scatter.
//!
//! Until then the passes that read them are bound to placeholders of one
//! texel, and Fog reads the plain scene-depth texture, so a pipeline whose
//! edits never use these effects holds none of their memory. The process
//! budget reserves them with the pipeline (`GpuResourceResidency::OnDemand`),
//! so allocating them later cannot exceed it. Once allocated they stay until
//! the pipeline is dropped.
//!
//! Passes bound to these inputs are tagged with an [`InputBoundPass`]; the
//! pipeline takes their bind groups from [`EffectInputs::bind_group`], which
//! follow the allocation.

use super::*;

/// The image-light passes: accumulate, resolve, horizontal and vertical blur.
pub(super) const IMAGE_LIGHT_PASSES: usize = 4;

/// A pass whose bind group follows the allocated effect inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputBoundPass {
    Creative,
    CreativeAfterBlur,
    SceneDepthGuide,
    SceneDepthGuideAfterBlur,
    RelightShadowMap,
    /// One of the image-light passes, in pass order.
    ImageLights(usize),
}

/// Bind groups of every [`InputBoundPass`].
struct InputBindGroups {
    creative: wgpu::BindGroup,
    creative_after_blur: wgpu::BindGroup,
    scene_depth_guide: wgpu::BindGroup,
    scene_depth_guide_after_blur: wgpu::BindGroup,
    relight_shadow_map: wgpu::BindGroup,
    image_lights: [wgpu::BindGroup; IMAGE_LIGHT_PASSES],
}

impl InputBindGroups {
    fn get(&self, pass: InputBoundPass) -> &wgpu::BindGroup {
        match pass {
            InputBoundPass::Creative => &self.creative,
            InputBoundPass::CreativeAfterBlur => &self.creative_after_blur,
            InputBoundPass::SceneDepthGuide => &self.scene_depth_guide,
            InputBoundPass::SceneDepthGuideAfterBlur => &self.scene_depth_guide_after_blur,
            InputBoundPass::RelightShadowMap => &self.relight_shadow_map,
            InputBoundPass::ImageLights(index) => &self.image_lights[index],
        }
    }
}

/// The pipeline's own layouts, buffers and views that the input-bound passes
/// also bind. Handles are reference-counted, so keeping them is cheap.
pub(super) struct InputBindingSources {
    pub(super) creative_layout: wgpu::BindGroupLayout,
    pub(super) scene_depth_guide_layout: wgpu::BindGroupLayout,
    pub(super) relight_shadow_map_layout: wgpu::BindGroupLayout,
    pub(super) image_light_layouts: [wgpu::BindGroupLayout; IMAGE_LIGHT_PASSES],
    pub(super) camera_uniforms: wgpu::Buffer,
    pub(super) mask_data: wgpu::Buffer,
    pub(super) tone_stats: wgpu::Buffer,
    pub(super) profile: wgpu::Buffer,
    pub(super) scene: wgpu::TextureView,
    pub(super) tex1: wgpu::TextureView,
    pub(super) tex2: wgpu::TextureView,
    pub(super) display_linear: wgpu::TextureView,
    pub(super) mask: wgpu::TextureView,
    pub(super) mask_sampler: wgpu::Sampler,
    pub(super) light_rays_mask: wgpu::TextureView,
    pub(super) pixelate_blocks: wgpu::TextureView,
    /// Stored scene depth alone, which Fog and Smoke read until the Relight
    /// surface, holding the same depth in its first channel, replaces it.
    pub(super) scene_depth: wgpu::TextureView,
}

/// Relight's scene-depth surface with its mip chain (`scene_surface`) and its
/// shadow map (relight.wgsl).
struct RelightInputs {
    surface: wgpu::Texture,
    surface_view: wgpu::TextureView,
    _shadow_map: wgpu::Texture,
    shadow_map_view: wgpu::TextureView,
}

impl RelightInputs {
    fn new(device: &wgpu::Device) -> Self {
        let surface = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw Relight scene-depth surface"),
            size: texture_size(SCENE_DEPTH_EDGE, SCENE_DEPTH_EDGE),
            mip_level_count: SCENE_DEPTH_MIP_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RELIGHT_SURFACE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[RELIGHT_SURFACE_FORMAT],
        });
        let (shadow_map, shadow_map_view) = shadow_map(device, SCENE_DEPTH_EDGE);
        Self {
            surface_view: default_texture_view(&surface),
            surface,
            _shadow_map: shadow_map,
            shadow_map_view,
        }
    }
}

/// A texture on the level-0 scene-depth grid that a pass writes and others
/// read, with `edge` × `edge` texels.
fn depth_grid_map(
    device: &wgpu::Device,
    edge: u32,
    format: wgpu::TextureFormat,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = create_processing_texture(
        device,
        texture_size(edge, edge),
        format,
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        label,
    );
    let view = default_texture_view(&texture);
    (texture, view)
}

fn scene_depth_guide(device: &wgpu::Device, edge: u32) -> (wgpu::Texture, wgpu::TextureView) {
    depth_grid_map(
        device,
        edge,
        SCENE_DEPTH_GUIDE_FORMAT,
        "calibraw full-image scene-depth guide",
    )
}

fn shadow_map(device: &wgpu::Device, edge: u32) -> (wgpu::Texture, wgpu::TextureView) {
    depth_grid_map(
        device,
        edge,
        RELIGHT_SHADOW_MAP_FORMAT,
        "calibraw full-image Relight shadow map",
    )
}

/// The image-light grid and its maps (tone_analysis.wgsl).
pub(super) struct ImageLightInputs {
    pub(super) cells: wgpu::Buffer,
    /// The resolved and finally blurred map Fog and Smoke sample; cropped and
    /// zoomed views copy it from the full frame.
    pub(super) map: wgpu::Texture,
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

/// Views the input-bound passes bind, allocated or placeholder.
struct BoundInputs<'a> {
    /// Scene depth: the Relight surface once allocated, else the plain depth.
    scene_depth: &'a wgpu::TextureView,
    scene_depth_guide: &'a wgpu::TextureView,
    shadow_map: &'a wgpu::TextureView,
    image_lights: &'a ImageLightInputs,
}

impl InputBindingSources {
    fn bind_groups(&self, device: &wgpu::Device, inputs: &BoundInputs<'_>) -> InputBindGroups {
        let creative = |label: &str, input: &wgpu::TextureView, output: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                &self.creative_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(24, input),
                    texture_binding(25, output),
                    texture_binding(30, &self.display_linear),
                    texture_binding(27, &self.mask),
                    sampler_binding(28, &self.mask_sampler),
                    buffer_binding(33, &self.mask_data),
                    texture_binding(34, &self.light_rays_mask),
                    texture_binding(35, inputs.scene_depth),
                    buffer_binding(16, &self.tone_stats),
                    texture_binding(36, &self.pixelate_blocks),
                    texture_binding(45, &inputs.image_lights.map_view),
                    texture_binding(46, inputs.shadow_map),
                    texture_binding(48, inputs.scene_depth_guide),
                ],
            )
        };
        let scene_depth_guide = |label: &str, input: &wgpu::TextureView| {
            create_bind_group(
                device,
                label,
                &self.scene_depth_guide_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    texture_binding(24, input),
                    texture_binding(35, inputs.scene_depth),
                    texture_binding(49, inputs.scene_depth_guide),
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
        InputBindGroups {
            creative: creative("bg creative glow", &self.tex1, &self.tex2),
            creative_after_blur: creative(
                "bg creative effects after mask Blur",
                &self.tex2,
                &self.tex1,
            ),
            scene_depth_guide: scene_depth_guide("bg scene-depth guide", &self.tex1),
            scene_depth_guide_after_blur: scene_depth_guide(
                "bg scene-depth guide after mask Blur",
                &self.tex2,
            ),
            relight_shadow_map: create_bind_group(
                device,
                "bg relight shadow map",
                &self.relight_shadow_map_layout,
                &[
                    buffer_binding(0, &self.camera_uniforms),
                    buffer_binding(33, &self.mask_data),
                    texture_binding(35, inputs.scene_depth),
                    texture_binding(47, inputs.shadow_map),
                ],
            ),
            image_lights,
        }
    }
}

struct Allocated {
    /// The scene-depth guide texture and its view.
    scene_depth_guide: Option<(wgpu::Texture, wgpu::TextureView)>,
    relight: Option<RelightInputs>,
    image_lights: Option<ImageLightInputs>,
    bind_groups: InputBindGroups,
}

pub(super) struct EffectInputs {
    device: wgpu::Device,
    sources: InputBindingSources,
    _placeholder_scene_depth_guide: wgpu::Texture,
    placeholder_scene_depth_guide_view: wgpu::TextureView,
    _placeholder_shadow_map: wgpu::Texture,
    placeholder_shadow_map_view: wgpu::TextureView,
    placeholder_image_lights: ImageLightInputs,
    allocated: Mutex<Allocated>,
}

impl EffectInputs {
    /// Inputs with nothing allocated yet: passes bind the placeholders.
    pub(super) fn new(device: &wgpu::Device, sources: InputBindingSources) -> Self {
        let (placeholder_scene_depth_guide, placeholder_scene_depth_guide_view) =
            scene_depth_guide(device, 1);
        let (placeholder_shadow_map, placeholder_shadow_map_view) = shadow_map(device, 1);
        let placeholder_image_lights = ImageLightInputs::new(device, 1);
        let bind_groups = sources.bind_groups(
            device,
            &BoundInputs {
                scene_depth: &sources.scene_depth,
                scene_depth_guide: &placeholder_scene_depth_guide_view,
                shadow_map: &placeholder_shadow_map_view,
                image_lights: &placeholder_image_lights,
            },
        );
        Self {
            device: device.clone(),
            sources,
            _placeholder_scene_depth_guide: placeholder_scene_depth_guide,
            placeholder_scene_depth_guide_view,
            _placeholder_shadow_map: placeholder_shadow_map,
            placeholder_shadow_map_view,
            placeholder_image_lights,
            allocated: Mutex::new(Allocated {
                scene_depth_guide: None,
                relight: None,
                image_lights: None,
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
    pub(super) fn bind_group(&self, pass: InputBoundPass) -> wgpu::BindGroup {
        self.lock().bind_groups.get(pass).clone()
    }

    /// Allocates Relight's inputs unless they exist. Returns whether they
    /// were allocated now: scene depth must then be uploaded to the surface.
    pub(super) fn allocate_relight(&self) -> bool {
        let mut allocated = self.lock();
        if allocated.relight.is_some() {
            return false;
        }
        allocated.relight = Some(RelightInputs::new(&self.device));
        self.rebind(&mut allocated);
        true
    }

    /// Allocates the scene-depth guide unless it exists.
    pub(super) fn allocate_scene_depth_guide(&self) {
        let mut allocated = self.lock();
        if allocated.scene_depth_guide.is_some() {
            return;
        }
        allocated.scene_depth_guide = Some(scene_depth_guide(&self.device, SCENE_DEPTH_EDGE));
        self.rebind(&mut allocated);
    }

    /// Allocates the image-light grid and maps unless they exist.
    pub(super) fn allocate_image_lights(&self) {
        let mut allocated = self.lock();
        if allocated.image_lights.is_some() {
            return;
        }
        allocated.image_lights = Some(ImageLightInputs::new(&self.device, IMAGE_LIGHT_GRID_LONG));
        self.rebind(&mut allocated);
    }

    fn rebind(&self, allocated: &mut Allocated) {
        let relight = allocated.relight.as_ref();
        allocated.bind_groups = self.sources.bind_groups(
            &self.device,
            &BoundInputs {
                scene_depth: relight
                    .map_or(&self.sources.scene_depth, |relight| &relight.surface_view),
                scene_depth_guide: allocated
                    .scene_depth_guide
                    .as_ref()
                    .map_or(&self.placeholder_scene_depth_guide_view, |(_, view)| view),
                shadow_map: relight.map_or(&self.placeholder_shadow_map_view, |relight| {
                    &relight.shadow_map_view
                }),
                image_lights: allocated
                    .image_lights
                    .as_ref()
                    .unwrap_or(&self.placeholder_image_lights),
            },
        );
    }

    /// The Relight surface, once allocated. Scene depth is uploaded to it
    /// instead of the plain depth texture from then on.
    pub(super) fn relight_surface(&self) -> Option<wgpu::Texture> {
        self.lock()
            .relight
            .as_ref()
            .map(|relight| relight.surface.clone())
    }

    /// The image-light grid buffer and map, once allocated.
    pub(super) fn image_lights(&self) -> Option<(wgpu::Buffer, wgpu::Texture)> {
        self.lock()
            .image_lights
            .as_ref()
            .map(|lights| (lights.cells.clone(), lights.map.clone()))
    }
}

#[cfg(test)]
impl EffectInputs {
    pub(super) fn scene_depth_guide_allocated(&self) -> bool {
        self.lock().scene_depth_guide.is_some()
    }
}
