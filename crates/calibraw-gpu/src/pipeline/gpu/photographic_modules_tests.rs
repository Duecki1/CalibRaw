use super::{tests::request_test_device, GpuParams, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{
    extract_padded_tile, EffectComponent, ExportTile, ExposureParams, LoadedRaw, LocalMask,
    MaskEffect, MaskKind, MaskStack, ProcessingStage,
};

const MASK_EDGE: u32 = 64;
// Match the existing float-readback regressions: allow normal GPU arithmetic
// differences, while keeping the tolerance well below a visible effect.
const RGB_TOLERANCE: f32 = 2.0e-4;
const FILM_EDGE: u32 = 2160;

struct PhotoScene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    source: LoadedRaw,
    exposure: ExposureParams,
    pipeline: RawGpuPipeline,
}

impl PhotoScene {
    fn new(source: LoadedRaw) -> anyhow::Result<Option<Self>> {
        let Some((device, queue)) = request_test_device() else {
            eprintln!("photographic module GPU regression skipped: no headless wgpu adapter");
            return Ok(None);
        };
        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            ..Default::default()
        };
        // Reserve an atlas layer before switching between global and local modules.
        let initial = local(EffectComponent::new(MaskEffect::Grain));
        let pipeline = RawGpuPipeline::new_headless_with_quality_and_mask_edge(
            &device,
            &queue,
            &source,
            &GpuParams::new(&exposure, &initial, &source),
            ProcessingQuality::High,
            MASK_EDGE,
        )?;
        Ok(Some(Self {
            device,
            queue,
            source,
            exposure,
            pipeline,
        }))
    }

    fn render(&self, masks: &MaskStack) -> anyhow::Result<Vec<f32>> {
        self.render_params(&GpuParams::new(&self.exposure, masks, &self.source))
    }

    fn render_film(&self, masks: &MaskStack, x: i32, y: i32) -> anyhow::Result<Vec<f32>> {
        // Small preview dimensions deliberately attenuate grain. This fixture is
        // a crop from a film-sized frame, so seed and size tests see resolved grain.
        self.render_params(&GpuParams::new_for_tile(
            &self.exposure,
            masks,
            &self.source,
            x,
            y,
            FILM_EDGE,
            FILM_EDGE,
        ))
    }

    fn render_params(&self, params: &GpuParams) -> anyhow::Result<Vec<f32>> {
        self.pipeline.recompute(&self.queue, &self.device, params);
        let rgb = self.pipeline.read_display_linear_region_blocking(
            &self.device,
            &self.queue,
            0,
            0,
            self.source.width,
            self.source.height,
        )?;
        assert_eq!(
            rgb.len(),
            (self.source.width * self.source.height * 3) as usize
        );
        assert!(
            rgb.iter().all(|v| v.is_finite()),
            "non-finite photographic RGB"
        );
        Ok(rgb)
    }

    fn coverage(&self, pixels: &[u16]) -> anyhow::Result<()> {
        self.pipeline.update_mask_layer(&self.queue, 0, pixels)
    }
}

fn flat(width: u32, height: u32, level: f32) -> anyhow::Result<LoadedRaw> {
    LoadedRaw::from_scene_linear_rec2020(width, height, vec![level; (width * height * 3) as usize])
}

fn bright_bar(width: u32, height: u32) -> anyhow::Result<LoadedRaw> {
    let pixels = (0..width * height)
        .flat_map(|i| {
            let x = i % width;
            [if (width * 3 / 8..width * 5 / 8).contains(&x) {
                0.8
            } else {
                0.03
            }; 3]
        })
        .collect();
    LoadedRaw::from_scene_linear_rec2020(width, height, pixels)
}

fn global(component: EffectComponent) -> MaskStack {
    MaskStack {
        global_effects: vec![component],
        ..Default::default()
    }
}

fn local(component: EffectComponent) -> MaskStack {
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components.push(component);
    MaskStack {
        masks: vec![mask],
        ..Default::default()
    }
}

fn grain(amount: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Grain);
    component.settings.grain.amount = amount;
    component
}

fn halation(amount: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Halation);
    component.settings.halation.amount = amount;
    component
}

fn vignette(amount: f32) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Vignette);
    component.settings.vignette.amount = amount;
    component
}

fn set_amount(component: &mut EffectComponent, amount: f32) {
    match component.effect {
        MaskEffect::Grain => component.settings.grain.amount = amount,
        MaskEffect::Halation => component.settings.halation.amount = amount,
        MaskEffect::Vignette => component.settings.vignette.amount = amount,
        _ => panic!("unexpected photographic component"),
    }
}

fn assert_close(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context}");
    let (index, error) = actual
        .iter()
        .zip(expected)
        .enumerate()
        .map(|(i, (a, b))| {
            assert!(
                a.is_finite() && b.is_finite(),
                "{context}: non-finite channel {i}"
            );
            (i, (a - b).abs())
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("nonempty RGB fixture");
    assert!(
        error <= RGB_TOLERANCE,
        "{context}: max error {error:e} at channel {index}: {} vs {}",
        actual[index],
        expected[index]
    );
}

fn rms_difference(actual: &[f32], expected: &[f32]) -> f32 {
    assert_eq!(actual.len(), expected.len());
    (actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        / actual.len() as f32)
        .sqrt()
}

fn patch_mean(rgb: &[f32], width: u32, x: u32, y: u32, w: u32, h: u32) -> [f32; 3] {
    let mut sum = [0.0; 3];
    for row in y..y + h {
        for column in x..x + w {
            let index = ((row * width + column) * 3) as usize;
            for c in 0..3 {
                sum[c] += rgb[index + c];
            }
        }
    }
    sum.map(|v| v / (w * h) as f32)
}

fn patch_delta(actual: &[f32], baseline: &[f32], width: u32, rect: [u32; 4]) -> [f32; 3] {
    let [x, y, w, h] = rect;
    let a = patch_mean(actual, width, x, y, w, h);
    let b = patch_mean(baseline, width, x, y, w, h);
    std::array::from_fn(|c| a[c] - b[c])
}

fn neighbor_rms(rgb: &[f32], width: u32, height: u32) -> f32 {
    let mut squared = 0.0;
    let mut count = 0;
    for y in 0..height - 1 {
        for x in 0..width - 1 {
            let i = ((y * width + x) * 3) as usize;
            squared += (rgb[i] - rgb[i + 3]).powi(2);
            squared += (rgb[i] - rgb[i + (width * 3) as usize]).powi(2);
            count += 2;
        }
    }
    (squared / count as f32).sqrt()
}

#[test]
fn photographic_grain_is_deterministic_and_seed_and_size_change_texture() -> anyhow::Result<()> {
    const W: u32 = 256;
    const H: u32 = 128;
    let Some(scene) = PhotoScene::new(flat(W, H, 0.18)?)? else {
        return Ok(());
    };
    let baseline = scene.render_film(&MaskStack::default(), 0, 0)?;
    let mut masks = global(grain(85.0));
    let original = scene.render_film(&masks, 0, 0)?;
    let signal = rms_difference(&original, &baseline);
    assert!(
        signal > 0.001,
        "grain fixture has no resolved texture: {signal}"
    );
    assert_eq!(
        original,
        scene.render_film(&masks, 0, 0)?,
        "grain must not flicker"
    );

    masks.global_effects[0].settings.grain.seed = 137.0;
    let reseeded = scene.render_film(&masks, 0, 0)?;
    assert!(
        rms_difference(&original, &reseeded) > signal * 0.25,
        "seed did not replace the grain pattern"
    );
    masks.global_effects[0].settings.grain.seed = 0.0;
    assert_eq!(
        original,
        scene.render_film(&masks, 0, 0)?,
        "restoring a seed must restore grain"
    );

    masks.global_effects[0].settings.grain.size = 0.5;
    let fine = scene.render_film(&masks, 0, 0)?;
    masks.global_effects[0].settings.grain.size = 4.0;
    let coarse = scene.render_film(&masks, 0, 0)?;
    assert!(
        rms_difference(&fine, &coarse) > signal * 0.25,
        "size did not change grain"
    );
    // Normalize by contrast: reducing amplitude alone is not larger grain.
    let fine_frequency = neighbor_rms(&fine, W, H) / rms_difference(&fine, &baseline);
    let coarse_frequency = neighbor_rms(&coarse, W, H) / rms_difference(&coarse, &baseline);
    assert!(
        coarse_frequency < fine_frequency * 0.8,
        "larger grain must be more correlated: fine={fine_frequency}, coarse={coarse_frequency}"
    );
    Ok(())
}

#[test]
fn photographic_grain_amount_is_monotonic_finite_and_monochrome() -> anyhow::Result<()> {
    const W: u32 = 192;
    const H: u32 = 192;
    // Include black and bright flat fields to exercise the noise envelope's ends.
    let pixels = (0..W * H)
        .flat_map(|i| [[0.0, 0.18, 0.8][(i / W / 64) as usize]; 3])
        .collect();
    let Some(scene) = PhotoScene::new(LoadedRaw::from_scene_linear_rec2020(W, H, pixels)?)? else {
        return Ok(());
    };
    let baseline = scene.render_film(&MaskStack::default(), 0, 0)?;
    let mut previous = 0.0;
    for amount in [25.0, 50.0, 100.0] {
        let rgb = scene.render_film(&global(grain(amount)), 0, 0)?;
        let signal = rms_difference(&rgb, &baseline);
        assert!(
            signal > previous + RGB_TOLERANCE,
            "amount={amount} did not increase grain: previous={previous}, actual={signal}"
        );
        for pixel in rgb.chunks_exact(3) {
            assert!(pixel.iter().all(|v| v.is_finite() && *v >= 0.0));
            assert!(
                (pixel[0] - pixel[1]).abs() < RGB_TOLERANCE
                    && (pixel[1] - pixel[2]).abs() < RGB_TOLERANCE,
                "monochrome grain introduced a color cast: {pixel:?}"
            );
        }
        let middle = (W * 64 * 3) as usize..(W * 128 * 3) as usize;
        let residuals: Vec<_> = rgb[middle.clone()]
            .iter()
            .zip(&baseline[middle])
            .map(|(a, b)| a - b)
            .collect();
        assert!(
            residuals.iter().any(|v| *v > RGB_TOLERANCE)
                && residuals.iter().any(|v| *v < -RGB_TOLERANCE),
            "grain must vary around the flat field, not just shift exposure"
        );
        previous = signal;
    }
    Ok(())
}

#[test]
fn photographic_modules_disabled_and_zero_amount_bypass_exactly_after_active_render(
) -> anyhow::Result<()> {
    let Some(scene) = PhotoScene::new(bright_bar(256, 128)?)? else {
        return Ok(());
    };
    scene.coverage(&vec![
        half::f16::ONE.to_bits();
        (MASK_EDGE * MASK_EDGE) as usize
    ])?;
    let baseline = scene.render_film(&MaskStack::default(), 0, 0)?;
    for component in [grain(100.0), halation(100.0), vignette(-80.0)] {
        let name = format!("{:?}", component.effect);
        for is_local in [false, true] {
            let active = if is_local {
                local(component.clone())
            } else {
                global(component.clone())
            };
            let rendered = scene.render_film(&active, 0, 0)?;
            assert!(
                rms_difference(&rendered, &baseline) > RGB_TOLERANCE,
                "{name}, local={is_local}: bypass fixture must first be active"
            );
            let mut disabled = active.clone();
            if is_local {
                disabled.masks[0].effect_components[0].enabled = false;
            } else {
                disabled.global_effects[0].enabled = false;
            }
            assert_eq!(
                scene.render_film(&disabled, 0, 0)?,
                baseline,
                "{name}, local={is_local}: disabled component must bypass exactly"
            );

            scene.render_film(&active, 0, 0)?;
            let mut zero = active.clone();
            if is_local {
                set_amount(&mut zero.masks[0].effect_components[0], 0.0);
            } else {
                set_amount(&mut zero.global_effects[0], 0.0);
            }
            assert_eq!(
                scene.render_film(&zero, 0, 0)?,
                baseline,
                "{name}, local={is_local}: amount zero must bypass stale results exactly"
            );
            if is_local {
                scene.render_film(&active, 0, 0)?;
                let mut disabled_mask = active.clone();
                disabled_mask.masks[0].enabled = false;
                assert_eq!(
                    scene.render_film(&disabled_mask, 0, 0)?,
                    baseline,
                    "{name}: disabled parent mask must bypass exactly"
                );
            }
            scene.render_film(&active, 0, 0)?;
            assert_eq!(
                scene.render_film(&MaskStack::default(), 0, 0)?,
                baseline,
                "{name}: removing the module must restore the original exactly"
            );
        }
    }
    Ok(())
}

#[test]
fn photographic_halation_preserves_uniform_fields_including_highlights() -> anyhow::Result<()> {
    for level in [0.03, 0.8] {
        let Some(scene) = PhotoScene::new(flat(96, 64, level)?)? else {
            return Ok(());
        };
        let baseline = scene.render(&MaskStack::default())?;
        for (radius, threshold) in [(4.0, 0.0), (32.0, 20.0), (8.0, 90.0)] {
            let mut component = halation(100.0);
            component.settings.halation.radius = radius;
            component.settings.halation.threshold = threshold;
            component.settings.halation.warmth = 100.0;
            let rgb = scene.render(&global(component))?;
            assert_close(&rgb, &baseline,
                &format!("uniform field {level}, radius={radius}, threshold={threshold} acquired halation"));
        }
    }
    Ok(())
}

#[test]
fn photographic_halation_has_warm_edges_and_independent_radius_and_threshold_controls(
) -> anyhow::Result<()> {
    const W: u32 = 256;
    const H: u32 = 128;
    let Some(scene) = PhotoScene::new(bright_bar(W, H)?)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let mut component = halation(100.0);
    component.settings.halation.warmth = 100.0;
    component.settings.halation.threshold = 20.0;
    component.settings.halation.radius = 4.0;
    let tight = scene.render(&global(component.clone()))?;
    let edge = patch_delta(&tight, &baseline, W, [94, 32, 2, 64]);
    assert!(
        edge[0] > 0.001 && edge[0] > edge[1] && edge[1] > edge[2],
        "ordinary bright detail should produce a warm dark-side halo: {edge:?}"
    );
    for x in [16, 124, 236] {
        assert_close(
            &patch_mean(&tight, W, x, 32, 4, 64),
            &patch_mean(&baseline, W, x, 32, 4, 64),
            "halation changed a distant flat field or bright core",
        );
    }

    component.settings.halation.radius = 20.0;
    let wide = scene.render(&global(component.clone()))?;
    // At this preview size, radius 20 has nine pixels of finite support.
    // Sample outside the tight halo but inside that support, on the dark side.
    let tight_far = patch_delta(&tight, &baseline, W, [90, 32, 4, 64])[0];
    let wide_far = patch_delta(&wide, &baseline, W, [90, 32, 4, 64])[0];
    assert!(
        wide_far > tight_far + RGB_TOLERANCE,
        "larger radius must extend the halo: tight={tight_far}, wide={wide_far}"
    );

    component.settings.halation.threshold = 90.0;
    let high_threshold = scene.render(&global(component.clone()))?;
    let low_energy = patch_delta(&wide, &baseline, W, [80, 32, 16, 64])[0];
    let high_energy = patch_delta(&high_threshold, &baseline, W, [80, 32, 16, 64])[0];
    assert!(
        low_energy > RGB_TOLERANCE && high_energy < low_energy * 0.8,
        "raising threshold must reject more highlight energy: low={low_energy}, high={high_energy}"
    );

    component.settings.halation.threshold = 20.0;
    component.settings.halation.amount = 30.0;
    let weaker = scene.render(&global(component))?;
    let weaker_energy = patch_delta(&weaker, &baseline, W, [80, 32, 16, 64])[0];
    assert!(weaker_energy > RGB_TOLERANCE && weaker_energy < low_energy,
        "halation amount must increase halo energy: amount30={weaker_energy}, amount100={low_energy}");
    Ok(())
}

#[test]
fn photographic_vignette_protects_center_and_amount_controls_both_signs() -> anyhow::Result<()> {
    const W: u32 = 192;
    const H: u32 = 128;
    let Some(scene) = PhotoScene::new(flat(W, H, 0.25)?)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    for sign in [-1.0, 1.0] {
        let mut previous = 0.0;
        for strength in [25.0, 60.0, 100.0] {
            let mut component = vignette(sign * strength);
            component.settings.vignette.highlights = 0.0;
            let rgb = scene.render(&global(component))?;
            assert_close(
                &patch_mean(&rgb, W, W / 2 - 2, H / 2 - 2, 4, 4),
                &patch_mean(&baseline, W, W / 2 - 2, H / 2 - 2, 4, 4),
                "vignette must protect its center",
            );
            let corner = sign * patch_delta(&rgb, &baseline, W, [0, 0, 8, 8])[0];
            assert!(corner > previous + 0.001,
                "amount={} must strengthen the signed corner effect: previous={previous}, actual={corner}", sign * strength);
            assert!(
                rgb.iter()
                    .zip(&baseline)
                    .all(|(a, b)| sign * (a - b) >= -RGB_TOLERANCE),
                "amount={} changed pixels in the wrong direction",
                sign * strength
            );
            previous = corner;
        }
    }
    Ok(())
}

#[test]
fn photographic_vignette_center_moves_in_both_axes() -> anyhow::Result<()> {
    const W: u32 = 192;
    const H: u32 = 128;
    let Some(scene) = PhotoScene::new(flat(W, H, 0.25)?)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let mut component = vignette(-85.0);
    component.settings.vignette.midpoint = 20.0;
    component.settings.vignette.feather = 40.0;
    component.settings.vignette.highlights = 0.0;
    for (center, protected, opposite) in [
        ([25.0, 25.0], [W / 4, H / 4], [3 * W / 4, 3 * H / 4]),
        ([75.0, 75.0], [3 * W / 4, 3 * H / 4], [W / 4, H / 4]),
    ] {
        component.settings.vignette.center = center;
        let rgb = scene.render(&global(component.clone()))?;
        assert_close(
            &patch_mean(&rgb, W, protected[0] - 1, protected[1] - 1, 2, 2),
            &patch_mean(&baseline, W, protected[0] - 1, protected[1] - 1, 2, 2),
            &format!("vignette failed to move its protected center to {center:?}"),
        );
        let delta = patch_delta(&rgb, &baseline, W, [opposite[0] - 1, opposite[1] - 1, 2, 2]);
        assert!(
            delta[0] < -0.005,
            "center={center:?}: opposite position was not darkened: {delta:?}"
        );
    }
    Ok(())
}

#[test]
fn photographic_vignette_highlights_control_protects_bright_corners() -> anyhow::Result<()> {
    const W: u32 = 192;
    const H: u32 = 128;
    let Some(scene) = PhotoScene::new(flat(W, H, 0.8)?)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let mut component = vignette(-80.0);
    component.settings.vignette.highlights = 0.0;
    let unprotected = scene.render(&global(component.clone()))?;
    component.settings.vignette.highlights = 100.0;
    let protected = scene.render(&global(component))?;
    let dark = patch_delta(&unprotected, &baseline, W, [0, 0, 8, 8])[0];
    let light = patch_delta(&protected, &baseline, W, [0, 0, 8, 8])[0];
    assert!(dark < -0.005 && light > dark + 0.001 && light <= RGB_TOLERANCE,
        "highlight protection must reduce darkening without brightening: unprotected={dark}, protected={light}");
    Ok(())
}

#[test]
fn photographic_modules_fullscreen_matches_global_and_mask_only_clips_contribution(
) -> anyhow::Result<()> {
    const W: u32 = 320;
    const H: u32 = 192;
    let Some(scene) = PhotoScene::new(bright_bar(W, H)?)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let full = vec![half::f16::ONE.to_bits(); (MASK_EDGE * MASK_EDGE) as usize];
    let empty = vec![0; full.len()];
    let split: Vec<_> = (0..MASK_EDGE * MASK_EDGE)
        .map(|i| {
            if i % MASK_EDGE < MASK_EDGE / 2 {
                0
            } else {
                half::f16::ONE.to_bits()
            }
        })
        .collect();
    for component in [grain(100.0), halation(100.0), vignette(-80.0)] {
        let name = format!("{:?}", component.effect);
        let global_rgb = scene.render(&global(component.clone()))?;
        assert!(
            rms_difference(&global_rgb, &baseline) > RGB_TOLERANCE,
            "inactive {name} mask fixture"
        );
        let masks = local(component);
        scene.coverage(&full)?;
        assert_close(
            &scene.render(&masks)?,
            &global_rgb,
            &format!("{name}: fullscreen vs global"),
        );
        scene.coverage(&empty)?;
        assert_close(
            &scene.render(&masks)?,
            &baseline,
            &format!("{name}: empty mask"),
        );
        scene.coverage(&split)?;
        let clipped = scene.render(&masks)?;
        let mut affected_inside = 0;
        for y in 0..H {
            for x in 0..W {
                // Skip only the atlas's bilinear coverage transition.
                if x.abs_diff(W / 2) < 4 {
                    continue;
                }
                let i = ((y * W + x) * 3) as usize;
                let expected = if x < W / 2 { &baseline } else { &global_rgb };
                assert_close(
                    &clipped[i..i + 3],
                    &expected[i..i + 3],
                    &format!("{name}: coverage at {x},{y}"),
                );
                if x > W / 2 && (clipped[i] - baseline[i]).abs() > RGB_TOLERANCE {
                    affected_inside += 1;
                }
            }
        }
        assert!(
            affected_inside > 0,
            "{name}: mask fixture has no affected inside pixels"
        );
    }
    Ok(())
}

#[test]
fn photographic_grain_is_anchored_to_the_film_plane_across_overlapping_tiles() -> anyhow::Result<()>
{
    const W: u32 = 192;
    const H: u32 = 128;
    let Some(scene) = PhotoScene::new(flat(W, H, 0.18)?)? else {
        return Ok(());
    };
    let mut component = grain(100.0);
    component.settings.grain.size = 2.4;
    component.settings.grain.roughness = 70.0;
    component.settings.grain.color = 35.0;
    component.settings.grain.seed = 39.0;
    let masks = global(component);
    let baseline = scene.render_film(&MaskStack::default(), 0, 0)?;
    let reference = scene.render_film(&masks, 0, 0)?;
    assert!(
        rms_difference(&reference, &baseline) > 0.001,
        "inactive tiled grain fixture"
    );
    for (dx, dy) in [(13, 9), (-7, -5)] {
        let shifted = scene.render_film(&masks, dx, dy)?;
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                let tile_x = x - dx;
                let tile_y = y - dy;
                if !(0..W as i32).contains(&tile_x) || !(0..H as i32).contains(&tile_y) {
                    continue;
                }
                let a = ((y * W as i32 + x) * 3) as usize;
                let b = ((tile_y * W as i32 + tile_x) * 3) as usize;
                assert_close(
                    &shifted[b..b + 3],
                    &reference[a..a + 3],
                    &format!("grain moved at {x},{y} in tile {dx},{dy}"),
                );
            }
        }
    }
    Ok(())
}

#[test]
fn photographic_modules_match_full_frame_in_padded_export_tiles() -> anyhow::Result<()> {
    const W: u32 = 320;
    const H: u32 = 240;
    const HALO: u32 = 64;
    let Some(scene) = PhotoScene::new(bright_bar(W, H)?)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let mut offset_vignette = vignette(-80.0);
    offset_vignette.settings.vignette.center = [30.0, 65.0];
    offset_vignette.settings.vignette.midpoint = 25.0;
    let mut wide_halation = halation(100.0);
    wide_halation.settings.halation.radius = 24.0;
    wide_halation.settings.halation.threshold = 30.0;
    let mut large_grain = grain(100.0);
    large_grain.settings.grain.size = 3.0;
    large_grain.settings.grain.seed = 71.0;
    for component in [large_grain, wide_halation, offset_vignette] {
        let name = format!("{:?}", component.effect);
        let masks = global(component);
        let full = scene.render(&masks)?;
        assert!(
            rms_difference(&full, &baseline) > RGB_TOLERANCE,
            "inactive {name} export fixture"
        );
        for (x, y) in [(104, 88), (111, 93)] {
            let tile = ExportTile {
                core_x: x,
                core_y: y,
                core_width: 96,
                core_height: 64,
                local_core_x: HALO,
                local_core_y: HALO,
                padded_width: 96 + 2 * HALO,
                padded_height: 64 + 2 * HALO,
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
                W,
                H,
            );
            let pipeline = RawGpuPipeline::new_headless_reusing_programs_with_mask_edge(
                &scene.device,
                &scene.queue,
                &raw,
                &params,
                ProcessingQuality::High,
                &scene.pipeline,
                MASK_EDGE,
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
                tile.core_width,
                tile.core_height,
            )?;
            let expected: Vec<_> = (y..y + tile.core_height)
                .flat_map(|row| {
                    let start = ((row * W + x) * 3) as usize;
                    full[start..start + (tile.core_width * 3) as usize]
                        .iter()
                        .copied()
                })
                .collect();
            assert_close(
                &actual,
                &expected,
                &format!("{name} export tile at {x},{y}"),
            );
        }
    }
    Ok(())
}
