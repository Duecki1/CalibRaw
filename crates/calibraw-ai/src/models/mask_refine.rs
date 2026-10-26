use anyhow::Result;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MaskCrop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub(super) fn mask_crop_above(
    mask: &[u8],
    width: u32,
    height: u32,
    threshold: u8,
    expand: f32,
) -> Option<MaskCrop> {
    if width == 0 || height == 0 || mask.len() != width as usize * height as usize {
        return None;
    }
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for (index, &alpha) in mask.iter().enumerate() {
        if alpha < threshold {
            continue;
        }
        let x = index as u32 % width;
        let y = index as u32 / width;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        found = true;
    }
    found.then(|| {
        expand_crop(
            MaskCrop {
                x: min_x,
                y: min_y,
                width: max_x - min_x + 1,
                height: max_y - min_y + 1,
            },
            width,
            height,
            expand,
        )
    })
}

pub(super) fn expand_crop(crop: MaskCrop, width: u32, height: u32, expand: f32) -> MaskCrop {
    let padding = (crop.width.max(crop.height) as f32 * expand.max(0.0)).round() as u32;
    let x0 = crop.x.saturating_sub(padding);
    let y0 = crop.y.saturating_sub(padding);
    let x1 = crop
        .x
        .saturating_add(crop.width)
        .saturating_add(padding)
        .min(width);
    let y1 = crop
        .y
        .saturating_add(crop.height)
        .saturating_add(padding)
        .min(height);
    MaskCrop {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

pub(super) fn merge_crop_pass(
    base: &mut [u8],
    width: u32,
    crop: MaskCrop,
    refined: &[u8],
    feather: u32,
) {
    if crop.width == 0
        || crop.height == 0
        || refined.len() != crop.width as usize * crop.height as usize
    {
        return;
    }
    for y in 0..crop.height as usize {
        for x in 0..crop.width as usize {
            let edge = x
                .min(y)
                .min(crop.width as usize - 1 - x)
                .min(crop.height as usize - 1 - y) as u32;
            let weight = if feather == 0 {
                1.0
            } else {
                (edge + 1) as f32 / feather as f32
            }
            .min(1.0);
            let source = y * crop.width as usize + x;
            let target = (crop.y as usize + y) * width as usize + crop.x as usize + x;
            base[target] = (base[target] as f32 * (1.0 - weight) + refined[source] as f32 * weight)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

/// Color-guided local linear filtering of an 8-bit mask (`guided_filter`).
pub(super) fn guided_filter_color(
    rgba: &[u8],
    alpha: &mut [u8],
    width: u32,
    height: u32,
    radius: u32,
    epsilon: f32,
) -> Result<()> {
    let mut values = alpha
        .iter()
        .map(|&value| value as f32 / 255.0)
        .collect::<Vec<_>>();
    guided_filter(rgba, &mut values, width, height, radius, epsilon)?;
    for (alpha, value) in alpha.iter_mut().zip(values) {
        *alpha = (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    Ok(())
}

/// Color-guided local linear filtering (He et al., "Guided Image Filtering"):
/// within each window of `radius` pixels, `values` are modelled as a linear
/// function of the guide, so their edges move onto the photo's edges while
/// regions of even colour are averaged. The RGB image is reduced to its
/// perceptual luminance for the linear model, which retains color edges
/// without allocating a prohibitively large three-channel covariance image for
/// full-resolution photos. `epsilon` regularizes the model against guide
/// texture, in squared luminance units. The result is not clamped.
pub(super) fn guided_filter(
    rgba: &[u8],
    values: &mut [f32],
    width: u32,
    height: u32,
    radius: u32,
    epsilon: f32,
) -> Result<()> {
    anyhow::ensure!(
        rgba.len() == width as usize * height as usize * 4,
        "guided-filter image dimensions mismatch"
    );
    guided_filter_by_luminance(
        &luminance_guide(rgba),
        values,
        width,
        height,
        radius,
        epsilon,
    )
}

/// The perceptual luminance, in [0, 1], of 8-bit RGBA pixels: the guide
/// `guided_filter` models values against.
pub(super) fn luminance_guide(rgba: &[u8]) -> Vec<f32> {
    rgba.chunks_exact(4)
        .map(|pixel| {
            (0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32) / 255.0
        })
        .collect()
}

/// `guided_filter` with a precomputed luminance `guide`, one value per pixel.
pub(super) fn guided_filter_by_luminance(
    guide: &[f32],
    values: &mut [f32],
    width: u32,
    height: u32,
    radius: u32,
    epsilon: f32,
) -> Result<()> {
    let pixels = width as usize * height as usize;
    anyhow::ensure!(
        guide.len() == pixels && values.len() == pixels,
        "guided-filter image dimensions mismatch"
    );
    if pixels == 0 {
        return Ok(());
    }
    let (width, height, radius) = (width as usize, height as usize, radius as usize);
    let mut workspace = vec![0.0; pixels];
    let mut mean_values = values.to_vec();
    box_mean_in_place(&mut mean_values, &mut workspace, width, height, radius);
    let mut mean_guide = guide.to_vec();
    box_mean_in_place(&mut mean_guide, &mut workspace, width, height, radius);
    let mut corr_guide = guide.iter().map(|value| value * value).collect::<Vec<_>>();
    box_mean_in_place(&mut corr_guide, &mut workspace, width, height, radius);
    let mut corr_guide_values = guide
        .iter()
        .zip(values.iter())
        .map(|(guide, value)| guide * value)
        .collect::<Vec<_>>();
    box_mean_in_place(
        &mut corr_guide_values,
        &mut workspace,
        width,
        height,
        radius,
    );
    for index in 0..pixels {
        let variance = (corr_guide[index] - mean_guide[index] * mean_guide[index]).max(0.0);
        let a = (corr_guide_values[index] - mean_guide[index] * mean_values[index])
            / (variance + epsilon.max(1e-8));
        corr_guide[index] = a;
        corr_guide_values[index] = mean_values[index] - a * mean_guide[index];
    }
    box_mean_in_place(&mut corr_guide, &mut workspace, width, height, radius);
    box_mean_in_place(
        &mut corr_guide_values,
        &mut workspace,
        width,
        height,
        radius,
    );
    for (index, value) in values.iter_mut().enumerate() {
        *value = corr_guide[index] * guide[index] + corr_guide_values[index];
    }
    Ok(())
}

/// The guided filter with `values` as their own guide, an edge-preserving
/// smoothing: within each window of `radius` pixels, variation well below
/// `epsilon` (a variance in squared value units) is averaged away while
/// steps well above it are kept.
pub(super) fn self_guided_filter(
    values: &mut [f32],
    width: u32,
    height: u32,
    radius: u32,
    epsilon: f32,
) -> Result<()> {
    let pixels = width as usize * height as usize;
    anyhow::ensure!(
        values.len() == pixels,
        "guided-filter image dimensions mismatch"
    );
    if pixels == 0 {
        return Ok(());
    }
    let (width, height, radius) = (width as usize, height as usize, radius as usize);
    let mut workspace = vec![0.0; pixels];
    let mut mean = values.to_vec();
    box_mean_in_place(&mut mean, &mut workspace, width, height, radius);
    let mut gain = values.iter().map(|value| value * value).collect::<Vec<_>>();
    box_mean_in_place(&mut gain, &mut workspace, width, height, radius);
    // Per window: the gain a = variance / (variance + epsilon) in `gain`,
    // the offset (1 − a) × mean in `mean`.
    for (gain, mean) in gain.iter_mut().zip(mean.iter_mut()) {
        let variance = (*gain - *mean * *mean).max(0.0);
        *gain = variance / (variance + epsilon.max(1e-8));
        *mean *= 1.0 - *gain;
    }
    box_mean_in_place(&mut gain, &mut workspace, width, height, radius);
    box_mean_in_place(&mut mean, &mut workspace, width, height, radius);
    for ((value, gain), offset) in values.iter_mut().zip(&gain).zip(&mean) {
        *value = gain * *value + offset;
    }
    Ok(())
}

/// Converts the model's broad uncertainty band into a hard-looking but still
/// anti-aliased boundary. This happens at the native mask resolution, before
/// the preview atlas can introduce visible stair-steps.
pub(super) fn harden_model_alpha(alpha: &mut [u8]) {
    for value in alpha {
        let probability = *value as f32 / 255.0;
        let hardened = smoothstep(0.20, 0.80, probability);
        *value = (hardened * 255.0 + 0.5) as u8;
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Mean over a (2 × `radius` + 1)² box, shrunk at the image's borders.
/// Rows run in parallel; the vertical pass keeps one running sum per column
/// and walks the image row by row, which reads memory in order.
fn box_mean_in_place(
    values: &mut [f32],
    workspace: &mut [f32],
    width: usize,
    height: usize,
    radius: usize,
) {
    debug_assert_eq!(values.len(), width * height);
    values
        .par_chunks(width)
        .zip(workspace.par_chunks_mut(width))
        .for_each(|(row, means)| {
            let mut sum = row[..radius.min(width - 1) + 1].iter().sum::<f32>();
            for x in 0..width {
                let left = x.saturating_sub(radius);
                let right = (x + radius).min(width - 1);
                means[x] = sum / (right - left + 1) as f32;
                if x >= radius {
                    sum -= row[x - radius];
                }
                if x + radius + 1 < width {
                    sum += row[x + radius + 1];
                }
            }
        });
    let mut sums = vec![0.0f32; width];
    for y in 0..=radius.min(height - 1) {
        for (sum, value) in sums.iter_mut().zip(&workspace[y * width..(y + 1) * width]) {
            *sum += value;
        }
    }
    for y in 0..height {
        let top = y.saturating_sub(radius);
        let bottom = (y + radius).min(height - 1);
        let count = (bottom - top + 1) as f32;
        for (mean, sum) in values[y * width..(y + 1) * width].iter_mut().zip(&sums) {
            *mean = sum / count;
        }
        if y >= radius {
            let leaving = &workspace[(y - radius) * width..(y - radius + 1) * width];
            for (sum, value) in sums.iter_mut().zip(leaving) {
                *sum -= value;
            }
        }
        if y + radius + 1 < height {
            let entering = &workspace[(y + radius + 1) * width..(y + radius + 2) * width];
            for (sum, value) in sums.iter_mut().zip(entering) {
                *sum += value;
            }
        }
    }
}
