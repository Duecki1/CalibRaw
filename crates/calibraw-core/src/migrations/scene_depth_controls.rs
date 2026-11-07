//! Relight's scene controls saved before control depth: a Relief control and
//! a light depth linear in distance, and Fog's start likewise
//! (docs/rework/DEPTH_EFFECTS_PLAN.md §7, M2 and M3).
//!
//! migration: remove in v2.0.0
//!
//! Relight kept `relief` (0 to 100) and, per light, `depth` (-100 to 100).
//! Relief chose the depth range of the frame lights were placed in,
//! `0.25 + 5.75 · (relief / 100)^1.5` in units of the nearest distance, and a
//! light depth `p ≥ 0` lay at `p / 100` of that range, linear in distance,
//! with the distance of an 8-bit map `D = 1 / (i + 0.115)`. Every other
//! effect used the frame of the default Relief 50.
//!
//! Edits now hold `relief_scale`, a percentage of the frame the depth result
//! gives, and `light_depth`, in control depth `c`, even in the log of
//! distance (`calibraw-gpu`, `shaders/scene/camera.wgsl`). The conversion is
//! exact on scene depth migrated from those versions (`scene_depth_layout`),
//! whose frame is the default Relief's and whose distances range over
//! [`LEGACY_RANGE_RATIO`]: the light lands where it was and the frame is the
//! one it was placed in. On regenerated depth the light keeps its control
//! depth and Relief scale, so it sits at the same perceptual depth.
//!
//! [`LegacySceneControls`] reads the old keys from the same `edits` object as
//! the decoded edit; Relights that already hold the new keys are left alone.
//! The old keys are dropped when the edit is written again.
//!
//! Fog's `start` (0 to 95, linear in distance like a light depth) becomes
//! `start_depth`, in control depth by the same conversion as a light depth, so
//! the onset lies where it did on migrated depth.

use crate::pipeline::{
    effect_params::{fog, relight},
    MaskEffect,
};
use crate::sidecar::EditState;
use serde::Deserialize;
use std::sync::Arc;

/// The frame every effect used, at Relight's default Relief 50:
/// `0.25 + 5.75 · 0.5^1.5`.
const LEGACY_FRAME_RELIEF: f32 = 2.282_932;
/// The ratio of the farthest to the nearest distance of that frame,
/// `1.115 / 0.115`.
const LEGACY_RANGE_RATIO: f32 = 9.695_652;

/// Relight's old keys of a serialized `edits` object, by position. Every
/// other field is ignored, so this reads any edit, old or current.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LegacySceneControls {
    #[serde(default)]
    masks: LegacyMasks,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyMasks {
    #[serde(default)]
    global_effects: Vec<LegacyComponent>,
    /// In the same order as the decoded edit's masks.
    #[serde(default)]
    masks: Vec<LegacyMask>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyMask {
    #[serde(default)]
    effect_components: Vec<LegacyComponent>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyComponent {
    #[serde(default)]
    settings: LegacySettings,
}

#[derive(Debug, Default, Deserialize)]
struct LegacySettings {
    #[serde(default)]
    relight: Option<LegacyRelight>,
    #[serde(default)]
    fog: Option<LegacyFog>,
}

/// Fog's start, old key and new. A new key next to an old one wins.
#[derive(Debug, Default, Deserialize)]
struct LegacyFog {
    start: Option<f32>,
    start_depth: Option<f32>,
}

/// The fields of a Relight that this migration converts. A new key next to
/// an old one wins.
#[derive(Debug, Default, Deserialize)]
struct LegacyRelight {
    relief: Option<f32>,
    relief_scale: Option<f32>,
    #[serde(flatten)]
    first: LegacyLight,
    #[serde(default)]
    additional: Vec<LegacyLight>,
}

#[derive(Debug, Default, Deserialize)]
struct LegacyLight {
    depth: Option<f32>,
    light_depth: Option<f32>,
}

impl LegacySceneControls {
    /// Converts the old controls of every Relight in `edits`, which must be
    /// decoded from the same `edits` object as `self`, before any mask or
    /// component is removed or reordered. Returns whether any was converted.
    pub(crate) fn migrate(self, edits: &mut EditState) -> bool {
        let saved = |components: &[LegacyComponent]| {
            components.iter().any(|component| {
                component.settings.relight.as_ref().is_some_and(is_old)
                    || component.settings.fog.as_ref().is_some_and(is_old_fog)
            })
        };
        if !saved(&self.masks.global_effects)
            && !self
                .masks
                .masks
                .iter()
                .any(|mask| saved(&mask.effect_components))
        {
            return false;
        }

        let masks = Arc::make_mut(&mut edits.masks);
        let mut migrated =
            migrate_components(&mut masks.global_effects, &self.masks.global_effects);
        for (mask, legacy) in masks.masks.iter_mut().zip(&self.masks.masks) {
            migrated |= migrate_components(&mut mask.effect_components, &legacy.effect_components);
        }
        migrated
    }
}

fn is_old(relight: &LegacyRelight) -> bool {
    let old_light = |light: &LegacyLight| light.depth.is_some() && light.light_depth.is_none();
    (relight.relief.is_some() && relight.relief_scale.is_none())
        || old_light(&relight.first)
        || relight.additional.iter().any(old_light)
}

fn is_old_fog(fog: &LegacyFog) -> bool {
    fog.start.is_some() && fog.start_depth.is_none()
}

fn migrate_components(
    components: &mut [crate::pipeline::EffectComponent],
    legacy: &[LegacyComponent],
) -> bool {
    let mut migrated = false;
    for (component, legacy) in components.iter_mut().zip(legacy) {
        if component.effect == MaskEffect::Fog {
            if let Some(start) = legacy
                .settings
                .fog
                .as_ref()
                .filter(|old| is_old_fog(old))
                .and_then(|old| old.start)
            {
                component.settings.fog.start_depth = control_depth(start, fog::START.max);
                migrated = true;
            }
            continue;
        }
        let Some(old) = legacy.settings.relight.as_ref().filter(|old| is_old(old)) else {
            continue;
        };
        if component.effect != MaskEffect::Relight {
            continue;
        }
        let settings = &mut component.settings.relight;
        if let (Some(relief), None) = (old.relief, old.relief_scale) {
            settings.relief_scale = relief_scale(relief);
        }
        let lights = std::iter::once(&old.first).chain(&old.additional);
        for (light, old) in settings.lights.as_mut_slice().iter_mut().zip(lights) {
            if let (Some(depth), None) = (old.depth, old.light_depth) {
                light.light_depth = light_depth(depth);
            }
        }
        migrated = true;
    }
    migrated
}

/// The Relief scale of the frame an old Relief control chose.
fn relief_scale(relief: f32) -> f32 {
    let control = (relief / 100.0).clamp(0.0, 1.0);
    let frame = 0.25 + 5.75 * control.powf(1.5);
    relight::RELIEF_SCALE.clamp(100.0 * frame / LEGACY_FRAME_RELIEF)
}

/// The control depth of an old light depth: lights before the nearest surface
/// keep their depth.
fn light_depth(depth: f32) -> f32 {
    let depth = relight::LIGHT_DEPTH.clamp(depth);
    if depth < 0.0 {
        return depth;
    }
    control_depth(depth, 100.0)
}

/// The control depth of an old linear depth `p` (percent of the old range):
/// `c = ln(1 + (Q − 1) · p) / ln Q` in the old range `Q`, so
/// `(Q^c − 1) / (Q − 1)` returns `p`. `limit` is the control's largest value.
fn control_depth(percent: f32, limit: f32) -> f32 {
    let fraction = percent.clamp(0.0, 100.0) / 100.0;
    (100.0 * (1.0 + (LEGACY_RANGE_RATIO - 1.0) * fraction).ln() / LEGACY_RANGE_RATIO.ln())
        .min(limit)
}
