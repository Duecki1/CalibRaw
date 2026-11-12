use super::lanes::{effect_lanes, options, EffectLanes, LaneName, PackContext};
use super::params::lens_blur::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// A defocus with an aperture-shaped kernel (lens_blur.wgsl). With Focus by
/// depth on, surfaces inside the focus range stay sharp and the blur grows
/// with their distance from it, read from scene depth; off, the mask blurs
/// evenly.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LensBlurEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub blades: f32,
    pub rotation: f32,
    pub highlight_boost: f32,
    /// Whether the blur follows scene depth. The serialized baseline is off,
    /// so edits saved before it existed blur evenly as they did; new effects
    /// start with it on ([`EffectSettings::initial`]).
    pub focus_by_depth: bool,
    /// The focus range's ends in control depth, 0 to 100 (`FOCUS_NEAR`).
    pub focus_near: f32,
    pub focus_far: f32,
}

impl Default for LensBlurEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            blades: BLADES.default,
            rotation: ROTATION.default,
            highlight_boost: HIGHLIGHTS.default,
            focus_by_depth: false,
            focus_near: FOCUS_NEAR.default,
            focus_far: FOCUS_FAR.default,
        }
    }
}

impl LensBlurEffectSettings {
    /// The focus range in control depth, 0 to 1, near end first.
    pub fn focus_range(&self) -> [f32; 2] {
        let near = FOCUS_NEAR.clamp(self.focus_near);
        let far = FOCUS_FAR.clamp(self.focus_far).max(near);
        [near / 100.0, far / 100.0]
    }
}

effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    BLADES = 2,
    ROTATION = 3,
    HIGHLIGHTS = 4,
    /// The focus range in control depth, 0 to 1, with FOCUS_NEAR ≤ FOCUS_FAR.
    FOCUS_NEAR = 5,
    FOCUS_FAR = 6,
}

impl EffectSettings for LensBlurEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn initial() -> Self {
        Self {
            focus_by_depth: true,
            ..Self::default()
        }
    }

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.radius > 1e-6
    }

    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Slider(&mut self.highlight_boost, HIGHLIGHTS),
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
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        // The aperture has whole blades.
        slot.set(lanes::BLADES, BLADES.clamp(self.blades).round());
        slot.set(lanes::ROTATION, ROTATION.clamp(self.rotation));
        slot.set(lanes::HIGHLIGHTS, HIGHLIGHTS.clamp(self.highlight_boost));
        let [near, far] = self.focus_range();
        slot.set(lanes::FOCUS_NEAR, near);
        slot.set(lanes::FOCUS_FAR, far);
        slot.set_switch(options::LENS_BLUR_DEPTH, self.focus_by_depth);
    }
}
