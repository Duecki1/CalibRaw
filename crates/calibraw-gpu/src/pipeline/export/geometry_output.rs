//! Output through a geometry transform: inverse-mapped, anisotropically filtered sampling.

use super::*;

pub(super) fn render_geometry_output<W: Write>(
    context: ExportContext<'_>,
    request: ExportRequest<'_>,
    output: &mut W,
    row_format: ExportRowFormat,
) -> Result<()> {
    with_staging_file(request.output, |staged_linear| {
        {
            let linear_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staged_linear)
                .with_context(|| {
                    format!("create geometry linear raster {}", staged_linear.display())
                })?;
            let mut linear_writer = BufWriter::new(linear_file);
            stream_tiled_linear_rows(context, request, |_source_y, row| {
                linear_writer
                    .write_all(bytemuck::cast_slice(row))
                    .context("write geometry linear source row")
            })?;
            linear_writer
                .flush()
                .context("flush geometry linear source raster")?;
        }

        let linear_file = fs::File::open(staged_linear)
            .with_context(|| format!("open geometry linear raster {}", staged_linear.display()))?;
        let mapped = unsafe { memmap2::MmapOptions::new().map(&linear_file) }
            .with_context(|| format!("map geometry linear raster {}", staged_linear.display()))?;
        let source = validate_linear_rgb_raster(&mapped, request.raw.width, request.raw.height)?;
        let resampler = GeometryResampler::new_with_lens(
            source,
            request.raw.width,
            request.raw.height,
            request.geometry,
            request.raw.lens_geometry.as_deref(),
            request.output_width,
            request.output_height,
        )?;
        let (geometry_width, geometry_height) = request
            .geometry
            .crop_pixel_dimensions(request.raw.width, request.raw.height);
        let mut output_sharpen = FinalSizeOutputSharpen::new(
            geometry_width,
            geometry_height,
            request.output_width,
            request.output_height,
        )
        .with_passthrough(row_format == ExportRowFormat::RgbF32Le);
        let output_transform = request.color.transform.as_ref();
        let finalize_started = Instant::now();
        for first_y in (0..request.output_height).step_by(EXPORT_CPU_ROW_BATCH) {
            let end_y = first_y
                .saturating_add(EXPORT_CPU_ROW_BATCH as u32)
                .min(request.output_height);
            let rows = resampler.output_rows(first_y..end_y, context.cancellation)?;
            for row in rows {
                ensure_export_not_cancelled(context.cancellation)?;
                output_sharpen.push_row(row, output_transform, row_format, output)?;
            }
        }
        output_sharpen.finish(output_transform, row_format, output)?;
        calibraw_core::diagnostics::record(format!(
            "Export geometry, final sharpening and output encoding finished in {:.3}s: {}x{}",
            finalize_started.elapsed().as_secs_f64(),
            request.output_width,
            request.output_height,
        ));
        Ok(())
    })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct GeometryFilterAxes {
    major: [f32; 2],
    minor: [f32; 2],
    major_scale: f32,
    minor_scale: f32,
    radius_x: f32,
    radius_y: f32,
}

pub(super) struct GeometryResampler<'a> {
    pub(super) source: &'a [f32],
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    inverse_map: GeometryInverseMap<'a>,
    affine_filter: Option<GeometryFilterAxes>,
}

impl<'a> GeometryResampler<'a> {
    #[cfg(test)]
    pub(super) fn new(
        source: &'a [f32],
        source_width: u32,
        source_height: u32,
        geometry: GeometryTransform,
        output_width: u32,
        output_height: u32,
    ) -> Result<Self> {
        Self::new_with_lens(
            source,
            source_width,
            source_height,
            geometry,
            None,
            output_width,
            output_height,
        )
    }

    pub(super) fn new_with_lens(
        source: &'a [f32],
        source_width: u32,
        source_height: u32,
        geometry: GeometryTransform,
        lens_geometry: Option<&'a LensGeometryMap>,
        output_width: u32,
        output_height: u32,
    ) -> Result<Self> {
        validate_export_dimensions(output_width, output_height)?;
        anyhow::ensure!(
            source_width > 0 && source_height > 0,
            "source image is empty"
        );
        anyhow::ensure!(
            source.len() == checked_rgb_len(source_width, source_height)?,
            "linear geometry raster length does not match its dimensions"
        );
        let inverse_map = GeometryInverseMap::new_with_lens(
            geometry,
            lens_geometry,
            source_width,
            source_height,
            output_width,
            output_height,
        );
        let affine_filter = lens_geometry
            .is_none()
            .then(|| geometry_filter_axes(inverse_map.pixel_jacobian()));
        Ok(Self {
            source,
            source_width,
            source_height,
            output_width,
            output_height,
            inverse_map,
            affine_filter,
        })
    }

    pub(super) fn output_rows(
        &self,
        rows: std::ops::Range<u32>,
        cancellation: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>> {
        // Lens correction, rotation and crop run after the last rendered tile.
        // Process a bounded band across CPU cores without changing sampling order
        // within a pixel or the row order seen by sharpening and the encoder.
        rows.into_par_iter()
            .map(|y| {
                ensure_export_not_cancelled(cancellation)?;
                self.output_row(y)
            })
            .collect()
    }

    pub(super) fn output_row(&self, output_y: u32) -> Result<Vec<f32>> {
        anyhow::ensure!(
            output_y < self.output_height,
            "geometry row is outside the output image"
        );
        let values = checked_rgb_len(self.output_width, 1)?;
        let mut row = Vec::new();
        row.try_reserve_exact(values)
            .context("reserve transformed linear output row")?;
        row.resize(values, 0.0);
        for output_x in 0..self.output_width {
            let [source_x, source_y] = self
                .inverse_map
                .source_position(output_x as f32, output_y as f32);
            let filter = self.affine_filter.unwrap_or_else(|| {
                geometry_filter_axes(
                    self.inverse_map
                        .pixel_jacobian_at(output_x as f32, output_y as f32),
                )
            });
            let rgb = self.sample(source_x, source_y, filter);
            let start = output_x as usize * 3;
            row[start..start + 3].copy_from_slice(&rgb);
        }
        Ok(row)
    }

    pub(super) fn sample(&self, x: f32, y: f32, filter: GeometryFilterAxes) -> [f32; 3] {
        if !x.is_finite()
            || !y.is_finite()
            || x < -0.5
            || y < -0.5
            || x > self.source_width as f32 - 0.5
            || y > self.source_height as f32 - 0.5
        {
            return [0.0; 3];
        }

        if filter.major_scale <= 1.0 + 1e-6 && filter.minor_scale <= 1.0 + 1e-6 {
            let nearest_x = x.round();
            let nearest_y = y.round();
            if (x - nearest_x).abs() <= 1e-6 && (y - nearest_y).abs() <= 1e-6 {
                let source_x = nearest_x as u32;
                let source_y = nearest_y as u32;
                let index =
                    (source_y as usize * self.source_width as usize + source_x as usize) * 3;
                return [
                    self.source[index],
                    self.source[index + 1],
                    self.source[index + 2],
                ];
            }
        }
        let min_x = (x - filter.radius_x).floor().max(0.0) as u32;
        let max_x = (x + filter.radius_x)
            .ceil()
            .min(self.source_width.saturating_sub(1) as f32) as u32;
        let min_y = (y - filter.radius_y).floor().max(0.0) as u32;
        let max_y = (y + filter.radius_y)
            .ceil()
            .min(self.source_height.saturating_sub(1) as f32) as u32;

        let mut sum = [0.0f32; 3];
        let mut weight_sum = 0.0f32;
        for source_y in min_y..=max_y {
            for source_x in min_x..=max_x {
                let dx = source_x as f32 - x;
                let dy = source_y as f32 - y;
                let major_distance =
                    (dx * filter.major[0] + dy * filter.major[1]) / filter.major_scale;
                let minor_distance =
                    (dx * filter.minor[0] + dy * filter.minor[1]) / filter.minor_scale;
                let radius_squared =
                    major_distance * major_distance + minor_distance * minor_distance;
                if radius_squared >= 4.0 {
                    continue;
                }
                let weight = mitchell_netravali_f32(radius_squared.sqrt());
                if weight == 0.0 {
                    continue;
                }
                let index =
                    (source_y as usize * self.source_width as usize + source_x as usize) * 3;
                for (channel, value) in sum.iter_mut().enumerate() {
                    *value += self.source[index + channel] * weight;
                }
                weight_sum += weight;
            }
        }

        if weight_sum.abs() > 1e-6 {
            for value in &mut sum {
                *value /= weight_sum;
            }
            return sum;
        }

        let nearest_x = x
            .round()
            .clamp(0.0, self.source_width.saturating_sub(1) as f32) as u32;
        let nearest_y = y
            .round()
            .clamp(0.0, self.source_height.saturating_sub(1) as f32) as u32;
        let index = (nearest_y as usize * self.source_width as usize + nearest_x as usize) * 3;
        [
            self.source[index],
            self.source[index + 1],
            self.source[index + 2],
        ]
    }
}

fn geometry_filter_axes(jacobian: [[f32; 2]; 2]) -> GeometryFilterAxes {
    let jx = jacobian[0];
    let jy = jacobian[1];
    let c00 = jx[0] * jx[0] + jy[0] * jy[0];
    let c01 = jx[0] * jx[1] + jy[0] * jy[1];
    let c11 = jx[1] * jx[1] + jy[1] * jy[1];
    let trace = c00 + c11;
    let discriminant = ((c00 - c11) * (c00 - c11) + 4.0 * c01 * c01).sqrt();
    let lambda_major = ((trace + discriminant) * 0.5).max(0.0);
    let lambda_minor = ((trace - discriminant) * 0.5).max(0.0);

    let major = if discriminant <= 1e-8 {
        [1.0, 0.0]
    } else {
        let candidate_a = [c01, lambda_major - c00];
        let candidate_b = [lambda_major - c11, c01];
        let norm_a = candidate_a[0] * candidate_a[0] + candidate_a[1] * candidate_a[1];
        let norm_b = candidate_b[0] * candidate_b[0] + candidate_b[1] * candidate_b[1];
        let candidate = if norm_a >= norm_b {
            candidate_a
        } else {
            candidate_b
        };
        let length = (candidate[0] * candidate[0] + candidate[1] * candidate[1]).sqrt();
        if length > 1e-8 {
            [candidate[0] / length, candidate[1] / length]
        } else {
            [1.0, 0.0]
        }
    };
    let minor = [-major[1], major[0]];
    let major_scale = lambda_major.sqrt().max(1.0);
    let minor_scale = lambda_minor.sqrt().max(1.0);
    let radius_x = 2.0 * (major[0].abs() * major_scale + minor[0].abs() * minor_scale);
    let radius_y = 2.0 * (major[1].abs() * major_scale + minor[1].abs() * minor_scale);
    GeometryFilterAxes {
        major,
        minor,
        major_scale,
        minor_scale,
        radius_x,
        radius_y,
    }
}

fn mitchell_netravali_f32(value: f32) -> f32 {
    let value = value.abs();
    if value >= 2.0 {
        return 0.0;
    }
    const B: f32 = 1.0 / 3.0;
    const C: f32 = 1.0 / 3.0;
    let value2 = value * value;
    let value3 = value2 * value;
    if value < 1.0 {
        ((12.0 - 9.0 * B - 6.0 * C) * value3
            + (-18.0 + 12.0 * B + 6.0 * C) * value2
            + (6.0 - 2.0 * B))
            / 6.0
    } else {
        ((-B - 6.0 * C) * value3
            + (6.0 * B + 30.0 * C) * value2
            + (-12.0 * B - 48.0 * C) * value
            + (8.0 * B + 24.0 * C))
            / 6.0
    }
}
