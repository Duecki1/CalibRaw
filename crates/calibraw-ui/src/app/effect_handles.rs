//! Edits of effect components made outside their own cards.
//!
//! On-canvas controls for effects with a place in the photo: Relight and
//! Light Rays sources, the Radial Blur and Vignette centers and the
//! Tilt-Shift focus band. The preview (`ui::preview::tools::effect_handles`)
//! draws them for each such effect whose card is on screen and returns these
//! actions; `apply_effect_handle_actions` edits the settings exactly as the
//! card's position pad and sliders do, within the same parameter ranges.
//!
//! The Effects card's add buttons (`add_global_effect`).

use super::*;
use crate::pipeline::{
    effect_params, EffectComponent, MaskEffect, MaskEffectSettings, MAX_EFFECT_COMPONENTS,
};

/// One effect component of the edit: a global effect or one stacked on a mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EffectComponentRef {
    Global(usize),
    Mask { mask: usize, component: usize },
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
}

/// One edit made on the canvas. Positions and widths are in the units the
/// settings store: percentages of the effect's frame, of the shorter image
/// edge, or degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EffectHandleEdit {
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
            .map(|(index, component)| EffectHandle {
                target: match mask_index {
                    None => EffectComponentRef::Global(index),
                    Some(mask) => EffectComponentRef::Mask {
                        mask,
                        component: index,
                    },
                },
                effect: component.effect,
                settings: component.settings,
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
        let before = component.settings;
        apply_edit(component.effect, &mut component.settings, action.edit);
        component.settings != before
    }

    /// Adds `effect` to the global effects, as the Effects card's buttons do.
    /// A vertical layout opens the new effect's tab, as adding it from the
    /// tab strip does. Returns whether it was added: an effect already
    /// present, or a full list, is left as it is.
    pub(crate) fn add_global_effect(&mut self, effect: MaskEffect, layout: ScreenLayout) -> bool {
        let effects = &mut self.masks.stack.global_effects;
        if effects.len() >= MAX_EFFECT_COMPONENTS
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

/// Applies `edit` within the parameter ranges; edits an effect does not have
/// are ignored.
fn apply_edit(effect: MaskEffect, settings: &mut MaskEffectSettings, edit: EffectHandleEdit) {
    use effect_params::{light_rays, radial_blur, relight, tilt_shift, vignette};
    use EffectHandleEdit::{Angle, Depth, Feather, FocusWidth, Position};
    let place =
        |slot: &mut [f32; 2], specs: [effect_params::FloatParamSpec; 2], position: [f32; 2]| {
            *slot = [specs[0].clamp(position[0]), specs[1].clamp(position[1])];
        };
    match (effect, edit) {
        (MaskEffect::Relight, Position(position)) => place(
            &mut settings.relight.source,
            [relight::SOURCE_X, relight::SOURCE_Y],
            position,
        ),
        (MaskEffect::Relight, Depth(depth)) => settings.relight.depth = relight::DEPTH.clamp(depth),
        (MaskEffect::LightRays, Position(position)) => place(
            &mut settings.light_rays.source,
            [light_rays::SOURCE_X, light_rays::SOURCE_Y],
            position,
        ),
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
