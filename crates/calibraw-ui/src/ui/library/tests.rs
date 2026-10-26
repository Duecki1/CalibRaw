use super::*;
use std::path::PathBuf;

fn test_asset(path: impl Into<PathBuf>) -> LibraryAsset {
    LibraryAsset::from_desktop_path(path.into(), 42, 123, Some([6000, 4000]))
}

#[test]
fn thumbnail_background_progress_is_generation_scoped_and_deduplicated() {
    let first = test_asset("one.CR3").id;
    let second = test_asset("two.NEF").id;
    let mut progress = ThumbnailBackgroundProgress::default();
    progress.begin(12, 2);
    progress.record_completion(11, first.clone());
    progress.record_completion(12, first.clone());
    progress.record_completion(12, first);

    assert_eq!(
        progress.snapshot(false),
        Some(ThumbnailProgress {
            completed: 1,
            total: 2,
            paused: false,
        })
    );

    progress.record_completion(12, second);
    assert_eq!(progress.snapshot(false), None);
}
