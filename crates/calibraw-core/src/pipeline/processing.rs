use super::{
    CompactPixelMap, DemosaicMode, DenoiseQuality, ExposureParams, FilmSource,
    HighlightReconstructionMethod, LoadedRaw, MaskEffect, MaskStack, WhiteBalance,
};
use rayon::prelude::*;

mod ai_denoised_regions;
mod binning;
mod display_downscale;
mod proxy;
mod tiles;
use ai_denoised_regions::*;
pub use display_downscale::downscale_display_rgba8;
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

    if RawStageControls::of(before) != RawStageControls::of(after) {
        Some(ProcessingStage::Raw)
    } else {
        Some(ProcessingStage::Output)
    }
}

/// The settings that make the RAW reconstruction stage run again when they change.
#[derive(PartialEq)]
struct RawStageControls {
    black_point: f32,
    white_balance: WhiteBalance,
    chroma_denoise: f32,
    luminance_denoise: f32,
    denoise_detail: f32,
    denoise_quality: DenoiseQuality,
    ai_denoise_enabled: bool,
    ai_denoise_amount: f32,
    demosaic_mode: DemosaicMode,
    dual_threshold: f32,
    frequency_chroma: f32,
    highlight_method: HighlightReconstructionMethod,
    highlight_clip: f32,
    highlight_reconstruction: f32,
    chromatic_aberration: f32,
    /// Tone analysis measures the positive, so the stages it starts must run again.
    film: FilmSource,
}

impl RawStageControls {
    /// `ExposureParams` is destructured without `..`, so adding a field to it fails to
    /// compile here until it is classified: a RAW-stage control goes into the struct, a
    /// later-stage setting is bound to `_`.
    fn of(params: &ExposureParams) -> Self {
        let ExposureParams {
            black_point,
            exposure: _,
            contrast: _,
            sigmoid: _,
            white_balance,
            hue: _,
            saturation: _,
            vibrance: _,
            tone_curve: _,
            tone_curve_red: _,
            tone_curve_green: _,
            tone_curve_blue: _,
            chroma_denoise,
            luminance_denoise,
            denoise_detail,
            denoise_quality,
            ai_denoise_enabled,
            ai_denoise_amount,
            demosaic_mode,
            dual_threshold,
            frequency_chroma,
            highlight_method,
            highlight_clip,
            highlight_reconstruction,
            chromatic_aberration,
            film,
            highlights: _,
            shadows: _,
            whites: _,
            blacks: _,
            texture: _,
            clarity: _,
            dehaze: _,
            sharpen_amount: _,
            sharpen_radius: _,
            sharpen_detail: _,
            sharpen_masking: _,
            hsl_hue: _,
            hsl_saturation: _,
            hsl_luminance: _,
            point_colors: _,
            point_color_visualize: _,
            color_grading: _,
        } = *params;
        Self {
            black_point,
            white_balance,
            chroma_denoise,
            luminance_denoise,
            denoise_detail,
            denoise_quality,
            ai_denoise_enabled,
            ai_denoise_amount,
            demosaic_mode,
            dual_threshold,
            frequency_chroma,
            highlight_method,
            highlight_clip,
            highlight_reconstruction,
            chromatic_aberration,
            film,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_region_proxy, build_region_proxy_until, ProxySpec};
    use crate::pipeline::{CameraProfile, CfaKind, CompactPixelMap, LoadedRaw, SensorSaturation};

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
            source_extent: [width, height],
            camera_make: "Test".to_owned(),
            camera_model: String::new(),
            lens_make: String::new(),
            lens_model: String::new(),
            focal_length: 0.0,
            aperture: 0.0,
            focus_distance: 0.0,
            capture_metadata: Default::default(),
            cfa_kind: CfaKind::Bayer,
            raw_pixels: pixels.into(),
            binned_sensor: None,
            scene_linear_raster: None,
            scene_referred_raster: false,
            color_indices: CompactPixelMap::dense(
                width,
                height,
                vec![0; (width * height) as usize],
            ),
            wb_coeffs: [1.0; 4],
            profile_color: std::sync::Arc::new(crate::pipeline::ProfileColor {
                cam_to_srgb: [[0.0; 4]; 3],
                camera_profile: CameraProfile::default(),
                camera_profile_source: None,
                white_balance_model: None,
            }),
            color_source: None,
            black_levels: [0.0; 4],
            black_levels_per_pixel: CompactPixelMap::dense(
                width,
                height,
                vec![0.0; (width * height) as usize],
            ),
            white_levels: [1023.0; 4],
            sensor_saturation: SensorSaturation::AT_WHITE,
            noise_profile: crate::pipeline::NoiseProfile::default(),
            available_camera_profiles: Vec::new(),
            lens_geometry: None,
            lens_shading: None,
            lateral_chromatic_aberration: crate::pipeline::LateralChromaticAberration::NONE,
            ai_denoised: std::sync::Arc::new(std::sync::RwLock::new(None)),
            opposed_chroma_cache: Default::default(),
            opposed_chroma_source_identity: Default::default(),
            opposed_chroma_reference_source: true,
        }
    }

    #[test]
    fn a_cancelled_proxy_build_gives_up_and_a_live_one_matches_the_plain_build() {
        use std::sync::atomic::AtomicBool;
        let spec = ProxySpec { max_edge: 32 };
        for raw in [test_raster(96, 64), test_raw(96, 64)] {
            let raster = raw.is_pre_demosaiced_raster();
            assert!(
                build_region_proxy_until(&raw, 8, 4, 80, 56, spec, &AtomicBool::new(true))
                    .is_none(),
                "raster={raster}"
            );
            let built = build_region_proxy_until(&raw, 8, 4, 80, 56, spec, &AtomicBool::new(false))
                .expect("a flag that is not set does not cancel the build");
            let plain = build_region_proxy(&raw, 8, 4, 80, 56, spec);
            assert_eq!(
                (built.width, built.height),
                (plain.width, plain.height),
                "raster={raster}"
            );
            if raster {
                assert_eq!(built.scene_linear_raster(), plain.scene_linear_raster());
            } else {
                let binned = |raw: &LoadedRaw| raw.binned_sensor.as_ref().unwrap().rgb().to_vec();
                assert_eq!(binned(&built), binned(&plain));
            }
        }
    }
}
