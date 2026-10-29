use super::*;
use crate::i18n::{tr, tr_fmt};

pub(super) fn apply_library_adjustment_paste(
    app: &mut CalibRawApp,
    assets: Vec<LibraryAsset>,
    mode: crate::sidecar::AdjustmentPasteMode,
    context: &egui::Context,
    frame: &eframe::Frame,
) {
    let total = assets.len();
    let outcome = app.paste_library_adjustments(&assets, mode, frame);
    finish_library_edit_transfer(
        app,
        LibraryEditVerb::PastedAdjustments,
        total,
        outcome,
        context,
    );
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
        app.library.status = tr("That preset no longer exists.").to_owned();
        return;
    };
    let total = assets.len();
    let outcome = app.apply_edit_transfer_to_library_assets(
        &assets,
        crate::app::EditTransfer::Preset(&preset),
        frame,
    );
    let verb = LibraryEditVerb::AppliedPreset(preset.name().to_string());
    finish_library_edit_transfer(app, verb, total, outcome, context);
}

/// Develops `assets` as film negatives, estimated when each is next decoded,
/// or as they are when `film` is positive.
pub(super) fn apply_library_film(
    app: &mut CalibRawApp,
    assets: Vec<LibraryAsset>,
    film: crate::pipeline::FilmSource,
    context: &egui::Context,
    frame: &eframe::Frame,
) {
    let total = assets.len();
    let outcome = app.apply_edit_transfer_to_library_assets(
        &assets,
        crate::app::EditTransfer::Film(film),
        frame,
    );
    let verb = if film.is_positive() {
        LibraryEditVerb::RemovedFilmNegative
    } else {
        LibraryEditVerb::AppliedFilmNegative
    };
    finish_library_edit_transfer(app, verb, total, outcome, context);
}

/// The action a Library edit transfer reports, kept whole so every status
/// sentence can be translated as one piece.
enum LibraryEditVerb {
    PastedAdjustments,
    AppliedPreset(String),
    RemovedFilmNegative,
    AppliedFilmNegative,
}

/// The Library status after an edit transfer; `failures` are listed after the
/// counts when any photo could not be changed.
fn edit_transfer_status(
    verb: &LibraryEditVerb,
    completed: usize,
    total: usize,
    failures: &[String],
) -> String {
    if !failures.is_empty() {
        let details = failures.join(" · ");
        return match verb {
            LibraryEditVerb::PastedAdjustments => tr_fmt(
                "Pasted adjustments to {completed} of {total} selected images. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("details", &details),
                ],
            ),
            LibraryEditVerb::AppliedPreset(preset) => tr_fmt(
                "Applied preset “{preset}” to {completed} of {total} selected images. {details}",
                &[
                    ("preset", preset),
                    ("completed", &completed),
                    ("total", &total),
                    ("details", &details),
                ],
            ),
            LibraryEditVerb::RemovedFilmNegative => tr_fmt(
                "Removed Film negative from {completed} of {total} selected images. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("details", &details),
                ],
            ),
            LibraryEditVerb::AppliedFilmNegative => tr_fmt(
                "Applied Film negative to {completed} of {total} selected images. {details}",
                &[
                    ("completed", &completed),
                    ("total", &total),
                    ("details", &details),
                ],
            ),
        };
    }
    let one = completed == 1;
    match (verb, one) {
        (LibraryEditVerb::PastedAdjustments, true) => tr_fmt(
            "Pasted adjustments to {completed} selected image",
            &[("completed", &completed)],
        ),
        (LibraryEditVerb::PastedAdjustments, false) => tr_fmt(
            "Pasted adjustments to {completed} selected images",
            &[("completed", &completed)],
        ),
        (LibraryEditVerb::AppliedPreset(preset), true) => tr_fmt(
            "Applied preset “{preset}” to {completed} selected image",
            &[("preset", preset), ("completed", &completed)],
        ),
        (LibraryEditVerb::AppliedPreset(preset), false) => tr_fmt(
            "Applied preset “{preset}” to {completed} selected images",
            &[("preset", preset), ("completed", &completed)],
        ),
        (LibraryEditVerb::RemovedFilmNegative, true) => tr_fmt(
            "Removed Film negative from {completed} selected image",
            &[("completed", &completed)],
        ),
        (LibraryEditVerb::RemovedFilmNegative, false) => tr_fmt(
            "Removed Film negative from {completed} selected images",
            &[("completed", &completed)],
        ),
        (LibraryEditVerb::AppliedFilmNegative, true) => tr_fmt(
            "Applied Film negative to {completed} selected image",
            &[("completed", &completed)],
        ),
        (LibraryEditVerb::AppliedFilmNegative, false) => tr_fmt(
            "Applied Film negative to {completed} selected images",
            &[("completed", &completed)],
        ),
    }
}

fn finish_library_edit_transfer(
    app: &mut CalibRawApp,
    verb: LibraryEditVerb,
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
    app.library.refresh(context);
    app.library.status = edit_transfer_status(&verb, completed, total, &failures);
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
