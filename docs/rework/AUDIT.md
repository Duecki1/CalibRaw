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
| 20 | Global tone curves as 36 `vec4` uniform fields, a 64-case WGSL switch, and a second Hermite/tangent evaluator for mask curves | global and local point curves | — (the local zero-slope guard was added afterwards as a behavior fix) | `tone_curves: array<array<vec4<f32>, 9>, 4>` (same bytes); shared `tone_curve_secant`/`interior_tangent`/`hermite` | golden hashes identical (see below); `layout_contract_tests` |
| 21 | Chromatic-aberration warp (3 shaders) and scene-source sampling (tone analysis, scene adjustments) | tone analysis, scene adjustments, CA finish | — | `Common::ca_warped_pos`; composable `scene_source.wgsl` declaring binding 11 | golden hashes; Naga validation at both qualities |
| 22 | Bayer/X-Trans finish helpers (dual-demosaic confidence, Scharr detail, YUV→RGB), four 1-4-6-4-1 kernel weights, three hash finalizers, two circular hue distances | `pass4`, `xtrans_finish`, scene adjustments, creative effects, blur, colour denoise, atmosphere, light rays, view transform | `bayer_uv` (`dot`) and `xt_uv` (explicit sum) stay separate to keep FMA rounding; false-colour strengths unchanged | shared functions in `noise_ca_finish`, `Common::binomial5_weight`, `mask_effect_hash_unit`, `Color::circular_hue_distance` | golden hashes |
| 23 | 15 blur/glow bind-group fields and passes written out per step; destructure lists repeating struct fields | pipeline builder | labels and entry-point names unchanged | `[wgpu::BindGroup; 5]` arrays and loops | golden hashes; 0/132 screenshot diffs |
| 24 | Readback padding/copy/map, R16 mask-layer upload (3 copies), RGBA16F/32F row upload, crop-job checks (3 copies) | readback, mask layers, resources, Remove scene, export crops | `update_mask_layer` keeps its own error text | private helpers | golden hashes; export byte-identical |
| 25 | `LoadedRaw` rebuilt field by field for crops, proxies, tiles, lens-corrected mosaics and AI-denoise tiles (5 copies) | core processing, Lensfun, AI denoise | per-site noise profile, lens geometry, AI-denoise slot and opposed-chroma reference source | `LoadedRaw::derive_with` | `raw_open_bench` fingerprint; core tests |
| 26 | Brush-dab footprint loop (4 copies), grouped max coverage (2), Lensfun coordinate/vignette batches, three base64 serde modules | brush rasterization, Lensfun correction, sidecar/mask/Remove serialization | `arc_u16_le_base64` is a different format and stays | `for_each_dab_coverage`, `StrokeGroupCoverage`, Lensfun batch helpers, `crate::base64_arc_bytes` | bench checksums identical (brush 7–11% faster); serialized bytes identical |
| 27 | Dead code: `thumbnail_cache::fingerprint_file`, `save_android_with_review` | none | — | removed | Android `cargo ndk check` |

Rows 20–27 were accepted with a temporary golden-hash harness (Bayer and X-Trans, Preview and High, three demosaic modes, three highlight methods, non-identity global, channel and local curves, CA, every mask effect at partial coverage: 104 hashes identical before and after), the export golden `ed2103ff…`, the `raw_open_bench` fingerprint and 0/132 screenshot differences.

## Remaining candidates

| Candidate | Callers | Note |
|---|---|---|
| Saved document preparation (sidecar → profile → decode → lens → AI denoise → mask source) implemented three times | interactive open, desktop batch export, library thumbnails | the copies already differ (`"."` camera profile, lens selection, AI-denoise restoration); merging changes batch and thumbnail output, so it is a separate behavior change |
| Mask effects listed in parallel (15 UI files, 15 validators, GPU packing, effect groupings) | sidebar, sidecar validation, GPU params | |
| Neon and Edge Glow edge detector | mask effect shaders | Neon's pass layout has no binding 24, so sharing needs a layout change |
| UI chrome: context menu vs selection bar, settings vs onboarding controls, name/confirm/progress dialogs | library, presets, settings, masks | |
| Views still taking `&mut CalibRawApp` | 129 functions under `ui/` | see [COVERAGE.md](COVERAGE.md) |
| Java copy-then-fsync (8) and best-effort delete (4); FFI JNI call wrappers | Android storage, export, thumbnails | |

Found while consolidating and fixed separately (see [COVERAGE.md](COVERAGE.md), pre-existing defects): mask curves whose first point lies above x = 0 extrapolated negative scene values with the endpoint tangent instead of mapping them to the curve's black like the global curve.

## Reviewed, no change

| Area | Outcome |
|---|---|
| WGSL helpers | only `log_luminance` was duplicated; colour, sampling and noise helpers already live in composable modules |
| `demosaic_format` / `highlight_work_format` / `work_format` | always equal today but name distinct texture roles in 34 places; kept, documented equal at the specialization site |
| Root and `android/` `settings.gradle` | Gradle requires `pluginManagement` in each settings file (an applied script is rejected); the copies stay |
| Android SDK, NDK, build-tools, LibRaw and Lensfun versions | already read from `[workspace.metadata]` by Gradle, xtask and the Android workflow |
| Background job lifecycles | policies differ by design (coalescing, document generation, cancellation); documented per job in ARCHITECTURE.md instead of a shared scheduler |
