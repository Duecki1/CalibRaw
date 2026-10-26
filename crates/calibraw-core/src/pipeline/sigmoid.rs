// SPDX-License-Identifier: GPL-3.0-or-later
// Adapted from darktable 5.6.0 sigmoid.
// Copyright (C) 2020-2026 darktable developers.
// Copyright (C) 2026 CalibRaw contributors (Rust adaptation).

pub const MIDDLE_GREY: f32 = 0.1845;
const CONTRAST_SLOPE_CALIBRATION: f32 = 0.9939394;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum SigmoidColorProcessing {
    #[default]
    PerChannel,
    RgbRatio,
}

impl SigmoidColorProcessing {
    pub const fn shader_value(self) -> f32 {
        match self {
            Self::PerChannel => 0.0,
            Self::RgbRatio => 1.0,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::PerChannel => "Per channel",
            Self::RgbRatio => "RGB ratio",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SigmoidParams {
    pub contrast: f32,
    pub skew: f32,
    pub display_white_target: f32,
    pub display_black_target: f32,
    pub color_processing: SigmoidColorProcessing,
    pub hue_preservation: f32,
}

impl Default for SigmoidParams {
    fn default() -> Self {
        Self {
            contrast: 1.5,
            skew: 0.0,
            display_white_target: 100.0,
            display_black_target: 0.0152,
            color_processing: SigmoidColorProcessing::PerChannel,
            hue_preservation: 100.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SigmoidCoefficients {
    pub white_target: f32,
    pub black_target: f32,
    pub paper_exposure: f32,
    pub film_fog: f32,
    pub film_power: f32,
    pub paper_power: f32,
    pub hue_preservation: f32,
    pub color_processing: f32,
}

fn generalized_loglogistic_sigmoid(
    value: f32,
    magnitude: f32,
    log2_paper_exposure: f32,
    film_fog: f32,
    film_power: f32,
    paper_power: f32,
) -> f32 {
    let clamped_value = value.max(0.0);
    let film_base = film_fog + clamped_value;
    let log2_film_response = if film_base > 0.0 {
        film_power * film_base.log2()
    } else {
        f32::NEG_INFINITY
    };
    let log2_ratio = log2_film_response - log2_paper_exposure;
    let ratio = if log2_ratio >= 0.0 {
        1.0 / (1.0 + (-log2_ratio).exp2())
    } else {
        let scaled = log2_ratio.exp2();
        scaled / (1.0 + scaled)
    };
    let paper_response = magnitude * ratio.powf(paper_power);
    if paper_response.is_finite() {
        paper_response
    } else if magnitude.is_finite() {
        magnitude.max(0.0)
    } else {
        1.0
    }
}

const DEFAULT_SIGMOID_COEFFICIENTS: SigmoidCoefficients = SigmoidCoefficients {
    white_target: 1.0,
    black_target: 0.000_152,
    paper_exposure: -1.475_152_1,
    film_fog: 0.001_384_322_1,
    film_power: 1.4909091,
    paper_power: 1.0,
    hue_preservation: 1.0,
    color_processing: 0.0,
};

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn coefficients_are_valid(coefficients: SigmoidCoefficients) -> bool {
    let values = [
        coefficients.white_target,
        coefficients.black_target,
        coefficients.paper_exposure,
        coefficients.film_fog,
        coefficients.film_power,
        coefficients.paper_power,
        coefficients.hue_preservation,
        coefficients.color_processing,
    ];
    if !values.into_iter().all(f32::is_finite)
        || coefficients.white_target <= 0.0
        || coefficients.black_target < 0.0
        || coefficients.black_target >= coefficients.white_target
        || coefficients.film_fog < 0.0
        || coefficients.film_power <= 0.0
        || coefficients.paper_power <= 0.0
    {
        return false;
    }

    let curve = |x| {
        generalized_loglogistic_sigmoid(
            x,
            coefficients.white_target,
            coefficients.paper_exposure,
            coefficients.film_fog,
            coefficients.film_power,
            coefficients.paper_power,
        )
    };
    // Probe the shadow end, middle grey and a far HDR value.
    let black = curve(0.0);
    let grey = curve(MIDDLE_GREY);
    let bright = curve(1.0e6);
    black.is_finite()
        && grey.is_finite()
        && bright.is_finite()
        && black <= grey
        && grey <= bright
        && (grey - MIDDLE_GREY).abs() <= 5e-3
}

pub fn coefficients(params: SigmoidParams) -> SigmoidCoefficients {
    let defaults = SigmoidParams::default();
    let contrast = finite_or(params.contrast, defaults.contrast).clamp(0.1, 10.0);
    let skew = finite_or(params.skew, defaults.skew).clamp(-1.0, 1.0);
    let display_white_target =
        finite_or(params.display_white_target, defaults.display_white_target).clamp(20.0, 1600.0);
    let display_black_target =
        finite_or(params.display_black_target, defaults.display_black_target).clamp(0.0, 15.0);
    let hue_preservation =
        (0.01 * finite_or(params.hue_preservation, defaults.hue_preservation)).clamp(0.0, 1.0);
    let color_processing = params.color_processing.shader_value();

    let ref_slope = contrast * CONTRAST_SLOPE_CALIBRATION * (1.0 - MIDDLE_GREY);

    let paper_power = 5.0f32.powf(-skew);
    let temp_white_target = 0.01 * display_white_target;
    let temp_white_grey_relation = (temp_white_target / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
    let temp_ratio = 1.0 / (1.0 + temp_white_grey_relation);
    let temp_slope = paper_power * (1.0 - temp_ratio);

    let film_power = ref_slope / temp_slope;
    let white_target = 0.01 * display_white_target;
    let black_target = 0.01 * display_black_target;
    let white_grey_relation = (white_target / MIDDLE_GREY).powf(1.0 / paper_power) - 1.0;
    let white_black_relation = if black_target == 0.0 {
        f32::INFINITY
    } else {
        (black_target / white_target).powf(-1.0 / paper_power) - 1.0
    };
    let film_fog = MIDDLE_GREY * white_grey_relation.powf(1.0 / film_power)
        / (white_black_relation.powf(1.0 / film_power)
            - white_grey_relation.powf(1.0 / film_power));
    let paper_exposure = film_power * (film_fog + MIDDLE_GREY).log2() + white_grey_relation.log2();

    let candidate = SigmoidCoefficients {
        white_target,
        black_target,
        paper_exposure,
        film_fog,
        film_power,
        paper_power,
        hue_preservation,
        color_processing,
    };
    if coefficients_are_valid(candidate) {
        candidate
    } else {
        SigmoidCoefficients {
            hue_preservation,
            color_processing,
            ..DEFAULT_SIGMOID_COEFFICIENTS
        }
    }
}
