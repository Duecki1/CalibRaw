//! Work that runs while the app stays usable: exports, Edit Replays, HDR
//! merges and focus stacks. Views follow them through the toolbar indicator
//! or, where the platform requires the app in the foreground while they run,
//! through a blocking progress dialog; results arrive as a status notice or,
//! for failures, the shared error dialog.

use super::*;
use crate::i18n::{tr, tr_fmt};
use crate::ui::background_tasks::{BackgroundTaskId, BackgroundTaskView};

impl CalibRawApp {
    /// The running background tasks, export first.
    pub(crate) fn background_tasks(&self) -> Vec<BackgroundTaskView> {
        let mut tasks = Vec::new();
        if let Some(task) = self.export.task.as_ref() {
            tasks.push(export_task_view(task));
        }
        if let Some(task) = self.library.merge_task() {
            tasks.push(task);
        }
        tasks
    }

    /// The toolbar indicator. Shows nothing while no task runs, and nothing
    /// at all where the tasks hold the app in the foreground: there they
    /// show in [`Self::show_background_task_dialog`] instead.
    pub(crate) fn show_background_tasks(&mut self, ui: &mut egui::Ui) {
        if self.caps().tasks_need_foreground {
            return;
        }
        let tasks = self.background_tasks();
        if let Some(id) = crate::ui::background_tasks::show_indicator(ui, &tasks) {
            self.cancel_background_task(id);
        }
    }

    /// Where the platform requires the app in the foreground while exports
    /// and merges run, their progress shows as a blocking dialog; the input
    /// blocker of `android_foreground_task_active` covers the rest of the
    /// app. Returns without a task.
    pub(crate) fn show_background_task_dialog(&mut self, ctx: &egui::Context) {
        if !self.caps().tasks_need_foreground {
            return;
        }
        let Some(task) = self.background_tasks().into_iter().next() else {
            return;
        };
        let dialog_id = match task.id {
            BackgroundTaskId::Export => "background-task-progress-export",
            BackgroundTaskId::Merge => "background-task-progress-merge",
        };
        if super::foreground::show_processing_dialog(
            ctx,
            dialog_id,
            &task.title,
            &background_task_progress(&task),
            task.cancelling,
        ) {
            self.cancel_background_task(task.id);
        }
    }

    fn cancel_background_task(&mut self, id: BackgroundTaskId) {
        match id {
            BackgroundTaskId::Export => {
                self.cancel_export_task();
            }
            BackgroundTaskId::Merge => {
                self.library.cancel_merge();
                self.egui_ctx.request_repaint();
            }
        }
    }

    /// Reports a finished HDR merge or focus stack.
    pub(super) fn poll_merge(&mut self) {
        use crate::ui::library::MergeOutcome;
        let Some((kind, outcome)) = self.library.poll_merge(&self.egui_ctx) else {
            return;
        };
        match outcome {
            MergeOutcome::Saved(name) => {
                let message = tr_fmt(
                    "{kind} saved: {name}",
                    &[("kind", &kind.title()), ("name", &name)],
                );
                self.library.set_status(message.clone());
                self.ui.notice = Some(message);
            }
            MergeOutcome::Cancelled => {
                self.ui.notice = Some(tr_fmt("{kind} cancelled.", &[("kind", &kind.title())]));
            }
            MergeOutcome::Failed(error) => self.report_error(kind.error_kind(), error),
        }
        self.egui_ctx.request_repaint();
    }
}

fn export_task_view(task: &ExportTask) -> BackgroundTaskView {
    let title = match task.kind {
        ExportTaskKind::Replay => tr("Edit Replay").to_owned(),
        _ if task.total > 1 => tr_fmt(
            "Exporting {completed} / {total}",
            &[
                ("completed", &task.completed.min(task.total)),
                ("total", &task.total),
            ],
        ),
        _ => tr("Exporting").to_owned(),
    };
    // Before the first tile there is nothing to measure.
    let measurable = task.total_tiles > 0 || task.progress > 0.0;
    BackgroundTaskView {
        id: BackgroundTaskId::Export,
        title,
        detail: task.phase.clone(),
        progress: measurable.then_some(task.progress.clamp(0.0, 1.0)),
        cancelling: task.cancelling,
    }
}

/// The task's progress as the blocking foreground dialog draws it: a share of
/// the whole task with the running phase beside the bar, since images and
/// tiles mix while it advances.
fn background_task_progress(task: &BackgroundTaskView) -> ForegroundProgress {
    match task.progress {
        Some(fraction) => ForegroundProgress::fraction(fraction, task.detail.as_str())
            .with_detail(task.detail.as_str()),
        None => ForegroundProgress::indeterminate(task.detail.as_str()),
    }
}
