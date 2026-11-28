//! The edit as the uniforms of a colour or black-and-white photo carry it
//! ([`Treatment`], whose documentation lists what each renders).
//!
//! A black-and-white photo's view node mixes the grey where the colour
//! mixers run (`apply_monochrome_mix` in view_transform.wgsl) and reads the
//! mix from the colour mixer's Luminance slots, the global one and each
//! mask's, which only the view node reads. The controls that need colour
//! pack at rest, so the shaders leave them out without knowing the treatment.
//!
//! [`Treatment`]: crate::pipeline::Treatment

use crate::pipeline::{ExposureParams, LocalAdjustments, PointColors};

/// `exposure`, the edit as it renders ([`ExposureParams::applied`]), as its
/// treatment packs it: in black and white with the mix in the colour mixer's
/// Luminance and Saturation, Vibrance, Hue, the mixer's Hue and Saturation and
/// Point Color at rest; in colour without the mix.
pub(super) fn treated_exposure(exposure: &ExposureParams) -> ExposureParams {
    if !exposure.renders_monochrome() {
        return ExposureParams {
            monochrome_mix: [0.0; 8],
            ..*exposure
        };
    }
    ExposureParams {
        saturation: 0.0,
        vibrance: 0.0,
        hue: 0.0,
        hsl_hue: [0.0; 8],
        hsl_saturation: [0.0; 8],
        hsl_luminance: exposure.monochrome_mix,
        monochrome_mix: [0.0; 8],
        point_colors: PointColors::default(),
        point_color_visualize: None,
        ..*exposure
    }
}

/// A mask's `adjustments` as the treatment of its photo packs them, like
/// [`treated_exposure`]: in black and white (`monochrome`, the photo's
/// [`ExposureParams::renders_monochrome`]) with the mix in the colour mixer's
/// Luminance and Hue, Saturation, the mixer's Hue and Saturation and Point
/// Color at rest; in colour without the mix.
pub(super) fn treated_adjustments(
    adjustments: LocalAdjustments,
    monochrome: bool,
) -> LocalAdjustments {
    if !monochrome {
        return LocalAdjustments {
            monochrome_mix: [0.0; 8],
            ..adjustments
        };
    }
    LocalAdjustments {
        hue: 0.0,
        saturation: 0.0,
        hsl_hue: [0.0; 8],
        hsl_saturation: [0.0; 8],
        hsl_luminance: adjustments.monochrome_mix,
        monochrome_mix: [0.0; 8],
        point_colors: PointColors::default(),
        point_color_visualize: None,
        ..adjustments
    }
}
