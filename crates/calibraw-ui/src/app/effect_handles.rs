//! Edits of effect components made outside their own cards.
//!
//! On-canvas controls for effects with a place in the photo: Relight's
//! lights and Light Rays' sources, the Radial Blur and Vignette centers and
//! the Tilt-Shift focus band. The preview (`ui::preview::tools::effect_handles`)
//! draws them for each such effect whose card is on screen and returns these
//! actions; `apply_effect_handle_actions` edits the settings exactly as the
//! card's position pad and sliders do, within the same parameter ranges.
//!
//! Which light or source each card shows ([`SelectedInstances`]), and the
//! Effects card's add buttons (`add_global_effect`).

use super::*;
use crate::pipeline::{
    effect_params, effect_places, EffectComponent, MaskEffect, MaskEffectSettings,
    MAX_EFFECT_COMPONENTS,
};
use std::collections::HashMap;

/// One effect component of the edit: a global effect or one stacked on a mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EffectComponentRef {
    Global(usize),
    Mask { mask: usize, component: usize },
}

impl EffectComponentRef {
    fn list(self) -> EffectList {
        match self {
            Self::Global(_) => EffectList::Global,
            Self::Mask { mask, .. } => EffectList::Mask(mask),
        }
    }
}

/// A list of effect components: the Effects card's or a mask's. Each effect
/// appears in a list at most once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EffectList {
    Global,
    Mask(usize),
}

/// The instance (light or ray source) each effect card with several shows,
/// and whose canvas handle is active. Not saved; an instance that no longer
/// exists reads as the last one.
#[derive(Debug, Default)]
pub(crate) struct SelectedInstances(HashMap<(EffectList, MaskEffect), usize>);

impl SelectedInstances {
    /// The instance `effect` in `list` shows, below `count`.
    pub(crate) fn get(&self, list: EffectList, effect: MaskEffect, count: usize) -> usize {
        self.0
            .get(&(list, effect))
            .copied()
            .unwrap_or(0)
            .min(count.saturating_sub(1))
    }

    /// The entry a card edits when it changes the selection or adds or
    /// removes an instance.
    pub(crate) fn entry(&mut self, list: EffectList, effect: MaskEffect) -> &mut usize {
        self.0.entry((list, effect)).or_insert(0)
    }
}

/// Effects with on-canvas controls.
pub(crate) const EFFECTS_WITH_HANDLES: [MaskEffect; 5] = [
    MaskEffect::Relight,
    MaskEffect::LightRays,
    MaskEffect::RadialBlur,
    MaskEffect::TiltShift,
    MaskEffect::Vignette,
];

/// An effect the preview shows controls for, with a copy of its settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EffectHandle {
    pub(crate) target: EffectComponentRef,
    pub(crate) effect: MaskEffect,
    pub(crate) settings: MaskEffectSettings,
    /// The instance its card shows, whose handle is drawn active.
    pub(crate) selected: usize,
}

/// One edit made on the canvas. Positions and widths are in the units the
/// settings store: percentages of the effect's frame, of the shorter image
/// edge, or degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EffectHandleEdit {
    /// Makes the instance the one its card shows.
    Select,
    /// The source, center or focus position.
    Position([f32; 2]),
    /// Relight's light depth.
    Depth(f32),
    /// Tilt-Shift's band angle.
    Angle(f32),
    /// Tilt-Shift's sharp band width.
    FocusWidth(f32),
    /// Tilt-Shift's transition width.
    Feather(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EffectHandleAction {
    pub(crate) target: EffectComponentRef,
    /// The light or source edited; 0 for an effect with one instance.
    pub(crate) instance: usize,
    pub(crate) edit: EffectHandleEdit,
}

impl CalibRawApp {
    /// Effects with canvas controls among the enabled cards the sidebar shows
    /// open: global effects on the Adjustments tab, the selected mask's
    /// effects on the Masks tab. A vertical layout shows the selected effect
    /// card alone; otherwise `card_open` says whether an effect's card is
    /// expanded.
    pub(crate) fn effect_handles(
        &self,
        layout: ScreenLayout,
        card_open: impl Fn(MaskEffect) -> bool,
    ) -> Vec<EffectHandle> {
        let vertical = layout == ScreenLayout::Vertical;
        let (components, selected, section_shown, mask_index) = match self.ui.sidebar_tab {
            SidebarTab::Adjustments => (
                &self.masks.stack.global_effects,
                self.develop_ui.effect_component,
                self.develop_ui.adjustment_section == AdjustmentSection::Effects,
                None,
            ),
            SidebarTab::Masks => {
                let Some(mask_index) = self.masks.stack.selected_mask else {
                    return Vec::new();
                };
                let Some(mask) = self
                    .masks
                    .stack
                    .masks
                    .get(mask_index)
                    .filter(|mask| mask.enabled)
                else {
                    return Vec::new();
                };
                (
                    &mask.effect_components,
                    self.develop_ui.mask_effect_component,
                    self.develop_ui.mask_section == MaskSection::Effects,
                    Some(mask_index),
                )
            }
            _ => return Vec::new(),
        };
        let shown = |effect: MaskEffect| {
            if vertical {
                section_shown && selected == Some(effect)
            } else {
                card_open(effect)
            }
        };
        components
            .iter()
            .enumerate()
            .filter(|(_, component)| {
                component.enabled
                    && EFFECTS_WITH_HANDLES.contains(&component.effect)
                    && shown(component.effect)
            })
            .map(|(index, component)| {
                let target = match mask_index {
                    None => EffectComponentRef::Global(index),
                    Some(mask) => EffectComponentRef::Mask {
                        mask,
                        component: index,
                    },
                };
                EffectHandle {
                    target,
                    effect: component.effect,
                    settings: component.settings,
                    selected: self.develop_ui.effect_instances.get(
                        target.list(),
                        component.effect,
                        component.places(),
                    ),
                }
            })
            .collect()
    }

    pub(crate) fn apply_effect_handle_actions(&mut self, actions: Vec<EffectHandleAction>) {
        let mut changed = false;
        for action in actions {
            changed |= self.apply_effect_handle_action(action);
        }
        if changed {
            self.mark_mask_adjustments_dirty();
        }
    }

    fn apply_effect_handle_action(&mut self, action: EffectHandleAction) -> bool {
        // A stale target (the component was removed or replaced) is ignored.
        let Some(component) = self.effect_component_mut(action.target) else {
            return false;
        };
        let effect = component.effect;
        let before = component.settings;
        apply_edit(
            effect,
            &mut component.settings,
            action.instance,
            action.edit,
        );
        let changed = component.settings != before;
        if action.instance < component.places() {
            *self
                .develop_ui
                .effect_instances
                .entry(action.target.list(), effect) = action.instance;
        }
        changed
    }

    /// Adds `effect` to the global effects, as the Effects card's buttons do.
    /// A vertical layout opens the new effect's tab, as adding it from the
    /// tab strip does. Returns whether it was added: an effect already
    /// present, or a full list, is left as it is.
    pub(crate) fn add_global_effect(&mut self, effect: MaskEffect, layout: ScreenLayout) -> bool {
        let effects = &mut self.masks.stack.global_effects;
        if effect_places(effects) >= MAX_EFFECT_COMPONENTS
            || effects.iter().any(|component| component.effect == effect)
        {
            return false;
        }
        effects.push(EffectComponent::new(effect));
        if layout == ScreenLayout::Vertical {
            self.develop_ui.effect_component = Some(effect);
        }
        self.mark_mask_adjustments_dirty();
        true
    }

    fn effect_component_mut(&mut self, target: EffectComponentRef) -> Option<&mut EffectComponent> {
        let stack = &mut self.masks.stack;
        match target {
            EffectComponentRef::Global(index) => stack.global_effects.get_mut(index),
            EffectComponentRef::Mask { mask, component } => stack
                .masks
                .get_mut(mask)?
                .effect_components
                .get_mut(component),
        }
    }
}

/// Applies `edit` to `instance` within the parameter ranges; edits an effect
/// or instance does not have are ignored.
fn apply_edit(
    effect: MaskEffect,
    settings: &mut MaskEffectSettings,
    instance: usize,
    edit: EffectHandleEdit,
) {
    use effect_params::{light_rays, radial_blur, relight, tilt_shift, vignette};
    use EffectHandleEdit::{Angle, Depth, Feather, FocusWidth, Position, Select};
    let place =
        |slot: &mut [f32; 2], specs: [effect_params::FloatParamSpec; 2], position: [f32; 2]| {
            *slot = [specs[0].clamp(position[0]), specs[1].clamp(position[1])];
        };
    match (effect, edit) {
        (_, Select) => {}
        (MaskEffect::Relight, Position(position)) => {
            if let Some(light) = settings.relight.lights.get_mut(instance) {
                place(
                    &mut light.source,
                    [relight::SOURCE_X, relight::SOURCE_Y],
                    position,
                );
            }
        }
        (MaskEffect::Relight, Depth(depth)) => {
            if let Some(light) = settings.relight.lights.get_mut(instance) {
                light.light_depth = relight::LIGHT_DEPTH.clamp(depth);
            }
        }
        (MaskEffect::LightRays, Position(position)) => {
            if let Some(source) = settings.light_rays.sources.get_mut(instance) {
                place(
                    &mut source.source,
                    [light_rays::SOURCE_X, light_rays::SOURCE_Y],
                    position,
                );
            }
        }
        (MaskEffect::RadialBlur, Position(position)) => place(
            &mut settings.radial_blur.center,
            [radial_blur::CENTER_X, radial_blur::CENTER_Y],
            position,
        ),
        (MaskEffect::Vignette, Position(position)) => place(
            &mut settings.vignette.center,
            [vignette::CENTER_X, vignette::CENTER_Y],
            position,
        ),
        (MaskEffect::TiltShift, Position(position)) => place(
            &mut settings.tilt_shift.center,
            [tilt_shift::CENTER_X, tilt_shift::CENTER_Y],
            position,
        ),
        (MaskEffect::TiltShift, Angle(angle)) => {
            settings.tilt_shift.angle = tilt_shift::ANGLE.clamp(angle);
        }
        (MaskEffect::TiltShift, FocusWidth(width)) => {
            settings.tilt_shift.focus_width = tilt_shift::FOCUS_WIDTH.clamp(width);
        }
        (MaskEffect::TiltShift, Feather(width)) => {
            settings.tilt_shift.feather = tilt_shift::FEATHER.clamp(width);
        }
        _ => {}
    }
}
