//! When feature tips are offered, and what the user's answers change.
//!
//! A tip is offered from the handler of the feature it explains, the first
//! time that feature is used. Its content lives in `ui::tips`.

use super::*;
use crate::ui::tips::{Tip, TipAnimation, TipChoice};
use std::collections::BTreeSet;

/// The tip choices saved in settings.
#[derive(Debug)]
pub(crate) struct TipPreferences {
    pub(crate) enabled: bool,
    /// Keys of the tips already shown. Kept as strings, so keys written by
    /// other versions survive a save.
    pub(crate) seen: BTreeSet<String>,
}

/// A tip that was offered, waiting for other dialogs to close or on screen.
pub(crate) struct ShownTip {
    tip: Tip,
    /// `None` when the clip could not be decoded; the tip then shows its text.
    animation: Option<TipAnimation>,
}

/// The tip explaining a newly created mask of `kind`.
pub(in crate::app) fn mask_tip(kind: MaskKind) -> Option<Tip> {
    match kind {
        MaskKind::Object => Some(Tip::ObjectMask),
        _ => None,
    }
}

impl CalibRawApp {
    /// Shows `tip` unless tips are off or it was seen. Only one tip is held:
    /// another offered meanwhile is not marked as seen, so it returns the
    /// next time its feature is used.
    pub(in crate::app) fn offer_tip(&mut self, tip: Tip) {
        let tips = &self.preferences.tips;
        if !tips.enabled || tips.seen.contains(tip.key()) || self.ui.tip.is_some() {
            return;
        }
        let animation = match TipAnimation::new(&self.egui_ctx, tip.key(), tip.media()) {
            Ok(animation) => Some(animation),
            Err(error) => {
                self.report_tip_animation_error(tip, &error);
                None
            }
        };
        self.ui.tip = Some(ShownTip { tip, animation });
        self.egui_ctx.request_repaint();
    }

    /// Shows the offered tip once no other dialog is open.
    pub(in crate::app) fn show_tip_dialog(&mut self, ctx: &egui::Context) {
        if self.dialog_open(ctx) {
            return;
        }
        let Some(shown) = self.ui.tip.as_mut() else {
            return;
        };
        let tip = shown.tip;
        if let Some(Err(error)) = shown
            .animation
            .as_mut()
            .map(|animation| animation.advance(ctx))
        {
            shown.animation = None;
            self.report_tip_animation_error(tip, &error);
            return;
        }
        if let Some(choice) = crate::ui::tips::show(ctx, tip, shown.animation.as_ref()) {
            self.answer_tip(choice);
        }
    }

    /// Closes the shown tip and marks it as seen.
    fn answer_tip(&mut self, choice: TipChoice) {
        let Some(shown) = self.ui.tip.take() else {
            return;
        };
        self.preferences
            .tips
            .seen
            .insert(shown.tip.key().to_owned());
        if choice == TipChoice::DisableTips {
            self.preferences.tips.enabled = false;
        }
        self.save_tip_preferences();
    }

    pub(crate) fn set_tips_enabled(&mut self, enabled: bool) {
        if self.preferences.tips.enabled != enabled {
            self.preferences.tips.enabled = enabled;
            self.save_tip_preferences();
        }
    }

    /// Shows every tip again the next time its feature is used.
    pub(crate) fn reset_seen_tips(&mut self) {
        if !self.preferences.tips.seen.is_empty() {
            self.preferences.tips.seen.clear();
            self.save_tip_preferences();
        }
    }

    fn save_tip_preferences(&mut self) {
        if !self.persist_performance_settings() {
            self.report_error(
                ErrorKind::Settings,
                "CalibRaw could not save your tip preferences.",
            );
        }
    }

    fn report_tip_animation_error(&mut self, tip: Tip, error: &image_webp::DecodingError) {
        self.report_error(
            ErrorKind::Tip,
            format!(
                "The animation of the “{}” tip could not be played: {error}",
                tip.title()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_tips(enabled: bool) -> CalibRawApp {
        let mut app = CalibRawApp::empty(&egui::Context::default());
        // Never write the developer's own settings file.
        app.preferences.performance_settings_path = None;
        app.preferences.tips = TipPreferences {
            enabled,
            seen: BTreeSet::new(),
        };
        app
    }

    fn offered(app: &CalibRawApp) -> Option<Tip> {
        app.ui.tip.as_ref().map(|shown| shown.tip)
    }

    #[test]
    fn offers_an_unseen_tip_with_its_animation() {
        let mut app = app_with_tips(true);
        app.offer_tip(Tip::ObjectMask);
        assert_eq!(offered(&app), Some(Tip::ObjectMask));
        assert!(app
            .ui
            .tip
            .as_ref()
            .is_some_and(|shown| shown.animation.is_some()));
    }

    #[test]
    fn skips_tips_when_disabled_or_seen() {
        let mut app = app_with_tips(false);
        app.offer_tip(Tip::ObjectMask);
        assert_eq!(offered(&app), None);

        let mut app = app_with_tips(true);
        app.preferences
            .tips
            .seen
            .insert(Tip::ObjectMask.key().to_owned());
        app.offer_tip(Tip::ObjectMask);
        assert_eq!(offered(&app), None);
    }

    #[test]
    fn a_held_tip_is_not_replaced() {
        let mut app = app_with_tips(true);
        app.offer_tip(Tip::MasksTab);
        app.offer_tip(Tip::ObjectMask);
        assert_eq!(offered(&app), Some(Tip::MasksTab));
        assert!(app.preferences.tips.seen.is_empty());
    }

    #[test]
    fn switching_to_masks_offers_the_masks_tip_once() {
        let mut app = app_with_tips(true);
        assert!(app.dispatch_action(AppAction::SelectSidebarTab(SidebarTab::Masks)));
        assert_eq!(offered(&app), Some(Tip::MasksTab));

        app.ui.tip = None;
        app.preferences
            .tips
            .seen
            .insert(Tip::MasksTab.key().to_owned());
        app.dispatch_action(AppAction::SelectSidebarTab(SidebarTab::Adjustments));
        app.dispatch_action(AppAction::SelectSidebarTab(SidebarTab::Masks));
        assert_eq!(offered(&app), None);
    }

    #[test]
    fn only_object_masks_have_a_creation_tip() {
        assert_eq!(mask_tip(MaskKind::Object), Some(Tip::ObjectMask));
        assert_eq!(mask_tip(MaskKind::Brush), None);
    }

    #[test]
    fn answers_mark_the_tip_seen_and_can_disable_tips() {
        let mut app = app_with_tips(true);
        app.offer_tip(Tip::MasksTab);
        app.answer_tip(TipChoice::Dismiss);
        assert_eq!(offered(&app), None);
        assert!(app.preferences.tips.enabled);
        assert!(app.preferences.tips.seen.contains(Tip::MasksTab.key()));

        app.offer_tip(Tip::ObjectMask);
        app.answer_tip(TipChoice::DisableTips);
        assert!(!app.preferences.tips.enabled);
        assert!(app.preferences.tips.seen.contains(Tip::ObjectMask.key()));
    }
}
