use super::*;

impl CalibRawApp {
    pub(in crate::app) fn show_subject_dialogs(&mut self, ctx: &egui::Context) {
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
            crate::ui::theme::dialog_window(
                egui::Window::new(title),
                ctx,
                crate::ui::theme::DIALOG_WIDTH_LARGE,
            )
                .show(ctx, |ui| {
                    if model_download_needed {
                        ui.strong("Subject-selection model");
                        ui.label(format!(
                            "{} quality uses {} with its native {} x {} input tensor.",
                            self.ai.birefnet_quality.label(),
                            model.checkpoint,
                            model.input_height,
                            model.input_width
                        ));
                        ui.label(model.explanation);
                        ui.label(format!(
                            "Download: about {:.0} MB. Model license: MIT.",
                            model.bytes as f64 / 1_000_000.0
                        ));
                    }
                    self.show_ai_consent_runtime_details(
                        ui,
                        model_download_needed,
                        runtime_download_needed,
                    );
                    ui.label("Subject masks use BiRefNet's calibrated soft selection directly. Not Subject is the exact inverse of the subject alpha.");
                    ui.label("Inference is local. No photograph is uploaded.");
                    Self::show_hugging_face_privacy(
                        ui,
                        model_download_needed,
                        &[(
                            "MIT model license",
                            "https://github.com/ZhengPeng7/BiRefNet/blob/main/LICENSE",
                        )],
                    );
                    self.show_manual_runtime_warning(ui);
                    let runtime_ready = self.ai_runtime_ready();
                    match Self::show_ai_consent_buttons(
                        ui,
                        "Consent, download and continue",
                        runtime_ready,
                    ) {
                        crate::ui::theme::DialogAction::Confirm => {
                            self.ai.consent = AiConsentState::None;
                            self.start_subject_worker(
                                self.birefnet_model_path(),
                                model_download_needed,
                            );
                        }
                        crate::ui::theme::DialogAction::Cancel => {
                            self.ai.consent = AiConsentState::None;
                            if self.ai.mask_update_active {
                                self.cancel_ai_mask_update();
                            }
                        }
                        crate::ui::theme::DialogAction::None => {}
                    }
                });
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
            crate::ui::theme::dialog_window(
                egui::Window::new(title),
                ctx,
                crate::ui::theme::DIALOG_WIDTH_LARGE,
            )
                .show(ctx, |ui| {
                    ui.label("Object masks use SAM 2.1 Hiera Tiny with local edge-aware cleanup.");
                    if model_download_needed {
                        ui.strong("Object-selection model");
                        ui.label(format!(
                            "SAM 2.1 Hiera Tiny encoder and decoder: about {:.0} MB download. Model license: Apache-2.0.",
                            SAM21_MODEL_BYTES_ESTIMATE as f64 / 1_000_000.0
                        ));
                    }
                    self.show_ai_consent_runtime_details(
                        ui,
                        model_download_needed,
                        runtime_download_needed,
                    );
                    ui.label("Inference is local. No photograph or prompt stroke is uploaded.");
                    Self::show_hugging_face_privacy(
                        ui,
                        model_download_needed,
                        &[(
                            "Apache-2.0 model license",
                            "https://github.com/facebookresearch/sam2/blob/main/LICENSE",
                        )],
                    );
                    self.show_manual_runtime_warning(ui);
                    let runtime_ready = self.ai_runtime_ready();
                    match Self::show_ai_consent_buttons(
                        ui,
                        "Consent, download and continue",
                        runtime_ready,
                    ) {
                        crate::ui::theme::DialogAction::Confirm => {
                            self.ai.consent = AiConsentState::None;
                            if let Some((mask_index, component_index)) = self.ai.object_pending_target.take() {
                                let (encoder, decoder) = self.sam21_model_paths();
                                self.start_object_worker(mask_index, component_index, encoder, decoder, model_download_needed);
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
                });
        }

        if let Some(message) = self.ai.object_error_dialog.clone() {
            let mut close = false;
            crate::ui::theme::dialog_window(
                egui::Window::new("AI mask failed"),
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
