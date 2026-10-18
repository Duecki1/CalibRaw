//! The system notification for a running export, where tasks need the app in
//! the foreground. The toolbar indicator is in
//! `app/background_tasks.rs`.

use super::*;

impl CalibRawApp {
    pub(crate) fn sync_android_export_notification(&self) {
        let Some(task) = self.export.task.as_ref() else {
            if let Err(error) = self.platform.clear_task_notification() {
                log::warn!("{error}");
            }
            return;
        };
        let title = if task.kind == ExportTaskKind::LibraryBatch {
            "CalibRaw batch export"
        } else {
            "CalibRaw export"
        };
        let detail = (task.total > 1).then(|| {
            format!(
                "{} / {} images complete",
                task.completed.min(task.total),
                task.total
            )
        });
        let percent = (task.progress.clamp(0.0, 1.0) * 100.0).round() as i32;
        if let Err(error) =
            self.platform
                .update_task_notification(crate::platform::TaskNotification {
                    title,
                    phase: &task.phase,
                    detail: detail.as_deref(),
                    progress_percent: percent,
                    indeterminate: task.total_tiles == 0 && task.progress <= 0.0,
                    queued_count: 0,
                })
        {
            log::warn!("{error}");
        }
    }
}
