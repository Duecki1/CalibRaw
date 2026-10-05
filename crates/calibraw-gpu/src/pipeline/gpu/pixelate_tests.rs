//! Pixelate block cache: blocks of 12 px and more are averaged once by
//! `prepare_pixelate_blocks` and looked up by the creative pass.

use super::{
    tests::request_test_device, GpuParams, PipelineOptions, ProcessingQuality, RawGpuPipeline,
};
use crate::pipeline::{
    extract_padded_tile, EffectComponent, ExportTile, ExposureParams, LoadedRaw, MaskEffect,
    MaskStack, ProcessingStage,
};

const EPSILON: f32 = 2.0e-4;
// Small test images use the minimum reference scale of 0.55.
const BLOCK_12: f32 = 22.0;
const BLOCK_18: f32 = 32.0;

struct Scene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    exposure: ExposureParams,
    quality: ProcessingQuality,
    pipeline: RawGpuPipeline,
}

impl Scene {
    fn new(
        width: u32,
        height: u32,
        quality: ProcessingQuality,
        pixel: impl Fn(u32, u32) -> [f32; 3],
    ) -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            eprintln!("Pixelate GPU regression skipped: no headless wgpu adapter");
            return Ok(None);
        };
        let pixels = (0..width * height)
            .flat_map(|i| pixel(i % width, i / width))
            .collect();
        let source = LoadedRaw::from_scene_linear_rec2020(width, height, pixels)?;
        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        let pipeline = RawGpuPipeline::new(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &MaskStack::default(), &source),
            PipelineOptions::new(quality),
        )?;
        Ok(Some(Self {
            device,
            queue,
            source,
            exposure,
            quality,
            pipeline,
        }))
    }

    /// Display-linear RGB at High quality; Preview cannot read it back, so it
    /// returns the encoded output instead, scaled to 0..1.
    fn render(&self, masks: &MaskStack) -> anyhow::Result<Vec<f32>> {
        self.pipeline.recompute(
            &self.queue,
            &self.device,
            &GpuParams::new(&self.exposure, masks, &self.source),
        );
        if self.quality == ProcessingQuality::Preview {
            let rgba = self.pipeline.read_output_region_blocking(
                &self.device,
                &self.queue,
                0,
                0,
                self.source.width,
                self.source.height,
            )?;
            return Ok(rgba
                .chunks_exact(4)
                .flat_map(|pixel| pixel[..3].iter().map(|&v| f32::from(v) / 255.0))
                .collect());
        }
        let result = self.pipeline.read_display_linear_region_blocking(
            &self.device,
            &self.queue,
            0,
            0,
            self.source.width,
            self.source.height,
        )?;
        assert!(result.iter().all(|v| v.is_finite()), "non-finite render");
        Ok(result)
    }

    fn rgb<'a>(&self, image: &'a [f32], x: u32, y: u32) -> &'a [f32] {
        let i = ((y * self.source.width + x) * 3) as usize;
        &image[i..i + 3]
    }
}

fn pixelate(amount: f32, block_size: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Pixelate);
    component.settings.pixelate.amount = amount;
    component.settings.pixelate.block_size = block_size;
    component
}

fn global(components: impl IntoIterator<Item = EffectComponent>) -> MaskStack {
    MaskStack {
        global_effects: components.into_iter().collect(),
        ..Default::default()
    }
}

fn difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

#[test]
fn cached_pixelate_averages_checkerboards_and_partial_edge_blocks() -> anyhow::Result<()> {
    // 66 = 3 * 18 + 12 = 5 * 12 + 6: every block, including the truncated edge
    // blocks, holds as many dark as light pixels and must average identically.
    for quality in [ProcessingQuality::High, ProcessingQuality::Preview] {
        let Some(scene) = Scene::new(66, 66, quality, |x, y| {
            [if (x + y).is_multiple_of(2) { 0.1 } else { 0.8 }; 3]
        })?
        else {
            return Ok(());
        };
        for block_size in [BLOCK_12, BLOCK_18] {
            let actual = scene.render(&global([pixelate(100.0, block_size)]))?;
            let mid = scene.rgb(&actual, 33, 33);
            for y in 0..66 {
                for x in 0..66 {
                    assert!(
                        difference(mid, scene.rgb(&actual, x, y)) < EPSILON,
                        "{quality:?} block {block_size}: average changed at {x},{y}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn cached_pixelate_reads_each_blocks_own_average() -> anyhow::Result<()> {
    const WIDTH: u32 = 100;
    const HEIGHT: u32 = 70;
    // Distinct levels per 18 px block, with a small ripple inside each block.
    let level = |x: u32, y: u32| {
        let block = (x / 18 * 3 + y / 18 * 5) % 7;
        let ripple = if (x + 2 * y).is_multiple_of(3) {
            0.01
        } else {
            0.0
        };
        [0.05 + 0.1 * block as f32 + ripple; 3]
    };
    let Some(scene) = Scene::new(WIDTH, HEIGHT, ProcessingQuality::High, level)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    // The ripple is small, so the view transform of a block mean is close to the
    // mean of the transformed block. Neighbouring blocks differ far more.
    let block_mean = |x: u32, y: u32, size: u32| -> [f32; 3] {
        let (x0, y0) = (x / size * size, y / size * size);
        let (x1, y1) = ((x0 + size).min(WIDTH), (y0 + size).min(HEIGHT));
        let mut sum = [0.0; 3];
        for yy in y0..y1 {
            for xx in x0..x1 {
                for (s, v) in sum.iter_mut().zip(scene.rgb(&baseline, xx, yy)) {
                    *s += v;
                }
            }
        }
        sum.map(|s| s / ((x1 - x0) * (y1 - y0)) as f32)
    };

    let single = scene.render(&global([pixelate(100.0, BLOCK_18)]))?;
    // Two sizes cached at once: the second is mixed in at half strength over
    // the first, and both average the same creative-pass input.
    let mixed = scene.render(&global([
        pixelate(100.0, BLOCK_12),
        pixelate(50.0, BLOCK_18),
    ]))?;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let expected = block_mean(x, y, 18);
            assert!(
                difference(scene.rgb(&single, x, y), &expected) < 0.01,
                "block 18 at {x},{y}: {:?} != {expected:?}",
                scene.rgb(&single, x, y)
            );
            let small = block_mean(x, y, 12);
            let expected: Vec<f32> = small
                .iter()
                .zip(expected)
                .map(|(a, b)| 0.5 * (a + b))
                .collect();
            assert!(
                difference(scene.rgb(&mixed, x, y), &expected) < 0.01,
                "blocks 12 and 18 at {x},{y}: {:?} != {expected:?}",
                scene.rgb(&mixed, x, y)
            );
        }
    }
    Ok(())
}

#[test]
fn cached_pixelate_tiles_match_the_full_frame_off_grid() -> anyhow::Result<()> {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 120;
    const HALO: u32 = 24;
    let Some(scene) = Scene::new(WIDTH, HEIGHT, ProcessingQuality::High, |x, y| {
        let u = x as f32 / WIDTH as f32;
        let v = y as f32 / HEIGHT as f32;
        let dots = if (x * 7 + y * 13) % 11 == 0 { 0.4 } else { 0.0 };
        [0.05 + 0.6 * u + dots, 0.1 + 0.5 * v, 0.3 + 0.2 * u * v]
    })?
    else {
        return Ok(());
    };
    let masks = global([pixelate(100.0, BLOCK_18)]);
    let full = scene.render(&masks)?;
    // Tile origins off the 18 px grid, including one whose halo starts outside
    // the image, so the cached grid starts on a partial block.
    for (x, y) in [(41, 29), (10, 7), (100, 70)] {
        let core_width = (WIDTH - x).min(50);
        let core_height = (HEIGHT - y).min(40);
        let tile = ExportTile {
            core_x: x,
            core_y: y,
            core_width,
            core_height,
            local_core_x: HALO,
            local_core_y: HALO,
            padded_width: core_width + 2 * HALO,
            padded_height: core_height + 2 * HALO,
            global_origin_x: x as i32 - HALO as i32,
            global_origin_y: y as i32 - HALO as i32,
        };
        let raw = extract_padded_tile(&scene.source, tile);
        let params = GpuParams::new_for_tile(
            &scene.exposure,
            &masks,
            &raw,
            tile.global_origin_x,
            tile.global_origin_y,
            WIDTH,
            HEIGHT,
        );
        let pipeline = RawGpuPipeline::new(
            &scene.device,
            &scene.queue,
            &raw,
            &params,
            PipelineOptions::new(ProcessingQuality::High)
                .programs(&scene.pipeline.program_template()),
        )?;
        pipeline.dispatch_stage(&scene.queue, &scene.device, &params, ProcessingStage::Raw);
        pipeline.dispatch_tone_guide_with_inherited_statistics(
            &scene.queue,
            &scene.device,
            &params,
            &scene.pipeline,
        );
        pipeline.dispatch_stage(
            &scene.queue,
            &scene.device,
            &params,
            ProcessingStage::Output,
        );
        let actual = pipeline.read_display_linear_region_blocking(
            &scene.device,
            &scene.queue,
            tile.local_core_x,
            tile.local_core_y,
            core_width,
            core_height,
        )?;
        for row in 0..core_height {
            for column in 0..core_width {
                let i = ((row * core_width + column) * 3) as usize;
                let expected = scene.rgb(&full, x + column, y + row);
                assert!(
                    difference(&actual[i..i + 3], expected) < EPSILON,
                    "tile at {x},{y}: pixel {},{} {:?} != {expected:?}",
                    x + column,
                    y + row,
                    &actual[i..i + 3]
                );
            }
        }
    }
    Ok(())
}
