//! The Android notification for a running export. The toolbar indicator is in
//! `app/background_tasks.rs`.

use super::*;

impl CalibRawApp {
    #[cfg(target_os = "android")]
    pub(crate) fn sync_android_export_notification(&self) {
        let Some(task) = self.export.task.as_ref() else {
            if let Err(error) =
                calibraw_ffi::clear_background_task_notification(&self.android.android_app)
            {
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
        if let Err(error) = calibraw_ffi::update_background_task_notification(
            &self.android.android_app,
            title,
            &task.phase,
            detail.as_deref(),
            percent,
            task.total_tiles == 0 && task.progress <= 0.0,
            0,
        ) {
            log::warn!("{error}");
        }
    }
}
