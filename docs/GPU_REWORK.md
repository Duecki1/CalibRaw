# GPU and platform rework

Plan for making local AI and platform handling uniform: one GPU path for
every vendor, one rule for GPU vs CPU, and platform differences decided once
instead of scattered through the code. Status: P0.1 (cfg ratchet), P0.2
(parity harness), P1, P2, P3, P4 (without the optional 4.5), P5 and P7
implemented; P0.3 (WebGPU spike) measured on Linux NVIDIA only, during P5;
P6.1 implemented without XNNPACK; P6.2 planned.

Target behaviour for AI: **run on the GPU when the model fits in free VRAM,
otherwise on the CPU** — the same rule for every model on every platform.

## Starting point

Snapshot of 2026-10-16 (v1.3.0, `151454ed`).

- **Execution providers are chosen per OS**
  ([execution_provider.rs](../crates/calibraw-ai/src/execution_provider.rs)):

  | OS | Providers | Reality |
  |---|---|---|
  | Linux | CUDA, TensorRT, ROCm | Automatic runtime is CPU-only unless the user picks CUDA 12/13 and installs the toolkit; AMD and Intel never accelerate; no ROCm package ships |
  | Android | NNAPI, XNNPACK | NNAPI is deprecated since Android 15; depth is forced to CPU anyway |
  | Windows | DirectML, CUDA, TensorRT | DirectML is in sustained engineering; the only provider with a VRAM check (DXGI) |
  | macOS | CoreML | float16 models never used |

- **Each model has its own OS exceptions:** depth CPU-only on Android
  ([depth.rs](../crates/calibraw-ai/src/ai_masks/depth.rs)), a Windows-only SAM
  encoder CPU profile and non-finite handling
  ([sam.rs](../crates/calibraw-ai/src/ai_masks/object/sam.rs)), BiRefNet float16
  only on Windows and Linux, High subject quality desktop-only, warm sessions
  disabled on Android and in the Linux AppImage. Only BiRefNet declares its
  GPU memory.
- **Out of memory does not fall back to CPU:** it disables GPU AI for the
  process and aborts the job; other errors do retry on CPU.
- **Runtime handling is spread out:** ONNX Runtime loading and its probe live
  in [ai_masks.rs](../crates/calibraw-ai/src/ai_masks.rs) although every model
  uses them; downloads in `onnx_runtime_artifact.rs`; provider choice in
  `execution_provider.rs`. Four `spawn_*` workers repeat the same thread, panic
  capture, runtime and install code. Five global setters configure the crate
  from UI startup. Every request carries an `Option<PathBuf>` /
  `Option<String>` pair for the runtime.
- **Platform branches:** about 1,240 `target_os` cfgs in 147 files, about 1,050
  of them Android branches in `calibraw-ui`. `PerformanceSettings` has 9
  platform-only fields, so the saved file differs per platform. Android tests
  are gated off Linux CI.
- **GPU devices:** five places create their own wgpu instance and adapter
  instead of [device.rs](../crates/calibraw-gpu/src/device.rs). The editor's GPU
  budget is unlimited on desktop and RAM-based on Android; AI never sees it.

## Overview

```
P0 safety nets + WebGPU EP spike  ──► gates P5/P6
P1 restructure calibraw-ai        (behaviour-preserving)
P2 OS checks → capabilities       (behaviour-preserving)
P3 one GPU context + memory query (behaviour-preserving)
P4 "GPU if it fits, else CPU"     (behaviour change)
P5 WebGPU EP on desktop           (behaviour change, gated by P0)
P6 Android AI                     (behaviour change)
P7 UI platform layer              (behaviour-preserving, parallel to P1–P6)
```

Every numbered step is one focused change that passes the
[verification](../AGENTS.md#verification) on its own. No step mixes moves
with behaviour changes.

---

## P0 — Safety nets and the WebGPU spike

**Goal:** make regression impossible to miss before anything moves, and
answer the WebGPU question with data.

### 0.1 cfg ratchet

`cargo xtask cfg-ratchet` (next to [arch.rs](../xtask/src/arch.rs)) counts
`target_os` cfgs per file, using the comment-aware scanner in
`rust_source.rs`, against a committed `xtask/cfg-baseline.txt`.

- CI fails when a file's count rises, or when a file outside the allowed
  directories gains its first one.
- A drop passes and prints the updated baseline.
- Allowed directories start empty and fill as P1/P7 create
  `calibraw-ai/src/{runtime,placement}/` and `calibraw-ui/src/platform/`.

### 0.2 AI parity harness

`#[ignore]` tests in `calibraw-ai/tests/parity.rs`: for each of the 11 models,
run one fixed input from the Demo photos and compare with a stored golden
output.

- Tolerances per model: IoU for masks, mean/max absolute error for depth and
  denoise, PSNR for LaMa.
- Goldens come from CPU sessions at the current revision and live under
  `data/ai-parity/` with SHA pins.
- Run locally with the models installed; this is the "same output" proof
  for P1–P5.

### 0.3 WebGPU EP spike

Throwaway branch, nothing merged. All 11 models through ONNX Runtime's
WebGPU EP.

| Question | How |
|---|---|
| Operator coverage / CPU fallback nodes | `ORT_LOG_LEVEL=verbose`, count node assignment |
| Numerical parity | 0.2 harness, float32 and float16 |
| Speed | Session create and run time vs CPU and vs DirectML/CUDA on the same machine |
| Peak VRAM | Memory budget before/after session create and run at the largest input |
| Overcommit behaviour | On Windows/D3D12: clean OOM, or paging and TDR? |
| Adapter choice | On a hybrid laptop, does `powerPreference=high-performance` pick the editor's GPU? |
| Packaging | Can `ort 2.0.0-rc.12` with `load-dynamic` register the WebGPU plugin EP library, or must WebGPU be built into the core library? |

Machines: Linux AMD, Intel and NVIDIA; Windows NVIDIA plus iGPU; Apple
Silicon. The maintainer runs these and reports observations.

**Exit:** a written go/no-go per model, and measured VRAM numbers for P4.2.

---

## P1 — Restructure `calibraw-ai` (behaviour-preserving)

**Goal:** one owner per concern; model code only does pre- and
postprocessing.

Target layout:

```
calibraw-ai/src/
  lib.rs            public API only
  config.rs         AiConfig, RuntimeSource, configure()
  runtime/          mod, load (init_from/commit), probe (subprocess), package (downloads)
  placement/        mod, providers, session (FallbackSession), failure, status,
                    job_record, directml (Windows, until P5)
  registry/         mod (ModelSpec table), artifact, install
  slot.rs           one-session slot + warm set (today model_runtime.rs)
  jobs.rs           worker thread, panic capture, runtime init, install, events
  models/           subject, sky, depth, object/, remove/, denoise/
```

### 1.1 Move the runtime out of `ai_masks.rs`

`initialize_runtime`, `probe_runtime_subprocess`, `run_runtime_probe_process`,
`runtime_loaded_in_process` and their statics move to `runtime/load.rs` and
`runtime/probe.rs`; `onnx_runtime_artifact.rs` becomes `runtime/package.rs`.
Update the callers (the probe entry in `calibraw-ui/src/lib.rs`, denoise,
remove, object, tests) with no re-export aliases. Pure move.

### 1.2 Split `execution_provider.rs`

Into `placement/{providers,session,failure,status}.rs`. Move
`gpu_job_record.rs` and `directml_adapter.rs` under `placement/`. Pure move.

### 1.3 Model registry

One `ModelSpec` per `AiModel` replaces `BIREFNET_*`, the SkySeg and depth
`MODEL_SPEC`s, and the LaMa and RawNIND constants:

```rust
pub(crate) struct ModelSpec {
    pub id: AiModel,
    pub feature: AiFeature,
    pub artifact: ModelArtifact,   // url, sha256, bytes, cache filename
    pub input: InputShape,         // fixed edge or tiled
    pub session: SessionProfile,   // CPU tuning, GPU memory (Option<bytes>)
    pub retention: ModelRetention,
}
impl AiModel {
    pub(crate) const fn spec(self) -> &'static ModelSpec;
}
```

Values are copied exactly. A test asserts that every `AiModel` has a spec and
that URLs and SHAs match the previous constants. Download URLs and cache
filenames stay byte-identical because they are on-disk state.

### 1.4 Shared job runner

The thread, panic capture, runtime init and install-with-progress code
duplicated in `ai_masks.rs`, `object.rs`, `remove.rs` (twice) and
`ai_denoise.rs` becomes:

```rust
pub(crate) fn spawn_job<T: Send + 'static>(
    name: &'static str,
    cancel: Arc<AtomicBool>,
    run: impl FnOnce(&mut JobContext) -> Result<T> + Send + 'static,
) -> mpsc::Receiver<AiJobEvent<T>>;
```

The public event enums stay, mapped from `AiJobEvent<T>`, so there is no API
change in this step. Justification: removes duplication.

### 1.5 Typed runtime source (API change, own step)

Replace the `runtime_path` / `runtime_sha256` option pair on every request,
whose mismatched states are rejected only at runtime, with:

```rust
pub enum RuntimeSource {
    Automatic,
    Manual { path: PathBuf, sha256: Sha256 },
}
```

`Sha256` validates lowercase hex once, at construction.

### 1.6 One configuration entry point (API change, own step)

Replace `set_ai_acceleration_enabled`, `set_automatic_onnx_runtime_kind`,
`set_directml_adapter` and `set_gpu_job_record_path` with:

```rust
calibraw_ai::configure(AiConfig {
    acceleration, runtime: RuntimeSource, model_root,
    job_record_path, gpu: Option<GpuIdentity>,
});
calibraw_ai::update(AiConfigChange::Acceleration(bool) | AiConfigChange::Runtime(..));
```

`set_warm_ai_features` stays separate because it changes every frame.

### 1.7 Unify public events (API change, own step)

`AiMaskEvent`, `ObjectMaskEvent`, `RemoveEvent` and `AiDenoiseEvent` become
`AiJobEvent<T>`. UI polling in `app/ai/*`, `app/ai_denoise.rs` and
`app/inpainting.rs` shares one drain helper.

**Exit:** parity harness bit-identical; tests pass;
[ARCHITECTURE.md](ARCHITECTURE.md)'s `calibraw-ai` row and lock table updated.

**As built** (differences from the sketch above):

- `ModelSpec::session` is the existing `SessionOptions` (model name, CPU
  profile, GPU memory); there is no separate `SessionProfile` type yet.
  Download and package membership are a `ModelSource` enum, and the app reads
  model facts through the public `ModelInfo` (`models::sky::MODEL` etc.).
- Events are `AiJobEvent<T, P>`: `P` is each job's own progress type
  (`AiMaskProgress`, `ObjectMaskProgress`, `RemoveProgress`,
  `AiDenoiseProgress`), so no job matches variants it never sends. The old
  event names remain as type aliases.
- The runtime is part of `AiConfig` and `AiConfigChange::Runtime`, not of each
  request. Manual mode without a selected library maps to `Automatic`, as
  the `(None, None)` pair did. `AutomaticOnnxRuntimeKind` has its own
  `AiConfigChange::AutomaticRuntime` until P5 deletes it.
- `AiConfig::gpu` is a minimal `GpuIdentity { vendor_id, device_id }`; P3.3
  extends it. `model_root` places the automatic runtime and RawNIND models.
- Remove and retouch now run on `spawn_job` too, so a panic there finishes the
  job with an error; before, the Remove dialog waited forever.
- OS checks left outside `runtime/` and `placement/` for P2: `config.rs`,
  `models/{masks,subject,depth}.rs`, `models/object/sam.rs`, `slot.rs`.

---

## P2 — OS checks become capabilities (behaviour-preserving)

**Goal:** model code asks what the device can do, never which OS it is. The
flags are computed in one place and reproduce today's per-platform behaviour
exactly.

### 2.1 `AiCapabilities`

Built once in `config.rs`, the only file in this phase with `target_os`:

```rust
pub struct AiCapabilities {
    pub memory: MemoryClass,       // Constrained (Android) | Standard
    pub keep_sessions_warm: bool,  // false: Android, Linux AppImage
    pub float16_sessions: bool,    // today: Windows/Linux, automatic runtime with a GPU provider
}
```

### 2.2 Replace each check (one per step)

| Today | Becomes |
|---|---|
| `depth.rs`: CPU-only on Android | `memory == Constrained` → `SessionProfile::LowMemoryCpu` (renamed from `MobileDepth`) |
| `SubjectMaskQuality::AVAILABLE` | `fn available(caps)`; High requires `Standard` |
| `ai_masks.rs`: crop refinement off on Android | `memory == Constrained` |
| `mask_model_retention` / `cache_object_ai_sessions` | `caps.keep_sessions_warm` |
| `SubjectModelPrecision::for_current_sessions` | `caps.float16_sessions` |
| `cpu_provider`: arena off on Android | from `SessionProfile` |

### 2.3 Quirks file

The SAM encoder's non-finite features on Windows CPU builds with graph
fusion are a genuine platform bug, not a capability. It goes into
`placement/quirks.rs` as a named entry with the observed failure and a
"recheck at ONNX Runtime ≥ X" note. `sam.rs` reads
`quirks::sam_encoder_tolerates_non_finite()`.

### 2.4 Arch-check rule

`target_os` is forbidden in `calibraw-ai/src/{models,registry}/`, `jobs.rs` and
`slot.rs`. This is the same mechanism as the existing `services/` egui rule.

**Exit:** `calibraw-ai` OS cfgs drop from 56 to those in `runtime/`,
`placement/` and `config.rs`. Parity holds on Linux; Android is confirmed by
the maintainer.

**As built** (differences from the sketch above):

- `AiCapabilities` is the crate-private const `config::CAPABILITIES`; nothing
  outside the crate needs it yet. `SubjectMaskQuality::AVAILABLE` stays a
  public const, computed by `const fn available(MemoryClass)`, so the UI is
  unchanged.
- `keep_sessions_warm` is `keep_mask_sessions_warm`: Android only ever
  unloaded mask models (subject, sky, depth, object), not LaMa or RawNIND.
  The AppImage only unloaded SAM, so it is a quirk,
  `quirks::sam_sessions_may_stay_warm`, rather than part of this flag.
- `float16_sessions` is the platform half (Windows, Linux); whether GPU AI is
  on and the automatic runtime has a GPU provider is still asked per job
  through `runtime::ai_sessions_may_use_gpu`, which Android now also has
  (always `true`, as `ai_acceleration_enabled` is there).
- The CPU profiles are `CpuFallbackProfile::{SamEncoder, LowMemory}` (was
  `WindowsSamEncoder`, `MobileDepth`). The arena stays off on every
  `Constrained` session, not only `LowMemory` ones.
- `quirks.rs` also owns the SAM encoder's unfused CPU graph and disabled
  arena, which are the same Windows bug as its non-finite tolerance.
- The provider generation counter moved from `slot.rs` to `placement`, which
  bumps it, so `slot.rs` needs no Android branch.
- `lib.rs` keeps one cfg on the `desktop_model_cache_root` re-export: it is
  public API that does not exist on Android, not a check.

---

## P3 — One GPU context and a memory query (behaviour-preserving)

**Goal:** one place creates GPU devices, and one function answers how much
VRAM is free on the editor's adapter.

### 3.1 Route device creation through `calibraw_gpu::device`

Sites that create their own instance and adapter:

- `calibraw-ui/src/app/processing_export/preview/rebuild.rs`
- `calibraw-ui/src/ui/library/thumbnails/developed.rs`
- `calibraw-ui/src/services/replay/mod.rs`
- `calibraw-gpu/src/pipeline/hdr.rs`
- `calibraw-cli/src/bin/calibraw-develop-export.rs`

`HeadlessDeviceRequest` gains `software_fallback: bool`, so sites without a
fallback today keep that behaviour. One site per step.

### 3.2 Memory query

In `calibraw-gpu/src/memory.rs`:

```rust
pub struct AdapterMemory { pub budget: u64, pub usage: u64 }
pub fn adapter_memory(adapter: &wgpu::Adapter) -> Option<AdapterMemory>;
```

- **Windows:** DXGI `QueryVideoMemoryInfo`, moved from `placement/directml.rs`.
  The `windows` dependency moves from `calibraw-ai` to `calibraw-gpu`.
- **Vulkan (Linux, Android):** `VK_EXT_memory_budget` through wgpu's HAL
  access, when the extension is present.
- **Metal:** `recommendedMaxWorkingSetSize` minus `currentAllocatedSize`.
- **GL or unknown:** `None`.

Every `unsafe` block states its assumptions. Unit tests cover the arithmetic;
the platform calls are confirmed per OS by the maintainer.

### 3.3 `GpuIdentity`

Vendor, device and backend, plus the memory query, captured where startup
calls `set_directml_adapter` today and passed as `AiConfig::gpu`. The DirectML
adapter lookup reads from it.

**Exit:** no behaviour change; diagnostics log the measured budget on every
OS.

**As built** (differences from the sketch above):

- Of the five sites, `developed.rs` and `calibraw-develop-export.rs` already
  used `request_headless_device`. The other three are test devices, as are
  two more that were converted with them (`mask_interaction_tests.rs` and the
  RawNIND test device in `models/denoise.rs`).
- `HeadlessDeviceRequest::adapter` is a `HeadlessAdapter` enum instead of
  `software_fallback: bool`: `Preferred(power)`, `PreferredOrSoftware(power)`
  (the previous behaviour, kept by the thumbnail device, the CLI and the
  parity harness) and `Software`, for the two mask tests that only ever used
  wgpu's fallback adapter.
- Still creating their own adapter: the editor's device (eframe creates it
  with the window surface; it already uses `base_device_limits`),
  `pipeline/gpu/tests.rs` (software adapter first, then hardware), and
  `preview_tests.rs` and `ui_review_tests.rs` (the editor's instance
  descriptor with its memory budget thresholds).
- `adapter_memory` takes the `wgpu::Device`, not the adapter: wgpu-hal 29
  exposes Metal's `MTLDevice` only through a device.
  `AdapterMemory::available()` is budget minus usage, never below 0.
- The query lives in `calibraw-gpu/src/memory/` (`dxgi`, `vulkan`, `metal`),
  a cfg-ratchet allowed directory. Windows uses DXGI whatever backend wgpu
  picked, matching PCI IDs as DirectML did; `find_dxgi_adapter` also returns
  the DXGI index DirectML needs, so one enumeration still serves both.
  Vulkan sums the device-local heaps (AMD splits video memory around its
  256 MiB CPU-visible heap) and needs `VK_EXT_memory_budget` on the device
  and `VK_KHR_get_physical_device_properties2` on the instance, both of which
  wgpu enables wherever the driver has them.
- `ash` and `objc2-metal` are added at the versions wgpu-hal already uses, so
  the lockfile gains no crates; `windows` moved from `calibraw-ai` to
  `calibraw-gpu`.
- `GpuIdentity::new(&wgpu::Device)` keeps the vendor, device and backend and a
  clone of the editor's device for `memory()`; its fields are private.
  Android now passes it too, although no Android provider reads it.
  `configure` records `editor GPU vvvv:dddd (Backend): X MiB of Y MiB budget
  free` (or `free memory not reported`) in diagnostics.
- `cargo test -p calibraw-gpu adapter_memory -- --ignored --nocapture`
  prints what this machine's preferred adapter reports, for checking each OS.

---

## P4 — "GPU if it fits, else CPU" (behaviour change)

### 4.1 Placement as a pure function

```rust
pub(crate) enum Placement {
    Gpu { provider: Provider },
    Cpu { reason: CpuReason },
}
pub(crate) enum CpuReason {
    Disabled,
    NoGpuBackend,
    Quarantined,
    InsufficientMemory { free: u64, needed: u64 },
}
pub(crate) fn place(
    spec: &ModelSpec,
    caps: &AiCapabilities,
    gpu: Option<&AdapterMemory>,
    settings: &AccelerationState,
) -> Placement;
```

Covered by table tests.

### 4.2 VRAM estimate for every model

Peak values measured in P0 plus a 20% margin, each with a comment saying how
and on which hardware it was measured.

### 4.3 Failure handling

| Failure | Today | New |
|---|---|---|
| OOM at session creation | next provider, then CPU | CPU for this job |
| OOM during a run | quarantine GPU AI, abort the job | rebuild on CPU and rerun this job; GPU stays enabled for the next job |
| Device lost | quarantine and restart | unchanged |
| Process died during a GPU job (job record) | GPU AI off at next start | unchanged |

OOM classification stays in `failure.rs` with its tests.

### 4.4 Status

Diagnostics and settings show `GPU · <adapter>` or `CPU · <reason>`, derived
from `Placement` instead of provider strings.

### 4.5 Optional, own step

The desktop editor working-set limit (`gpu_working_set_limit_bytes`, today
`u64::MAX`) uses the same memory query.

**As built** (differences from the sketch above):

- `place(session, state)` takes the model's `SessionOptions` and an
  `AccelerationState { enabled, quarantined, providers }`. Each
  `ProviderOffer` carries the free memory of the adapter its provider runs
  on. `place` needs no `AiCapabilities`, because depth on `Constrained`
  memory is already a CPU-only session (`CpuReason::CpuOnly`).
  `Placement::Gpu` names the first provider that has room or whose free
  memory is unknown. A provider that fails to create a session for a reason
  other than out of memory is dropped and `place` runs again, as the
  provider loop did before.
- Free memory is known only for providers on the editor's adapter:
  - DirectML, when DXGI lists the adapter;
  - CUDA and TensorRT, on an NVIDIA editor GPU;
  - ROCm, on AMD;
  - Core ML, on Metal.

  A provider on another adapter, such as CUDA on a hybrid laptop whose
  editor uses the iGPU, is tried anyway, and 4.3 handles an out-of-memory
  failure. NNAPI and XNNPACK are left to P6.
- `CpuReason` adds three reasons a session can end up on the CPU: `CpuOnly`,
  `OutOfMemory` and `GpuFailed`.
- 4.2: P0.3 has not run, so the numbers come from ONNX Runtime 1.30.0's CUDA
  13 provider on an NVIDIA GeForce RTX 4070 Ti (driver 615.71, Linux). Method:
  - The CUDA arena was switched to grow by exact requests for the run only.
  - Each parity case's GPU memory was sampled through NVML every 5 ms.
  - Each figure is that peak less what the process held before the session's
    weights were loaded: the CUDA context (about 200 MiB), plus wgpu memory
    for Remove and denoise.

  Re-measuring BiRefNet this way reproduced its previous figures.
  `registry` stores the peaks rounded up to 100 MiB, and `place` adds the
  20% margin (`needed_memory`):

  | Model | Measured | Stored | Needed free |
  |---|---|---|---|
  | BiRefNet 1024 / 768 | 2854 / 1758 MiB | 2900 / 1800 MiB | 3480 / 2160 MiB |
  | BiRefNet 1024 / 768 float16 | 1774 / 1066 MiB | 1800 / 1100 MiB | 2160 / 1320 MiB |
  | SkySeg | 574 MiB | 600 MiB | 720 MiB |
  | Depth Anything V2 Small | 404 MiB | 500 MiB | 600 MiB |
  | SAM 2.1 encoder / decoder | 1176 / 102 MiB | 1200 / 200 MiB | 1440 / 240 MiB |
  | Big-LaMa | 590 MiB | 600 MiB | 720 MiB |
  | RawNIND Bayer / linear | 342 / 347 MiB | 400 / 400 MiB | 480 / 480 MiB |

  DirectML and Core ML are assumed to need about as much; P0.3 replaces
  these numbers for WebGPU.
- 4.3: a session that is on the CPU for lack of memory (`InsufficientMemory`
  or `OutOfMemory`) is placed again when the next job acquires it. It is
  replaced only if the GPU now has room. A warm session that fell back after
  `GpuFailed` stays on the CPU, as before.
  - After running out of memory, the model (by model file) stays on the CPU
    until more memory is free than when it was placed, by the margin of its
    peak and at least 256 MiB (`placement::out_of_memory`). Where free memory
    is not reported, it waits instead: 1 minute after the first failure,
    doubling up to 30 minutes. A GPU run or switching GPU AI on again in
    Settings forgets it.
  - Before this, a model whose free memory was unknown, or whose real peak
    exceeded the stored one, went back to the GPU for every job: a new GPU
    session, the failed run, then a CPU rebuild and rerun. Out of memory no longer switches
  GPU AI off, so `take_ai_gpu_failure`/`AiGpuFailure` became
  `take_ai_gpu_device_loss() -> bool`, and the app's out-of-memory dialog is
  gone. Open decision 2 is taken as recommended: the fallback is silent, and
  the status shows it.
- 4.4: `active_execution_providers`/`ExecutionProviderStatus` became
  `ai_session_statuses`/`AiSessionStatus { placement, adapter }`, with
  `label()`:
  - `GPU · <adapter> (<provider>)` when the provider runs on the editor's
    adapter;
  - `GPU · <provider>` otherwise;
  - `CPU · <reason>`.

  Settings warn only for `GpuFailed`, because a memory fallback is not a
  failure. The diagnostics section is now "AI sessions (current)".
- The subject model's precision is still chosen before placement, so a
  float16 model that does not fit runs on the CPU, where it is about 30%
  slower than float32. P5.3 revisits precision per model.
- Checked on Linux with CUDA, with VRAM filled through wgpu:
  - With 2.4 GB free, a Medium subject mask (3480 MiB needed) was placed on
    the CPU, and its next job reused that CPU session. Once the memory was
    freed, the next job moved it to CUDA.
  - With free memory unknown and 1.1 GB free, a run that ran out of memory
    finished on the CPU with GPU AI still on, and the next job tried CUDA
    again.
- Parity: CPU runs are bit-identical. On CUDA every case passes except the
  float16 subject masks: Low reached an IoU of 0.99465 and Medium 0.99245,
  against a minimum of 0.995. Low scored the same at P3, before this
  change.
- 4.5 is not done; it is a separate change to the editor's admission
  control. Until then, when the editor, an export or a thumbnail render runs
  out of GPU memory, GPU AI is switched off only if an AI job was on the GPU
  at that moment (`gpu_job_in_progress`). Otherwise CalibRaw releases
  optional preview textures and an idle AI session on the GPU
  (`release_idle_gpu_session`), and AI work goes on. Before, every editor
  out-of-memory stopped AI work and saved GPU AI as off.
- For the maintainer: no Windows toolchain was available, so the
  Windows-only code (`placement/directml.rs`, the DirectML offer) was checked
  by reading only. Placement on Windows DirectML, macOS Core ML and Android
  still need to be confirmed. On Android, out of memory on NNAPI now reruns
  on the CPU instead of failing the job.

---

## P5 — WebGPU EP on desktop (behaviour change, gated by P0)

### 5.1 Runtime packages

One ONNX Runtime package with the WebGPU EP per OS and architecture,
SHA-pinned on the CalibRaw Hugging Face repository like today. Prebuilt
binaries exist for Windows x64, Linux x64 and macOS arm64. Windows arm64,
Linux arm64 and macOS x86_64 stay CPU until a build exists; the placement
reason says so.

### 5.2 Provider list

`[WebGPU, CPU]` with `powerPreference=high-performance` and
`bufferCacheMode=bucket`. CUDA as an opt-in extra is an open decision.

### 5.3 Precision variants

Precision variants per model in the registry; float16 only where P0
confirmed it.

### 5.4 Deletions

- DirectML, CUDA, TensorRT, ROCm and CoreML providers
- `placement/directml.rs`
- `AutomaticOnnxRuntimeKind` and its setting, and the runtime-kind onboarding step
- per-OS `ort` features in `calibraw-ai/Cargo.toml`

### 5.5 Migrations

- **Settings:** `PerformanceSettings` is `#[serde(default)]`, so removed fields
  read fine; bump `SETTINGS_VERSION` to 5 with no conversion code.
- **App data:** remove the downloaded CUDA runtimes (up to about 440 MB) and the
  DirectML runtime from the model cache through `remove_retired_app_data`,
  marked `migration: remove in v1.5.0` (two minor versions later) and listed
  in `migrations/mod.rs`.

**Exit:** parity within tolerances on WebGPU; the maintainer confirms the P0
machines in the real app.

**As built** (differences from the sketch above):

- 5.1, packaging (the P0.3 question): Microsoft ships the WebGPU EP as a
  plugin library, `onnxruntime-ep-webgpu` 0.4.0 on PyPI, which needs ONNX
  Runtime 1.24.4 or newer. `ort`'s `webgpu` feature links pyke's static
  builds, which `load-dynamic` cannot use, so CalibRaw:
  - registers the plugin with `RegisterExecutionProviderLibrary`;
  - creates sessions on its EP device with `SessionOptionsAppendExecutionProvider_V2`.

  Both need `ort`'s `api-22`, so a manual runtime must now be 1.22 or newer
  (was 1.18).
- 5.1, platforms: plugin builds also exist for Windows arm64 and Linux arm64.
  Only macOS x86_64 stays on the CPU: Microsoft's last build there is 1.23.2,
  and the plugin is arm64 only. Its sessions report
  `CpuReason::NoGpuBuild`.
- 5.1, archives (open decision taken with the maintainer): one archive per
  platform, built byte-reproducibly by
  `packaging/onnxruntime/package_webgpu.py`. Each holds Microsoft's unmodified
  1.30.0 core library and the plugin beside it in `lib/`, with both licenses;
  on Windows the plugin's `dxcompiler.dll` and `dxil.dll` sit beside it too.
  The plugin also imports `msvcp140_atomic_wait.dll`, which the core does
  not. ONNX Runtime loads a plugin with `LOAD_WITH_ALTERED_SEARCH_PATH`,
  which does not search CalibRaw's folder, so the Windows build ships the
  DLL beside `calibraw.exe` with the rest of the Visual C++ runtime, and
  `runtime/load.rs` loads it from there before registering the plugin. The
  script checks that every library the Windows archives import is a Windows
  one, in the archive, or shipped beside `calibraw.exe`.
  The archives are on CalibRaw-Artifacts at `30b40682` like the other
  runtimes, so the consent text naming Hugging Face stays true. Sizes:

  | Platform | Before | Now |
  |---|---|---|
  | Linux x86_64 | 11.3 MB CPU (CUDA: 236–439 MB) | 17.9 MB |
  | Linux arm64 | 10.3 MB CPU | 16.1 MB |
  | macOS arm64 | 42.4 MB | 17.6 MB |
  | Windows x86_64 | 15.8 MB DirectML | 19.8 MB |
  | Windows arm64 | 15.6 MB DirectML | 20.9 MB |

  - Install folders are `<platform>-webgpu`, so the folders of the runtimes
    they replace can be deleted by name.
  - The install manifest gained `webgpu_sha256=` and `webgpu_path=` lines;
    macOS x86_64's three-line manifest still reads.
  - `runtime::load` checks the plugin's hash again and registers it after
    the core library loads. A plugin that fails to register leaves AI on the
    CPU and is recorded in diagnostics.
- 5.1, other runtime sources:
  - A manual runtime uses only the providers built into it. A plugin beside
    it is not loaded, because the SHA-256 the user approved does not cover
    it.
  - Bundled runtimes stay on the CPU: the Flatpak builds ONNX Runtime with
    `onnxruntime_USE_WEBGPU=OFF`. The AppImage downloads like other Linux
    builds.
- 5.2: `GpuProvider` is `WebGpu` on desktop, `Nnapi` and `Xnnpack` on Android
  (both dropped in P6.1).
  - The WebGPU session adds its EP device, then the existing `cpu_provider`
    for the nodes WebGPU leaves, so CPU arena settings and the SAM quirk
    apply as before.
  - Options are `powerPreference=high-performance` and
    `storageBufferCacheMode=bucket`.
  - The plugin picks its adapter itself; the editor's eframe device asks
    wgpu for high performance too. WebGPU counts as running on the editor's
    adapter, so the editor's free memory decides placement, when the EP
    device's PCI IDs match the editor's (or on Metal).
  - `bucket` costs memory: against `disabled`, the SAM encoder peaked at
    2096 instead of 1564 MiB and BiRefNet at 4574 instead of 3722 MiB, for
    runs 35% and 10% faster.
- 5.2, provider bug: WebGPU runs a Split as one shader with a storage buffer
  per output and fails once a node needs more than the adapter's 16 (Dawn's
  cap here). BiRefNet's 50 decoder Splits have 16 or 32 outputs, so every
  BiRefNet run failed and reran on the CPU. `quirks::WebGpuCpuNodes` leaves
  those nodes to the CPU through `forceCpuNodeNames`; BiRefNet 1024 then runs
  in about 1.0 s against 3.7 s on the CPU. No other model has such a node.
- 5.3: float16 is not confirmed on WebGPU, so subject masks use float32
  everywhere; `registry::BIREFNET_FLOAT16_ON_GPU` replaces
  `AiCapabilities::float16_sessions`.
  - Against the float32 CPU goldens, Medium float16 reached an IoU of
    0.98449 and Low 0.99519, against a minimum of 0.995.
  - Float16 saved about 10% of the time.
  - The float16 models stay in the registry and the parity harness. Windows
    and Linux users who had them download the float32 model on their next
    subject mask; the float16 files stay in their cache.
- 4.2 replaced: WebGPU peaks on the same NVIDIA GeForce RTX 4070 Ti (driver
  615.71, Linux, Vulkan), measured as P4 did, with three differences:
  - sessions were set up as `placement` sets them up;
  - runs used fixed inputs of the model's shape;
  - the 15 MiB the process holds once the provider's device exists is taken
    out, as P4 took out the CUDA context.

  | Model | CUDA (P4) | WebGPU | Stored | Needed free |
  |---|---|---|---|---|
  | BiRefNet 1024 / 768 | 2854 / 1758 MiB | 4557 / 3018 MiB | 4600 / 3100 MiB | 5520 / 3720 MiB |
  | BiRefNet 1024 / 768 float16 | 1774 / 1066 MiB | 2886 / 1713 MiB | 2900 / 1800 MiB | 3480 / 2160 MiB |
  | SkySeg | 574 MiB | 650 MiB | 700 MiB | 840 MiB |
  | Depth Anything V2 Small | 404 MiB | 530 MiB | 600 MiB | 720 MiB |
  | SAM 2.1 encoder / decoder | 1176 / 102 MiB | 2088 / 94 MiB | 2100 / 100 MiB | 2520 / 120 MiB |
  | Big-LaMa | 590 MiB | 773 MiB | 800 MiB | 960 MiB |
  | RawNIND Bayer / linear | 342 / 347 MiB | 310 / 290 MiB | 400 / 300 MiB | 480 / 360 MiB |

  Direct3D 12 and Metal are assumed to need about as much.
- Speed on that machine, per run after the first, which compiles shaders:
  - SkySeg 0.05 s (0.19 s on the CPU);
  - Depth 0.04 s;
  - SAM encoder 0.07–0.12 s;
  - Big-LaMa 0.24 s;
  - a RawNIND tile 0.01–0.03 s;
  - BiRefNet as above.

  Sessions took 0.1–5.3 s to create, most of it Big-LaMa's.
- Parity on WebGPU: every float32 case passes.
  - Object, sky and subject Low are bit-identical.
  - Subject Medium and High reach an IoU of 1.00000, at most 3 levels apart.
  - Depth, Bayer denoise and the decoder-only object case differ by at most
    one level or code, X-Trans denoise by at most 1.2e-4.
  - Remove reaches a PSNR of 123.8 dB.
  - CPU runs are bit-identical.
- 4.3, failures: plugin 0.4.0 checks a run for validation errors only. Any
  other WebGPU error, running out of memory among them, goes to ONNX
  Runtime's log alone, as `WebGPU device error(<type>): <message>` with type
  3 for out of memory. The run returns success with whatever its buffers
  hold, or fails on a later validation error that names no memory.
  - CalibRaw therefore creates the environment with its own logger
    (`runtime/logger.rs`). It forwards the log to `log`, records these
    lines in diagnostics and counts them (`failure::LoggedGpuError`).
  - A session created or run while one was logged has failed: on running
    out of memory, or otherwise as a GPU failure. A run's output is then
    dropped, and errors logged after a run returned fail the next one.
  - The plugin's 0.5 branch returns these errors as the run's error.
  - Dawn names lost objects `[Device] is lost.`. `failure.rs` matches these
    and Vulkan's, Direct3D 12's and Metal's own texts in returned errors.
  - They come from the plugin's strings and source; no run here ran out of
    memory or lost its device. A lost device still switches GPU AI off.
- 5.4, deleted beyond the list:
  - `calibraw_gpu::memory::find_dxgi_adapter`'s DXGI index (the lookup is
    private now);
  - the CUDA runtime help in Settings;
  - `packaging/onnxruntime/package_windows_directml.py`.
- 5.5, migrations:
  - `RETIRED_RUNTIME_FILES` (`runtime/package.rs`) lists the CPU, CUDA and
    DirectML install folders and archives. Start-up deletes them, except a
    folder holding the manually selected runtime.
  - `remove_retired_app_data`, which other cleanups due in v1.4.0 also use,
    now carries this one's `v1.5.0` marker.
  - Deleting these migrations is a release-note item, as is the end of CUDA.
- For the maintainer, not checked here:
  - Windows Direct3D 12, including whether Dawn finds `dxcompiler.dll`
    beside the plugin and whether the SAM encoder's CPU quirk still applies
    to Microsoft's 1.30.0 CPU build.
  - macOS arm64 on Metal; Linux on AMD and Intel.
  - Which adapter the plugin picks on a hybrid laptop.
  - Whether the AppImage still needs to unload SAM sessions (open decision
    4).
  - Running out of memory and losing the device on each OS.

---

## P6 — Android AI (behaviour change)

### 6.1 Drop NNAPI

Deprecated since Android 15. Android becomes CPU plus XNNPACK through the
same `place()` function. Small feature and provider change.

### 6.2 Optional

A custom ONNX Runtime Android build with the WebGPU EP on Vulkan, built by a
script under `packaging/` and linked via `ORT_LIB_LOCATION`. Android then
follows P4 with the P3 Vulkan budget, falling back to the RAM-based budget.
Only after P5 has been stable for one release.

**As built** (differences from the sketch above):

- 6.1 was prompted by a Samsung Galaxy S25 Ultra (SM-S938B, Adreno 830,
  Android 16). Its logcat during a subject and a sky mask showed:
  - NNAPI found no vendor driver (`hwservicemanager is not supported`) and
    compiled every partition on `nnapi-reference`, Android's CPU reference
    code, so it never reached the GPU or NPU.
  - BiRefNet failed there (`Conv size is too large, not enough memory`,
    `ANEURALNETWORKS_OP_FAILED`) after a 5.8 s session setup, then was
    rebuilt on the CPU and run again.
  - SkySeg ran there in about 4.2 s.
- Android has no XNNPACK either: pyke's prebuilt Android ONNX Runtime, which
  `ort`'s `download-binaries` fetches, is not built with it, whatever the
  `xnnpack` feature says. `ort-sys` reads the `nnapi` and `xnnpack` features
  only when linking a custom static build, so dropping them changes no
  download.
- Android's GPU provider list is therefore empty. Its built-in runtime
  counts as having no GPU build, so every Android session runs on ONNX
  Runtime's CPU provider and reports `CPU · no GPU build of ONNX Runtime
  for this platform` (depth keeps `CpuOnly`).
  `runtime::ai_sessions_may_use_gpu` is `false` there.
- `GpuProvider` is `WebGpu` alone; the `nnapi` marker left `failure.rs`.
- For the maintainer: time the subject and sky masks on the device against
  the log above (subject 16.3 s, sky 4.9 s, end to end).

---

## P7 — UI platform layer (behaviour-preserving, parallel to P1–P6)

**Goal:** `calibraw-ui` OS cfgs go from about 1,068 to one `platform/` module.

### 7.1 Skeleton

`calibraw-ui/src/platform/{mod,desktop,android}.rs` with:

- `PlatformCaps { form_factor, storage: StorageKind, file_dialogs, can_restart, can_reveal_in_files, .. }`
  for views;
- a `Platform` service for actions: pickers, publishing, clipboard,
  notifications, restart, trash.

The existing `ui/library/platform/` merges into it.

### 7.2 One settings schema

The 9 platform-only `PerformanceSettings` fields exist everywhere, with
platform-dependent defaults. This is additive: old files load because the
struct is `#[serde(default)]`.

### 7.3 Migrate by area (largest first, one area per step)

1. Library: `mod.rs` (74), `actions` (34), `storage` (32), `state` (28),
   `view` (24), `dialogs`, `clipboard`, `catalog`
2. Lifecycle: `eframe_impl` (43), `startup` (41), `settings` (32),
   `documents` (20)
3. Settings and onboarding UI
4. Export, batch and replay
5. Top bar and the rest

Views take `&PlatformCaps` and return actions; handlers call `Platform`.

### 7.4 Tests

A fake `Platform` lets Android-gated tests (for example
`ui/library/tests.rs`, 50 cfgs) run on Linux CI.

### 7.5 Close the ratchet

Each finished area leaves the baseline. Final arch-check rule: `target_os` in
`calibraw-ui` only under `src/platform/`.

**As built** (differences from the sketch above):

- `calibraw-ui` went from 1,029 `target_os` conditions in 111 files to none
  outside `src/platform/` (39 lines there); the workspace baseline fell from
  1,150 to 71. All five areas were migrated in one change, because the
  shared state types (`AndroidState`, `LibraryState`, the export jobs) could
  not compile on every target one area at a time.
- `PlatformCaps` has `form_factor` (`Desktop`/`Mobile`), `storage`
  (`Folders`/`MediaLibrary`), `memory` (`Standard`/`Constrained`),
  `tasks_need_foreground`, `configurable_ai_runtime`, `video_encoder`
  (`Ffmpeg`/`Platform`) and `can_restart`. `file_dialogs` and
  `can_reveal_in_files` were not needed: native dialogs come with
  `StorageKind::Folders`, and nothing reveals files yet. `DESKTOP` and
  `ANDROID` are constants; `CURRENT` is the one cfg-selected value, for leaf
  widgets and worker-pool sizes that have no app.
- `Platform` is a trait object (`Arc<dyn Platform>`) in `CalibRawApp` and
  `LibraryState`, with default methods that report "unavailable" or do
  nothing. `AndroidPlatform` wraps every `calibraw-ffi` call;
  `DesktopPlatform` supplies the settings path, model and cache folders and
  the GPU-loss restart command (moved from `app/gpu_recovery.rs`). Native
  dialogs (`FileDialog`), trash, the fatal-error dialog and camera-profile
  search folders are free functions, since they keep no state.
  `copy_text_to_clipboard` returns `None` where egui's clipboard reaches the
  system one.
- Both entry points moved in: `android_main` to `platform/android.rs` and
  the desktop `main` (with the ONNX Runtime probe CLI) to
  `platform/desktop/entry.rs`. `desktop_portal.rs` became
  `platform/desktop/portal.rs`; Discord IPC moved to `platform/discord.rs`
  and `platform/desktop/discord.rs`.
- `ui/library/platform/` did not merge into `src/platform/`: its contents are
  library views and workers over two kinds of storage, not OS calls. It
  became `ui/library/folders/` and `ui/library/media_library/`, with
  `ui/library/sources.rs` dispatching on `PlatformCaps::storage`; their OS
  calls go through `Platform`.
- Data that names its kind is matched instead of a capability:
  `LibraryLocator`, `SidecarTarget::Android` (now on every target),
  `ExportDestination`, and the new `LibraryJobPhoto` and
  `LibraryBatchExportJob` enums for library AI refresh and batch export.
- 7.2: every `PerformanceSettings` field exists and is saved on every
  target, so each platform's file now also carries the other's fields at
  their defaults. Old files still load (`#[serde(default)]`). The
  defaults that differ come from `PlatformCaps::CURRENT`.
- `calibraw-core` APIs for folders on disk (desktop sidecars and developed
  thumbnails, `raw_companion_paths`, `display_metadata_cache`, the desktop
  thumbnail cache) compile on Android too; only the UI's folder branch
  calls them. `calibraw-ai`'s built-in Android runtime answers the
  automatic-runtime queries, so the AI settings code compiles everywhere.
- Onboarding drops the camera-profile and Discord steps where
  `StorageKind::MediaLibrary` (Android imports profiles in Settings).
- arch-check also forbids `calibraw_ffi`, `rfd`, `zbus`, `env_logger` and
  `android_logger` outside `src/platform/`; `SourceRule` gained `excluded`
  directories for it. `src/platform` is a cfg-ratchet allowed directory.
- 7.4: `platform::fake::FakePlatform` takes any `PlatformCaps` and scripted
  media-library folders and documents and records the services called. The
  Android-only tests of `ui/library/tests.rs` (media-library folder paths
  and assets) now run on Linux, the back-navigation test runs with Android
  capabilities, and new tests cover a media-library scan, opening a library
  photo through the platform and the mobile onboarding steps.
- Checked: host workspace clippy and tests; `cargo ndk` clippy of
  `calibraw-ui` for arm64-v8a. Windows and macOS code was read but not
  compiled here.

---

## Checks for every step

- The six [AGENTS.md verification](../AGENTS.md#verification) commands.
- Parity harness (0.2) for every step touching `calibraw-ai`.
- The cfg ratchet does not rise.
- [ARCHITECTURE.md](ARCHITECTURE.md) is updated in the step that changes a
  boundary.
- Real-app or device checks are listed for the maintainer to run.

## Open decisions

Recommended answer first.

1. **CUDA:** drop it and use WebGPU only, or keep it as an opt-in for NVIDIA?
   Taken in P5: dropped.
2. **CPU fallback notice:** silent with the status line, or a one-time notice
   per session? A fallback is not a failure, so a silent fallback is
   recommended.
3. **Android GPU AI:** 6.1 only, or plan for 6.2?
4. **AppImage disables warm sessions** (introduced in `837ab43d`, "Try fix
   pixelated"): the reason is unknown. It is the quirks entry
   `sam_sessions_may_stay_warm` since P2; recheck it with the P5 runtime.

## References

- [ONNX Runtime WebGPU Execution Provider](https://onnxruntime.ai/docs/execution-providers/WebGPU-ExecutionProvider.html):
  native via Dawn on D3D12, Vulkan and Metal; plugin EP library; device,
  power-preference and buffer-cache options.
- [onnxruntime releases](https://github.com/microsoft/onnxruntime/releases) and
  [standalone Dawn PR #33006](https://github.com/microsoft/onnxruntime/pull/33006).
- `ort 2.0.0-rc.12` has a `webgpu` feature (`src/ep/webgpu.rs`); `ort-sys`
  ships prebuilt WebGPU binaries for `x86_64-pc-windows-msvc`,
  `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin`.
- [DirectML Execution Provider](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)
  and [Microsoft Learn: DirectML](https://learn.microsoft.com/windows/ai/directml):
  sustained engineering, new work in Windows ML.
- [NNAPI Migration Guide](https://developer.android.com/ndk/guides/neuralnetworks/migration-guide):
  NNAPI deprecated from Android 15.
- [Importing ONNX Models in Burn](https://burn.dev/burn-book/import/onnx-model.html):
  pure-Rust wgpu alternative; limited operator coverage, so not planned now,
  but worth a later spike for the small UNets (SkySeg, RawNIND).
