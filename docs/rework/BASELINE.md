# Rework baseline

Recorded before structural changes. Later measurements are compared against
these values on the same machine; dependency updates get new reference runs
instead of overwriting this record.

## Revisions and environment

| Item | Value |
|---|---|
| CalibRaw | `82e81057` (`main`, after merging `presets` in PR #70) |
| Moduwu | `64acb1e` pinned in `Cargo.lock` (local `main` `aeae7f8` differs only in its README) |
| `Cargo.lock` sha256 | `e179b249cdd99b9bb4efa15d32e9e38b83daa340e9cd6508aa1dccc20317b9c6` |
| Toolchain | Rust 1.92.0 |
| Host | Linux 7.2.8 (CachyOS), AMD Ryzen 7 5700X3D, 46 GiB RAM |
| GPU | NVIDIA GeForce RTX 4070 Ti, driver 615.71.09, Vulkan |
| Artifact store | `CALIBRAW_BASELINE_DIR=~/calibraw-baselines` (outside every checkout) |

## Line counts

Method: `cargo xtask loc` (git-listed files; Rust module trees from `cargo
metadata` targets; `#[test]`/`#[cfg(test)]` items and modules count as tests;
code lines exclude blank and comment-only lines). Production code at baseline:

| Area | Production | Test | Build |
|---|---:|---:|---:|
| calibraw-core | 23,246 | 10,460 | 534 |
| calibraw-gpu (Rust) | 11,660 | 6,279 | 52 |
| calibraw-gpu (WGSL) | 6,703 | — | — |
| calibraw-ai | 7,002 | 1,707 | — |
| calibraw-ui | 45,970 | 10,137 | 50 |
| calibraw-ffi | 1,889 | — | — |
| calibraw-cli | 513 | 13 | — |
| xtask | 1,303 | 457 | — |
| Android Java | 2,825 | 433 | — |
| Build tooling (Gradle, CMake, scripts, workflows, manifests) | — | — | 2,358 |
| **CalibRaw total** | **101,111** | **29,486** | **2,994** |
| moduwu-design | 1,475 | 773 | 30 |

Measured on a clean worktree of the baseline commit (`cargo xtask loc <worktree>`);
the JSON report is kept with the baseline artifacts.

## Checks

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy … -D warnings` (CI flags) | **fail**: `redundant_clone` in a `calibraw-ai` test (`remove/lama.rs`); fixed first |
| `cargo test --locked --workspace --all-targets` | pass in 1 m 46 s: core 414, gpu 130 (+2 ignored), ui 323 (+2 ignored), ai 69 (+9 ignored), xtask 19, cli 1, lensfun contract 5 |
| Android `cargo check -p calibraw-ui --lib` (aarch64-linux-android) | pass |
| Android clippy (not in CI) | 3 findings in Android-only code (`processing_export/export.rs` redundant clone, `library/state.rs` too many arguments, `lib.rs` redundant clone) and a `missing_const_for_thread_local` false positive |
| `cargo xtask arch-check` | pass (core, moduwu rules) |
| Ignored GPU tests | `raw_demosaic_merge_preserves_highlights_and_shadows`: pass; `portrait_gpu_layout_and_input` (isolated `XDG_CONFIG_HOME`): pass |

Ignored tests that need downloaded models or ONNX Runtime (AI masks, depth,
sky, RawNIND) were not run; they are listed in [FEATURE_MATRIX.md](FEATURE_MATRIX.md).

## Screenshots

| Run | Captures | Notes |
|---|---|---|
| `2026-10-03-82e810577-ui-before` | 132 | 4 themes × 3 viewports × 11 states |
| `2026-10-03-82e810577-ui-repeat` | 132 | `baseline-compare` against `ui-before`: 0 differences at tolerance 0 |
| `2026-10-04-82e810577-ui-before-harness-fixed` | 132 | **reference for comparisons**: baseline code plus a one-line harness fix (below) |
| `2026-10-04-e7c871b5c-step6-number-field` | 132 | `NumberField` in export and settings: 0 differences against the reference |
| `2026-10-04-e7c871b5c-step7-slider` | 132 | slider moved to Moduwu: 0 differences |
| `2026-10-04-e7c871b5c-step8-theme` | 132 | theme re-exports removed: 0 differences |
| `2026-10-04-e7c871b5c-step9-module-splits` | 132 | module splits, back-navigation and export-view changes: 0 differences |
| `2026-10-04-e7c871b5c-step10-final` | 132 | final state after visibility tightening: 0 differences |
| `2026-10-04-e7c871b5c-step11-views` | 132 | settings cards and frame phases extracted: 0 differences |
| `2026-10-04-e7c871b5c-step12-library-grid` | 132 | library thumbnail grid extracted: 0 differences |
| `2026-10-04-0dba15690-rework2-before` | 132 | reference for the action-returning views (captured from a clean worktree): 0 differences against `step12-library-grid` |
| `2026-10-04-0dba15690-rework2-mask-tool` | 132 | mask tool returns actions: 0 differences against `rework2-before` |
| `2026-10-04-037956b8d-rework2-viewport` | 132 | preview viewport returns actions: 0 differences |
| `2026-10-04-fb5e986ec-rework2-mask-strip` | 132 | mask strip returns actions: 0 differences |
| `2026-10-04-f9c51dc02-rework2-mask-properties` | 132 | mask properties return actions: 0 differences |
| `2026-10-04-b610dc12c-step2-dedupe` | 132 | audit rows 20–27 (GPU, WGSL and core consolidation), working tree on `b610dc12`: 0 differences against `rework2-mask-properties` |
| `2026-10-04-0f71335e4-mask-curve-black` | 132 | intentional fix: mask curves map negative values below a lifted first point to black (no review state uses such a curve): 0 differences against `step2-dedupe` |

The harness is deterministic on this machine, so any later difference at
tolerance 0 is a real change.

**Harness defect found in step 4.** The harness reuses one GPU pipeline but
creates a new egui renderer for each of the 12 theme × viewport iterations.
`register_egui_texture` returned the ID cached from the first renderer without
registering with the new one, so 99 of the 132 `ui-before` captures show no
photo (the states without a photo were unaffected). Production uses one
renderer for its lifetime and was not affected. `ui-before-harness-fixed` was
captured from a clean worktree of the baseline commit with only
`pipeline.egui_texture_id = None;` added before registration; it is the
reference for later comparisons. `ui-before` is kept unchanged as recorded. Run folders contain `run-manifest.json` (revisions,
lockfile hash, command, environment, capture hashes) and the harness's
`manifest.json` (adapter, scene).

## Numerical fixtures

| Fixture | Command | Result |
|---|---|---|
| Local Sony ARW `RAWTEST/DSC01825.ARW`, 7028×4688, sha256 `16d9ba38…10e9` (not redistributable; kept outside the repository) | `calibraw-develop-export --input DSC01825.ARW --output export.png` | three runs byte-identical, sha256 `ed2103ffa8f97930b508fe8c16699c430a7e68e4a9d61025de31e9811f270acb` (run folder `2026-10-03-82e810577-export-before`) |
| same | `raw_open_bench DSC01825.ARW` | corrected pixel fingerprint `0ef93f3cdaae2b92` in all runs |
| Synthetic GPU scenes | `cargo test -p calibraw-gpu --lib` | covered by existing tile/region equivalence and effect regressions |

Byte-identical export output is the acceptance bar for structural GPU and
export changes on this machine. A tolerance is needed only for intentional
numerical work, which is out of scope for the rework.

## Performance (release build, warm OS file cache, 3 runs)

| Operation | Time | Peak RSS |
|---|---|---|
| `raw_open_bench` (decode, lens catalog and correction, highlight analysis, proxy) | 1.46–1.47 s | 270 MiB |
| headless develop export, full-size PNG | 5.65–5.95 s | 597 MiB |
| `cargo bench -p calibraw-core --bench mask_rasterization` | 512² brush: 289 MP/s positive, 575 MP/s erase; 2048² first raster 91.2 ms (generated source) / 86.5 ms (curved path); 2048² grow sweep 8.80 / 4.54 ms | — |

Final measurements (same machine, fixture and method; working tree on top of
`e7c871b5`):

| Operation | Time | Peak RSS |
|---|---|---|
| `raw_open_bench` | 1.38–1.40 s, fingerprint `0ef93f3cdaae2b92` | 269–270 MiB |
| headless develop export, full-size PNG | 5.17–5.20 s, sha256 `ed2103ff…270acb` (byte-identical) | 596 MiB |
| `mask_rasterization` | 512² brush: 302 MP/s positive, 617 MP/s erase; 2048² first raster 90.7 / 81.9 ms; grow sweep 8.40 / 4.44 ms | — |

Budget for the rework: no operation slower than its baseline by more than
normal run-to-run variance (≈5 % here) and no peak-memory increase.
GPU shader preparation is included in the export timing (no persistent
pipeline cache in the CLI).
