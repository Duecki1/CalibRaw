# Android export and Glow / Windows Glow follow-up

Investigated against `9840feb4` on 2026-11-17. This supplements
[the stability investigation](V1_5_0_STABILITY_INVESTIGATION.md) and
[the workload plan](GPU_WORKLOAD_STABILITY_PLAN.md). Results below distinguish
the original captures, temporary experiments and the working-tree candidate.

**Outcome:** conservative RAW/output bands have reached the end of Samsung's
tile workload. One run rejected non-finite last-tile RGB; a later run passed
all tile readbacks but lost the device in the subsequent geometry footprint
pass. That separate dispatch path is now bounded too, pending device
verification. No full Samsung export is declared fixed. PowerVR and Windows
native compilation failures remain unresolved.

## Evidence preserved

Local artifacts are under `target/adb-captures/20261117-followup/` (ignored by
Git). The original installed Samsung APK, its logs, ADB logcat, process-exit
records and both sidecars were copied before installing experiments. The three
supplied logs were copied as `powervr-{session,crash}.log` and
`windows-session.log`. `SHA256SUMS` identifies the original copies. Temporary
APK/source pairs are retained separately; their revision banner alone does
not distinguish an experiment from HEAD.

| Report | What the new evidence establishes | What it does not establish |
| --- | --- | --- |
| Samsung SM-A336B, Mali-G68 r32p1, Android 15 | Repeated X100V export loses Vulkan device; reproduced without Glow on the installed APK | A kernel timeout, MMU fault, OOM, or a specific failing shader instruction |
| Redmi 24094RAD4G, PowerVR BXM-8-256, Android 16, driver `25.1@6715691` | Basic startup completes; adding Glow reaches its pyramids and overlaps two creative-effect compilations | That the newest failure is caused by the last submitted map pass, or that it has the error code quoted from an earlier session |
| Windows, RX 580 2048SP, Vulkan, AMD `26.1.1` | Startup and photo opening succeed; journal ends entering `build_styled_glow_near_first` compilation #33 | A TDR, physical GPU reset, native exception, or successful submission of that shader |

The Windows adapter is the RX 580 2048SP in this log, not the RX 480 named in
the earlier report. This run uses Vulkan; bundling DXC changes the DX12 path
and cannot repair this native Vulkan compilation.

## Samsung: controlled dispatch comparison

Source is the same 6246×4170 Fujifilm X100V X-Trans photo. JPEG quality is 90.
The less-edited photo has no Glow; it still has saved sensor/colour NR and a
Camera match. It is not described here as an entirely neutral edit.

The supplied 19:36 failure used core 768, halo 240 and a 1248×1248 padded tile.
Recalculation bands grew from 24 to 29 to 33 to 41 workgroup rows. Two
156×41 dispatches were outstanding when the loss was reported at +45.878 s,
248/247 ms after submission. Available system memory was 1,323,466,752 bytes.
These are callback/submission ages, not timestamp-measured shader durations.

The fresh less-edited baseline resolves to core 768, halo 144, padded tile
1056×1056 and 54 tiles. Export starts at +67.624 s; loss is recorded at
+68.983 s, with 1,148,133,376 bytes of system memory available. This repeats
the earlier first-tile failure without needing Glow. System memory counters
do not measure the driver's private allocation/spill limits.

ADB also contains a separate 19:33:21 SIGSEGV on `Jit thread pool` in PID
23353. It supplies no vendor-compiler stack; the corresponding exit-info
entry says EXIT_SELF. That event is preserved, but is not attributed to Bayer
compilation or merged into the repeatable export device-loss diagnosis.

Temporary builds keep tile size, shaders, Final arithmetic and the image edit
unchanged. The changed variable is the maximum workgroups in a dispatch:
256, with at least one workgroup row. At 132 columns this is one row.

| Experiment | Result |
| --- | --- |
| Original adaptive bands | Device lost during first tile |
| Probe-sized bands for every banded pass | All 54 tone-analysis tiles completed; final rendering reached tile 4/54 without loss. Deliberately cancelled to isolate a smaller change; this was not a completed export |
| Probe-sized recalculation alone | Device lost at +42.647 s after export started at +40.119 s. Outstanding work is now homogeneity + accumulation, 132×132 each, followed by smoothing, 132×71. Available system memory 1,429,274,624 bytes |
| Probe-sized Markesteijn family, with capacity/loss-guard repairs | Device lost at +60.445 s. Earlier one-row smoothing dispatches remain outstanding; the same queue also holds full-grid demosaic finish and colour-NR dispatches at +60.311/+60.312 s. Available system memory 1,426,825,216 bytes. No tracked submission is recorded after terminal loss |

Bounding recalculation alone therefore does not fix export. The earlier
recalculation breadcrumb was not the only vulnerable workload. Smaller bands
are a supported containment direction, but these experiments do not identify
the native failure as a watchdog event. The family-only bound also fails.
The final candidate bounds all tracked banded passes on the reproduced Mali-G68/r32p1/Vulkan combination; its
complete-export results are recorded below.

## PowerVR: the newest log must be separated from the quoted old failure

Both preview and export basic prewarm complete. At +25.962 s the first Glow
near-level program enters compilation and returns in 7 ms. Its dispatches
complete, followed by near levels, map accumulation and resolution.

At +26.043 s map-level dispatch #89, 48×26, is submitted. Compilation #36
enters `apply_creative_effects` on the UI thread and has no matching return.
A second `apply_creative_effects` compilation #37 starts on
`calibraw-effect-programs` at +26.445 s and returns at +26.454 s.

Source explains this overlap: `Readiness::WhenReady` queues the specialized
effect set. Until it returns, `dispatch_program` calls `ComputeProgram::get`
for the generic program containing every effect. With the old optional
startup sweep removed, that generic program may itself need its first native
compilation on the UI thread. Thus removing the sweep moves this compilation
exposure to effect activation; it does not remove it. The two programs have
the same entry name but different effect definitions. Existing compilation
records do not include those definitions, so the source path is needed to
interpret the pair. This is a concrete next isolation target: the generic
creative program and specialized Glow program separately, then overlapping.

The `ERROR_UNKNOWN` (-13), 4126 ms and map-level loss description occur in a
**quoted previous-session error dialog**, including at the beginning of the
new crash log. There is no fresh direct device-loss record at the end of that
log. The next session repeats the old description again. Do not assign that
old error code or duration to compilation #36 merely because the dialog
repeats it. A native dump or a fresh direct backend failure remains necessary.

The log also contains larger bands after small probes: map accumulation
96×86, resolution 96×66 and map level 48×26. The source's origin-capacity
fallback can enlarge bands, but fast completion-based estimates can choose
large bands too. The trace does not record the requested band before the
fallback and cannot prove which caused a particular large band.

`e99162cd` (Add basic hot pixel removal) does not change the styled-Glow WGSL
or the scheduler. The current startup containment explicitly stops prewarming
optional effects. That explains how an affected compiler can now reach the
library and fail when an effect is first used, without proving hot-pixel
removal repaired the driver or caused the Glow issue.

## Windows: first Glow compilation is now identified

The journal names AMD Radeon RX 580 2048SP, Vulkan, proprietary shader compiler
`26.1.1`, with 7164 MiB of 7367 MiB GPU budget free at startup. Default
prewarming finishes. A 3984×2984 DNG opens and previews; NR-enabled Bayer
compilation #31 returns and prefetch completes. At +620 s a resized preview
also renders normally.

At +633.719 s, compilation #33 enters `build_styled_glow_near_first` on the
main thread. The file ends there. There is no matching return, no Glow
dispatch and no device-loss or panic record. This narrows the next reproduction
to native pipeline compilation before Glow's first dispatch. A truncated log
alone cannot distinguish a native crash, a stalled compiler, an OS reset or
a separately killed process. The free-budget measurement is from startup,
not the failure boundary.

Next reporting-PC capture: retain the exact binary/cache, Windows Application
Error/WER exception details and a process dump while repeating the first Glow
activation. If the system reports an actual display-driver recovery, preserve
the corresponding System/Reliability event and GPU watchdog dump too. Compare
loaded, empty and disabled **application pipeline cache**, preserving edits
and settings, then test the isolated generated shader on that driver. A
successful Linux/NVIDIA compile cannot validate AMD's Windows compiler.

[Vulkan permits several error returns from compute pipeline creation](https://docs.vulkan.org/refpages/latest/refpages/source/vkCreateComputePipelines.html).
[Microsoft's DRED](https://microsoft.github.io/DirectX-Specs/d3d/DeviceRemovedExtendedData.html)
is Direct3D 12 device-removal evidence; it does not diagnose this Vulkan run.

## Focused candidate changes

- Bound tracked banded dispatches independently of adaptive timings on the
  reproduced Mali-G68/r32p1 Vulkan driver. Other adapters' tracked processing
  limits and shader arithmetic remain unchanged. This can increase submission
  overhead on that driver.
- Allocate band origins for every supported 2D texture row, rather than the
  source height. This covers Glow/defocus maps and padded levels larger than
  small previews. Reserve the buffer's actual GPU bytes for its lifetime.
  Remove the fallback that enlarges a band when the next origin is absent.
- Stop tracked dispatch/submission and timing resolution after the device-loss
  handler publishes terminal loss. The original Samsung capture queued
  thousands of later dispatches after that point. This does not by itself
  make the underlying driver failure recoverable or cover every direct GPU
  producer.
- Include the effect-set bits in specialized pipeline labels, so future
  compile records distinguish the generic and specialized creative programs.
- Recognize direct loss/panic/compile records in previous-session diagnosis.
  Quoted dialog text must not become a new failure. This repairs recursive
  PowerVR reporting and prevents a quoted unmatched compile from being
  mistaken for the current session's failure. Screenshot 19:30 is a breadcrumb
  report, not independent proof that Bayer compilation crashed natively.

- Bound the separate export geometry footprint, accumulation and resolve
  dispatches. Share the measured driver's 256-workgroup policy with RAW;
  elsewhere allow at most 16,384 workgroups per geometry dispatch. Footprints
  use workgroup-aligned subrectangles; accumulation/resolve split independent
  columns, preserving each pixel's source-row addition order. Wait between
  submissions, honour cancellation and include geometry in in-flight records.
  The uniform layout, filter and numerical operations stay unchanged.
- Add opt-in [export and native-compiler probes](GPU_EXPORT_PROBES.md) to retain
  invalid readbacks and stage snapshots, compare full/rectangular copies, and
  isolate generic/specialized creative compilation, overlap and cache use.
  These controls do not sanitize invalid RGB or select a different default
  effect program.

The other memory-admission, untracked scene-work, compiler-artifact and renderer
recovery items in the workload plan remain open. Neither the PowerVR Glow
failure nor the Windows native compiler failure is declared fixed here.

## Complete Samsung candidate run: reset avoided, export still fails

Candidate `candidate-all.apk` has native Build ID
`84c6a40fba66348ad25eee46c1ab92b473bf1a24`; hashes of all four experiments
and the original are in `apk-identity.json`. The same less-edited photo,
JPEG 90, full 6246×4170 output, 54 tiles and 1056×1056 padded grid were used.

- All 54 analysis tiles completed. Final rendering starts at +280.882 s.
- All 54 final tiles were processed; no `GPU device lost` record occurs.
- At +411.662 s, after 360.154 s of export, the worker returns an error:
  `read export tile 54: display-linear export readback contains NaN or infinity`.
- The app remains alive (PID 30268), and `Pictures/CalibRaw` contains no
  published output. The failure was handled as an export error, without a
  restart. Stage timing was enabled, so this is a diagnostic duration.

The readback copies **only the core**, not the padded texture: the last core
is photo position (6144,3840), extent 102×330, at local tile origin (144,144).
The check ignores staging-row padding and alpha. Thus this cannot be dismissed
as an unused padding texel or alpha channel. It is an additional invalid-RGB
problem in the region requested for output. The log lacks its first coordinate,
count and earliest producing stage. There is no evidence yet to choose between
shader arithmetic, driver code generation and a resource/readback error.

Do not replace these values with zero, remove the finite check, bless output
or declare export fixed. The next narrow device experiment should render this
last tile independently and record finite/range summaries after RAW, tone and
output, retaining the failing readback and parameters. Compare Camera match
and NR as controlled inputs, preserving the saved sidecar. Earlier failure
captures did not reach this validation boundary.

The conservative bound therefore remains a costly, driver-scoped containment
candidate. It is not release qualification: the exact full export must complete
with valid pixels, then the heavy Glow edit and repeated runs must pass.

## Last-tile probes and the separate geometry reset

Further artifacts are in `target/adb-captures/20261117-last-tile/`.
With GPU timing disabled, `raw` develops only tile 54 at its original source
coordinates in 4.889 s. Its entire padded RAW scene and core are finite.
`stages` performs the original 54-tile global analysis first (235.874 s total),
then develops tile 54 first: RAW scene, prepared scene, tone output and display
output all have finite RGB in both core and halo. The four retained textures
each contain 3,345,408 channel values; independent CPU inspection agrees.
Those captures precede the later addition of the exact-core/full-copy comparison
and tone-stat buffer capture. They do not prove normal export succeeds.

A subsequent `observe` export retains normal tile order and pipelined readback,
also with GPU timing disabled. All 54 final tile readbacks pass. The new
geometry programs compile and return (#72 footprint, #73 accumulate, #74
resolve); at +426.601 s Vulkan reports `ERROR_DEVICE_LOST`. The worker returns
at +426.648 s, after 372.418 s of export, with:

```text
GPU poll failed during export geometry footprint readback: GPU device lost
```

Available system memory was 1,167,314,944 bytes. The capture is
`geometry-loss.log` plus `geometry-loss-logcat.txt`; no JPEG was published.
This localizes a further failure beyond RAW/tile output. Before the new geometry
change, the footprint dispatched the whole 6246×4170 window at once (781×522
workgroups), through `dispatch_compute`, outside the conservative `WorkEncoder`
bands. Even its accumulation and resolve bands could exceed that driver's
limit. The new geometry change bounds all three and records their subrectangles
in the existing in-flight tracker, which now has a second real caller.

The earlier non-finite run had GPU timing enabled; these probes and the later
geometry-loss run did not. This is a comparison variable, not evidence that
timestamps caused invalid pixels. Repeat the complete export with timing off
and on, then with the heavy Glow edit, before declaring acceptance. If invalid
RGB recurs, `observe` retains the actual rejected readback plus current GPU
textures without rerendering; `sync` separates inter-tile pipelining as an
explicit comparison. Neither is a numerical repair.

The phone disconnected from ADB before installing the geometry candidate.
Device verification of this last change is pending reconnection; the user
requested local checks in the meantime. `geometry-candidate.apk` is retained
with native Build ID `5f276eed1f77d7e6c49b2dce75c2601989336bbe`;
`geometry-identity.json` contains the APK hash and source hashes. Reconnect,
install this candidate, remove any probe markers and repeat the original
full export before changing edits. Then compare timing off/on and heavy Glow.
The installed older observer marker keeps ordinary ordering and publication,
but it should be removed before release acceptance.

## Candidate verification

The diagnostic parser was exercised through the public `PreviousSession` API
against the supplied files (`diagnosis-review.txt` in the capture folder). The
PowerVR session with only quoted errors now has no current cause; its crash
log identifies unmatched compile #36; Windows identifies unmatched compile
#33. The older Samsung crash archive, whose recorded compilations all return,
now has no compiler-crash cause. A direct Samsung device loss remains
recognized. No parser unit test or new test-only API was added.

Device export results follow below. The additional existing render-parity
case runner passed with `CALIBRAW_GPU_CHUNK_MS=0.01`, forcing small bands
through the Glow/defocus map grids as well as the image grids. Its goldens
were unchanged (`banded-parity.log`).

Checks before the last-tile/geometry follow-up: 98 workspace tests passed, 16 existing tests ignored,
none failed; workspace Clippy with the prescribed warnings passed. Formatting,
architecture, JNI, UI lint and migrations passed. Android `assembleDebug`
built and packaged the candidate successfully. The original sidecars were
compared with their on-device copies after the experiments and are byte-for-byte
unchanged. No output golden was blessed.

Existing unrelated failures remain: `cfg-ratchet` reports the same counts in
`calibraw-ai/src/config.rs` (7 vs 5), `pipeline/gpu.rs` (5 vs 3) and
`pipeline/gpu/resources.rs` (6 vs 3). This follow-up adds no `target_os`
conditions and leaves those pre-existing counts unchanged.
`cargo deny check` fails on the existing `chrono` RUSTSEC-2026-0543 advisory;
bans, licenses and sources pass. Neither baseline nor advisory policy was
relaxed. See the check logs in the capture directory.

## Final local verification of probes and geometry candidate

`20261117-last-tile/checks/status.json` records the final commands and their
separate logs. Workspace tests: 98 passed, 16 existing ignores, zero failures;
render parity uses unchanged goldens. Formatting, workspace Clippy with all
prescribed warnings, architecture, 82 JNI contracts, UI lint and migrations
pass. Android `assembleDebug` succeeds. The affected GPU crate also passes
independently, including shader validation and buffer-layout contracts.
Generic-only and specialized-only creative compilation each passed render
parity with `serial no-cache` before the later geometry-only change.

The same pre-existing `cfg-ratchet` counts and `chrono` advisory still fail;
no baseline, dependency or advisory policy was changed. Geometry pacing and
its smaller driver-specific dispatches can add overhead; their performance
and actual Samsung export completion need the device. PowerVR/Windows native
compiler comparisons and Samsung timestamp/heavy-Glow repetitions remain
hardware work, not successful local-test claims.
