//! Work that runs while the app stays usable: exports, Edit Replays and HDR
//! merges. The toolbar indicator follows and cancels them; results arrive as
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
        #[cfg(not(target_os = "android"))]
        if let Some(task) = self.library.hdr_merge_task() {
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
            #[cfg(not(target_os = "android"))]
            BackgroundTaskId::HdrMerge => {
                self.library.cancel_hdr_merge();
                self.egui_ctx.request_repaint();
            }
        }
    }

    /// Reports a finished HDR merge.
    #[cfg(not(target_os = "android"))]
    pub(super) fn poll_hdr_merge(&mut self) {
        use crate::ui::library::HdrMergeOutcome;
        let Some(outcome) = self.library.poll_hdr_merge(&self.egui_ctx) else {
            return;
        };
        match outcome {
            HdrMergeOutcome::Saved(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let message = format!("HDR merge saved: {name}");
                self.library.set_status(message.clone());
                self.ui.notice = Some(message);
            }
            HdrMergeOutcome::Cancelled => {
                self.ui.notice = Some("HDR merge cancelled.".to_owned());
            }
            HdrMergeOutcome::Failed(error) => self.report_error(ErrorKind::HdrMerge, error),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn task(kind: ExportTaskKind, completed: usize, total: usize, progress: f32) -> ExportTask {
        ExportTask {
            kind,
            cancellation: Arc::new(AtomicBool::new(false)),
            receiver: None,
            destination: None,
            progress,
            phase: "Rendering tile 4/10".to_owned(),
            completed,
            total,
            completed_tiles: 0,
            total_tiles: 0,
            cancelling: false,
        }
    }

    #[test]
    fn export_tasks_are_named_by_kind_and_count() {
        let batch = export_task_view(&task(ExportTaskKind::LibraryBatch, 3, 12, 0.25));
        assert_eq!(batch.title, "Exporting 3 / 12");
        assert_eq!(batch.detail, "Rendering tile 4/10");
        let single = export_task_view(&task(ExportTaskKind::Single, 0, 1, 0.5));
        assert_eq!(single.title, "Exporting");
        let replay = export_task_view(&task(ExportTaskKind::Replay, 0, 1, 0.5));
        assert_eq!(replay.title, "Edit Replay");
    }

    #[test]
    fn export_progress_is_unmeasured_until_work_starts() {
        assert_eq!(
            export_task_view(&task(ExportTaskKind::Single, 0, 1, 0.0)).progress,
            None
        );
        let mut started = task(ExportTaskKind::Single, 0, 1, 0.0);
        started.total_tiles = 10;
        assert_eq!(export_task_view(&started).progress, Some(0.0));
        assert_eq!(
            export_task_view(&task(ExportTaskKind::Single, 0, 1, 1.4)).progress,
            Some(1.0)
        );
    }
}
