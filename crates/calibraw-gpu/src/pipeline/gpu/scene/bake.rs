//! The scene bake's passes (`shaders/scene/bake_*.wgsl`): their programs,
//! compiled once per device and shared by every pipeline on it, and the
//! order they run in.
//!
//! Each pass has its own bind group layout listing exactly what it binds, as
//! a pass reads some layers of the bake array while it writes others, and
//! wgpu checks the use of every resource a bind group holds. Bind groups are
//! made when the passes are encoded, which happens once per depth result and
//! scene.

use super::super::shader_manager::ShaderManager;
use super::super::{
    buffer_entry, create_bind_group, create_bind_group_layout, create_compute_pipeline,
    dispatch_compute, sampler_entry, shaders, storage_buffer_entry, storage_texture_entry,
    texture_entry, WORKGROUP_EDGE,
};
use super::lights::SceneLightPrograms;
use super::{SceneBake, SceneBakeInputs, SceneBakeUniforms, SCENE_BAKE_FORMAT, SCENE_LAYER_LEVELS};
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex, Weak};

/// What a bake pass binds, by its binding in bake_common.wgsl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binding {
    /// 84, read and written.
    Scratch,
    /// 85.
    Uniforms,
    /// 86 to 88: the depth result's disparity, sky and normals.
    Disparity,
    Sky,
    Normals,
    /// 89.
    Sampler,
    /// 90 and 91: a level of the working image, read or written.
    WorkingIn,
    WorkingOut,
    /// 92 to 96 and 97 to 101: a layer of the bake array, read or written.
    LayerIn(u32),
    LayerOut(u32),
    /// 102.
    Focus,
    /// 103: the level a pyramid or mip pass works on.
    Level,
}

impl Binding {
    fn number(self) -> u32 {
        match self {
            Self::Scratch => 84,
            Self::Uniforms => 85,
            Self::Disparity => 86,
            Self::Sky => 87,
            Self::Normals => 88,
            Self::Sampler => 89,
            Self::WorkingIn => 90,
            Self::WorkingOut => 91,
            Self::LayerIn(layer) => 92 + layer,
            Self::LayerOut(layer) => 97 + layer,
            Self::Focus => 102,
            Self::Level => 103,
        }
    }

    fn layout_entry(self) -> wgpu::BindGroupLayoutEntry {
        let binding = self.number();
        let float = |filterable| wgpu::TextureSampleType::Float { filterable };
        match self {
            Self::Scratch | Self::Focus => storage_buffer_entry(binding, false),
            Self::Uniforms | Self::Level => buffer_entry(binding),
            Self::Disparity => texture_entry(binding, float(false)),
            Self::Sky | Self::Normals | Self::WorkingIn | Self::LayerIn(_) => {
                texture_entry(binding, float(true))
            }
            Self::Sampler => sampler_entry(binding),
            Self::WorkingOut | Self::LayerOut(_) => storage_texture_entry(
                binding,
                SCENE_BAKE_FORMAT,
                wgpu::StorageTextureAccess::WriteOnly,
            ),
        }
    }
}

// Layers (`SCENE_LAYER_*`).
pub(super) const GEOMETRY: u32 = 0;
pub(super) const COVERAGE: u32 = 1;
pub(super) const NORMALS: u32 = 2;
pub(super) const NEAR: u32 = 3;
const FAR: u32 = 4;

use Binding::{
    Disparity, Focus, LayerIn, LayerOut, Level, Normals, Sampler, Scratch, Sky, Uniforms,
    WorkingIn, WorkingOut,
};

/// The bake's passes in the order they run (`Pass` indexes this).
const PASSES: [(shaders::ShaderSource, &str, &[Binding]); 16] = [
    (
        shaders::SCENE_BAKE_DEPTH,
        "resolve_scene_working_image",
        &[Scratch, Uniforms, WorkingOut],
    ),
    (
        shaders::SCENE_BAKE_DEPTH,
        "bake_scene_distance",
        &[Uniforms, Disparity, Sky, Sampler, LayerOut(COVERAGE)],
    ),
    (
        shaders::SCENE_BAKE_DEPTH,
        "bake_scene_extent_rows",
        &[Scratch, Uniforms, LayerIn(COVERAGE)],
    ),
    (
        shaders::SCENE_BAKE_DEPTH,
        "bake_scene_extent_columns",
        &[
            Scratch,
            Uniforms,
            LayerIn(COVERAGE),
            LayerOut(GEOMETRY),
            LayerOut(FAR),
        ],
    ),
    (
        shaders::SCENE_BAKE_FOCUS,
        "bake_scene_focus_samples",
        &[Uniforms, Sampler, WorkingIn, LayerIn(GEOMETRY), Focus],
    ),
    (
        shaders::SCENE_BAKE_FOCUS,
        "bake_scene_focus_fit",
        &[Uniforms, Focus],
    ),
    (
        shaders::SCENE_BAKE_COVERAGE,
        "bake_scene_sky_colours",
        &[
            Uniforms,
            Sampler,
            WorkingIn,
            LayerIn(GEOMETRY),
            LayerIn(FAR),
            LayerOut(NORMALS),
            LayerOut(NEAR),
        ],
    ),
    (
        shaders::SCENE_BAKE_COVERAGE,
        "bake_scene_coverage",
        &[
            Scratch,
            Uniforms,
            WorkingIn,
            LayerIn(GEOMETRY),
            LayerIn(NORMALS),
            LayerIn(NEAR),
            LayerIn(FAR),
            LayerOut(COVERAGE),
            Focus,
        ],
    ),
    (
        shaders::SCENE_BAKE_COVERAGE,
        "bake_scene_coverage_sides",
        &[
            Scratch,
            Uniforms,
            LayerIn(GEOMETRY),
            LayerOut(COVERAGE),
            LayerOut(FAR),
        ],
    ),
    (
        shaders::SCENE_BAKE_LAYERS,
        "bake_scene_layers_push_first",
        &[
            Scratch,
            Uniforms,
            Normals,
            Sampler,
            WorkingIn,
            LayerIn(GEOMETRY),
            LayerIn(COVERAGE),
            LayerIn(FAR),
        ],
    ),
    (
        shaders::SCENE_BAKE_LAYERS,
        "bake_scene_layers_push",
        &[Scratch, Uniforms, Level],
    ),
    (
        shaders::SCENE_BAKE_LAYERS,
        "bake_scene_layers_pull",
        &[Scratch, Uniforms, Level],
    ),
    (
        shaders::SCENE_BAKE_LAYERS,
        "bake_scene_layers_near",
        &[
            Scratch,
            Uniforms,
            Normals,
            Sampler,
            WorkingIn,
            LayerIn(GEOMETRY),
            LayerIn(COVERAGE),
            LayerIn(FAR),
            LayerOut(NORMALS),
            LayerOut(NEAR),
        ],
    ),
    (
        shaders::SCENE_BAKE_LAYERS,
        "bake_scene_layers_far",
        &[
            Scratch,
            Uniforms,
            Disparity,
            Sky,
            Normals,
            Sampler,
            WorkingIn,
            LayerIn(COVERAGE),
            LayerIn(NEAR),
            LayerOut(GEOMETRY),
            LayerOut(FAR),
        ],
    ),
    (
        shaders::SCENE_BAKE_MIPS,
        "bake_scene_working_mip",
        &[
            WorkingIn,
            WorkingOut,
            LayerIn(GEOMETRY),
            LayerIn(COVERAGE),
            Level,
        ],
    ),
    (
        shaders::SCENE_BAKE_MIPS,
        "bake_scene_working_mip_flat",
        &[WorkingIn, WorkingOut, Level],
    ),
];

#[derive(Clone, Copy)]
enum Pass {
    Resolve,
    Distance,
    ExtentRows,
    ExtentColumns,
    FocusSamples,
    FocusFit,
    SkyColours,
    Coverage,
    CoverageSides,
    PushFirst,
    Push,
    Pull,
    LayersNear,
    LayersFar,
    WorkingMip,
    WorkingMipFlat,
}

struct Program {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    bindings: &'static [Binding],
    label: &'static str,
}

/// The bake's compiled passes on one device, and the light bake's.
pub(in crate::pipeline::gpu) struct SceneBakePrograms {
    device: wgpu::Device,
    programs: Vec<Program>,
    /// Linear filtering, clamped at the edges (89).
    sampler: wgpu::Sampler,
    lights: SceneLightPrograms,
}

/// Programs alive on some device, found again by device.
static SHARED: Mutex<Vec<Weak<SceneBakePrograms>>> = Mutex::new(Vec::new());

impl SceneBakePrograms {
    /// The programs of `device`, compiled on first use. Compiling blocks for
    /// as long as the driver takes.
    pub(in crate::pipeline::gpu) fn shared(device: &wgpu::Device) -> Result<Arc<Self>> {
        let mut shared = SHARED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        shared.retain(|programs| programs.strong_count() > 0);
        if let Some(programs) = shared
            .iter()
            .filter_map(Weak::upgrade)
            .find(|programs| programs.device == *device)
        {
            return Ok(programs);
        }
        let programs = Arc::new(Self::compile(device)?);
        shared.push(Arc::downgrade(&programs));
        Ok(programs)
    }

    fn compile(device: &wgpu::Device) -> Result<Self> {
        let mut manager = ShaderManager::with_modules(&[
            shaders::SCENE_CAMERA,
            shaders::SCENE_BAKE_COMMON,
            shaders::SCENE_LIGHTS_COMMON,
        ])
        .context("initialize the scene bake's WGSL composer")?;
        let mut modules: Vec<(&'static str, wgpu::ShaderModule)> = Vec::new();
        let mut programs = Vec::with_capacity(PASSES.len());
        for (source, entry, bindings) in PASSES {
            let module = match modules.iter().find(|(file, _)| *file == source.file_name) {
                Some((_, module)) => module.clone(),
                None => {
                    let text = source.module_text();
                    let module = manager.create_shader_module(
                        device,
                        source.file_name,
                        &text,
                        source.file_name,
                    )?;
                    modules.push((source.file_name, module.clone()));
                    module
                }
            };
            let entries: Vec<_> = bindings
                .iter()
                .map(|binding| binding.layout_entry())
                .collect();
            let layout = create_bind_group_layout(device, entry, &entries);
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline =
                create_compute_pipeline(device, entry, &pipeline_layout, &module, entry, None);
            programs.push(Program {
                pipeline,
                layout,
                bindings,
                label: entry,
            });
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("calibraw scene bake sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let lights = SceneLightPrograms::compile(device, &mut manager)?;
        Ok(Self {
            device: device.clone(),
            programs,
            sampler,
            lights,
        })
    }

    pub(super) fn lights(&self) -> &SceneLightPrograms {
        &self.lights
    }

    /// Encodes the bake of `bake` from `inputs`: with `accumulated`, first the
    /// working image from the accumulated sums.
    pub(super) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        bake: &SceneBake,
        inputs: &SceneBakeInputs,
        accumulated: bool,
    ) {
        let [width, height] = bake.size;
        let resources = Resources::new(bake, Some(inputs), &self.sampler);
        let run = |encoder: &mut wgpu::CommandEncoder,
                   pass: Pass,
                   working: [u32; 2],
                   level: Option<&wgpu::Buffer>,
                   [x, y]: [u32; 2]| {
            let program = &self.programs[pass as usize];
            let bind_group = resources.bind_group(device, program, working, level);
            dispatch_compute(
                encoder,
                program.label,
                &program.pipeline,
                &[&bind_group],
                [x.div_ceil(WORKGROUP_EDGE), y.div_ceil(WORKGROUP_EDGE), 1],
            );
        };
        let size = [width, height];
        if accumulated {
            run(encoder, Pass::Resolve, [0, 0], None, size);
        }
        run(encoder, Pass::Distance, [0, 0], None, size);
        run(encoder, Pass::ExtentRows, [0, 0], None, size);
        run(encoder, Pass::ExtentColumns, [0, 0], None, size);
        encoder.clear_buffer(&bake.focus, 0, None);
        run(encoder, Pass::FocusSamples, [0, 0], None, size);
        let fit = &self.programs[Pass::FocusFit as usize];
        dispatch_compute(
            encoder,
            fit.label,
            &fit.pipeline,
            &[&resources.bind_group(device, fit, [0, 0], None)],
            [1, 1, 1],
        );
        // Effects read the fit from the uniforms.
        encoder.copy_buffer_to_buffer(
            &bake.focus,
            0,
            &bake.uniforms,
            std::mem::offset_of!(SceneBakeUniforms, focus_fit) as u64,
            16,
        );
        run(encoder, Pass::SkyColours, [0, 0], None, size);
        run(encoder, Pass::Coverage, [0, 0], None, size);
        run(encoder, Pass::CoverageSides, [0, 0], None, size);

        let level_size = |level: u32| size.map(|edge| edge.div_ceil(1 << level));
        run(encoder, Pass::PushFirst, [0, 0], None, level_size(1));
        for level in 2..=SCENE_LAYER_LEVELS {
            let uniform = &bake.pyramid_uniforms[level as usize - 1];
            run(
                encoder,
                Pass::Push,
                [0, 0],
                Some(uniform),
                level_size(level),
            );
        }
        for level in (1..SCENE_LAYER_LEVELS).rev() {
            let uniform = &bake.pyramid_uniforms[level as usize - 1];
            run(
                encoder,
                Pass::Pull,
                [0, 0],
                Some(uniform),
                level_size(level),
            );
        }
        run(encoder, Pass::LayersNear, [0, 0], None, size);
        run(encoder, Pass::LayersFar, [0, 0], None, size);

        for level in 1..bake.working.mip_level_count() {
            let uniform = &bake.mip_uniforms[level as usize - 1];
            let out = size.map(|edge| (edge >> level).max(1));
            run(
                encoder,
                Pass::WorkingMip,
                [level - 1, level],
                Some(uniform),
                out,
            );
        }
    }
}

impl SceneBakePrograms {
    /// Encodes the bake of `bake` without a depth result: with `accumulated`,
    /// the working image from the accumulated sums, then its mip chain. The
    /// chain's alpha, the nearest visible depth, holds 1: no geometry.
    pub(super) fn encode_image(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        bake: &SceneBake,
        accumulated: bool,
    ) {
        let resources = Resources::new(bake, None, &self.sampler);
        let run = |encoder: &mut wgpu::CommandEncoder,
                   pass: Pass,
                   working: [u32; 2],
                   level: Option<&wgpu::Buffer>,
                   [x, y]: [u32; 2]| {
            let program = &self.programs[pass as usize];
            let bind_group = resources.bind_group(device, program, working, level);
            dispatch_compute(
                encoder,
                program.label,
                &program.pipeline,
                &[&bind_group],
                [x.div_ceil(WORKGROUP_EDGE), y.div_ceil(WORKGROUP_EDGE), 1],
            );
        };
        let size = bake.size;
        if accumulated {
            run(encoder, Pass::Resolve, [0, 0], None, size);
        }
        for level in 1..bake.working.mip_level_count() {
            let uniform = &bake.mip_uniforms[level as usize - 1];
            let out = size.map(|edge| (edge >> level).max(1));
            run(
                encoder,
                Pass::WorkingMipFlat,
                [level - 1, level],
                Some(uniform),
                out,
            );
        }
    }
}

/// The bake's resources a pass may bind. A layer's view serves both for
/// reading and writing it.
struct Resources<'a> {
    bake: &'a SceneBake,
    /// The depth result's inputs; none for a bake of the working image alone,
    /// whose passes bind no depth input.
    inputs: Option<[wgpu::TextureView; 3]>,
    sampler: &'a wgpu::Sampler,
    layers: Vec<wgpu::TextureView>,
}

impl<'a> Resources<'a> {
    fn new(
        bake: &'a SceneBake,
        inputs: Option<&SceneBakeInputs>,
        sampler: &'a wgpu::Sampler,
    ) -> Self {
        Self {
            bake,
            inputs: inputs.map(|inputs| {
                [&inputs.disparity, &inputs.sky, &inputs.normals]
                    .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            }),
            sampler,
            layers: (0..super::SCENE_BAKE_LAYERS)
                .map(|layer| bake.layer_view(layer))
                .collect(),
        }
    }

    /// The depth result's input `index`. Only passes of a bake with a depth
    /// result bind them (`SceneBakePrograms::encode_image` runs none that do).
    fn input(&self, index: usize) -> &wgpu::TextureView {
        &self
            .inputs
            .as_ref()
            .expect("a pass that binds a depth input runs only with a depth result")[index]
    }

    /// `program`'s bind group, with working image levels `[read, write]`.
    fn bind_group(
        &self,
        device: &wgpu::Device,
        program: &Program,
        [read, write]: [u32; 2],
        level: Option<&wgpu::Buffer>,
    ) -> wgpu::BindGroup {
        let working_in = self.bake.working_view(read);
        let working_out = self.bake.working_view(write);
        let entries: Vec<wgpu::BindGroupEntry<'_>> = program
            .bindings
            .iter()
            .map(|&binding| {
                let resource = match binding {
                    Scratch => self.bake.scratch.as_entire_binding(),
                    Uniforms => self.bake.uniforms.as_entire_binding(),
                    Focus => self.bake.focus.as_entire_binding(),
                    Level => level
                        .expect("pyramid and mip passes are given their level")
                        .as_entire_binding(),
                    Disparity => wgpu::BindingResource::TextureView(self.input(0)),
                    Sky => wgpu::BindingResource::TextureView(self.input(1)),
                    Normals => wgpu::BindingResource::TextureView(self.input(2)),
                    Sampler => wgpu::BindingResource::Sampler(self.sampler),
                    WorkingIn => wgpu::BindingResource::TextureView(&working_in),
                    WorkingOut => wgpu::BindingResource::TextureView(&working_out),
                    LayerIn(layer) | LayerOut(layer) => {
                        wgpu::BindingResource::TextureView(&self.layers[layer as usize])
                    }
                };
                wgpu::BindGroupEntry {
                    binding: binding.number(),
                    resource,
                }
            })
            .collect();
        create_bind_group(device, program.label, &program.layout, &entries)
    }
}
