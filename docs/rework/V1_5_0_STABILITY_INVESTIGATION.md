**CalibRaw 1.5.0 stability investigation and repair plan**

Investigated on 2026-11-17. The evidence below records the withdrawn release.
The subsequent Samsung/Mali export capture, PowerVR zoom report and heavy-pass
audit have a separate [workload stability repair plan](GPU_WORKLOAD_STABILITY_PLAN.md).
The following repairs have since been implemented in the working tree, but
have not been released or verified on the reporting devices:

- GPU-loss recovery saves first and exits the old render process without
  lost-swapchain teardown. Android's independent recovery Activity owns the
  notice and optional restart, with no native-thread dialog wait.
- Descriptor exports carry an open file into the worker instead of reopening
  `/proc/self/fd/N`.
- A winit 0.30.13 backport handles Destroy and event-loop recreation; Android
  disables eframe's TLS loop cache so destruction precedes the native handshake.
- All processing pipeline compilations record a flushed begin event and a
  matching return ID. An unfinished compile is identified in the next report.
- Android native libraries receive Build IDs and release packages retain native
  function symbols for Play crash reports. CI archives them separately and
  excludes that diagnostics archive from public application downloads.
- Startup now warms the default preview/export passes only. The unconditional
  common-program sweep and unused demosaic-variant prewarm were removed;
  optional glow, defocus and other effect programs compile when rendered.
  Saved edits and edited thumbnails can require them too. This contains the
  optional-effect startup exposure; it does not fix a native compiler defect.

These repairs do not establish the missing shader/driver causes described below.

The evidence establishes multiple independent failures. The strongest new
finding is a deadlock in Android's GPU-loss recovery introduced between 1.4.1
and 1.5.0. The supplied 1.5.0 Play Console ANR captures both sides of that
deadlock. Separate 1.4.1 reports establish crashes inside Qualcomm's shader
compiler and aborts during Rust unwinding. Windows also has a confirmed FXC
compilation failure. The Windows RX 480 startup crash is narrowed to a startup
phase, but its exact native failure is still unproved.

The initial GPU failure and the failure to recover from it must be tracked
separately. Fixing the Android deadlock will prevent that ANR; it will not,
by itself, prevent the GPU loss that preceded the dialog.

**Scope and release identity.** The released tag `v1.5.0` resolves to
`0a990dd1869759e357f5d8cd6d1b2adb7d397188`. Its Git commit count is 1322,
matching the supplied Play version code `1001322`. `v1.4.1` resolves to
`0dd9719bdc090ccfc66fa61ca86c3c32866fde4b`. The supplied older Play stacks
identify version `1001245 (1.4.1)` and Build ID
`f4d67fdcb0bf57c005fc0428460b16e4`; they must not be symbolicated against an
arbitrary local build. The main-branch commit `2d143b4a` has count 1245, but
matching a version code alone does not establish binary identity.

The working checkout starts at `a00b02a9`, two commits after the release:

| Commit after the tag | What it changes | Limit of what it fixes |
| --- | --- | --- |
| `1ede51e8` | Bundles `dxcompiler.dll` in Windows packages | Addresses the DX12/FXC route; cannot fix #192's Vulkan route |
| `a00b02a9` | Caps Android export tile sizing using available RAM and wraps application GPU polls after device loss | Not present in the withdrawn release; does not fix the Java dialog deadlock or native compiler crashes |

The untracked `crates/calibraw-gpu/src/memory/policy.rs` present at the start
was left untouched and is not treated as released code. `wgpu`/Naga remain
29.0.4, egui/eframe remain 0.35.0, and winit remains 0.30.13 across the
1.4.1-to-1.5.0 lockfile comparison. A wgpu version upgrade is therefore not
the change that explains the increase in failures.

| Evidence | Established finding | Still missing |
| --- | --- | --- |
| Owner's 1.5.0 Play ANR, Infinix NOTE 30 Pro, Android 14, version 1001322 | Native event thread waits for the GPU-loss dialog while Java's main thread waits for that native thread to acknowledge Pause | The diagnostic events preceding the initial GPU loss |
| Owner's 1.4.1 Play Redmi stacks | Native Qualcomm compiler crashes in `vkCreateComputePipelines`, reached through `ComputeProgram::compile` during RAW-stage constructor warmup | Exact entry point, variant, generated shader, driver identification, and matching symbols |
| Owner's 1.4.1 Play abort stacks, including Galaxy Tab S9+ | `panic_in_cleanup`: a second panic during unwinding aborts the process; one stack goes through destruction of egui's GPU painter | The first and second panic messages |
| Owner's 1.4.1 Lenovo Idea Tab Pro ANR | Java waits for native destruction while the native thread remains in the event loop; pinned winit ignores Destroy | Device validation of the lifecycle repair |
| [#181](https://github.com/Duecki1/CalibRaw/issues/181), [#185](https://github.com/Duecki1/CalibRaw/issues/185) | Windows DX12 uses FXC and shader compilation fails; the reporter's Vulkan workaround succeeds | Packaged DXC verification on both Windows architectures |
| [#186](https://github.com/Duecki1/CalibRaw/issues/186) | JPEG export fails opening `/proc/self/fd/195` with EACCES | Validation of descriptor-based writing on the affected Android provider |
| [#192](https://github.com/Duecki1/CalibRaw/issues/192#issuecomment-6948606186) | RX 480/Vulkan startup terminates after export prewarm, before common-program completion is logged, on two runs | Native exception/dump and per-program begin/end records |
| [#184](https://github.com/Duecki1/CalibRaw/issues/184#issuecomment-6909590022) | Mali-G77 export resets even with AI GPU acceleration disabled, including a development build before 1.5.0 | A log from the export itself and the device-loss message |
| [#191](https://github.com/Duecki1/CalibRaw/issues/191) | Reported Xclipse 960 loss during onboarding before photo import | Actual diagnostics or native fault data; no screenshot/log is attached |
| [#176](https://github.com/Duecki1/CalibRaw/issues/176) | Intel/Linux system freeze is reported reproducibly with two other editors open and CalibRaw zooming | Kernel GPU hang/error-state data; no causal GPU fault is captured |

**The 1.5.0 Android recovery deadlock is confirmed.** The owner's attached
report identifies one affected user and three events on version 1001322.
The decisive thread pair is:

```text
Java main:
  NativeActivity.onPauseNative
  NativeActivity.onPause

android_main:
  CountDownLatch.await
  CalibRawActivity.showFatalMessage
```

The corresponding application path is
`CalibRawApp::handle_gpu_device_loss` in
`crates/calibraw-ui/src/app/gpu_recovery.rs`, then
`AndroidPlatform::show_fatal_message`, the JNI bridge in
`crates/calibraw-ffi/src/android.rs`, and
`CalibRawActivity.showFatalMessage` in
`android/app/src/main/java/de/duecki/calibraw/CalibRawActivity.java:487`.

`showFatalMessage` schedules an `AlertDialog` on Java's main thread, then
blocks the calling `android_main` on `dismissed.await()` at line 508.
Android's Pause callback sends a native lifecycle command and waits for that
same native event thread to acknowledge it. That thread cannot poll lifecycle
events while waiting for the dialog. Java's main thread cannot process the
dialog's dismissal while blocked in Pause. This is a circular wait, not a
slow shader. The report does not establish what initiated Pause.

The latch release in `onDestroy` does not help a thread already blocked in
Pause. Merely checking that the caller is not Java's main thread is also
insufficient: the native event thread participates in synchronous Android
lifecycle handshakes. Android documents this main-thread blocking mechanism
as a cause of [ANRs](https://developer.android.com/topic/performance/issues/anr).

`git log -S showFatalMessage` identifies the introducing commit as
[`3b4fafde`](https://github.com/Duecki1/CalibRaw/commit/3b4fafde4f20eb38e89a50dc0c6116663b41f85c),
“Wgpu support for android,” included in 1.5.0 and absent from the 1.4.1 tag.
This provides a concrete explanation for a new Android failure after an
otherwise recoverable GPU loss.

The repair should transfer the notice and restart decision to an independent
Android recovery activity without waiting for user input on `android_main`.
The existing `GpuLossRestartActivity`, already in its own process, provides
an appropriate owner. Persist edits and diagnostics first, transfer the
notice and restart allowance, then let that activity own ending the failed
process and showing the notice. Preserve the existing single-restart guard
and display any sidecar-save failure. A second loss must offer a terminal
notice rather than restart indefinitely. Returning immediately from the
current dialog method alone is incomplete: its caller currently proceeds
straight to restart, so that would dismiss or destroy the notice immediately.

**The Redmi crashes happen during native compilation, before this graph is
executed.** Multiple supplied 1.4.1 stacks contain this path:

```text
vendor/libllvm-qgl.so
vendor/hw/vulkan.adreno.so: vkCreateComputePipelines
wgpu_hal: create_compute_pipeline
calibraw_gpu::ComputeProgram::compile
RawGpuPipeline::encode_bound_pass
RawGpuPipeline::encode_raw_stage
RawGpuPipeline::new_internal
```

The callers include export construction for more than one format and
`install_prepared_preview_detail` on the application event thread. In the
1.4.1 constructor, RAW/Tone/Output passes are encoded into a warmup encoder
that is discarded. Encoding forces lazy programs to compile; that warmup
does not execute the graph. Therefore changing JPEG quality, reducing GPU
dispatch duration, or changing an AI execution provider does not address
this observed failure location. A Rust `catch_unwind` or wgpu error scope
cannot recover from a crash inside the vendor's native compiler.

These stacks establish the failing subsystem and application call path.
They do not identify the individual RAW shader, demonstrate invalid shader
input, or distinguish a driver/compiler defect from a compiler resource
failure. RAW-stage encoding can select raster finish, binned finish,
highlight reconstruction, demosaic passes, CA, and colour denoise. Naming
one of those without a label or matching binary would be a guess.

The next diagnostic build needs a durable record immediately before and
after each driver compilation, including entry point, shader identity,
work format, sensor layout, override constants/effect set, thread/job,
cache state, and driver information. Include the application revision and
binary Build ID in the same record. Export the composed WGSL and generated
SPIR-V for the failing program in a diagnostic mode, and validate the
SPIR-V. Then reduce that program while retaining the crash on the affected
driver. Compare a loaded cache, an empty cache, and no pipeline cache as
controlled experiments; cache corruption has not been established here.
Do the same for serial versus overlapping compilation before attributing
the failure to compiler concurrency.

For a confirmed shader/compiler incompatibility, change the smallest
offending shader construct or decomposition and verify Final output parity.
If a device-specific workaround is necessary, key it to evidence about the
driver and program. Globally disabling precision, all effects, or GPU AI
would conceal unrelated behavior and would not constitute this diagnosis.

**The Windows RX 480 startup failure is localized, not fully diagnosed.**
The reporter supplies revision `0a990dd1`, Vulkan, RX 480, and two sessions
with the same last completed phase:

| Startup event | Previous run | Current run |
| --- | --- | --- |
| Preview prewarm finished | 2.001 s | 2.004 s |
| Export prewarm finished | 3.414 s | 3.486 s |
| Common programs finished | Not recorded | Not recorded |

The reported termination is after approximately four to five seconds.
`spawn_gpu_preview_prewarm` in
`crates/calibraw-ui/src/app/lifecycle/mod.rs:84` proceeds from export
prewarm to `RawGpuProgramTemplate::compile_common_programs`. That function
in `crates/calibraw-gpu/src/pipeline/gpu.rs:433` compiles every dispatchable
program except the two explicitly disabled weather passes, then a common
demosaic variant, even with no photo or effects enabled.

1.5.0 adds eight defocus passes and nine styled-glow passes to that graph,
along with changes to existing shaders. The common-program loop reaches
those additions. Startup prewarming itself already existed in 1.4.1; the
new compilation exposure is the relevant difference. A failure inside that
phase is consistent with the report. The current journal cannot prove the
exact entry point or even the exact native call because it records successful
compiles only at debug level, after they return. The absence of a recorded
Rust panic or device loss must not be converted into proof of a driver crash.

There was approximately 7 GiB of reported free GPU budget at startup. That
does not prove compilation could not exhaust a different resource, but it
does not support describing this report as ordinary photo VRAM exhaustion.
The DXC packaging fix cannot affect this Vulkan-selected run.

Use the per-compilation records above and a Windows native exception dump to
identify the stopping point. First compare startup with optional common
prewarming suppressed and with it enabled. If suppression prevents the
crash, enable the recorded programs incrementally to locate the failing
program. Suppression is containment: that program might still crash when
the associated tool is first used. Settings and diagnostic access should
remain reachable before optional photo-processing compilation starts.

**Windows FXC failures have a different established cause.** #181 reports
DX12 on an RTX 5060 Ti, with FXC errors in `apply_view_node` and
`highlight_reconstruct`. #185 confirms that forcing Vulkan allows editing.
The release's Windows packaging does not stage DXC. The pinned wgpu DX12
implementation tries the dynamic compiler in Automatic mode and falls back
to FXC when it cannot load it; see the
[29.0.4 compiler selection](https://github.com/gfx-rs/wgpu/blob/v29.0.4/wgpu-hal/src/dx12/instance.rs).

The checkout's `1ede51e8` packaging commit is the correct targeted remedy
for this route. Verify the packaged x86_64 and ARM64 binaries, rather than
the build machine's compiler installation: run outside MSYS/developer PATH,
log the selected compiler, open a raster and a RAW, and compile the used
preview/export programs. Test both installer and portable outputs. A
backend selector is useful recovery access, but does not repair a broken
default compiler installation. Do not switch every Windows user to Vulkan
to address the RX 480 report or assume this validation error is a TDR.

**The Android export descriptor abstraction is wrong for #186.**
`ExportPublisher.createPendingExport` obtains a MediaStore descriptor through
`ContentResolver.openFileDescriptor`. `TransferredFileDescriptor` owns the
transferred `File`, but exposes only `/proc/self/fd/N` as the export target.
`open_export_destination` in
`crates/calibraw-gpu/src/pipeline/export/destination.rs:168` then opens that
path again. The report's failure is exactly at this second open.

An existing descriptor carries authority to access its open file; reopening
its proc path performs permission checks again. This distinction is described
by the [Linux proc descriptor documentation](https://man7.org/linux/man-pages/man5/proc_pid_fd.5.html)
and is already recognized by this repository's `StorageManager.decodersCanRead`
fallback for imported files. A local Linux experiment opened a file, removed
its path permissions, then attempted both operations. Reopen returned
`PermissionError(13)`; duplicating the original descriptor, truncating it and
writing succeeded. This reproduces the abstraction failure, not the affected
phone's exact MediaStore implementation.

Pass an owned or duplicated `std::fs::File` into the export job and write to
it directly. Truncate/seek the duplicate as needed; `try_clone`/dup shares
the file offset, so keep the pending descriptor idle and give one writer
ownership for the job. Preserve staging for intermediates, finalize the
MediaStore item only after writing finishes, and remove pending items on
error/cancellation. Formats requiring seek should use a seekable descriptor
or encode to an application-private staging file and copy into the original
descriptor. Do not request broad storage permissions to compensate for a
second open the app does not need.

**The older Android destruction ANR has a separate lifecycle defect.**
The Lenovo 1.4.1 report shows Java in `NativeActivity.onDestroy`, waiting in
`android_activity::...::on_destroy`, while `android_main` is polling
`ALooper_pollOnce`. The pinned `android-activity` 0.6.1 implementation sends
Destroy and waits for the native thread's state to become Stopped. The pinned
[winit 0.30.13 Android implementation](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/android/mod.rs#L289)
handles Destroy with a TODO and keeps running. That combination explains the
observed wait even when the GPU is idle. It predates the 1.5.0 fatal dialog.

Use a narrow upstream fix/backport that exits the native event loop on
Destroy and allows the activity shutdown handshake to finish. Also resolve
recreation: winit has a process-wide one-event-loop guard and eframe's
cached loop belongs to its original native thread. Simply making Destroy
exit does not make creation of a replacement activity in the same surviving
process valid. Define and verify either supported reattachment/recreation
or a saved-state, controlled fresh-process lifecycle. Do not bypass
`super.onDestroy` or add arbitrary timeouts to Android's native handshake.
The S25 1.4.1 Pause report does not contain an expanded native-thread stack,
so its particular blocking operation remains unidentified.

**The panic-in-cleanup reports prove recovery is not complete.** The 1.4.1
abort stacks contain `core::panicking::panic_in_cleanup`; the Tab S9+ stack
also identifies `drop_in_place<egui_wgpu::winit::Painter>`. These are process
aborts caused by a second panic during unwinding. A generic outer
`catch_unwind` cannot rescue an abort in a destructor.

There is a concrete matching hazard in pinned wgpu 29.0.4: surface
`present`/`discard` check device validity before taking the acquired
texture. After loss, the early return can retain that texture. Surface
destruction unconfigures the swapchain, whose
[release code](https://github.com/gfx-rs/wgpu/blob/v29.0.4/wgpu-hal/src/vulkan/swapchain/native.rs#L340)
expects exclusive ownership of its semaphores and can panic if a surface
texture still holds them. This is a code-supported candidate for the
cleanup panic, not proof of the exact Tab S9+ panic without its messages.
Capture the first panic and the cleanup panic before selecting a backport.

The release also directly calls `Device::poll` from application waits and
the preview dispatch gate. The pinned wgpu front end fatally panics for
poll failures other than its public timeout case. The post-tag
`a00b02a9` wrapper handles that path after the loss callback has fired.
It does not cover egui's uploads, swapchain acquisition/presentation,
surface teardown, or native driver faults.

Make loss a terminal state for the old render session: stop accepting and
submitting GPU work, cancel workers, persist edits, and transfer recovery
without another attempt to paint on that device. Audit the painter and
surface lifecycle as well as application polls. Restrict unwind recovery
to established device-loss paths and propagate unrelated programming
panics. No report here justifies swallowing every GPU panic.

**Memory pressure is a confirmed admission-policy gap, not a proved cause
of every reset.** On desktop, `gpu_admission_limit_bytes` and
`gpu_working_set_limit_bytes` return `u64::MAX`; the native memory thresholds
are also disabled by default. DXGI memory is queried for AI placement and
the optional full-resolution preview, but does not admit every graph and
effect allocation. Full-resolution admission estimates the baseline preview
graph, not all the optional defocus/glow/scene resources that the edit may
later allocate. Other editors can reduce the available budget after a
check; #176's concurrency with other editors is consistent with pressure,
but its kernel error is not proof of that cause.

Android 1.5.0 changed resident admission from the RAM/8 working-set budget
to RAM/4, capped at 3 GiB. Tile/preview sizing still uses RAM/8, capped at
1.5 GiB. Export intermediates have a separate total-RAM/8 CPU allowance;
row bands are another allocation, and 1.5.0 also retains the RAW-stage
canvas where its intermediate allowance permits. None of these independent
allowances is a complete current free-memory budget. For example, with
`total_memory_bytes = 8 GiB`, resident GPU admission can allow 2 GiB and
export intermediates another 1 GiB before decoded RAWs, CPU row bands,
compiler memory and the rest of the application are counted. These are
allowances, not measured resident usage for an affected phone.

The Mali-G77 diagnostic explicitly reports that free GPU memory is
unavailable. The tagged export tile planner uses installed RAM without
current available RAM. `a00b02a9` improves tile sizing, but still floors
the tile budget at 168 MiB; `device_export_core_edge` does not choose a
core smaller than its default. `IntermediateMemory` still uses installed
RAM. A partial tile cap is therefore not evidence of a complete solution.

Use current per-process GPU headroom where the backend provides it, current
system memory for unified-memory devices, reservations for overlapping jobs,
and estimates for the complete active graph and host buffers. Trim optional
previews/caches first, then reduce a tile where supported or fail cleanly
before submitting work. Keep a conservative policy when budgets are unknown.
Do not blindly restore a percentage check over every Vulkan memory heap:
heap reporting is not equivalent to DXGI's per-process budget. Microsoft
documents that applications should target the
[DXGI budget](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/ns-dxgi1_4-dxgi_query_video_memory_info).

Also correct the diagnostic wording. CalibRaw calls every unexpected device
loss a graphics-driver reset. Pinned wgpu can invalidate a device for
memory pressure through `lose_if_oom`, with the same application callback;
see its [device error handling](https://github.com/gfx-rs/wgpu/blob/v29.0.4/wgpu-core/src/device/resource.rs#L689).
The popup alone does not establish a physical reset or a watchdog timeout.

**Execution time must be measured separately from compilation and memory.**
Windows TDR's default two-second interval concerns a task that cannot finish
or be preempted; it is not a blanket two-second limit on an entire export.
See [Microsoft's TDR description](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/timeout-detection-and-recovery).
1.5.0 already introduces `WorkEncoder`: a 100 ms target, row bands for
recognized passes, small initial probes, and bounded per-encoder submissions.
Recommending “split the GPU work” without auditing that implementation would
miss work already in the release.

Actual remaining risks include untracked scene dispatches, a single band
that cannot be subdivided below one row, several producers sharing the GPU,
and edits whose cost changes sharply. Completion callbacks measure an upper
bound including delayed polling/queue work, not a precise kernel duration.
Use stage timestamps where supported and submission records to establish
which work was outstanding at loss. Subdivide a measured problematic pass
without changing its per-pixel arithmetic. Changing tile layout can change
reduction rounding, so verify parity and preserve reduction order where
needed. Changing Windows TDR registry settings is not a product fix.

**Implementation order and acceptance conditions.** Keep these as separate,
reviewable changes; the diagnostic work can proceed alongside the first two
fixes. Nothing below requires a new persisted photo/edit layout.

| Order | Concrete change and owner | Acceptance condition |
| --- | --- | --- |
| P0.1 | Replace the blocking Android fatal-dialog/restart handshake; application persistence remains in `app/gpu_recovery.rs`, lifecycle UI/process transfer belongs to the Android platform/FFI/Java bridge | Trigger loss while onboarding, editing and exporting; background, lock, rotate and dismiss the notice; no latch/Pause cycle, edits preserved or save failure shown, at most one automatic restart |
| P0.2 | Pass a real owned/duplicated file through descriptor export targets, across FFI/UI/export ownership boundaries | MediaStore JPEG, PNG, TIFF and JXL either complete and publish valid files or fail and remove the pending item; cancellation does not publish partial output; no proc-path reopen |
| P0.3 | Include and verify the existing DXC packaging change in a new Windows candidate | Clean x86_64 and ARM64 packaged runs log DXC and compile RAW/raster preview/export paths without relying on developer PATH |
| P0.4 | Add durable compiler begin/end records, symbol artifacts and native exit diagnostics | An interrupted compile can be named on next launch; native reports map to the exact shipped binary; distinguish native crash, Rust panic, GPU loss, OOM and lifecycle ANR |
| P1.1 | Diagnose the RX 480 common-prewarm failure and the Redmi RAW-stage compiler failures using those records; preserve shader/program artifacts | Identify an exact program/variant and reproduce its failure on the affected driver, then verify the smallest repair; do not close the investigation on “warmup disabled” alone |
| P1.2 | Stop optional common prewarming before onboarding/settings are usable; perform necessary compilation off the event thread and make compilation failures explicit | Cold and existing-cache startup remain usable; a disabled optional effect is not compiled merely to complete onboarding; settings/diagnostics remain accessible; compile failures are not cached as usable pipelines |
| P1.3 | Complete device-loss handling through application jobs, egui painting and surface teardown; verify or backport the relevant dependency repair | Forced loss at upload, submit, acquire, present, wait and close reaches recovery without a second cleanup panic or further GPU submissions |
| P1.4 | Fix Android Destroy handling and its activity-recreation contract in the pinned dependency/platform integration | Destroy/relaunch, configuration changes, resize/multiwindow, language/font changes, background and screen-off cycles finish lifecycle handshakes and reopen correctly |
| P2.1 | Add budget-aware admission for full active desktop/mobile workloads, including optional effects and host export allocations | Low-headroom runs trim optional work or return a recoverable error before allocation/submission; no native kill/reset is used as an admission signal |
| P2.2 | Audit measured expensive passes and queue producers under the existing work scheduler | The failing workload has bounded submissions and acceptable UI/lifecycle latency on the affected driver; Final render parity is preserved |
| P3 | Validate packaged candidates with original reporters and staged store/internal distribution | Each failure signature is checked independently; record new build revision/version code, device, backend/compiler and cold/warm cache results |

For compilation, logging only after success is insufficient. Record before
entering the driver, preserve the unfinished record if the process dies,
and keep separate concurrent records rather than one overwritten global
“last shader.” Keep normal logs small; full shader dumps and expensive
driver validation belong to an explicit diagnostic build/mode. A native
crash cannot be converted into a Rust error by adding more unwind guards.

For symbols, preserve the unstripped Rust/native binaries and their Build
IDs with each release. Supply Play with the matching native symbols;
[Android's symbol configuration](https://developer.android.com/build/include-native-symbols)
supports function names or full file/line information. The withdrawn release's
Windows/Android workflows did not explicitly archive matching product debug
symbols; the working-tree Android workflow now does. A downloaded GitHub APK
for commit `2d143b4a` was stripped and did
not expose the Play report's Build ID, so it was not used to guess source
lines. Add Windows native exception dumps; D3D12 diagnostic builds can use
[DRED](https://learn.microsoft.com/en-us/windows/win32/direct3d12/use-dred)
for GPU breadcrumbs/page faults. DRED does not diagnose a Vulkan-selected
run. On Android 12+, collect the previous process's exit reason and native
tombstone where available, as described in the
[NDK debugging guide](https://developer.android.com/ndk/guides/debug).

**Device and workload validation.** Use the actual reporting driver families.
A fast Linux NVIDIA success does not qualify a Windows AMD, Mali, Xclipse
or older Adreno repair.

| Target | Required workload |
| --- | --- |
| Windows RX 480, reported Vulkan driver | Cold and existing-cache startup; no file open; optional common prewarm isolation; then activate the identified effect/program |
| Windows NVIDIA, AMD and Intel; DX12/DXC and Vulkan | Packaged startup, RAW/raster import, fitted/zoomed edits, export, AI on/off, repeated open/close, deliberate low headroom |
| Redmi Note 14 Pro+ / reporting Qualcomm driver | RAW-stage constructor compilation for export and zoom; exact failing shader/variant; cold/warm/no-cache comparison |
| Infinix NOTE 30 Pro / reporting Android 14 environment | Forced device loss followed by Home, lock, rotation, notice dismissal and relaunch; confirm the original ANR's thread cycle is gone |
| Xiaomi 11T / Mali-G77 r32p1 | Unedited RAW export and edited export in several formats, AI GPU off, full and reduced output sizes; capture loss diagnostics through the actual export |
| Samsung S26 / reported Xclipse environment | Complete onboarding before any photo exists; record compiler and device-loss events |
| Lenovo Idea Tab Pro, Tab S9+, S25 Ultra | Destroy/recreation/Pause and lost-surface teardown; expand both panic messages and the native ANR thread stacks |
| Intel/Linux from #176 | Same zoomed photo with and without other editors, plus kernel GPU hang/error-state capture and current memory headroom |

For each candidate: compare 1.4.1, the exact withdrawn tag, and the candidate
using the same image/edit and driver; record the cache state rather than
repeatedly replacing builds with no baseline. Keep photo edits and settings
when isolating the GPU pipeline cache. Do not ask reporters to delete their
whole application data or change TDR settings.

Use the existing render-parity cases and format fixtures to check numerical
and export behavior. For changes to concurrency, cancellation or lifetime,
add only a focused public-API regression allowed by AGENTS.md if automated
coverage is needed. Exercise native lifecycle and recovery in the real app;
do not add layout/wiring unit tests or a separate test harness. Run affected
crate checks while iterating, then the repository's full required checks
once for an implemented candidate, including Android target checks and the
JNI contract for bridge changes. Do not bless output differences silently.

**Verification performed for this investigation.** Read all public issue
bodies and the relevant comment threads, inspected the tag/working-tree
differences and pinned dependency source, matched Play version 1001322 to
the release, and inspected the supplied 1.4.1 and 1.5.0 stacks. The descriptor
permission experiment reproduced EACCES on reopening while dup/write
succeeded. Existing shader validation passed (2 tests), and existing GPU
layout contracts passed (21 tests), using:

```sh
cargo test --locked -p calibraw-gpu --lib shaders_validate -- --nocapture
cargo test --locked -p calibraw-gpu --lib layout_contract_tests -- --nocapture
```

Those checks validate shader composition and contracts, not the affected
vendor compilers or recovery behavior. This environment reports an NVIDIA
RTX 4070 Ti on Linux and has no attached Android device. There is no Windows
or affected Android hardware reproduction in this investigation. No baseline
was blessed, issue comment posted, or release published.

**Verification of the implemented repairs.** The full workspace test run passed:
98 tests passed, 16 existing tests ignored, none failed. This includes GPU render
parity, shader validation, all GPU layout contracts, format fixtures and export
tests. Workspace Clippy with all targets/features and the prescribed warnings
passed. Formatting, architecture, JNI (81 contracts), UI lint, and migrations
checks passed. The Android arm64 Rust library check and Java compilation passed.
Gradle's release native-symbol extraction and archive assembly passed, producing
`native-debug-symbols.zip` with Rust function symbols. This checked symbol
packaging against staged native libraries, not a newly linked release candidate.
The workflow YAML parses; the release-file collector was checked with a
diagnostics archive alongside all twelve application assets.

Two existing repository checks remain failing. `cfg-ratchet` reports stale
counts in `calibraw-ai/src/config.rs` (7 vs 5), `pipeline/gpu.rs` (5 vs 3), and
`pipeline/gpu/resources.rs` (6 vs 3); every `target_os` line in those files is
unchanged from HEAD. `cargo deny check` fails its advisories check because
`chrono 0.4.45`, already in the original lockfile through rawler/trash, is
unmaintained (RUSTSEC-2026-0543). Bans, licenses and sources pass. The only
lockfile change is replacing registry winit with the same-version local patch.
Neither check was suppressed or its baseline raised as part of these repairs.

**Follow-up: basic startup prewarm only.** The app no longer calls or exposes
`compile_common_programs`; its unused alternate-demosaic prewarm and filtering
helpers were removed too. The preview/export constructors still warm the
default RAW, tone and display passes. Optional effect programs retain their
existing lazy compilation, shared templates and cache persistence. Their
shader code, pass ordering and numerical output were not changed.

A before/after run of the real desktop app on Linux/Vulkan, RTX 4070 Ti,
NVIDIA 620.70.06, with isolated settings/cache and no photo open recorded:

| Build / application cache | Pipeline-creation calls during startup | Optional effect pipelines |
| --- | ---: | --- |
| Before, empty app cache | 94 | Includes all nine styled-glow and eight defocus passes |
| After, empty app cache | 27 | None |
| After, reused basic-program cache | 27 | None |
| After, full cache produced by the old startup sweep | 27 | None |

All runs finished prewarming and remained alive until stopped. Driver caches
were not separately cleared; these counts are driver API calls, not measurements
of compiler cache misses. The workspace tests passed again (98 passed, 16
ignored), including glow/defocus render parity with unchanged goldens. Clippy,
Android arm64 compilation, formatting, architecture, JNI, UI lint and migrations
passed; the two pre-existing check failures above are unchanged.

This removes the optional compilation phase where #192's log stopped. It is
containment pending an affected-device result, not proof of the exact crashing
shader. An effect can still crash its driver when first rendered, including
when a saved edit or edited thumbnail uses it. Basic-program compiler crashes,
such as the supplied older Redmi RAW-stage stacks, are not removed by this
change.

These checks do not replace the reporting-device recovery/lifecycle and
MediaStore-provider acceptance cases above. The native compiler calls now
record their entry point, variant constants, backend, thread and unique ID
before entering the driver, so a native termination can leave useful evidence.

The Android dialog deadlock and descriptor boundary have actionable causes
now. The native compiler failures have an established failure location that
requires program-level evidence for the final shader repair. The initial
Mali/Xclipse losses and the broader Windows reset frequency remain open
until loss reasons, in-flight work and native fault data are captured.
