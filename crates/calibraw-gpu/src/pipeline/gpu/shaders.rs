//! Every WGSL source the processing pipeline compiles, declared once.
//!
//! Composable modules are registered with naga_oil, in dependency order,
//! under an import path derived from the file name (`xtrans/seed.wgsl` is
//! imported as `calibraw::xtrans::seed`). Entry shaders become
//! `wgpu::ShaderModule`s. Sources marked `work_format` declare their storage
//! textures with the `CALIBRAW_WORK_FORMAT` marker and are specialized to the
//! processing quality's work format before use.

use crate::pipeline::CfaKind;
use std::borrow::Cow;

/// A WGSL file compiled into the binary.
#[derive(Clone, Copy, Debug)]
pub(super) struct ShaderSource {
    /// Path below `src/shaders/`.
    pub(super) file_name: &'static str,
    /// The file's own text.
    pub(super) text: &'static str,
    /// Files appended to this one to form a single module.
    appended: &'static [ShaderSource],
}

impl ShaderSource {
    /// The complete module: this file followed by its appended files.
    pub(super) fn module_text(&self) -> Cow<'static, str> {
        if self.appended.is_empty() {
            return Cow::Borrowed(self.text);
        }
        let mut text = String::from(self.text);
        for part in self.appended {
            text.push('\n');
            text.push_str(part.text);
        }
        Cow::Owned(text)
    }

    /// The naga_oil import path other modules use, e.g. `calibraw::tonemap`.
    pub(super) fn import_path(&self) -> String {
        format!(
            "calibraw::{}",
            self.file_name.trim_end_matches(".wgsl").replace('/', "::")
        )
    }
}

macro_rules! wgsl {
    ($file:literal) => {
        wgsl!($file, &[])
    };
    ($file:literal, $appended:expr) => {
        ShaderSource {
            file_name: $file,
            text: include_str!(concat!("../../shaders/", $file)),
            appended: $appended,
        }
    };
}

pub(super) const COMMON: ShaderSource = wgsl!("common.wgsl");
pub(super) const COLOR: ShaderSource = wgsl!("color.wgsl");
pub(super) const NOISE: ShaderSource = wgsl!("noise.wgsl");
pub(super) const RAW_SAMPLING: ShaderSource = wgsl!("raw_sampling.wgsl");
pub(super) const PROFILE: ShaderSource = wgsl!("profile.wgsl");
pub(super) const BASIC_ADJUSTMENTS: ShaderSource = wgsl!("basic_adjustments.wgsl");
pub(super) const TONE_COMMON: ShaderSource = wgsl!("tone_common.wgsl");
pub(super) const TONEMAP: ShaderSource = wgsl!("tonemap.wgsl");
pub(super) const NOISE_CA_FINISH: ShaderSource = wgsl!("noise_ca_finish.wgsl");
pub(super) const DETAIL_UTILS: ShaderSource = wgsl!("detail_utils.wgsl");
pub(super) const DETAIL_CAPTURE: ShaderSource = wgsl!("detail_capture.wgsl");
pub(super) const DETAIL_SCALE_SPACE: ShaderSource = wgsl!("detail_scale_space.wgsl");
pub(super) const XTRANS_SEED: ShaderSource = wgsl!("xtrans/seed.wgsl");
pub(super) const XTRANS_MARKESTEIJN_INTERPOLATE: ShaderSource =
    wgsl!("xtrans/markesteijn_interpolate.wgsl");
pub(super) const XTRANS_MARKESTEIJN_REFINE: ShaderSource = wgsl!("xtrans/markesteijn_refine.wgsl");
pub(super) const XTRANS_MARKESTEIJN_CANDIDATES: ShaderSource =
    wgsl!("xtrans/markesteijn_candidates.wgsl");
pub(super) const XTRANS_MARKESTEIJN_DERIVATIVES: ShaderSource =
    wgsl!("xtrans/markesteijn_derivatives.wgsl");
pub(super) const XTRANS_MARKESTEIJN_HOMOGENEITY: ShaderSource =
    wgsl!("xtrans/markesteijn_homogeneity.wgsl");
pub(super) const XTRANS_MARKESTEIJN_ACCUMULATE: ShaderSource =
    wgsl!("xtrans/markesteijn_accumulate.wgsl");

pub(super) const MASK_EFFECTS_SHARED: ShaderSource = wgsl!("mask_effects/shared.wgsl");
pub(super) const MASK_LENS_BLUR: ShaderSource = wgsl!("mask_effects/lens_blur.wgsl");
pub(super) const MASK_MOTION_BLUR: ShaderSource = wgsl!("mask_effects/motion_blur.wgsl");
pub(super) const MASK_RADIAL_BLUR: ShaderSource = wgsl!("mask_effects/radial_blur.wgsl");
pub(super) const MASK_TILT_SHIFT: ShaderSource = wgsl!("mask_effects/tilt_shift.wgsl");
pub(super) const MASK_BLUR: ShaderSource = wgsl!("mask_effects/blur.wgsl");
pub(super) const MASK_EDGE_GLOW: ShaderSource = wgsl!("mask_effects/edge_glow.wgsl");
pub(super) const MASK_GLOW: ShaderSource = wgsl!("mask_effects/glow.wgsl");
pub(super) const MASK_NEON: ShaderSource = wgsl!("mask_effects/neon.wgsl");
pub(super) const MASK_PIXELATE: ShaderSource = wgsl!("mask_effects/pixelate.wgsl");
pub(super) const MASK_LIGHT_RAYS: ShaderSource = wgsl!("mask_effects/light_rays.wgsl");
pub(super) const MASK_ATMOSPHERE: ShaderSource = wgsl!("mask_effects/atmosphere.wgsl");
pub(super) const MASK_FILM_FINISH: ShaderSource = wgsl!("mask_effects/film_finish.wgsl");

/// Creative effects and every mask effect, compiled as one module because the
/// effects share private helpers and the creative pass dispatches to them.
pub(super) const CREATIVE_EFFECTS: ShaderSource = wgsl!(
    "creative_effects.wgsl",
    &[
        MASK_EFFECTS_SHARED,
        MASK_LENS_BLUR,
        MASK_MOTION_BLUR,
        MASK_RADIAL_BLUR,
        MASK_TILT_SHIFT,
        MASK_BLUR,
        MASK_EDGE_GLOW,
        MASK_GLOW,
        MASK_NEON,
        MASK_PIXELATE,
        MASK_LIGHT_RAYS,
        MASK_ATMOSPHERE,
        MASK_FILM_FINISH,
    ]
);

pub(super) const HIGHLIGHTS: ShaderSource = wgsl!("highlights.wgsl");
pub(super) const BAYER_RCD_P1: ShaderSource = wgsl!("pass1.wgsl");
pub(super) const BAYER_RCD_P2: ShaderSource = wgsl!("pass2.wgsl");
pub(super) const BAYER_RCD_P3: ShaderSource = wgsl!("pass3.wgsl");
pub(super) const BAYER_RCD_P4: ShaderSource = wgsl!("pass4.wgsl");
pub(super) const DUAL_DEMOSAIC: ShaderSource = wgsl!("dual_demosaic.wgsl");
pub(super) const XTRANS_DEMOSAIC: ShaderSource = wgsl!("xtrans_demosaic.wgsl");
pub(super) const XTRANS_FINISH: ShaderSource = wgsl!("xtrans_finish.wgsl");
pub(super) const COLOR_DENOISE: ShaderSource = wgsl!("color_denoise.wgsl");
pub(super) const TONE_ANALYSIS: ShaderSource = wgsl!("tone_analysis.wgsl");
pub(super) const SCENE_ADJUSTMENTS: ShaderSource = wgsl!("scene_adjustments.wgsl");
pub(super) const VIEW_TRANSFORM: ShaderSource = wgsl!("view_transform.wgsl");
pub(super) const REMOVE_COMPOSITE: ShaderSource = wgsl!("remove_composite.wgsl");

/// A module other shaders import.
pub(super) struct ComposableModule {
    pub(super) source: ShaderSource,
    /// Registered only for this sensor layout.
    pub(super) sensor: Option<CfaKind>,
    /// Specialize `CALIBRAW_WORK_FORMAT` markers before registering.
    pub(super) work_format: bool,
}

const fn module(source: ShaderSource) -> ComposableModule {
    ComposableModule {
        source,
        sensor: None,
        work_format: false,
    }
}

const fn xtrans_module(source: ShaderSource) -> ComposableModule {
    ComposableModule {
        source,
        sensor: Some(CfaKind::XTrans),
        work_format: false,
    }
}

/// Composable modules in registration order: a module may only import
/// modules listed before it.
pub(super) const COMPOSABLE_MODULES: [ComposableModule; 21] = [
    module(COMMON),
    module(COLOR),
    module(NOISE),
    module(RAW_SAMPLING),
    xtrans_module(XTRANS_SEED),
    xtrans_module(XTRANS_MARKESTEIJN_INTERPOLATE),
    xtrans_module(XTRANS_MARKESTEIJN_REFINE),
    xtrans_module(XTRANS_MARKESTEIJN_CANDIDATES),
    xtrans_module(XTRANS_MARKESTEIJN_DERIVATIVES),
    xtrans_module(XTRANS_MARKESTEIJN_HOMOGENEITY),
    xtrans_module(XTRANS_MARKESTEIJN_ACCUMULATE),
    module(PROFILE),
    module(BASIC_ADJUSTMENTS),
    module(TONE_COMMON),
    module(TONEMAP),
    module(NOISE_CA_FINISH),
    module(DETAIL_UTILS),
    module(DETAIL_CAPTURE),
    ComposableModule {
        source: SCENE_ADJUSTMENTS,
        sensor: None,
        work_format: true,
    },
    module(DETAIL_SCALE_SPACE),
    module(CREATIVE_EFFECTS),
];

/// A shader compiled into its own `wgpu::ShaderModule`.
#[derive(Clone, Copy, Debug)]
pub(super) struct EntryShader {
    pub(super) label: &'static str,
    pub(super) source: ShaderSource,
    /// Specialize `CALIBRAW_WORK_FORMAT` markers before compiling.
    pub(super) work_format: bool,
    /// Compiled only for this sensor layout.
    pub(super) sensor: Option<CfaKind>,
}

impl EntryShader {
    /// Whether a pipeline for `cfa_kind` compiles this shader.
    pub(super) fn applies_to(&self, cfa_kind: CfaKind) -> bool {
        self.sensor.is_none_or(|sensor| sensor == cfa_kind)
    }
}

const fn entry(
    label: &'static str,
    source: ShaderSource,
    work_format: bool,
    sensor: Option<CfaKind>,
) -> EntryShader {
    EntryShader {
        label,
        source,
        work_format,
        sensor,
    }
}

pub(super) const HIGHLIGHTS_ENTRY: EntryShader =
    entry("calibraw highlight module", HIGHLIGHTS, false, None);
pub(super) const BAYER_RCD_P1_ENTRY: EntryShader = entry(
    "calibraw Bayer RCD pass 1",
    BAYER_RCD_P1,
    true,
    Some(CfaKind::Bayer),
);
pub(super) const BAYER_RCD_P2_ENTRY: EntryShader = entry(
    "calibraw Bayer RCD pass 2",
    BAYER_RCD_P2,
    true,
    Some(CfaKind::Bayer),
);
pub(super) const BAYER_RCD_P3_ENTRY: EntryShader = entry(
    "calibraw Bayer RCD pass 3",
    BAYER_RCD_P3,
    true,
    Some(CfaKind::Bayer),
);
pub(super) const BAYER_RCD_P4_ENTRY: EntryShader = entry(
    "calibraw Bayer RCD pass 4",
    BAYER_RCD_P4,
    true,
    Some(CfaKind::Bayer),
);
pub(super) const DUAL_DEMOSAIC_ENTRY: EntryShader =
    entry("calibraw robust dual demosaic", DUAL_DEMOSAIC, true, None);
pub(super) const XTRANS_DEMOSAIC_ENTRY: EntryShader = entry(
    "calibraw grouped X-Trans demosaic",
    XTRANS_DEMOSAIC,
    true,
    Some(CfaKind::XTrans),
);
pub(super) const XTRANS_FINISH_ENTRY: EntryShader = entry(
    "calibraw X-Trans finish",
    XTRANS_FINISH,
    true,
    Some(CfaKind::XTrans),
);
pub(super) const COLOR_DENOISE_ENTRY: EntryShader = entry(
    "calibraw multiscale color denoise",
    COLOR_DENOISE,
    true,
    None,
);
pub(super) const TONE_ANALYSIS_ENTRY: EntryShader =
    entry("calibraw tone analysis", TONE_ANALYSIS, false, None);
pub(super) const SCENE_ADJUSTMENTS_ENTRY: EntryShader =
    entry("calibraw scene adjustments", SCENE_ADJUSTMENTS, true, None);
pub(super) const CREATIVE_EFFECTS_ENTRY: EntryShader =
    entry("calibraw creative effects", CREATIVE_EFFECTS, false, None);
pub(super) const VIEW_TRANSFORM_ENTRY: EntryShader =
    entry("calibraw view transform", VIEW_TRANSFORM, false, None);
/// Compiled directly with wgpu (it imports nothing), outside the composer.
pub(super) const REMOVE_COMPOSITE_ENTRY: EntryShader = entry(
    "calibraw Remove composite shader",
    REMOVE_COMPOSITE,
    true,
    None,
);

/// Every entry shader, for validation and layout tests.
#[cfg(test)]
pub(super) const ALL_ENTRY_SHADERS: [EntryShader; 14] = [
    HIGHLIGHTS_ENTRY,
    BAYER_RCD_P1_ENTRY,
    BAYER_RCD_P2_ENTRY,
    BAYER_RCD_P3_ENTRY,
    BAYER_RCD_P4_ENTRY,
    DUAL_DEMOSAIC_ENTRY,
    XTRANS_DEMOSAIC_ENTRY,
    XTRANS_FINISH_ENTRY,
    COLOR_DENOISE_ENTRY,
    TONE_ANALYSIS_ENTRY,
    SCENE_ADJUSTMENTS_ENTRY,
    CREATIVE_EFFECTS_ENTRY,
    VIEW_TRANSFORM_ENTRY,
    REMOVE_COMPOSITE_ENTRY,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_paths_follow_file_names() {
        assert_eq!(COMMON.import_path(), "calibraw::common");
        assert_eq!(
            XTRANS_MARKESTEIJN_REFINE.import_path(),
            "calibraw::xtrans::markesteijn_refine"
        );
    }

    #[test]
    fn creative_effects_module_appends_every_mask_effect_in_order() {
        let text = CREATIVE_EFFECTS.module_text();
        assert!(text.starts_with(CREATIVE_EFFECTS.text));
        let mut position = CREATIVE_EFFECTS.text.len();
        for part in CREATIVE_EFFECTS.appended {
            let found = text[position..]
                .find(part.text)
                .unwrap_or_else(|| panic!("{} is appended", part.file_name));
            position += found + part.text.len();
        }
        assert_eq!(position, text.len());
    }

    #[test]
    fn work_format_entries_are_exactly_those_with_markers() {
        for shader in ALL_ENTRY_SHADERS {
            assert_eq!(
                shader.work_format,
                shader
                    .source
                    .text
                    .contains(super::super::WORK_FORMAT_MARKER),
                "{}",
                shader.label
            );
        }
    }
}
