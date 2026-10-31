use super::{export_to_destinations, ExportTarget};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn cancelled_export_removes_temporary_output_before_publication() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "calibraw-export-cancel-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let destinations = [directory.join("photo-1.png"), directory.join("photo-2.png")];
    let cancellation = AtomicBool::new(false);

    let targets = destinations.clone().map(ExportTarget::File);
    let result = export_to_destinations(&targets, &cancellation, |outputs| {
        for output in outputs {
            std::fs::write(output.path, b"complete but not published")?;
        }
        cancellation.store(true, Ordering::Release);
        Ok(())
    });

    assert!(result.is_err());
    assert!(destinations.iter().all(|destination| !destination.exists()));
    assert!(std::fs::read_dir(&directory).unwrap().next().is_none());
    std::fs::remove_dir_all(directory).unwrap();
}
