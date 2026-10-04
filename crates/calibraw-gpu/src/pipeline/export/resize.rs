//! Linear-light Lanczos resizing and final-size output sharpening.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct SampleWeight {
    pub(super) index: u32,
    pub(super) weight: f32,
}

#[derive(Clone, Copy, Debug)]
struct OutputSampleWeight {
    output_index: u32,
    pub(super) weight: f32,
}

pub(super) struct FinalSizeOutputSharpen {
    pub(super) width: u32,
    pub(super) strength: f32,
    previous: Option<Arc<Vec<f32>>>,
    pub(super) current: Option<Arc<Vec<f32>>>,
    pub(super) pending: Vec<[Arc<Vec<f32>>; 3]>,
    pub(super) encoded_rows: u32,
    passthrough: bool,
}

impl FinalSizeOutputSharpen {
    pub(super) fn new(
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        let scale_x = source_width as f32 / output_width.max(1) as f32;
        let scale_y = source_height as f32 / output_height.max(1) as f32;
        let downsample = scale_x.max(scale_y).max(1.0);
        let upscale = (output_width as f32 / source_width.max(1) as f32)
            .max(output_height as f32 / source_height.max(1) as f32)
            .max(1.0);
        let downsample_boost = (downsample.log2() / 3.0).clamp(0.0, 1.0);
        let upscale_reduction = ((upscale - 1.0) / 2.0).clamp(0.0, 1.0);
        let strength = (0.44 + 0.22 * downsample_boost) * (1.0 - 0.28 * upscale_reduction);
        Self {
            width: output_width,
            strength,
            previous: None,
            current: None,
            pending: Vec::new(),
            encoded_rows: 0,
            passthrough: false,
        }
    }

    pub(super) fn with_passthrough(mut self, passthrough: bool) -> Self {
        self.passthrough = passthrough;
        self
    }

    pub(super) fn push_row<W: Write>(
        &mut self,
        row: Vec<f32>,
        output_transform: Option<&SrgbOutputTransform>,
        row_format: ExportRowFormat,
        output: &mut W,
    ) -> Result<()> {
        anyhow::ensure!(
            row.len() == checked_rgb_len(self.width, 1)?,
            "final-size sharpen row length does not match output width"
        );
        let row = Arc::new(row);
        if self.passthrough {
            self.pending.push([Arc::clone(&row), Arc::clone(&row), row]);
            return self.flush_full_batch(output_transform, row_format, output);
        }
        let Some(current) = self.current.take() else {
            self.current = Some(row);
            return Ok(());
        };
        let top = self.previous.as_ref().unwrap_or(&current);
        self.pending
            .push([Arc::clone(top), Arc::clone(&current), Arc::clone(&row)]);
        self.previous = Some(current);
        self.current = Some(row);
        self.flush_full_batch(output_transform, row_format, output)
    }

    pub(super) fn finish<W: Write>(
        &mut self,
        output_transform: Option<&SrgbOutputTransform>,
        row_format: ExportRowFormat,
        output: &mut W,
    ) -> Result<()> {
        if self.passthrough {
            return self.flush_batch(output_transform, row_format, output);
        }
        if let Some(current) = self.current.take() {
            let top = self.previous.as_ref().unwrap_or(&current);
            self.pending
                .push([Arc::clone(top), Arc::clone(&current), current]);
        }
        self.previous = None;
        self.flush_batch(output_transform, row_format, output)
    }

    fn flush_full_batch<W: Write>(
        &mut self,
        output_transform: Option<&SrgbOutputTransform>,
        row_format: ExportRowFormat,
        output: &mut W,
    ) -> Result<()> {
        if self.pending.len() >= EXPORT_CPU_ROW_BATCH {
            self.flush_batch(output_transform, row_format, output)?;
        }
        Ok(())
    }

    fn flush_batch<W: Write>(
        &mut self,
        output_transform: Option<&SrgbOutputTransform>,
        row_format: ExportRowFormat,
        output: &mut W,
    ) -> Result<()> {
        // Only a small band is retained. Rayon preserves indexed row order;
        // compression and writes stay sequential, with identical pixel math.
        let encoded: Result<Vec<Vec<u8>>> = self
            .pending
            .par_iter()
            .map(|[top, center, bottom]| {
                if self.passthrough {
                    encode_output_row(center, output_transform, row_format)
                } else {
                    let sharpened = output_sharpen_linear_row(top, center, bottom, self.strength)?;
                    encode_output_row(&sharpened, output_transform, row_format)
                }
            })
            .collect();
        for row in encoded? {
            output
                .write_all(&row)
                .with_context(|| format!("write output row {}", self.encoded_rows))?;
            self.encoded_rows += 1;
        }
        self.pending.clear();
        Ok(())
    }
}

fn rec2020_luminance(pixel: &[f32]) -> f32 {
    (pixel[0] * 0.2627 + pixel[1] * 0.6780 + pixel[2] * 0.0593).max(1e-8)
}

pub(super) fn output_sharpen_linear_row(
    top: &[f32],
    center: &[f32],
    bottom: &[f32],
    strength: f32,
) -> Result<Vec<f32>> {
    anyhow::ensure!(
        top.len() == center.len() && center.len() == bottom.len() && center.len().is_multiple_of(3),
        "output sharpen rows have incompatible lengths"
    );
    let pixels = center.len() / 3;
    let mut sharpened = Vec::new();
    sharpened
        .try_reserve_exact(center.len())
        .context("reserve final-size sharpen row")?;
    sharpened.resize(center.len(), 0.0);

    for x in 0..pixels {
        let x_left = x.saturating_sub(1);
        let x_right = (x + 1).min(pixels.saturating_sub(1));
        let center_start = x * 3;
        let left_start = x_left * 3;
        let right_start = x_right * 3;
        let c = rec2020_luminance(&center[center_start..center_start + 3]);
        let l = rec2020_luminance(&center[left_start..left_start + 3]);
        let r = rec2020_luminance(&center[right_start..right_start + 3]);
        let u = rec2020_luminance(&top[center_start..center_start + 3]);
        let d = rec2020_luminance(&bottom[center_start..center_start + 3]);
        let center_ev = c.log2();

        let mut weighted = c * 4.0;
        let mut weight_sum = 4.0;
        for neighbour in [l, r, u, d] {
            let delta = neighbour.log2() - center_ev;
            let weight = (-4.2 * delta * delta).exp();
            weighted += neighbour * weight;
            weight_sum += weight;
        }
        let base = (weighted / weight_sum.max(1e-6)).max(1e-8);
        let detail_ev = center_ev - base.log2();

        let shadow = (1.0 - ((center_ev + 7.5) / 4.5).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let threshold = 0.0065 + 0.012 * shadow;
        let thresholded = detail_ev.signum() * (detail_ev.abs() - threshold).max(0.0);
        let edge = [l, r, u, d]
            .into_iter()
            .map(|value| (value.log2() - center_ev).abs())
            .fold(0.0f32, f32::max);
        let edge_select = (0.30 + 0.70 * ((edge - 0.008) / 0.16).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let delta_ev = (thresholded * strength * edge_select).clamp(-0.16, 0.18);
        let proposed = c * 2.0f32.powf(delta_ev);

        let local_min = c.min(l).min(r).min(u).min(d);
        let local_max = c.max(l).max(r).max(u).max(d);
        let target_luma = proposed.clamp(local_min * 0.985, local_max * 1.015);
        let gain = (target_luma / c).clamp(0.78, 1.28);
        for channel in 0..3 {
            sharpened[center_start + channel] = (center[center_start + channel] * gain).max(0.0);
        }
    }
    Ok(sharpened)
}

pub(super) struct LinearLightResizer {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    horizontal: Vec<Vec<SampleWeight>>,
    vertical_by_source: Vec<Vec<OutputSampleWeight>>,
    output_last_source: Vec<u32>,
    pub(super) pending_rows: Vec<Option<Vec<f32>>>,
    next_source_row: u32,
    next_output_row: u32,
    row_format: ExportRowFormat,
    output_sharpen: FinalSizeOutputSharpen,
}

impl LinearLightResizer {
    #[cfg(test)]
    pub(super) fn new(
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    ) -> Result<Self> {
        Self::new_with_format(
            source_width,
            source_height,
            output_width,
            output_height,
            ExportRowFormat::Rgba8,
        )
    }

    pub(super) fn new_with_format(
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
        row_format: ExportRowFormat,
    ) -> Result<Self> {
        validate_export_dimensions(output_width, output_height)?;
        anyhow::ensure!(
            source_width > 0 && source_height > 0,
            "source image is empty"
        );
        let vertical = build_lanczos_contributions(source_height, output_height)?;
        let (vertical_by_source, output_last_source) =
            invert_vertical_contributions(source_height, &vertical)?;
        let mut pending_rows = Vec::new();
        pending_rows
            .try_reserve_exact(output_height as usize)
            .context("reserve vertical resize row slots")?;
        pending_rows.resize_with(output_height as usize, || None);
        Ok(Self {
            source_width,
            source_height,
            output_width,
            output_height,
            horizontal: build_lanczos_contributions(source_width, output_width)?,
            vertical_by_source,
            output_last_source,
            pending_rows,
            next_source_row: 0,
            next_output_row: 0,
            row_format,
            output_sharpen: FinalSizeOutputSharpen::new(
                source_width,
                source_height,
                output_width,
                output_height,
            )
            .with_passthrough(row_format == ExportRowFormat::RgbF32Le),
        })
    }

    pub(super) fn push_source_row<W: Write>(
        &mut self,
        source_y: u32,
        source: &[f32],
        output_transform: Option<&SrgbOutputTransform>,
        output: &mut W,
    ) -> Result<()> {
        anyhow::ensure!(
            source_y == self.next_source_row,
            "source rows arrived out of order: got {source_y}, expected {}",
            self.next_source_row
        );
        anyhow::ensure!(
            source.len() == checked_rgb_len(self.source_width, 1)?,
            "source row length does not match export width"
        );
        let horizontal = resize_horizontal_row(source, &self.horizontal)?;
        let contribution_count = self
            .vertical_by_source
            .get(source_y as usize)
            .context("source resize row is outside the contribution table")?
            .len();
        for contribution_index in 0..contribution_count {
            let contribution = self.vertical_by_source[source_y as usize][contribution_index];
            {
                let output_index = contribution.output_index as usize;
                let slot = self
                    .pending_rows
                    .get_mut(output_index)
                    .context("output resize row is outside the pending table")?;
                if slot.is_none() {
                    let row_values = checked_rgb_len(self.output_width, 1)?;
                    let mut row = Vec::new();
                    row.try_reserve_exact(row_values)
                        .context("reserve active vertical resize row")?;
                    row.resize(row_values, 0.0f32);
                    *slot = Some(row);
                }
                let row = slot
                    .as_mut()
                    .context("pending output row failed to initialize")?;
                for (destination, value) in row.iter_mut().zip(&horizontal) {
                    *destination += *value * contribution.weight;
                }
            }
            self.write_ready_rows_through(
                source_y,
                contribution.output_index,
                output_transform,
                output,
            )?;
        }
        self.next_source_row += 1;
        Ok(())
    }

    pub(super) fn finish<W: Write>(
        &mut self,
        output_transform: Option<&SrgbOutputTransform>,
        output: &mut W,
    ) -> Result<()> {
        anyhow::ensure!(
            self.next_source_row == self.source_height,
            "linear resizer received {} of {} source rows",
            self.next_source_row,
            self.source_height
        );
        self.write_ready_rows_through(
            self.source_height - 1,
            self.output_height - 1,
            output_transform,
            output,
        )?;
        self.output_sharpen
            .finish(output_transform, self.row_format, output)?;
        anyhow::ensure!(
            self.next_output_row == self.output_height,
            "linear resizer produced {} of {} output rows",
            self.next_output_row,
            self.output_height
        );
        anyhow::ensure!(
            self.output_sharpen.encoded_rows == self.output_height,
            "output sharpen produced {} of {} rows",
            self.output_sharpen.encoded_rows,
            self.output_height
        );
        anyhow::ensure!(
            self.pending_rows.iter().all(Option::is_none),
            "linear resizer retained incomplete output rows"
        );
        Ok(())
    }

    fn write_ready_rows_through<W: Write>(
        &mut self,
        source_y: u32,
        completed_through_output: u32,
        output_transform: Option<&SrgbOutputTransform>,
        output: &mut W,
    ) -> Result<()> {
        while self.next_output_row < self.output_height
            && self.next_output_row <= completed_through_output
            && self.output_last_source[self.next_output_row as usize] <= source_y
        {
            let row = self.pending_rows[self.next_output_row as usize]
                .take()
                .context("completed resize row has no accumulated pixels")?;
            self.output_sharpen
                .push_row(row, output_transform, self.row_format, output)?;
            self.next_output_row += 1;
        }
        Ok(())
    }
}

fn invert_vertical_contributions(
    source_height: u32,
    vertical: &[Vec<SampleWeight>],
) -> Result<(Vec<Vec<OutputSampleWeight>>, Vec<u32>)> {
    let source_rows = source_height as usize;
    let mut counts = Vec::new();
    counts
        .try_reserve_exact(source_rows)
        .context("reserve vertical contribution counts")?;
    counts.resize(source_rows, 0usize);
    let mut output_last_source = Vec::new();
    output_last_source
        .try_reserve_exact(vertical.len())
        .context("reserve output resize boundaries")?;
    for samples in vertical {
        let last = samples
            .last()
            .map(|sample| sample.index)
            .context("vertical resampling kernel is empty")?;
        output_last_source.push(last);
        for sample in samples {
            let count = counts
                .get_mut(sample.index as usize)
                .context("vertical contribution references an invalid source row")?;
            *count = count
                .checked_add(1)
                .context("vertical contribution count overflow")?;
        }
    }

    let mut by_source = Vec::new();
    by_source
        .try_reserve_exact(source_rows)
        .context("reserve vertical contribution rows")?;
    for count in counts {
        let mut row = Vec::new();
        row.try_reserve_exact(count)
            .context("reserve vertical source contributions")?;
        by_source.push(row);
    }
    for (output_index, samples) in vertical.iter().enumerate() {
        let output_index =
            u32::try_from(output_index).context("output resize index does not fit in u32")?;
        for sample in samples {
            by_source[sample.index as usize].push(OutputSampleWeight {
                output_index,
                weight: sample.weight,
            });
        }
    }
    Ok((by_source, output_last_source))
}

pub(super) fn build_lanczos_contributions(
    source: u32,
    output: u32,
) -> Result<Vec<Vec<SampleWeight>>> {
    anyhow::ensure!(
        source > 0 && output > 0,
        "resize dimensions must be non-zero"
    );
    let mut all = Vec::new();
    all.try_reserve_exact(output as usize)
        .context("reserve resize contribution table")?;
    if source == output {
        for index in 0..output {
            all.push(vec![SampleWeight { index, weight: 1.0 }]);
        }
        return Ok(all);
    }

    let scale = source as f64 / output as f64;
    let filter_scale = scale.max(1.0);
    let support = 3.0 * filter_scale;
    for destination in 0..output {
        let center = (destination as f64 + 0.5) * scale - 0.5;
        let first = ((center - support).floor() as i64 + 1).max(0);
        let last = ((center + support).ceil() as i64 - 1).min(i64::from(source) - 1);
        anyhow::ensure!(first <= last, "resize kernel contains no source samples");
        let capacity = usize::try_from(last - first + 1)
            .context("resize kernel is too large for this platform")?;
        let mut samples = Vec::<SampleWeight>::new();
        samples
            .try_reserve_exact(capacity)
            .context("reserve resize kernel")?;
        for source_index in first..=last {
            let distance = (center - source_index as f64) / filter_scale;
            let weight = lanczos3(distance) / filter_scale;
            if weight.abs() <= 1e-15 {
                continue;
            }
            samples.push(SampleWeight {
                index: source_index as u32,
                weight: weight as f32,
            });
        }
        let sum: f32 = samples.iter().map(|sample| sample.weight).sum();
        anyhow::ensure!(
            sum.is_finite() && sum.abs() > 1e-12,
            "invalid resize kernel"
        );
        for sample in &mut samples {
            sample.weight /= sum;
        }
        all.push(samples);
    }
    Ok(all)
}

fn lanczos3(value: f64) -> f64 {
    let value = value.abs();
    if value < 1e-12 {
        return 1.0;
    }
    if value >= 3.0 {
        return 0.0;
    }
    let pi_value = std::f64::consts::PI * value;
    (pi_value.sin() / pi_value) * ((pi_value / 3.0).sin() / (pi_value / 3.0))
}

fn resize_horizontal_row(source: &[f32], weights: &[Vec<SampleWeight>]) -> Result<Vec<f32>> {
    let values = weights
        .len()
        .checked_mul(3)
        .context("resize row overflow")?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(values)
        .context("reserve horizontal resize row")?;
    output.resize(values, 0.0f32);
    for (destination, samples) in weights.iter().enumerate() {
        for sample in samples {
            let source_start = usize::try_from(sample.index)
                .ok()
                .and_then(|index| index.checked_mul(3))
                .context("source resize index overflow")?;
            let destination_start = destination
                .checked_mul(3)
                .context("destination resize index overflow")?;
            for channel in 0..3 {
                output[destination_start + channel] +=
                    source[source_start + channel] * sample.weight;
            }
        }
    }
    Ok(output)
}
