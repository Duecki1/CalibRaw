//! Chromatic aberration (chromatic_aberration.wgsl): the lateral model moves
//! red and blue onto green, the fringe pass desaturates axial fringes, and
//! neither touches aligned colour.

use super::tests::request_test_device_with_info;
use super::{GpuParams, PipelineOptions, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::raw_loader::{CameraColorModel, CameraWhiteBalanceModel};
use crate::pipeline::{
    AiDenoisedImage, CfaKind, CompactPixelMap, ExposureParams, LateralChromaticAberration,
    LoadedRaw, MaskStack,
};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;

/// A rotated checkerboard with soft edges, in [0.1, 0.8].
fn pattern(x: f32, y: f32) -> f32 {
    let (u, v) = (0.866 * x + 0.5 * y, -0.5 * x + 0.866 * y);
    let soft = |t: f32, period: f32| (std::f32::consts::TAU * t / period).sin().tanh() * 4.0;
    0.45 + 0.35 * (soft(u, 37.0) * soft(v, 29.0)).tanh()
}

fn raster(rgb_at: impl Fn(f32, f32) -> [f32; 3]) -> LoadedRaw {
    let mut rgb = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            rgb.extend(rgb_at(x as f32, y as f32));
        }
    }
    LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, rgb).expect("valid raster")
}

fn exposure(amount: f32) -> ExposureParams {
    ExposureParams {
        chromatic_aberration: amount,
        ..ExposureParams::default()
    }
}

/// The RAW-stage scene (camera RGB) of `raw` at chromatic aberration `amount`.
fn render_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    raw: &LoadedRaw,
    exposure: &ExposureParams,
) -> anyhow::Result<Vec<f32>> {
    let params = GpuParams::new(exposure, &MaskStack::default(), raw);
    let pipeline = RawGpuPipeline::new(
        device,
        queue,
        raw,
        &params,
        PipelineOptions::new(ProcessingQuality::High),
    )?;
    pipeline.render_camera_scene_blocking(device, queue, &params)
}

/// Pixels at least `margin` from the edges, by index into an RGB scene.
fn interior(margin: u32) -> impl Iterator<Item = usize> {
    (margin..HEIGHT - margin)
        .flat_map(move |y| (margin..WIDTH - margin).map(move |x| ((y * WIDTH + x) * 3) as usize))
}

/// Mean |log2(R/G)| + |log2(B/G)| over `pixels`: colour in a neutral subject.
fn mean_chroma(scene: &[f32], pixels: impl Iterator<Item = usize>) -> f32 {
    let ratio =
        |value: f32, green: f32| ((value.max(0.0) + 0.002) / (green.max(0.0) + 0.002)).log2();
    let (sum, count) = pixels.fold((0.0, 0.0), |(sum, count), index| {
        let [r, g, b] = [scene[index], scene[index + 1], scene[index + 2]];
        (sum + ratio(r, g).abs() + ratio(b, g).abs(), count + 1.0)
    });
    sum / count
}

#[test]
fn lateral_aberration_is_measured_and_moved_back_onto_green() -> anyhow::Result<()> {
    let Some((device, queue, _)) = request_test_device_with_info() else {
        eprintln!("Chromatic aberration check skipped: no headless wgpu adapter");
        return Ok(());
    };
    // A neutral subject whose red is magnified and blue shrunk about the
    // centre: red/cyan and blue/yellow fringes that grow toward the corners.
    let center = [(WIDTH - 1) as f32 * 0.5, (HEIGHT - 1) as f32 * 0.5];
    let scaled = |x: f32, y: f32, magnification: f32| {
        pattern(
            center[0] + (x - center[0]) * (1.0 + magnification),
            center[1] + (y - center[1]) * (1.0 + magnification),
        )
    };
    let mut raw = raster(|x, y| [scaled(x, y, 0.005), pattern(x, y), scaled(x, y, -0.004)]);
    raw.lateral_chromatic_aberration = LateralChromaticAberration::estimate(&raw);
    assert!(!raw.lateral_chromatic_aberration.is_none());

    let before = render_scene(&device, &queue, &raw, &exposure(0.0))?;
    let after = render_scene(&device, &queue, &raw, &exposure(100.0))?;
    let fringes = |scene: &[f32]| mean_chroma(scene, interior(24));
    assert!(
        fringes(&after) < 0.35 * fringes(&before),
        "fringes {} -> {}",
        fringes(&before),
        fringes(&after)
    );
    // Half the amount moves the channels half way.
    let half = render_scene(&device, &queue, &raw, &exposure(50.0))?;
    assert!(fringes(&half) < fringes(&before) && fringes(&half) > fringes(&after));
    Ok(())
}

/// Dark vertical bars on a bright, slightly blue background, white-balanced.
/// Green is sharp; red and blue spread wider, as axial aberration images
/// them, and tint the dark side of every edge purple.
fn axial_bars(x: f32) -> [f32; 3] {
    let bars = |softness: f32| {
        let phase = (x / 60.0).fract() * 60.0;
        let edge = |at: f32| 1.0 / (1.0 + (-(phase - at) / softness).exp());
        edge(20.0) - edge(40.0)
    };
    let mix = |bar: f32, dark: f32, light: f32| light + (dark - light) * bar;
    [
        mix(bars(1.2), 0.03, 0.62),
        mix(bars(0.4), 0.03, 0.65),
        mix(bars(1.2), 0.03, 0.75),
    ]
}

fn bar_phase(index: usize) -> f32 {
    let x = ((index / 3) as u32 % WIDTH) as f32;
    (x / 60.0).fract() * 60.0
}

/// Pixels within the bars, near their edges, where the purple lies.
fn bar_fringes() -> impl Iterator<Item = usize> {
    interior(16).filter(|index| {
        let phase = bar_phase(*index);
        (21.0..=26.0).contains(&phase) || (34.0..=39.0).contains(&phase)
    })
}

/// Background pixels farther from any edge than the fringe window reaches.
fn bar_background() -> impl Iterator<Item = usize> {
    interior(16).filter(|index| bar_phase(*index) > 52.0)
}

#[test]
fn axial_fringes_are_desaturated_and_flat_colour_is_kept() -> anyhow::Result<()> {
    let Some((device, queue, _)) = request_test_device_with_info() else {
        eprintln!("Chromatic aberration check skipped: no headless wgpu adapter");
        return Ok(());
    };
    let raw = raster(|x, _| axial_bars(x));
    assert!(LateralChromaticAberration::estimate(&raw).is_none());

    let before = render_scene(&device, &queue, &raw, &exposure(0.0))?;
    let after = render_scene(&device, &queue, &raw, &exposure(100.0))?;
    let removed = 1.0 - mean_chroma(&after, bar_fringes()) / mean_chroma(&before, bar_fringes());
    assert!(removed > 0.5, "removed {removed} of the fringe colour");
    // The background keeps its blue.
    for index in bar_background() {
        for channel in 0..3 {
            let (old, new) = (before[index + channel], after[index + channel]);
            assert!((old - new).abs() < 1e-3, "background moved: {old} -> {new}");
        }
    }
    Ok(())
}

/// A camera-linear raster (LinearRaw DNG) holds camera RGB before white
/// balance. Fringes are told from neutral after balancing it, so they lose
/// their colour rather than take on the camera's.
#[test]
fn camera_linear_fringes_are_judged_after_white_balance() -> anyhow::Result<()> {
    let Some((device, queue, _)) = request_test_device_with_info() else {
        eprintln!("Chromatic aberration check skipped: no headless wgpu adapter");
        return Ok(());
    };
    let gains = [2.0, 1.0, 1.5];
    let mut raw = raster(|x, _| {
        let balanced = axial_bars(x);
        std::array::from_fn(|channel| balanced[channel] / gains[channel])
    });
    raw.wb_coeffs = [gains[0], gains[1], gains[2], gains[1]];
    std::sync::Arc::make_mut(&mut raw.profile_color).white_balance_model =
        Some(CameraWhiteBalanceModel {
            base_wb: [gains[0], gains[1], gains[2], gains[1]],
            cdesc: *b"RGBG",
            base_cct: 5_000.0,
            color: CameraColorModel::Matrix {
                xyz_to_camera: [
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0],
                    [0.0, 1.0, 0.0],
                ],
            },
        });
    assert!(raw.is_camera_linear_raster());
    let balance = |scene: Vec<f32>| -> Vec<f32> {
        scene
            .iter()
            .enumerate()
            .map(|(index, value)| value * gains[index % 3])
            .collect()
    };

    let before = balance(render_scene(&device, &queue, &raw, &exposure(0.0))?);
    let after = balance(render_scene(&device, &queue, &raw, &exposure(100.0))?);
    let removed = 1.0 - mean_chroma(&after, bar_fringes()) / mean_chroma(&before, bar_fringes());
    assert!(removed > 0.5, "removed {removed} of the fringe colour");
    for index in bar_background() {
        for channel in 0..3 {
            let (old, new) = (before[index + channel], after[index + channel]);
            assert!((old - new).abs() < 2e-3, "background moved: {old} -> {new}");
        }
    }
    Ok(())
}

#[test]
fn aligned_colour_and_warm_hues_pass_unchanged() -> anyhow::Result<()> {
    let Some((device, queue, _)) = request_test_device_with_info() else {
        eprintln!("Chromatic aberration check skipped: no headless wgpu adapter");
        return Ok(());
    };
    // Sharp, aligned edges between saturated blue, yellow, orange and white,
    // like a lit sign: no channel is displaced or blurrier than green.
    let colours = [
        [0.05, 0.12, 0.70],
        [0.70, 0.62, 0.08],
        [0.85, 0.30, 0.05],
        [0.80, 0.80, 0.80],
    ];
    let raw = raster(|x, y| {
        let column = ((x / 23.0) as usize + (y / 31.0) as usize * 3) % colours.len();
        colours[column]
    });
    assert!(LateralChromaticAberration::estimate(&raw).is_none());
    let before = render_scene(&device, &queue, &raw, &exposure(0.0))?;
    let after = render_scene(&device, &queue, &raw, &exposure(100.0))?;
    let largest = interior(16)
        .flat_map(|index| (0..3).map(move |channel| index + channel))
        .map(|index| (before[index] - after[index]).abs())
        .fold(0.0f32, f32::max);
    assert!(largest < 0.01, "aligned colour moved by {largest}");
    Ok(())
}

/// X-Trans AI denoise replaces demosaicing with its own scene. The RAW stage
/// starts from it every time, so the correction never compounds and an
/// amount of 0 shows the AI scene exactly.
#[test]
fn ai_denoised_scenes_are_corrected_from_the_stored_result() -> anyhow::Result<()> {
    let Some((device, queue, _)) = request_test_device_with_info() else {
        eprintln!("Chromatic aberration check skipped: no headless wgpu adapter");
        return Ok(());
    };
    const XTRANS: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];
    let base = raster(|_, _| [0.5; 3]);
    let raw = LoadedRaw {
        cfa_kind: CfaKind::XTrans,
        white_levels: [16_383.0; 4],
        ..base.derive_with(
            WIDTH,
            HEIGHT,
            vec![4_000; (WIDTH * HEIGHT) as usize],
            CompactPixelMap::repeating(WIDTH, HEIGHT, 6, 6, XTRANS.concat()),
            CompactPixelMap::repeating(WIDTH, HEIGHT, 1, 1, vec![0.0]),
        )
    };
    // The AI scene: purple fringes on a soft-edged pattern.
    let mut scene = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (x, y) = (x as f32, y as f32);
            let blurred = (pattern(x - 1.0, y) + pattern(x + 1.0, y) + pattern(x, y)) / 3.0;
            scene.extend(
                [blurred, pattern(x, y), blurred].map(|value| half::f16::from_f32(value).to_bits()),
            );
        }
    }
    raw.set_ai_denoised_image(AiDenoisedImage::new(WIDTH, HEIGHT, scene.clone())?)?;

    let ai = |amount: f32| ExposureParams {
        ai_denoise_enabled: true,
        ..exposure(amount)
    };
    let params = |amount: f32| GpuParams::new(&ai(amount), &MaskStack::default(), &raw);
    let pipeline = RawGpuPipeline::new(
        &device,
        &queue,
        &raw,
        &params(100.0),
        PipelineOptions::new(ProcessingQuality::High),
    )?;
    let first = pipeline.render_camera_scene_blocking(&device, &queue, &params(100.0))?;
    let second = pipeline.render_camera_scene_blocking(&device, &queue, &params(100.0))?;
    assert_eq!(first, second, "the correction compounded");
    let uncorrected = pipeline.render_camera_scene_blocking(&device, &queue, &params(0.0))?;
    let stored: Vec<f32> = scene
        .iter()
        .map(|bits| half::f16::from_bits(*bits).to_f32())
        .collect();
    assert_eq!(uncorrected, stored);
    assert_ne!(first, uncorrected);
    assert!(pipeline.immutable_ai_source_matches(CfaKind::XTrans, true));
    Ok(())
}
