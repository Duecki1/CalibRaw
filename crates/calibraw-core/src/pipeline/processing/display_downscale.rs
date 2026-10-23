//! Scaling a rendered, display-encoded picture down for the screen.

use crate::color_math::{srgb_decode, srgb_encode};
use rayon::prelude::*;

/// The source samples output sample `index` of `count` averages, splitting
/// `length` source samples as evenly as the output allows: every source
/// sample belongs to exactly one output sample.
fn partition(index: u32, length: u32, count: u32) -> std::ops::Range<usize> {
    let start = u64::from(index) * u64::from(length) / u64::from(count);
    let end = (u64::from(index + 1) * u64::from(length) / u64::from(count)).max(start + 1);
    start as usize..(end as usize).min(length as usize)
}

/// Averages sRGB-encoded RGBA8 pixels (`input` wide and high) down to
/// `output`, which must not be larger. The average is taken in linear light,
/// so fine detail keeps the brightness the sensor saw instead of darkening
/// as an average of the encoded values would. Alpha is made opaque.
///
/// # Panics
///
/// When `source` is not `input[0] * input[1]` RGBA8 pixels, or an output
/// dimension is zero or larger than the input's.
pub fn downscale_display_rgba8(source: &[u8], input: [u32; 2], output: [u32; 2]) -> Vec<u8> {
    let [input_width, input_height] = input;
    let [output_width, output_height] = output;
    assert_eq!(
        source.len(),
        input_width as usize * input_height as usize * 4,
        "source is not RGBA8 of the stated size"
    );
    assert!(
        (1..=input_width).contains(&output_width) && (1..=input_height).contains(&output_height),
        "output {output:?} is not a reduction of {input:?}"
    );
    let decode: [f32; 256] = std::array::from_fn(|value| srgb_decode(value as f32 / 255.0));
    let mut pixels = vec![255u8; output_width as usize * output_height as usize * 4];
    pixels
        .par_chunks_mut(output_width as usize * 4)
        .enumerate()
        .for_each(|(output_y, row)| {
            let rows = partition(output_y as u32, input_height, output_height);
            for output_x in 0..output_width {
                let columns = partition(output_x, input_width, output_width);
                let mut sum = [0.0f32; 3];
                for source_y in rows.clone() {
                    let line = &source[source_y * input_width as usize * 4..];
                    for source_x in columns.clone() {
                        let pixel = &line[source_x * 4..source_x * 4 + 3];
                        sum[0] += decode[usize::from(pixel[0])];
                        sum[1] += decode[usize::from(pixel[1])];
                        sum[2] += decode[usize::from(pixel[2])];
                    }
                }
                let count = (rows.len() * columns.len()) as f32;
                let destination = output_x as usize * 4;
                for (channel, light) in sum.into_iter().enumerate() {
                    row[destination + channel] = (srgb_encode(light / count) * 255.0 + 0.5) as u8;
                }
            }
        });
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averaging_is_done_in_linear_light() {
        // A black and a white pixel average to the sRGB value of half the light,
        // which is brighter than the 128 of an average of the encoded values.
        let source = [0, 0, 0, 255, 255, 255, 255, 255];
        let averaged = downscale_display_rgba8(&source, [2, 1], [1, 1]);
        assert_eq!(averaged[3], 255);
        assert!((186..=190).contains(&averaged[0]), "{}", averaged[0]);
        assert_eq!(averaged[0], averaged[1]);
        assert_eq!(averaged[1], averaged[2]);
    }

    #[test]
    fn flat_areas_and_the_output_size_are_kept() {
        let source: Vec<u8> = (0..7 * 5).flat_map(|_| [40u8, 90, 200, 255]).collect();
        let averaged = downscale_display_rgba8(&source, [7, 5], [3, 2]);
        assert_eq!(averaged.len(), 3 * 2 * 4);
        for pixel in averaged.chunks_exact(4) {
            assert_eq!(pixel, [40, 90, 200, 255]);
        }
    }

    #[test]
    fn the_same_size_keeps_every_pixel() {
        let source: Vec<u8> = (0..4 * 3 * 4).map(|value| (value * 5) as u8 | 1).collect();
        let kept = downscale_display_rgba8(&source, [4, 3], [4, 3]);
        for (kept, source) in kept.chunks_exact(4).zip(source.chunks_exact(4)) {
            assert_eq!(&kept[..3], &source[..3]);
            assert_eq!(kept[3], 255);
        }
    }

    #[test]
    fn every_source_sample_is_averaged_exactly_once() {
        for (length, count) in [(7u32, 3u32), (10, 4), (5, 5), (1000, 7)] {
            let mut covered = vec![0u32; length as usize];
            for index in 0..count {
                for sample in partition(index, length, count) {
                    covered[sample] += 1;
                }
            }
            assert!(covered.iter().all(|times| *times == 1), "{length}/{count}");
        }
    }
}
