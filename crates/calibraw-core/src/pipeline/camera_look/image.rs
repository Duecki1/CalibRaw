//! Images for the camera match fit: decoding, area resampling, blurring and
//! the alignment of the embedded preview to the render.
//!
//! Coordinates are pixel indices with pixel centres at integers. Planes hold
//! linear sRGB unless noted.

use crate::color_math::srgb_decode;
use crate::pipeline::RawThumbnail;

#[derive(Clone)]
pub(super) struct Plane<T> {
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) pixels: Vec<T>,
}

pub(super) type RgbPlane = Plane<[f32; 3]>;

impl<T: Copy> Plane<T> {
    pub(super) fn at(&self, x: usize, y: usize) -> T {
        self.pixels[y * self.width + x]
    }
}

/// An sRGB-encoded RGBA8 image as linear sRGB.
pub(super) fn decode_linear(image: &RawThumbnail) -> RgbPlane {
    let lut: [f32; 256] = std::array::from_fn(|value| srgb_decode(value as f32 / 255.0));
    let pixels = image
        .rgba
        .chunks_exact(4)
        .map(|pixel| {
            [
                lut[pixel[0] as usize],
                lut[pixel[1] as usize],
                lut[pixel[2] as usize],
            ]
        })
        .collect();
    Plane {
        width: image.width as usize,
        height: image.height as usize,
        pixels,
    }
}

/// Brightest channel value, of 255, up to which a preview row or column
/// counts as part of a black border.
const BORDER_LEVEL: u8 = 16;

/// `preview` without the black bars some cameras put around a preview of
/// another aspect (letterbox or pillarbox). Bars are removed only in pairs
/// of about equal size and only when that brings the preview's aspect
/// closer to `render_aspect` (width / height), so a dark sky or ground is
/// never taken for one.
pub(super) fn trim_preview_borders(preview: &RawThumbnail, render_aspect: f32) -> RawThumbnail {
    let (width, height) = (preview.width as usize, preview.height as usize);
    let dark = |x: usize, y: usize| {
        let pixel = &preview.rgba[(y * width + x) * 4..][..3];
        pixel.iter().all(|&value| value <= BORDER_LEVEL)
    };
    let dark_row = |y: usize| (0..width).all(|x| dark(x, y));
    let dark_column = |x: usize| (0..height).all(|y| dark(x, y));
    let leading = |count: usize, is_dark: &dyn Fn(usize) -> bool| {
        (0..count / 4).take_while(|&index| is_dark(index)).count()
    };
    let top = leading(height, &dark_row);
    let bottom = leading(height, &|index| dark_row(height - 1 - index));
    let left = leading(width, &dark_column);
    let right = leading(width, &|index| dark_column(width - 1 - index));

    let aspect_error = |w: usize, h: usize| ((w as f32 / h as f32) / render_aspect).ln().abs();
    let paired = |a: usize, b: usize, extent: usize| {
        a > 0 && b > 0 && a.abs_diff(b) <= 2.max(extent * 3 / 100)
    };
    let (mut x0, mut x1, mut y0, mut y1) = (0, width, 0, height);
    if paired(top, bottom, height)
        && aspect_error(width, height - top - bottom) < aspect_error(width, height)
    {
        (y0, y1) = (top, height - bottom);
    }
    if paired(left, right, width)
        && aspect_error(width - left - right, y1 - y0) < aspect_error(width, y1 - y0)
    {
        (x0, x1) = (left, width - right);
    }
    let mut rgba = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        rgba.extend_from_slice(&preview.rgba[(y * width + x0) * 4..(y * width + x1) * 4]);
    }
    RawThumbnail {
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        rgba,
    }
}

/// `plane` resampled to `width × height` by averaging the source area each
/// output pixel covers. Enlarging repeats source pixels.
pub(super) fn resample_area(plane: &RgbPlane, width: usize, height: usize) -> RgbPlane {
    let columns = axis_weights(plane.width, width);
    let rows = axis_weights(plane.height, height);
    let mut horizontal = vec![[0.0_f32; 3]; width * plane.height];
    for y in 0..plane.height {
        for (x, taps) in columns.iter().enumerate() {
            let mut sum = [0.0_f32; 3];
            for &(source, weight) in taps {
                let value = plane.pixels[y * plane.width + source];
                for channel in 0..3 {
                    sum[channel] += weight * value[channel];
                }
            }
            horizontal[y * width + x] = sum;
        }
    }
    let mut pixels = vec![[0.0_f32; 3]; width * height];
    for (y, taps) in rows.iter().enumerate() {
        for x in 0..width {
            let mut sum = [0.0_f32; 3];
            for &(source, weight) in taps {
                let value = horizontal[source * width + x];
                for channel in 0..3 {
                    sum[channel] += weight * value[channel];
                }
            }
            pixels[y * width + x] = sum;
        }
    }
    Plane {
        width,
        height,
        pixels,
    }
}

/// For each of `output` pixels along an axis of `input` pixels, the source
/// pixels it covers and their normalized coverage.
fn axis_weights(input: usize, output: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = input as f64 / output as f64;
    (0..output)
        .map(|index| {
            let start = index as f64 * scale;
            let end = ((index + 1) as f64 * scale).min(input as f64);
            if end - start < 1.0 {
                // Enlarging: the source pixel under the output centre.
                let centre = ((start + end) * 0.5).floor() as usize;
                return vec![(centre.min(input - 1), 1.0)];
            }
            let mut taps = Vec::new();
            let mut source = start.floor() as usize;
            while (source as f64) < end && source < input {
                let covered = (end.min(source as f64 + 1.0) - start.max(source as f64)).max(0.0);
                if covered > 0.0 {
                    taps.push((source, (covered / (end - start)) as f32));
                }
                source += 1;
            }
            taps
        })
        .collect()
}

/// A separable Gaussian blur with standard deviation `sigma` in pixels;
/// edges repeat the border pixel.
pub(super) fn blur(plane: &RgbPlane, sigma: f32) -> RgbPlane {
    let radius = (sigma * 3.0).ceil() as isize;
    let kernel: Vec<f32> = (-radius..=radius)
        .map(|offset| (-(offset * offset) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|weight| weight / total).collect();
    let (width, height) = (plane.width as isize, plane.height as isize);
    let pass = |source: &[[f32; 3]], horizontal: bool| {
        let mut output = vec![[0.0_f32; 3]; source.len()];
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0.0_f32; 3];
                for (tap, weight) in kernel.iter().enumerate() {
                    let offset = tap as isize - radius;
                    let (sx, sy) = if horizontal {
                        ((x + offset).clamp(0, width - 1), y)
                    } else {
                        (x, (y + offset).clamp(0, height - 1))
                    };
                    let value = source[(sy * width + sx) as usize];
                    for channel in 0..3 {
                        sum[channel] += weight * value[channel];
                    }
                }
                output[(y * width + x) as usize] = sum;
            }
        }
        output
    };
    let horizontal = pass(&plane.pixels, true);
    Plane {
        width: plane.width,
        height: plane.height,
        pixels: pass(&horizontal, false),
    }
}

/// Bilinear sample of a plane at `(x, y)`; `None` outside the pixel
/// centres' hull.
pub(super) fn sample_bilinear<T, const N: usize>(
    plane: &Plane<T>,
    x: f32,
    y: f32,
    value: impl Fn(T) -> [f32; N],
) -> Option<[f32; N]>
where
    T: Copy,
{
    let max_x = (plane.width - 1) as f32;
    let max_y = (plane.height - 1) as f32;
    if !(0.0..=max_x).contains(&x) || !(0.0..=max_y).contains(&y) {
        return None;
    }
    let x0 = x.floor().min(max_x - 1.0).max(0.0);
    let y0 = y.floor().min(max_y - 1.0).max(0.0);
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as usize, y0 as usize);
    let x1 = (ix + 1).min(plane.width - 1);
    let y1 = (iy + 1).min(plane.height - 1);
    let corners = [
        value(plane.at(ix, iy)),
        value(plane.at(x1, iy)),
        value(plane.at(ix, y1)),
        value(plane.at(x1, y1)),
    ];
    Some(std::array::from_fn(|channel| {
        let top = corners[0][channel] + fx * (corners[1][channel] - corners[0][channel]);
        let bottom = corners[2][channel] + fx * (corners[3][channel] - corners[2][channel]);
        top + fy * (bottom - top)
    }))
}

/// Where a pixel of the working render lies in the working preview:
/// `preview = (render - render_centre) · scale + preview_centre + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitAlignment {
    pub scale: f32,
    /// In working preview pixels.
    pub offset: [f32; 2],
    /// Pearson correlation of the two images' luminance ranks where they
    /// overlap, at this alignment.
    pub correlation: f32,
}

impl FitAlignment {
    pub(super) fn map(
        &self,
        render: (usize, usize),
        preview: (usize, usize),
        x: f32,
        y: f32,
    ) -> (f32, f32) {
        let render_centre = ((render.0 as f32 - 1.0) * 0.5, (render.1 as f32 - 1.0) * 0.5);
        let preview_centre = (
            (preview.0 as f32 - 1.0) * 0.5,
            (preview.1 as f32 - 1.0) * 0.5,
        );
        (
            (x - render_centre.0) * self.scale + preview_centre.0 + self.offset[0],
            (y - render_centre.1) * self.scale + preview_centre.1 + self.offset[1],
        )
    }
}

/// Luminance ranks in `[0, 1]`: each pixel's position in the sorted
/// luminance of its image, which removes any monotone tone difference
/// between the two images before they are compared.
pub(super) fn luminance_ranks(plane: &RgbPlane) -> Plane<f32> {
    let luminance: Vec<f32> = plane
        .pixels
        .iter()
        .map(|[r, g, b]| 0.2126 * r + 0.7152 * g + 0.0722 * b)
        .collect();
    let mut order: Vec<usize> = (0..luminance.len()).collect();
    order.sort_by(|&a, &b| luminance[a].total_cmp(&luminance[b]));
    let mut ranks = vec![0.0_f32; luminance.len()];
    let last = (luminance.len().max(2) - 1) as f32;
    let mut start = 0;
    while start < order.len() {
        // Equal luminances share their mean rank.
        let mut end = start + 1;
        while end < order.len() && luminance[order[end]] == luminance[order[start]] {
            end += 1;
        }
        let rank = (start + end - 1) as f32 * 0.5 / last;
        for &index in &order[start..end] {
            ranks[index] = rank;
        }
        start = end;
    }
    Plane {
        width: plane.width,
        height: plane.height,
        pixels: ranks,
    }
}

/// Halves a rank plane by averaging 2×2 blocks.
fn halve(plane: &Plane<f32>) -> Plane<f32> {
    let width = (plane.width / 2).max(1);
    let height = (plane.height / 2).max(1);
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0;
            let mut count = 0.0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (2 * x + dx, 2 * y + dy);
                if sx < plane.width && sy < plane.height {
                    sum += plane.at(sx, sy);
                    count += 1.0;
                }
            }
            pixels.push(sum / count);
        }
    }
    Plane {
        width,
        height,
        pixels,
    }
}

/// The share of render pixels that must land inside the preview.
const MIN_OVERLAP: f32 = 0.5;
/// Render pixels this close to the border are not compared.
const ALIGNMENT_MARGIN: usize = 3;

/// The mean squared rank difference at `alignment` and the share of render
/// pixels compared, or `None` without enough overlap.
fn alignment_cost(
    render: &Plane<f32>,
    preview: &Plane<f32>,
    alignment: &FitAlignment,
) -> Option<f32> {
    let mut sum = 0.0;
    let mut count = 0usize;
    let mut total = 0usize;
    let margin = ALIGNMENT_MARGIN
        .min(render.width / 4)
        .min(render.height / 4);
    for y in margin..render.height - margin {
        for x in margin..render.width - margin {
            total += 1;
            let (px, py) = alignment.map(
                (render.width, render.height),
                (preview.width, preview.height),
                x as f32,
                y as f32,
            );
            if let Some([value]) = sample_bilinear(preview, px, py, |v| [v]) {
                let difference = render.at(x, y) - value;
                sum += difference * difference;
                count += 1;
            }
        }
    }
    (total > 0 && count as f32 >= MIN_OVERLAP * total as f32).then(|| sum / count as f32)
}

fn rank_correlation(render: &Plane<f32>, preview: &Plane<f32>, alignment: &FitAlignment) -> f32 {
    let mut pairs = Vec::new();
    for y in 0..render.height {
        for x in 0..render.width {
            let (px, py) = alignment.map(
                (render.width, render.height),
                (preview.width, preview.height),
                x as f32,
                y as f32,
            );
            if let Some([value]) = sample_bilinear(preview, px, py, |v| [v]) {
                pairs.push((render.at(x, y), value));
            }
        }
    }
    let count = pairs.len().max(1) as f64;
    let mean_a = pairs.iter().map(|(a, _)| f64::from(*a)).sum::<f64>() / count;
    let mean_b = pairs.iter().map(|(_, b)| f64::from(*b)).sum::<f64>() / count;
    let (mut covariance, mut variance_a, mut variance_b) = (0.0, 0.0, 0.0);
    for (a, b) in &pairs {
        let (da, db) = (f64::from(*a) - mean_a, f64::from(*b) - mean_b);
        covariance += da * db;
        variance_a += da * da;
        variance_b += db * db;
    }
    let denominator = (variance_a * variance_b).sqrt();
    if denominator > 0.0 {
        (covariance / denominator) as f32
    } else {
        0.0
    }
}

/// One level of the alignment search: scales `scale_steps` either side of
/// `centre.scale` in steps of `scale_step`, and offsets `offset_steps` either
/// side of `centre.offset` in steps of `offset_step` pixels of this level.
struct SearchLevel {
    scale_steps: i32,
    scale_step: f32,
    offset_steps: i32,
    offset_step: f32,
}

/// The coarsest level covers scales 0.90–1.10 and offsets of ±6 pixels at a
/// quarter of the working size (±24 working pixels); each finer level
/// refines the previous one's result at twice its resolution.
const SEARCH_LEVELS: [SearchLevel; 3] = [
    SearchLevel {
        scale_steps: 5,
        scale_step: 0.02,
        offset_steps: 6,
        offset_step: 1.0,
    },
    SearchLevel {
        scale_steps: 2,
        scale_step: 0.01,
        offset_steps: 2,
        offset_step: 1.0,
    },
    SearchLevel {
        scale_steps: 2,
        scale_step: 0.0025,
        offset_steps: 2,
        offset_step: 0.5,
    },
];

/// Searches the scale and offset that best overlay the preview on the
/// render, from a quarter of the working size to the full one. `None` when
/// no candidate overlaps enough.
pub(super) fn align(render: &RgbPlane, preview: &RgbPlane) -> Option<FitAlignment> {
    let render_ranks = luminance_ranks(render);
    let preview_ranks = luminance_ranks(preview);
    let render_half = halve(&render_ranks);
    let preview_half = halve(&preview_ranks);
    let pyramid = [
        (halve(&render_half), halve(&preview_half)),
        (render_half, preview_half),
        (render_ranks, preview_ranks),
    ];

    let mut centre = FitAlignment {
        scale: 1.0,
        offset: [0.0, 0.0],
        correlation: 0.0,
    };
    for (level, (render, preview)) in SEARCH_LEVELS.iter().zip(&pyramid) {
        let mut best: Option<(f32, FitAlignment)> = None;
        for scale_step in -level.scale_steps..=level.scale_steps {
            let scale = centre.scale + scale_step as f32 * level.scale_step;
            for dy in -level.offset_steps..=level.offset_steps {
                for dx in -level.offset_steps..=level.offset_steps {
                    let candidate = FitAlignment {
                        scale,
                        offset: [
                            centre.offset[0] + dx as f32 * level.offset_step,
                            centre.offset[1] + dy as f32 * level.offset_step,
                        ],
                        correlation: 0.0,
                    };
                    let Some(cost) = alignment_cost(render, preview, &candidate) else {
                        continue;
                    };
                    if best.as_ref().is_none_or(|(best_cost, _)| cost < *best_cost) {
                        best = Some((cost, candidate));
                    }
                }
            }
        }
        let (_, found) = best?;
        // The next level has twice the resolution.
        centre = FitAlignment {
            offset: [found.offset[0] * 2.0, found.offset[1] * 2.0],
            ..found
        };
    }
    let [render_ranks, preview_ranks] = [&pyramid[2].0, &pyramid[2].1];
    // The loop doubled the last level's offset for a level that does not
    // exist.
    centre.offset = [centre.offset[0] * 0.5, centre.offset[1] * 0.5];
    centre.correlation = rank_correlation(render_ranks, preview_ranks, &centre);
    Some(centre)
}
