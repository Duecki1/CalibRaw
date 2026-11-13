//! Glow: light that spreads from the bright parts of the mask, in one of six
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
//! - Star spreads it along Points rays from each light, as a cross-screen
//!   filter would: Points / 2 lines through the light, the first along
//!   Direction and the rest turned evenly from it, each a Streak's kernel.
//!   The lines share the light equally.
//! - Every kernel holds unit energy, so spreading light farther does not add
//!   to it.
//! - Amount up to 100 sets how much of that light is spread (or moved, or
//!   for Halation the halo's gain). Past 100 it brightens the spread light
//!   beyond the light that emits, [`AMOUNT_BOOST_STOPS`] stops more at 200,
//!   as Oniric's glow Exposure does; Diffusion then adds light as well as
//!   moving it.
//! - Warmth tints the spread light: along the Planckian locus from cool to
//!   warm, or from yellow to deep red for Halation.
//! - Dispersion scales the kernel of each primary by its wavelength, as
//!   diffraction does: red spreads wider and blue tighter, so Bloom,
//!   Diffusion and Halation turn bluish at the core and red at the rim, and
//!   rays of Streak and Star run from blue into red. Each primary's kernel
//!   keeps unit energy, so the glow's colour as a whole stays the same; only
//!   where it lands changes. Motion, a shutter drag, has no Dispersion.
//! - Split moves the spread light's red one way and its blue the other, as
//!   Oniric's Chromatic Aberration (formerly Light Dispersion) splits a
//!   glow's channels, while the photo stays in place: Shift moves red along
//!   Split direction and blue against it everywhere, Lens moves red outward
//!   from the full image's centre and blue inward by a shift that grows with
//!   the distance and is Split's at the corners, as Color Fringe's styles
//!   do. Green stays. The split light is read from the full-image map, so
//!   it may move farther than an export tile's halo; it is as fine as the
//!   map, about a 512th of the shorter edge.
//!
//! Scales are fractions of the image's shorter edge, so the look does not
//! depend on the resolution. This module turns the controls into those
//! quantities; the shader only integrates the profile and composites. The
//! share of energy per octave is the one quantity the shader takes from the
//! style itself (`styled_glow_profile_slope`), as the slot has no lane left.
//! The wavelengths Dispersion scales by are the Rec.2020 primaries', the
//! working space's, and are set there too.

use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::styled_glow::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};
use super::warmth::warmth_tint;

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
    /// Rays from highlights, as through a cross-screen star filter.
    Star,
}

impl GlowStyle {
    pub const ALL: [Self; 6] = [
        Self::Bloom,
        Self::Diffusion,
        Self::Halation,
        Self::Motion,
        Self::Streak,
        Self::Star,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bloom => "Bloom",
            Self::Diffusion => "Diffusion",
            Self::Halation => "Halation",
            Self::Motion => "Motion",
            Self::Streak => "Streak",
            Self::Star => "Star",
        }
    }

    /// Whether light spreads in streaks along a Direction rather than all
    /// around, which the card's Direction and Softness set.
    pub const fn streaks(self) -> bool {
        matches!(self, Self::Motion | Self::Streak | Self::Star)
    }

    /// Whether the card's Dispersion applies: to every style but Motion,
    /// whose streaks are a shutter drag, not light bent by optics.
    pub const fn disperses(self) -> bool {
        !matches!(self, Self::Motion)
    }

    /// `STYLE` lane value (`STYLED_GLOW_*` in styled_glow.wgsl).
    const fn shader_value(self) -> f32 {
        match self {
            Self::Bloom => 0.0,
            Self::Diffusion => 1.0,
            Self::Halation => 2.0,
            Self::Motion => 3.0,
            Self::Streak => 4.0,
            Self::Star => 5.0,
        }
    }

    /// The style a slot's `STYLE` lane holds.
    pub fn from_shader_value(value: f32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|style| style.shader_value() == value)
    }
}

/// The rays of a Star glow, chosen with its card's Points control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum StarPoints {
    Four,
    #[default]
    Six,
    Eight,
}

impl StarPoints {
    pub const ALL: [Self; 3] = [Self::Four, Self::Six, Self::Eight];

    /// The control's label: the number of rays.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Four => "4",
            Self::Six => "6",
            Self::Eight => "8",
        }
    }

    /// The most lines through a light any Points has, which bounds the
    /// streak passes a render runs.
    pub const MOST_AXES: u32 = 4;

    /// The lines through a light, each with a ray either way.
    const fn axes(self) -> u32 {
        match self {
            Self::Four => 2,
            Self::Six => 3,
            Self::Eight => 4,
        }
    }
}

const _: () = {
    let mut index = 0;
    while index < StarPoints::ALL.len() {
        assert!(StarPoints::ALL[index].axes() <= StarPoints::MOST_AXES);
        index += 1;
    }
};

/// How Split moves the glow's red and blue apart, chosen on the card.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum GlowSplitMode {
    /// Outward from the image's centre, as a lens's lateral chromatic
    /// aberration.
    Lens,
    /// Along Split direction everywhere, an RGB split.
    #[default]
    Shift,
}

impl GlowSplitMode {
    pub const ALL: [Self; 2] = [Self::Lens, Self::Shift];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lens => "Lens",
            Self::Shift => "Shift",
        }
    }

    /// `SPLIT_MODE` lane value (`STYLED_GLOW_SPLIT_*` in styled_glow.wgsl).
    const fn shader_value(self) -> f32 {
        match self {
            Self::Lens => 1.0,
            Self::Shift => 0.0,
        }
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
    /// Every style but Motion: how much wider red spreads than blue.
    pub dispersion: f32,
    /// Motion, Streak and Star: the direction light streaks toward (Star's
    /// first ray), in degrees.
    pub direction: f32,
    /// Motion, Streak and Star: how far the streaks spread across their
    /// direction.
    pub softness: f32,
    /// Star: its rays.
    pub points: StarPoints,
    /// How far the spread light's red and blue move apart.
    pub split: f32,
    pub split_mode: GlowSplitMode,
    /// Split's Shift: the direction red moves toward, in degrees.
    pub split_direction: f32,
}

impl Default for StyledGlowEffectSettings {
    fn default() -> Self {
        Self {
            style: GlowStyle::Bloom,
            amount: AMOUNT.default,
            range: RANGE.default,
            spread: SPREAD.default,
            warmth: WARMTH.default,
            dispersion: DISPERSION.default,
            direction: DIRECTION.default,
            softness: SOFTNESS.default,
            points: StarPoints::default(),
            split: SPLIT.default,
            split_mode: GlowSplitMode::default(),
            split_direction: SPLIT_DIRECTION.default,
        }
    }
}

effect_lanes! {
    /// [`GlowStyle`] as 0 to 5.
    STYLE = 0,
    /// The share of the emitted light that is spread (or, for Halation, the
    /// gain on the halo's luminance).
    STRENGTH = 1,
    /// The Range threshold: log2 of the luminance at the knee's middle.
    THRESHOLD = 2,
    /// Bloom, Diffusion and Halation: log2 of the finest Gaussian deviation
    /// of the spread profile, in shorter edges.
    FINEST = 3,
    /// How far light spreads, in shorter edges: for Bloom, Diffusion and
    /// Halation log2 of the widest Gaussian deviation of the profile; for
    /// Motion, Streak and Star the length λ over which a streak falls to 1/e.
    /// One lane serves both, as no style uses both.
    REACH = 4,
    /// The exponent of wavelength by which each primary's kernel scales:
    /// widths go as (λ / λ_green)^DISPERSION (styled_glow.wgsl), so 1 is
    /// diffraction's. 0 for Motion.
    DISPERSION = 5,
    /// The tint of the spread light, scene-linear Rec.2020, times Amount's
    /// boost past 100.
    TINT = 6 (color),
    /// Motion, Streak and Star: the direction light streaks toward, in
    /// radians, in the full image's pixels with y down (clockwise on screen),
    /// as Motion Blur's Angle.
    DIRECTION = 9,
    /// Motion, Streak and Star: the lines through a light that streaks run
    /// along, 1 but for Star, turned π / AXES apart from DIRECTION.
    AXES = 10,
    /// Motion, Streak and Star: the Gaussian deviation across a streak, in
    /// shorter edges.
    SOFTNESS = 11,
    /// How far Split moves red, and blue the other way, in the full image's
    /// shorter edges: everywhere for Shift, at the corners for Lens. 0
    /// without Split.
    SPLIT = 12,
    /// [`GlowSplitMode`]: 1 for Lens, 0 for Shift.
    SPLIT_MODE = 13,
    /// Split's Shift: the direction red moves toward, in radians, in the full
    /// image's pixels with y down, as DIRECTION.
    SPLIT_DIRECTION = 14,
}

/// Stops Amount 200 brightens the spread light by over Amount 100, evenly
/// per stop between them.
const AMOUNT_BOOST_STOPS: f32 = 4.0;

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
/// Amount 100 adds the light that emits, spread along the streaks; a
/// Star's lines share it.
const STREAK_STRENGTH: f32 = 1.0;
/// The exponent of wavelength at Dispersion 100 (the `DISPERSION` lane):
/// red then spreads about twice as wide as green and blue about 0.6 times
/// as wide. Dispersion 25 is diffraction's exponent of 1.
const DISPERSION_AT_FULL: f32 = 4.0;
/// Split 100 moves red and blue this share of the shorter edge, about the
/// split of Oniric's samples; the shift grows with the square of Split, so
/// low values give a fine fringe.
const SPLIT_AT_FULL: f32 = 0.15;

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
            GlowStyle::Bloom
            | GlowStyle::Diffusion
            | GlowStyle::Motion
            | GlowStyle::Streak
            | GlowStyle::Star => warmth_tint(warmth),
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
        let mut separation = Vec::new();
        if self.style.disperses() {
            separation.push(EffectControl::Slider(&mut self.dispersion, DISPERSION));
        }
        separation.push(EffectControl::Slider(&mut self.split, SPLIT));
        separation.push(EffectControl::Angle(
            &mut self.split_direction,
            SPLIT_DIRECTION,
        ));
        EffectControls::new(controls).details(COLOR_SEPARATION, separation)
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        let amount = AMOUNT.clamp(self.amount) / 100.0;
        let range = RANGE.clamp(self.range) / 100.0;
        slot.set(lanes::STYLE, self.style.shader_value());
        slot.set(
            lanes::THRESHOLD,
            MIDDLE_GREY.log2() + RANGE_TOP_STOPS - RANGE_STOPS * range,
        );
        // Up to 100 Amount is the share of the light; past it, a gain on the
        // tint, which multiplies only the spread light in every style.
        let (amount, boost) = if amount > 1.0 {
            (1.0, (AMOUNT_BOOST_STOPS * (amount - 1.0)).exp2())
        } else {
            (amount, 1.0)
        };
        slot.set_color(lanes::TINT, self.tint().map(|channel| channel * boost));
        let split = SPLIT.clamp(self.split) / 100.0;
        slot.set(lanes::SPLIT, SPLIT_AT_FULL * split * split);
        slot.set(lanes::SPLIT_MODE, self.split_mode.shader_value());
        slot.set(
            lanes::SPLIT_DIRECTION,
            SPLIT_DIRECTION.clamp(self.split_direction).to_radians(),
        );
        if self.style.disperses() {
            slot.set(
                lanes::DISPERSION,
                DISPERSION_AT_FULL * DISPERSION.clamp(self.dispersion) / 100.0,
            );
        }
        let strength = match self.style {
            GlowStyle::Bloom => BLOOM_STRENGTH,
            GlowStyle::Diffusion => DIFFUSION_STRENGTH,
            GlowStyle::Halation => HALATION_STRENGTH,
            GlowStyle::Motion | GlowStyle::Streak | GlowStyle::Star => {
                let axes = match self.style {
                    GlowStyle::Star => self.points.axes(),
                    _ => 1,
                };
                slot.set(lanes::STRENGTH, STREAK_STRENGTH * amount);
                slot.set(
                    lanes::DIRECTION,
                    DIRECTION.clamp(self.direction).to_radians(),
                );
                slot.set(lanes::AXES, axes as f32);
                slot.set(lanes::REACH, self.streak_length());
                slot.set(lanes::SOFTNESS, self.streak_softness());
                return;
            }
        };
        let widest = self.widest_octave();
        // At least one octave, so a narrow Halation still has a profile.
        let finest = FINEST_SCALE.log2().min(widest - 1.0);
        slot.set(lanes::STRENGTH, strength * amount);
        slot.set(lanes::FINEST, finest);
        slot.set(lanes::REACH, widest);
    }
}
