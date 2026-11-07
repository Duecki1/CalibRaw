//! Export geometry on the GPU (`shaders/geometry_resample.wgsl`): resamples the developed,
//! display-linear photo through the edit's crop, quarter turns, flips, rotation, perspective
//! and lens correction into the output raster.
//!
//! The source stays in CPU memory and the GPU holds a bounded part of it. A first pass finds
//! the source rectangle each [`CELL_EDGE`]-pixel square of the output reads. The output is
//! then resampled in bands one cell high: a band's cells are grouped into tiles whose source
//! rectangle fits [`CHUNK_BYTES`], and a tile whose rectangle alone is larger (a strong
//! reduction) is uploaded in runs of rows. Taps are added in source-row order however a
//! rectangle is split, so the split never changes a pixel. One band renders on the GPU while
//! the caller consumes the previous one.

use super::*;
use crate::pipeline::geometry::{GeometryInverseMap, Homography};

/// Output pixels per footprint-cell edge, and rows per band. A multiple of the shader's
/// workgroup edge, so each footprint workgroup lies in one cell.
const CELL_EDGE: u32 = 64;
/// `@workgroup_size` of every geometry_resample.wgsl entry point.
pub(super) const GEOMETRY_WORKGROUP_EDGE: u32 = 8;
const _: () = assert!(CELL_EDGE.is_multiple_of(GEOMETRY_WORKGROUP_EDGE));
/// Largest source rectangle on the GPU at once.
const CHUNK_BYTES: u64 = 32 << 20;
/// Interleaved f32 RGB.
const RGB_BYTES: u64 = 12;
/// A band pixel's accumulator: two `vec4<f32>`.
const ACCUMULATOR_BYTES: u64 = 32;
/// `GEOMETRY_AFFINE` and `GEOMETRY_LENS` in geometry_resample.wgsl.
pub(super) const GEOMETRY_AFFINE: u32 = 1;
pub(super) const GEOMETRY_LENS: u32 = 2;

/// `GeometryResampleParams` in geometry_resample.wgsl, where each field is described.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GeometryResampleParams {
    pub(super) source_from_output: [[f32; 4]; 3],
    pub(super) source_size: [u32; 2],
    pub(super) raster_size: [u32; 2],
    pub(super) window: [u32; 4],
    pub(super) lens_map_size: [u32; 2],
    pub(super) lens_grid_size: [u32; 2],
    pub(super) band: [u32; 2],
    pub(super) region: [u32; 2],
    pub(super) chunk: [u32; 4],
    pub(super) cell_edge: u32,
    pub(super) cells_per_row: u32,
    pub(super) flags: u32,
    pub(super) horizon_epsilon: f32,
}

/// What [`resample_geometry`] renders.
#[derive(Clone, Copy)]
pub(crate) struct GeometryResampleJob<'a> {
    /// From output-raster pixel to source pixel.
    pub(crate) map: GeometryInverseMap<'a>,
    /// Display-linear interleaved RGB rows of the source, `map.source_size()` pixels.
    pub(crate) source: &'a [f32],
    /// The part of the output raster to render: left, top, width, height.
    pub(crate) window: [u32; 4],
}

/// Resamples `job` on the GPU and hands `row` each row of the window, top to bottom, as
/// interleaved display-linear RGB. Readbacks wait as `wait` allows.
pub(crate) fn resample_geometry(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    job: GeometryResampleJob<'_>,
    mut wait: GpuWait<'_>,
    mut row: impl FnMut(Vec<f32>) -> Result<()>,
) -> Result<()> {
    let resampler = Resampler::new(device, job)?;
    let cells = resampler.cell_footprints(device, queue, wait.reborrow())?;
    let mut pending: Option<PendingBand> = None;
    for band in 0..cells.rows {
        let submitted = resampler.render_band(device, queue, &cells, band)?;
        if let Some(previous) = pending.replace(submitted) {
            resampler.finish_band(device, previous, wait.reborrow(), &mut row)?;
        }
    }
    if let Some(last) = pending {
        resampler.finish_band(device, last, wait, &mut row)?;
    }
    Ok(())
}

/// An inclusive rectangle of source pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceRect {
    min: [u32; 2],
    max: [u32; 2],
}

impl SourceRect {
    fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    fn width(self) -> u32 {
        self.max[0] - self.min[0] + 1
    }

    fn bytes(self) -> u64 {
        u64::from(self.width()) * u64::from(self.max[1] - self.min[1] + 1) * RGB_BYTES
    }
}

/// The source rectangle each footprint cell reads, row by row; `None` for a cell whose
/// pixels all lie outside the source.
struct CellFootprints {
    columns: u32,
    rows: u32,
    cells: Vec<Option<SourceRect>>,
}

/// Window columns of a band accumulated from one source rectangle.
#[derive(Clone, Copy, Debug)]
struct Tile {
    first_column: u32,
    width: u32,
    source: SourceRect,
}

/// Groups a band's cells, left to right, into tiles whose source rectangle stays within
/// [`CHUNK_BYTES`]; a cell larger on its own is a tile of its own. Cells that read nothing
/// join no tile, and their pixels stay black.
fn band_tiles(cells: &[Option<SourceRect>], window_width: u32) -> Vec<Tile> {
    let mut tiles = Vec::new();
    let mut current: Option<(u32, u32, SourceRect)> = None;
    let mut close = |(first, last, source): (u32, u32, SourceRect)| {
        let first_column = first * CELL_EDGE;
        tiles.push(Tile {
            first_column,
            width: ((last + 1) * CELL_EDGE).min(window_width) - first_column,
            source,
        });
    };
    for (index, cell) in (0u32..).zip(cells) {
        let Some(cell) = *cell else {
            continue;
        };
        current = match current {
            Some((first, _, source)) if source.union(cell).bytes() <= CHUNK_BYTES => {
                Some((first, index, source.union(cell)))
            }
            Some(group) => {
                close(group);
                Some((index, index, cell))
            }
            None => Some((index, index, cell)),
        };
    }
    if let Some(group) = current {
        close(group);
    }
    tiles
}

/// A band whose rows are on their way to a readback buffer.
struct PendingBand {
    readback: usize,
    rows: u32,
    submission: wgpu::SubmissionIndex,
    receiver: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

struct Resampler<'a> {
    job: GeometryResampleJob<'a>,
    /// The parameters every pass shares; band, region and chunk change per dispatch.
    base: GeometryResampleParams,
    params: wgpu::Buffer,
    chunk: wgpu::Buffer,
    chunk_bytes: u64,
    cell_bounds: wgpu::Buffer,
    cell_readback: wgpu::Buffer,
    accumulator: wgpu::Buffer,
    output: wgpu::Buffer,
    /// Two, so one band can be read while the next renders.
    readbacks: [wgpu::Buffer; 2],
    bind_group: wgpu::BindGroup,
    footprint: wgpu::ComputePipeline,
    accumulate: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    _budget: GpuBudgetReservation,
}

impl<'a> Resampler<'a> {
    fn new(device: &wgpu::Device, job: GeometryResampleJob<'a>) -> Result<Self> {
        let [source_width, source_height] = job.map.source_size();
        let [_, _, width, height] = job.window;
        let [raster_width, raster_height] = job.map.output_size();
        anyhow::ensure!(
            job.window[0]
                .checked_add(width)
                .is_some_and(|right| right <= raster_width)
                && job.window[1]
                    .checked_add(height)
                    .is_some_and(|bottom| bottom <= raster_height)
                && width > 0
                && height > 0,
            "export geometry window lies outside its raster"
        );
        let source_bytes = u64::from(source_width) * u64::from(source_height) * RGB_BYTES;
        anyhow::ensure!(
            job.source.len() as u64 * 4 == source_bytes,
            "export geometry source does not match its dimensions"
        );

        let columns = width.div_ceil(CELL_EDGE);
        let rows = height.div_ceil(CELL_EDGE);
        let cell_bytes = u64::from(columns) * u64::from(rows) * 16;
        let band_pixels = u64::from(width) * u64::from(CELL_EDGE.min(height));
        let chunk_bytes = CHUNK_BYTES.min(source_bytes);
        let (lens_map_size, lens_grid_size, lens_grid) = match job.map.lens_geometry() {
            Some(lens) => {
                let (grid_size, grid) = lens.grid();
                ([lens.width(), lens.height()], grid_size, grid)
            }
            // The grid binding needs one element.
            None => ([0; 2], [0; 2], &[[0.0; 2]][..]),
        };
        let lens_bytes = std::mem::size_of_val(lens_grid) as u64;
        let budget = GpuBudgetReservation::acquire_bytes(
            "export geometry resampler",
            chunk_bytes
                + cell_bytes * 2
                + band_pixels * (ACCUMULATOR_BYTES + RGB_BYTES * 3)
                + lens_bytes,
            gpu_working_set_limit_bytes(),
        )?;

        let scopes = GpuErrorScopes::push(device);
        let storage = |label, size| {
            create_gpu_buffer(
                device,
                label,
                size,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            )
        };
        let readback = |label, size| {
            create_gpu_buffer(
                device,
                label,
                size,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            )
        };
        let params = create_gpu_buffer(
            device,
            "calibraw export geometry parameters",
            std::mem::size_of::<GeometryResampleParams>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let lens = create_initialized_buffer(
            device,
            "calibraw export geometry lens grid",
            bytemuck::cast_slice(lens_grid),
            wgpu::BufferUsages::STORAGE,
        );
        let chunk = storage("calibraw export geometry source chunk", chunk_bytes);
        let cell_bounds = storage("calibraw export geometry cell bounds", cell_bytes);
        let cell_readback = readback("calibraw export geometry cell bounds readback", cell_bytes);
        let accumulator = storage(
            "calibraw export geometry accumulator",
            band_pixels * ACCUMULATOR_BYTES,
        );
        let output = storage("calibraw export geometry band", band_pixels * RGB_BYTES);
        let readbacks = [
            readback(
                "calibraw export geometry band readback",
                band_pixels * RGB_BYTES,
            ),
            readback(
                "calibraw export geometry band readback",
                band_pixels * RGB_BYTES,
            ),
        ];

        let layout = create_bind_group_layout(
            device,
            "calibraw export geometry layout",
            &[
                buffer_entry(0),
                storage_buffer_entry(1, true),
                storage_buffer_entry(2, true),
                storage_buffer_entry(3, false),
                storage_buffer_entry(4, false),
                storage_buffer_entry(5, false),
            ],
        );
        let bind_group = create_bind_group(
            device,
            "calibraw export geometry bind group",
            &layout,
            &[
                buffer_binding(0, &params),
                buffer_binding(1, &lens),
                buffer_binding(2, &chunk),
                buffer_binding(3, &cell_bounds),
                buffer_binding(4, &accumulator),
                buffer_binding(5, &output),
            ],
        );
        let shader = shaders::GEOMETRY_RESAMPLE_ENTRY;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader.label),
            source: wgpu::ShaderSource::Wgsl(shader.source.text.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("calibraw export geometry pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            create_compute_pipeline(
                device,
                entry_point,
                &pipeline_layout,
                &module,
                entry_point,
                None,
            )
        };
        let footprint = pipeline("geometry_footprint");
        let accumulate = pipeline("geometry_accumulate");
        let resolve = pipeline("geometry_resolve");
        scopes.finish("create the export geometry resampler")?;

        let mut flags = 0;
        if job.map.is_affine() {
            flags |= GEOMETRY_AFFINE;
        }
        if job.map.lens_geometry().is_some() {
            flags |= GEOMETRY_LENS;
        }
        let base = GeometryResampleParams {
            source_from_output: job
                .map
                .source_from_output()
                .rows()
                .map(|[x, y, w]| [x, y, w, 0.0]),
            source_size: [source_width, source_height],
            raster_size: [raster_width, raster_height],
            window: job.window,
            lens_map_size,
            lens_grid_size,
            band: [0; 2],
            region: [0; 2],
            chunk: [0; 4],
            cell_edge: CELL_EDGE,
            cells_per_row: columns,
            flags,
            horizon_epsilon: Homography::HORIZON_EPSILON as f32,
        };
        Ok(Self {
            job,
            base,
            params,
            chunk,
            chunk_bytes,
            cell_bounds,
            cell_readback,
            accumulator,
            output,
            readbacks,
            bind_group,
            footprint,
            accumulate,
            resolve,
            _budget: budget,
        })
    }

    /// Writes the parameters of the next submission.
    fn write_params(&self, queue: &wgpu::Queue, band: [u32; 2], region: [u32; 2], chunk: [u32; 4]) {
        let params = GeometryResampleParams {
            band,
            region,
            chunk,
            ..self.base
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
    }

    fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        size: [u32; 2],
    ) {
        dispatch_compute(
            encoder,
            "calibraw export geometry",
            pipeline,
            &[&self.bind_group],
            [
                size[0].div_ceil(GEOMETRY_WORKGROUP_EDGE),
                size[1].div_ceil(GEOMETRY_WORKGROUP_EDGE),
                1,
            ],
        );
    }

    /// Runs the footprint pass over the window and reads back what each cell reads, widened
    /// by a pixel: the passes compile separately, and their positions may round apart.
    fn cell_footprints(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        wait: GpuWait<'_>,
    ) -> Result<CellFootprints> {
        let [_, _, width, height] = self.job.window;
        self.write_params(queue, [0, height], [0, width], [0; 4]);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export geometry footprints"),
        });
        encoder.clear_buffer(&self.cell_bounds, 0, None);
        self.dispatch(&mut encoder, &self.footprint, [width, height]);
        encoder.copy_buffer_to_buffer(&self.cell_bounds, 0, &self.cell_readback, 0, None);
        let submission = queue.submit(Some(encoder.finish()));
        let receiver = map_read_async(&self.cell_readback);
        wait_for_mapping(
            device,
            submission,
            receiver,
            "export geometry footprint",
            "export geometry footprint",
            wait,
        )?;
        let [source_width, source_height] = self.job.map.source_size();
        let cells = {
            let mapped = self.cell_readback.get_mapped_range(..);
            let words: &[u32] = bytemuck::try_cast_slice(&mapped)
                .map_err(|error| anyhow!("read export geometry footprints: {error}"))?;
            words
                .chunks_exact(4)
                .map(|slot| {
                    (slot[2] != 0).then(|| SourceRect {
                        min: [(!slot[0]).saturating_sub(1), (!slot[1]).saturating_sub(1)],
                        max: [
                            slot[2].min(source_width - 1),
                            slot[3].min(source_height - 1),
                        ],
                    })
                })
                .collect()
        };
        self.cell_readback.unmap();
        Ok(CellFootprints {
            columns: self.base.cells_per_row,
            rows: height.div_ceil(CELL_EDGE),
            cells,
        })
    }

    /// Accumulates and resolves band `band` and starts reading it back.
    fn render_band(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cells: &CellFootprints,
        band: u32,
    ) -> Result<PendingBand> {
        let [_, _, width, height] = self.job.window;
        let first_row = band * CELL_EDGE;
        let rows = CELL_EDGE.min(height - first_row);
        let band_rows = [first_row, rows];
        let band_bytes = u64::from(width) * u64::from(rows);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export geometry band clear"),
        });
        encoder.clear_buffer(&self.accumulator, 0, Some(band_bytes * ACCUMULATOR_BYTES));
        queue.submit(Some(encoder.finish()));

        let first_cell = (band * cells.columns) as usize;
        let band_cells = &cells.cells[first_cell..first_cell + cells.columns as usize];
        for tile in band_tiles(band_cells, width) {
            let source = tile.source;
            let row_bytes = u64::from(source.width()) * RGB_BYTES;
            let chunk_rows = u32::try_from(self.chunk_bytes / row_bytes)
                .unwrap_or(u32::MAX)
                .max(1);
            let mut chunk_top = source.min[1];
            while chunk_top <= source.max[1] {
                let chunk_height = chunk_rows.min(source.max[1] - chunk_top + 1);
                let chunk = [source.min[0], chunk_top, source.width(), chunk_height];
                self.upload_chunk(queue, chunk)?;
                self.write_params(queue, band_rows, [tile.first_column, tile.width], chunk);
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("calibraw export geometry accumulate"),
                });
                self.dispatch(&mut encoder, &self.accumulate, [tile.width, rows]);
                queue.submit(Some(encoder.finish()));
                chunk_top += chunk_height;
            }
        }

        self.write_params(queue, band_rows, [0, width], [0; 4]);
        let readback = band as usize % self.readbacks.len();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("calibraw export geometry resolve"),
        });
        self.dispatch(&mut encoder, &self.resolve, [width, rows]);
        encoder.copy_buffer_to_buffer(
            &self.output,
            0,
            &self.readbacks[readback],
            0,
            band_bytes * RGB_BYTES,
        );
        let submission = queue.submit(Some(encoder.finish()));
        Ok(PendingBand {
            readback,
            rows,
            submission,
            receiver: map_read_async(&self.readbacks[readback]),
        })
    }

    /// Uploads the source pixels `[x, y, width, height]` to the chunk buffer, row by row.
    fn upload_chunk(&self, queue: &wgpu::Queue, [x, y, width, height]: [u32; 4]) -> Result<()> {
        let [source_width, _] = self.job.map.source_size();
        let row_values = width as usize * 3;
        let row_bytes = row_values * 4;
        let size = wgpu::BufferSize::new(row_bytes as u64 * u64::from(height))
            .context("export geometry chunk is empty")?;
        let mut view = queue
            .write_buffer_with(&self.chunk, 0, size)
            .context("stage the export geometry source chunk")?;
        for (index, source_y) in (y..y + height).enumerate() {
            let start = (source_y as usize * source_width as usize + x as usize) * 3;
            view.slice(index * row_bytes..(index + 1) * row_bytes)
                .copy_from_slice(bytemuck::cast_slice(
                    &self.job.source[start..start + row_values],
                ));
        }
        Ok(())
    }

    /// Waits for `band`'s readback and hands its rows to `row`.
    fn finish_band(
        &self,
        device: &wgpu::Device,
        band: PendingBand,
        wait: GpuWait<'_>,
        row: &mut impl FnMut(Vec<f32>) -> Result<()>,
    ) -> Result<()> {
        wait_for_mapping(
            device,
            band.submission,
            band.receiver,
            "export geometry band",
            "export geometry band",
            wait,
        )?;
        let readback = &self.readbacks[band.readback];
        let row_values = self.job.window[2] as usize * 3;
        let rows: Vec<Vec<f32>> = {
            let mapped = readback.get_mapped_range(..);
            let values: &[f32] = bytemuck::try_cast_slice(&mapped)
                .map_err(|error| anyhow!("read export geometry band: {error}"))?;
            values[..row_values * band.rows as usize]
                .chunks_exact(row_values)
                .map(<[f32]>::to_vec)
                .collect()
        };
        readback.unmap();
        rows.into_iter().try_for_each(row)
    }
}
