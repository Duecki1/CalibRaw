//! Work that runs while the app stays usable: exports, Edit Replays, HDR
//! merges and focus stacks. The toolbar indicator follows and cancels them; results arrive as
//! a status notice or, for failures, the shared error dialog.

use super::*;
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

    /// The toolbar indicator. Shows nothing while no task runs.
    pub(crate) fn show_background_tasks(&mut self, ui: &mut egui::Ui) {
        let tasks = self.background_tasks();
        if let Some(id) = crate::ui::background_tasks::show_indicator(ui, &tasks) {
            self.cancel_background_task(id);
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
                let message = format!("{} saved: {name}", kind.title());
                self.library.set_status(message.clone());
                self.ui.notice = Some(message);
            }
            MergeOutcome::Cancelled => {
                self.ui.notice = Some(format!("{} cancelled.", kind.title()));
            }
            MergeOutcome::Failed(error) => self.report_error(kind.error_kind(), error),
        }
        self.egui_ctx.request_repaint();
    }
}

fn export_task_view(task: &ExportTask) -> BackgroundTaskView {
    let title = match task.kind {
        ExportTaskKind::Replay => "Edit Replay".to_owned(),
        _ if task.total > 1 => format!(
            "Exporting {} / {}",
            task.completed.min(task.total),
            task.total
        ),
        _ => "Exporting".to_owned(),
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
