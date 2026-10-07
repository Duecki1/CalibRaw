use super::{CompactPixelMap, DenoiseQuality, ExposureParams, LoadedRaw, MaskEffect, MaskStack};
use rayon::prelude::*;

mod ai_denoised_regions;
mod proxy;
mod tiles;
use ai_denoised_regions::*;
pub use proxy::*;
pub use tiles::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessingStage {
    Raw,
    Tone,
    Output,
}

impl ProcessingStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Raw => "RAW reconstruction",
            Self::Tone => "tone analysis",
            Self::Output => "display rendering",
        }
    }
}

pub fn affected_stage(before: &ExposureParams, after: &ExposureParams) -> Option<ProcessingStage> {
    if before == after {
        return None;
    }

    if raw_controls_changed(before, after)
        || before.temperature != after.temperature
        || before.tint != after.tint
    {
        Some(ProcessingStage::Raw)
    } else {
        Some(ProcessingStage::Output)
    }
}

fn raw_controls_changed(before: &ExposureParams, after: &ExposureParams) -> bool {
    before.black_point != after.black_point
        || before.chroma_denoise != after.chroma_denoise
        || before.luminance_denoise != after.luminance_denoise
        || before.denoise_detail != after.denoise_detail
        || before.denoise_quality != after.denoise_quality
        || before.ai_denoise_enabled != after.ai_denoise_enabled
        || before.demosaic_mode != after.demosaic_mode
        || before.dual_threshold != after.dual_threshold
        || before.frequency_chroma != after.frequency_chroma
        || before.ca_red != after.ca_red
        || before.ca_blue != after.ca_blue
        || before.highlight_method != after.highlight_method
        || before.highlight_clip != after.highlight_clip
        || before.highlight_reconstruction != after.highlight_reconstruction
}

#[cfg(test)]
mod tests {
    use super::{
        affected_stage, build_proxy, build_region_proxy, crop_raw, extract_padded_tile,
        extract_padded_tile_into, required_export_tile_halo, ExportTile, ProcessingStage,
        ProxySpec, TilePlan, TileSpec, EXPORT_TILE_HALO, GLOW_SUPPORT, MIN_EXPORT_TILE_HALO,
    };
    use crate::pipeline::{
        AiDenoisedImage, CameraProfile, CfaKind, CompactPixelMap, DenoiseQuality, ExposureParams,
        LoadedRaw, MaskEffect, MaskStack,
    };

    fn test_raster(width: u32, height: u32) -> LoadedRaw {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&[x as f32, y as f32, (x + y) as f32]);
            }
        }
        LoadedRaw::from_scene_linear_rec2020(width, height, rgb).unwrap()
    }

    fn test_raw(width: u32, height: u32) -> LoadedRaw {
        let pixels = (0..width * height)
            .map(|value| value as u16)
            .collect::<Vec<_>>();
        LoadedRaw {
            width,
            height,
            camera_make: "Test".to_owned(),
            camera_model: String::new(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: pixels,
            scene_linear_raster: None,
            color_indices: CompactPixelMap::dense(
                width,
                height,
                vec![0; (width * height) as usize],
            ),
            wb_coeffs: [1.0; 4],
            cam_to_srgb: [[0.0; 4]; 3],
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::dense(
                width,
                height,
                vec![0.0; (width * height) as usize],
            ),
            white_levels: [1023.0; 4],
            noise_profile: crate::pipeline::NoiseProfile::default(),
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: None,
            lens_geometry: None,
            ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        }
    }

    fn colored_highlight_raw(width: u32, height: u32) -> LoadedRaw {
        let mut raw = test_raw(width, height);
        raw.color_indices = CompactPixelMap::repeating(width, height, 2, 2, vec![0, 1, 3, 2]);
        raw.white_levels = [10_000.0; 4];
        raw.black_levels_per_pixel = CompactPixelMap::repeating(width, height, 1, 1, vec![0.0]);
        raw.raw_pixels.clear();
        raw.raw_pixels.reserve((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let physical = raw.color_indices[(y * width + x) as usize];
                let logical = usize::from(if physical == 3 { 1 } else { physical });
                let mut value = [0.82_f32, 0.58, 0.36][logical];
                if (width / 3..2 * width / 3).contains(&x)
                    && (height / 3..2 * height / 3).contains(&y)
                    && (logical == 0 || logical == 2)
                {
                    value = 1.0;
                }
                raw.raw_pixels.push((value * 10_000.0).round() as u16);
            }
        }
        raw
    }

    #[test]
    fn opposed_chroma_full_reference_is_shared_by_moved_crops_and_proxy() {
        let raw = colored_highlight_raw(120, 96);
        let wb = [1.45, 1.0, 0.72, 1.0];
        let reference = raw.inpaint_opposed_chroma(0.0, 1.0, false, wb);
        assert_eq!(raw.opposed_chroma_cache.read().unwrap().len(), 1);

        let first = crop_raw(&raw, 18, 12, 78, 70);
        let shifted = crop_raw(&raw, 24, 18, 78, 70);
        let proxy = build_region_proxy(&raw, 16, 10, 86, 74, ProxySpec { max_edge: 42 });

        for derived in [&first, &shifted, &proxy] {
            assert!(std::sync::Arc::ptr_eq(
                &derived.opposed_chroma_cache,
                &raw.opposed_chroma_cache
            ));
            assert!(std::sync::Arc::ptr_eq(
                &derived.opposed_chroma_source_identity,
                &raw.opposed_chroma_source_identity
            ));
            assert!(!derived.opposed_chroma_reference_source);
            assert_eq!(
                derived.inpaint_opposed_chroma(0.0, 1.0, false, wb),
                reference
            );
        }
    }

    #[test]
    fn derived_opposed_chroma_miss_does_not_poison_full_source_cache() {
        let raw = colored_highlight_raw(120, 96);
        let wb = [1.35, 1.0, 0.78, 1.0];
        let crop = crop_raw(&raw, 24, 18, 72, 66);

        let _local_fallback = crop.inpaint_opposed_chroma(0.0, 1.0, false, wb);
        assert!(raw.opposed_chroma_cache.read().unwrap().is_empty());

        let full_reference = raw.inpaint_opposed_chroma(0.0, 1.0, false, wb);
        assert_eq!(raw.opposed_chroma_cache.read().unwrap().len(), 1);
        assert_eq!(
            crop.inpaint_opposed_chroma(0.0, 1.0, false, wb),
            full_reference
        );
    }

    #[test]
    fn padded_tile_allocates_first_export_buffer_and_clamps_edges() {
        let raw = test_raw(3, 2);
        let tile = ExportTile {
            core_x: 0,
            core_y: 0,
            core_width: 3,
            core_height: 2,
            local_core_x: 1,
            local_core_y: 1,
            padded_width: 5,
            padded_height: 4,
            global_origin_x: -1,
            global_origin_y: -1,
        };

        let extracted = extract_padded_tile(&raw, tile);

        assert_eq!(extracted.raw_pixels.len(), 20);
        assert_eq!(&extracted.raw_pixels[0..5], &[0, 0, 1, 2, 2]);
        assert_eq!(&extracted.raw_pixels[5..10], &[0, 0, 1, 2, 2]);
        assert_eq!(&extracted.raw_pixels[10..15], &[3, 3, 4, 5, 5]);
        assert_eq!(&extracted.raw_pixels[15..20], &[3, 3, 4, 5, 5]);
    }

    #[test]
    fn padded_tile_reuse_resizes_buffer_for_new_tile_shape() {
        let raw = test_raw(4, 3);
        let first = ExportTile {
            core_x: 0,
            core_y: 0,
            core_width: 2,
            core_height: 2,
            local_core_x: 0,
            local_core_y: 0,
            padded_width: 2,
            padded_height: 2,
            global_origin_x: 0,
            global_origin_y: 0,
        };
        let second = ExportTile {
            core_x: 0,
            core_y: 0,
            core_width: 4,
            core_height: 3,
            local_core_x: 1,
            local_core_y: 1,
            padded_width: 6,
            padded_height: 5,
            global_origin_x: -1,
            global_origin_y: -1,
        };

        let mut scratch = extract_padded_tile(&raw, first);
        extract_padded_tile_into(&raw, second, &mut scratch);

        assert_eq!(scratch.raw_pixels.len(), 30);
        assert_eq!(&scratch.raw_pixels[0..6], &[0, 0, 1, 2, 3, 3]);
        assert_eq!(&scratch.raw_pixels[24..30], &[8, 8, 9, 10, 11, 11]);
    }

    #[test]
    fn ai_denoise_cache_tracks_crop_proxy_and_export_tile_geometry() {
        let raw = test_raw(4, 2);
        let raw_cfa16 = (0..8).map(|pixel| pixel as u16).collect();
        raw.set_ai_denoised_image(AiDenoisedImage::new_bayer_cfa(4, 2, raw_cfa16).unwrap())
            .unwrap();

        let crop = crop_raw(&raw, 1, 0, 2, 2)
            .ai_denoised_image()
            .expect("crop retains aligned AI output");
        assert_eq!(crop.raw_cfa16.as_ref(), &[1, 2, 5, 6]);

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 2 })
            .ai_denoised_image()
            .expect("proxy derives AI output");
        assert_eq!(proxy.raw_cfa16.as_ref(), &[0, 2, 4, 6]);

        let tile = extract_padded_tile(
            &raw,
            ExportTile {
                core_x: 0,
                core_y: 0,
                core_width: 4,
                core_height: 2,
                local_core_x: 1,
                local_core_y: 1,
                padded_width: 6,
                padded_height: 4,
                global_origin_x: -1,
                global_origin_y: -1,
            },
        )
        .ai_denoised_image()
        .expect("export tile retains aligned AI output");
        assert_eq!(tile.raw_cfa16[0], 0);
        assert_eq!(tile.raw_cfa16[tile.raw_cfa16.len() - 1], 7);
    }

    #[test]
    fn develop_adjustments_only_invalidate_output() {
        let before = ExposureParams::default();
        let mut after = before;
        after.exposure = 1.0;
        assert_eq!(
            affected_stage(&before, &after),
            Some(ProcessingStage::Output)
        );
    }

    #[test]
    fn raw_controls_invalidate_every_downstream_stage() {
        let before = ExposureParams::default();

        let mut black_point = before;
        black_point.black_point = 0.01;
        assert_eq!(
            affected_stage(&before, &black_point),
            Some(ProcessingStage::Raw)
        );

        let mut luminance_denoise = before;
        luminance_denoise.luminance_denoise = 25.0;
        assert_eq!(
            affected_stage(&before, &luminance_denoise),
            Some(ProcessingStage::Raw)
        );

        let mut denoise_quality = before;
        denoise_quality.denoise_quality = crate::pipeline::DenoiseQuality::High;
        assert_eq!(
            affected_stage(&before, &denoise_quality),
            Some(ProcessingStage::Raw)
        );
    }

    #[test]
    fn global_wb_invalidates_raw_reconstruction_and_downstream_stages() {
        let before = ExposureParams::default();
        for after in [
            ExposureParams {
                temperature: 1.0,
                ..before
            },
            ExposureParams {
                tint: 1.0,
                ..before
            },
        ] {
            assert_eq!(affected_stage(&before, &after), Some(ProcessingStage::Raw));
        }
    }

    #[test]
    fn export_halo_shrinks_when_wide_radius_effects_are_neutral() {
        let masks = MaskStack::default();
        let mut exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        assert_eq!(
            required_export_tile_halo(&exposure, &masks),
            MIN_EXPORT_TILE_HALO
        );

        let mut neon_masks = MaskStack::default();
        neon_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        neon_masks.masks[0].effect = MaskEffect::Neon;
        assert!(required_export_tile_halo(&exposure, &neon_masks) > MIN_EXPORT_TILE_HALO);
        neon_masks.masks[0].effect_settings.neon.amount = 0.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &neon_masks),
            MIN_EXPORT_TILE_HALO
        );

        let mut glow_masks = MaskStack::default();
        glow_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        glow_masks.masks[0].effect = MaskEffect::Glow;
        assert!(required_export_tile_halo(&exposure, &glow_masks) > MIN_EXPORT_TILE_HALO);
        glow_masks.masks[0].effect_settings.glow.amount = 0.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &glow_masks),
            MIN_EXPORT_TILE_HALO
        );

        let mut light_rays_masks = MaskStack::default();
        light_rays_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        light_rays_masks.masks[0].effect = MaskEffect::LightRays;
        assert_eq!(
            required_export_tile_halo(&exposure, &light_rays_masks),
            MIN_EXPORT_TILE_HALO
        );

        let mut atmosphere_masks = MaskStack::default();
        atmosphere_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        atmosphere_masks.masks[0].effect = MaskEffect::Fog;
        assert_eq!(
            required_export_tile_halo(&exposure, &atmosphere_masks),
            MIN_EXPORT_TILE_HALO
        );
        atmosphere_masks.masks[0].effect = MaskEffect::Smoke;
        assert_eq!(
            required_export_tile_halo(&exposure, &atmosphere_masks),
            MIN_EXPORT_TILE_HALO
        );

        let mut creative_masks = MaskStack::default();
        creative_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        creative_masks.masks[0].effect = MaskEffect::Blur;
        let blur_halo = required_export_tile_halo(&exposure, &creative_masks);
        assert!(blur_halo > MIN_EXPORT_TILE_HALO);
        creative_masks.masks[0].effect = MaskEffect::LensBlur;
        let focus_blur_halo = required_export_tile_halo(&exposure, &creative_masks);
        assert!(focus_blur_halo > blur_halo);
        creative_masks.masks[0].effect_settings.lens_blur.amount = 0.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &creative_masks),
            MIN_EXPORT_TILE_HALO
        );
        creative_masks.masks[0].effect_settings.lens_blur.amount = 50.0;
        creative_masks.masks[0].effect = MaskEffect::Pixelate;
        assert!(required_export_tile_halo(&exposure, &creative_masks) > blur_halo);
        creative_masks.masks[0].effect_settings.pixelate.amount = 0.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &creative_masks),
            MIN_EXPORT_TILE_HALO
        );

        exposure.grain_amount = 100.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &masks),
            MIN_EXPORT_TILE_HALO
        );
        exposure.halation_amount = 100.0;
        let halation_halo = required_export_tile_halo(&exposure, &masks);
        assert!(halation_halo > MIN_EXPORT_TILE_HALO);
        exposure.halation_amount = 0.0;
        let mut halation_masks = MaskStack::default();
        halation_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        halation_masks.masks[0].adjustments.halation_amount = 100.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &halation_masks),
            halation_halo
        );
        halation_masks.masks[0].enabled = false;
        assert_eq!(
            required_export_tile_halo(&exposure, &halation_masks),
            MIN_EXPORT_TILE_HALO
        );

        exposure.glow_amount = 1.0;
        assert!(required_export_tile_halo(&exposure, &masks) > MIN_EXPORT_TILE_HALO);
        exposure.clarity = 1.0;
        exposure.chroma_denoise = 1.0;
        exposure.denoise_quality = DenoiseQuality::High;
        neon_masks.masks[0].effect_settings.neon.amount = 50.0;
        neon_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        neon_masks.masks[1].effect = MaskEffect::Pixelate;
        neon_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        neon_masks.masks[2].effect = MaskEffect::Blur;
        neon_masks.add_mask(crate::pipeline::MaskKind::Fullscreen);
        neon_masks.masks[3].effect = MaskEffect::LensBlur;
        assert_eq!(
            required_export_tile_halo(&exposure, &neon_masks),
            EXPORT_TILE_HALO
        );
    }

    #[test]
    fn export_halo_covers_global_masked_and_legacy_spatial_effects() {
        use crate::pipeline::{EffectComponent, MaskKind};

        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        for effect in MaskEffect::ALL {
            if effect == MaskEffect::Adjustment {
                continue;
            }
            let mut stack = MaskStack::default();
            stack.add_mask(MaskKind::Fullscreen).unwrap();
            stack.masks[0].effect = effect;
            let legacy_halo = required_export_tile_halo(&exposure, &stack);
            let effect_support = match effect {
                MaskEffect::Neon => super::NEON_SUPPORT,
                MaskEffect::Blur => super::MASK_BLUR_SUPPORT,
                MaskEffect::LensBlur
                | MaskEffect::MotionBlur
                | MaskEffect::RadialBlur
                | MaskEffect::TiltShift => super::FOCUS_BLUR_SUPPORT,
                MaskEffect::EdgeGlow => super::EDGE_GLOW_SUPPORT,
                MaskEffect::Pixelate => super::PIXELATE_SUPPORT,
                MaskEffect::Glow | MaskEffect::Halation => GLOW_SUPPORT,
                MaskEffect::Adjustment
                | MaskEffect::LightRays
                | MaskEffect::Relight
                | MaskEffect::Fog
                | MaskEffect::Smoke
                | MaskEffect::Grain
                | MaskEffect::Vignette => 0,
            };
            assert_eq!(
                legacy_halo,
                MIN_EXPORT_TILE_HALO + effect_support,
                "{effect:?}"
            );
            stack.masks[0].enabled = false;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
            stack.masks[0].enabled = true;
            stack.masks[0].opacity = 0.0;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
            stack.masks[0].opacity = 1.0;
            stack.masks[0].effect = MaskEffect::Adjustment;
            stack.masks[0]
                .effect_components
                .push(EffectComponent::new(effect));
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                legacy_halo,
                "{effect:?}"
            );
            stack.masks[0].effect_components[0].enabled = false;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
            stack.masks[0].effect_components[0].enabled = true;
            stack.masks[0].enabled = false;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
            stack.masks[0].enabled = true;
            stack.masks[0].opacity = 0.0;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );

            stack.global_effects.push(EffectComponent::new(effect));
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                legacy_halo,
                "{effect:?}"
            );
            stack.global_effects[0].enabled = false;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
        }
    }

    #[test]
    fn photographic_export_halo_uses_maximum_footprint_and_skips_point_effects() {
        use crate::pipeline::{EffectComponent, MaskKind};

        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        let mut stack = MaskStack::default();
        for effect in [MaskEffect::Grain, MaskEffect::Vignette] {
            stack.global_effects.push(EffectComponent::new(effect));
            stack.add_mask(MaskKind::Fullscreen).unwrap();
            stack
                .masks
                .last_mut()
                .unwrap()
                .effect_components
                .push(EffectComponent::new(effect));
        }
        assert_eq!(
            required_export_tile_halo(&exposure, &stack),
            MIN_EXPORT_TILE_HALO
        );

        stack
            .global_effects
            .push(EffectComponent::new(MaskEffect::Halation));
        let neighborhood_halo = required_export_tile_halo(&exposure, &stack);
        assert_eq!(neighborhood_halo, MIN_EXPORT_TILE_HALO + GLOW_SUPPORT);
        stack
            .global_effects
            .last_mut()
            .unwrap()
            .settings
            .halation
            .amount = 0.0;
        assert_eq!(
            required_export_tile_halo(&exposure, &stack),
            MIN_EXPORT_TILE_HALO
        );

        stack
            .global_effects
            .last_mut()
            .unwrap()
            .settings
            .halation
            .amount = 25.0;
        stack
            .global_effects
            .push(EffectComponent::new(MaskEffect::Glow));
        stack.masks[0]
            .effect_components
            .push(EffectComponent::new(MaskEffect::Halation));
        stack.masks[0].adjustments.halation_amount = 50.0;
        let legacy_exposure = ExposureParams {
            halation_amount: 50.0,
            glow_amount: 50.0,
            ..exposure
        };
        assert_eq!(
            required_export_tile_halo(&legacy_exposure, &stack),
            neighborhood_halo
        );

        // Independent modules retain their own radii, but their footprints
        // overlap rather than accumulate, including at the largest radii.
        for component in stack.global_effects.iter_mut().chain(
            stack
                .masks
                .iter_mut()
                .flat_map(|mask| &mut mask.effect_components),
        ) {
            component.settings.halation.radius =
                crate::pipeline::effect_params::halation::RADIUS.max;
            component.settings.glow.radius = crate::pipeline::effect_params::glow::RADIUS.max;
        }
        assert_eq!(
            required_export_tile_halo(&legacy_exposure, &stack),
            neighborhood_halo
        );
    }

    #[test]
    fn export_halo_ignores_disabled_or_transparent_legacy_adjustments() {
        use crate::pipeline::{LocalAdjustments, MaskKind};

        let exposure = ExposureParams {
            sharpen_amount: 0.0,
            ..Default::default()
        };
        type AdjustmentCase = (fn(&mut LocalAdjustments) -> &mut f32, u32);
        let cases: [AdjustmentCase; 4] = [
            (|a| &mut a.texture, super::LOCAL_EFFECTS_SUPPORT),
            (|a| &mut a.clarity, super::LOCAL_EFFECTS_SUPPORT),
            (|a| &mut a.dehaze, super::LOCAL_EFFECTS_SUPPORT),
            (|a| &mut a.halation_amount, GLOW_SUPPORT),
        ];
        for (field, support) in cases {
            let mut stack = MaskStack::default();
            stack.add_mask(MaskKind::Fullscreen).unwrap();
            *field(&mut stack.masks[0].adjustments) = 50.0;
            let expected = (super::HIGHLIGHT_RECONSTRUCTION_SUPPORT
                + super::DEMOSAIC_CHAIN_SUPPORT
                + super::TONE_GUIDE_SUPPORT
                + super::COLOR_MIXER_SUPPORT
                + support)
                .div_ceil(8)
                * 8;
            assert_eq!(required_export_tile_halo(&exposure, &stack), expected);
            stack.masks[0].adjustments_enabled = false;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
            stack.masks[0].adjustments_enabled = true;
            stack.masks[0].opacity = 0.0;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
            stack.masks[0].opacity = 0.25;
            assert_eq!(required_export_tile_halo(&exposure, &stack), expected);
            stack.masks[0].enabled = false;
            assert_eq!(
                required_export_tile_halo(&exposure, &stack),
                MIN_EXPORT_TILE_HALO
            );
        }
    }

    #[test]
    fn tile_plan_covers_partial_edges() {
        let plan = TilePlan::new(
            2500,
            1300,
            TileSpec {
                core_edge: 1024,
                halo: 48,
            },
        );
        assert_eq!(plan.tile_count(), 6);
        assert_eq!(plan.tiles.last().unwrap().core_width, 452);
        assert_eq!(plan.tiles.last().unwrap().core_height, 276);
    }

    #[test]
    fn raster_crop_proxy_and_export_tile_stay_in_scene_linear_rgb() {
        let raw = test_raster(4, 4);
        let cropped = crop_raw(&raw, 1, 1, 2, 2);
        assert!(cropped.is_pre_demosaiced_raster());
        assert_eq!(
            cropped.scene_linear_raster().unwrap(),
            &[1.0, 1.0, 2.0, 2.0, 1.0, 3.0, 1.0, 2.0, 3.0, 2.0, 2.0, 4.0]
        );

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 2 });
        assert_eq!((proxy.width, proxy.height), (2, 2));
        assert!(proxy.is_pre_demosaiced_raster());
        let proxy_rgb = proxy.scene_linear_raster().unwrap();
        assert_eq!(
            proxy_rgb,
            &[0.5, 0.5, 1.0, 2.5, 0.5, 3.0, 0.5, 2.5, 3.0, 2.5, 2.5, 5.0]
        );

        let tile = ExportTile {
            core_x: 0,
            core_y: 0,
            core_width: 2,
            core_height: 2,
            local_core_x: 1,
            local_core_y: 1,
            padded_width: 4,
            padded_height: 4,
            global_origin_x: -1,
            global_origin_y: -1,
        };
        let mut padded = extract_padded_tile(&raw, tile);
        assert!(padded.is_pre_demosaiced_raster());
        let allocation = padded.scene_linear_raster().unwrap().as_ptr();
        let padded_rgb = padded.scene_linear_raster().unwrap();
        assert_eq!(&padded_rgb[0..3], &[0.0, 0.0, 0.0]);
        assert_eq!(
            &padded_rgb[(2 * 4 + 2) * 3..(2 * 4 + 2) * 3 + 3],
            &[1.0, 1.0, 2.0]
        );

        let shifted = ExportTile {
            global_origin_x: 0,
            global_origin_y: 0,
            ..tile
        };
        extract_padded_tile_into(&raw, shifted, &mut padded);
        assert_eq!(
            padded.scene_linear_raster().unwrap().as_ptr(),
            allocation,
            "same-size raster export tiles should reuse their float buffer"
        );
        assert_eq!(
            &padded.scene_linear_raster().unwrap()[0..3],
            &[0.0, 0.0, 0.0]
        );
        assert_eq!(
            &padded.scene_linear_raster().unwrap()[(3 * 4 + 3) * 3..(3 * 4 + 3) * 3 + 3],
            &[3.0, 3.0, 6.0]
        );
    }

    #[test]
    fn raster_derivatives_preserve_camera_transform_metadata() {
        let mut raw = test_raster(4, 4);
        raw.wb_coeffs = [2.0, 1.0, 0.5, 1.0];
        raw.cam_to_srgb = [
            [1.0, 2.0, 3.0, 0.0],
            [4.0, 5.0, 6.0, 0.0],
            [7.0, 8.0, 9.0, 0.0],
        ];
        let cropped = crop_raw(&raw, 1, 1, 2, 2);
        assert_eq!(cropped.wb_coeffs, raw.wb_coeffs);
        assert_eq!(cropped.cam_to_srgb, raw.cam_to_srgb);
        assert_eq!(cropped.white_levels, raw.white_levels);
        assert_eq!(cropped.black_levels, raw.black_levels);
    }

    #[test]
    fn crop_raw_copies_only_the_requested_sensor_region() {
        let width = 4;
        let height = 3;
        let raw = LoadedRaw {
            width,
            height,
            camera_make: "Test".to_owned(),
            camera_model: String::new(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: (0..width * height).map(|value| value as u16).collect(),
            scene_linear_raster: None,
            color_indices: CompactPixelMap::dense(
                width,
                height,
                (0..width * height).map(|value| (value % 4) as u8).collect(),
            ),
            wb_coeffs: [1.0; 4],
            cam_to_srgb: [[0.0; 4]; 3],
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::dense(
                width,
                height,
                (0..width * height).map(|value| value as f32).collect(),
            ),
            white_levels: [1023.0; 4],
            noise_profile: crate::pipeline::NoiseProfile::default(),
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: None,
            lens_geometry: None,
            ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        };

        let cropped = crop_raw(&raw, 1, 1, 2, 2);
        assert_eq!((cropped.width, cropped.height), (2, 2));
        assert_eq!(cropped.raw_pixels, vec![5, 6, 9, 10]);
        assert_eq!(
            cropped.color_indices.iter().copied().collect::<Vec<_>>(),
            vec![1, 2, 1, 2]
        );
        assert_eq!(
            cropped
                .black_levels_per_pixel
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![5.0, 6.0, 9.0, 10.0]
        );
        assert_eq!(cropped.camera_make, "Test");
    }

    #[test]
    fn proxy_preserves_bayer_phase_when_scale_is_even() {
        let width = 8;
        let height = 8;
        let mut color_indices = Vec::new();
        for y in 0..height {
            for x in 0..width {
                color_indices.push(match (x % 2, y % 2) {
                    (0, 0) => 0,
                    (1, 0) => 1,
                    (0, 1) => 3,
                    _ => 2,
                });
            }
        }
        let raw = LoadedRaw {
            width,
            height,
            camera_make: String::new(),
            camera_model: String::new(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: vec![100; (width * height) as usize],
            scene_linear_raster: None,
            color_indices: CompactPixelMap::dense(width, height, color_indices),
            wb_coeffs: [1.0; 4],
            cam_to_srgb: [[0.0; 4]; 3],
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::dense(
                width,
                height,
                vec![0.0; (width * height) as usize],
            ),
            white_levels: [1023.0; 4],
            noise_profile: crate::pipeline::NoiseProfile::default(),
            camera_profile: CameraProfile::default(),
            camera_profile_source: None,
            available_camera_profiles: Vec::new(),
            white_balance_model: None,
            lens_geometry: None,
            ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        };

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 4 });
        assert_eq!(proxy.width, 4);
        assert_eq!(proxy.height, 4);
        let proxy_cfa = proxy.color_indices.iter().copied().collect::<Vec<_>>();
        assert_eq!(&proxy_cfa[..4], &[0, 1, 0, 1]);
        assert_eq!(&proxy_cfa[4..8], &[3, 2, 3, 2]);
    }

    fn patterned_bayer_raw() -> LoadedRaw {
        let mut raw = test_raw(8, 8);
        raw.color_indices = CompactPixelMap::repeating(8, 8, 2, 2, vec![0, 1, 3, 2]);
        raw
    }

    #[test]
    fn cfa_proxy_can_hide_a_native_clipped_photosite() {
        let mut raw = patterned_bayer_raw();
        raw.raw_pixels.fill(100);
        raw.raw_pixels[0] = 1023;

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 2 });

        assert_eq!(raw.raw_pixels.iter().copied().max(), Some(1023));
        assert!(
            proxy.raw_pixels.iter().all(|sample| *sample < 1023),
            "pre-reconstruction averaging must not be treated as a clipping reference"
        );
    }

    #[test]
    fn cfa_proxy_erases_fine_phase_detail_and_reduces_shadow_variance() {
        let mut raw = patterned_bayer_raw();
        for y in 0..raw.height {
            for x in 0..raw.width {
                // A two-pixel coloured/checker structure: each CFA phase sees
                // alternating shadow values, while every proxy footprint sees
                // the same mean.
                raw.raw_pixels[(y * raw.width + x) as usize] =
                    if (x / 2 + y / 2) % 2 == 0 { 300 } else { 500 };
            }
        }

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 2 });
        let variance = |samples: &[u16]| {
            let mean =
                samples.iter().map(|value| f64::from(*value)).sum::<f64>() / samples.len() as f64;
            samples
                .iter()
                .map(|value| (f64::from(*value) - mean).powi(2))
                .sum::<f64>()
                / samples.len() as f64
        };

        assert!(raw.raw_pixels.contains(&300) && raw.raw_pixels.contains(&500));
        assert_eq!(proxy.raw_pixels, vec![400; 4]);
        assert!(variance(&raw.raw_pixels) > 0.0);
        assert_eq!(variance(&proxy.raw_pixels), 0.0);
    }

    #[test]
    fn proxy_long_edge_does_not_drop_at_integer_scale_thresholds() {
        let larger = build_proxy(&test_raw(82, 54), ProxySpec { max_edge: 26 });
        let smaller = build_proxy(&test_raw(70, 46), ProxySpec { max_edge: 26 });
        let portrait = build_proxy(&test_raw(54, 82), ProxySpec { max_edge: 26 });

        assert_eq!(larger.width.max(larger.height), 26);
        assert_eq!(smaller.width.max(smaller.height), 26);
        assert_eq!(portrait.width.max(portrait.height), 26);
        assert_eq!(larger.width % 2, 0);
        assert_eq!(larger.height % 2, 0);
    }

    #[test]
    fn fractional_xtrans_proxy_keeps_complete_six_by_six_phases() {
        let pattern = vec![
            0, 1, 0, 0, 1, 0, 1, 2, 1, 2, 1, 2, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 2, 1, 2, 1,
            2, 0, 1, 0, 0, 1, 0,
        ];
        let mut raw = test_raw(98, 66);
        raw.cfa_kind = CfaKind::XTrans;
        raw.color_indices = CompactPixelMap::repeating(98, 66, 6, 6, pattern.clone());

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 38 });
        assert!(proxy.width.max(proxy.height) >= 32);
        assert_eq!(proxy.width % 6, 0);
        assert_eq!(proxy.height % 6, 0);
        let proxy_cfa = &proxy.color_indices;
        let proxy_width = proxy.width;
        let first_period = (0..6)
            .flat_map(|y| (0..6).map(move |x| proxy_cfa[(y * proxy_width + x) as usize]))
            .collect::<Vec<_>>();
        assert_eq!(first_period, pattern);
    }

    #[test]
    fn xtrans_proxy_filters_each_output_pixel_not_a_whole_six_pixel_macrocell() {
        let pattern = vec![
            0, 1, 0, 0, 1, 0, 1, 2, 1, 2, 1, 2, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 2, 1, 2, 1,
            2, 0, 1, 0, 0, 1, 0,
        ];
        let mut raw = test_raw(120, 120);
        raw.cfa_kind = CfaKind::XTrans;
        raw.color_indices = CompactPixelMap::repeating(120, 120, 6, 6, pattern);
        raw.raw_pixels = (0..120)
            .flat_map(|_| (0..120).map(|x| (x * 100) as u16))
            .collect();

        let proxy = build_proxy(&raw, ProxySpec { max_edge: 60 });
        assert!(proxy.raw_pixels[0] <= 200, "left sample was over-blurred");
        assert!(proxy.raw_pixels[5] >= 900, "right sample was over-blurred");
    }
}
