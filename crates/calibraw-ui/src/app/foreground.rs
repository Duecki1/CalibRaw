use super::*;
use std::sync::atomic::Ordering;

fn try_install_foreground_operation(
    slot: &mut Option<ForegroundOperation>,
    mut operation: ForegroundOperation,
) -> bool {
    if slot.is_some() {
        operation.cancel();
        return false;
    }
    *slot = Some(operation);
    true
}

impl ForegroundOperation {
    fn cancel(&mut self) {
        self.cancellation.store(true, Ordering::Release);
        self.cancelling = true;
        self.progress = ForegroundProgress::indeterminate("Cancelling…");
    }

    pub(in crate::app) fn is_cancelled(&self) -> bool {
        self.cancelling || self.cancellation.load(Ordering::Acquire)
    }

    pub(in crate::app) fn accepts_result(&self, document_id: u64) -> bool {
        self.document_id == document_id && !self.is_cancelled()
    }
}

impl ForegroundOperationKind {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::SubjectMask => "Preparing subject mask",
            Self::SkyMask => "Preparing sky mask",
            Self::ObjectMask => "Preparing object mask",
            Self::AiDenoise => "Applying AI denoise",
            Self::LensCorrection => "Applying lens correction",
        }
    }
}

pub(super) fn show_processing_dialog(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    progress: &ForegroundProgress,
    cancelling: bool,
) -> bool {
    let mut cancel = false;
    crate::ui::theme::dialog_window(title, ctx, crate::ui::theme::DIALOG_WIDTH_FORM)
        .id(egui::Id::new(id))
        .movable(false)
        .show(ctx, |ui| {
            let measured =
                matches!(progress.value, ForegroundProgressValue::Units { total, .. } if total > 0);
            ui.horizontal(|ui| {
                if !measured {
                    ui.add(egui::Spinner::new().size(20.0));
                    ui.add_space(crate::ui::theme::SPACE_XS);
                }
                ui.add(egui::Label::new(egui::RichText::new(&progress.phase).size(14.0)).wrap());
            });

            if let ForegroundProgressValue::Units {
                completed,
                total,
                ref unit,
            } = progress.value
            {
                if total > 0 {
                    let fraction = (completed as f32 / total as f32).clamp(0.0, 1.0);
                    ui.add_space(crate::ui::theme::SPACE_MD);
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_height(10.0)
                            .fill(ui.visuals().selection.bg_fill),
                    );
                    ui.horizontal(|ui| {
                        let units = unit.as_deref().map_or_else(
                            || format!("{completed} / {total}"),
                            |unit| format!("{completed} / {total} {unit}"),
                        );
                        ui.label(
                            egui::RichText::new(progress.detail.as_deref().unwrap_or(&units))
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", fraction * 100.0))
                                    .small()
                                    .strong(),
                            );
                        });
                    });
                } else if let Some(detail) = &progress.detail {
                    ui.label(
                        egui::RichText::new(detail)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
            } else if let Some(detail) = &progress.detail {
                ui.label(
                    egui::RichText::new(detail)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }

            if cancelling {
                ui.label(
                    egui::RichText::new("Stopping at the next safe point…")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.add_space(crate::ui::theme::SPACE_SM);
            ui.separator();
            ui.add_space(crate::ui::theme::SPACE_XS);
            ui.allocate_ui_with_layout(
                egui::vec2(
                    ui.available_width().max(1.0),
                    crate::ui::theme::CONTROL_HEIGHT,
                ),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    cancel = ui
                        .add_enabled_ui(!cancelling, |ui| {
                            crate::ui::theme::secondary_button(ui, "Cancel")
                        })
                        .inner
                        .clicked();
                },
            );
            if !cancel
                && !cancelling
                && crate::ui::theme::dialog_keyboard_action(
                    ui,
                    crate::ui::theme::DialogKeyboard::CLOSE_ONLY,
                    false,
                ) == crate::ui::theme::DialogAction::Cancel
            {
                cancel = true;
            }
        });
    ctx.request_repaint_after(Duration::from_millis(50));
    cancel
}

impl CalibRawApp {
    pub(crate) fn foreground_operation_kind(&self) -> Option<ForegroundOperationKind> {
        self.foreground_operation
            .as_ref()
            .map(|operation| operation.kind)
    }

    pub(crate) fn foreground_operation_is(&self, kind: ForegroundOperationKind) -> bool {
        self.foreground_operation_kind() == Some(kind)
    }

    pub(crate) fn foreground_operation_active(&self) -> bool {
        self.foreground_operation.is_some()
    }

    pub(crate) fn begin_foreground_operation(&mut self, operation: ForegroundOperation) -> bool {
        let started = try_install_foreground_operation(&mut self.foreground_operation, operation);
        if started {
            self.egui_ctx.request_repaint();
        }
        started
    }

    pub(crate) fn cancel_foreground_operation(&mut self) -> bool {
        let Some(operation) = self.foreground_operation.as_mut() else {
            return false;
        };
        operation.cancel();
        self.egui_ctx.request_repaint();
        true
    }

    pub(crate) fn cancel_foreground_operation_if(&mut self, kind: ForegroundOperationKind) -> bool {
        if self.foreground_operation_is(kind) {
            self.cancel_foreground_operation()
        } else {
            false
        }
    }

    pub(crate) fn show_foreground_operation_dialog(&mut self, ctx: &egui::Context) {
        if self.ai.library_mask_refresh.is_some() {
            return;
        }
        let Some(operation) = self.foreground_operation.as_ref() else {
            return;
        };
        let kind = operation.kind;
        let progress = operation.progress.clone();
        let cancelling = operation.cancelling;
        if show_processing_dialog(
            ctx,
            "foreground-operation-progress",
            kind.title(),
            &progress,
            cancelling,
        ) {
            self.cancel_foreground_operation();
        }
    }
}

impl CalibRawApp {
    pub(in crate::app) fn poll_foreground_operation(&mut self, frame: &eframe::Frame) {
        match self.foreground_operation_kind() {
            Some(ForegroundOperationKind::SubjectMask | ForegroundOperationKind::SkyMask) => {
                self.poll_subject_worker()
            }
            Some(ForegroundOperationKind::ObjectMask) => self.poll_object_worker(),
            Some(ForegroundOperationKind::AiDenoise) => self.poll_ai_denoise_worker(),
            Some(ForegroundOperationKind::LensCorrection) => {
                self.poll_lens_correction_worker(frame)
            }
            None => {}
        }
    }

    pub(super) fn cancel_document_bound_foreground_operation(&mut self) {
        self.cancel_foreground_operation();
    }

    #[cfg(target_os = "android")]
    pub(crate) fn android_foreground_task_active(&self) -> bool {
        self.foreground_operation_active() || self.ai.library_mask_refresh.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_operation(document_id: u64) -> ForegroundOperation {
        let (_sender, receiver) = mpsc::channel::<SubjectMaskEvent>();
        ForegroundOperation {
            kind: ForegroundOperationKind::SubjectMask,
            document_id,
            cancellation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            progress: ForegroundProgress::indeterminate("Testing…"),
            cancelling: false,
            receiver: ForegroundOperationReceiver::Subject(receiver),
            context: ForegroundOperationContext::Subject,
        }
    }

    #[test]
    fn only_one_foreground_operation_can_occupy_the_slot() {
        let mut slot = None;
        assert!(try_install_foreground_operation(
            &mut slot,
            test_operation(7)
        ));
        assert!(!try_install_foreground_operation(
            &mut slot,
            test_operation(7)
        ));
        assert_eq!(
            slot.as_ref().map(|operation| operation.kind),
            Some(ForegroundOperationKind::SubjectMask)
        );
    }

    #[test]
    fn foreground_completion_releases_the_slot() {
        let mut slot = Some(test_operation(7));
        let completed = slot.take();
        assert!(completed.is_some());
        assert!(slot.is_none());
    }

    #[test]
    fn foreground_cancellation_rejects_late_results() {
        let mut operation = test_operation(7);
        operation.cancel();
        assert!(operation.is_cancelled());
        assert!(!operation.accepts_result(7));
    }

    #[test]
    fn stale_foreground_results_are_rejected_after_document_change() {
        let operation = test_operation(7);
        assert!(operation.accepts_result(7));
        assert!(!operation.accepts_result(8));
    }
}
