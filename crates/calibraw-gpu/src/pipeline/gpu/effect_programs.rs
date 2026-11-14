//! Programs that hold only the effects an edit uses.
//!
//! The creative pass (`apply_creative_effects`) and the first Blur stage
//! (`diffuse_mask_blur_0`) choose an effect's code by the slot's effect ID at
//! run time, so one compiled program holds every effect they dispatch. Some
//! mobile GPUs run such a program far slower than its parts (Adreno: several
//! hundred times), whatever the edit uses.
//!
//! The creative effects module wraps each dispatched effect in
//! `#ifdef EFFECT_<NAME>` ([`effect_def`]). A program built for an
//! [`EffectSet`] defines only the set's effects, so the others' code is not
//! compiled into it. An effect that is left out has no active slot, and without
//! an active slot its code leaves the colour as it is, so a set's program and
//! the program with every effect compute the same image.
//!
//! [`EffectPrograms`] keeps a program per set for as long as the programs it
//! belongs to live: pipelines built from one template share them. A preview
//! renders with the program that holds every effect until a set's program is
//! compiled on a worker; a full-quality render compiles it first
//! ([`Readiness`]).

use super::shader_manager::ShaderManager;
use super::shaders;
use super::{CfaKind, MaskData, PersistentGpuPipelineCache};
use crate::gpu_errors::GpuErrorScopes;
use crate::pipeline::{EffectPass, MaskEffect};
use anyhow::{Context, Result};
use naga_oil::compose::ShaderDefValue;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// A set of effects, one bit per [`MaskEffect::shader_id`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct EffectSet(u32);

// Every shader ID has a bit.
const _: () = {
    let mut index = 0;
    while index < MaskEffect::ALL.len() {
        assert!(MaskEffect::ALL[index].shader_id() < u32::BITS);
        index += 1;
    }
};

impl EffectSet {
    const fn bit(effect: MaskEffect) -> u32 {
        1 << effect.shader_id()
    }

    pub(super) fn from_effects(effects: impl IntoIterator<Item = MaskEffect>) -> Self {
        Self(
            effects
                .into_iter()
                .fold(0, |bits, effect| bits | Self::bit(effect)),
        )
    }

    /// The effects of the active slots among `slots`.
    pub(super) fn of_slots(slots: &[MaskData]) -> Self {
        Self::from_effects(slots.iter().filter_map(MaskData::active_effect))
    }

    /// Every effect of the registry.
    pub(super) fn all() -> Self {
        Self::from_effects(MaskEffect::ALL)
    }

    pub(super) const fn bits(self) -> u32 {
        self.0
    }

    pub(super) const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    const fn contains(self, effect: MaskEffect) -> bool {
        self.0 & Self::bit(effect) != 0
    }

    const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The shader defs of the set: [`effect_def`] of each effect, and the
    /// defs of code two effects share.
    pub(super) fn shader_defs(self) -> HashMap<String, ShaderDefValue> {
        let mut defs: HashMap<String, ShaderDefValue> = MaskEffect::ALL
            .into_iter()
            .filter(|effect| *effect != MaskEffect::Adjustment && self.contains(*effect))
            .map(|effect| (effect_def(effect), ShaderDefValue::Bool(true)))
            .collect();
        let mut shared = |name: &str, effects: [MaskEffect; 2]| {
            if effects.into_iter().any(|effect| self.contains(effect)) {
                defs.insert(name.to_owned(), ShaderDefValue::Bool(true));
            }
        };
        // Light that Glow and Neon cast on the scene (scene_emission.wgsl).
        shared(SCENE_EMISSION_DEF, [MaskEffect::Glow, MaskEffect::Neon]);
        // The cores of Glowing Areas and of the legacy highlight Glow
        // (`apply_mask_glow_cores`).
        shared(
            GLOW_CORES_DEF,
            [MaskEffect::Glow, MaskEffect::LegacyHighlightGlow],
        );
        defs
    }
}

const SCENE_EMISSION_DEF: &str = "EFFECTS_SCENE_EMISSION";
const GLOW_CORES_DEF: &str = "EFFECTS_GLOW_CORES";

/// The shader def that compiles `effect` into the creative effects module:
/// `EFFECT_` and [`MaskEffect::shader_name`] in upper case.
pub(super) fn effect_def(effect: MaskEffect) -> String {
    format!("EFFECT_{}", effect.shader_name().to_uppercase())
}

/// The effects whose code the program `entry` compiles only when they are
/// defined, or `None` for a program that is the same for every set. These
/// lists name the effects behind an `#ifdef` on the program's path:
///
/// - the creative pass: the per-slot effects the registry dispatches
///   (effect_wgsl.rs), the effects it calls after them, and Neon, whose scene
///   light shares Glow's def;
/// - the first Blur stage: the focus blurs (`mask_focus_blur` and
///   `apply_mask_focus_blurs` in blur.wgsl). Blur itself runs in every stage.
fn dispatched_effects(entry: &str) -> Option<EffectSet> {
    match entry {
        "apply_creative_effects" => Some(EffectSet::from_effects(
            MaskEffect::ALL.into_iter().filter(|effect| {
                effect.runs_in(EffectPass::Creative)
                    || effect.runs_in(EffectPass::CreativeLight)
                    || *effect == MaskEffect::Neon
            }),
        )),
        "diffuse_mask_blur_0" => Some(EffectSet::from_effects([
            MaskEffect::Defocus,
            MaskEffect::LensBlur,
            MaskEffect::MotionBlur,
            MaskEffect::RadialBlur,
            MaskEffect::TiltShift,
        ])),
        _ => None,
    }
}

/// Whether a render waits for its set's program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Readiness {
    /// Compile the program now if it is missing (exports and other
    /// full-quality renders, where a slow program costs every tile).
    Wait,
    /// Use the program with every effect while the set's compiles on a
    /// worker (previews, which must not stall on a compile).
    WhenReady,
}

/// The creative effects modules of the effect sets met so far, shared by the
/// programs of one template.
pub(super) struct EffectModules {
    device: wgpu::Device,
    work_format: wgpu::TextureFormat,
    cfa_kind: CfaKind,
    state: Mutex<ModuleState>,
}

#[derive(Default)]
struct ModuleState {
    /// Composes the modules; built for the first set and kept, as registering
    /// the modules it imports costs more than composing one set.
    manager: Option<ShaderManager>,
    modules: HashMap<EffectSet, wgpu::ShaderModule>,
}

impl EffectModules {
    pub(super) fn new(
        device: &wgpu::Device,
        work_format: wgpu::TextureFormat,
        cfa_kind: CfaKind,
    ) -> Arc<Self> {
        Arc::new(Self {
            device: device.clone(),
            work_format,
            cfa_kind,
            state: Mutex::default(),
        })
    }

    /// The creative effects module with the effects of `set`.
    fn module(&self, set: EffectSet) -> Result<wgpu::ShaderModule> {
        // A panic while composing leaves the cache as it was.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(module) = state.modules.get(&set) {
            return Ok(module.clone());
        }
        let ModuleState { manager, modules } = &mut *state;
        let manager = match manager {
            Some(manager) => manager,
            None => manager.insert(
                ShaderManager::new(self.work_format, self.cfa_kind)
                    .context("initialize the WGSL composer for an effect set")?,
            ),
        };
        let source = shaders::CREATIVE_EFFECTS_ENTRY.source;
        let module = manager.create_shader_module_with_defs(
            &self.device,
            "calibraw creative effects of an effect set",
            &source.module_text(),
            source.file_name,
            set.shader_defs(),
        )?;
        modules.insert(set, module.clone());
        Ok(module)
    }
}

/// One program's variants per effect set.
pub(super) struct EffectPrograms(Arc<ProgramVariants>);

struct ProgramVariants {
    device: wgpu::Device,
    layouts: [wgpu::BindGroupLayout; 3],
    entry: String,
    cache: Option<Arc<PersistentGpuPipelineCache>>,
    /// The effects the program dispatches (`dispatched_effects`).
    dispatched: EffectSet,
    modules: Arc<EffectModules>,
    /// By the set's dispatched effects. A cell holds `None` for a set whose
    /// program failed to compile, which then renders with every effect.
    programs: Mutex<HashMap<EffectSet, Arc<OnceLock<Option<wgpu::ComputePipeline>>>>>,
}

impl EffectPrograms {
    /// The variants of the program `entry`, or `None` for a program that
    /// dispatches no effects.
    pub(super) fn for_entry(
        device: &wgpu::Device,
        layouts: [wgpu::BindGroupLayout; 3],
        entry: &str,
        cache: Option<Arc<PersistentGpuPipelineCache>>,
        modules: &Arc<EffectModules>,
    ) -> Option<Self> {
        let dispatched = dispatched_effects(entry)?;
        Some(Self(Arc::new(ProgramVariants {
            device: device.clone(),
            layouts,
            entry: entry.to_owned(),
            cache,
            dispatched,
            modules: Arc::clone(modules),
            programs: Mutex::default(),
        })))
    }

    /// The program for an edit whose active effects are `set`, or `None` to
    /// render with the program that holds every effect: when the edit uses
    /// every effect the program dispatches, when the set's program failed to
    /// compile, and with [`Readiness::WhenReady`] until it is compiled.
    pub(super) fn program(
        &self,
        set: EffectSet,
        readiness: Readiness,
    ) -> Option<wgpu::ComputePipeline> {
        let variants = &self.0;
        let key = set.intersection(variants.dispatched);
        if key == variants.dispatched {
            return None;
        }
        let (cell, new) = {
            // Cells are only added, so a panic elsewhere leaves a usable map.
            let mut programs = variants
                .programs
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let new = !programs.contains_key(&key);
            (Arc::clone(programs.entry(key).or_default()), new)
        };
        match readiness {
            // Waits for a worker that is compiling the same set.
            Readiness::Wait => cell.get_or_init(|| variants.compile(key)).clone(),
            Readiness::WhenReady => {
                if new {
                    let variants = Arc::clone(variants);
                    let worker = Arc::clone(&cell);
                    let spawned = std::thread::Builder::new()
                        .name("calibraw-effect-program".to_owned())
                        .spawn(move || {
                            worker.get_or_init(|| variants.compile(key));
                            variants.persist_cache();
                        });
                    if let Err(error) = spawned {
                        // The set renders with every effect, as before.
                        log::error!("could not start an effect program compile: {error}");
                        cell.get_or_init(|| None);
                    }
                }
                cell.get().cloned().flatten()
            }
        }
    }
}

impl ProgramVariants {
    /// Compiles the program for `key`; `None`, with the error logged, if it
    /// does not compile.
    fn compile(&self, key: EffectSet) -> Option<wgpu::ComputePipeline> {
        let started = std::time::Instant::now();
        let scopes = GpuErrorScopes::push(&self.device);
        let pipeline = self.modules.module(key).map(|module| {
            let layout = self
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(&self.entry),
                    bind_group_layouts: &self.layouts.each_ref().map(Some),
                    immediate_size: 0,
                });
            self.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(&self.entry),
                    layout: Some(&layout),
                    module: &module,
                    entry_point: Some(&self.entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: self.cache.as_ref().map(|cache| cache.raw()),
                })
        });
        let result = scopes
            .finish("compile an effect set's program")
            .and(pipeline);
        match result {
            Ok(pipeline) => {
                log::debug!(
                    "GPU program {} for effect set {:#x} compiled in {:.3}s",
                    self.entry,
                    key.bits(),
                    started.elapsed().as_secs_f64()
                );
                Some(pipeline)
            }
            Err(error) => {
                // The render is the same with the program that holds every
                // effect, only slower on some GPUs, so nothing is shown.
                let message = format!(
                    "GPU program {} for effect set {:#x} did not compile, rendering with every effect: {error:#}",
                    self.entry,
                    key.bits()
                );
                log::error!("{message}");
                calibraw_core::diagnostics::record(message);
                None
            }
        }
    }

    /// Saves the programs compiled so far, so the next launch loads this
    /// set's from the persistent cache.
    fn persist_cache(&self) {
        if let Some(cache) = &self.cache {
            if let Err(error) = cache.persist() {
                log::warn!("could not save the GPU pipeline cache: {error:#}");
            }
        }
    }
}
