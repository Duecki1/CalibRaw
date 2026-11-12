//! Glow: light that spreads from the bright parts of the mask, in one of five
//! styles (styled_glow.wgsl).
//!
//! The mask selects the pixels that emit; their light spreads past it. Every
//! style shares one model:
//!
//! - Range picks what emits: a pixel's light counts by a soft knee two stops
//!   wide around a threshold on its luminance, and counts whole, colour
//!   included, so a light just above the threshold glows too.
//! - Bloom, Diffusion and Halation spread it all around. Spread sets the
//!   widest scale of their point spread function, a sum of Gaussians one
//!   octave apart from [`FINEST_SCALE`] up to it.
//!   Bloom and Halation give every octave the same energy, which falls off
//!   as 1/r² like lens glare and film halation; Diffusion gives each octave
//!   half the energy of the next wider one, a wide, soft veil.
//! - The styles then add the spread light differently: Bloom adds it, so
//!   highlights keep their core; Diffusion moves the light it spreads away
//!   from where it was, keeping the total, which softens detail and lifts the
//!   shadows around lights; Halation adds the luminance that spreads beyond
//!   bright edges as a film's red layer would record it.
//! - Motion and Streak spread it along Direction in streaks, as a shutter
//!   drag (Motion, one way) or an anamorphic lens (Streak, both ways) would.
//!   Their kernel falls off along the streak as e^(−t/λ), the shape of
//!   Kawase's streak filter (DOUBLE-S.T.E.A.L, GDC 2003) that Blender's
//!   Streaks glare also builds; Spread sets the length λ. Across the streak it
//!   is a Gaussian whose deviation Softness sets.
//! - Every kernel holds unit energy, so spreading light farther does not add
//!   to it.
//! - Warmth tints the spread light: along the Planckian locus from cool to
//!   warm, or from yellow to deep red for Halation.
//!
//! Scales are fractions of the image's shorter edge, so the look does not
//! depend on the resolution. This module turns the controls into those
//! quantities; the shader only integrates the profile and composites.

use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::styled_glow::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// How a Glow looks, chosen with its card's Style control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum GlowStyle {
    /// A soft glow around highlights.
    #[default]
    Bloom,
    /// Dreamy softening across tones (the Orton look).
    Diffusion,
    /// A film's red halo around bright edges.
    Halation,
    /// Light trailing one way from highlights, as with a shutter drag.
    Motion,
    /// Light streaking both ways from highlights, as through an anamorphic
    /// lens.
    Streak,
}

impl GlowStyle {
    pub const ALL: [Self; 5] = [
        Self::Bloom,
        Self::Diffusion,
        Self::Halation,
        Self::Motion,
        Self::Streak,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bloom => "Bloom",
            Self::Diffusion => "Diffusion",
            Self::Halation => "Halation",
            Self::Motion => "Motion",
            Self::Streak => "Streak",
        }
    }

    /// Whether light spreads in streaks along a Direction rather than all
    /// around, which the card's Direction and Softness set.
    pub const fn streaks(self) -> bool {
        matches!(self, Self::Motion | Self::Streak)
    }

    /// `STYLE` lane value (`STYLED_GLOW_*` in styled_glow.wgsl).
    const fn shader_value(self) -> f32 {
        match self {
            Self::Bloom => 0.0,
            Self::Diffusion => 1.0,
            Self::Halation => 2.0,
            Self::Motion => 3.0,
            Self::Streak => 4.0,
        }
    }

    /// The style a slot's `STYLE` lane holds.
    pub fn from_shader_value(value: f32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|style| style.shader_value() == value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct StyledGlowEffectSettings {
    pub style: GlowStyle,
    pub amount: f32,
    pub range: f32,
    pub spread: f32,
    pub warmth: f32,
    /// Motion and Streak: the direction light streaks toward, in degrees.
    pub direction: f32,
    /// Motion and Streak: how far the streaks spread across their direction.
    pub softness: f32,
}

impl Default for StyledGlowEffectSettings {
    fn default() -> Self {
        Self {
            style: GlowStyle::Bloom,
            amount: AMOUNT.default,
            range: RANGE.default,
            spread: SPREAD.default,
            warmth: WARMTH.default,
            direction: DIRECTION.default,
            softness: SOFTNESS.default,
        }
    }
}

effect_lanes! {
    /// [`GlowStyle`] as 0 to 4.
    STYLE = 0,
    /// The share of the emitted light that is spread (or, for Halation, the
    /// gain on the halo's luminance).
    STRENGTH = 1,
    /// The Range threshold: log2 of the luminance at the knee's middle.
    THRESHOLD = 2,
    /// Bloom, Diffusion and Halation: log2 of the finest and the widest
    /// Gaussian deviation of the spread profile, in shorter edges.
    FINEST = 3,
    WIDEST = 4,
    /// Energy per octave grows as 2^(SLOPE × octave) toward the widest.
    SLOPE = 5,
    /// The tint of the spread light, scene-linear Rec.2020.
    TINT = 6 (color),
    /// Motion and Streak: the direction light streaks toward, in radians, in
    /// the full image's pixels with y down (clockwise on screen), as Motion
    /// Blur's Angle.
    DIRECTION = 9,
    /// Motion and Streak: the length λ over which a streak falls to 1/e, in
    /// shorter edges.
    LENGTH = 10,
    /// Motion and Streak: the Gaussian deviation across a streak, in shorter
    /// edges.
    SOFTNESS = 11,
}

/// Middle grey, the reference of the Range threshold.
const MIDDLE_GREY: f32 = 0.18;
/// Range 0 puts the knee this many stops above middle grey (only light
/// sources glow), and every 100 of Range lowers it by [`RANGE_STOPS`].
const RANGE_TOP_STOPS: f32 = 4.0;
const RANGE_STOPS: f32 = 7.0;
/// The finest Gaussian deviation of every style, in shorter edges.
const FINEST_SCALE: f32 = 0.0015;
/// The widest deviation at Spread 0, and the octaves Spread 100 adds, for
/// Bloom and Diffusion and for Halation, whose halo is set by the thickness
/// of the film base and stays short.
const GLOW_WIDEST_AT_ZERO: f32 = 0.003;
const GLOW_SPREAD_OCTAVES: f32 = 6.5;
const HALATION_WIDEST_AT_ZERO: f32 = FINEST_SCALE;
const HALATION_SPREAD_OCTAVES: f32 = 4.0;
/// Amount 100 adds the light that emits (Bloom), moves this share of it
/// (Diffusion), or adds this gain on the halo's luminance (Halation).
const BLOOM_STRENGTH: f32 = 1.0;
const DIFFUSION_STRENGTH: f32 = 0.6;
const HALATION_STRENGTH: f32 = 1.5;
/// Warmth ±100 moves the tint's temperature one octave from neutral.
const NEUTRAL_KELVIN: f32 = 6504.0;
const WARMTH_OCTAVES: f32 = 1.0;
/// Halation's tints in linear sRGB at Warmth -100, 0 and 100.
const HALATION_YELLOW: [f32; 3] = [1.0, 0.80, 0.10];
const HALATION_ORANGE: [f32; 3] = [1.0, 0.35, 0.06];
const HALATION_RED: [f32; 3] = [1.0, 0.08, 0.02];
/// The streak length λ at Spread 0 and at 100, in shorter edges; Spread
/// moves it on a log scale, about 0.1 at 50.
const STREAK_LENGTH_AT_ZERO: f32 = 0.02;
const STREAK_LENGTH_AT_FULL: f32 = 0.5;
/// The deviation across a streak at Softness 100, in shorter edges; it grows
/// with the square of Softness, so low values keep streaks crisp. At 0 a
/// streak is as thin as the full-image map holds it.
const STREAK_SOFTNESS_AT_FULL: f32 = 0.04;
/// Amount 100 adds the light that emits, spread along the streaks.
const STREAK_STRENGTH: f32 = 1.0;

impl StyledGlowEffectSettings {
    /// log2 of the widest Gaussian deviation of Bloom, Diffusion and
    /// Halation, in shorter edges.
    fn widest_octave(&self) -> f32 {
        let spread = SPREAD.clamp(self.spread) / 100.0;
        if self.style == GlowStyle::Halation {
            HALATION_WIDEST_AT_ZERO.log2() + HALATION_SPREAD_OCTAVES * spread
        } else {
            GLOW_WIDEST_AT_ZERO.log2() + GLOW_SPREAD_OCTAVES * spread
        }
    }

    /// The streak length λ of Motion and Streak, in shorter edges.
    fn streak_length(&self) -> f32 {
        let spread = SPREAD.clamp(self.spread) / 100.0;
        STREAK_LENGTH_AT_ZERO * (STREAK_LENGTH_AT_FULL / STREAK_LENGTH_AT_ZERO).powf(spread)
    }

    /// The deviation across a streak, in shorter edges.
    fn streak_softness(&self) -> f32 {
        let softness = SOFTNESS.clamp(self.softness) / 100.0;
        STREAK_SOFTNESS_AT_FULL * softness * softness
    }

    fn tint(&self) -> [f32; 3] {
        let warmth = WARMTH.clamp(self.warmth) / 100.0;
        match self.style {
            GlowStyle::Bloom | GlowStyle::Diffusion | GlowStyle::Motion | GlowStyle::Streak => {
                planckian_tint(NEUTRAL_KELVIN * (-WARMTH_OCTAVES * warmth).exp2())
            }
            GlowStyle::Halation => {
                let (from, to, t) = if warmth < 0.0 {
                    (HALATION_ORANGE, HALATION_YELLOW, -warmth)
                } else {
                    (HALATION_ORANGE, HALATION_RED, warmth)
                };
                crate::color_math::linear_srgb_to_rec2020(std::array::from_fn(|channel| {
                    from[channel] + (to[channel] - from[channel]) * t
                }))
            }
        }
    }
}

impl EffectSettings for StyledGlowEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        let mut controls = vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.range, RANGE),
            EffectControl::Slider(&mut self.spread, SPREAD),
            EffectControl::Slider(&mut self.warmth, WARMTH),
        ];
        if self.style.streaks() {
            controls.push(EffectControl::Angle(&mut self.direction, DIRECTION));
            controls.push(EffectControl::Slider(&mut self.softness, SOFTNESS));
        }
        EffectControls::new(controls)
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        let amount = AMOUNT.clamp(self.amount) / 100.0;
        let range = RANGE.clamp(self.range) / 100.0;
        slot.set(lanes::STYLE, self.style.shader_value());
        slot.set(
            lanes::THRESHOLD,
            MIDDLE_GREY.log2() + RANGE_TOP_STOPS - RANGE_STOPS * range,
        );
        slot.set_color(lanes::TINT, self.tint());
        let (strength, slope) = match self.style {
            GlowStyle::Bloom => (BLOOM_STRENGTH, 0.0),
            GlowStyle::Diffusion => (DIFFUSION_STRENGTH, 1.0),
            GlowStyle::Halation => (HALATION_STRENGTH, 0.0),
            GlowStyle::Motion | GlowStyle::Streak => {
                slot.set(lanes::STRENGTH, STREAK_STRENGTH * amount);
                slot.set(
                    lanes::DIRECTION,
                    DIRECTION.clamp(self.direction).to_radians(),
                );
                slot.set(lanes::LENGTH, self.streak_length());
                slot.set(lanes::SOFTNESS, self.streak_softness());
                return;
            }
        };
        let widest = self.widest_octave();
        // At least one octave, so a narrow Halation still has a profile.
        let finest = FINEST_SCALE.log2().min(widest - 1.0);
        slot.set(lanes::STRENGTH, strength * amount);
        slot.set(lanes::FINEST, finest);
        slot.set(lanes::WIDEST, widest);
        slot.set(lanes::SLOPE, slope);
    }
}

/// The colour of a blackbody at `kelvin` relative to one at
/// [`NEUTRAL_KELVIN`], scene-linear Rec.2020 with luminance 1, so a tint at
/// the neutral temperature is exactly white.
fn planckian_tint(kelvin: f32) -> [f32; 3] {
    let rgb = planckian_rec2020(kelvin);
    let neutral = planckian_rec2020(NEUTRAL_KELVIN);
    let ratio: [f32; 3] = std::array::from_fn(|channel| rgb[channel] / neutral[channel]);
    let luminance = REC2020_LUMA
        .iter()
        .zip(ratio)
        .map(|(weight, value)| weight * value)
        .sum::<f32>();
    ratio.map(|value| value / luminance)
}

/// Rec.2020 luminance weights.
const REC2020_LUMA: [f32; 3] = [0.2627002, 0.6779981, 0.0593017];

/// The linear Rec.2020 colour of a blackbody at `kelvin` (1667 K to 25000 K),
/// with luminance 1: its chromaticity from the cubic approximation of the
/// Planckian locus by Kim et al. (2002).
fn planckian_rec2020(kelvin: f32) -> [f32; 3] {
    let t = f64::from(kelvin.clamp(1667.0, 25000.0));
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.2661239e9 / t3 - 0.2343589e6 / t2 + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t3 + 2.1070379e6 / t2 + 0.2226347e3 / t + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t <= 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    // CIE XYZ (D65) to linear Rec.2020 (ITU-R BT.2020).
    const XYZ_TO_REC2020: [[f64; 3]; 3] = [
        [1.7166512, -0.3556708, -0.2533663],
        [-0.6666844, 1.6164812, 0.0157685],
        [0.0176399, -0.0427706, 0.9421031],
    ];
    XYZ_TO_REC2020.map(|row| (row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2]) as f32)
}
