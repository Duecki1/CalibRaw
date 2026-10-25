//! The amount of the preset applied last: how far its adjustments and LUT
//! move the photo from where it was before. It lasts while the edit is what
//! the amount produced; any other change, an undo or another photo ends it.
//! Saved edits keep the faded settings, never the preset or its amount.
use super::*;
use crate::luts::LookLut;
use crate::pipeline::AdjustmentGroupSet;

/// The settings a preset's amount fades.
#[derive(Clone, Debug, PartialEq)]
struct FadedSettings {
    exposure: ExposureParams,
    look_lut: Option<LookLut>,
}

pub(crate) struct AppliedPreset {
    name: String,
    groups: AdjustmentGroupSet,
    fades_lut: bool,
    document_generation: u64,
    before: FadedSettings,
    applied: FadedSettings,
    /// 0–1.
    amount: f32,
    /// What `amount` gave the photo.
    produced: FadedSettings,
}

impl CalibRawApp {
    fn faded_settings(&self) -> FadedSettings {
        FadedSettings {
            exposure: self.develop.exposure,
            look_lut: self.develop.look_lut.clone(),
        }
    }

    /// Lets the Presets panel change the amount of `preset`, which was just
    /// applied over `before`.
    fn begin_preset_amount(&mut self, preset: &Preset, before: FadedSettings) {
        let selection = preset.selection();
        let applied = self.faded_settings();
        self.presets.applied =
            (!selection.adjustment_groups.is_empty() || selection.lut).then(|| AppliedPreset {
                name: preset.name().to_owned(),
                groups: selection.adjustment_groups,
                fades_lut: selection.lut,
                document_generation: self.persistence.document_generation,
                before,
                produced: applied.clone(),
                applied,
                amount: 1.0,
            });
    }

    /// The applied preset whose amount can still change, while the edit is
    /// what its amount produced.
    fn current_applied_preset(&self) -> Option<&AppliedPreset> {
        self.presets.applied.as_ref().filter(|applied| {
            applied.document_generation == self.persistence.document_generation
                && applied.produced == self.faded_settings()
        })
    }

    /// The name and amount (0–1) of the preset whose amount can still change.
    pub(crate) fn applied_preset_amount(&self) -> Option<(&str, f32)> {
        self.current_applied_preset()
            .map(|applied| (applied.name.as_str(), applied.amount))
    }

    /// Fades the applied preset's adjustments and LUT to `amount` (0–1) of
    /// their effect, as an edit of the photo.
    pub(crate) fn set_applied_preset_amount(&mut self, amount: f32) {
        let Some(applied) = self.current_applied_preset() else {
            return;
        };
        let amount = amount.clamp(0.0, 1.0);
        let exposure = applied.applied.exposure.faded_from(
            &applied.before.exposure,
            applied.groups,
            amount,
            self.develop.loaded_raw.as_deref(),
        );
        let look_lut = if applied.fades_lut {
            faded_look(
                applied.before.look_lut.as_ref(),
                applied.applied.look_lut.as_ref(),
                amount,
            )
        } else {
            self.develop.look_lut.clone()
        };
        if exposure != self.develop.exposure {
            self.develop.exposure = exposure;
            self.mark_pipeline_dirty();
        }
        if look_lut != self.develop.look_lut {
            self.install_look_lut(look_lut);
            self.note_edit_changed();
        }
        let produced = self.faded_settings();
        if let Some(applied) = self.presets.applied.as_mut() {
            applied.amount = amount;
            applied.produced = produced;
        }
    }

    /// Applies `preset` through `apply`, then lets its amount change.
    pub(super) fn apply_preset_with_amount(
        &mut self,
        preset: &Preset,
        apply: impl FnOnce(&mut Self) -> Result<bool, String>,
    ) -> Result<bool, String> {
        let before = self.faded_settings();
        let result = apply(self);
        if result.is_ok() {
            self.begin_preset_amount(preset, before);
        }
        result
    }
}

/// The LUT of an applied preset at `amount`: the same LUT moves its amount
/// from `before`'s, another LUT grows from none. A preset without a LUT
/// keeps none.
fn faded_look(before: Option<&LookLut>, applied: Option<&LookLut>, amount: f32) -> Option<LookLut> {
    let applied = applied?;
    let from = before
        .filter(|before| before.id == applied.id)
        .map_or(0.0, |before| before.amount);
    Some(LookLut {
        amount: if amount >= 1.0 {
            applied.amount
        } else {
            from + (applied.amount - from) * amount
        },
        ..applied.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::luts::LutId;
    use crate::pipeline::AdjustmentGroup;

    fn look(id: &[u8], amount: f32) -> LookLut {
        LookLut {
            id: LutId::of_bytes(id),
            name: "Look".to_owned(),
            amount,
        }
    }

    #[test]
    fn a_new_lut_grows_from_none_and_the_same_lut_moves_its_amount() {
        let film = look(b"film", 0.8);
        assert_eq!(faded_look(None, Some(&film), 0.5).unwrap().amount, 0.4);
        let moved = faded_look(Some(&look(b"film", 0.4)), Some(&film), 0.5).unwrap();
        assert!((moved.amount - 0.6).abs() < 1e-6, "{}", moved.amount);
        assert_eq!(faded_look(Some(&film), None, 0.5), None);
        assert_eq!(faded_look(None, Some(&film), 1.0), Some(film));
    }

    #[test]
    fn the_amount_fades_the_applied_preset_until_another_edit() {
        let ctx = egui::Context::default();
        let mut app = CalibRawApp::empty(&ctx);
        app.develop.exposure.exposure = 0.5;
        let mut source = crate::sidecar::default_edit_state();
        source.exposure.exposure = 1.5;
        source.exposure.clarity = 40.0;
        let selection = EditSelection {
            adjustment_groups: [AdjustmentGroup::Light, AdjustmentGroup::Effects]
                .into_iter()
                .collect(),
            ..EditSelection::default()
        };
        let preset = Preset::new("Bright", "", selection, &source).unwrap();

        let result = app.apply_preset_with_amount(&preset, |app| {
            app.develop.exposure.exposure = 1.5;
            app.develop.exposure.clarity = 40.0;
            Ok(false)
        });
        assert!(result.is_ok());
        assert_eq!(app.applied_preset_amount(), Some(("Bright", 1.0)));

        app.set_applied_preset_amount(0.25);
        assert_eq!(app.develop.exposure.exposure, 0.75);
        assert_eq!(app.develop.exposure.clarity, 10.0);
        assert_eq!(app.applied_preset_amount(), Some(("Bright", 0.25)));
        app.set_applied_preset_amount(1.0);
        assert_eq!(app.develop.exposure.exposure, 1.5);

        // Another edit ends the amount; the faded settings stay.
        app.develop.exposure.contrast = 10.0;
        assert_eq!(app.applied_preset_amount(), None);
        app.set_applied_preset_amount(0.0);
        assert_eq!(app.develop.exposure.exposure, 1.5);
    }

    #[test]
    fn a_failed_apply_offers_no_amount() {
        let ctx = egui::Context::default();
        let mut app = CalibRawApp::empty(&ctx);
        let source = crate::sidecar::default_edit_state();
        let selection = EditSelection {
            adjustment_groups: [AdjustmentGroup::Light].into_iter().collect(),
            ..EditSelection::default()
        };
        let preset = Preset::new("Bright", "", selection, &source).unwrap();
        let result = app.apply_preset_with_amount(&preset, |_| Err("no photo".to_owned()));
        assert!(result.is_err());
        assert_eq!(app.applied_preset_amount(), None);
    }
}
