use super::{
    tests::request_test_device, GpuParams, PipelineOptions, ProcessingQuality, RawGpuPipeline,
};
use crate::pipeline::{ColorLut, ColorLutEdit, ExposureParams, LoadedRaw, MaskStack};
use calibraw_core::color_math::{srgb_decode, srgb_encode};

const W: u32 = 64;
const H: u32 = 8;

fn neutral() -> ExposureParams {
    ExposureParams {
        sharpen_amount: 0.0,
        ..Default::default()
    }
}

/// A grey ramp from near black to bright, in scene-linear Rec.2020.
fn ramp() -> anyhow::Result<LoadedRaw> {
    let pixels = (0..W * H)
        .flat_map(|i| {
            let level = 0.02 + 0.8 * (i % W) as f32 / (W - 1) as f32;
            [level; 3]
        })
        .collect();
    LoadedRaw::from_scene_linear_rec2020(W, H, pixels)
}

fn look(amount: f32, map: impl Fn([f32; 3]) -> [f32; 3]) -> ColorLutEdit {
    let mut look = ColorLutEdit::new("test", ColorLut::from_function(17, map).unwrap());
    look.amount = amount;
    look
}

#[test]
fn the_colour_look_follows_its_table_amount_and_removal() -> anyhow::Result<()> {
    let Some((device, queue)) = request_test_device() else {
        eprintln!("colour LUT GPU regression skipped: no headless wgpu adapter");
        return Ok(());
    };
    let source = ramp()?;
    let masks = MaskStack::default();
    let exposure = neutral();
    let pipeline = RawGpuPipeline::new(
        &device,
        &queue,
        &source,
        &GpuParams::new(&exposure, &masks, &source),
        PipelineOptions::new(ProcessingQuality::High),
    )?;
    let render = |look: Option<&ColorLutEdit>| -> anyhow::Result<Vec<f32>> {
        let params = GpuParams::new(&exposure, &masks, &source).with_color_lut(look);
        pipeline.recompute(&queue, &device, &params);
        pipeline.read_display_linear_region_blocking(&device, &queue, 0, 0, W, H)
    };
    let baseline = render(None)?;
    assert!(baseline.iter().all(|value| value.is_finite()));
    let max_difference = |left: &[f32], right: &[f32]| {
        left.iter()
            .zip(right)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max)
    };

    // An identity table changes nothing beyond half-float rounding.
    let identity = render(Some(&look(100.0, |rgb| rgb)))?;
    assert!(
        max_difference(&identity, &baseline) < 0.003,
        "identity moved by {}",
        max_difference(&identity, &baseline)
    );

    // Halving every encoded channel darkens greys exactly as sRGB math says.
    let halved = look(100.0, |rgb| rgb.map(|value| value * 0.5));
    let darkened = render(Some(&halved))?;
    let expected: Vec<f32> = baseline
        .iter()
        .map(|&value| srgb_decode(0.5 * srgb_encode(value.clamp(0.0, 1.0))))
        .collect();
    assert!(
        max_difference(&darkened, &expected) < 0.004,
        "darkened by the wrong amount: {}",
        max_difference(&darkened, &expected)
    );
    assert!(darkened
        .iter()
        .zip(&baseline)
        .all(|(after, before)| after <= before));

    // The amount mixes the look into the image in linear light.
    let half = render(Some(&look(50.0, |rgb| rgb.map(|value| value * 0.5))))?;
    let halfway: Vec<f32> = baseline
        .iter()
        .zip(&expected)
        .map(|(before, after)| 0.5 * (before + after))
        .collect();
    assert!(
        max_difference(&half, &halfway) < 0.004,
        "half amount is off by {}",
        max_difference(&half, &halfway)
    );

    // A look at 0% and removing the look both return to the plain image.
    assert_eq!(
        render(Some(&look(0.0, |rgb| rgb.map(|v| v * 0.5))))?,
        baseline
    );
    render(Some(&halved))?;
    assert_eq!(render(None)?, baseline);

    // Replacing one table with another takes effect on the next render.
    let inverted = render(Some(&look(100.0, |rgb| rgb.map(|value| 1.0 - value))))?;
    assert!(max_difference(&inverted, &baseline) > 0.1);
    assert!(
        inverted[0] > inverted[(W as usize - 1) * 3],
        "the ramp must run backwards"
    );
    Ok(())
}

#[test]
fn the_look_is_part_of_the_uniform_and_texture_budget() {
    let source = ramp().unwrap();
    let masks = MaskStack::default();
    let exposure = neutral();
    let plain = GpuParams::new(&exposure, &masks, &source);
    assert_eq!(plain.effects.film_effects[3], 0.0);
    assert!(plain.color_lut.is_none());

    let with = plain.clone().with_color_lut(Some(&look(40.0, |rgb| rgb)));
    assert!((with.effects.film_effects[3] - 0.4).abs() < 1e-6);
    assert!(with.color_lut.is_some());
    // Everything but the look is unchanged, so the other passes are not redone.
    assert_eq!(with.camera_bytes(), plain.camera_bytes());
    assert_eq!(with.scene_tone_bytes(), plain.scene_tone_bytes());

    // A look at zero never reaches the shader or the upload.
    let off = plain.with_color_lut(Some(&look(0.0, |rgb| rgb)));
    assert_eq!(off.effects.film_effects[3], 0.0);
    assert!(off.color_lut.is_none());
}
