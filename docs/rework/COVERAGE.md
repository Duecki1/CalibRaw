# Review coverage and completion record

One row per area (plan §15). "Reviewed" means responsibilities, public surface,
coupling and duplication were examined and the outcome recorded, including
"no change needed".

| Area | Reviewed responsibilities | Refactor candidates | Accepted exceptions | Verification |
|---|---|---|---|---|
| `calibraw-core` | models, colour, geometry, loading (LibRaw, Rawler, Lensfun), masks, remove, sidecars, processing proxies and tiles | done: split `libraw_loader` (thumbnail, DCP index, sensor, camera matrix, temperature/tint, CCT), `raw_loader` (pixel map, opposed chroma, white balance), `masks` (model, raster, layers, probability, brush, shapes, editing), `sidecar` (mask/remove assets, files, size limits, effect validation), `processing` (proxy, AI-denoised regions, tiles), `remove`, `geometry`, Lensfun backend moved out of an inline module and split | `validation.rs` keeps one 390-line `validate_edit_state` (a flat list of field checks); `effects/params.rs` (957 lines) is data tables | 414 tests incl. sidecar compatibility fixtures; export byte-identical; `raw_open_bench` fingerprint |
| `calibraw-gpu` (Rust) | pipeline graph, shader composition, uniforms, readback, export, headless devices | done: egui removed (texture registration in the UI), one `RawGpuPipeline::new(.., PipelineOptions)`, shader registry, `request_headless_device`, `ExportTarget`, `gpu.rs` split (params, mask/stage params, construction, mask layers, readback, remove scene), `builder.rs` split (surfaces, layouts, bind groups, passes), `export.rs` split (settings, worker, streaming, rows, crop, destination, encoders, TIFF, geometry output, resize) | `create_bind_group_layouts`/`create_bind_groups` stay single functions mirroring the WGSL binding order | 141 tests (+2 ignored); export golden; arch-check |
| WGSL shaders | composable modules, entry points, Rust/WGSL layout | done: one registry for all entry shaders (every shader validated at both qualities), `log_luminance` deduplicated into `Common`, remove composite moved to its own module | specialization markers stay explicit per entry | Naga validation, `layout_contract_tests`, and a GPU test that builds and runs the pipeline for Bayer and X-Trans at both qualities (wgpu checks every entry shader against the Rust bind group layouts); 0/132 screenshot diffs |
| `calibraw-ai` | model artifacts, ONNX runtime, SAM, LaMa, RawNIND | done: `object` split (prompts, SAM runs, candidates, cleanup), `ai_denoise` split (result cache, inference, tiling), `remove` split (retouch), LaMa grain synthesis split | model-specific normalization kept explicit per model (plan §9) | 69 tests (+9 ignored, need models) |
| `calibraw-ui` | app coordination, state, views, preview presentation, services | done: `PreviewPipeline`/`TextureRetirement`, `services::replay`, `LibraryPreferences`, NumberField/Slider migration, theme re-exports removed, `appearance.rs`, back-navigation consolidated, `document_generation`, `app.rs` state split into seven modules, splits of library state/thumbnails, sidebar navigation, preview transforms, mask tools, sidecar persistence, export task view; the mask tool, preview viewport, mask strip and mask properties views take narrow inputs and return actions applied by app handlers (`app/mask_tool.rs`, `preview_viewport.rs`, `mask_strip.rs`, `mask_properties.rs`), each split into smaller functions | settings cards, the frame phases (`eframe_impl`) and the library thumbnail grid are separate functions; the preview tool handlers called after the viewport (crop, inpaint, white-balance and point-colour pickers) and the mask rename/delete dialogs still take the app | 328 tests (+4 ignored); scripted state dumps of the four reworked views identical before and after; 132 GPU review captures at tolerance 0; Android clippy |
| `calibraw-ffi` | JNI exports and calls, storage, export, notifications, replay encoder | done: egui removed (`attach_ui`), `java_string`/`non_empty_path`, `android.rs` split (library, thumbnail cache, sidecars, export, notifications, callbacks) | — | Android clippy; `jni-contract` 55/55 |
| `calibraw-cli` | arguments and headless export | done: uses `request_headless_device` and `ExportTarget`; `pollster` dropped | — | export golden; `cargo tree` shows no egui |
| Android Java | activity, storage contract, picker store, profile import, export publishing, thumbnails, notifications, replay encoder | done: profile import and asset copy use `BoundedStreams.copy` (were hand-written loops); obsolete SDK check removed | lifecycle ownership unchanged and documented in ARCHITECTURE; three lint warnings about launcher-icon artwork remain | 21 JVM tests (4 new); `lintDebug` 0 errors, 3 warnings |
| `moduwu-design` | themes, metrics, controls, dialogs, responsive helpers | done: `NumberField`, `Slider` (with AccessKit slider semantics), runtime `Metrics` via `Theme::apply_with_metrics`, `dialog_text_field`, gallery example; version 2.0.0 (new `Metrics` field) | — | 58 tests + gallery render test; clippy |
| `build.rs` scripts and `xtask` | native library discovery, licenses, Android builds, diagnostics | done: GPU `build.rs` removed (`include_str!` tracks shaders), xtask split into modules, new `arch-check`, `jni-contract` (split into Rust and Java sides), `ui-lint`, `baseline-run/compare`, `loc` | — | 33 + 6 xtask tests |
| Gradle and CMake | Android build, native staging | reviewed: versions come from `[workspace.metadata]` | root and `android/` `settings.gradle` both keep `pluginManagement` (Gradle rejects an applied script there) | Java tests and lint with `-PcalibrawBuildRust=false` |
| Scripts and packaging | release builds, AppImage, Flatpak, Windows, licenses | reviewed, no change needed; `generate_licenses.sh` re-run (no notice change) | — | `cargo deny check`: advisories, bans, licenses, sources ok |
| CI and release workflows | build, test, Android, release | done: CI runs `arch-check`, `jni-contract`, `ui-lint`; the Android workflow runs JVM unit tests; every workflow and the release script read the Rust version from `rust-toolchain.toml` | workflow edits need a real run before relying on them | not yet run on GitHub |

## Completion record

| Field | Evidence |
|---|---|
| Baseline/final revision | baseline CalibRaw `82e81057`, Moduwu `64acb1e` ([BASELINE.md](BASELINE.md)); final: branch `refactor/rework` with Moduwu `a77b977` (published and pinned in `Cargo.lock`) |
| Behavior preserved | full test suites; 132 review captures identical at tolerance 0 after every UI step; persisted serde names covered by tests |
| Reuse achieved | headless device, pipeline options, export targets, replay service, NumberField and Slider shared across callers and with Moduwu |
| Duplication removed | 17 consolidations in [AUDIT.md](AUDIT.md); 101 new modules from responsibility splits (about 300 lines of module boundaries); 414 moved items made private where nothing outside their new module uses them |
| Compatibility | sidecar fixtures and settings serde names unchanged; JNI contract 55/55 |
| Failure/recovery | stale-result checks unified (`is_for_document`); worker disconnection and terminal events tested |
| Accessibility | NumberField exposes a spin button; the slider track now exposes AccessKit slider role, value, range, step and Increment/Decrement/SetValue (tested); desktop screen-reader review not yet done |
| Numerical/visual result | export sha256 `ed2103ff…` byte-identical; `raw_open_bench` fingerprint unchanged; 0/132 screenshot differences |
| Performance | within budget, see BASELINE final measurements (open 1.38–1.40 s, export 5.17–5.20 s, same peak memory) |
| Dependency improvement | GPU, CLI and FFI free of egui/egui-wgpu; GPU free of FFI; enforced by `arch-check` |
| Concurrency | thread affinity, job table, synchronization and lock order in ARCHITECTURE.md |
| Remaining exceptions | CI edits unvalidated on GitHub; views outside the four reworked ones still take `&mut CalibRawApp` (129 functions in 36 files under `ui/`, most in `library/`, `presets.rs`, `settings.rs`, `develop.rs`, the preview tools and the mask dialogs) and nothing enforces the action rule; interactive open, desktop batch export and library thumbnails each prepare a document from its sidecar and differ in `"."` camera-profile handling, lens selection and AI-denoise restoration (a behavior change, left for its own PR); Linux screen-reader review |

Production code (`cargo xtask loc`, comments and blanks excluded): CalibRaw
product crates 99,808 → 99,504 lines (100,533 at `b610dc12` after the action-returning views → 99,693 after audit rows 20–27; WGSL 6,721 → 6,481) and xtask 1,303 → 4,427; Moduwu
1,475 → 2,353 (NumberField, Slider and runtime metrics moved or added there).

## Pre-existing defects found and fixed

| Defect | Fix |
|---|---|
| CI clippy failed on `redundant_clone` in a `calibraw-ai` test (`remove/lama.rs`) | removed the clone |
| Java unit test `rawLibrarySelectionReadsModifiedTimeOnceAndPreservesStableCutoffTies` failed since JPEG became a library format; CI never ran Java tests | fixture uses an unsupported `.txt` name; Java tests now run in the Android workflow |
| UI review harness showed no photo in 99 of 132 captures: a cached egui texture ID from a previous renderer was reused | texture registration moved to the UI (`PreviewPipeline`); each registration belongs to one renderer |
| Android-only clippy findings (two redundant clones, an eight-argument constructor) were invisible to host CI | fixed; the library constructors share `LibraryPreferences` |
| `activate_tab` published Android back-press routing without the selection or folder-sidebar state; 15 other call sites repeated the decision | one `handles_back_navigation()` published once per frame after all views |
| ARCHITECTURE.md stated every job channel is unbounded; the thumbnail pool uses bounded channels | documented the bounded channels and their limits |
| Android TIFF export was published as `image/png` and named `*.tif.png`: the Java export table knew only MP4, JPEG and JXL | `image/tiff` and `.tif`/`.tiff` in `AndroidStorageContract`; JVM test |
| Android workflow cache keys hashed `xtask/src/main.rs` after the native build moved to `xtask/src/android.rs`, and a root `build.rs` that does not exist | keys hash `xtask/src/**` and `crates/**/build.rs` |
