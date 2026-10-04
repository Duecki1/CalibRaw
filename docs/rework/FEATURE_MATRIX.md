# Feature matrix

Working table for the rework: each feature group, where it runs, the checks
that protect it, and the gaps. "CI" means `cargo test --workspace` on Linux
without models; "GPU" means an explicit local run with an adapter.

| Feature group | Platforms | Existing tests and fixtures | Gaps | Acceptance evidence |
|---|---|---|---|---|
| Image loading (RAW via LibRaw/Rawler, JPEG/PNG/HEIC/TIFF, orientation, metadata, embedded thumbnails) | all | core `raw_loader` (17), `libraw_loader` (32), `rawler_loader/tests` (13), `rendered_loader` (7), `tiff_loader` (10), `source_format` (5), `exif_metadata` (2); `raw_open_bench` fingerprint | no redistributable camera RAW in the repository; LibRaw paths use the local fixture | CI + local `raw_open_bench` fingerprint `0ef93f3cdaae2b92` |
| Colour processing (profiles, DCP/ICC, white balance, transfer functions, HDR/negative values) | all | core `color_profile/tests` (8), `dcp`, `icc`, `white_balance_presets` (4), `color_math` (4), `sigmoid` (8), `basicadj` (15); gpu `tests` (22), `black_tone_tests`, `point_color_tests` (8) | — | CI + GPU tests |
| Processing (demosaic modes, tone/detail/noise, stage order, region/tile equivalence) | all | core `processing` (22), `noise` (5); gpu `photographic_modules_tests` (11), `tests` (tile/region equivalence), `layout_contract_tests` (5) | — | CI (GPU tests run in CI when an adapter exists, otherwise locally) + byte-identical export golden |
| Geometry (crop, rotation, flips, lens corrections, coordinate mapping) | all | core `geometry` (15), `lensfun` (2), `masks/lens_tests` (7), `masks/zoom_tests` (3); gpu `export/tests` (resampling) | — | CI |
| Masks and effects (brush/shape/depth/subject/object, composition, effect parameters and packing) | all | core `masks/tests` (81), `raster_cache/tests` (16), `effects/tests` (5), `content_dependencies` (4); gpu `fog_tests` (14), `existing_effects_tests` (8), `film_effects_tests` (3), `light_rays_tests` (3); ui `mask_regression_tests` (8), `mask_effects/controls_tests` (12) | — | CI + `mask_rasterization` bench checksums |
| AI (installation, consent, providers/fallback, denoise, remove, masks, retention) | all (models download on demand) | ai `model_artifact` (7), `model_install` (2), `model_runtime` (8), `execution_provider` (1), `remove` (5), `lama` (10), `ai_denoise` (11), `ai_masks` (8), `depth` (14), `sky` (3), `object` (5); ui `app/ai/*` (29) | 9 ignored tests need downloaded models/ONNX Runtime | CI; model-backed checks are manual |
| Editing and persistence (undo/redo, grouped edits, sidecars, presets, serde names, unsupported data) | all | ui `edit_history` (15), `sidecar_persistence` (3); core `sidecar/tests` (55, incl. legacy curves, legacy inline depth, unknown fields, corrupt and future schemas), `presets/tests` (14, incl. newer schemas) | see policy below | CI |
| Presets and library (groups, preview/apply, dialogs, selection/filtering, thumbnails, catalog, batch actions) | all | ui `library/tests` (59), `app/presets` (2), `presets/preview` (5), `ui/presets` (2), `library/review` (2), `library/hdr` (2); core `thumbnail_cache` (9) | — | CI + screenshot review |
| Export and replay (formats, bit depths, resize, profiles/metadata, replay ordering, platform codecs) | all; replay MP4 via ffmpeg (desktop) or MediaCodec (Android) | gpu `export/tests` (33); ui `processing_export/tests` (8), `replay` (13), `replay/android` (2), `export_naming` (5); Java `ReplayVideoEncoderTest` | the replay end-to-end test (`replay_renders_encodes_and_publishes_an_mp4`: render, encode, publish an MP4) is `#[ignore]` (needs a wgpu adapter and FFmpeg with libx264) and runs manually | CI + byte-identical export golden; replay end-to-end test run manually |
| UI and platforms (themes, responsive layouts, keyboard/pointer/touch, desktop accessibility, Android storage/lifecycle) | all | ui `adjustment_slider` (15), `preview/tests` (10), `sidebar` (8), `actions` (10), `preview_visibility` (7); `portrait_gpu_layout_and_input` (GPU); `gpu_ui_review` (132 captures, GPU); Java `AndroidStorageContractTest` | no automated accessibility checks; no Android runtime smoke test | screenshot comparison at tolerance 0; Java unit tests (`:app:testDebugUnitTest`) in the Android CI workflow (`build-linux-android.yml`); manual Orca review for shared controls |
| Failure and recovery (interrupted saves, cancelled output, malformed data, stale results, allocation failure, cleanup) | all | core `file_ops` (3), `sidecar/tests` (corrupt/future); gpu `export/tests` (cancellation, writer errors), `gpu_errors` (1); ui `foreground` (3), `inpainting` (1), `ai/generated` (7) | — | CI |

## Persistence compatibility policy (current behavior)

- New versions read older settings, presets and sidecars; legacy fields are
  migrated on load (tests: legacy curves, legacy inline depth, legacy film
  adjustments).
- A sidecar or preset with a **newer schema version** is rejected as
  `Unsupported`; it is never overwritten implicitly. Desktop offers an explicit
  backup-and-replace (`backup_and_replace_desktop_sidecar`).
- **Unknown fields within the current schema are ignored** on load and are not
  written back on the next save.

Open decision (plan §16.1): whether older app versions must read files written
by newer ones, which affects how new fields are introduced (schema bump versus
optional field). The rework does not change any persisted format, so the
decision is recorded here and left to the maintainer.
