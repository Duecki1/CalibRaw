use super::*;

impl CalibRawApp {
    pub(in crate::app) fn show_subject_dialogs(&mut self, ctx: &egui::Context) {
        if let AiConsentState::Sky {
            runtime_download_needed,
        } = self.ai.consent
        {
            let model_download_needed =
                !crate::ai_masks::skyseg_model_is_verified(&self.skyseg_model_path());
            let runtime_ready = self.ai_runtime_ready();
            let mut action = crate::ui::theme::DialogAction::None;
            crate::ui::theme::dialog_window(
                "Download sky-selection model?", ctx, crate::ui::theme::DIALOG_WIDTH_LARGE,
            ).show_with_footer(
                ctx,
                |ui| {
                    Self::show_ai_download_summary(
                        ui,
                        "SkySeg U2Net (~176 MB)",
                        "create sky masks",
                        model_download_needed,
                        runtime_download_needed,
                    );
                    self.show_ai_download_details(
                        ui,
                        "sky-download-details",
                        model_download_needed,
                        runtime_download_needed,
                        &[("MIT model license", "https://github.com/xiongzhu666/Sky-Segmentation-and-Post-processing/blob/main/LICENSE")],
                        |ui| { ui.label("SkySeg U2Net runs locally on a 320 × 320 image and produces a sky probability mask. License: MIT."); },
                    );
                    self.show_manual_runtime_warning(ui);
                },
                |ui| {
                    action = Self::show_ai_consent_buttons(ui,
                        if model_download_needed || runtime_download_needed { "Accept & download" } else { "Continue" },
                        runtime_ready);
                },
            );
            match action {
                crate::ui::theme::DialogAction::Confirm => {
                    self.ai.consent = AiConsentState::None;
                    self.start_sky_worker(self.skyseg_model_path(), model_download_needed);
                }
                crate::ui::theme::DialogAction::Cancel => {
                    self.ai.consent = AiConsentState::None;
                    if self.ai.mask_update_active {
                        self.cancel_ai_mask_update();
                    }
                }
                crate::ui::theme::DialogAction::None => {}
            }
        }
        if let AiConsentState::Subject {
            runtime_download_needed,
        } = self.ai.consent
        {
            let model = self.ai.birefnet_quality.model();
            let model_download_needed = !crate::ai_masks::birefnet_model_is_verified(
                self.ai.birefnet_quality,
                &self.birefnet_model_path(),
            );
            let title = match (model_download_needed, runtime_download_needed) {
                (true, true) => "Download subject model and ONNX Runtime?",
                (true, false) => "Download subject-selection model?",
                (false, true) => "Download ONNX Runtime?",
                (false, false) => "Prepare subject selection?",
            };
            let runtime_ready = self.ai_runtime_ready();
            let mut action = crate::ui::theme::DialogAction::None;
            crate::ui::theme::dialog_window(title, ctx, crate::ui::theme::DIALOG_WIDTH_LARGE)
                .show_with_footer(
                    ctx,
                    |ui| {
                        Self::show_ai_download_summary(
                            ui,
                            &format!("BiRefNet (~{:.0} MB)", model.bytes as f64 / 1_000_000.0),
                            "create subject and background masks",
                            model_download_needed,
                            runtime_download_needed,
                        );
                        self.show_ai_download_details(
                            ui,
                            "subject-download-details",
                            model_download_needed,
                            runtime_download_needed,
                            &[(
                                "MIT model license",
                                "https://github.com/ZhengPeng7/BiRefNet/blob/main/LICENSE",
                            )],
                            |ui| {
                                ui.label(format!(
                                    "{} quality uses {} with a {} × {} input. License: MIT.",
                                    self.ai.birefnet_quality.label(),
                                    model.checkpoint,
                                    model.input_height,
                                    model.input_width,
                                ));
                            },
                        );
                        self.show_manual_runtime_warning(ui);
                    },
                    |ui| {
                        action = Self::show_ai_consent_buttons(
                            ui,
                            if model_download_needed || runtime_download_needed {
                                "Accept & download"
                            } else {
                                "Continue"
                            },
                            runtime_ready,
                        );
                    },
                );
            match action {
                crate::ui::theme::DialogAction::Confirm => {
                    self.ai.consent = AiConsentState::None;
                    self.start_subject_worker(self.birefnet_model_path(), model_download_needed);
                }
                crate::ui::theme::DialogAction::Cancel => {
                    self.ai.consent = AiConsentState::None;
                    if self.ai.mask_update_active {
                        self.cancel_ai_mask_update();
                    }
                }
                crate::ui::theme::DialogAction::None => {}
            }
        }

        if let AiConsentState::Object {
            runtime_download_needed,
        } = self.ai.consent
        {
            let (encoder, decoder) = self.sam21_model_paths();
            let model_download_needed =
                !crate::ai_masks::object_models_are_verified(&encoder, &decoder);
            let title = match (model_download_needed, runtime_download_needed) {
                (true, true) => "Download object model and ONNX Runtime?",
                (true, false) => "Download object-selection model?",
                (false, true) => "Download ONNX Runtime?",
                (false, false) => "Prepare object selection?",
            };
            let runtime_ready = self.ai_runtime_ready();
            let mut action = crate::ui::theme::DialogAction::None;
            crate::ui::theme::dialog_window(title, ctx, crate::ui::theme::DIALOG_WIDTH_LARGE)
                .show_with_footer(
                    ctx,
                    |ui| {
                        Self::show_ai_download_summary(
                            ui,
                            &format!(
                                "SAM 2.1 (~{:.0} MB)",
                                SAM21_MODEL_BYTES_ESTIMATE as f64 / 1_000_000.0
                            ),
                            "create object masks",
                            model_download_needed,
                            runtime_download_needed,
                        );
                        self.show_ai_download_details(
                            ui,
                            "object-download-details",
                            model_download_needed,
                            runtime_download_needed,
                            &[(
                                "Apache-2.0 model license",
                                "https://github.com/facebookresearch/sam2/blob/main/LICENSE",
                            )],
                            |ui| {
                                ui.label("SAM 2.1 Hiera Tiny uses an encoder and decoder with local edge-aware cleanup. License: Apache-2.0.");
                            },
                        );
                        self.show_manual_runtime_warning(ui);
                    },
                    |ui| {
                        action = Self::show_ai_consent_buttons(
                            ui,
                            if model_download_needed || runtime_download_needed {
                                "Accept & download"
                            } else {
                                "Continue"
                            },
                            runtime_ready,
                        );
                    },
                );
            match action {
                crate::ui::theme::DialogAction::Confirm => {
                    self.ai.consent = AiConsentState::None;
                    if let Some((mask_index, component_index)) =
                        self.ai.object_pending_target.take()
                    {
                        let (encoder, decoder) = self.sam21_model_paths();
                        self.start_object_worker(
                            mask_index,
                            component_index,
                            encoder,
                            decoder,
                            model_download_needed,
                        );
                    }
                }
                crate::ui::theme::DialogAction::Cancel => {
                    self.ai.consent = AiConsentState::None;
                    self.ai.object_pending_target = None;
                    if self.ai.mask_update_active {
                        self.cancel_ai_mask_update();
                    }
                }
                crate::ui::theme::DialogAction::None => {}
            }
        }

        if let Some(message) = self.ai.object_error_dialog.clone() {
            let mut close = false;
            crate::ui::theme::dialog_window(
                "AI mask failed",
                ctx,
                crate::ui::theme::DIALOG_WIDTH_DEFAULT,
            )
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(message);
                crate::ui::theme::dialog_button_row(ui, |ui| {
                    close |= crate::ui::theme::secondary_button(ui, "Close").clicked();
                });
                if !close
                    && crate::ui::theme::dialog_keyboard_action(
                        ui,
                        crate::ui::theme::DialogKeyboard::CLOSE_ONLY,
                        false,
                    ) == crate::ui::theme::DialogAction::Cancel
                {
                    close = true;
                }
            });
            if close {
                self.ai.object_error_dialog = None;
            }
        }
    }
}
