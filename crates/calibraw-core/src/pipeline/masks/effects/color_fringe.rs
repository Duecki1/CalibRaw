//! Color Fringe: the photo's red and blue channels moved apart, as lateral
//! chromatic aberration or an RGB split would (color_fringe.wgsl).
//!
//! - Lens moves red outward from the full image's centre and blue inward,
//!   by a shift that grows linearly with the distance from the centre and
//!   equals Amount at the corners, as a lens whose magnification differs per
//!   colour does. The centre is the full image's, the lens axis, so cropping
//!   does not move it.
//! - Shift moves red along Direction and blue against it, by Amount
//!   everywhere. Direction has Motion Blur's frame: the full image's pixels
//!   with y down, so cropping, turning or straightening does not rotate it.
//! - Green stays in place.
//!
//! Amount is a fraction of the full image's shorter edge, up to
//! [`MAX_SHIFT`]. The export halo is a fixed number of pixels
//! (`COLOR_FRINGE_SUPPORT`), so the shader caps the shift there: the full
//! Amount holds for shorter edges up to `COLOR_FRINGE_SUPPORT / MAX_SHIFT`
//! pixels (9600).
//!
//! Prism spreads each channel from its own position toward its neighbours'
//! with the weights of the dispersion in Blender's Lens Distortion node:
//! red ramps from its position toward green's, green is a tent over both
//! segments, blue ramps from its position toward green's. Prism sets how far
//! along each segment the ramps reach: at 0 they are three sharp copies, at
//! 100 they are Blender's smear. Each channel's weights are normalized, so
//! an even area keeps its colour.

use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::color_fringe::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// How the channels move, chosen with the card's Style control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ColorFringeStyle {
    /// Outward from the image's centre, as a lens's lateral chromatic
    /// aberration.
    #[default]
    Lens,
    /// Along one direction everywhere.
    Shift,
}

impl ColorFringeStyle {
    pub const ALL: [Self; 2] = [Self::Lens, Self::Shift];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lens => "Lens",
            Self::Shift => "Shift",
        }
    }

    /// `STYLE` lane value (`COLOR_FRINGE_*` in color_fringe.wgsl).
    const fn shader_value(self) -> f32 {
        match self {
            Self::Lens => 0.0,
            Self::Shift => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ColorFringeEffectSettings {
    pub style: ColorFringeStyle,
    pub amount: f32,
    pub prism: f32,
    /// Shift: the direction red moves toward, in degrees.
    pub direction: f32,
}

impl Default for ColorFringeEffectSettings {
    fn default() -> Self {
        Self {
            style: ColorFringeStyle::Lens,
            amount: AMOUNT.default,
            prism: PRISM.default,
            direction: DIRECTION.default,
        }
    }
}

effect_lanes! {
    /// [`ColorFringeStyle`] as 0 or 1.
    STYLE = 0,
    /// How far red and blue move, in shorter edges: everywhere for Shift, at
    /// the corners for Lens.
    SHIFT = 1,
    /// How far along the segment to its neighbour each channel spreads, 0
    /// to 1.
    PRISM = 2,
    /// Shift: the direction red moves toward, in radians, in the full image's
    /// pixels with y down, as Motion Blur's Angle.
    DIRECTION = 3,
}

/// Amount 100 moves red and blue by this share of the shorter edge.
const MAX_SHIFT: f32 = 0.015;

impl EffectSettings for ColorFringeEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        let mut controls = vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.prism, PRISM),
        ];
        if self.style == ColorFringeStyle::Shift {
            controls.push(EffectControl::Angle(&mut self.direction, DIRECTION));
        }
        EffectControls::new(controls)
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::STYLE, self.style.shader_value());
        slot.set(lanes::SHIFT, MAX_SHIFT * AMOUNT.clamp(self.amount) / 100.0);
        slot.set(lanes::PRISM, PRISM.clamp(self.prism) / 100.0);
        slot.set(
            lanes::DIRECTION,
            DIRECTION.clamp(self.direction).to_radians(),
        );
    }
}
