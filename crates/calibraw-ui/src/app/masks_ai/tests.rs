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

#[test]
fn mask_thumbnails_refresh_after_drag_release() {
    let context = egui::Context::default();
    crate::ui::theme::install(&context);
    let mut app = CalibRawApp::empty(&context);
    app.masks.stack.add_mask(MaskKind::Brush);
    let frame = eframe::Frame::_new_kittest();
    let draw = |app: &mut CalibRawApp, events| {
        let _ = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| crate::ui::sidebar::Sidebar::show_horizontal_mask_strip(ui, app, &frame),
        );
    };
    draw(&mut app, vec![]);
    assert_eq!(app.masks.thumbnail_group_textures.len(), 1);
    assert_eq!(app.masks.thumbnail_component_textures.len(), 1);
    let cached_revision = app.masks.thumbnail_revision;

    app.note_mask_geometry_interaction(0);
    assert_ne!(app.masks.overlay_revision, cached_revision);
    let pointer_event = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(900.0, 500.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    draw(&mut app, vec![pointer_event(true)]);
    assert_eq!(app.masks.thumbnail_revision, cached_revision);

    // The sidebar can run before the preview finishes the interaction. Releasing
    // the pointer must refresh the cards even while the interaction is recorded.
    draw(&mut app, vec![pointer_event(false)]);
    assert_eq!(app.masks.thumbnail_revision, app.masks.overlay_revision);
}

#[test]
fn stationary_mask_drag_flushes_latest_value_at_the_throttle_deadline() {
    use crate::pipeline::MaskGeometry;
    use std::time::{Duration, Instant};

    let context = egui::Context::default();
    let mut app = CalibRawApp::empty(&context);
    app.masks.stack.add_mask(MaskKind::Path);
    app.note_mask_geometry_interaction(0);
    let previous = app.preview_mask_stack();
    let revision = app.masks.overlay_revision;
    app.masks.dirty_layers.fill(false);
    app.masks.interaction_last_upload = Some(Instant::now());
    if let MaskGeometry::Path { grow, .. } = &mut app.masks.stack.masks[0].components[0].geometry {
        *grow = 0.4;
    }
    app.note_mask_geometry_interaction(0);
    assert!(app.masks.interaction_has_uncommitted_change);
    assert_eq!(app.masks.overlay_revision, revision);
    assert!(std::sync::Arc::ptr_eq(&app.preview_mask_stack(), &previous));

    app.masks.interaction_last_upload = Some(Instant::now() - Duration::from_millis(46));
    app.flush_mask_geometry_interaction();
    assert!(!app.masks.interaction_has_uncommitted_change);
    assert!(app.masks.dirty_layers[0]);
    assert_ne!(app.masks.overlay_revision, revision);
    assert!(matches!(
        app.preview_mask_stack().masks[0].components[0].geometry,
        MaskGeometry::Path { grow: 0.4, .. }
    ));
    let revision = app.masks.overlay_revision;
    app.flush_mask_geometry_interaction();
    assert_eq!(app.masks.overlay_revision, revision);
}

#[test]
fn stationary_subject_refinement_drag_refreshes_all_shared_layers() {
    use std::time::{Duration, Instant};

    let context = egui::Context::default();
    let mut app = CalibRawApp::empty(&context);
    app.masks.stack.add_mask(MaskKind::Subject);
    app.masks.stack.add_mask(MaskKind::Background);
    app.note_subject_refinement_interaction();
    app.masks.dirty_layers.fill(false);
    app.masks.interaction_last_upload = Some(Instant::now());
    app.note_subject_refinement_interaction();
    assert!(app.masks.interaction_has_uncommitted_change);

    app.masks.interaction_last_upload = Some(Instant::now() - Duration::from_millis(46));
    app.flush_mask_geometry_interaction();
    assert!(!app.masks.interaction_has_uncommitted_change);
    assert!(app.masks.dirty_layers.iter().all(|dirty| *dirty));
    assert!(app.masks.detail_dirty_layers.iter().all(|dirty| *dirty));
    assert!(app.masks.navigation_dirty_layers.iter().all(|dirty| *dirty));
}
