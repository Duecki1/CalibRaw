# Duplication audit

Ranked candidates from the time-boxed audit (plan §12, step 5). Savings are
production code lines as counted by `cargo xtask loc`; "done" rows were
accepted with the listed evidence.

## Implemented

| Rank | Candidate | Callers | Intentional differences kept | Result | Acceptance |
|---|---|---|---|---|---|
| 1 | GPU pipeline registered its own egui texture (`egui_texture_id`, unused egui constructors) | UI previews (main, detail, navigation), tests | Android frees a replaced detail texture immediately | `PreviewPipeline`/`RegisteredTexture` in the UI own registration; GPU crate egui-free | 0/132 screenshot diffs vs corrected reference; export byte-identical |
| 2 | Shader source list repeated in `gpu.rs` constants, `ShaderManager` registration, `load_shader_set`, two validation tests, layout tests and `build.rs` | production pipeline build, tests | specialization stays explicit per entry (a marker test proves it matches) | one `shaders.rs` registry; `build.rs` removed (`include_str!` already tracks files) | all shaders validate at both qualities (was 7 at High); layout tests |
| 3 | Five `RawGpuPipeline::new_headless_*` constructors differing only in options | 50 call sites in GPU, AI, UI | — | one `new(.., PipelineOptions)` | full test suite, export golden |
| 4 | Headless wgpu device setup (adapter fallback, texture check, GL limits) | CLI, library thumbnail renderer; base limits also in the window device | callers keep their own error messages | `request_headless_device`, `base_device_limits` | export golden; CLI drops `pollster` |
| 5 | Export destinations queried Android FFI state from the GPU crate | UI export, batch, replay, CLI | descriptor targets are written in place | `ExportTarget::{File, Descriptor}`; GPU no longer depends on FFI | new descriptor test; arch-check |
| 6 | Full-frame `RemoveSceneContext::new(.., [0, 0], [w, h])` | 9 preview/AI sites | crops keep explicit origins | `RemoveSceneContext::full_frame` | tests |
| 7 | JNI `String` result conversion and empty-path checks | 13 + 8 FFI wrappers | each wrapper keeps its error text | `java_string`, `non_empty_path` | Android clippy; `jni-contract` 55/55 |
| 8 | Replay planning/rendering/encoding inside the app with egui repaint | replay command | — | `services::replay` with callbacks and typed errors | 2 new end-to-end tests (render+encode, cancellation) |
| 9 | Pass indices copied field by field from `StageIndices` into `RawGpuPipeline` | pipeline dispatch | — | the pipeline stores `StageIndices` | GPU tests |
| 10 | Tone-statistics pipeline choice duplicated in detail and processing paths | zoomed detail render and refresh | — | `full_frame_tone_pipeline` | tests |
| 11 | Library constructors took the same five preferences separately on desktop and Android (Android tripped `too_many_arguments`) | startup | platform-specific folder argument | `LibraryPreferences` | Android clippy clean |
| 12 | `log_luminance` defined identically in two WGSL modules | detail capture, scene adjustments, scale space | — | `Common::log_luminance` | shader validation; GPU image tests |
| 13 | Numeric value fields built on `egui::DragValue` with per-site formatting and arrow handling | export, settings, point colour, slider value field, depth range | depth range keeps its own track | `moduwu_design::NumberField` | 0/132 screenshot diffs; NumberField tests incl. AccessKit |
| 14 | Slider interaction (track/handle hit areas, drag intent, scroll lock, reset, keyboard) inside CalibRaw's adjustment slider | every adjustment slider, zoom slider, depth range (scroll lock) | photographic gradients and `FloatParamSpec` adaptation stay in CalibRaw | `moduwu_design::Slider` with generic gradients, metrics and AccessKit slider semantics | 10 interaction tests moved to Moduwu; 0/132 screenshot diffs |
| 15 | 69 Moduwu items, icon-button aliases and layout types re-exported through `theme.rs`, `icons.rs`, `layout.rs` | 50 files | — | direct `moduwu_design` imports; `UiDesign`/`PreviewBackdrop` moved to `appearance.rs`; `dialog_text_field` moved to Moduwu | `ui-lint` rule `moduwu-reexport`; serde-name test |
| 16 | Android back-press routing published from 16 places, some with an incomplete condition | app, library views and actions | — | one end-of-frame `handles_back_navigation()` | host test; Android clippy |
| 17 | Hand-written stale checks `operation.document_id != generation` | AI denoise, object and generated masks | cancellation is still reported separately | `ForegroundOperation::is_for_document`; `sidecar_generation` renamed `document_generation` | existing stale-result tests |
| 18 | Rust toolchain version repeated in three workflow `toolchain:` inputs, four cache keys and two `rustup-init` calls | CI, macOS, Windows, Linux/Android workflows; `scripts/build_linux_release.sh` | — | all read `rust-toolchain.toml` (step output, `hashFiles`, `sed`) | YAML parses, script `bash -n`, command output checked locally; needs a real CI run |
| 19 | Java stream-copy loops beside `BoundedStreams.copy` | profile import, Lensfun asset copy | profile import keeps its per-file and per-tree limit messages | `BoundedStreams.copy` | 3 new JVM tests |

## Remaining candidates

None; long single-screen view functions are tracked in [COVERAGE.md](COVERAGE.md).

## Reviewed, no change

| Area | Outcome |
|---|---|
| WGSL helpers | only `log_luminance` was duplicated; colour, sampling and noise helpers already live in composable modules |
| `demosaic_format` / `highlight_work_format` / `work_format` | always equal today but name distinct texture roles in 34 places; kept, documented equal at the specialization site |
| Root and `android/` `settings.gradle` | Gradle requires `pluginManagement` in each settings file (an applied script is rejected); the copies stay |
| Android SDK, NDK, build-tools, LibRaw and Lensfun versions | already read from `[workspace.metadata]` by Gradle, xtask and the Android workflow |
| Background job lifecycles | policies differ by design (coalescing, document generation, cancellation); documented per job in ARCHITECTURE.md instead of a shared scheduler |
