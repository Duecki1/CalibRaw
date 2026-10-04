//! Android replay encoding through MediaCodec (`calibraw-ffi`).

#[cfg(target_os = "android")]
use super::REPLAY_FPS;
#[cfg(target_os = "android")]
use std::path::Path;

#[cfg(target_os = "android")]
pub(super) struct ReplayFrameWriter {
    encoder: Option<calibraw_ffi::ReplayVideoEncoder>,
    width: usize,
    height: usize,
    yuv: Vec<u8>,
}

#[cfg(target_os = "android")]
impl ReplayFrameWriter {
    pub(super) fn start(
        app: &calibraw_ffi::AndroidApp,
        path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            encoder: Some(calibraw_ffi::ReplayVideoEncoder::start(
                app, path, width, height, REPLAY_FPS,
            )?),
            width: width as usize,
            height: height as usize,
            yuv: vec![0; width as usize * height as usize * 3 / 2],
        })
    }

    pub(super) fn write_frame(&mut self, rgb: &[u8]) -> Result<(), String> {
        rgb_to_yuv420(rgb, self.width, self.height, &mut self.yuv);
        self.encoder
            .as_mut()
            .ok_or("Replay encoder is closed")?
            .write_frame(&self.yuv)
    }

    pub(super) fn finish(mut self) -> Result<(), String> {
        self.encoder
            .take()
            .ok_or("Replay encoder is closed")?
            .finish()
    }

    pub(super) fn cancel(&mut self) {
        self.encoder.take();
    }
}

// BT.601 limited range, matching the Android codec's configured color metadata.
fn rgb_to_yuv420(rgb: &[u8], width: usize, height: usize, yuv: &mut [u8]) {
    assert!(width.is_multiple_of(2) && height.is_multiple_of(2));
    let pixels = width * height;
    assert_eq!(rgb.len(), pixels * 3);
    assert_eq!(yuv.len(), pixels * 3 / 2);
    let (luma, chroma) = yuv.split_at_mut(pixels);
    let (u, v) = chroma.split_at_mut(pixels / 4);
    for row in (0..height).step_by(2) {
        for col in (0..width).step_by(2) {
            let mut sum = [0i32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let index = (row + dy) * width + col + dx;
                    let r = i32::from(rgb[index * 3]);
                    let g = i32::from(rgb[index * 3 + 1]);
                    let b = i32::from(rgb[index * 3 + 2]);
                    luma[index] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                    sum[0] += r;
                    sum[1] += g;
                    sum[2] += b;
                }
            }
            let [r, g, b] = sum.map(|value| (value + 2) / 4);
            let index = row / 2 * (width / 2) + col / 2;
            u[index] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            v[index] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_video_colors_use_limited_range_planar_yuv() {
        for (rgb, expected) in [
            ([0, 0, 0], [16, 128, 128]),
            ([255, 255, 255], [235, 128, 128]),
            ([255, 0, 0], [82, 90, 240]),
            ([0, 255, 0], [144, 54, 34]),
            ([0, 0, 255], [41, 240, 110]),
        ] {
            let mut output = [0; 6];
            rgb_to_yuv420(&rgb.repeat(4), 2, 2, &mut output);
            assert_eq!(
                output,
                [
                    expected[0],
                    expected[0],
                    expected[0],
                    expected[0],
                    expected[1],
                    expected[2]
                ]
            );
        }
    }

    #[test]
    fn chroma_averages_each_two_by_two_block() {
        let mut output = [0; 6];
        rgb_to_yuv420(
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
            2,
            2,
            &mut output,
        );
        assert_eq!(output, [82, 144, 41, 235, 128, 128]);
    }
}
