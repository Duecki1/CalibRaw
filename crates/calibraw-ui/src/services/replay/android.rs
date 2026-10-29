//! Replay encoding through the platform's codec (MediaCodec on Android),
//! which takes planar YUV.

use super::REPLAY_FPS;
use crate::i18n::tr;
use crate::platform::{Platform, ReplayVideoEncoder};
use std::path::Path;

pub(super) struct ReplayFrameWriter {
    encoder: Option<Box<dyn ReplayVideoEncoder>>,
    width: usize,
    height: usize,
    yuv: Vec<u8>,
}

impl ReplayFrameWriter {
    pub(super) fn start(
        platform: &dyn Platform,
        path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            encoder: Some(platform.start_replay_encoder(path, width, height, REPLAY_FPS)?),
            width: width as usize,
            height: height as usize,
            yuv: vec![0; width as usize * height as usize * 3 / 2],
        })
    }

    pub(super) fn write_frame(&mut self, rgb: &[u8]) -> Result<(), String> {
        rgb_to_yuv420(rgb, self.width, self.height, &mut self.yuv);
        self.encoder
            .as_mut()
            .ok_or(tr("Replay encoder is closed"))?
            .write_frame(&self.yuv)
    }

    pub(super) fn finish(mut self) -> Result<(), String> {
        self.encoder
            .take()
            .ok_or(tr("Replay encoder is closed"))?
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
