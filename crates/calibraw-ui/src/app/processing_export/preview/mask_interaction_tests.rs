use super::*;

#[test]
fn a_moved_view_cancels_the_crop_being_prepared_and_waits_for_its_worker() {
    let mut app = CalibRawApp::empty(&egui::Context::default());
    let (_sender, receiver) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.preview.detail_rebuild = Some(DetailRebuild::new(receiver, Arc::clone(&cancel)));

    app.note_preview_motion();
    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    // The worker is kept until it reports, so the next crop is not prepared
    // beside it; its photo going away drops it at once.
    assert!(app.preview.detail_rebuild.is_some());
    app.preview.drop_detail_rebuild();
    assert!(app.preview.detail_rebuild.is_none());
}
