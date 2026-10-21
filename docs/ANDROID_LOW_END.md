# Low-end Android

Plan for making CalibRaw smooth on the minimum supported Android device and
stopping the AI jobs that get it killed. Status: steps 1 and 2 of the order
(P1.1, P0.1–0.3, P2.1, P2.2) are done and measured with `android-bench`.

## Test device and method

- Lenovo Tab M11 (TB330FU): MediaTek Helio G88 (2× Cortex-A75, 6× A55),
  Mali-G52 MC2 (Vulkan 1.3, driver r49p1), 4 GB RAM with 2.9 GB zram,
  1920 × 1200 at 90 Hz, Android 15. With the system and background apps
  running, about 1.45 GB is available before CalibRaw starts.
- Build: `529ffd2e` (v1.3.1) with release-optimized Rust in a debuggable APK,
  plus a temporary GPU timestamp around every compute pass. The release APK
  `1.3.1` (versionCode 1000958, 26 commits older) was measured first; its
  open and slider timings match, its warm-cache startup preparation took
  21.6 s.
- Photo: `Demo2.5/DSC03335.ARW` (Sony ILCE-7CM2, 4688 × 7028) with its
  sidecar: one Object mask, colour NR 3.6, capture sharpening 40, Lensfun
  correction, Medium preview (652 × 978).
- Frame timing from SurfaceFlinger timestats (`present2present`,
  `post2acquire`); GPU time from the timestamps (stage totals are reliable;
  the split between adjacent passes is not, because Mali overlaps
  dispatches); memory from `/proc/<pid>/status`, `dumpsys meminfo`,
  `dumpsys gpu --gpumem`; kills from the `lowmemorykiller` log. Gestures were
  `adb shell input swipe` over 2 s.

## Starting point

### AI jobs (all on the CPU: pyke's ONNX Runtime has no XNNPACK, Android has no GPU provider)

| Job | Model input | Session | Run | Peak RSS | Lowest MemAvailable | Outcome |
|---|---|---:|---:|---:|---:|---|
| Subject, Medium (default) | BiRefNet 1024 | 10.1 s | killed after 58 s | 1.25 GB + 2.04 GB swapped | — | **killed by LMK** |
| Subject, Low | BiRefNet 768 | 9.5 s | 18.8 s | 1.69 GB | 455 MB | ok |
| Object | SAM 2.1 tiny 1024 | 4.8 s + 0.5 s | 14.3 s | 1.87 GB | 382 MB | ok, near the limit |
| Sky | U²-Net 320 | 0.5 s | 5.3 s | 1.13 GB | 866 MB | ok |
| Depth (low-memory profile) | DA-V2 Small 518 | 0.5 s | 7.5 s | 1.15 GB | 804 MB | ok |
| Remove, first stroke | Big-LaMa 512 | 27.1 s | ~27 s (+1.2 s GPU crop) | 1.09 GB + 0.2 GB | 965 MB | ok, ~55 s total |
| Remove, next stroke | warm session | — | ~34 s total | | | ok |

- The Medium subject mask is not survivable here: the registry's own
  measured peak for BiRefNet 1024 is 4.6 GB. Before CalibRaw died the
  low-memory killer made 42 kills of other processes, including the
  launcher, the keyboard and Google services, several of them restarted and
  killed again ("device is not responding").
  Onboarding offers Medium as "for most devices · recommended".
- **Freed memory stays in the process.** After the Low subject job the
  native heap held 1.68 GB mapped but only 275 MB allocated; idle RSS stayed
  at 1.90 GB and MemAvailable at 297 MB, so the next job or any other app
  triggers the killer. A temporary `mallopt(M_PURGE_ALL, 0)` after the job
  brought RSS from 1717 MiB to 417 MiB in 362 ms. Apps run with
  `M_DECAY_TIME = 1`, which releases memory only during later allocations.
- Nothing checks free memory before a job, and the app ignores
  `onTrimMemory`. During the Medium subject job the preview was rebuilt
  (GPU 468 → 607 MB) in the middle of inference.
- Only Depth uses the low-memory CPU profile. The other models run with
  ONNX Runtime's default thread pool, weight prepacking (a second, packed
  copy of the weights) and full graph optimization at every session
  creation.
- The Android mask source is the preview read back (~652 × 978), yet the
  Medium model resizes it up to 1024 × 1024.
- After a kill, Android restarts the app into the library with no message;
  the empty mask stays in the sidecar (reopening does not retry, so there is
  no crash loop). After a Remove stroke, "Update AI results" re-runs every AI
  mask of the photo.

### Editing

| Interaction | GPU per preview update | Preview updates/s | UI fps | Notes |
|---|---:|---:|---:|---|
| Light, Color (hue, vibrance), capture sharpening | Output 54–77 ms | 13–18 | 18–28 | 110–120 ms from buffer post to GPU done |
| Mask-local sliders, Grow | Output 69–73 ms | ~14 | 15–20 | |
| Straighten, Transform | Output 69–73 ms | ~14 | 15–18 | |
| Temperature, Tint, luminance and colour NR (all Raw-stage controls) | Raw 400 + Tone 4 + Output 54 ms | ~2 | 6–9 | UI frames stall 450–500 ms |
| Any slider while zoomed | detail 58 + fitted 54 + navigation 9 ms | ~8 | 9 | 250–350 ms latency |
| Pan when zoomed | Raw 417 ms when it settles | | 48 | |
| Panel scroll, crop handles, Remove brush | UI only | | 46–69 | egui itself is fine |
| Object selection stroke | | | 21 | |

- Output at 652 × 978 is three passes: `prepare_scene_node` ~34 ms (camera
  characterization, dehaze, exposure, local exposure, profile look),
  `apply_scene_tone_node` ~20 ms (capture sharpening, tone), the view
  ~0.2 ms. Nothing upstream of the dragged control is reused.
- Raw at 652 × 978: RCD and demosaic finish ~260 ms, colour NR (six
  scales) ~140 ms. RCD reads about 70 texels per pixel straight from
  textures, with no workgroup tiling.
- Cost follows pixels: the 256 × 384 navigation pipeline runs Output in
  8.8 ms and Raw in 96 ms.
- Every UI frame dispatches the next pending stage. Only the white-balance
  refresh and deferred mask edits wait for the previous dispatch
  (`interactive_render_ready`), so the slider thumb and the panel wait behind
  queued compute. Android already presents with frame latency 1.
- While zoomed, every update also refreshes the hidden fitted preview and the
  navigation proxy ("panning can expose any part of it").

### Startup, open and resident memory

- Program preparation at startup, on a background thread at ~100 % of one
  core: cold cache 10.1 + 3.7 + 11.0 s, warm cache 3.5 + 2.7 + 3.4 s. RSS
  grows by ~150 MB during it.
- Opening the 33 MP ARW takes ~6 s: LibRaw unpack 1.3–1.5 s, mosaic
  0.7–0.9 s, the Lensfun database parsed twice (`catalog` and `apply` each
  call `Database::load`, ~0.4 s each), Lensfun TCA/shading on the full CFA
  1.5–1.9 s, highlight analysis 0.4 s.
- With the photo open the release build used 1.03 GB RSS: native heap 586 MB
  of which 215 MB allocated (404 MB free but resident), GPU 355–420 MB.
  MemAvailable dropped to 0.8–0.9 GB.
- Back in the library the document stays loaded: 504 MiB GPU and 723 MB RSS
  with no photo open. GPU use reached 580 MiB against the 477 MiB
  working-set budget.
- Remove renders a 1339 × 1237 GPU crop (Raw ~1 s) before LaMa runs.

## Principles

1. **The same picture on every device.** Export and the settled preview stay
   numerically unchanged. Devices differ only in what the preview shows while
   a control is dragged and in which AI models they offer.
2. **Refuse instead of dying.** A job that cannot fit is refused with
   `report_error`, or runs a smaller model, never killed by the system.
3. **Measure each step on the tablet** against the numbers above (P0).
4. Behaviour-preserving and behaviour-changing steps are separate changes,
   as AGENTS.md requires; persisted-setting changes come with a migration.

## P0 — Measurement (behaviour-preserving)

### 0.1 GPU stage timer

`calibraw-gpu` writes a timestamp around each compute pass and logs stage
totals through `diagnostics`, only when a diagnostics switch is on; the
device then requests `TIMESTAMP_QUERY` where the adapter offers it (pass
timestamps need nothing more). All passes go through `dispatch_compute`, so
the hook is one function plus the stage entry points. Zero cost when off.
Done: `calibraw_gpu::stage_timing`, switched on by `CALIBRAW_GPU_STAGE_TIMING=1`
or the Android launch extra `de.duecki.calibraw.GPU_STAGE_TIMING`.

### 0.2 Profiling build

A Gradle build type with release Rust and native libraries, `profileable`,
and an `applicationIdSuffix` so it installs beside the release app and keeps
its data. Today the debug variant builds Rust at opt-level 1 and shares the
release application ID, so profiling means uninstalling the release app.
Done: `./gradlew assembleProfiling` (`de.duecki.calibraw.profiling`).

### 0.3 Device benchmark

`cargo xtask android-bench`: install, open a Demo photo, run the gestures
above through `adb shell input`, and collect timestats, stage times,
`meminfo`, `gpumem` and kill lines into JSON, compared like
`baseline-compare`. This document's numbers are the first baseline.
Done: the gestures are data (`xtask/android-bench/tab-m11.json`, with the
photo's original sidecar), `android-bench-compare` sets two runs side by
side. First run: `~/calibraw-baselines/2026-10-21-140044248-step1`. Its
GPU figures repeat the tables above (Output 54 ms; Raw 400 ms and 5–6
updates per 2 s temperature or NR drag; zoomed drags update the 256 × 384,
652 × 978 and 914 × 722 pipelines alike); back in the library the app still
holds 712 MiB RSS and 419 MiB GPU (P4.1).

### 0.4 Static shader analysis

Compile every compute entry point to SPIR-V with naga and run Arm's Mali
Offline Compiler (`malioc`) for Mali-G52 (and a Valhall part such as G57 or
G68): work registers, stack spilling, bound unit, cycles. Bifrost has 64
registers per thread and loses occupancy above 32; spilling is the likely
cause of RCD and `prepare_scene_node` costing several times their arithmetic.

### 0.5 Heap attribution

A heapprofd capture of startup, open and one AI job, to attribute the live
heap after open (two full-size `LoadedRaw` copies, original and
lens-corrected, are 2 × 66 MB of `u16` alone) and the retained pages.

## P1 — AI without kills (behaviour change: availability and messages, not output)

### 1.1 Return freed memory

Call `mallopt(M_PURGE_ALL, 0)` (API 34+; `M_PURGE`, API 28+, before) after
every AI job once its session and buffers are dropped, after opening and
closing a document, after startup program preparation, when leaving
Develop and on memory callbacks (4.2). Measured: −1.3 GB in 0.36 s.
`calibraw-ai`'s `jobs.rs` and `slot.rs` may not name an OS, so the call
lives in `platform/` (or behind an `AiCapabilities` hook) and the
application handlers trigger it.
Done: `Platform::release_free_memory` (`calibraw_ffi::android::heap`, on its
own thread) runs after every AI job (`poll_ai_job`), every document load,
startup program preparation and leaving Develop, and logs resident memory
before and after; memory callbacks follow with 4.2. Measured (`14004424`,
profiling build): after open the native heap holds 218 MiB mapped for
211 MiB allocated (before: 586 for 215) and RSS is 687 MiB (1.03 GB); after
startup preparation RSS drops from 409 to 254 MiB.

### 1.2 Admission before a job

Record each model's CPU peak (the RSS increase measured on this device) in
the registry beside the GPU figure. Before a job: purge, free what the job
does not need (on constrained memory release the preview pipeline as AI
denoise already does in `ai_denoise.rs`), then compare with
`ActivityManager.MemoryInfo.availMem`. If it still does not fit, refuse with
`report_error` naming the shortfall and the smaller choice. Hold preview
rebuilds and other GPU work until the job ends.

Record the running job (as `GpuJobRecord` does for GPU work). On the next
start, `ApplicationExitInfo` (`REASON_LOW_MEMORY`, API 30+) tells whether the
system killed the app during it; then explain it in a dialog and switch the
default down.

### 1.3 Memory tiers

Split `MemoryClass::Constrained` by total RAM (and `isLowRamDevice`): a
4 GB tier gets Low subject quality by default and does not offer Medium,
gates SAM and LaMa by admission, and defaults to Low preview quality.
Onboarding text follows the tier. Moving saved Medium settings on such
devices is a settings migration.

### 1.4 Lean CPU sessions

Use the low-memory profile for every model on Android: no prepacking,
sequential execution, 2–4 intra-op threads (the A75s first), no spinning.
Measure time and peak per model; Depth already runs this way.

### 1.5 Faster, file-backed sessions

Session creation costs 10 s for BiRefNet and 27 s for LaMa, mostly graph
optimization and prepacking. Either ship pre-optimized ORT-format models or
save the optimized model on first use (`session.optimized_model_filepath`),
and load it memory-mapped (`session.use_memory_mapped_ort_model` with
`session.use_ort_model_bytes_for_initializers`) so weights are clean file
pages the kernel can drop instead of anonymous memory pushed to zram. New
artifacts change registry hashes and need the AI parity goldens.

### 1.6 Smaller models for the 4 GB tier

Separate changes, each with parity goldens and a visual review:

- Subject at 768 or 512 input; the Android source is ~652 × 978 anyway.
- Remove: MI-GAN (~6 M parameters, an ONNX export exists; check the weight
  licence) or LaMa on a smaller crop.
- Object: a lighter SAM-class encoder, or the encoder at a lower input.

### 1.7 Batched AI refresh

"Update AI results" runs one job at a time with admission and purge between
jobs.

## P2 — Interactive editing (behaviour change only while dragging, except 2.1 and 2.2)

### 2.1 One preview dispatch in flight (behaviour-preserving)

Gate every stage dispatch on `on_submitted_work_done` of the previous one,
the mechanism white balance already uses, and coalesce to the newest
parameters. The UI and the slider thumb keep their own frame rate; the
preview updates as fast as the GPU allows, with one frame of latency
instead of two.
Done: `PreviewDispatchGate`. Each batch renders every pending stage of each
preview, so every batch ends in a displayed update; the white-balance path
already worked this way and no longer needs its own flag. Measured against
`step1` (two runs, `2026-10-21-884f37b1d-p2.1-p2.2*`): exposure drag 15.3 →
22.2–22.6 fps, post-to-acquire p50 110 → 58 ms; luminance NR drag 1 → 6
displayed updates in 2 s (a new Raw dispatch used to restart the chain
before Output). Raw-stage drags stay at 7.5–8.1 fps (7.3–8.7 before) with
the longest frame 500 instead of 450 ms: UI frames now wait for a whole
Raw → Output batch, which 2.3 and 2.4 shrink.

### 2.2 Zoomed drags update only the visible crop (behaviour-preserving)

Extend `defer_background_mask_processing` from mask edits to all controls:
the fitted preview and the navigation proxy catch up when the drag ends.
Saves ~55 % of GPU time per zoomed update.
Done: `defer_background_preview_processing`, for any drag whose change is
Output-stage only. Zoomed exposure drag 9.6 → 21.2–21.3 fps; the crop
updates 27 instead of 16 times, the fitted and navigation previews once, on
release.

### 2.3 Interactive resolution

While a control is dragged, render into a smaller interactive pipeline and
render the preview once at full size on release. Choose its size from the
measured stage time, aiming at 30 updates per second (the navigation size
already runs Output in 8.8 ms). On fast GPUs the size equals the preview
and nothing changes.

### 2.4 Raw-stage controls while dragging

Temperature and tint apply as a gain on the demosaiced scene during the
drag; the exact Raw stage runs on release. NR, highlight reconstruction and
CA use the interactive resolution. Targets ≥ 10 updates per second instead
of 2.

### 2.5 Reuse the characterized scene (behaviour-preserving)

Write `profiled_scene_at` (camera characterization, gamut compression, HSV
maps) once per Raw result to a work texture (≈5 MB at preview size) and
start the Output stage from it, if 0.1 and 0.4 confirm it dominates
`prepare_scene_node`. Results must stay bit-identical; the layout contract
tests and parity cover it.

## P3 — Shader cost on Mali (behaviour-preserving unless noted)

- Act on 0.4: split or specialize register-heavy entry points (pipeline
  constants for features an edit does not use) until nothing spills.
- Tile neighbourhood passes through workgroup memory: RCD, colour NR scales,
  capture sharpening, tone guide. Tile halos and boundary handling stay as
  they are.
- Optional, numerical change: `f16` arithmetic in preview-only passes where
  `SHADER_F16` exists (Mali-G52 reports `shaderFloat16`), only if the above
  is not enough.

## P4 — Resident memory while editing (behaviour-preserving)

- **4.1** On constrained memory, release the preview pipelines, mask atlases
  and AI caches when leaving Develop; keep the decoded RAW cache entry.
- **4.2** Forward `onTrimMemory` and `onLowMemory` through the JNI bridge:
  drop the decoded RAW and thumbnail caches, auxiliary previews and warm
  sessions, then purge (1.1).
- **4.3** Reduce what 0.5 attributes, starting with the second full-size RAW
  copy.
- **4.4** Count every GPU allocation against the working-set budget (580 MiB
  used against 477 MiB).

## P5 — Startup and open (behaviour-preserving)

- **5.1** Keep the Lensfun database loaded for the process (−0.8 s per open).
- **5.2** Run Lensfun TCA/shading after the first preview, on the GPU, or
  only for export and detail crops (1.5–1.9 s today).
- **5.3** Cache naga's SPIR-V per shader hash and create Vulkan modules with
  wgpu's SPIR-V passthrough, or compile at build time. On constrained
  devices compile effect programs on first use instead of
  `compile_common_programs` at start.
- **5.4** Do not create the GL instance on Android; startup still probes EGL.

## P6 — ONNX Runtime on Android (research, then behaviour change)

- Benchmark the official `onnxruntime-android` package (ships the XNNPACK
  provider) loaded dynamically against pyke's static CPU build: time and
  peak per model on this tablet. XNNPACK packs weights itself, so measure
  memory, not only time.
- Thread placement on 2 big + 6 little cores.

## Performance targets (this tablet)

| Area | Target |
|---|---|
| Output-stage drags | UI ≥ 30 fps, preview ≥ 20 updates/s, ≤ 100 ms from touch to preview |
| White balance, NR drags | preview ≥ 10 updates/s, no UI stall over 100 ms |
| Zoomed drags | as fitted |
| AI jobs | none killed; a refused job says why; MemAvailable within 100 MB of its pre-job value afterwards |
| Library, no photo open | RSS < 300 MB, GPU < 150 MB |
| Opening a 33 MP RAW | editable preview ≤ 4 s |

## Order

1. P1.1 (one small change, largest relief) and P0.1–0.3.
2. P2.1 and P2.2 (low risk, behaviour-preserving).
3. P1.2–1.5, P4.1–4.2.
4. P2.3–2.5, guided by P0.4.
5. P3, P5, P1.6, P6.

## Not tested

Export (it writes to the shared Pictures folder), HDR merge, focus stack,
AI denoise, a library with many photos, X-Trans files, portrait orientation
and thermal throttling over a long session.

## Open decisions

- Which AI models the 4 GB tier offers, and whether Medium is hidden or only
  refused by admission.
- Whether a softer preview while dragging (2.3, 2.4) is acceptable.
- Tier thresholds by RAM alone, or also by a measured GPU speed.
- Shipping ORT-format models versus optimizing them on the device.
- The official ONNX Runtime package versus pyke's build on Android.

## References

- Android bionic, [native allocator notes](https://android.googlesource.com/platform/bionic/+/main/docs/native_allocator.md):
  `M_DECAY_TIME`, `M_PURGE`, app defaults.
- ONNX Runtime: [XNNPACK execution provider](https://onnxruntime.ai/docs/execution-providers/Xnnpack-ExecutionProvider.html),
  [mobile deployment](https://onnxruntime.ai/docs/tutorials/mobile),
  [session option keys](https://huggingface.co/csukuangfj/ios-onnxruntime/blob/main/1.22.2/onnxruntime.xcframework/Headers/onnxruntime_session_options_config_keys.h)
  (`session.disable_prepacking`, `session.use_memory_mapped_ort_model`,
  `session.use_ort_model_bytes_for_initializers`).
- MI-GAN: [paper](https://openaccess.thecvf.com/content/ICCV2023/papers/Sargsyan_MI-GAN_A_Simple_Baseline_for_Image_Inpainting_on_Mobile_Devices_ICCV_2023_paper.pdf),
  [repository](https://github.com/Picsart-AI-Research/MI-GAN).
- Background-removal model comparisons:
  [withoutBG](https://withoutbg.com/models),
  [BiRefNet vs RMBG vs MODNet](https://lofttools.com/blog/browser-background-removal-models/).
- Arm: [Bifrost shader core](https://developer.arm.com/community/arm-community-blogs/b/mobile-graphics-and-gaming-blog/posts/the-mali-gpu-an-abstract-machine-part-4---the-bifrost-shader-core),
  [Mali Offline Compiler](https://learn.arm.com/learning-paths/smartphones-and-mobile/ams/malioc).
- wgpu: [shader module passthrough](https://wgpu.rs/doc/wgpu/api/shader_module/type.ShaderModuleDescriptorPassthrough.html).
