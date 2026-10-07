//! On-canvas light handles for Relight. The preview
//! (`ui::preview::tools::light`) draws a handle for each light whose card is
//! on screen and returns these actions; `apply_light_handle_actions` edits
//! the effect settings, exactly as the card's position pad and depth slider do.

use super::*;
use crate::pipeline::{effect_params::relight as params, EffectComponent, MaskEffect};

/// One effect component of the edit: a global effect or one stacked on a mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EffectComponentRef {
    Global(usize),
    Mask { mask: usize, component: usize },
}

/// A Relight light the preview can move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightHandle {
    pub(crate) target: EffectComponentRef,
    /// Percentages of the full, uncropped image, as stored in the settings.
    pub(crate) source: [f32; 2],
    /// Light depth as stored (-100 at the camera, 100 at the farthest surface).
    pub(crate) depth: f32,
    /// Picker color (sRGB, 0–1).
    pub(crate) color: [f32; 3],
}

pub(crate) enum LightHandleAction {
    /// Place the light at `source` (percent of the full image).
    Move {
        target: EffectComponentRef,
        source: [f32; 2],
    },
    SetDepth {
        target: EffectComponentRef,
        depth: f32,
    },
}

fn is_relight(component: &EffectComponent) -> bool {
    component.effect == MaskEffect::Relight && component.enabled
}

impl CalibRawApp {
    /// Lights of the enabled Relight cards the sidebar shows open: global
    /// effects on the Adjustments tab, the selected mask's effects on the
    /// Masks tab. A vertical layout shows the selected effect card alone;
    /// otherwise `card_open` says whether the Relight card is expanded.
    pub(crate) fn light_handles(&self, layout: ScreenLayout, card_open: bool) -> Vec<LightHandle> {
        let vertical = layout == ScreenLayout::Vertical;
        if !vertical && !card_open {
            return Vec::new();
        }
        let handle = |target, component: &EffectComponent| {
            let settings = &component.settings.relight;
            LightHandle {
                target,
                source: settings.source,
                depth: settings.depth,
                color: settings.color,
            }
        };
        match self.ui.sidebar_tab {
            SidebarTab::Adjustments => {
                if vertical
                    && (self.develop_ui.adjustment_section != AdjustmentSection::Effects
                        || self.develop_ui.effect_component != Some(MaskEffect::Relight))
                {
                    return Vec::new();
                }
                self.masks
                    .stack
                    .global_effects
                    .iter()
                    .enumerate()
                    .filter(|(_, component)| is_relight(component))
                    .map(|(index, component)| handle(EffectComponentRef::Global(index), component))
                    .collect()
            }
            SidebarTab::Masks => {
                if vertical
                    && (self.develop_ui.mask_section != MaskSection::Effects
                        || self.develop_ui.mask_effect_component != Some(MaskEffect::Relight))
                {
                    return Vec::new();
                }
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
                mask.effect_components
                    .iter()
                    .enumerate()
                    .filter(|(_, component)| is_relight(component))
                    .map(|(component_index, component)| {
                        handle(
                            EffectComponentRef::Mask {
                                mask: mask_index,
                                component: component_index,
                            },
                            component,
                        )
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn apply_light_handle_actions(&mut self, actions: Vec<LightHandleAction>) {
        let mut changed = false;
        for action in actions {
            changed |= self.apply_light_handle_action(action);
        }
        if changed {
            self.mark_mask_adjustments_dirty();
        }
    }

    fn apply_light_handle_action(&mut self, action: LightHandleAction) -> bool {
        let target = match action {
            LightHandleAction::Move { target, .. } | LightHandleAction::SetDepth { target, .. } => {
                target
            }
        };
        // A stale target (the component was removed or replaced) is ignored.
        let Some(settings) = self
            .effect_component_mut(target)
            .filter(|component| component.effect == MaskEffect::Relight)
            .map(|component| &mut component.settings.relight)
        else {
            return false;
        };
        let before = *settings;
        match action {
            LightHandleAction::Move { source, .. } => {
                settings.source = [
                    params::SOURCE_X.clamp(source[0]),
                    params::SOURCE_Y.clamp(source[1]),
                ];
            }
            LightHandleAction::SetDepth { depth, .. } => {
                settings.depth = params::DEPTH.clamp(depth);
            }
        }
        *settings != before
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{LocalMask, MaskKind};

    fn app_with_lights() -> CalibRawApp {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        let stack = &mut app.masks.stack;
        stack
            .global_effects
            .push(EffectComponent::new(MaskEffect::Fog));
        stack
            .global_effects
            .push(EffectComponent::new(MaskEffect::Relight));
        let mut mask = LocalMask::new(MaskKind::Brush, 1);
        mask.effect_components
            .push(EffectComponent::new(MaskEffect::Relight));
        stack.masks.push(mask);
        stack.selected_mask = Some(0);
        app
    }

    fn targets(app: &CalibRawApp, layout: ScreenLayout) -> Vec<EffectComponentRef> {
        app.light_handles(layout, true)
            .into_iter()
            .map(|handle| handle.target)
            .collect()
    }

    #[test]
    fn handles_follow_the_effect_cards_the_sidebar_shows() {
        let mut app = app_with_lights();
        app.ui.sidebar_tab = SidebarTab::Adjustments;
        assert_eq!(
            targets(&app, ScreenLayout::Horizontal),
            [EffectComponentRef::Global(1)]
        );
        assert!(targets(&app, ScreenLayout::Vertical).is_empty());
        // A collapsed card on a wide layout shows no light.
        assert!(app
            .light_handles(ScreenLayout::Horizontal, false)
            .is_empty());
        app.develop_ui.adjustment_section = AdjustmentSection::Effects;
        app.develop_ui.effect_component = Some(MaskEffect::Relight);
        assert_eq!(
            targets(&app, ScreenLayout::Vertical),
            [EffectComponentRef::Global(1)]
        );

        app.ui.sidebar_tab = SidebarTab::Masks;
        let in_mask = [EffectComponentRef::Mask {
            mask: 0,
            component: 0,
        }];
        assert_eq!(targets(&app, ScreenLayout::Horizontal), in_mask);
        app.masks.stack.masks[0].effect_components[0].enabled = false;
        assert!(targets(&app, ScreenLayout::Horizontal).is_empty());
        app.masks.stack.masks[0].effect_components[0].enabled = true;
        app.masks.stack.masks[0].enabled = false;
        assert!(targets(&app, ScreenLayout::Horizontal).is_empty());
        app.masks.stack.masks[0].enabled = true;
        app.masks.stack.selected_mask = None;
        assert!(targets(&app, ScreenLayout::Horizontal).is_empty());

        app.ui.sidebar_tab = SidebarTab::Crop;
        assert!(targets(&app, ScreenLayout::Horizontal).is_empty());
    }

    #[test]
    fn actions_edit_clamped_settings_and_ignore_stale_targets() {
        let mut app = app_with_lights();
        app.apply_light_handle_actions(vec![
            LightHandleAction::Move {
                target: EffectComponentRef::Global(1),
                source: [-80.0, 42.5],
            },
            LightHandleAction::SetDepth {
                target: EffectComponentRef::Mask {
                    mask: 0,
                    component: 0,
                },
                depth: 250.0,
            },
            // Fog, a missing component and a missing mask stay untouched.
            LightHandleAction::SetDepth {
                target: EffectComponentRef::Global(0),
                depth: 10.0,
            },
            LightHandleAction::Move {
                target: EffectComponentRef::Global(7),
                source: [1.0, 1.0],
            },
            LightHandleAction::SetDepth {
                target: EffectComponentRef::Mask {
                    mask: 3,
                    component: 0,
                },
                depth: 1.0,
            },
        ]);
        let stack = &app.masks.stack;
        assert_eq!(
            stack.global_effects[1].settings.relight.source,
            [params::SOURCE_X.min, 42.5]
        );
        assert_eq!(
            stack.masks[0].effect_components[0].settings.relight.depth,
            params::DEPTH.max
        );
        assert_eq!(
            stack.global_effects[0].settings,
            crate::pipeline::MaskEffectSettings::default()
        );
    }
}
