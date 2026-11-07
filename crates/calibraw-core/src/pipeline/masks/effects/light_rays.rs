use super::instances::{mirrored_position, EffectInstances};
use super::lanes::{effect_lanes, EffectLanes, LaneName, PackContext};
use super::params::light_rays::*;
use super::settings::{EffectControl, EffectControls, EffectSettings};

/// Light shafts from one or more sources through the openings the mask
/// paints, shown only where the mask covers. The sources share the look of
/// their rays.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LightRaysEffectSettings {
    pub spread: f32,
    pub fade: f32,
    pub ray_count: f32,
    pub variation: f32,
    pub softness: f32,
    #[serde(flatten)]
    pub sources: EffectInstances<LightRaySource>,
}

/// One source of a Light Rays effect.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct LightRaySource {
    pub amount: f32,
    pub length: f32,
    /// Percentages of the full, uncropped image.
    pub source: [f32; 2],
    pub color: [f32; 3],
}

impl Default for LightRaysEffectSettings {
    fn default() -> Self {
        Self {
            spread: SPREAD.default,
            fade: FADE.default,
            ray_count: RAY_COUNT.default,
            variation: VARIATION.default,
            softness: SOFTNESS.default,
            sources: EffectInstances::default(),
        }
    }
}

impl Default for LightRaySource {
    fn default() -> Self {
        Self {
            amount: AMOUNT.default,
            length: LENGTH.default,
            source: [SOURCE_X.default, SOURCE_Y.default],
            color: COLOR.default,
        }
    }
}

impl LightRaySource {
    fn is_active(&self) -> bool {
        self.amount.abs() > 1e-6 && self.length > 1e-6
    }

    /// A copy of this source placed across the frame from it.
    pub fn mirrored(&self) -> Self {
        Self {
            source: mirrored_position(self.source, [SOURCE_X, SOURCE_Y]),
            ..*self
        }
    }
}

effect_lanes! {
    AMOUNT = 0,
    LENGTH = 1,
    SOURCE_X = 2,
    SOURCE_Y = 3,
    COLOR = 4 (color),
    FADE = 7,
    SPREAD = 8,
    RAY_COUNT = 9,
    VARIATION = 10,
    SOFTNESS = 11,
}

impl EffectSettings for LightRaysEffectSettings {
    const LANES: &'static [LaneName] = lanes::ALL;

    fn is_active(&self) -> bool {
        self.sources
            .as_slice()
            .iter()
            .any(LightRaySource::is_active)
    }

    fn instance_count(&self) -> usize {
        self.sources.count()
    }

    fn controls(&mut self) -> EffectControls<'_> {
        let mut shown = Vec::new();
        for source in self.sources.as_mut_slice() {
            shown.extend([
                EffectControl::Slider(&mut source.amount, AMOUNT),
                EffectControl::Slider(&mut source.length, LENGTH),
                EffectControl::Position(&mut source.source, [SOURCE_X, SOURCE_Y]),
                EffectControl::Color(&mut source.color, COLOR),
            ]);
        }
        EffectControls::new(shown).details(
            "Ray details",
            vec![
                EffectControl::Slider(&mut self.spread, SPREAD),
                EffectControl::Slider(&mut self.fade, FADE),
                EffectControl::Slider(&mut self.softness, SOFTNESS),
                EffectControl::Slider(&mut self.variation, VARIATION),
                EffectControl::Slider(&mut self.ray_count, RAY_COUNT),
            ],
        )
    }

    /// Each source's slot also holds the shared look of the rays.
    fn pack(&self, slot: &mut EffectLanes, context: &PackContext) {
        let Some(source) = self.sources.get(context.instance) else {
            return;
        };
        slot.set(lanes::AMOUNT, AMOUNT.clamp(source.amount));
        slot.set(lanes::LENGTH, LENGTH.clamp(source.length));
        slot.set(lanes::SOURCE_X, SOURCE_X.clamp(source.source[0]));
        slot.set(lanes::SOURCE_Y, SOURCE_Y.clamp(source.source[1]));
        slot.set_color(lanes::COLOR, COLOR.clamp(source.color));
        slot.set(lanes::FADE, FADE.clamp(self.fade));
        slot.set(lanes::SPREAD, SPREAD.clamp(self.spread));
        slot.set(lanes::RAY_COUNT, RAY_COUNT.clamp(self.ray_count));
        slot.set(lanes::VARIATION, VARIATION.clamp(self.variation));
        slot.set(lanes::SOFTNESS, SOFTNESS.clamp(self.softness));
    }
}
