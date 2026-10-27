//! Library grid keys: moving the active photo, selecting, opening, rating
//! and the photo actions. While browsing they work on the active photo,
//! while selecting on the selection.

use super::review::{take_review_key, ReviewKey};
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum GridKey {
    /// Moves the active photo; `extend` selects the photos on the way.
    Move {
        step: GridStep,
        extend: bool,
    },
    ToggleActive,
    Open,
    StopSelecting,
    SelectAll,
    Review(ReviewKey),
    Delete,
    CopyAdjustments,
    PasteAdjustments,
    Export,
}

const MOVES: [(egui::Key, GridStep); 8] = [
    (egui::Key::ArrowLeft, GridStep::Previous),
    (egui::Key::ArrowRight, GridStep::Next),
    (egui::Key::ArrowUp, GridStep::Up),
    (egui::Key::ArrowDown, GridStep::Down),
    (egui::Key::PageUp, GridStep::PageUp),
    (egui::Key::PageDown, GridStep::PageDown),
    (egui::Key::Home, GridStep::First),
    (egui::Key::End, GridStep::Last),
];

/// Takes this frame's first grid key press.
pub(super) fn take_grid_key(ctx: &egui::Context) -> Option<GridKey> {
    use egui::{Key, Modifiers};
    let command_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    // `consume_key` ignores an extra Shift, so every Shift form comes first.
    let shortcuts = [
        (command_shift, Key::C, GridKey::CopyAdjustments),
        (command_shift, Key::V, GridKey::PasteAdjustments),
        (command_shift, Key::E, GridKey::Export),
        (Modifiers::COMMAND, Key::A, GridKey::SelectAll),
    ]
    .into_iter()
    .chain(MOVES.into_iter().flat_map(|(key, step)| {
        [
            (Modifiers::SHIFT, key, GridKey::Move { step, extend: true }),
            (
                Modifiers::NONE,
                key,
                GridKey::Move {
                    step,
                    extend: false,
                },
            ),
        ]
    }))
    .chain([
        (Modifiers::NONE, Key::Space, GridKey::ToggleActive),
        (Modifiers::NONE, Key::Enter, GridKey::Open),
        (Modifiers::NONE, Key::Escape, GridKey::StopSelecting),
        (Modifiers::NONE, Key::Delete, GridKey::Delete),
        (Modifiers::NONE, Key::Backspace, GridKey::Delete),
    ]);
    let pressed = ctx.input_mut(|input| {
        for (modifiers, key, command) in shortcuts {
            if input.consume_key(modifiers, key) {
                return Some(command);
            }
        }
        None
    });
    pressed.or_else(|| take_review_key(ctx).map(GridKey::Review))
}

/// Carries out a grid key. `visible` and `placements` are the grid as
/// shown; `page_height` is how far Page Up and Page Down move.
pub(super) fn apply_grid_key(
    app: &mut CalibRawApp,
    key: GridKey,
    visible: &[usize],
    placements: &[egui::Rect],
    page_height: f32,
    open_asset: &mut Option<LibraryAsset>,
    library_action: &mut Option<LibraryAction>,
) {
    match key {
        GridKey::Move { step, extend } => {
            let from = app.library.active_position(visible);
            if let Some(target) = grid_step_target(placements, from, step, page_height) {
                app.library.activate(visible, target, extend);
            }
        }
        GridKey::ToggleActive => {
            if app.library.is_selecting() {
                if let Some(asset_id) = app.library.active_asset_id().cloned() {
                    app.library.toggle_selected(&asset_id);
                }
            }
        }
        GridKey::Open => *open_asset = app.library.active_asset(visible),
        GridKey::StopSelecting => app.library.clear_selection(),
        GridKey::SelectAll => app.library.select_all(),
        GridKey::Review(ReviewKey { change, advance }) => {
            if !library_actions_enabled(app) {
                return;
            }
            // Neighbours are found before the change, which can re-sort
            // the photos or hide the active one behind a filter.
            let position = app
                .library
                .active_position(visible)
                .filter(|_| !app.library.is_selecting());
            let neighbour = |offset: isize| {
                let index = visible.get(position?.checked_add_signed(offset)?)?;
                Some(app.library.entries[*index].asset.id.clone())
            };
            let (next, previous) = (neighbour(1), neighbour(-1));
            app.review_photos_from_keyboard(app.library.target_assets(visible), change);
            let hidden = position.is_some()
                && app
                    .library
                    .active_position(&app.library.visible_entry_indices())
                    .is_none();
            // Shift moves on; a photo the filters now hide hands over to
            // its neighbour.
            if advance || hidden {
                if let Some(asset_id) = next.or(previous.filter(|_| hidden)) {
                    app.library.activate_id(asset_id);
                }
            }
        }
        GridKey::CopyAdjustments
        | GridKey::PasteAdjustments
        | GridKey::Delete
        | GridKey::Export => {
            // Like the bar and menus, no photo action starts while another runs.
            if library_actions_enabled(app) {
                *library_action = photo_action(app, key, visible);
            }
        }
    }
}

fn photo_action(app: &CalibRawApp, key: GridKey, visible: &[usize]) -> Option<LibraryAction> {
    if let GridKey::CopyAdjustments = key {
        return app
            .library
            .active_asset(visible)
            .map(LibraryAction::CopyAdjustments);
    }
    let targets = app.library.target_assets(visible);
    if targets.is_empty() {
        return None;
    }
    match key {
        GridKey::PasteAdjustments => app
            .library
            .has_copied_adjustments()
            .then_some(LibraryAction::PasteAdjustments(targets)),
        GridKey::Delete => Some(LibraryAction::Delete(targets)),
        GridKey::Export => Some(LibraryAction::Export(targets)),
        _ => None,
    }
}
