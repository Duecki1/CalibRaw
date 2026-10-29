use super::super::{
    ExportDestination, ExportItemEvent, ExportTask, ExportTaskKind, ExportTaskReceiver,
};
use super::batch::batch_export_overall_fraction;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

#[test]
fn stale_tile_progress_is_ignored_without_a_current_image() {
    let progress = batch_export_overall_fraction(1, 3, false, Some((10, 10)));
    assert!((progress - (1.0 / 3.0)).abs() < f32::EPSILON);
}

fn test_export_task() -> ExportTask {
    let (_sender, receiver) = mpsc::channel::<ExportItemEvent>();
    ExportTask {
        kind: ExportTaskKind::Single,
        cancellation: Arc::new(AtomicBool::new(false)),
        receiver: Some(ExportTaskReceiver::Tiled(receiver)),
        destination: Some(ExportDestination::File("photo.png".into())),
        progress: 0.42,
        phase: "Rendering tile 4/10".to_owned(),
        completed: 0,
        total: 1,
        completed_tiles: 4,
        total_tiles: 10,
        cancelling: false,
    }
}

#[test]
fn export_cancellation_sets_the_shared_token_and_state() {
    let mut task = test_export_task();
    task.request_cancel();
    assert!(task.cancelling);
    assert!(task.cancellation.load(Ordering::Acquire));
    assert_eq!(task.phase, "Cancelling export…");
}
