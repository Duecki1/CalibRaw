# GPU and platform rework

Plan for making local AI and platform handling uniform: one GPU path for
every vendor, one rule for GPU vs CPU, and platform differences decided once
instead of scattered through the code. Status: P0.1 (cfg ratchet) and P0.2
(parity harness) implemented; P0.3 (WebGPU spike) and P1–P7 planned.

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
2. **CPU fallback notice:** silent with the status line, or a one-time notice
   per session? A fallback is not a failure, so a silent fallback is
   recommended.
3. **Android GPU AI:** 6.1 only, or plan for 6.2?
4. **AppImage disables warm sessions** (introduced in `837ab43d`, "Try fix
   pixelated"): the reason is unknown. If it was a runtime loading issue, P5
   may remove it; otherwise it becomes a quirks entry.

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
