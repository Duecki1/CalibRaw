//! The toolbar indicator for work that runs while the app stays usable, such
//! as exports, HDR merges and focus stacks: a chip with the first task's progress, and a
//! dropdown that lists every task with its progress and a cancel button.

use eframe::egui::{self, Ui};

/// A task the indicator can show and cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackgroundTaskId {
    /// An export, a library batch export or an Edit Replay.
    Export,
    /// An HDR merge or a focus stack, in a folder library.
    Merge,
}

/// What the indicator shows for one task.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BackgroundTaskView {
    pub(crate) id: BackgroundTaskId,
    /// Short, for the chip: "Exporting 3 / 12".
    pub(crate) title: String,
    /// The current step: "Encoding DSC01234.ARW".
    pub(crate) detail: String,
    /// Completed fraction in `0..=1`; `None` while it cannot be measured.
    pub(crate) progress: Option<f32>,
    pub(crate) cancelling: bool,
}

/// Shows the chip for `tasks`, nothing when there are none, and returns the
/// task whose cancel button was clicked.
pub(crate) fn show_indicator(
    ui: &mut Ui,
    tasks: &[BackgroundTaskView],
) -> Option<BackgroundTaskId> {
    let first = tasks.first()?;
    let mut label = first.title.clone();
    if let Some(progress) = first.progress {
        label.push_str(&format!(" · {:.0}%", progress.clamp(0.0, 1.0) * 100.0));
    }
    if tasks.len() > 1 {
        label.push_str(&format!(" · +{}", tasks.len() - 1));
    }
    // Right-to-left toolbars place the spinner before the chip.
    let chip = moduwu_design::secondary_button(ui, label);
    ui.spinner();
    let mut cancel = None;
    moduwu_design::dropdown_menu(&chip, |ui| {
        ui.set_min_width(280.0);
        for (index, task) in tasks.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            if task_row(ui, task) {
                cancel = Some(task.id);
            }
        }
    });
    cancel
}

/// One task in the dropdown. Returns whether its cancel button was clicked.
fn task_row(ui: &mut Ui, task: &BackgroundTaskView) -> bool {
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.strong(&task.title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            cancel = ui
                .add_enabled_ui(!task.cancelling, |ui| {
                    moduwu_design::ghost_icon_button(
                        ui,
                        egui_phosphor::regular::X,
                        moduwu_design::toolbar_icon_size(),
                        &format!("Cancel {}", task.title),
                    )
                })
                .inner
                .clicked();
        });
    });
    let detail = if task.cancelling {
        "Stopping…"
    } else {
        task.detail.as_str()
    };
    ui.label(egui::RichText::new(detail).color(ui.visuals().weak_text_color()));
    let bar = match task.progress {
        Some(progress) => egui::ProgressBar::new(progress.clamp(0.0, 1.0)).show_percentage(),
        None => egui::ProgressBar::new(0.0).animate(!task.cancelling),
    };
    ui.add(bar);
    cancel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(progress: Option<f32>) -> BackgroundTaskView {
        BackgroundTaskView {
            id: BackgroundTaskId::Export,
            title: "Exporting 3 / 12".to_owned(),
            detail: "Encoding DSC01234.ARW".to_owned(),
            progress,
            cancelling: false,
        }
    }

    fn painted_texts(ctx: &egui::Context, tasks: &[BackgroundTaskView]) -> Vec<String> {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, out)),
                _ => {}
            }
        }
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            show_indicator(ui, tasks);
        });
        let mut texts = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut texts);
        }
        texts
    }

    #[test]
    fn no_tasks_show_nothing() {
        let ctx = egui::Context::default();
        assert!(painted_texts(&ctx, &[]).is_empty());
    }

    #[test]
    fn the_chip_names_the_first_task_its_progress_and_the_others() {
        let ctx = egui::Context::default();
        let texts = painted_texts(&ctx, &[export(Some(0.42))]);
        assert!(
            texts.iter().any(|text| text == "Exporting 3 / 12 · 42%"),
            "{texts:?}"
        );

        let mut second = export(None);
        second.title = "HDR merge".to_owned();
        let texts = painted_texts(&ctx, &[export(None), second]);
        assert!(
            texts.iter().any(|text| text == "Exporting 3 / 12 · +1"),
            "{texts:?}"
        );
    }
}
