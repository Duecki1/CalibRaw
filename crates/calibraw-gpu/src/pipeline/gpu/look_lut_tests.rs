//! The LUT pass (look_lut.wgsl) against `CubeLut::sample_tetrahedral`.

use super::{
    shaders, tests::request_test_device, GpuParams, PipelineOptions, ProcessingQuality,
    RawGpuPipeline,
};
use crate::pipeline::{ExposureParams, LoadedRaw, MaskStack};
use calibraw_core::color_math::{
    linear_srgb_to_rec2020, rec2020_to_linear_srgb, srgb_decode, srgb_encode,
};
use calibraw_core::luts::{parse_cube, CubeLut, ResolvedLook};
use std::fmt::Write as _;
use std::sync::Arc;

const WIDTH: u32 = 6;

/// Muted colours that stay inside sRGB after the view transform, so the
/// LUT's input encoding needs no gamut compression.
fn source() -> anyhow::Result<LoadedRaw> {
    LoadedRaw::from_scene_linear_rec2020(
        WIDTH,
        1,
        vec![
            0.18, 0.18, 0.18, // grey
            0.30, 0.14, 0.08, // skin
            0.10, 0.18, 0.12, // foliage
            0.08, 0.10, 0.30, // sky
            0.60, 0.52, 0.42, // highlight
            0.02, 0.02, 0.025, // shadow
        ],
    )
}

fn cube(size: u32, map: impl Fn([f32; 3]) -> [f32; 3]) -> Arc<CubeLut> {
    let mut text = format!("LUT_3D_SIZE {size}\n");
    let last = (size - 1) as f32;
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let [x, y, z] = map([r as f32 / last, g as f32 / last, b as f32 / last]);
                writeln!(text, "{x:.7} {y:.7} {z:.7}").unwrap();
            }
        }
    }
    Arc::new(parse_cube(text.as_bytes()).unwrap())
}

fn warm([r, g, b]: [f32; 3]) -> [f32; 3] {
    [
        (r * 1.08 + 0.02).min(1.0),
        g.powf(1.15),
        b * 0.85 + 0.04 * (1.0 - b),
    ]
}

fn look(lut: &Arc<CubeLut>, amount: f32) -> ResolvedLook {
    ResolvedLook {
        lut: Arc::clone(lut),
        amount,
    }
}

struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    pipeline: RawGpuPipeline,
}

impl Renderer {
    fn new() -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            return Ok(None);
        };
        let source = source()?;
        let pipeline = RawGpuPipeline::new(
            &device,
            &queue,
            &source,
            &GpuParams::new(&ExposureParams::default(), &MaskStack::default(), &source),
            PipelineOptions::new(ProcessingQuality::High).mask_atlas_edge(64),
        )?;
        Ok(Some(Self {
            device,
            queue,
            source,
            pipeline,
        }))
    }

    /// Display-linear Rec.2020 and the encoded output of every pixel.
    fn render(&self, look: Option<&ResolvedLook>) -> anyhow::Result<(Vec<f32>, Vec<u8>)> {
        let params = GpuParams::new(
            &ExposureParams::default(),
            &MaskStack::default(),
            &self.source,
        )
        .with_look_lut(look);
        self.pipeline.recompute(&self.queue, &self.device, &params);
        let display = self.pipeline.read_display_linear_region_blocking(
            &self.device,
            &self.queue,
            0,
            0,
            WIDTH,
            1,
        )?;
        let output =
            self.pipeline
                .read_output_region_blocking(&self.device, &self.queue, 0, 0, WIDTH, 1)?;
        Ok((display, output))
    }
}

/// What the pass should make of the display-linear `input`.
fn expected(input: [f32; 3], look: &ResolvedLook) -> [f32; 3] {
    let linear_srgb = rec2020_to_linear_srgb(input);
    assert!(
        linear_srgb.iter().all(|value| (0.0..=1.0).contains(value)),
        "{input:?} must stay inside sRGB"
    );
    let encoded = linear_srgb.map(srgb_encode);
    let looked = look.lut.sample_tetrahedral(encoded);
    let change = linear_srgb_to_rec2020(std::array::from_fn(|channel| {
        srgb_decode(looked[channel]) - srgb_decode(encoded[channel])
    }));
    std::array::from_fn(|channel| input[channel] + look.amount * change[channel])
}

#[test]
fn an_identity_lut_changes_nothing() -> anyhow::Result<()> {
    let Some(renderer) = Renderer::new()? else {
        return Ok(());
    };
    let (plain, plain_output) = renderer.render(None)?;
    let identity = cube(17, |rgb| rgb);
    let (looked, looked_output) = renderer.render(Some(&look(&identity, 1.0)))?;
    assert_eq!(looked_output, plain_output);
    for (looked, plain) in looked.iter().zip(&plain) {
        assert!((looked - plain).abs() < 1e-5, "{looked} != {plain}");
    }
    Ok(())
}

#[test]
fn the_lut_matches_the_cpu_reference() -> anyhow::Result<()> {
    let Some(renderer) = Renderer::new()? else {
        return Ok(());
    };
    let (plain, _) = renderer.render(None)?;
    let warm = cube(9, warm);
    for amount in [1.0, 0.4] {
        let look = look(&warm, amount);
        let (looked, _) = renderer.render(Some(&look))?;
        for (input, output) in plain.chunks_exact(3).zip(looked.chunks_exact(3)) {
            let expected = expected([input[0], input[1], input[2]], &look);
            for channel in 0..3 {
                assert!(
                    (output[channel] - expected[channel]).abs() < 2e-4,
                    "amount {amount}: {input:?} became {output:?}, expected {expected:?}"
                );
            }
        }
        assert_ne!(looked, plain);
    }
    Ok(())
}

#[test]
fn switching_luts_uploads_each_one() -> anyhow::Result<()> {
    let Some(renderer) = Renderer::new()? else {
        return Ok(());
    };
    let small = cube(5, warm);
    let large = cube(9, |[r, g, b]| [g, b, r]);
    let (first, _) = renderer.render(Some(&look(&small, 1.0)))?;
    let (rotated, _) = renderer.render(Some(&look(&large, 1.0)))?;
    let (again, _) = renderer.render(Some(&look(&small, 1.0)))?;
    assert_ne!(first, rotated);
    assert_eq!(first, again);
    Ok(())
}

#[test]
fn a_zero_amount_renders_like_no_lut() -> anyhow::Result<()> {
    let source = source()?;
    let plain = GpuParams::new(&ExposureParams::default(), &MaskStack::default(), &source);
    let zero = plain
        .clone()
        .with_look_lut(Some(&look(&cube(2, warm), 0.0)));
    assert!(zero.look_lut().is_none());
    assert_eq!(zero.scene_tone_bytes(), plain.scene_tone_bytes());
    Ok(())
}

#[test]
fn the_lut_follows_point_colours_and_precedes_grain_and_vignette() {
    let text = shaders::VIEW_TRANSFORM.text;
    let position = |call: &str| {
        text.find(call)
            .unwrap_or_else(|| panic!("view_transform.wgsl calls {call}"))
    };
    let point_colours = position("display_linear = apply_local_point_colors(");
    let look = position("display_linear = LookLut::apply_look_lut(display_linear);");
    let film_finish = position("CreativeEffects::apply_film_finish_modules(");
    assert!(point_colours < look && look < film_finish);
}
