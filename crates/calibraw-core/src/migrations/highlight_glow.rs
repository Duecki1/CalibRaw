//! Glows saved with Self-illuminating off, before the Glow was rebuilt with
//! styles.
//!
//! migration: remove in v2.0.0
//!
//! A Glow kept `self_illuminating` in its settings, on unless saved off. On,
//! it is the Glowing Area, which keeps the variant `Glow` and drops the key
//! when the edit is written again. Off, its bright pixels bloomed inside the
//! mask; that look is now the legacy effect `LegacyHighlightGlow`, which
//! renders exactly as before, so the component becomes one with the same
//! values. This covers effect components, global or of a mask, and a mask's
//! single effect of edits from before effect components.
//!
//! [`LegacyHighlightGlows`] reads the key from the same `edits` object as the
//! decoded edit, by position. It runs before the Effects-card sliders are
//! migrated (`effect_sliders`), which then find the legacy Glow already
//! present, as they found the Glow before.

use crate::pipeline::{
    EffectComponent, GlowEffectSettings, LegacyHighlightGlowEffectSettings, MaskEffect,
    MaskEffectSettings,
};
use crate::sidecar::EditState;
use serde::Deserialize;
use std::sync::Arc;

/// Where a serialized `edits` object saved a Glow with Self-illuminating
/// off. Every other field is ignored, so this reads any edit, old or current.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LegacyHighlightGlows {
    #[serde(default)]
    masks: SavedMasks,
}

#[derive(Debug, Default, Deserialize)]
struct SavedMasks {
    #[serde(default)]
    global_effects: Vec<SavedComponent>,
    /// In the same order as the decoded edit's masks.
    #[serde(default)]
    masks: Vec<SavedMask>,
}

#[derive(Debug, Default, Deserialize)]
struct SavedMask {
    #[serde(default)]
    effect_components: Vec<SavedComponent>,
    /// The mask's single effect of edits from before effect components.
    #[serde(default)]
    effect_settings: SavedSettings,
}

#[derive(Debug, Default, Deserialize)]
struct SavedComponent {
    #[serde(default)]
    settings: SavedSettings,
}

#[derive(Debug, Default, Deserialize)]
struct SavedSettings {
    #[serde(default)]
    glow: Option<SavedGlow>,
}

#[derive(Debug, Default, Deserialize)]
struct SavedGlow {
    self_illuminating: Option<bool>,
}

impl SavedSettings {
    /// Whether a Glow with these settings bloomed its highlights.
    fn highlight_glow(&self) -> bool {
        self.glow
            .as_ref()
            .is_some_and(|glow| glow.self_illuminating == Some(false))
    }
}

impl LegacyHighlightGlows {
    /// Turns every Glow of `edits` saved with Self-illuminating off into a
    /// `LegacyHighlightGlow`. `edits` must be decoded from the same `edits`
    /// object as `self`, before any mask or component is removed or
    /// reordered. Returns whether any was converted.
    pub(crate) fn migrate(self, edits: &mut EditState) -> bool {
        let saved = |components: &[SavedComponent]| {
            components
                .iter()
                .any(|component| component.settings.highlight_glow())
        };
        let any = saved(&self.masks.global_effects)
            || self.masks.masks.iter().any(|mask| {
                saved(&mask.effect_components) || mask.effect_settings.highlight_glow()
            });
        if !any {
            return false;
        }

        let masks = Arc::make_mut(&mut edits.masks);
        let mut migrated =
            migrate_components(&mut masks.global_effects, &self.masks.global_effects);
        for (mask, saved) in masks.masks.iter_mut().zip(&self.masks.masks) {
            migrated |= migrate_components(&mut mask.effect_components, &saved.effect_components);
            if mask.effect == MaskEffect::Glow && saved.effect_settings.highlight_glow() {
                mask.effect = MaskEffect::LegacyHighlightGlow;
                mask.effect_settings = highlight_settings(mask.effect_settings.glow);
                migrated = true;
            }
        }
        migrated
    }
}

fn migrate_components(components: &mut [EffectComponent], saved: &[SavedComponent]) -> bool {
    let mut migrated = false;
    for (component, saved) in components.iter_mut().zip(saved) {
        if component.effect == MaskEffect::Glow && saved.settings.highlight_glow() {
            component.effect = MaskEffect::LegacyHighlightGlow;
            component.settings = highlight_settings(component.settings.glow);
            migrated = true;
        }
    }
    migrated
}

/// The decoded Glow's values as the legacy effect's settings; Scene light
/// never applied to a highlight Glow.
fn highlight_settings(glow: GlowEffectSettings) -> MaskEffectSettings {
    MaskEffectSettings {
        legacy_highlight_glow: LegacyHighlightGlowEffectSettings {
            amount: glow.amount,
            radius: glow.radius,
            core: glow.core,
            color: glow.color,
        },
        ..MaskEffectSettings::default()
    }
}
