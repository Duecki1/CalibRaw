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
