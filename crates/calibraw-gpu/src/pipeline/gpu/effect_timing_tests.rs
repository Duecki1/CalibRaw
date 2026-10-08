//! GPU timing of the depth-guided, light-receiving and masked effects, to
//! compare shader changes on one machine. Run explicitly:
//!
//! ```sh
//! cargo test -p calibraw-gpu --lib effect_timing -- --ignored --nocapture
//! ```
//!
//! With `CALIBRAW_EFFECT_TIMING_DIR` set, each case's linear output is kept
//! there as raw little-endian f32 RGB. A later run compares against the kept
//! output and prints the largest and mean difference, so an optimization can
//! show that it leaves the image unchanged.

use super::fog_tests::{neutral_exposure, FogScene, MASK_EDGE};
use super::{GpuParams, ProcessingQuality};
use crate::pipeline::{
    EffectComponent, LoadedRaw, LocalMask, MaskEffect, MaskImage, MaskKind, MaskStack,
    ProcessingStage,
};
use std::time::{Duration, Instant};

/// A fitted preview of a 3:2 photo.
const WIDTH: u32 = 2048;
const HEIGHT: u32 = 1366;
const DEPTH_WIDTH: u32 = 512;
const DEPTH_HEIGHT: u32 = 342;
const WARMUP_RUNS: usize = 2;
const TIMED_RUNS: usize = 12;

fn hash(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65_535.0
}

/// Whether normalized `x` lies on one of the near vertical posts.
fn on_post(x: f32) -> bool {
    [0.22, 0.47, 0.71]
        .iter()
        .any(|centre| (x - centre).abs() < 0.035)
}

/// Sky above a textured ground plane, dark posts in front and a few lamps.
fn photo() -> anyhow::Result<LoadedRaw> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    let lamps = [(0.3, 0.55), (0.62, 0.48), (0.85, 0.6)];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (u, v) = (x as f32 / WIDTH as f32, y as f32 / HEIGHT as f32);
            let mut rgb = if v < 0.4 {
                [0.35 + 0.3 * v, 0.45 + 0.3 * v, 0.7]
            } else {
                let grain = 0.6 + 0.8 * hash(x / 3, y / 3);
                [0.12 * grain, 0.1 * grain, 0.07 * grain]
            };
            if on_post(u) && v > 0.25 {
                rgb = [0.02, 0.02, 0.025];
            }
            for (lx, ly) in lamps {
                let distance = ((u - lx) * 1.5).hypot(v - ly);
                if distance < 0.008 {
                    rgb = [9.0, 7.0, 4.5];
                }
            }
            pixels.extend_from_slice(&rgb);
        }
    }
    LoadedRaw::from_scene_linear_rec2020(WIDTH, HEIGHT, pixels)
}

/// Far sky, a ground plane receding to the horizon and near posts. Every case
/// shares one depth result, as edits of one photo do, so the relighting
/// surface is derived once.
fn depth() -> MaskImage {
    static DEPTH: std::sync::OnceLock<MaskImage> = std::sync::OnceLock::new();
    DEPTH.get_or_init(depth_result).clone()
}

fn depth_result() -> MaskImage {
    let pixels = (0..DEPTH_WIDTH * DEPTH_HEIGHT)
        .map(|i| {
            let u = (i % DEPTH_WIDTH) as f32 / DEPTH_WIDTH as f32;
            let v = (i / DEPTH_WIDTH) as f32 / DEPTH_HEIGHT as f32;
            let depth = if on_post(u) && v > 0.25 {
                0.12
            } else if v < 0.4 {
                0.99
            } else {
                0.85 - 0.75 * (v - 0.4) / 0.6
            };
            (depth * 255.0).round() as u8
        })
        .collect();
    MaskImage::new(DEPTH_WIDTH, DEPTH_HEIGHT, pixels).expect("depth dimensions")
}

fn component(effect: MaskEffect, edit: impl FnOnce(&mut EffectComponent)) -> EffectComponent {
    let mut component = EffectComponent::new(effect);
    edit(&mut component);
    component
}

fn relight(source: [f32; 2], shadows: bool) -> EffectComponent {
    component(MaskEffect::Relight, |c| {
        c.settings.relight.source = source;
        c.settings.relight.shadows_enabled = shadows;
        c.settings.relight.ambient = 70.0;
    })
}

fn fog(light_glow: f32, image_lights: bool) -> EffectComponent {
    component(MaskEffect::Fog, |c| {
        c.settings.fog.variation = 50.0;
        c.settings.fog.light_glow = light_glow;
        c.settings.fog.image_lights = image_lights;
    })
}

fn global(effects: Vec<EffectComponent>) -> MaskStack {
    MaskStack {
        global_effects: effects,
        scene_depth: Some(depth()),
        ..Default::default()
    }
}

/// One mask with a radial coverage falloff carrying `effects`.
fn masked(effects: Vec<EffectComponent>) -> MaskStack {
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components = effects;
    MaskStack {
        masks: vec![mask],
        scene_depth: Some(depth()),
        ..Default::default()
    }
}

fn radial_coverage() -> Vec<u16> {
    let edge = MASK_EDGE as f32;
    (0..MASK_EDGE * MASK_EDGE)
        .map(|i| {
            let x = (i % MASK_EDGE) as f32 / edge - 0.5;
            let y = (i / MASK_EDGE) as f32 / edge - 0.5;
            let coverage = (1.0 - (x.hypot(y) - 0.15) / 0.25).clamp(0.0, 1.0);
            half::f16::from_f32(coverage).to_bits()
        })
        .collect()
}

/// Each case renders its stack, and while timed alternates with the second
/// stack when there is one, as dragging a control does.
fn cases() -> Vec<(&'static str, MaskStack, Option<MaskStack>)> {
    let lights = [
        [30.0, 35.0],
        [70.0, 40.0],
        [50.0, 20.0],
        [15.0, 60.0],
        [85.0, 30.0],
    ];
    let dragged = [lights[0][0] + 0.5, lights[0][1]];
    let fixed = |name, masks| (name, masks, None);
    vec![
        fixed("baseline", global(Vec::new())),
        fixed("fog_depth", global(vec![fog(0.0, false)])),
        fixed(
            "fog_relight_glow",
            global(vec![fog(60.0, false), relight(lights[0], false)]),
        ),
        fixed("fog_image_lights", global(vec![fog(60.0, true)])),
        fixed(
            "smoke_light_rays",
            global(vec![
                component(MaskEffect::Smoke, |c| c.settings.smoke.light_glow = 60.0),
                component(MaskEffect::LightRays, |c| {
                    c.settings.light_rays.source = [60.0, 30.0];
                }),
            ]),
        ),
        fixed(
            "relight_no_shadows",
            global(vec![relight(lights[0], false)]),
        ),
        fixed("relight_shadow_map", global(vec![relight(lights[0], true)])),
        (
            "relight_shadow_drag",
            global(vec![relight(lights[0], true)]),
            Some(global(vec![relight(dragged, true)])),
        ),
        fixed(
            "relight_5_shadowed",
            global(lights.iter().map(|&source| relight(source, true)).collect()),
        ),
        fixed(
            "masked_blur",
            masked(vec![component(MaskEffect::Blur, |c| {
                c.settings.blur.amount = 80.0;
                c.settings.blur.radius = 60.0;
            })]),
        ),
    ]
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Compares `rgb` with the output kept for `name`, or keeps it.
fn compare_or_keep(name: &str, rgb: &[f32]) -> anyhow::Result<String> {
    let Some(dir) = std::env::var_os("CALIBRAW_EFFECT_TIMING_DIR") else {
        return Ok(String::new());
    };
    let path = std::path::Path::new(&dir).join(format!("{name}.f32"));
    let Ok(kept) = std::fs::read(&path) else {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytemuck::cast_slice(rgb))?;
        return Ok("kept".into());
    };
    let kept: &[f32] = bytemuck::try_cast_slice(&kept)
        .map_err(|error| anyhow::anyhow!("{}: {error:?}", path.display()))?;
    anyhow::ensure!(kept.len() == rgb.len(), "{}: size changed", path.display());
    let (mut largest, mut at, mut sum) = (0.0_f32, 0, 0.0_f64);
    let mut differing = 0;
    for (index, (a, b)) in kept.iter().zip(rgb).enumerate() {
        let difference = (a - b).abs();
        if difference > largest {
            largest = difference;
            at = index / 3;
        }
        differing += usize::from(difference > 1e-3);
        sum += f64::from(difference);
    }
    Ok(format!(
        "max diff {largest:.5} at ({}, {}), mean diff {:.6}, {differing} values off by > 0.001",
        at as u32 % WIDTH,
        at as u32 / WIDTH,
        sum / rgb.len() as f64
    ))
}

#[test]
#[ignore = "GPU timing; run explicitly with --ignored --nocapture"]
fn effect_timing() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(photo()?, ProcessingQuality::Preview)? else {
        return Ok(());
    };
    scene
        .pipeline
        .update_mask_layer(&scene.queue, 0, &radial_coverage())?;
    let exposure = neutral_exposure();
    for (name, masks, alternate) in cases() {
        let params = GpuParams::new(&exposure, &masks, &scene.source);
        let alternate = alternate.map(|masks| GpuParams::new(&exposure, &masks, &scene.source));
        let rgb = scene.render_params(&params)?;
        let comparison = compare_or_keep(name, &rgb)?;
        let mut samples = Vec::with_capacity(TIMED_RUNS);
        for run in 0..WARMUP_RUNS + TIMED_RUNS {
            let params = match &alternate {
                Some(alternate) if run % 2 == 1 => alternate,
                _ => &params,
            };
            let started = Instant::now();
            scene.pipeline.dispatch_stage(
                &scene.queue,
                &scene.device,
                params,
                ProcessingStage::Output,
            );
            scene.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })?;
            if run >= WARMUP_RUNS {
                samples.push(started.elapsed());
            }
        }
        println!(
            "{name:>20}: output stage {:7.2} ms  {comparison}",
            median(samples).as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
