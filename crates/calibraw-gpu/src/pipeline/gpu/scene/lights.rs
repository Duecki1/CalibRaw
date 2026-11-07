//! The light bake (docs/rework/DEPTH_EFFECTS_PLAN.md §4.4): per light that
//! casts shadows a shadow cube of the scene's visible surface (L1,
//! `shaders/scene/shadow_raster.wgsl`), and from the cubes the visibility of
//! every surface of the scene bake (L2, `visibility.wgsl`). Conventions are
//! documented in `lights_common.wgsl`.
//!
//! From the cubes, each pipeline marches its own region's view rays for the
//! share of each light the air receives (L3, `air_light.wgsl`); that result
//! is the pipeline's ([`SceneAirLightKey`] says what it was built from).
//!
//! It belongs to the [`SceneBake`] it is made from, so the views that share
//! a bake share its lights. It is rebuilt only for what changed:
//! - the mesh the cubes are drawn from, when the scene bake is baked again;
//! - a light's cube, when the scene, the light's position or relief scale, or
//!   the quality tier changes;
//! - a group's visibility, when one of its cubes or a light's Size changes.
//!
//! Amount, colour, reach, Ambient and the shadow strength are applied per
//! pixel and rebuild nothing.
//!
//! Faces that see no part of the scene (`face_sees_scene`) are cleared but not
//! drawn: no surface lies in their direction, so no receiver reads them.

use super::super::shader_manager::ShaderManager;
use super::super::{
    buffer_entry, create_bind_group, create_bind_group_layout, create_compute_pipeline,
    create_gpu_buffer, dispatch_compute, gpu_working_set_limit_bytes, shaders,
    storage_texture_entry, texture_size, GpuBudgetReservation, WORKGROUP_EDGE,
};
use super::{SceneBake, FULL_BAKE_BUDGET};
use anyhow::Result;
use calibraw_core::pipeline::SceneCamera;

/// Lights the bake holds: the first Relight lights that cast shadows, in slot
/// order (`SCENE_SHADOW_LIGHTS` in lights_common.wgsl).
pub(in crate::pipeline::gpu) const SCENE_SHADOW_LIGHTS: usize = 8;
/// Lights per layer pair of the visibility array, one per RGBA channel.
const LIGHTS_PER_GROUP: usize = 4;
const VISIBILITY_GROUPS: usize = SCENE_SHADOW_LIGHTS / LIGHTS_PER_GROUP;
const CUBE_FACES: u32 = 6;
/// `SCENE_VERTICES_PER_TRIANGLE` in shadow_raster.wgsl.
pub(in crate::pipeline::gpu) const VERTICES_PER_TRIANGLE: u32 = 6;
const CUBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MESH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const VISIBILITY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// `SCENE_CUBE_MARGIN` in lights_common.wgsl.
const CUBE_MARGIN: f32 = 0.1;

/// How carefully the light bake and the air light are built (§4.4). Draft
/// while a control, a light's handle or the view is dragged, so a drag stays
/// quick; Final when the preview is idle, and for every export.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SceneLightQuality {
    #[default]
    Final,
    Draft,
}

impl SceneLightQuality {
    /// A cube face's edge in texels: 512 in Final, 256 in Draft and wherever
    /// the bake works at its reduced size.
    fn face_texels(self) -> u32 {
        match self {
            Self::Final if gpu_working_set_limit_bytes() >= FULL_BAKE_BUDGET => 512,
            _ => 256,
        }
    }

    /// The mesh level cubes are drawn from: the working size, or half of it.
    fn mesh_level(self) -> u32 {
        match self {
            Self::Final => 0,
            Self::Draft => 1,
        }
    }

    /// Taps of the visibility filter: the blocker search and the penumbra.
    fn taps(self) -> [u32; 2] {
        match self {
            Self::Final => [16, 32],
            Self::Draft => [8, 12],
        }
    }

    /// The air light's march (§4.4 L3): the pipeline's pixels per grid
    /// texel on an edge, steps along each ray and cube taps per step.
    fn air_march(self) -> [u32; 3] {
        match self {
            Self::Final => [2, 48, 4],
            Self::Draft => [4, 16, 1],
        }
    }
}

/// Which effect places a light of the bake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::pipeline::gpu) enum SceneLightKind {
    /// A Relight light, placed by its card.
    Relight,
    /// A Light Rays source: a point light at the farthest surface, with a
    /// reach that follows the rays' Length.
    LightRays,
}

/// One light of the bake as its effect places it: `light_position` in
/// camera.rs, and `scene_light_at` in scene_lights.wgsl for the receivers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::pipeline::gpu) struct SceneShadowLight {
    pub(in crate::pipeline::gpu) kind: SceneLightKind,
    /// Percent of the full image.
    pub(in crate::pipeline::gpu) source: [f32; 2],
    /// Control depth in percent (Relight's Light depth).
    pub(in crate::pipeline::gpu) light_depth: f32,
    /// The relief scale E, as a fraction.
    pub(in crate::pipeline::gpu) relief_scale: f32,
    /// Light size in [0, 1].
    pub(in crate::pipeline::gpu) size: f32,
    /// Relight's Reach in percent: shorter-edge widths at the nearest
    /// surface at which the falloff has halved. For Light Rays, its Length in
    /// percent.
    pub(in crate::pipeline::gpu) reach: f32,
}

/// What the light bake is built from besides the scene bake: the lights in
/// their channels and the tier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::pipeline::gpu) struct SceneLightSet {
    pub(in crate::pipeline::gpu) lights: [Option<SceneShadowLight>; SCENE_SHADOW_LIGHTS],
    pub(in crate::pipeline::gpu) quality: SceneLightQuality,
}

/// `SceneShadowLight` in lights_common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::pipeline::gpu) struct SceneShadowLightUniform {
    pub(in crate::pipeline::gpu) position: [f32; 4],
    pub(in crate::pipeline::gpu) shape: [f32; 4],
    pub(in crate::pipeline::gpu) air: [f32; 4],
}

/// `SceneAirLight` in lights_common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::pipeline::gpu) struct SceneAirLightUniform {
    pub(in crate::pipeline::gpu) region: [i32; 4],
    pub(in crate::pipeline::gpu) march: [u32; 4],
    pub(in crate::pipeline::gpu) image: [u32; 4],
}

/// `SceneShadowLights` in lights_common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(in crate::pipeline::gpu) struct SceneShadowLightsUniform {
    pub(in crate::pipeline::gpu) lights: [SceneShadowLightUniform; SCENE_SHADOW_LIGHTS],
    pub(in crate::pipeline::gpu) taps: [u32; 4],
}

/// The light's radius in frame units at Size `size`: from a point to a tenth
/// and a half of the image's shorter edge at the nearest surface, where that
/// edge spans `2 · focal_tan`.
fn light_radius(camera: &SceneCamera, size: f32) -> f32 {
    (0.002 + 0.15 * size.clamp(0.0, 1.0)) * 2.0 * camera.focal_tan
}

/// The falloff's half distance in frame units at Reach `reach` (percent):
/// as `relight_light` in relight.wgsl.
fn light_reach(camera: &SceneCamera, reach: f32) -> f32 {
    (reach / 100.0).clamp(0.1, 4.0) * 2.0 * camera.focal_tan
}

/// The falloff's half distance of a Light Rays source with Length `length`
/// (percent) in frame units: the share of the rays' reach
/// (`LIGHT_RAYS_LIGHT_REACH` in scene_lights.wgsl) in shorter edges at the
/// source, which lies at the farthest surface, `z = 1 + R`.
fn rays_reach(camera: &SceneCamera, length: f32) -> f32 {
    const LIGHT_RAYS_LIGHT_REACH: f32 = 0.35;
    let length = (length / 100.0).clamp(0.0, 2.0);
    (length * LIGHT_RAYS_LIGHT_REACH * 2.0 * camera.focal_tan * (1.0 + camera.frame_relief))
        .max(1e-3)
}

/// Lights uploaded for the next encode.
#[derive(Clone, Copy)]
struct PendingLights {
    set: SceneLightSet,
    uniform: SceneShadowLightsUniform,
    camera: SceneCamera,
}

/// What a light's cube was drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CubeKey {
    generation: u64,
    position: [f32; 4],
    face_texels: u32,
    mesh_level: u32,
}

/// What a group's visibility was computed from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct VisibilityKey {
    cubes: [Option<(CubeKey, f32)>; LIGHTS_PER_GROUP],
    taps: [u32; 2],
}

/// The light bake's resources and what they hold.
pub(super) struct SceneLightBake {
    /// Cube layers, six per light, for this many lights.
    capacity: usize,
    face_texels: u32,
    cubes: wgpu::Texture,
    /// The visible surface's depth (shadow_raster.wgsl), two levels.
    mesh: wgpu::Texture,
    /// Two layers per group (visibility.wgsl).
    visibility: wgpu::Texture,
    uniforms: wgpu::Buffer,
    /// Per group: its index (`scene_visibility_group`).
    group_uniforms: Vec<wgpu::Buffer>,
    /// The lights `prepare_lights` uploaded last.
    pending: Option<PendingLights>,
    mesh_generation: Option<u64>,
    cubes_built: [Option<CubeKey>; SCENE_SHADOW_LIGHTS],
    visibility_built: [Option<VisibilityKey>; VISIBILITY_GROUPS],
    _reservation: GpuBudgetReservation,
}

impl SceneLightBake {
    fn new(device: &wgpu::Device, size: [u32; 2], capacity: usize) -> Result<Self> {
        let [width, height] = size;
        let face_texels = SceneLightQuality::Final.face_texels();
        let texels = u64::from(width) * u64::from(height);
        let cube_layers = CUBE_FACES * capacity as u32;
        let groups = capacity.div_ceil(LIGHTS_PER_GROUP) as u32;
        let bytes = u64::from(face_texels).pow(2) * 4 * u64::from(cube_layers)
            + texels * 4 * 5 / 4
            + texels * 4 * 2 * u64::from(groups);
        let reservation = GpuBudgetReservation::acquire_bytes(
            "scene light bake",
            bytes,
            gpu_working_set_limit_bytes(),
        )?;
        let cubes = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw scene shadow cubes"),
            size: wgpu::Extent3d {
                width: face_texels,
                height: face_texels,
                depth_or_array_layers: cube_layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CUBE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mesh = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw scene shadow mesh"),
            size: texture_size(width, height),
            mip_level_count: 2,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MESH_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let visibility = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("calibraw scene visibility"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 2 * groups,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VISIBILITY_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let uniforms = create_gpu_buffer(
            device,
            "calibraw scene shadow lights",
            std::mem::size_of::<SceneShadowLightsUniform>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let group_uniforms = (0..groups)
            .map(|group| {
                super::create_initialized_buffer(
                    device,
                    "calibraw scene visibility group",
                    bytemuck::bytes_of(&[group, 0, 0, 0]),
                    wgpu::BufferUsages::UNIFORM,
                )
            })
            .collect();
        Ok(Self {
            capacity,
            face_texels,
            cubes,
            mesh,
            visibility,
            uniforms,
            group_uniforms,
            pending: None,
            mesh_generation: None,
            cubes_built: [None; SCENE_SHADOW_LIGHTS],
            visibility_built: [None; VISIBILITY_GROUPS],
            _reservation: reservation,
        })
    }

    fn mesh_view(&self, level: Option<u32>) -> wgpu::TextureView {
        self.mesh.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene shadow mesh level"),
            base_mip_level: level.unwrap_or(0),
            mip_level_count: level.map(|_| 1),
            ..Default::default()
        })
    }

    fn layer_view(texture: &wgpu::Texture, layer: u32, label: &str) -> wgpu::TextureView {
        texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        })
    }
}

/// The light bake's compiled programs on one device (with the scene bake's,
/// `bake.rs`).
pub(in crate::pipeline::gpu) struct SceneLightPrograms {
    mesh_prepare: Compute,
    mesh_reduce: Compute,
    visibility: Compute,
    air: Compute,
    raster: wgpu::RenderPipeline,
    raster_layout: wgpu::BindGroupLayout,
    compare: wgpu::Sampler,
}

struct Compute {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    label: &'static str,
}

/// Bindings of the light bake's programs (bake_common.wgsl, lights_common.wgsl,
/// shadow_raster.wgsl and visibility.wgsl).
mod binding {
    pub(super) const BAKE_UNIFORMS: u32 = 85;
    pub(super) const WORKING: u32 = 90;
    pub(super) const GEOMETRY: u32 = 92;
    pub(super) const COVERAGE: u32 = 93;
    pub(super) const NORMALS: u32 = 94;
    pub(super) const NEAR: u32 = 95;
    pub(super) const LIGHTS: u32 = 104;
    pub(super) const CUBES: u32 = 105;
    pub(super) const COMPARE: u32 = 106;
    pub(super) const MESH: u32 = 107;
    pub(super) const MESH_OUT: u32 = 108;
    pub(super) const VISIBILITY_NEAR_OUT: u32 = 109;
    pub(super) const VISIBILITY_FAR_OUT: u32 = 110;
    pub(super) const GROUP: u32 = 111;
    pub(super) const AIR: u32 = 115;
    pub(super) const AIR_OUT: u32 = 116;
}

/// The air light's texture format (air_light.wgsl).
pub(in crate::pipeline::gpu) const AIR_LIGHT_FORMAT: wgpu::TextureFormat =
    wgpu::TextureFormat::Rgba16Float;

fn float_texture(
    binding: u32,
    filterable: bool,
    stages: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stages,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform(binding: u32, stages: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        visibility: stages,
        ..buffer_entry(binding)
    }
}

impl SceneLightPrograms {
    pub(super) fn compile(device: &wgpu::Device, manager: &mut ShaderManager) -> Result<Self> {
        let compute = wgpu::ShaderStages::COMPUTE;
        let module = |manager: &mut ShaderManager, source: shaders::ShaderSource| {
            manager.create_shader_module(
                device,
                source.file_name,
                &source.module_text(),
                source.file_name,
            )
        };
        let raster_module = module(manager, shaders::SCENE_SHADOW_RASTER)?;
        let visibility_module = module(manager, shaders::SCENE_VISIBILITY)?;
        let air_module = module(manager, shaders::SCENE_AIR_LIGHT)?;
        let program = |module: &wgpu::ShaderModule,
                       label: &'static str,
                       entries: &[wgpu::BindGroupLayoutEntry]| {
            let layout = create_bind_group_layout(device, label, entries);
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            Compute {
                pipeline: create_compute_pipeline(
                    device,
                    label,
                    &pipeline_layout,
                    module,
                    label,
                    None,
                ),
                layout,
                label,
            }
        };
        let mesh_prepare = program(
            &raster_module,
            "prepare_scene_shadow_mesh",
            &[
                uniform(binding::BAKE_UNIFORMS, compute),
                float_texture(binding::WORKING, true, compute),
                float_texture(binding::GEOMETRY, true, compute),
                float_texture(binding::COVERAGE, true, compute),
                storage_texture_entry(
                    binding::MESH_OUT,
                    MESH_FORMAT,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
            ],
        );
        let mesh_reduce = program(
            &raster_module,
            "reduce_scene_shadow_mesh",
            &[
                uniform(binding::BAKE_UNIFORMS, compute),
                float_texture(binding::MESH, false, compute),
                storage_texture_entry(
                    binding::MESH_OUT,
                    MESH_FORMAT,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
            ],
        );
        let visibility = program(
            &visibility_module,
            "bake_scene_visibility",
            &[
                uniform(binding::BAKE_UNIFORMS, compute),
                float_texture(binding::GEOMETRY, true, compute),
                float_texture(binding::NORMALS, true, compute),
                float_texture(binding::NEAR, true, compute),
                uniform(binding::LIGHTS, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: binding::CUBES,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: binding::COMPARE,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                storage_texture_entry(
                    binding::VISIBILITY_NEAR_OUT,
                    VISIBILITY_FORMAT,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
                storage_texture_entry(
                    binding::VISIBILITY_FAR_OUT,
                    VISIBILITY_FORMAT,
                    wgpu::StorageTextureAccess::WriteOnly,
                ),
                uniform(binding::GROUP, compute),
            ],
        );

        let cubes_entry = wgpu::BindGroupLayoutEntry {
            binding: binding::CUBES,
            visibility: compute,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let air = program(
            &air_module,
            "bake_scene_air_light",
            &[
                uniform(binding::BAKE_UNIFORMS, compute),
                float_texture(binding::GEOMETRY, true, compute),
                float_texture(binding::COVERAGE, true, compute),
                uniform(binding::LIGHTS, compute),
                cubes_entry,
                wgpu::BindGroupLayoutEntry {
                    binding: binding::COMPARE,
                    visibility: compute,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                uniform(binding::AIR, compute),
                wgpu::BindGroupLayoutEntry {
                    binding: binding::AIR_OUT,
                    visibility: compute,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: AIR_LIGHT_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                    count: None,
                },
            ],
        );

        let vertex = wgpu::ShaderStages::VERTEX;
        let raster_layout = create_bind_group_layout(
            device,
            "scene shadow raster",
            &[
                uniform(binding::BAKE_UNIFORMS, vertex),
                uniform(binding::LIGHTS, vertex),
                float_texture(binding::MESH, false, vertex),
            ],
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene shadow raster"),
            bind_group_layouts: &[Some(&raster_layout)],
            immediate_size: 0,
        });
        let raster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene shadow raster"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &raster_module,
                entry_point: Some("scene_shadow_vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: CUBE_FORMAT,
                depth_write_enabled: Some(true),
                // Reversed depth: nearer is larger.
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("calibraw scene shadow compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::GreaterEqual),
            ..Default::default()
        });
        Ok(Self {
            mesh_prepare,
            mesh_reduce,
            visibility,
            air,
            raster,
            raster_layout,
            compare,
        })
    }
}

impl SceneBake {
    /// Allocates the light bake on first use and uploads `set`'s lights for
    /// the next `encode_lights`. Nothing happens before a depth result is
    /// uploaded.
    pub(super) fn prepare_lights(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        set: &SceneLightSet,
    ) {
        let Some(camera) = self.camera() else {
            return;
        };
        let needed = set
            .lights
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |last| last + 1);
        let mut guard = self.lock_lights();
        if needed == 0 {
            if let Some(lights) = guard.as_mut() {
                lights.pending = None;
            }
            return;
        }
        if guard.as_ref().is_none_or(|bake| bake.capacity < needed) {
            // Room for every light of a group, so the array rarely grows.
            let capacity = needed.next_multiple_of(LIGHTS_PER_GROUP);
            *guard = None;
            match SceneLightBake::new(device, self.size, capacity) {
                Ok(bake) => {
                    *guard = Some(bake);
                    self.lights_revision
                        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                }
                Err(error) => {
                    log::error!("scene light bake not allocated: {error:#}");
                    return;
                }
            }
        }
        let Some(lights) = guard.as_mut() else {
            return;
        };
        let quality = set.quality;
        let face_texels = quality.face_texels().min(lights.face_texels);
        let mesh_level = quality.mesh_level();
        let taps = quality.taps();
        let mut uniform = SceneShadowLightsUniform {
            taps: [taps[0], taps[1], 0, 0],
            ..Default::default()
        };
        let aspect = self.aspect();
        for (channel, light) in set.lights.iter().enumerate() {
            let Some(light) = light else {
                continue;
            };
            let position = super::camera::light_position(
                &camera,
                aspect,
                light.source,
                light.light_depth,
                light.relief_scale,
            );
            let reach = match light.kind {
                SceneLightKind::Relight => light_reach(&camera, light.reach),
                SceneLightKind::LightRays => rays_reach(&camera, light.reach),
            };
            uniform.lights[channel] = SceneShadowLightUniform {
                position: [position[0], position[1], position[2], light.relief_scale],
                shape: [
                    light_radius(&camera, light.size),
                    face_texels as f32,
                    (CUBE_FACES as usize * channel) as f32,
                    mesh_level as f32,
                ],
                air: [reach, 0.0, 0.0, 0.0],
            };
        }
        queue.write_buffer(&lights.uniforms, 0, bytemuck::bytes_of(&uniform));
        lights.pending = Some(PendingLights {
            set: *set,
            uniform,
            camera,
        });
    }

    /// Encodes what of the light bake is outdated for the lights
    /// `prepare_lights` uploaded (module docs). It encodes nothing while the
    /// scene bake is not baked.
    pub(super) fn encode_lights(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        programs: &SceneLightPrograms,
    ) {
        let Some(generation) = self.baked_generation() else {
            return;
        };
        let mut guard = self.lock_lights();
        let Some(lights) = guard.as_mut() else {
            return;
        };
        let Some(PendingLights {
            set,
            uniform,
            camera,
        }) = lights.pending
        else {
            return;
        };
        let bake_views = self.light_bake_views();
        if lights.mesh_generation != Some(generation) {
            encode_mesh(device, encoder, programs, self, lights, &bake_views);
            lights.mesh_generation = Some(generation);
        }
        let aspect = self.aspect();
        let keys: [Option<CubeKey>; SCENE_SHADOW_LIGHTS] = std::array::from_fn(|channel| {
            set.lights[channel].map(|_| {
                let light = &uniform.lights[channel];
                CubeKey {
                    generation,
                    position: light.position,
                    face_texels: light.shape[1] as u32,
                    mesh_level: light.shape[3] as u32,
                }
            })
        });
        for (channel, key) in keys.iter().enumerate() {
            let Some(key) = key else {
                continue;
            };
            if lights.cubes_built[channel] == Some(*key) {
                continue;
            }
            let draw = CubeDraw {
                channel,
                faces: scene_faces(&camera, aspect, key.position),
                face_texels: key.face_texels,
                mesh_level: key.mesh_level,
            };
            encode_cube(device, encoder, programs, self, lights, &draw);
            lights.cubes_built[channel] = Some(*key);
        }
        for group in 0..lights.capacity.div_ceil(LIGHTS_PER_GROUP) {
            let channels = group * LIGHTS_PER_GROUP..(group + 1) * LIGHTS_PER_GROUP;
            if keys[channels.clone()].iter().all(Option::is_none) {
                continue;
            }
            let key = VisibilityKey {
                cubes: std::array::from_fn(|index| {
                    let channel = channels.start + index;
                    keys[channel].map(|key| (key, uniform.lights[channel].shape[0]))
                }),
                taps: set.quality.taps(),
            };
            if lights.visibility_built[group] == Some(key) {
                continue;
            }
            encode_visibility(device, encoder, programs, self, lights, group, &bake_views);
            lights.visibility_built[group] = Some(key);
        }
    }

    fn lock_lights(&self) -> std::sync::MutexGuard<'_, Option<SceneLightBake>> {
        self.lights
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The visibility array, once the light bake exists (geometry.wgsl).
    pub(in crate::pipeline::gpu) fn visibility_texture(&self) -> Option<wgpu::Texture> {
        self.lock_lights()
            .as_ref()
            .map(|lights| lights.visibility.clone())
    }

    /// The shadow cubes, once the light bake exists (geometry.wgsl
    /// `scene_point_visibility`).
    pub(in crate::pipeline::gpu) fn cubes_texture(&self) -> Option<wgpu::Texture> {
        self.lock_lights()
            .as_ref()
            .map(|lights| lights.cubes.clone())
    }

    /// The lights' uniform, once the light bake exists.
    pub(in crate::pipeline::gpu) fn lights_uniforms(&self) -> Option<wgpu::Buffer> {
        self.lock_lights()
            .as_ref()
            .map(|lights| lights.uniforms.clone())
    }

    /// The working size's width and height in shorter edges.
    fn aspect(&self) -> [f32; 2] {
        let [width, height] = self.size.map(|edge| edge as f32);
        let short = width.min(height).max(1.0);
        [width / short, height / short]
    }
}

/// The bake's layers and working image the light passes read.
struct BakeViews {
    working: wgpu::TextureView,
    geometry: wgpu::TextureView,
    coverage: wgpu::TextureView,
    normals: wgpu::TextureView,
    near: wgpu::TextureView,
}

impl SceneBake {
    fn light_bake_views(&self) -> BakeViews {
        BakeViews {
            working: self.working_view(0),
            geometry: self.layer_view(super::bake::GEOMETRY),
            coverage: self.layer_view(super::bake::COVERAGE),
            normals: self.layer_view(super::bake::NORMALS),
            near: self.layer_view(super::bake::NEAR),
        }
    }
}

fn entry<'a>(binding: u32, resource: wgpu::BindingResource<'a>) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry { binding, resource }
}

fn view(view: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::TextureView(view)
}

fn run(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    program: &Compute,
    entries: &[wgpu::BindGroupEntry<'_>],
    [width, height]: [u32; 2],
) {
    let bind_group = create_bind_group(device, program.label, &program.layout, entries);
    dispatch_compute(
        encoder,
        program.label,
        &program.pipeline,
        &[&bind_group],
        [
            width.div_ceil(WORKGROUP_EDGE),
            height.div_ceil(WORKGROUP_EDGE),
            1,
        ],
    );
}

fn encode_mesh(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    programs: &SceneLightPrograms,
    bake: &SceneBake,
    lights: &SceneLightBake,
    views: &BakeViews,
) {
    let level0 = lights.mesh_view(Some(0));
    let level1 = lights.mesh_view(Some(1));
    run(
        device,
        encoder,
        &programs.mesh_prepare,
        &[
            entry(binding::BAKE_UNIFORMS, bake.uniforms.as_entire_binding()),
            entry(binding::WORKING, view(&views.working)),
            entry(binding::GEOMETRY, view(&views.geometry)),
            entry(binding::COVERAGE, view(&views.coverage)),
            entry(binding::MESH_OUT, view(&level0)),
        ],
        bake.size,
    );
    run(
        device,
        encoder,
        &programs.mesh_reduce,
        &[
            entry(binding::BAKE_UNIFORMS, bake.uniforms.as_entire_binding()),
            entry(binding::MESH, view(&level0)),
            entry(binding::MESH_OUT, view(&level1)),
        ],
        bake.size.map(|edge| (edge >> 1).max(1)),
    );
}

/// One light's cube to draw.
#[derive(Clone, Copy)]
struct CubeDraw {
    channel: usize,
    /// The faces that see part of the scene (`scene_faces`).
    faces: [bool; CUBE_FACES as usize],
    face_texels: u32,
    mesh_level: u32,
}

fn encode_cube(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    programs: &SceneLightPrograms,
    bake: &SceneBake,
    lights: &SceneLightBake,
    draw: &CubeDraw,
) {
    let CubeDraw {
        channel,
        faces,
        face_texels,
        mesh_level,
    } = *draw;
    let mesh = lights.mesh_view(None);
    let bind_group = create_bind_group(
        device,
        "scene shadow raster",
        &programs.raster_layout,
        &[
            entry(binding::BAKE_UNIFORMS, bake.uniforms.as_entire_binding()),
            entry(binding::LIGHTS, lights.uniforms.as_entire_binding()),
            entry(binding::MESH, view(&mesh)),
        ],
    );
    let [width, height] = bake.size.map(|edge| (edge >> mesh_level).max(1));
    // Two triangles per quad of texels, six vertices each (shadow_raster.wgsl).
    let vertices = 2 * VERTICES_PER_TRIANGLE * width.saturating_sub(1) * height.saturating_sub(1);
    for face in 0..CUBE_FACES {
        let layer = CUBE_FACES * channel as u32 + face;
        let target = SceneLightBake::layer_view(&lights.cubes, layer, "calibraw scene shadow face");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene shadow face"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if !faces[face as usize] || vertices == 0 {
            continue;
        }
        pass.set_pipeline(&programs.raster);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_viewport(0.0, 0.0, face_texels as f32, face_texels as f32, 0.0, 1.0);
        let instance = CUBE_FACES * channel as u32 + face;
        pass.draw(0..vertices, instance..instance + 1);
    }
}

fn encode_visibility(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    programs: &SceneLightPrograms,
    bake: &SceneBake,
    lights: &SceneLightBake,
    group: usize,
    views: &BakeViews,
) {
    let cubes = lights.cubes.create_view(&wgpu::TextureViewDescriptor {
        label: Some("calibraw scene shadow cubes"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let near = SceneLightBake::layer_view(
        &lights.visibility,
        2 * group as u32,
        "calibraw scene visibility near",
    );
    let far = SceneLightBake::layer_view(
        &lights.visibility,
        2 * group as u32 + 1,
        "calibraw scene visibility far",
    );
    run(
        device,
        encoder,
        &programs.visibility,
        &[
            entry(binding::BAKE_UNIFORMS, bake.uniforms.as_entire_binding()),
            entry(binding::GEOMETRY, view(&views.geometry)),
            entry(binding::NORMALS, view(&views.normals)),
            entry(binding::NEAR, view(&views.near)),
            entry(binding::LIGHTS, lights.uniforms.as_entire_binding()),
            entry(binding::CUBES, view(&cubes)),
            entry(
                binding::COMPARE,
                wgpu::BindingResource::Sampler(&programs.compare),
            ),
            entry(binding::VISIBILITY_NEAR_OUT, view(&near)),
            entry(binding::VISIBILITY_FAR_OUT, view(&far)),
            entry(
                binding::GROUP,
                lights.group_uniforms[group].as_entire_binding(),
            ),
        ],
        bake.size,
    );
}

/// Which faces of a cube at `light` (frame position, relief scale in w) see
/// part of the scene: the camera's view between the nearest and the farthest
/// surface at that relief scale. A face is skipped when all eight corners of
/// that frustum lie outside one plane of the face's own (with its guard
/// band), which leaves no point of the frustum in it.
fn scene_faces(
    camera: &SceneCamera,
    aspect: [f32; 2],
    light: [f32; 4],
) -> [bool; CUBE_FACES as usize] {
    let relief_scale = light[3];
    let far = 1.0 + relief_scale * camera.frame_relief;
    let mut corners = Vec::with_capacity(8);
    for z in [1.0, far] {
        for sx in [-0.5, 0.5] {
            for sy in [-0.5, 0.5] {
                let lateral = 2.0 * camera.focal_tan * z;
                corners.push([
                    sx * aspect[0] * lateral - light[0],
                    sy * aspect[1] * lateral - light[1],
                    z - light[2],
                ]);
            }
        }
    }
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    std::array::from_fn(|face| {
        let axis = face / 2;
        let sign = if face % 2 == 0 { 1.0 } else { -1.0 };
        let mut forward = [0.0; 3];
        forward[axis] = sign * (1.0 + CUBE_MARGIN);
        // The face's four side planes, normals pointing inward, plus the
        // plane behind its apex.
        let mut planes = vec![forward];
        for other in (0..3).filter(|&other| other != axis) {
            for side in [-1.0, 1.0] {
                let mut normal = forward;
                normal[other] = side;
                planes.push(normal);
            }
        }
        !planes
            .iter()
            .any(|&normal| corners.iter().all(|&corner| dot(normal, corner) < 0.0))
    })
}

/// The part of the air light's grid (lights_common.wgsl `SceneAirLight`) a
/// pipeline needs: every texel the upsample in geometry.wgsl reads for its
/// pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::pipeline::gpu) struct SceneAirLightRegion {
    /// First grid texel and the texels on each edge.
    pub(in crate::pipeline::gpu) origin: [i32; 2],
    pub(in crate::pipeline::gpu) size: [u32; 2],
    /// Full-image pixels per grid texel on an edge.
    pub(in crate::pipeline::gpu) factor: u32,
    /// The full image in pixels.
    pub(in crate::pipeline::gpu) image: [u32; 2],
}

impl SceneAirLightRegion {
    /// The region for a pipeline whose pixels are full-image pixels
    /// `[origin, origin + size)`, in tier `quality`. A pixel at `p` reads the
    /// texels `floor(p / factor − 1/2)` and one past it.
    pub(in crate::pipeline::gpu) fn covering(
        origin: [u32; 2],
        size: [u32; 2],
        image: [u32; 2],
        quality: SceneLightQuality,
    ) -> Self {
        let factor = quality.air_march()[0];
        let first = |pixel: u32| {
            ((2 * pixel as i64 + 1 - factor as i64).div_euclid(2 * factor as i64)) as i32
        };
        let start = [first(origin[0]), first(origin[1])];
        let end = [
            first(origin[0] + size[0].max(1) - 1) + 1,
            first(origin[1] + size[1].max(1) - 1) + 1,
        ];
        Self {
            origin: start,
            size: [
                (end[0] - start[0] + 1) as u32,
                (end[1] - start[1] + 1) as u32,
            ],
            factor,
            image,
        }
    }
}

/// What a pipeline's air light was built from: the scene bake, each light's
/// cube and reach, the region and the march.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::pipeline::gpu) struct SceneAirLightKey {
    cubes: [Option<(CubeKey, f32)>; SCENE_SHADOW_LIGHTS],
    region: SceneAirLightRegion,
    march: [u32; 3],
}

/// A pipeline's air light texture: a 2D array of `layers` layers and its
/// uniform buffer.
pub(in crate::pipeline::gpu) struct SceneAirLightTarget<'a> {
    pub(in crate::pipeline::gpu) texture: &'a wgpu::Texture,
    pub(in crate::pipeline::gpu) uniforms: &'a wgpu::Buffer,
}

/// Layers of the air light for `groups` groups of four lights: the depths,
/// then near and far shares per group.
pub(in crate::pipeline::gpu) fn air_light_layers(groups: u32) -> u32 {
    1 + 2 * groups
}

impl SceneBake {
    /// Groups of four lights the light bake holds, once lights were
    /// prepared.
    pub(in crate::pipeline::gpu) fn air_light_groups(&self) -> u32 {
        self.lock_lights()
            .as_ref()
            .filter(|lights| lights.pending.is_some())
            .map_or(0, |lights| {
                lights.capacity.div_ceil(LIGHTS_PER_GROUP) as u32
            })
    }

    /// Encodes the air light of `region` into `target` unless `built` says
    /// it holds it already, and returns what it holds: `None` when there is
    /// nothing to march (no baked scene or no lights). Call after
    /// `encode_lights`, whose cubes it reads.
    pub(in crate::pipeline::gpu) fn encode_air_light(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        programs: &SceneLightPrograms,
        target: &SceneAirLightTarget<'_>,
        region: SceneAirLightRegion,
        built: Option<SceneAirLightKey>,
    ) -> Option<SceneAirLightKey> {
        self.baked_generation()?;
        let guard = self.lock_lights();
        let lights = guard.as_ref()?;
        let pending = lights.pending?;
        let march = pending.set.quality.air_march();
        let key = SceneAirLightKey {
            cubes: std::array::from_fn(|channel| {
                lights.cubes_built[channel]
                    .filter(|_| pending.set.lights[channel].is_some())
                    .map(|cube| (cube, pending.uniform.lights[channel].air[0]))
            }),
            region,
            march,
        };
        if key.cubes.iter().all(Option::is_none) {
            return None;
        }
        if built == Some(key) {
            return built;
        }
        let groups = lights.capacity.div_ceil(LIGHTS_PER_GROUP) as u32;
        let uniform = SceneAirLightUniform {
            region: [
                region.origin[0],
                region.origin[1],
                region.size[0] as i32,
                region.size[1] as i32,
            ],
            march: [march[0], march[1], march[2], groups],
            image: [region.image[0], region.image[1], 0, 0],
        };
        let staging = super::super::create_initialized_buffer(
            device,
            "calibraw scene air light uniforms",
            bytemuck::bytes_of(&uniform),
            wgpu::BufferUsages::COPY_SRC,
        );
        encoder.copy_buffer_to_buffer(
            &staging,
            0,
            target.uniforms,
            0,
            std::mem::size_of::<SceneAirLightUniform>() as u64,
        );
        let views = self.light_bake_views();
        let cubes = lights.cubes.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene shadow cubes"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let out = target.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("calibraw scene air light"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        run(
            device,
            encoder,
            &programs.air,
            &[
                entry(binding::BAKE_UNIFORMS, self.uniforms.as_entire_binding()),
                entry(binding::GEOMETRY, view(&views.geometry)),
                entry(binding::COVERAGE, view(&views.coverage)),
                entry(binding::LIGHTS, lights.uniforms.as_entire_binding()),
                entry(binding::CUBES, view(&cubes)),
                entry(
                    binding::COMPARE,
                    wgpu::BindingResource::Sampler(&programs.compare),
                ),
                entry(binding::AIR, target.uniforms.as_entire_binding()),
                entry(binding::AIR_OUT, view(&out)),
            ],
            region.size,
        );
        Some(key)
    }
}
