use std::ops::RangeInclusive;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatParamSpec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub step: f64,
    pub decimals: usize,
    pub tooltip: Option<&'static str>,
}

impl FloatParamSpec {
    pub fn range(self) -> RangeInclusive<f32> {
        self.min..=self.max
    }

    pub fn clamp(self, value: f32) -> f32 {
        value.clamp(self.min, self.max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorParamSpec {
    pub label: &'static str,
    pub title: &'static str,
    pub default: [f32; 3],
    pub min: f32,
    pub max: f32,
    pub tooltip: &'static str,
}

impl ColorParamSpec {
    pub fn clamp(self, color: [f32; 3]) -> [f32; 3] {
        color.map(|channel| channel.clamp(self.min, self.max))
    }
}

macro_rules! float_param {
    (
        $name:ident,
        $label:literal,
        $min:expr,
        $max:expr,
        $default:expr,
        $step:expr,
        $decimals:expr,
        $tooltip:expr
        $(,)?
    ) => {
        pub const $name: FloatParamSpec = FloatParamSpec {
            label: $label,
            min: $min,
            max: $max,
            default: $default,
            step: $step,
            decimals: $decimals,
            tooltip: $tooltip,
        };
    };
}

macro_rules! color_param {
    ($name:ident, $label:literal, $title:literal, $default:expr, $tooltip:literal $(,)?) => {
        pub const $name: ColorParamSpec = ColorParamSpec {
            label: $label,
            title: $title,
            default: $default,
            min: 0.0,
            max: 1.0,
            tooltip: $tooltip,
        };
    };
}

pub mod adjustment {
    use super::*;
    use crate::pipeline::basicadj::HUE_ROTATION_LIMIT_DEGREES;

    float_param!(EXPOSURE, "Exposure", -5.0, 5.0, 0.0, 0.05, 2, None);
    float_param!(CONTRAST, "Contrast", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(HIGHLIGHTS, "Highlights", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(SHADOWS, "Shadows", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(WHITES, "Whites", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(BLACKS, "Blacks", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(TEMPERATURE, "Temperature", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(TINT, "Tint", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(
        HUE,
        "Hue",
        -HUE_ROTATION_LIMIT_DEGREES,
        HUE_ROTATION_LIMIT_DEGREES,
        0.0,
        1.0,
        1,
        Some("Rotates colors inside the mask around the perceptual color wheel."),
    );
    float_param!(SATURATION, "Saturation", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(TEXTURE, "Texture", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(CLARITY, "Clarity", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(DEHAZE, "Dehaze", -100.0, 100.0, 0.0, 1.0, 0, None);
    float_param!(
        HALATION,
        "Halation",
        0.0,
        100.0,
        0.0,
        1.0,
        0,
        Some("Adds a warm film halo around bright edges inside the mask.")
    );
}

pub mod blur {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls how strongly the blur softens the photograph."),
    );
    float_param!(
        RADIUS,
        "Radius",
        0.0,
        16.0,
        8.0,
        0.1,
        1,
        Some("Sets the width of the blur. Larger values soften broader details."),
    );
}

pub mod lens_blur {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls how strongly the photograph falls out of focus."),
    );
    float_param!(
        RADIUS,
        "Radius",
        0.0,
        48.0,
        12.0,
        0.1,
        1,
        Some("Sets the size of out-of-focus highlights and the overall blur."),
    );
    float_param!(
        BLADES,
        "Aperture blades",
        3.0,
        12.0,
        6.0,
        1.0,
        0,
        Some("Sets the number of aperture blades that shape out-of-focus highlights."),
    );
    float_param!(
        ROTATION,
        "Bokeh rotation",
        -180.0,
        180.0,
        0.0,
        1.0,
        0,
        Some("Rotates the shape of out-of-focus highlights."),
    );
    float_param!(
        HIGHLIGHTS,
        "Highlight boost",
        0.0,
        100.0,
        0.0,
        0.5,
        0,
        Some("Makes bright out-of-focus highlights stand out."),
    );
}

pub mod motion_blur {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the strength of the motion streaks."),
    );
    float_param!(
        DISTANCE,
        "Trail length",
        0.0,
        96.0,
        32.0,
        0.1,
        1,
        Some("Sets the length of the motion streaks, like a longer shutter exposure."),
    );
    float_param!(
        ANGLE,
        "Angle",
        -180.0,
        180.0,
        0.0,
        1.0,
        0,
        Some("Sets the direction of motion.")
    );
}

pub mod radial_blur {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the strength of the zoom or spin blur."),
    );
    float_param!(
        STRENGTH,
        "Trail length",
        0.0,
        96.0,
        36.0,
        0.1,
        1,
        Some("Sets how far details streak away from or around the blur center."),
    );
    float_param!(
        CENTER_X,
        "Center X",
        -50.0,
        150.0,
        50.0,
        1.0,
        0,
        Some("Horizontal origin in the full image; values may extend beyond the frame."),
    );
    float_param!(
        CENTER_Y,
        "Center Y",
        -50.0,
        150.0,
        50.0,
        1.0,
        0,
        Some("Vertical origin in the full image; values may extend beyond the frame."),
    );
}

pub mod tilt_shift {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        75.0,
        0.5,
        0,
        Some("Controls the maximum defocus strength outside the focus band."),
    );
    float_param!(
        RADIUS,
        "Radius",
        0.0,
        48.0,
        16.0,
        0.1,
        1,
        Some("Sets how far the foreground and background fall out of focus."),
    );
    float_param!(
        CENTER_X,
        "Center X",
        -50.0,
        150.0,
        50.0,
        1.0,
        0,
        Some("Horizontal position of a point on the sharp band."),
    );
    float_param!(
        CENTER_Y,
        "Center Y",
        -50.0,
        150.0,
        50.0,
        1.0,
        0,
        Some("Vertical position of a point on the sharp band."),
    );
    float_param!(
        ANGLE,
        "Angle",
        -180.0,
        180.0,
        0.0,
        1.0,
        0,
        Some("Rotates the in-focus band.")
    );
    float_param!(
        FOCUS_WIDTH,
        "Focus Width",
        0.0,
        100.0,
        24.0,
        0.5,
        0,
        Some("Width of the sharp band as a percentage of the image's shorter edge."),
    );
    float_param!(
        FEATHER,
        "Feather",
        0.1,
        100.0,
        18.0,
        0.1,
        1,
        Some("Softens the transition from sharp to defocused areas."),
    );
}

pub mod edge_glow {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the strength of the emitted edge light."),
    );
    float_param!(
        EDGE_WIDTH,
        "Edge Width",
        0.5,
        8.0,
        1.5,
        0.05,
        1,
        Some("Sets the thickness of the glowing outlines."),
    );
    float_param!(
        DETAIL,
        "Detail",
        0.0,
        100.0,
        35.0,
        0.5,
        0,
        Some("Higher values include finer, lower-contrast edges."),
    );
    float_param!(
        GLOW,
        "Glow",
        0.0,
        100.0,
        55.0,
        0.5,
        0,
        Some("Adds a broader halo around the detected edges."),
    );
    color_param!(
        COLOR,
        "Color",
        "Edge Glow color",
        [1.0, 0.42, 0.08],
        "Choose the color emitted by the Edge Glow effect.",
    );
}

pub mod glow {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the strength of the bright core and emitted halo."),
    );
    float_param!(
        RADIUS,
        "Radius",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls how far the glow spreads beyond the mask."),
    );
    float_param!(
        CORE,
        "Source brightness",
        0.0,
        100.0,
        65.0,
        0.5,
        0,
        Some("Makes the masked source brighter and more white-hot."),
    );
    color_param!(
        COLOR,
        "Color",
        "Glow color",
        [0.1, 0.65, 1.0],
        "Choose the color emitted by the Glow effect.",
    );
}

pub mod light_rays {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        35.0,
        0.5,
        0,
        Some("Controls the brightness of soft atmospheric light shafts emitted beyond the source mask."),
    );
    float_param!(
        LENGTH,
        "Length",
        0.0,
        200.0,
        100.0,
        1.0,
        0,
        Some("Controls how far light shafts reach beyond their sources, as a percentage of the image's shorter edge."),
    );
    float_param!(
        SOURCE_X, "Source X", -50.0, 150.0, 50.0, 1.0, 0,
        Some("Places the source horizontally, as a percentage of the full image width. Values outside 0–100 place it beyond the frame."),
    );
    float_param!(
        SOURCE_Y, "Source Y", -50.0, 150.0, 35.0, 1.0, 0,
        Some("Places the source vertically, as a percentage of the full image height. Values outside 0–100 place it beyond the frame."),
    );
    float_param!(
        SPREAD,
        "Spread",
        0.0,
        45.0,
        8.0,
        0.25,
        1,
        Some("Widens the light shafts around each source direction for broader atmospheric scattering."),
    );
    float_param!(
        FADE,
        "Fade",
        0.0,
        100.0,
        60.0,
        0.5,
        0,
        Some("Controls how quickly light shafts fade with distance from their sources. Higher values shorten the bright reach."),
    );
    float_param!(
        RAY_COUNT,
        "Ray Count",
        4.0,
        96.0,
        32.0,
        1.0,
        0,
        Some("Controls the approximate number of light shafts around the source. Higher values create finer rays."),
    );
    float_param!(
        VARIATION,
        "Variation",
        0.0,
        100.0,
        65.0,
        0.5,
        0,
        Some("Varies shaft brightness for irregular atmospheric rays. Lower values make emission more uniform."),
    );
    float_param!(
        SOFTNESS,
        "Softness",
        0.0,
        100.0,
        60.0,
        0.5,
        0,
        Some("Softens shaft edges and blends nearby source directions into a gentle atmospheric haze."),
    );
    color_param!(
        COLOR,
        "Color",
        "Light Rays color",
        [1.0, 0.85, 0.62],
        "Choose the color emitted by the Light Rays effect.",
    );
}

pub mod neon {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the strength of the emitted neon lines."),
    );
    float_param!(
        EDGE_WIDTH,
        "Edge Width",
        0.5,
        8.0,
        1.0,
        0.05,
        1,
        Some("Sets the thickness of the neon outlines."),
    );
    float_param!(
        DETAIL,
        "Detail",
        0.0,
        100.0,
        10.0,
        0.5,
        0,
        Some("Higher values include finer, lower-contrast edges."),
    );
    float_param!(
        GLOW,
        "Glow",
        0.0,
        100.0,
        10.0,
        0.5,
        0,
        Some("Adds a broader halo around the detected edge lines."),
    );
    float_param!(
        BACKGROUND,
        "Original image",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Retains the original image behind the Neon effect."),
    );
    color_param!(
        COLOR,
        "Color",
        "Neon color",
        [0.05, 0.85, 1.0],
        "Choose the emitted Neon color."
    );
}

pub mod pixelate {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        100.0,
        0.5,
        0,
        Some("Blends the pixelated result into the developed image."),
    );
    float_param!(
        BLOCK_SIZE,
        "Block Size",
        2.0,
        32.0,
        16.0,
        1.0,
        0,
        Some("Sets the size of the square blocks. Larger values hide more detail."),
    );
}

pub mod grain {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        25.0,
        0.5,
        0,
        Some("Controls the strength of the film grain. Zero leaves the image unchanged."),
    );
    float_param!(
        SIZE,
        "Size",
        0.5,
        4.0,
        1.0,
        0.05,
        2,
        Some("Sets the grain size, from fine texture to coarse film grain."),
    );
    float_param!(
        ROUGHNESS,
        "Roughness",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Varies the grain texture from smooth and even to rough and irregular."),
    );
    float_param!(
        COLOR,
        "Color",
        0.0,
        100.0,
        0.0,
        0.5,
        0,
        Some("Blends monochrome grain into colored grain."),
    );
    float_param!(
        SEED,
        "Pattern",
        0.0,
        1_000.0,
        0.0,
        1.0,
        0,
        Some("Chooses a different grain pattern that stays fixed between preview and export."),
    );
}

pub mod halation {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        25.0,
        0.5,
        0,
        Some("Adds a film-like halo around bright highlights. Zero leaves the image unchanged."),
    );
    float_param!(
        RADIUS,
        "Radius",
        0.0,
        32.0,
        8.0,
        0.1,
        1,
        Some("Controls how far the highlight halo spreads into surrounding tones."),
    );
    float_param!(
        THRESHOLD,
        "Highlight threshold",
        0.0,
        100.0,
        60.0,
        0.5,
        0,
        Some("Higher values limit the halo to brighter highlights."),
    );
    float_param!(
        WARMTH,
        "Warmth",
        0.0,
        100.0,
        75.0,
        0.5,
        0,
        Some("Shifts the halo from neutral light toward warm red and orange film tones."),
    );
}

pub mod vignette {
    use super::*;

    float_param!(
        AMOUNT, "Amount", -100.0, 100.0, -25.0, 0.5, 0,
        Some("Negative values darken the edges; positive values brighten them. Zero leaves the image unchanged."),
    );
    float_param!(
        MIDPOINT, "Midpoint", 0.0, 100.0, 50.0, 0.5, 0,
        Some("Sets how far the vignette reaches toward the center. Higher values keep more of the center clear."),
    );
    float_param!(
        ROUNDNESS,
        "Roundness",
        -100.0,
        100.0,
        0.0,
        0.5,
        0,
        Some("Changes the vignette shape from a rounded rectangle toward a circle."),
    );
    float_param!(
        FEATHER,
        "Feather",
        0.0,
        100.0,
        70.0,
        0.5,
        0,
        Some("Softens the transition between the clear center and the vignette."),
    );
    float_param!(
        HIGHLIGHTS,
        "Preserve highlights",
        0.0,
        100.0,
        30.0,
        0.5,
        0,
        Some("Protects bright highlights from a darkening vignette."),
    );
    float_param!(
        CENTER_X,
        "Center X",
        0.0,
        100.0,
        50.0,
        0.5,
        1,
        Some("Places the vignette center horizontally, as a percentage of the cropped and rotated frame width."),
    );
    float_param!(
        CENTER_Y,
        "Center Y",
        0.0,
        100.0,
        50.0,
        0.5,
        1,
        Some("Places the vignette center vertically, as a percentage of the cropped and rotated frame height."),
    );
}

pub mod fog {
    use super::*;

    float_param!(
        START,
        "Fog start",
        0.0,
        95.0,
        8.0,
        0.5,
        0,
        Some("Keeps the nearest part of the scene clear. Distance is relative to the depth map, not meters."),
    );
    float_param!(
        DEPTH_INFLUENCE,
        "Depth influence",
        0.0,
        100.0,
        100.0,
        0.5,
        0,
        Some("Makes fog accumulate with scene distance. Scene depth is generated automatically when needed."),
    );

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the overall strength of the atmospheric veil."),
    );
    float_param!(
        DENSITY,
        "Density",
        0.0,
        100.0,
        55.0,
        0.5,
        0,
        Some("Controls how quickly light is scattered as it travels through the fog."),
    );
    float_param!(
        SCALE,
        "Bank size",
        1.0,
        100.0,
        65.0,
        0.5,
        0,
        Some("Higher values create broader fog banks."),
    );
    float_param!(
        SOFTNESS,
        "Softness",
        0.0,
        100.0,
        70.0,
        0.5,
        0,
        Some("Softens the fog onset and the shape of the mist banks."),
    );
    float_param!(
        VARIATION,
        "Variation",
        0.0,
        100.0,
        30.0,
        0.5,
        0,
        Some("Varies density inside the fog volume. Zero creates uniform atmospheric haze."),
    );
    float_param!(
        SEED,
        "Pattern",
        0.0,
        1_000.0,
        0.0,
        1.0,
        0,
        Some("Chooses a different fog pattern that stays fixed between preview and export."),
    );
    color_param!(
        COLOR,
        "Color",
        "Fog color",
        [0.82, 0.87, 0.92],
        "Choose the color of the atmospheric veil.",
    );
}

pub mod smoke {
    use super::*;

    float_param!(
        AMOUNT,
        "Amount",
        0.0,
        100.0,
        50.0,
        0.5,
        0,
        Some("Controls the overall strength of the smoke overlay."),
    );
    float_param!(
        DENSITY,
        "Density",
        0.0,
        100.0,
        60.0,
        0.5,
        0,
        Some("Controls the opacity and body of the plumes."),
    );
    float_param!(
        SCALE,
        "Plume size",
        1.0,
        100.0,
        55.0,
        0.5,
        0,
        Some("Higher values create larger smoke plumes."),
    );
    float_param!(
        TURBULENCE,
        "Swirl",
        0.0,
        100.0,
        65.0,
        0.5,
        0,
        Some("Adds curls and distortion to the smoke."),
    );
    float_param!(
        SOFTNESS,
        "Softness",
        0.0,
        100.0,
        55.0,
        0.5,
        0,
        Some("Softens the boundaries of individual plumes."),
    );
    float_param!(
        ANGLE,
        "Flow direction",
        -180.0,
        180.0,
        -12.0,
        1.0,
        0,
        Some("Rotates the direction of the smoke flow."),
    );
    float_param!(
        SEED,
        "Pattern",
        0.0,
        1_000.0,
        0.0,
        1.0,
        0,
        Some("Chooses a different smoke pattern that stays fixed between preview and export."),
    );
    color_param!(
        COLOR,
        "Color",
        "Smoke color",
        [0.32, 0.34, 0.37],
        "Choose the color of the smoke plumes.",
    );
}
