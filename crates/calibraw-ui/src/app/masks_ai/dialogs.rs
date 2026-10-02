use super::*;

impl CalibRawApp {
    fn show_generated_mask_dialog(&mut self, ctx: &egui::Context) {
        let (model, runtime_download_needed, model_download_needed) = match self.ai.consent {
            AiConsentState::Subject {
                runtime_download_needed,
            } => (
                AiMaskModel::Subject,
                runtime_download_needed,
                !self
                    .generated_model_is_verified(AiMaskModel::Subject, &self.birefnet_model_path()),
            ),
            AiConsentState::Sky {
                runtime_download_needed,
            } => (
                AiMaskModel::Sky,
                runtime_download_needed,
                !self.generated_model_is_verified(AiMaskModel::Sky, &self.skyseg_model_path()),
            ),
            AiConsentState::Depth {
                runtime_download_needed,
                model_download_needed,
            } => (
                AiMaskModel::Depth,
                runtime_download_needed,
                model_download_needed,
            ),
            _ => return,
        };
        let title = match (model_download_needed, runtime_download_needed) {
            (true, true) => format!("Download {} model and ONNX Runtime?", model.noun()),
            (true, false) => format!("Download {}-selection model?", model.noun()),
            (false, true) => "Download ONNX Runtime?".to_owned(),
            (false, false) => format!("Prepare {} selection?", model.noun()),
        };
        let (summary, purpose, artifact, details) = match model {
            AiMaskModel::Subject => {
                let checkpoint = self.ai.birefnet_quality.model();
                (
                    format!("BiRefNet (~{:.0} MB)", checkpoint.bytes as f64 / 1_000_000.0),
                    "create subject and background masks",
                    ("Subject model artifacts", "https://huggingface.co/Duecki/CalibRaw-Artifacts/tree/main/models/briefnet"),
                    format!("{} quality uses {} with a {} × {} input. License: MIT.",
                        self.ai.birefnet_quality.label(), checkpoint.checkpoint,
                        checkpoint.input_height, checkpoint.input_width),
                )
            }
            AiMaskModel::Sky => (
                "SkySeg U2Net (~176 MB)".to_owned(), "create sky masks",
                ("Sky model artifact", "https://huggingface.co/Duecki/CalibRaw-Artifacts/tree/main/models/skyseg"),
                "SkySeg U2Net runs locally on a 320 × 320 image and produces a sky probability mask. License: MIT.".to_owned(),
            ),
            AiMaskModel::Depth => {
                let depth_model = calibraw_ai::ai_masks::DEPTH_MODEL;
                (
                    format!("{} (~{:.0} MB)", depth_model.name, depth_model.download_bytes as f64 / 1_000_000.0),
                    "create depth masks and fog",
                    ("Depth model artifact", depth_model.artifact_url),
                    format!("{} runs locally on a {} × {} letterboxed image and produces relative depth. License: Apache-2.0.",
                        depth_model.name, depth_model.input_edge, depth_model.input_edge),
                )
            }
        };
        let runtime_ready = self.ai_runtime_ready();
        let mut action = crate::ui::theme::DialogAction::None;
        crate::ui::theme::dialog_window(title, ctx, crate::ui::theme::DIALOG_WIDTH_LARGE)
            .show_with_footer(
                ctx,
                |ui| {
                    Self::show_ai_download_summary(
                        ui,
                        &summary,
                        purpose,
                        model_download_needed,
                        runtime_download_needed,
                    );
                    self.show_ai_download_details(
                        ui,
                        match model {
                            AiMaskModel::Subject => "subject-download-details",
                            AiMaskModel::Sky => "sky-download-details",
                            AiMaskModel::Depth => "depth-download-details",
                        },
                        model_download_needed,
                        runtime_download_needed,
                        &[artifact],
                        |ui| {
                            ui.label(&details);
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
                self.start_generated_mask_worker(model, model_download_needed);
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

    pub(in crate::app) fn show_subject_dialogs(&mut self, ctx: &egui::Context) {
        self.show_generated_mask_dialog(ctx);

        if let AiConsentState::Object {
            runtime_download_needed,
        } = self.ai.consent
        {
            let (encoder, decoder) = self.sam21_model_paths();
            let model_download_needed =
                !calibraw_ai::ai_masks::object_models_are_verified(&encoder, &decoder);
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
                            &[("Object model artifacts", "https://huggingface.co/Duecki/CalibRaw-Artifacts/tree/main/models/sam2")],
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
