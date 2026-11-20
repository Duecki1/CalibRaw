//! Effects a pipeline renders without because their GPU resources could not
//! be allocated within the admission limit (`gpu_admission_limit_bytes`).
//!
//! The shaders then leave such an effect out: the image shows without it,
//! never black. The pipeline remembers which effects fail, so callers learn of
//! each failure once ([`RawGpuPipeline::take_unrendered_effects`]): a preview
//! tells the user, an export fails rather than write a file without them.

use super::*;

/// An effect, or group of effects sharing resources, left out of a render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnrenderedEffect {
    /// Lens Blur's textures or its full-image map.
    LensBlur,
    /// The Glow's pyramids or its full-image map.
    Glow,
    /// The scene bake, light bake or air light that depth effects (Relight,
    /// Fog, Smoke, Light Rays) read.
    Scene,
}

impl UnrenderedEffect {
    /// The effect's name in English, for logs and errors.
    pub const fn label(self) -> &'static str {
        match self {
            Self::LensBlur => "Lens Blur",
            Self::Glow => "Glow",
            Self::Scene => "Depth effects",
        }
    }
}

/// A resource an effect allocates when it first renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectResource {
    LensBlurTextures,
    LensBlurMap,
    GlowPyramid,
    GlowMap,
    SceneBake,
    LightBake,
    AirLight,
}

impl EffectResource {
    const COUNT: usize = 7;

    const fn effect(self) -> UnrenderedEffect {
        match self {
            Self::LensBlurTextures | Self::LensBlurMap => UnrenderedEffect::LensBlur,
            Self::GlowPyramid | Self::GlowMap => UnrenderedEffect::Glow,
            Self::SceneBake | Self::LightBake | Self::AirLight => UnrenderedEffect::Scene,
        }
    }
}

/// Which resources failed to allocate the last time they were needed, and
/// the effects no caller has taken yet.
#[derive(Debug, Default)]
pub(super) struct UnrenderedEffects {
    failing: [bool; EffectResource::COUNT],
    untaken: Vec<UnrenderedEffect>,
}

impl RawGpuPipeline {
    /// Records the outcome of allocating `resource`: an error is logged and,
    /// the first time in a row, its effect kept for
    /// [`Self::take_unrendered_effects`]. A change of outcome changes the
    /// image without changing the parameters, so the next output render
    /// starts from its first part. Returns whether it was allocated.
    pub(super) fn note_effect_allocation(
        &self,
        resource: EffectResource,
        result: Result<()>,
    ) -> bool {
        let effect = resource.effect();
        let failed = result.is_err();
        if let Err(error) = &result {
            log::error!(
                "{} renders without its {resource:?}: {error:#}",
                effect.label()
            );
        }
        let changed = {
            let mut state = self
                .unrendered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let index = resource as usize;
            let changed = state.failing[index] != failed;
            state.failing[index] = failed;
            if changed && failed && !state.untaken.contains(&effect) {
                state.untaken.push(effect);
            }
            changed
        };
        if changed {
            self.forget_output();
        }
        !failed
    }

    /// Fails when an effect was left out since the last check, for renders
    /// that must not lack one: exports and the full-resolution preview.
    pub fn ensure_effects_rendered(&self) -> Result<()> {
        let effects = self.take_unrendered_effects();
        if effects.is_empty() {
            return Ok(());
        }
        let names = effects
            .iter()
            .map(|effect| effect.label())
            .collect::<Vec<_>>()
            .join(", ");
        Err(anyhow!(
            "not enough GPU memory to render {names}; see the log for the sizes"
        ))
    }

    /// The effects that started to fail since the last call, in the order
    /// they failed. A render after them shows the image without them.
    pub fn take_unrendered_effects(&self) -> Vec<UnrenderedEffect> {
        std::mem::take(
            &mut self
                .unrendered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .untaken,
        )
    }
}
