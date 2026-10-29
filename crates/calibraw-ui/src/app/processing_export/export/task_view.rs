//! The system notification for a running export or merge, where tasks need the
//! app in the foreground. The toolbar indicator is in
//! `app/background_tasks.rs`.

use super::*;
use crate::i18n::{tr, tr_fmt};

impl CalibRawApp {
    pub(crate) fn sync_android_export_notification(&self) {
        let Some(task) = self.export.task.as_ref() else {
            let result = match self.library.merge_task() {
                Some(merge) => {
                    self.platform
                        .update_task_notification(crate::platform::TaskNotification {
                            title: &tr_fmt("CalibRaw {title}", &[("title", &merge.title)]),
                            phase: &merge.detail,
                            detail: None,
                            progress_percent: 0,
                            indeterminate: true,
                            queued_count: 0,
                        })
                }
                None => self.platform.clear_task_notification(),
            };
            if let Err(error) = result {
                log::warn!("{error}");
            }
            return;
        };
        let title = if task.kind == ExportTaskKind::LibraryBatch {
            tr("CalibRaw batch export")
        } else {
            tr("CalibRaw export")
        };
        let detail = (task.total > 1).then(|| {
            tr_fmt(
                "{completed} / {total} images complete",
                &[
                    ("completed", &task.completed.min(task.total)),
                    ("total", &task.total),
                ],
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
