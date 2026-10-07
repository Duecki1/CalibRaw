use super::fog_tests::{
    assert_close, mean_difference, neutral_exposure, FogScene, MASK_EDGE, RGB_TOLERANCE,
};
use super::{GpuParams, PipelineOptions, ProcessingQuality, RawGpuPipeline};
use crate::pipeline::{
    extract_padded_tile, EffectComponent, ExportTile, LoadedRaw, LocalMask, MaskEffect, MaskImage,
    MaskKind, MaskStack, ProcessingStage, RelightEffectSettings,
};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;

fn grey(width: u32, height: u32) -> anyhow::Result<LoadedRaw> {
    LoadedRaw::from_scene_linear_rec2020(width, height, vec![0.18; (width * height * 3) as usize])
}

fn depth_map(width: u32, height: u32, depth: impl Fn(f32, f32) -> f32) -> MaskImage {
    let pixels = (0..width * height)
        .map(|i| {
            let x = (i % width) as f32 / (width - 1) as f32;
            let y = (i / width) as f32 / (height - 1) as f32;
            (depth(x, y).clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    MaskImage::new(width, height, pixels).unwrap()
}

/// Nearest in the middle, receding to both sides: the left half faces left.
fn ridge() -> MaskImage {
    depth_map(WIDTH, HEIGHT, |x, _| 0.2 + 0.6 * (x - 0.5).abs())
}

fn flat() -> MaskImage {
    depth_map(WIDTH, HEIGHT, |_, _| 0.3)
}

/// A near vertical bar in front of a mid-distance background.
fn bar_before_wall() -> MaskImage {
    depth_map(WIDTH, HEIGHT, |x, _| {
        if (0.42..0.58).contains(&x) {
            0.05
        } else {
            0.5
        }
    })
}

fn relight(settings: RelightEffectSettings) -> EffectComponent {
    let mut component = EffectComponent::new(MaskEffect::Relight);
    component.settings.relight = settings;
    component
}

fn global_relight(settings: RelightEffectSettings, depth: Option<MaskImage>) -> MaskStack {
    MaskStack {
        global_effects: vec![relight(settings)],
        scene_depth: depth,
        ..Default::default()
    }
}

/// Mean added light (render minus baseline) over pixel columns `columns`,
/// leaving out the top and bottom rows.
fn added_light(render: &[f32], baseline: &[f32], columns: std::ops::Range<u32>) -> f32 {
    let mut sum = 0.0;
    let mut count = 0;
    for y in 8..HEIGHT - 8 {
        for x in columns.clone() {
            let index = ((y * WIDTH + x) * 3) as usize;
            for channel in 0..3 {
                sum += render[index + channel] - baseline[index + channel];
                count += 1;
            }
        }
    }
    sum / count as f32
}

#[test]
fn relight_params_request_the_surface_only_for_an_active_component() -> anyhow::Result<()> {
    let source = grey(WIDTH, HEIGHT)?;
    let exposure = neutral_exposure();
    let active = global_relight(RelightEffectSettings::default(), Some(flat()));
    assert!(GpuParams::new(&exposure, &active, &source).needs_relight_surface());

    let mut hidden = active;
    hidden.global_effects[0].enabled = false;
    assert!(!GpuParams::new(&exposure, &hidden, &source).needs_relight_surface());

    let inactive = global_relight(
        RelightEffectSettings {
            amount: 0.0,
            ..Default::default()
        },
        Some(flat()),
    );
    assert!(!GpuParams::new(&exposure, &inactive, &source).needs_relight_surface());
    assert!(!GpuParams::new(&exposure, &MaskStack::default(), &source).needs_relight_surface());
    Ok(())
}

#[test]
fn relight_gpu_inactive_or_hidden_components_leave_the_render_unchanged() -> anyhow::Result<()> {
    for quality in [ProcessingQuality::Preview, ProcessingQuality::High] {
        let Some(scene) = FogScene::with_source(grey(WIDTH, HEIGHT)?, quality)? else {
            return Ok(());
        };
        let baseline = scene.render(&MaskStack::default())?;
        let inactive = RelightEffectSettings {
            amount: 0.0,
            ..Default::default()
        };
        for depth in [None, Some(ridge())] {
            let rendered = scene.render(&global_relight(inactive, depth.clone()))?;
            assert_close(&rendered, &baseline, RGB_TOLERANCE, "zero amount");
            let mut hidden = global_relight(RelightEffectSettings::default(), depth);
            hidden.global_effects[0].enabled = false;
            let rendered = scene.render(&hidden)?;
            assert_close(&rendered, &baseline, RGB_TOLERANCE, "hidden component");
        }
    }
    Ok(())
}

#[test]
fn relight_gpu_adds_light_and_ambient_dims_the_existing_light() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey(WIDTH, HEIGHT)?, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    for depth in [None, Some(flat()), Some(ridge())] {
        let lit = scene.render(&global_relight(
            RelightEffectSettings::default(),
            depth.clone(),
        ))?;
        assert!(
            lit.iter()
                .zip(&baseline)
                .all(|(lit, base)| lit >= &(base - 1e-5)),
            "added light never darkens"
        );
        assert!(mean_difference(&lit, &baseline) > 0.01, "light is visible");

        let dimmed = scene.render(&global_relight(
            RelightEffectSettings {
                amount: 0.0,
                ambient: 50.0,
                ..Default::default()
            },
            depth,
        ))?;
        assert!(dimmed.iter().zip(&baseline).all(|(dim, base)| dim < base));
    }
    Ok(())
}

#[test]
fn relight_gpu_surfaces_facing_the_light_receive_more_of_it() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey(WIDTH, HEIGHT)?, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let settings = RelightEffectSettings {
        source: [-20.0, 50.0],
        depth: -20.0,
        size: 0.0,
        shadows: 0.0,
        ..Default::default()
    };
    let left_to_right = |depth: MaskImage| -> anyhow::Result<f32> {
        let render = scene.render(&global_relight(settings, Some(depth)))?;
        let left = added_light(&render, &baseline, 10..38);
        let right = added_light(&render, &baseline, 58..86);
        assert!(left > 0.0 && right >= 0.0, "{left} {right}");
        Ok(left / right.max(1e-6))
    };
    let flat_ratio = left_to_right(flat())?;
    let ridge_ratio = left_to_right(ridge())?;
    assert!(
        ridge_ratio > 1.5 * flat_ratio,
        "a slope facing the light must gain more than its averted twin: ridge {ridge_ratio}, flat {flat_ratio}"
    );
    Ok(())
}

#[test]
fn relight_gpu_near_occluder_casts_a_shadow_on_the_wall_behind_it() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey(WIDTH, HEIGHT)?, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let settings = RelightEffectSettings {
        source: [20.0, 50.0],
        depth: -50.0,
        size: 10.0,
        ..Default::default()
    };
    let render = |shadows: f32| {
        scene.render(&global_relight(
            RelightEffectSettings {
                shadows,
                ..settings
            },
            Some(bar_before_wall()),
        ))
    };
    let unshadowed = render(0.0)?;
    let shadowed = render(100.0)?;
    // Turning shadows off keeps the strength but casts nothing.
    let switched_off = scene.render(&global_relight(
        RelightEffectSettings {
            shadows_enabled: false,
            shadows: 100.0,
            ..settings
        },
        Some(bar_before_wall()),
    ))?;
    assert_close(&switched_off, &unshadowed, RGB_TOLERANCE, "shadows off");
    // The bar's shadow falls on the wall to its right, away from the light.
    let behind = |render: &[f32]| added_light(render, &baseline, 60..72);
    assert!(
        behind(&shadowed) < 0.5 * behind(&unshadowed),
        "{} vs {}",
        behind(&shadowed),
        behind(&unshadowed)
    );
    // Between the light and the bar nothing blocks it.
    let open = |render: &[f32]| added_light(render, &baseline, 6..28);
    assert!(
        open(&shadowed) > 0.95 * open(&unshadowed),
        "{} vs {}",
        open(&shadowed),
        open(&unshadowed)
    );
    Ok(())
}

#[test]
fn relight_gpu_mask_limits_the_light_and_full_mask_matches_global() -> anyhow::Result<()> {
    let Some(scene) = FogScene::with_source(grey(WIDTH, HEIGHT)?, ProcessingQuality::High)? else {
        return Ok(());
    };
    let baseline = scene.render(&MaskStack::default())?;
    let global = global_relight(RelightEffectSettings::default(), Some(ridge()));
    let expected = scene.render(&global)?;
    let mut mask = LocalMask::new(MaskKind::Fullscreen, 1);
    mask.effect_components
        .push(relight(RelightEffectSettings::default()));
    let masked = MaskStack {
        masks: vec![mask],
        scene_depth: Some(ridge()),
        ..Default::default()
    };
    let one = half::f16::ONE.to_bits();
    scene.pipeline.update_mask_layer(
        &scene.queue,
        0,
        &vec![one; (MASK_EDGE * MASK_EDGE) as usize],
    )?;
    assert_close(
        &scene.render(&masked)?,
        &expected,
        RGB_TOLERANCE,
        "full mask equals global",
    );

    // Only the left half of the mask is selected; the right half stays as shot.
    let left_half: Vec<u16> = (0..MASK_EDGE * MASK_EDGE)
        .map(|i| {
            if i % MASK_EDGE < MASK_EDGE / 2 {
                one
            } else {
                0
            }
        })
        .collect();
    scene
        .pipeline
        .update_mask_layer(&scene.queue, 0, &left_half)?;
    let half_masked = scene.render(&masked)?;
    assert!(added_light(&half_masked, &baseline, 8..40) > 0.01);
    assert!(added_light(&half_masked, &baseline, 58..96).abs() < 1e-5);
    Ok(())
}

#[test]
fn relight_gpu_derives_the_surface_for_depth_first_uploaded_without_it() -> anyhow::Result<()> {
    let source = grey(WIDTH, HEIGHT)?;
    let (Some(reused), Some(fresh)) = (
        FogScene::with_source(source.clone(), ProcessingQuality::High)?,
        FogScene::with_source(source, ProcessingQuality::High)?,
    ) else {
        return Ok(());
    };
    // Clones share the pixel allocation, so the upload cache sees one depth.
    let depth = ridge();
    let fog_only = MaskStack {
        global_effects: vec![EffectComponent::new(MaskEffect::Fog)],
        scene_depth: Some(depth.clone()),
        ..Default::default()
    };
    reused.render(&fog_only)?;
    let relit = global_relight(RelightEffectSettings::default(), Some(depth));
    assert_close(
        &reused.render(&relit)?,
        &fresh.render(&relit)?,
        RGB_TOLERANCE,
        "Relight after depth uploaded for fog alone",
    );
    Ok(())
}

#[test]
fn relight_gpu_matches_the_full_frame_in_overlapping_tiles() -> anyhow::Result<()> {
    const TILE_WIDTH: u32 = 320;
    const TILE_HEIGHT: u32 = 240;
    let Some(scene) =
        FogScene::with_source(grey(TILE_WIDTH, TILE_HEIGHT)?, ProcessingQuality::High)?
    else {
        return Ok(());
    };
    let depth = depth_map(37, 29, |x, y| {
        if (0.4..0.55).contains(&x) && y > 0.3 {
            0.1
        } else {
            0.35 + 0.4 * x * y
        }
    });
    let masks = global_relight(
        RelightEffectSettings {
            source: [15.0, 20.0],
            depth: -40.0,
            shadows: 100.0,
            ambient: 80.0,
            ..Default::default()
        },
        Some(depth),
    );
    let baseline = scene.render(&MaskStack::default())?;
    let full = scene.render(&masks)?;
    assert!(
        mean_difference(&full, &baseline) > 0.01,
        "tile fixture relight is inactive"
    );
    for (x, y) in [(104, 88), (111, 93)] {
        const HALO: u32 = 64;
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
            scene.source.width,
            scene.source.height,
        );
        let pipeline = RawGpuPipeline::new(
            &scene.device,
            &scene.queue,
            &raw,
            &params,
            PipelineOptions::new(ProcessingQuality::High)
                .mask_atlas_edge(MASK_EDGE)
                .programs(&scene.pipeline.program_template()),
        )?;
        pipeline.dispatch_stage(&scene.queue, &scene.device, &params, ProcessingStage::Raw);
        // The reflectance estimate uses full-image illumination, not each tile's.
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
                let start = ((row * scene.source.width + x) * 3) as usize;
                full[start..start + (tile.core_width * 3) as usize]
                    .iter()
                    .copied()
            })
            .collect();
        assert_close(
            &actual,
            &expected,
            RGB_TOLERANCE,
            &format!("relight tile at {x},{y} must match the full frame"),
        );
    }
    Ok(())
}
