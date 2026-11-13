//! Lens Blur: a lens's defocus, synthesized from scene depth
//! (mask_effects/defocus.wgsl, pipeline/gpu/defocus.rs in calibraw-gpu).
//!
//! The model is a thin lens focused on the focus range. Every surface is
//! drawn as its own aperture-shaped disc, whose radius (its circle of
//! confusion) grows linearly with its distance from the focus range in
//! inverse distance, as a lens's does: 0 inside the range, and at Blur
//! amount 100 [`LARGEST_RADIUS`] of the image's longer edge at the scene's
//! most out-of-focus end. Nearer surfaces cover farther ones, so a blurred
//! foreground spreads over the scene behind it and a sharp subject keeps its
//! edge against a blurred background. With Focus by depth off, or until
//! scene depth exists, the mask blurs evenly by the largest radius; the
//! unmasked part stays sharp in front of it.
//!
//! Boost puts back the energy that clipping took from lights: bright pixels
//! that stand out from their surroundings gain up to [`LARGEST_BOOST`] times
//! their energy before they are spread, so out-of-focus lights become bright
//! discs instead of pale ones. The Bokeh shape sets the aperture; Blades
//! and Rotation shape the Blades aperture.

use super::lanes::{effect_lanes, options, EffectLanes, LaneName, PackContext};
use super::params::defocus::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// The aperture out-of-focus lights take the shape of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum BokehShape {
    /// An evenly lit disc, as a modern lens draws it.
    #[default]
    Circle,
    /// A disc brighter toward its rim, as over-corrected spherical
    /// aberration draws it (soap-bubble bokeh).
    Bubble,
    /// A polygon of Aperture blades, as a stopped-down iris draws it.
    Blades,
    /// A ring, as a mirror lens's central obstruction draws it.
    Ring,
    /// A disc cut by the lens barrel toward the image's corners (optical
    /// vignetting).
    CatsEye,
}

impl BokehShape {
    pub const ALL: [Self; 5] = [
        Self::Circle,
        Self::Bubble,
        Self::Blades,
        Self::Ring,
        Self::CatsEye,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Circle => "Circle",
            Self::Bubble => "Bubble",
            Self::Blades => "Blades",
            Self::Ring => "Ring",
            Self::CatsEye => "Cat's eye",
        }
    }

    /// `SHAPE` lane value (`DEFOCUS_SHAPE_*` in defocus.wgsl).
    const fn shader_value(self) -> f32 {
        match self {
            Self::Circle => 0.0,
            Self::Bubble => 1.0,
            Self::Blades => 2.0,
            Self::Ring => 3.0,
            Self::CatsEye => 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct DefocusEffectSettings {
    pub blur_amount: f32,
    pub shape: BokehShape,
    pub highlight_boost: f32,
    /// Whether the blur follows scene depth. Off, or until scene depth
    /// exists, the mask blurs evenly.
    pub focus_by_depth: bool,
    /// The focus range's ends in control depth, 0 to 100 (`FOCUS_NEAR`).
    pub focus_near: f32,
    pub focus_far: f32,
    /// The Blades aperture's blade count and turn.
    pub blades: f32,
    pub rotation: f32,
}

impl Default for DefocusEffectSettings {
    fn default() -> Self {
        Self {
            blur_amount: BLUR_AMOUNT.default,
            shape: BokehShape::Circle,
            highlight_boost: BOOST.default,
            focus_by_depth: true,
            focus_near: FOCUS_NEAR.default,
            focus_far: FOCUS_FAR.default,
            blades: BLADES.default,
            rotation: ROTATION.default,
        }
    }
}

impl DefocusEffectSettings {
    /// The focus range in control depth, 0 to 1, near end first.
    pub fn focus_range(&self) -> [f32; 2] {
        let near = FOCUS_NEAR.clamp(self.focus_near);
        let far = FOCUS_FAR.clamp(self.focus_far).max(near);
        [near / 100.0, far / 100.0]
    }
}

effect_lanes! {
    /// The largest disc radius, at the scene's most out-of-focus end or
    /// everywhere when the mask blurs evenly, in the full image's longer
    /// edges.
    RADIUS = 0,
    /// [`BokehShape`] as 0 to 4.
    SHAPE = 1,
    /// The energy gain of the brightest standing-out lights, 1 or more.
    BOOST = 2,
    /// The Blades aperture's whole blade count and its turn in radians.
    BLADES = 3,
    ROTATION = 4,
    /// The focus range in control depth, 0 to 1, with FOCUS_NEAR ≤ FOCUS_FAR.
    FOCUS_NEAR = 5,
    FOCUS_FAR = 6,
}

/// The disc radius at Blur amount 100, in the image's longer edges: a disc
/// 8% of the longer edge across. A full-frame 85 mm f/1.4 focused at 2 m
/// draws a distant background in discs 7.5% across, a 135 mm f/2 at 3 m 8.8%
/// and a 50 mm f/1.4 at 1.5 m 3.4%, so 100 is a fast portrait lens wide open
/// and lower values cover ordinary primes. The radius is linear in Blur
/// amount, as a lens's is in its aperture's diameter.
const LARGEST_RADIUS: f32 = 0.04;
/// The energy gain at Boost 100 of a light that clearly stands out: four
/// stops, about what a lamp or the sun loses to clipping at a normal
/// exposure.
const LARGEST_BOOST: f32 = 16.0;

impl EffectSettings for DefocusEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.blur_amount > 1e-6
    }

    /// The Bokeh shape is valid once decoded and has no control here; the
    /// card draws its choice.
    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.blur_amount, BLUR_AMOUNT),
            EffectControl::Slider(&mut self.highlight_boost, BOOST),
            EffectControl::Toggle(&mut self.focus_by_depth, FOCUS_BY_DEPTH),
            EffectControl::Slider(&mut self.focus_near, FOCUS_NEAR),
            EffectControl::Slider(&mut self.focus_far, FOCUS_FAR),
        ])
        .details(
            "Aperture details",
            vec![
                EffectControl::Slider(&mut self.blades, BLADES),
                EffectControl::Angle(&mut self.rotation, ROTATION),
            ],
        )
    }

    fn wants_scene_depth(&self) -> bool {
        self.focus_by_depth
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        let blur = BLUR_AMOUNT.clamp(self.blur_amount) / 100.0;
        slot.set(lanes::RADIUS, LARGEST_RADIUS * blur);
        slot.set(lanes::SHAPE, self.shape.shader_value());
        let boost = BOOST.clamp(self.highlight_boost) / 100.0;
        slot.set(lanes::BOOST, 1.0 + (LARGEST_BOOST - 1.0) * boost);
        // The aperture has whole blades.
        slot.set(lanes::BLADES, BLADES.clamp(self.blades).round());
        slot.set(lanes::ROTATION, ROTATION.clamp(self.rotation).to_radians());
        let [near, far] = self.focus_range();
        slot.set(lanes::FOCUS_NEAR, near);
        slot.set(lanes::FOCUS_FAR, far);
        slot.set_switch(options::DEFOCUS_DEPTH, self.focus_by_depth);
    }
}
