use super::*;

pub(super) fn apply_library_adjustment_paste(
    app: &mut CalibRawApp,
    assets: Vec<LibraryAsset>,
    mode: crate::sidecar::AdjustmentPasteMode,
    context: &egui::Context,
    frame: &eframe::Frame,
) {
    let total = assets.len();
    let outcome = app.paste_library_adjustments(&assets, mode, frame);
    finish_library_edit_transfer(app, "Pasted adjustments to", total, outcome, context);
}

/// Applies `preset_path` to `assets` and reports the result in the Library.
pub(super) fn apply_library_preset(
    app: &mut CalibRawApp,
    assets: Vec<LibraryAsset>,
    preset_path: &Path,
    context: &egui::Context,
    frame: &eframe::Frame,
) {
    let Some(preset) = app.preset_at(preset_path).cloned() else {
        app.library.status = "That preset no longer exists.".to_owned();
        return;
    };
    let total = assets.len();
    let outcome = app.apply_edit_transfer_to_library_assets(
        &assets,
        crate::app::EditTransfer::Preset(&preset),
        frame,
    );
    let verb = format!("Applied preset “{}” to", preset.name());
    finish_library_edit_transfer(app, &verb, total, outcome, context);
}

fn finish_library_edit_transfer(
    app: &mut CalibRawApp,
    verb: &str,
    total: usize,
    outcome: crate::app::LibraryEditTransferOutcome,
    context: &egui::Context,
) {
    let crate::app::LibraryEditTransferOutcome {
        completed,
        ai_refresh,
        failures,
    } = outcome;
    app.library.clear_selection();
    #[cfg(target_os = "android")]
    calibraw_ffi::set_back_navigation_active(false);
    app.library.refresh(context);
    app.library.status = if failures.is_empty() {
        format!(
            "{verb} {completed} selected {}",
            if completed == 1 { "image" } else { "images" }
        )
    } else {
        format!(
            "{verb} {completed} of {total} selected images. {}",
            failures.join(" · ")
        )
    };
    app.library.ai_mask_refresh_prompt =
        (!ai_refresh.is_empty()).then_some(LibraryAiMaskRefreshPrompt { assets: ai_refresh });
}

pub(super) fn start_library_ai_mask_refresh_for_assets(
    app: &mut CalibRawApp,
    assets: Vec<LibraryAsset>,
    frame: &eframe::Frame,
) {
    start_local_library_ai_mask_refresh(app, &assets, frame);
}
