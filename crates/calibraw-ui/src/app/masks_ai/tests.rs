use super::ai_mask_source_proxy_edge;
use crate::{
    app::CalibRawApp,
    pipeline::{LoadedRaw, MaskKind},
};
use eframe::egui;

#[test]
fn canonical_source_keeps_4k_for_three_by_two_but_caps_square_pixel_count() {
    assert_eq!(ai_mask_source_proxy_edge(6000, 4000), 4096);
    assert_eq!(ai_mask_source_proxy_edge(6000, 6000), 3464);
    assert_eq!(ai_mask_source_proxy_edge(3000, 3000), 3000);
}

#[test]
fn resetting_masks_clears_transient_ui_state_and_remains_undoable() {
    let context = egui::Context::default();
    crate::ui::theme::install(&context);
    let mut app = CalibRawApp::empty(&context);
    app.develop.loaded_raw = Some(std::sync::Arc::new(
        LoadedRaw::from_scene_linear_rec2020(8, 8, vec![0.2; 8 * 8 * 3]).unwrap(),
    ));
    app.masks.stack.add_mask(MaskKind::Brush);
    app.reset_edit_history();

    app.masks.stack.add_mask(MaskKind::Brush);
    app.masks.active_tool = Some(MaskKind::Brush);
    app.masks.dirty_layers[0] = true;
    app.mark_mask_adjustments_dirty();
    app.commit_edit_history_now();

    app.reset_masks();
    assert!(app.masks.stack.masks.is_empty());
    assert!(app.masks.active_tool.is_none());
    assert!(app.masks.dirty_layers.iter().all(|dirty| *dirty));
    app.commit_edit_history_now();
    assert!(app.can_undo_edit());

    app.undo_edit();
    assert_eq!(app.masks.stack.masks.len(), 2);
}
