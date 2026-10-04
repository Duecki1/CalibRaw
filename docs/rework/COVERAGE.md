# Review coverage and completion record

One row per area (plan §15). "Reviewed" means responsibilities, public surface,
coupling and duplication were examined and the outcome recorded, including
"no change needed".

| Area | Reviewed responsibilities | Refactor candidates | Accepted exceptions | Verification |
|---|---|---|---|---|
| `calibraw-core` | pending | | | |
| `calibraw-gpu` (Rust) | pending | | | |
| WGSL shaders | pending | | | |
| `calibraw-ai` | pending | | | |
| `calibraw-ui` | pending | | | |
| `calibraw-ffi` | pending | | | |
| `calibraw-cli` | pending | | | |
| Android Java | pending | | | |
| `moduwu-design` | pending | | | |
| `build.rs` scripts and `xtask` | pending | | | |
| Gradle and CMake | pending | | | |
| Scripts and packaging | pending | | | |
| CI and release workflows | pending | | | |

## Completion record

| Field | Evidence |
|---|---|
| Baseline/final revision | baseline CalibRaw `82e81057`, Moduwu `64acb1e` ([BASELINE.md](BASELINE.md)) |
| Behavior preserved | |
| Reuse achieved | |
| Duplication removed | |
| Compatibility | |
| Failure/recovery | |
| Accessibility | |
| Numerical/visual result | |
| Performance | |
| Dependency improvement | |
| Concurrency | |
| Remaining exceptions | |

## Pre-existing defects found and fixed

| Defect | Fix |
|---|---|
| CI clippy failed on `redundant_clone` in a `calibraw-ai` test (`remove/lama.rs`) | removed the clone |
| Java unit test `rawLibrarySelectionReadsModifiedTimeOnceAndPreservesStableCutoffTies` failed since JPEG became a library format; CI never ran Java tests | fixture uses an unsupported `.txt` name; Java tests now run in the Android workflow |
| UI review harness showed no photo in 99 of 132 captures: a cached egui texture ID from a previous renderer was reused | texture registration moved to the UI (`PreviewPipeline`); each registration belongs to one renderer |
| Android-only clippy findings (two redundant clones, an eight-argument constructor) were invisible to host CI | fixed; the library constructors share `LibraryPreferences` |
