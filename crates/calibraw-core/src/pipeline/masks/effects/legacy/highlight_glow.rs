//! Highlight Glow: bright pixels inside the mask bloom in their own colour,
//! tinted toward the picked one, over a radius capped by the export halo. It
//! was a Glow with Self-illuminating off; the Glow's Bloom style replaces it
//! (mask_effects/legacy/highlight_glow.wgsl, migrations/highlight_glow.rs).
//!
//! legacy: remove in v2.0.0

use super::super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::super::params::glow::{AMOUNT, COLOR, CORE, RADIUS};
use super::super::settings::{EffectControl, EffectControls, EffectSettings};

/// The highlight Glow's values, which it shared with the Glowing Area.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LegacyHighlightGlowEffectSettings {
    pub amount: f32,
    pub radius: f32,
    pub core: f32,
    pub color: [f32; 3],
}

impl Default for LegacyHighlightGlowEffectSettings {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            radius: RADIUS.default,
            core: CORE.default,
            color: COLOR.default,
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    RADIUS = 1,
    CORE = 2,
    COLOR = 4 (color),
}

impl EffectSettings for LegacyHighlightGlowEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6
    }

    /// Validates saved values; its card shows none of them.
    fn controls(&mut self) -> EffectControls<'_> {
        EffectControls::new(vec![
            EffectControl::Slider(&mut self.amount, AMOUNT),
            EffectControl::Slider(&mut self.radius, RADIUS),
            EffectControl::Color(&mut self.color, COLOR),
            EffectControl::Slider(&mut self.core, CORE),
        ])
    }

    fn pack(&self, slot: &mut EffectLanes, _: &PackContext) {
        slot.set(lanes::AMOUNT, AMOUNT.clamp(self.amount));
        slot.set(lanes::RADIUS, RADIUS.clamp(self.radius));
        slot.set(lanes::CORE, CORE.clamp(self.core));
        slot.set_color(lanes::COLOR, COLOR.clamp(self.color));
    }
}
