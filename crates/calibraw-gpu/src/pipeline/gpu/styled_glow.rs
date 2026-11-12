//! The GPU side of the Glow with styles (`mask_effects/styled_glow.wgsl`): its
//! two pyramids of what each Glow slot emits, one layer per slot, and the
//! passes that build them.
//!
//! - The near pyramid covers this pipeline's image at half to a sixteenth of
//!   its resolution ([`STYLED_GLOW_NEAR_LEVELS`]). Every pipeline with a Glow
//!   builds it in its output stage, before the creative pass reads it.
//! - The full-image map averages the emission over a grid of the whole image
//!   ([`STYLED_GLOW_MAP_LONG`] cells along its longer edge) and halves it down
//!   to a texel. A pipeline that renders the whole image builds it in its
//!   output stage whenever a Glow is active, so a zoomed view of it can share
//!   it at any spread ([`EffectInputs::share_styled_glow_map`]). An export
//!   builds it once in a prepass over its tiles
//!   ([`RawGpuPipeline::begin_export_styled_glow`]).
//!
//! Both are allocated when a Glow first renders, each with a budget
//! reservation of its own sized for the slots, so pipelines whose edits hold
//! none reserve nothing. Until then their passes bind placeholders that hold
//! the same levels at a few texels, and the shader is told through the effect
//! uniforms whether either is ready (`EffectsUniforms::glow_diffusion`).

use super::*;

/// The programs of the Glow's passes, in the order the pass plan places them.
pub(super) const STYLED_GLOW_PASSES: usize = 5;
pub(super) const STYLED_GLOW_ENTRIES: [&str; STYLED_GLOW_PASSES] = [
    "build_styled_glow_near_first",
    "build_styled_glow_near_level",
    "accumulate_styled_glow_map",
    "resolve_styled_glow_map",
    "build_styled_glow_map_level",
];
const NEAR_FIRST: usize = 0;
const NEAR_LEVEL: usize = 1;
const MAP_ACCUMULATE: usize = 2;
const MAP_RESOLVE: usize = 3;
const MAP_LEVEL: usize = 4;

/// Cells of the full-image map along the image's longer edge
/// (`STYLED_GLOW_MAP_LONG` in styled_glow.wgsl). Its sums take a square of
/// this side per layer.
pub(super) const STYLED_GLOW_MAP_LONG: u32 = 768;
/// Levels of the near pyramid. Their reach past a pixel, 63 pixels with the
/// B-spline read of the last, fits the export halo `GLOW_SUPPORT` reserves.
pub(super) const STYLED_GLOW_NEAR_LEVELS: u32 = 4;
const STYLED_GLOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Bytes of one cell of the map's sums: an RGBA f32 vector.
const STYLED_GLOW_CELL_BYTES: u64 = 16;

/// Whether `mask` is an active Glow slot, which takes a layer.
pub(super) fn is_styled_glow(mask: &MaskData) -> bool {
    mask.applies(MaskEffect::StyledGlow)
}

/// log2 of the deviation of the widest near level in shorter edges of an
/// image `short_edge` pixels short, as `styled_glow_near_deviation` in
/// styled_glow.wgsl gives it.
pub(super) fn styled_glow_near_top(short_edge: u32) -> f32 {
    let texels = (1u64 << (2 * STYLED_GLOW_NEAR_LEVELS)) as f32;
    let variance = 0.25 * (texels - 1.0) + texels / 3.0;
    0.5 * variance.log2() - (short_edge.max(1) as f32).log2()
}

/// The map's grid for a whole image of `full` pixels, as
/// `Common::full_image_grid(STYLED_GLOW_MAP_LONG)` gives it.
pub(super) fn styled_glow_map_grid([width, height]: [u32; 2]) -> [u32; 2] {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let long = width.max(height);
    let cells = |side: f32| {
        ((STYLED_GLOW_MAP_LONG as f32 * side / long).round() as u32).clamp(1, STYLED_GLOW_MAP_LONG)
    };
    [cells(width), cells(height)]
}

/// The size of the near pyramid's first level for an image of `size` pixels:
/// every level must hold the texels of its full-image grid that the image
/// touches at any tile origin, one more than the image's extent at that
/// level, and the sizes must halve exactly through the mips.
fn near_size([width, height]: [u32; 2]) -> [u32; 2] {
    let unit = 1 << (STYLED_GLOW_NEAR_LEVELS - 1);
    let first = |side: u32| unit * (side.div_ceil(1 << STYLED_GLOW_NEAR_LEVELS) + 1);
    [first(width), first(height)]
}

fn mip_levels([width, height]: [u32; 2]) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

fn mip_size([width, height]: [u32; 2], mip: u32) -> [u32; 2] {
    [(width >> mip).max(1), (height >> mip).max(1)]
}

/// The bytes of a texture of `size` with `mips` levels and `layers` layers.
fn texture_bytes(size: [u32; 2], mips: u32, layers: u32) -> u64 {
    (0..mips)
        .map(|mip| {
            let [width, height] = mip_size(size, mip);
            u64::from(width) * u64::from(height) * 8
        })
        .sum::<u64>()
        * u64::from(layers)
}

/// A texture array with `mips` levels, its view of every level and a view of
/// each level alone.
struct LevelTexture {
    _texture: wgpu::Texture,
    all: wgpu::TextureView,
    levels: Vec<wgpu::TextureView>,
}

impl LevelTexture {
    fn new(device: &wgpu::Device, size: [u32; 2], mips: u32, layers: u32, label: &str) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: layers,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: STYLED_GLOW_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[STYLED_GLOW_FORMAT],
        });
        let view = |base_mip_level, mip_level_count| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(label),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_mip_level,
                mip_level_count,
                ..Default::default()
            })
        };
        Self {
            all: view(0, None),
            levels: (0..mips).map(|mip| view(mip, Some(1))).collect(),
            _texture: texture,
        }
    }
}

/// The near pyramid: level `mip + 1` in mip `mip`, one layer per Glow slot.
pub(super) struct StyledGlowNear {
    texture: LevelTexture,
    /// Per mip from 1, the level it holds (`styled_glow_level`).
    level_numbers: Vec<wgpu::Buffer>,
    size: [u32; 2],
    layers: u32,
    _reservation: Option<GpuBudgetReservation>,
}

impl StyledGlowNear {
    fn build(
        device: &wgpu::Device,
        size: [u32; 2],
        layers: u32,
        reservation: Option<GpuBudgetReservation>,
    ) -> Self {
        let level_numbers = (1..STYLED_GLOW_NEAR_LEVELS)
            .map(|mip| {
                create_initialized_buffer(
                    device,
                    "calibraw Glow near level",
                    bytemuck::bytes_of(&[mip + 1, 0, 0, 0]),
                    wgpu::BufferUsages::UNIFORM,
                )
            })
            .collect();
        Self {
            texture: LevelTexture::new(
                device,
                size,
                STYLED_GLOW_NEAR_LEVELS,
                layers,
                "calibraw Glow near pyramid",
            ),
            level_numbers,
            size,
            layers,
            _reservation: reservation,
        }
    }

    /// Never read: its levels hold the structure of a real pyramid at a few
    /// texels, so every pass has a bind group.
    fn placeholder(device: &wgpu::Device) -> Self {
        Self::build(device, near_size([1, 1]), 1, None)
    }

    /// A pyramid for an image of `image` pixels and `layers` Glow slots.
    fn reserved(device: &wgpu::Device, image: [u32; 2], layers: u32) -> Result<Self> {
        let size = near_size(image);
        let reservation = GpuBudgetReservation::acquire_bytes(
            "Glow near pyramid",
            texture_bytes(size, STYLED_GLOW_NEAR_LEVELS, layers),
            gpu_working_set_limit_bytes(),
        )?;
        Ok(Self::build(device, size, layers, Some(reservation)))
    }
}

/// The full-image map: the sums tiles add to, and the mean emission per cell
/// halved down to a texel, one layer per Glow slot.
pub(super) struct StyledGlowMap {
    cells: wgpu::Buffer,
    texture: LevelTexture,
    grid: [u32; 2],
    layers: u32,
    _reservation: Option<GpuBudgetReservation>,
}

impl StyledGlowMap {
    fn build(
        device: &wgpu::Device,
        grid: [u32; 2],
        cell_layers: u32,
        layers: u32,
        reservation: Option<GpuBudgetReservation>,
    ) -> Self {
        Self {
            cells: create_gpu_buffer(
                device,
                "calibraw Glow map sums",
                Self::cells_bytes(cell_layers).max(STYLED_GLOW_CELL_BYTES),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            texture: LevelTexture::new(
                device,
                grid,
                mip_levels(grid),
                layers,
                "calibraw Glow full-image map",
            ),
            grid,
            layers,
            _reservation: reservation,
        }
    }

    /// Never read: the shader is told no map is ready. Two levels and no
    /// room for sums, so every pass has a bind group and adds nothing.
    fn placeholder(device: &wgpu::Device) -> Self {
        Self::build(device, [2, 2], 0, 1, None)
    }

    fn cells_bytes(layers: u32) -> u64 {
        u64::from(STYLED_GLOW_MAP_LONG).pow(2) * STYLED_GLOW_CELL_BYTES * u64::from(layers)
    }

    /// A map of `grid` cells for `layers` Glow slots.
    fn reserved(device: &wgpu::Device, grid: [u32; 2], layers: u32) -> Result<Self> {
        let reservation = GpuBudgetReservation::acquire_bytes(
            "Glow full-image map",
            texture_bytes(grid, mip_levels(grid), layers) + Self::cells_bytes(layers),
            gpu_working_set_limit_bytes(),
        )?;
        Ok(Self::build(device, grid, layers, layers, Some(reservation)))
    }

    /// Whether it holds `grid` cells and room for `layers` slots.
    fn fits(&self, grid: [u32; 2], layers: u32) -> bool {
        self.grid == grid && self.layers >= layers
    }
}

/// One dispatch of a Glow pass: its bind group and the texels it covers.
#[derive(Clone)]
pub(super) struct StyledGlowDispatch {
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) texels: [u32; 2],
}

/// The bind groups of the Glow's passes over the bound pyramids: the first
/// near level and the map's sums from either work texture, then each further
/// level from the one before.
#[derive(Clone)]
pub(super) struct StyledGlowBindGroups {
    /// Reading the first work texture, then the second.
    pub(super) near_first: [StyledGlowDispatch; 2],
    pub(super) near_levels: Vec<StyledGlowDispatch>,
    pub(super) map_accumulate: [StyledGlowDispatch; 2],
    pub(super) map_resolve: StyledGlowDispatch,
    pub(super) map_levels: Vec<StyledGlowDispatch>,
    /// The sums the accumulation adds to, cleared before a new image.
    pub(super) map_cells: wgpu::Buffer,
}

impl StyledGlowBindGroups {
    /// The bind group of `program`'s first dispatch, which the pass list
    /// records (`InputBoundPass::StyledGlow`).
    pub(super) fn first(&self, program: usize) -> &wgpu::BindGroup {
        match program {
            NEAR_FIRST => &self.near_first[0].bind_group,
            NEAR_LEVEL => &self.near_levels[0].bind_group,
            MAP_ACCUMULATE => &self.map_accumulate[0].bind_group,
            MAP_RESOLVE => &self.map_resolve.bind_group,
            _ => &self.map_levels[0].bind_group,
        }
    }
}

/// What the Glow's passes bind besides the pyramids.
pub(super) struct StyledGlowSources<'a> {
    pub(super) layouts: &'a [wgpu::BindGroupLayout; STYLED_GLOW_PASSES],
    pub(super) camera_uniforms: &'a wgpu::Buffer,
    pub(super) mask_data: &'a wgpu::Buffer,
    pub(super) tex1: &'a wgpu::TextureView,
    pub(super) tex2: &'a wgpu::TextureView,
    pub(super) mask: &'a wgpu::TextureView,
    pub(super) mask_sampler: &'a wgpu::Sampler,
}

impl StyledGlowSources<'_> {
    pub(super) fn bind_groups(
        &self,
        device: &wgpu::Device,
        near: &StyledGlowNear,
        map: &StyledGlowMap,
    ) -> StyledGlowBindGroups {
        let inputs = [self.tex1, self.tex2];
        let near_first = inputs.map(|input| StyledGlowDispatch {
            bind_group: create_bind_group(
                device,
                "bg Glow near pyramid first level",
                &self.layouts[NEAR_FIRST],
                &[
                    buffer_binding(0, self.camera_uniforms),
                    texture_binding(24, input),
                    texture_binding(27, self.mask),
                    sampler_binding(28, self.mask_sampler),
                    buffer_binding(33, self.mask_data),
                    texture_binding(120, &near.texture.levels[0]),
                ],
            ),
            texels: near.size,
        });
        let near_levels = (1..STYLED_GLOW_NEAR_LEVELS as usize)
            .map(|mip| StyledGlowDispatch {
                bind_group: create_bind_group(
                    device,
                    "bg Glow near pyramid level",
                    &self.layouts[NEAR_LEVEL],
                    &[
                        buffer_binding(0, self.camera_uniforms),
                        sampler_binding(28, self.mask_sampler),
                        buffer_binding(33, self.mask_data),
                        texture_binding(120, &near.texture.levels[mip]),
                        texture_binding(121, &near.texture.levels[mip - 1]),
                        buffer_binding(123, &near.level_numbers[mip - 1]),
                    ],
                ),
                texels: mip_size(near.size, mip as u32),
            })
            .collect();
        let map_accumulate = inputs.map(|input| StyledGlowDispatch {
            bind_group: create_bind_group(
                device,
                "bg Glow map accumulation",
                &self.layouts[MAP_ACCUMULATE],
                &[
                    buffer_binding(0, self.camera_uniforms),
                    texture_binding(24, input),
                    texture_binding(27, self.mask),
                    sampler_binding(28, self.mask_sampler),
                    buffer_binding(33, self.mask_data),
                    buffer_binding(122, &map.cells),
                ],
            ),
            texels: [STYLED_GLOW_MAP_LONG; 2],
        });
        let map_resolve = StyledGlowDispatch {
            bind_group: create_bind_group(
                device,
                "bg Glow map resolve",
                &self.layouts[MAP_RESOLVE],
                &[
                    buffer_binding(0, self.camera_uniforms),
                    buffer_binding(33, self.mask_data),
                    buffer_binding(122, &map.cells),
                    texture_binding(120, &map.texture.levels[0]),
                ],
            ),
            texels: map.grid,
        };
        let map_levels = (1..map.texture.levels.len())
            .map(|mip| StyledGlowDispatch {
                bind_group: create_bind_group(
                    device,
                    "bg Glow map level",
                    &self.layouts[MAP_LEVEL],
                    &[
                        buffer_binding(0, self.camera_uniforms),
                        sampler_binding(28, self.mask_sampler),
                        buffer_binding(33, self.mask_data),
                        texture_binding(120, &map.texture.levels[mip]),
                        texture_binding(121, &map.texture.levels[mip - 1]),
                    ],
                ),
                texels: mip_size(map.grid, mip as u32),
            })
            .collect();
        StyledGlowBindGroups {
            near_first,
            near_levels,
            map_accumulate,
            map_resolve,
            map_levels,
            map_cells: map.cells.clone(),
        }
    }
}

/// The pyramids a pipeline binds: its own or placeholders, and a map of its
/// own or one it shares.
pub(super) struct StyledGlowInputs {
    placeholder_near: StyledGlowNear,
    placeholder_map: Arc<StyledGlowMap>,
    near: Option<StyledGlowNear>,
    map: Option<Arc<StyledGlowMap>>,
    /// The map is this pipeline's to build, not a full frame's it shares.
    owns_map: bool,
}

impl StyledGlowInputs {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            placeholder_near: StyledGlowNear::placeholder(device),
            placeholder_map: Arc::new(StyledGlowMap::placeholder(device)),
            near: None,
            map: None,
            owns_map: false,
        }
    }

    pub(super) fn near(&self) -> &StyledGlowNear {
        self.near.as_ref().unwrap_or(&self.placeholder_near)
    }

    pub(super) fn map(&self) -> &StyledGlowMap {
        self.map.as_deref().unwrap_or(&self.placeholder_map)
    }

    /// The near pyramid's view of every level and the map's, as the creative
    /// pass reads them.
    pub(super) fn creative_views(&self) -> (&wgpu::TextureView, &wgpu::TextureView) {
        (&self.near().texture.all, &self.map().texture.all)
    }

    /// Whether a near pyramid for `layers` slots of an image of `image`
    /// pixels is allocated.
    pub(super) fn near_fits(&self, image: [u32; 2], layers: u32) -> bool {
        self.near
            .as_ref()
            .is_some_and(|near| near.size == near_size(image) && near.layers >= layers)
    }

    /// Allocates the near pyramid for `layers` slots of an image of `image`
    /// pixels, releasing the one it replaces first.
    pub(super) fn allocate_near(
        &mut self,
        device: &wgpu::Device,
        image: [u32; 2],
        layers: u32,
    ) -> Result<()> {
        self.near = None;
        self.near = Some(StyledGlowNear::reserved(device, image, layers)?);
        Ok(())
    }

    /// Whether this pipeline owns a map of `grid` cells for `layers` slots.
    pub(super) fn owned_map_fits(&self, grid: [u32; 2], layers: u32) -> bool {
        self.owns_map && self.map.as_ref().is_some_and(|map| map.fits(grid, layers))
    }

    /// Allocates a map of this pipeline's own, releasing the one it replaces
    /// first.
    pub(super) fn allocate_map(
        &mut self,
        device: &wgpu::Device,
        grid: [u32; 2],
        layers: u32,
    ) -> Result<()> {
        self.map = None;
        self.owns_map = false;
        self.map = Some(Arc::new(StyledGlowMap::reserved(device, grid, layers)?));
        self.owns_map = true;
        Ok(())
    }

    /// Shares `map`, a full frame's; whether the bound map changed.
    pub(super) fn share_map(&mut self, map: &Arc<StyledGlowMap>) -> bool {
        if self
            .map
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, map))
        {
            return false;
        }
        self.map = Some(Arc::clone(map));
        self.owns_map = false;
        true
    }

    /// This pipeline's map, own or shared.
    pub(super) fn shared_map(&self) -> Option<Arc<StyledGlowMap>> {
        self.map.clone()
    }

    /// Whether this pipeline builds the map it binds.
    pub(super) fn owns_map(&self) -> bool {
        self.owns_map
    }

    /// Whether a map is bound that holds `layers` slots.
    pub(super) fn map_holds(&self, layers: u32) -> bool {
        self.map.as_ref().is_some_and(|map| map.layers >= layers)
    }

    /// The bind groups of the passes over these pyramids. The map's passes
    /// write a map only this pipeline owns; with a shared one they bind the
    /// placeholder.
    pub(super) fn bind_groups(
        &self,
        device: &wgpu::Device,
        sources: &StyledGlowSources<'_>,
    ) -> StyledGlowBindGroups {
        let map = if self.owns_map {
            self.map()
        } else {
            &self.placeholder_map
        };
        sources.bind_groups(device, self.near(), map)
    }
}

/// The bind group layouts of the Glow's passes, in program order.
pub(super) fn styled_glow_layout_entries() -> [Vec<wgpu::BindGroupLayoutEntry>; STYLED_GLOW_PASSES]
{
    let level_out = wgpu::BindGroupLayoutEntry {
        binding: 120,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: STYLED_GLOW_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        },
        count: None,
    };
    let parent = texture_array_entry(121, wgpu::TextureSampleType::Float { filterable: true });
    let input = texture_entry(24, wgpu::TextureSampleType::Float { filterable: false });
    let mask = texture_array_entry(27, wgpu::TextureSampleType::Float { filterable: true });
    [
        vec![
            buffer_entry(0),
            input,
            mask,
            sampler_entry(28),
            storage_buffer_entry(33, true),
            level_out,
        ],
        vec![
            buffer_entry(0),
            sampler_entry(28),
            storage_buffer_entry(33, true),
            level_out,
            parent,
            buffer_entry(123),
        ],
        vec![
            buffer_entry(0),
            input,
            mask,
            sampler_entry(28),
            storage_buffer_entry(33, true),
            storage_buffer_entry(122, false),
        ],
        vec![
            buffer_entry(0),
            storage_buffer_entry(33, true),
            storage_buffer_entry(122, false),
            level_out,
        ],
        vec![
            buffer_entry(0),
            sampler_entry(28),
            storage_buffer_entry(33, true),
            level_out,
            parent,
        ],
    ]
}

pub(super) const STYLED_GLOW_LAYOUT_LABELS: [&str; STYLED_GLOW_PASSES] = [
    "bgl Glow near pyramid first level",
    "bgl Glow near pyramid level",
    "bgl Glow map accumulation",
    "bgl Glow map resolve",
    "bgl Glow map level",
];

/// The layers the pyramids need for `params`: one per Glow slot.
pub(super) fn styled_glow_layers(params: &GpuParams) -> u32 {
    params.styled_glow_slots() as u32
}

impl RawGpuPipeline {
    /// The full image's size at this pipeline's processing resolution.
    fn styled_glow_full_size(params: &GpuParams) -> [u32; 2] {
        [params.camera.full_width, params.camera.full_height]
    }

    /// Allocates the pyramids `params` needs: the near one for any Glow, and
    /// for a pipeline that renders the whole image a map of its own, which it
    /// builds whatever the spread so that zoomed views can share it. Returns
    /// whether the near pyramid and the map are ready to be read.
    pub(super) fn prepare_styled_glow(&self, params: &GpuParams) -> (bool, bool) {
        let layers = styled_glow_layers(params);
        if layers == 0 {
            return (false, false);
        }
        let image = [params.camera.width, params.camera.height];
        let near = match self.effect_inputs.allocate_styled_glow_near(image, layers) {
            Ok(()) => true,
            Err(error) => {
                log::error!("Glow renders without its near pyramid: {error:#}");
                false
            }
        };
        if params.renders_whole_image() {
            let grid = styled_glow_map_grid(Self::styled_glow_full_size(params));
            if let Err(error) = self.effect_inputs.allocate_styled_glow_map(grid, layers) {
                log::error!("Glow renders without its full-image map: {error:#}");
            }
        }
        let map =
            params.needs_styled_glow_map() && self.effect_inputs.styled_glow_map_holds(layers);
        (near, map)
    }

    /// Builds the near pyramid from the creative pass's input, in the second
    /// work texture when `image_in_tex2`, and, for a pipeline that renders
    /// the whole image, the full-image map.
    pub(super) fn encode_styled_glow(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        params: &GpuParams,
        image_in_tex2: bool,
    ) {
        let groups = self.effect_inputs.styled_glow_bind_groups();
        let input = usize::from(image_in_tex2);
        self.dispatch_styled_glow(encoder, NEAR_FIRST, &groups.near_first[input]);
        for dispatch in &groups.near_levels {
            self.dispatch_styled_glow(encoder, NEAR_LEVEL, dispatch);
        }
        if params.renders_whole_image() && self.effect_inputs.owns_styled_glow_map() {
            encoder.clear_buffer(&groups.map_cells, 0, None);
            self.dispatch_styled_glow(encoder, MAP_ACCUMULATE, &groups.map_accumulate[input]);
            self.encode_styled_glow_map_levels(encoder, &groups);
        }
    }

    /// The map's first level from its sums, then the levels after it.
    fn encode_styled_glow_map_levels(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        groups: &StyledGlowBindGroups,
    ) {
        self.dispatch_styled_glow(encoder, MAP_RESOLVE, &groups.map_resolve);
        for dispatch in &groups.map_levels {
            self.dispatch_styled_glow(encoder, MAP_LEVEL, dispatch);
        }
    }

    fn dispatch_styled_glow(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        program: usize,
        dispatch: &StyledGlowDispatch,
    ) {
        let pass = &self.passes[self.indices.styled_glow_pass_index + program];
        dispatch_compute(
            encoder,
            &pass.pipeline.entry,
            pass.pipeline.get(),
            &[
                &dispatch.bind_group,
                &self.scene_tone_bind_group,
                &self.effects_bind_group,
            ],
            [
                dispatch.texels[0].div_ceil(WORKGROUP_EDGE),
                dispatch.texels[1].div_ceil(WORKGROUP_EDGE),
                1,
            ],
        );
    }

    /// Starts an export's Glow map prepass when `params`, any tile's, read the
    /// map: allocates it and clears its sums. Returns whether the tiles must
    /// add to it ([`Self::accumulate_export_styled_glow_tile`]).
    pub fn begin_export_styled_glow(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
    ) -> Result<bool> {
        if !params.needs_styled_glow_map() {
            return Ok(false);
        }
        let grid = styled_glow_map_grid(Self::styled_glow_full_size(params));
        self.effect_inputs
            .allocate_styled_glow_map(grid, styled_glow_layers(params))
            .context("allocate the Glow's full-image map")?;
        let groups = self.effect_inputs.styled_glow_bind_groups();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export Glow map clear"),
        });
        encoder.clear_buffer(&groups.map_cells, 0, None);
        queue.submit(Some(encoder.finish()));
        Ok(true)
    }

    /// Adds the core of the export tile `params` describe to the Glow map:
    /// renders the tile up to the creative pass's input from the RAW stage
    /// it holds, whose tone statistics the tone prepass resolved. `params`
    /// must set the tile's core as its tone histogram bounds.
    pub fn accumulate_export_styled_glow_tile(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
    ) {
        self.upload_params(queue, params);
        let groups = self.effect_inputs.styled_glow_bind_groups();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export Glow map tile"),
        });
        let timing = stage_timing::begin(device, || self.timed_stage_name("Export Glow map"));
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        self.ensure_output_inputs(&mut encoder, params);
        let image_in_tex2 = self.encode_creative_input(&mut encoder, params);
        self.dispatch_styled_glow(
            &mut encoder,
            MAP_ACCUMULATE,
            &groups.map_accumulate[usize::from(image_in_tex2)],
        );
        stage_timing::submit(queue, encoder, timing);
    }

    /// Resolves the Glow map every tile added to and builds its levels.
    pub fn finish_export_styled_glow(&self, queue: &wgpu::Queue, device: &wgpu::Device) {
        let groups = self.effect_inputs.styled_glow_bind_groups();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export Glow map levels"),
        });
        self.encode_styled_glow_map_levels(&mut encoder, &groups);
        queue.submit(Some(encoder.finish()));
    }
}
