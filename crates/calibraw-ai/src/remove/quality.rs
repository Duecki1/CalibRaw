//! Sampling and mask operations for the fixed-size LaMa model. Keep the native
//! coverage separate from model pixels so enlarging a repair never enlarges a
//! 512px mask's stair steps.
use image::{GrayImage, Luma, Rgb32FImage};

pub(super) fn fit_dimensions(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let scale = (edge as f64 / width.max(height) as f64).min(1.0);
    (
        (width as f64 * scale).round().max(1.0) as u32,
        (height as f64 * scale).round().max(1.0) as u32,
    )
}

/// Every source mask pixel must reach the model, even thin strokes reduced by
/// more than one model pixel. Nearest-neighbor sampling can miss them entirely.
pub(super) fn resize_mask(mask: &GrayImage, width: u32, height: u32) -> GrayImage {
    let mut result = GrayImage::new(width, height);
    for (x, y, pixel) in mask.enumerate_pixels() {
        if pixel[0] == 0 {
            continue;
        }
        let left = x as u64 * width as u64 / mask.width() as u64;
        let right = ((x as u64 + 1) * width as u64).div_ceil(mask.width() as u64);
        let top = y as u64 * height as u64 / mask.height() as u64;
        let bottom = ((y as u64 + 1) * height as u64).div_ceil(mask.height() as u64);
        for dy in top..bottom {
            for dx in left..right {
                result.put_pixel(dx as u32, dy as u32, Luma([255]));
            }
        }
    }
    result
}

pub(super) fn reflect(index: u32, length: u32) -> u32 {
    if length <= 1 {
        return 0;
    }
    let period = 2 * (length - 1);
    let index = index % period;
    index.min(period - index)
}

/// Approximate Euclidean distance in two linear passes; image edges are not
/// treated as clean context. Thus a mask touching an image edge still fills it.
pub(super) fn mask_distance(mask: &GrayImage, to_covered: bool) -> Vec<f32> {
    let width = mask.width() as usize;
    let height = mask.height() as usize;
    let far = (width + height + 1) as f32;
    let mut distance: Vec<f32> = mask
        .as_raw()
        .iter()
        .map(|v| if (*v != 0) == to_covered { 0.0 } else { far })
        .collect();
    let diagonal = std::f32::consts::SQRT_2;
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if x > 0 {
                distance[i] = distance[i].min(distance[i - 1] + 1.0);
            }
            if y > 0 {
                distance[i] = distance[i].min(distance[i - width] + 1.0);
                if x > 0 {
                    distance[i] = distance[i].min(distance[i - width - 1] + diagonal);
                }
                if x + 1 < width {
                    distance[i] = distance[i].min(distance[i - width + 1] + diagonal);
                }
            }
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let i = y * width + x;
            if x + 1 < width {
                distance[i] = distance[i].min(distance[i + 1] + 1.0);
            }
            if y + 1 < height {
                distance[i] = distance[i].min(distance[i + width] + 1.0);
                if x > 0 {
                    distance[i] = distance[i].min(distance[i + width - 1] + diagonal);
                }
                if x + 1 < width {
                    distance[i] = distance[i].min(distance[i + width + 1] + diagonal);
                }
            }
        }
    }
    distance
}

pub(super) fn feather_mask(mask: &GrayImage, radius: f32) -> Vec<u8> {
    mask_distance(mask, false)
        .into_iter()
        .map(|d| {
            // Smoothly reach zero at the outside pixel edge, not a clipped 50%
            // Gaussian at the boundary. Full opacity is reached inside the guard.
            let t = ((d - 0.5) / radius.max(1.0)).clamp(0.0, 1.0);
            (255.0 * t * t * (3.0 - 2.0 * t)).round() as u8
        })
        .collect()
}

pub(super) fn sample_cubic(image: &Rgb32FImage, u: f32, v: f32) -> [f32; 3] {
    fn axis(position: f32, length: u32) -> [(u32, f32); 4] {
        let position = (position * length as f32 - 0.5).clamp(0.0, (length - 1) as f32);
        let base = position.floor() as i32;
        let t = position - base as f32;
        let weights = [
            -0.5 * t + t * t - 0.5 * t * t * t,
            1.0 - 2.5 * t * t + 1.5 * t * t * t,
            0.5 * t + 2.0 * t * t - 1.5 * t * t * t,
            -0.5 * t * t + 0.5 * t * t * t,
        ];
        std::array::from_fn(|i| {
            (
                (base + i as i32 - 1).clamp(0, length as i32 - 1) as u32,
                weights[i],
            )
        })
    }
    let mut result = [0.0; 3];
    for (y, wy) in axis(v, image.height()) {
        for (x, wx) in axis(u, image.width()) {
            let pixel = image.get_pixel(x, y);
            for c in 0..3 {
                result[c] += pixel[c] * wx * wy;
            }
        }
    }
    result.map(|v| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn thin_strokes_survive_reduction_and_non_square_images_keep_proportions() {
        assert_eq!(fit_dimensions(1536, 768, 512), (512, 256));
        assert_eq!(fit_dimensions(120, 80, 512), (120, 80));
        let mut mask = GrayImage::new(2048, 16);
        mask.put_pixel(5, 7, Luma([255]));
        let reduced = resize_mask(&mask, 64, 1);
        assert_eq!(reduced.get_pixel(0, 0)[0], 255);
        assert_eq!(reduced.pixels().filter(|p| p[0] != 0).count(), 1);
        assert_eq!(
            (0..6).map(|x| reflect(x, 3)).collect::<Vec<_>>(),
            vec![0, 1, 2, 1, 0, 1]
        );
        assert_eq!(reflect(511, 1), 0);
    }

    #[test]
    fn feather_is_smooth_inside_mask_and_opaque_before_reaching_subject() {
        let mask = GrayImage::from_fn(40, 12, |x, _| {
            Luma([if (5..35).contains(&x) { 255 } else { 0 }])
        });
        let alpha = feather_mask(&mask, 6.0);
        assert_eq!(alpha[4], 0);
        assert!(alpha[5] < 8); // no 50% jump at the binary boundary
        assert_eq!(alpha[12], 255);
        assert_eq!(alpha[35], 0);
        assert!(alpha[5..12].windows(2).all(|w| w[0] <= w[1]));
        // Image border must not fade a repair that touches it.
        assert_eq!(alpha[12], alpha[6 * 40 + 12]);
    }

    #[test]
    fn cubic_reconstruction_preserves_gradients_and_sub_byte_color() {
        let source = Rgb32FImage::from_fn(8, 8, |x, y| {
            Rgb([0.25 + x as f32 * 0.001, 0.3 + y as f32 * 0.002, 0.34567])
        });
        for x in 16..48 {
            let u = (x as f32 + 0.5) / 64.0;
            let rgb = sample_cubic(&source, u, 0.5);
            assert!((rgb[0] - (0.25 + (u * 8.0 - 0.5) * 0.001)).abs() < 1e-6);
            assert!((rgb[2] - 0.34567).abs() < 1e-6);
        }
    }
}
