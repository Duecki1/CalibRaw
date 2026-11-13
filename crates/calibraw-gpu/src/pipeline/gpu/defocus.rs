//! The GPU side of Lens Blur (`mask_effects/defocus.wgsl`): the textures its
//! passes build, one group of layers per Lens Blur slot, and the passes.
//!
//! - Local: the focus slab's sources per pixel, the sources and slabs of
//!   bins 0 to 2 at their levels of this pipeline's image (one mip per level
//!   from the first bin's, [`defocus_levels`]), the fill pyramid, the
//!   occupancy flags that let empty blocks skip their gather, and the result
//!   the first Blur stage blends. Every pipeline with a Lens Blur builds them
//!   in its output stage, before the Blur passes.
//! - Map: the sums, sources and slabs of bins 3 to 5 over a grid of the whole
//!   image ([`DEFOCUS_MAP_LONG`] cells along its longer edge, at most one per
//!   pixel, halved per bin) and the fill's coarser levels. A pipeline that renders the whole image
//!   builds it whenever a Lens Blur reaches those bins, so a zoomed view can
//!   share it ([`EffectInputs::share_defocus_map`]); an export builds it once
//!   in a prepass over its tiles ([`RawGpuPipeline::begin_export_defocus`]).
//!
//! Both are allocated when a Lens Blur first renders, each with a budget
//! reservation of its own, so pipelines whose edits hold none reserve
//! nothing. Until then the passes bind placeholders, which the shader never
//! reads: the first Blur stage ignores a result of another size.

use super::styled_glow::{full_image_grid, mip_levels, mip_size, texture_bytes, LevelTexture};
use super::*;
use crate::pipeline::effect_lanes::defocus::RADIUS;

/// The programs of Lens Blur's passes, in the order the pass plan places them.
pub(super) const DEFOCUS_PASSES: usize = 8;
pub(super) const DEFOCUS_ENTRIES: [&str; DEFOCUS_PASSES] = [
    "prepare_defocus",
    "build_defocus_sources",
    "gather_defocus",
    "build_defocus_fill",
    "halve_defocus_fill",
    "composite_defocus",
    "accumulate_defocus_map",
    "resolve_defocus_map",
];
const PREPARE: usize = 0;
const SOURCES: usize = 1;
const GATHER: usize = 2;
const FILL: usize = 3;
const HALVE: usize = 4;
const COMPOSITE: usize = 5;
const MAP_ACCUMULATE: usize = 6;
const MAP_RESOLVE: usize = 7;

pub(super) const DEFOCUS_LAYOUT_LABELS: [&str; DEFOCUS_PASSES] = [
    "bgl Lens Blur focus sources",
    "bgl Lens Blur bin sources",
    "bgl Lens Blur gather",
    "bgl Lens Blur fill",
    "bgl Lens Blur fill level",
    "bgl Lens Blur composite",
    "bgl Lens Blur map accumulation",
    "bgl Lens Blur map resolve",
];

/// The focus slab's largest radius ρ0 in shorter edges
/// (`DEFOCUS_FOCUS_RADIUS` in defocus.wgsl); bin b starts at ρ0·2^b.
pub(super) const DEFOCUS_FOCUS_RADIUS: f32 = 1.0 / 512.0;
/// Bins gathered from a pipeline's own pixels; the next three are the map's.
pub(super) const LOCAL_BINS: usize = 3;
const MAP_BINS: u32 = 3;
/// Octaves over which a bin's edge fades (`DEFOCUS_BIN_SOFTNESS`).
const BIN_SOFTNESS: f32 = 0.1;
/// A bin's widest radius in its starting radius (`DEFOCUS_BIN_REACH`).
const BIN_REACH: f32 = 2.144;
/// Cells of the map along the image's longer edge for bin 3
/// (`DEFOCUS_MAP_LONG` in defocus.wgsl).
pub(super) const DEFOCUS_MAP_LONG: u32 = 768;
/// The map's slab mip of each map bin (`defocus_map_slab_mip`): bins 4 and
/// 5 are written at twice their sources' resolution, so their rims, one
/// source texel wide, are drawn smoothly.
const MAP_SLAB_MIPS: [u32; MAP_BINS as usize] = [0, 0, 1];
pub(super) const SOURCE_LAYERS: u32 = 5;
pub(super) const SLAB_LAYERS: u32 = 10;
/// Vectors of a map cell's sums (`DEFOCUS_CELL_VECTORS`).
pub(super) const CELL_VECTORS: u64 = 5;
const CELL_BYTES: u64 = 16;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Whether `mask` is an active Lens Blur slot, which takes layers.
pub(super) fn is_defocus(mask: &MaskData) -> bool {
    mask.applies(MaskEffect::Defocus)
}

/// Whether the Lens Blur slot `mask` reaches the map's bins on an image of
/// `aspect` (longer over shorter edge): its largest radius, in shorter edges,
/// reaches past bin 2's soft edge.
pub(super) fn defocus_reaches_map(mask: &MaskData, aspect: f32) -> bool {
    let first_map_bin = DEFOCUS_FOCUS_RADIUS * (LOCAL_BINS as f32 - BIN_SOFTNESS).exp2();
    mask.lane(RADIUS) * aspect > first_map_bin
}

/// The pipeline level of each local bin for an image whose shorter edge is
/// `short` pixels: the level whose texel is a quarter of bin 0's starting
/// radius and an eighth of the others', so every bin spans about 4 to 17
/// texels (`defocus_local_level` in defocus.wgsl reads them from the pass
/// uniforms, so they are decided here once).
pub(super) fn defocus_levels(short: u32) -> [u32; LOCAL_BINS] {
    std::array::from_fn(|bin| {
        let start = DEFOCUS_FOCUS_RADIUS * (1u32 << bin) as f32 * short.max(1) as f32;
        let texel = start / if bin == 0 { 4.0 } else { 8.0 };
        texel.log2().round().max(0.0) as u32
    })
}

/// The mips of the local textures from the first bin's level: through the
/// last bin's and the fill's, which reaches past bin 2's widest radius.
fn local_mips(short: u32, levels: [u32; LOCAL_BINS]) -> u32 {
    let widest = DEFOCUS_FOCUS_RADIUS * 4.0 * BIN_REACH * short.max(1) as f32;
    let fill_top = (widest.max(1.0).log2().ceil() as u32 + 1).max(levels[2]);
    fill_top - levels[0] + 1
}

/// The first mip's size of a local texture of `mips` levels from level
/// `first` over an image side of `side` pixels: every mip must hold the
/// texels of its full-image grid the image touches at any tile origin, one
/// more than the image's extent, and the sizes must halve exactly.
fn local_side(side: u32, first: u32, mips: u32) -> u32 {
    let unit = 1 << (mips - 1);
    unit * (side.div_ceil(1 << (first + mips - 1)) + 1)
}

/// The map's grid for bin 3 (`defocus_map_grid` in defocus.wgsl): at most
/// one cell per pixel along the longer edge (`defocus_map_long`), since a
/// finer cell may hold no pixel's centre and the empty cells would show as a
/// lattice in small whole-image renders.
fn defocus_map_grid(params: &GpuParams) -> [u32; 2] {
    let full = [params.camera.full_width, params.camera.full_height];
    let long = full[0].max(full[1]).clamp(1, DEFOCUS_MAP_LONG);
    full_image_grid(full, long)
}

/// Blocks of occupancy flags per region (`defocus_occupancy_region`).
fn occupancy_blocks([width, height]: [u32; 2]) -> u64 {
    u64::from(width.div_ceil(8) + 1) * u64::from(height.div_ceil(8) + 1)
}

/// A pass's uniforms (`DefocusPass` in defocus.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
struct DefocusPassUniform {
    bin: u32,
    mip: u32,
    map_ready: u32,
    unused: u32,
    levels: [u32; 4],
}

fn uniform_buffer(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    create_gpu_buffer(
        device,
        label,
        std::mem::size_of::<DefocusPassUniform>() as u64,
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    )
}

/// What the local passes build over this pipeline's image.
pub(super) struct DefocusLocal {
    focus: LevelTexture,
    sources: LevelTexture,
    slabs: LevelTexture,
    result: LevelTexture,
    occupancy: wgpu::Buffer,
    /// A pass's uniforms: the bins' (also the fill's first level, prepare's
    /// and composite's), then each fill mip's from mip 1.
    bin_uniforms: [wgpu::Buffer; LOCAL_BINS],
    fill_uniforms: Vec<wgpu::Buffer>,
    image: [u32; 2],
    short: u32,
    levels: [u32; LOCAL_BINS],
    layers: u32,
    _reservation: Option<GpuBudgetReservation>,
}

impl DefocusLocal {
    fn build(
        device: &wgpu::Device,
        image: [u32; 2],
        short: u32,
        layers: u32,
        reservation: Option<GpuBudgetReservation>,
    ) -> Self {
        let levels = defocus_levels(short);
        let mips = local_mips(short, levels);
        let size = Self::level_size(image, levels, mips);
        Self {
            focus: LevelTexture::new(
                device,
                image,
                1,
                2 * layers,
                "calibraw Lens Blur focus sources",
            ),
            sources: LevelTexture::new(
                device,
                size,
                mips,
                layers * LOCAL_BINS as u32 * SOURCE_LAYERS,
                "calibraw Lens Blur sources",
            ),
            slabs: LevelTexture::new(
                device,
                size,
                mips,
                layers * SLAB_LAYERS,
                "calibraw Lens Blur slabs",
            ),
            result: LevelTexture::new(device, image, 1, layers, "calibraw Lens Blur result"),
            occupancy: create_gpu_buffer(
                device,
                "calibraw Lens Blur occupancy",
                Self::occupancy_bytes(image, layers),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            bin_uniforms: std::array::from_fn(|_| {
                uniform_buffer(device, "calibraw Lens Blur bin pass")
            }),
            fill_uniforms: (1..mips)
                .map(|_| uniform_buffer(device, "calibraw Lens Blur fill pass"))
                .collect(),
            image,
            short,
            levels,
            layers,
            _reservation: reservation,
        }
    }

    fn level_size(image: [u32; 2], levels: [u32; LOCAL_BINS], mips: u32) -> [u32; 2] {
        image.map(|side| local_side(side, levels[0], mips))
    }

    fn occupancy_bytes(image: [u32; 2], layers: u32) -> u64 {
        occupancy_blocks(image) * 4 * u64::from(layers) * 4
    }

    /// Never read: the first Blur stage ignores a result of another size.
    fn placeholder(device: &wgpu::Device) -> Self {
        Self::build(device, [1, 1], 1, 1, None)
    }

    /// Textures for `layers` Lens Blur slots over an image of `image`
    /// pixels whose full image's shorter edge is `short` pixels.
    fn reserved(device: &wgpu::Device, image: [u32; 2], short: u32, layers: u32) -> Result<Self> {
        let levels = defocus_levels(short);
        let mips = local_mips(short, levels);
        let size = Self::level_size(image, levels, mips);
        let bytes = texture_bytes(image, 1, 2 * layers)
            + texture_bytes(
                size,
                mips,
                layers * (LOCAL_BINS as u32 * SOURCE_LAYERS + SLAB_LAYERS),
            )
            + texture_bytes(image, 1, layers)
            + Self::occupancy_bytes(image, layers);
        let reservation = GpuBudgetReservation::acquire_bytes(
            "Lens Blur textures",
            bytes,
            gpu_working_set_limit_bytes(),
        )?;
        Ok(Self::build(device, image, short, layers, Some(reservation)))
    }

    fn fits(&self, image: [u32; 2], short: u32, layers: u32) -> bool {
        self.image == image && self.short == short && self.layers >= layers
    }

    fn mips(&self) -> u32 {
        self.slabs.levels.len() as u32
    }
}

/// The full-image map of bins 3 to 5 and the fill's coarse levels.
pub(super) struct DefocusMap {
    cells: wgpu::Buffer,
    sources: LevelTexture,
    slabs: LevelTexture,
    /// Each map bin's pass uniforms, then each fill mip's from mip 1.
    bin_uniforms: Vec<wgpu::Buffer>,
    fill_uniforms: Vec<wgpu::Buffer>,
    grid: [u32; 2],
    layers: u32,
    _reservation: Option<GpuBudgetReservation>,
}

impl DefocusMap {
    fn build(
        device: &wgpu::Device,
        grid: [u32; 2],
        layers: u32,
        reservation: Option<GpuBudgetReservation>,
    ) -> Self {
        let bin_uniforms = (0..MAP_BINS)
            .map(|bin| {
                create_initialized_buffer(
                    device,
                    "calibraw Lens Blur map pass",
                    bytemuck::bytes_of(&DefocusPassUniform {
                        bin: LOCAL_BINS as u32 + bin,
                        mip: MAP_SLAB_MIPS[bin as usize],
                        ..Default::default()
                    }),
                    wgpu::BufferUsages::UNIFORM,
                )
            })
            .collect();
        let mips = mip_levels(grid);
        let fill_uniforms = (1..mips)
            .map(|mip| {
                create_initialized_buffer(
                    device,
                    "calibraw Lens Blur map fill pass",
                    bytemuck::bytes_of(&DefocusPassUniform {
                        bin: LOCAL_BINS as u32,
                        mip,
                        ..Default::default()
                    }),
                    wgpu::BufferUsages::UNIFORM,
                )
            })
            .collect();
        Self {
            cells: create_gpu_buffer(
                device,
                "calibraw Lens Blur map sums",
                Self::cells_bytes(grid, layers).max(CELL_BYTES),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            sources: LevelTexture::new(
                device,
                grid,
                MAP_BINS.min(mips),
                layers * LOCAL_BINS as u32 * SOURCE_LAYERS,
                "calibraw Lens Blur map sources",
            ),
            slabs: LevelTexture::new(
                device,
                grid,
                mips,
                layers * SLAB_LAYERS,
                "calibraw Lens Blur map slabs",
            ),
            bin_uniforms,
            fill_uniforms,
            grid,
            layers,
            _reservation: reservation,
        }
    }

    /// The bytes of the sums: per slot each map bin's cells at its grid,
    /// then the fill's at bin 3's (`defocus_cells_per_slot`).
    fn cells_bytes(grid: [u32; 2], layers: u32) -> u64 {
        let cells: u64 = (0..MAP_BINS)
            .map(|bin| {
                let [width, height] = mip_size(grid, bin);
                u64::from(width) * u64::from(height) * CELL_VECTORS
            })
            .sum::<u64>()
            + u64::from(grid[0]) * u64::from(grid[1]);
        cells * CELL_BYTES * u64::from(layers)
    }

    /// Never read: the composite is told no map is built.
    fn placeholder(device: &wgpu::Device) -> Self {
        Self::build(device, [2, 2], 1, None)
    }

    fn reserved(device: &wgpu::Device, grid: [u32; 2], layers: u32) -> Result<Self> {
        let mips = mip_levels(grid);
        let bytes = Self::cells_bytes(grid, layers)
            + texture_bytes(
                grid,
                MAP_BINS.min(mips),
                layers * LOCAL_BINS as u32 * SOURCE_LAYERS,
            )
            + texture_bytes(grid, mips, layers * SLAB_LAYERS);
        let reservation = GpuBudgetReservation::acquire_bytes(
            "Lens Blur full-image map",
            bytes,
            gpu_working_set_limit_bytes(),
        )?;
        Ok(Self::build(device, grid, layers, Some(reservation)))
    }

    fn fits(&self, grid: [u32; 2], layers: u32) -> bool {
        self.grid == grid && self.layers >= layers
    }
}

/// One dispatch of a Lens Blur pass: its program, bind group and the texels
/// it covers per slot.
#[derive(Clone)]
pub(super) struct DefocusDispatch {
    program: usize,
    bind_group: wgpu::BindGroup,
    texels: [u32; 2],
}

/// The dispatches of Lens Blur's passes over the bound textures.
#[derive(Clone)]
pub(super) struct DefocusBindGroups {
    /// The local passes in order: prepare, each bin's sources, each bin's
    /// gather, the fill and its levels, the composite.
    local: Vec<DefocusDispatch>,
    /// Map accumulation per bin, then its resolve per bin, gathers and fill
    /// levels.
    map_accumulate: Vec<DefocusDispatch>,
    map_levels: Vec<DefocusDispatch>,
    occupancy: wgpu::Buffer,
    map_cells: wgpu::Buffer,
    /// The view the first Blur stage reads.
    pub(super) result: wgpu::TextureView,
}

impl DefocusBindGroups {
    /// The bind group of `program`'s first dispatch, which the pass list
    /// records (`InputBoundPass::Defocus`).
    pub(super) fn first(&self, program: usize) -> &wgpu::BindGroup {
        &self
            .local
            .iter()
            .chain(&self.map_accumulate)
            .chain(&self.map_levels)
            .find(|dispatch| dispatch.program == program)
            .unwrap_or(&self.local[0])
            .bind_group
    }
}

/// What Lens Blur's passes bind besides its textures.
pub(super) struct DefocusSources<'a> {
    pub(super) layouts: &'a [wgpu::BindGroupLayout; DEFOCUS_PASSES],
    pub(super) camera_uniforms: &'a wgpu::Buffer,
    pub(super) mask_data: &'a wgpu::Buffer,
    pub(super) profile: &'a wgpu::Buffer,
    pub(super) scene: &'a wgpu::TextureView,
    /// The first Blur stage's input.
    pub(super) tex1: &'a wgpu::TextureView,
    pub(super) mask: &'a wgpu::TextureView,
    pub(super) mask_sampler: &'a wgpu::Sampler,
    pub(super) scene_bake_uniforms: &'a wgpu::Buffer,
    pub(super) scene_bake_layers: &'a wgpu::TextureView,
}

impl DefocusSources<'_> {
    /// What the passes that read the photo's pixels bind.
    fn pixel_entries(&self) -> Vec<wgpu::BindGroupEntry<'_>> {
        vec![
            buffer_binding(0, self.camera_uniforms),
            texture_binding(11, self.scene),
            buffer_binding(20, self.profile),
            texture_binding(24, self.tex1),
            texture_binding(27, self.mask),
            sampler_binding(28, self.mask_sampler),
            buffer_binding(33, self.mask_data),
            buffer_binding(85, self.scene_bake_uniforms),
            texture_binding(113, self.scene_bake_layers),
        ]
    }

    fn bind_groups(
        &self,
        device: &wgpu::Device,
        local: &DefocusLocal,
        map: &DefocusMap,
    ) -> DefocusBindGroups {
        let pixels = |program: usize, label: &str, extra: &[wgpu::BindGroupEntry<'_>]| {
            let mut entries = self.pixel_entries();
            entries.extend_from_slice(extra);
            create_bind_group(device, label, &self.layouts[program], &entries)
        };
        let gather = |label: &str,
                      sources: &wgpu::TextureView,
                      out: &wgpu::TextureView,
                      uniform: &wgpu::Buffer,
                      occupancy: &wgpu::Buffer| {
            create_bind_group(
                device,
                label,
                &self.layouts[GATHER],
                &[
                    buffer_binding(0, self.camera_uniforms),
                    buffer_binding(33, self.mask_data),
                    texture_binding(127, sources),
                    texture_binding(130, out),
                    buffer_binding(131, uniform),
                    buffer_binding(132, occupancy),
                ],
            )
        };
        let halve = |label: &str,
                     parent: &wgpu::TextureView,
                     out: &wgpu::TextureView,
                     uniform: &wgpu::Buffer| {
            create_bind_group(
                device,
                label,
                &self.layouts[HALVE],
                &[
                    buffer_binding(0, self.camera_uniforms),
                    buffer_binding(33, self.mask_data),
                    texture_binding(129, parent),
                    texture_binding(130, out),
                    buffer_binding(131, uniform),
                ],
            )
        };
        let image = local.image;
        let level_texels = |level: u32| mip_size(local.slabs_size(), level - local.levels[0]);

        let mut local_dispatches = vec![DefocusDispatch {
            program: PREPARE,
            bind_group: pixels(
                PREPARE,
                "bg Lens Blur focus sources",
                &[
                    texture_binding(126, &local.focus.levels[0]),
                    buffer_binding(131, &local.bin_uniforms[0]),
                    buffer_binding(132, &local.occupancy),
                ],
            ),
            texels: image,
        }];
        for bin in 0..LOCAL_BINS {
            let mip = (local.levels[bin] - local.levels[0]) as usize;
            local_dispatches.push(DefocusDispatch {
                program: SOURCES,
                bind_group: pixels(
                    SOURCES,
                    "bg Lens Blur bin sources",
                    &[
                        texture_binding(128, &local.sources.levels[mip]),
                        buffer_binding(131, &local.bin_uniforms[bin]),
                        buffer_binding(132, &local.occupancy),
                    ],
                ),
                texels: level_texels(local.levels[bin]),
            });
        }
        for bin in 0..LOCAL_BINS {
            let mip = (local.levels[bin] - local.levels[0]) as usize;
            local_dispatches.push(DefocusDispatch {
                program: GATHER,
                bind_group: gather(
                    "bg Lens Blur gather",
                    &local.sources.all,
                    &local.slabs.levels[mip],
                    &local.bin_uniforms[bin],
                    &local.occupancy,
                ),
                texels: level_texels(local.levels[bin]),
            });
        }
        local_dispatches.push(DefocusDispatch {
            program: FILL,
            bind_group: pixels(
                FILL,
                "bg Lens Blur fill",
                &[
                    texture_binding(130, &local.slabs.levels[0]),
                    buffer_binding(131, &local.bin_uniforms[0]),
                ],
            ),
            texels: level_texels(local.levels[0]),
        });
        for mip in 1..local.mips() as usize {
            local_dispatches.push(DefocusDispatch {
                program: HALVE,
                bind_group: halve(
                    "bg Lens Blur fill level",
                    &local.slabs.levels[mip - 1],
                    &local.slabs.levels[mip],
                    &local.fill_uniforms[mip - 1],
                ),
                texels: mip_size(local.slabs_size(), mip as u32),
            });
        }
        local_dispatches.push(DefocusDispatch {
            program: COMPOSITE,
            bind_group: create_bind_group(
                device,
                "bg Lens Blur composite",
                &self.layouts[COMPOSITE],
                &[
                    buffer_binding(0, self.camera_uniforms),
                    sampler_binding(28, self.mask_sampler),
                    buffer_binding(33, self.mask_data),
                    texture_binding(125, &local.focus.all),
                    texture_binding(129, &local.slabs.all),
                    buffer_binding(131, &local.bin_uniforms[0]),
                    buffer_binding(132, &local.occupancy),
                    texture_binding(133, &map.slabs.all),
                    texture_binding(135, &local.result.levels[0]),
                ],
            ),
            texels: image,
        });

        let map_accumulate = (0..MAP_BINS as usize)
            .map(|bin| DefocusDispatch {
                program: MAP_ACCUMULATE,
                bind_group: pixels(
                    MAP_ACCUMULATE,
                    "bg Lens Blur map accumulation",
                    &[
                        buffer_binding(131, &map.bin_uniforms[bin]),
                        buffer_binding(134, &map.cells),
                    ],
                ),
                texels: mip_size(map.grid, bin as u32),
            })
            .collect();
        let mut map_levels: Vec<DefocusDispatch> = (0..MAP_BINS as usize)
            .map(|bin| DefocusDispatch {
                program: MAP_RESOLVE,
                bind_group: create_bind_group(
                    device,
                    "bg Lens Blur map resolve",
                    &self.layouts[MAP_RESOLVE],
                    &[
                        buffer_binding(0, self.camera_uniforms),
                        buffer_binding(33, self.mask_data),
                        texture_binding(
                            128,
                            &map.sources.levels[bin.min(map.sources.levels.len() - 1)],
                        ),
                        texture_binding(130, &map.slabs.levels[0]),
                        buffer_binding(131, &map.bin_uniforms[bin]),
                        buffer_binding(134, &map.cells),
                    ],
                ),
                texels: mip_size(map.grid, bin as u32),
            })
            .collect();
        for (bin, &slab_mip) in MAP_SLAB_MIPS.iter().enumerate() {
            let mip = (slab_mip as usize).min(map.slabs.levels.len() - 1);
            map_levels.push(DefocusDispatch {
                program: GATHER,
                bind_group: gather(
                    "bg Lens Blur map gather",
                    &map.sources.all,
                    &map.slabs.levels[mip],
                    &map.bin_uniforms[bin],
                    &local.occupancy,
                ),
                texels: mip_size(map.grid, mip as u32),
            });
        }
        for mip in 1..map.slabs.levels.len() {
            map_levels.push(DefocusDispatch {
                program: HALVE,
                bind_group: halve(
                    "bg Lens Blur map fill level",
                    &map.slabs.levels[mip - 1],
                    &map.slabs.levels[mip],
                    &map.fill_uniforms[mip - 1],
                ),
                texels: mip_size(map.grid, mip as u32),
            });
        }
        DefocusBindGroups {
            local: local_dispatches,
            map_accumulate,
            map_levels,
            occupancy: local.occupancy.clone(),
            map_cells: map.cells.clone(),
            result: local.result.all.clone(),
        }
    }
}

impl DefocusLocal {
    fn slabs_size(&self) -> [u32; 2] {
        Self::level_size(self.image, self.levels, self.mips())
    }

    /// Writes the pass uniforms: the levels and whether the map is built.
    fn write_uniforms(&self, queue: &wgpu::Queue, map_ready: bool) {
        let levels = [self.levels[0], self.levels[1], self.levels[2], 0];
        let map_ready = u32::from(map_ready);
        for (bin, buffer) in self.bin_uniforms.iter().enumerate() {
            let uniform = DefocusPassUniform {
                bin: bin as u32,
                map_ready,
                levels,
                ..Default::default()
            };
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        }
        for (index, buffer) in self.fill_uniforms.iter().enumerate() {
            let uniform = DefocusPassUniform {
                mip: index as u32 + 1,
                map_ready,
                levels,
                ..Default::default()
            };
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&uniform));
        }
    }
}

/// The textures a pipeline binds: its own or placeholders, and a map of its
/// own or one it shares.
pub(super) struct DefocusInputs {
    placeholder_local: DefocusLocal,
    placeholder_map: Arc<DefocusMap>,
    local: Option<DefocusLocal>,
    map: Option<Arc<DefocusMap>>,
    /// The map is this pipeline's to build, not a full frame's it shares.
    owns_map: bool,
}

impl DefocusInputs {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            placeholder_local: DefocusLocal::placeholder(device),
            placeholder_map: Arc::new(DefocusMap::placeholder(device)),
            local: None,
            map: None,
            owns_map: false,
        }
    }

    fn local(&self) -> &DefocusLocal {
        self.local.as_ref().unwrap_or(&self.placeholder_local)
    }

    fn map(&self) -> &DefocusMap {
        self.map.as_deref().unwrap_or(&self.placeholder_map)
    }

    pub(super) fn local_fits(&self, image: [u32; 2], short: u32, layers: u32) -> bool {
        self.local
            .as_ref()
            .is_some_and(|local| local.fits(image, short, layers))
    }

    /// Allocates the local textures, releasing the ones they replace first.
    pub(super) fn allocate_local(
        &mut self,
        device: &wgpu::Device,
        image: [u32; 2],
        short: u32,
        layers: u32,
    ) -> Result<()> {
        self.local = None;
        self.local = Some(DefocusLocal::reserved(device, image, short, layers)?);
        Ok(())
    }

    pub(super) fn owned_map_fits(&self, grid: [u32; 2], layers: u32) -> bool {
        self.owns_map && self.map.as_ref().is_some_and(|map| map.fits(grid, layers))
    }

    pub(super) fn allocate_map(
        &mut self,
        device: &wgpu::Device,
        grid: [u32; 2],
        layers: u32,
    ) -> Result<()> {
        self.map = None;
        self.owns_map = false;
        self.map = Some(Arc::new(DefocusMap::reserved(device, grid, layers)?));
        self.owns_map = true;
        Ok(())
    }

    /// Shares `map`, a full frame's; whether the bound map changed.
    pub(super) fn share_map(&mut self, map: &Arc<DefocusMap>) -> bool {
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

    pub(super) fn shared_map(&self) -> Option<Arc<DefocusMap>> {
        self.map.clone()
    }

    pub(super) fn owns_map(&self) -> bool {
        self.owns_map
    }

    pub(super) fn map_holds(&self, layers: u32) -> bool {
        self.map.as_ref().is_some_and(|map| map.layers >= layers)
    }

    pub(super) fn write_uniforms(&self, queue: &wgpu::Queue, map_ready: bool) {
        if let Some(local) = &self.local {
            local.write_uniforms(queue, map_ready);
        }
    }

    /// The bind groups of the passes. The map's passes write a map only this
    /// pipeline owns; with a shared one they write the placeholder, while the
    /// composite reads the shared map.
    pub(super) fn bind_groups(
        &self,
        device: &wgpu::Device,
        sources: &DefocusSources<'_>,
    ) -> DefocusBindGroups {
        let mut groups = sources.bind_groups(device, self.local(), self.map());
        if !self.owns_map {
            let placeholder = sources.bind_groups(device, self.local(), &self.placeholder_map);
            groups.map_accumulate = placeholder.map_accumulate;
            groups.map_levels = placeholder.map_levels;
            groups.map_cells = placeholder.map_cells;
        }
        groups
    }
}

/// The bind group layouts of Lens Blur's passes, in program order.
pub(super) fn defocus_layout_entries() -> [Vec<wgpu::BindGroupLayoutEntry>; DEFOCUS_PASSES] {
    let float = wgpu::TextureSampleType::Float { filterable: true };
    let unfiltered = wgpu::TextureSampleType::Float { filterable: false };
    let write = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        },
        count: None,
    };
    let pixels = || {
        vec![
            buffer_entry(0),
            texture_entry(11, unfiltered),
            storage_buffer_entry(20, true),
            texture_entry(24, unfiltered),
            texture_array_entry(27, float),
            sampler_entry(28),
            storage_buffer_entry(33, true),
            buffer_entry(85),
            texture_array_entry(113, float),
        ]
    };
    let with = |mut entries: Vec<wgpu::BindGroupLayoutEntry>,
                extra: &[wgpu::BindGroupLayoutEntry]| {
        entries.extend_from_slice(extra);
        entries
    };
    [
        with(
            pixels(),
            &[
                write(126),
                buffer_entry(131),
                storage_buffer_entry(132, false),
            ],
        ),
        with(
            pixels(),
            &[
                write(128),
                buffer_entry(131),
                storage_buffer_entry(132, false),
            ],
        ),
        vec![
            buffer_entry(0),
            storage_buffer_entry(33, true),
            texture_array_entry(127, unfiltered),
            write(130),
            buffer_entry(131),
            storage_buffer_entry(132, false),
        ],
        with(pixels(), &[write(130), buffer_entry(131)]),
        vec![
            buffer_entry(0),
            storage_buffer_entry(33, true),
            texture_array_entry(129, unfiltered),
            write(130),
            buffer_entry(131),
        ],
        vec![
            buffer_entry(0),
            sampler_entry(28),
            storage_buffer_entry(33, true),
            texture_array_entry(125, unfiltered),
            texture_array_entry(129, float),
            buffer_entry(131),
            storage_buffer_entry(132, false),
            texture_array_entry(133, float),
            write(135),
        ],
        with(
            pixels(),
            &[buffer_entry(131), storage_buffer_entry(134, false)],
        ),
        vec![
            buffer_entry(0),
            storage_buffer_entry(33, true),
            write(128),
            write(130),
            buffer_entry(131),
            storage_buffer_entry(134, false),
        ],
    ]
}

/// The layers the textures need for `params`: one group per Lens Blur slot.
pub(super) fn defocus_layers(params: &GpuParams) -> u32 {
    params.defocus_slots() as u32
}

impl RawGpuPipeline {
    /// Allocates the textures `params` needs: the local ones for any Lens
    /// Blur, and for a pipeline that renders the whole image a map of its
    /// own when a Lens Blur reaches it, so that zoomed views can share it.
    /// Writes the pass uniforms. Returns whether the local textures are ready.
    pub(super) fn prepare_defocus(&self, queue: &wgpu::Queue, params: &GpuParams) -> bool {
        let layers = defocus_layers(params);
        if layers == 0 {
            return false;
        }
        let image = [params.camera.width, params.camera.height];
        let short = params.camera.full_width.min(params.camera.full_height);
        let local = match self
            .effect_inputs
            .allocate_defocus_local(image, short, layers)
        {
            Ok(()) => true,
            Err(error) => {
                log::error!("Lens Blur renders without its textures: {error:#}");
                false
            }
        };
        if params.renders_whole_image() && params.needs_defocus_map() {
            let grid = defocus_map_grid(params);
            if let Err(error) = self.effect_inputs.allocate_defocus_map(grid, layers) {
                log::error!("Lens Blur renders without its full-image map: {error:#}");
            }
        }
        let map_ready = params.needs_defocus_map() && self.effect_inputs.defocus_map_holds(layers);
        self.effect_inputs.write_defocus_uniforms(queue, map_ready);
        local
    }

    /// Encodes Lens Blur from the first Blur stage's input in the first work
    /// texture: for a pipeline that renders the whole image and owns its map,
    /// the map first, then the local passes and the composite.
    pub(super) fn encode_defocus(&self, encoder: &mut wgpu::CommandEncoder, params: &GpuParams) {
        let groups = self.effect_inputs.defocus_bind_groups();
        let layers = defocus_layers(params);
        if params.renders_whole_image()
            && params.needs_defocus_map()
            && self.effect_inputs.owns_defocus_map()
        {
            encoder.clear_buffer(&groups.map_cells, 0, None);
            for dispatch in &groups.map_accumulate {
                self.dispatch_defocus(encoder, dispatch, layers);
            }
            for dispatch in &groups.map_levels {
                self.dispatch_defocus(encoder, dispatch, layers);
            }
        }
        encoder.clear_buffer(&groups.occupancy, 0, None);
        for dispatch in &groups.local {
            self.dispatch_defocus(encoder, dispatch, layers);
        }
    }

    fn dispatch_defocus(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        dispatch: &DefocusDispatch,
        layers: u32,
    ) {
        let pass = &self.passes[self.indices.defocus_pass_index + dispatch.program];
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
                layers.max(1),
            ],
        );
    }

    /// Starts an export's Lens Blur map prepass when `params`, any tile's,
    /// reach the map: allocates it and clears its sums. Returns whether the
    /// tiles must add to it ([`Self::accumulate_export_defocus_tile`]).
    pub fn begin_export_defocus(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
    ) -> Result<bool> {
        if !params.needs_defocus_map() {
            return Ok(false);
        }
        let grid = defocus_map_grid(params);
        self.effect_inputs
            .allocate_defocus_map(grid, defocus_layers(params))
            .context("allocate Lens Blur's full-image map")?;
        let groups = self.effect_inputs.defocus_bind_groups();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export Lens Blur map clear"),
        });
        encoder.clear_buffer(&groups.map_cells, 0, None);
        queue.submit(Some(encoder.finish()));
        Ok(true)
    }

    /// Adds the core of the export tile `params` describe to the Lens Blur
    /// map: renders the tile up to the first Blur stage's input from the RAW
    /// stage it holds, whose tone statistics the tone prepass resolved.
    /// `params` must set the tile's core as its tone histogram bounds.
    pub fn accumulate_export_defocus_tile(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
    ) {
        self.upload_params(queue, params);
        let groups = self.effect_inputs.defocus_bind_groups();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export Lens Blur map tile"),
        });
        let timing = stage_timing::begin(device, || self.timed_stage_name("Export Lens Blur map"));
        self.encode_pass_range(
            &mut encoder,
            self.indices.tone_prepare_pass_index,
            self.indices.tone_reduce_pass_index,
        );
        self.ensure_output_inputs(&mut encoder, params);
        self.encode_blur_input(&mut encoder, params);
        let layers = defocus_layers(params);
        for dispatch in &groups.map_accumulate {
            self.dispatch_defocus(&mut encoder, dispatch, layers);
        }
        stage_timing::submit(queue, encoder, timing);
    }

    /// Resolves the Lens Blur map every tile added to and gathers its bins
    /// and fill levels.
    pub fn finish_export_defocus(
        &self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        params: &GpuParams,
    ) {
        let groups = self.effect_inputs.defocus_bind_groups();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export Lens Blur map levels"),
        });
        let layers = defocus_layers(params);
        for dispatch in &groups.map_levels {
            self.dispatch_defocus(&mut encoder, dispatch, layers);
        }
        queue.submit(Some(encoder.finish()));
    }
}
