# GPU workload stability: investigation and repair plan

Investigated 2026-11-17 against `9749bc2e` plus the working-tree change that
defers optional startup programs. This follow-up is an investigation and plan;
the repairs below have not been implemented or device-validated. Preserve the
existing warmup/recovery changes as separate reviewable work.

The [later same-day investigation](2026_11_17_GPU_CRASH_FOLLOWUP.md) records
the new Glow/Windows logs, controlled Samsung dispatch experiments and the
candidate repairs for band origins, terminal-loss submission and diagnostics.
The findings below describe the earlier checkpoint; remaining plan items are
not implied complete by that follow-up.

**Finding:** there are real gaps in workload and memory bounds, but neither
new report establishes RAM exhaustion as its cause. Repair those gaps and use
controlled experiments to identify the failing driver workload. Do not group
all device losses under a single shader, OOM or watchdog diagnosis.

## 1. Evidence and limits

### Samsung export: captured twice over ADB

Capture: `target/adb-captures/20261117-154616-export/logcat.txt`, particularly
lines 799–803, 894, 929–940, 4690–4694, 4708 and 4736–4745.

- Device: Samsung SM-A336B, Android 15, Mali-G68/Vulkan. Installed package
  reports version 1.5.0 and development version code 1; archive its APK hash,
  native Build ID and source revision before comparing a new candidate.
- Source: Fujifilm X100V, 6246×4170, X-Trans; JPEG quality 90.
- Requested tile settings are 768/960, but the **resolved** plan is 54 tiles,
  core 768, halo 144, padded tile **1056×1056**. The initial request is not
  the allocation size. Tile setup completed in 0.356/0.096 seconds.
- Free system memory at tile planning: 1384/1352 MiB. Tile allowance: 168 MiB;
  installed-RAM-derived working-set share: 294 MiB. GPU free memory is unknown.
- Export reserves capacity for a 74.5 MiB encoded raster and 298.1 MiB float
  geometry raster. `IntermediateWriter` reserves vector capacity first and
  fills it as rows arrive. These log lines do **not** prove that all 372.6 MiB
  was physically resident when the first RAW-stage work failed.
- Device loss occurs 0.908/0.604 seconds after export starts. Both traces name
  `xtrans_markesteijn_recalculate` in flight: 132×53 and 132×42 workgroups,
  then 132×58 and 132×41 on the repeat. Ages are about 101–103 ms.
- Every recorded compute-pipeline creation returned. `Device::poll` panics
  **after** device loss with `Parent device is lost`; it is a secondary event.
- On the repeat, the old common-effect warmup finished at 15:46:44.199;
  export began at 15:46:47.140. This export failure does not require concurrent
  optional-effect startup warmup.

The in-flight name identifies outstanding work, not a faulting instruction.
Submission ages are CPU-side ages, not precise GPU durations. There is no
captured MMU fault, kernel timeout, allocation failure or OOM-killer termination.
Do not claim a two-second watchdog timeout from a roughly 100 ms breadcrumb.

### PowerVR zoom: reporter's log and confirmed actions

Original: `/home/duecki/Downloads/log.log`; preserved copy:
`target/adb-captures/20261117-powervr-zoom/log.log` with source hash metadata.
The reporter confirms opening a photo and zooming/panning.

- Redmi 24094RAD4G, Android 16, PowerVR B-Series BXM-8-256, Vulkan driver
  `25.1@6715691`.
- The header says 1.4.1, but revision `6c8bb6b5` is **73 commits after the
  1.4.1 tag**, before the 1.5.0 tag. New glow/defocus work already exists in
  that history. This is not evidence of the official 1.4.1 binary failing.
- Bayer DNG, 3984×2984; Reference demosaic, luminance NR 34, colour NR 38,
  InpaintOpposed highlights. AI GPU acceleration is explicitly disabled.
- Fitted preview is binned 640×479, then DPI preview 978×733. Zoom is 3.24×;
  native crops installed are 1920×1968, 1924×1652 and 1920×1616. A reduced
  interactive graph is also built. Local mask count is zero; the log does
  not enumerate global effects, so zero masks does not prove no effects.
- Loss at +25.158 seconds coincides with the common-program loop returning,
  11.085 seconds after export prewarm. Zoom rendering and background program
  compilation overlap. No export action is recorded.
- `Unexpected error variant (driver implementation is at fault)` is the
  fixed display text of `wgpu_hal::DeviceError::Unexpected`, not an independent
  diagnosis. This journal omits the raw Vulkan result and exact active program.

The existing warmup containment removes that background sweep, making it the
first comparison for this report. It does not establish that zoom rendering,
resource replacement or a driver compilation was the original cause.

### PowerVR library: staging-upload panic

Original: `/home/duecki/Downloads/log (1).log`; preserved copy:
`target/adb-captures/20261117-powervr-library/log.log`. The owner understands
that it happened in the library; the exact actions are not yet confirmed.
It has the same PowerVR device, driver and development revision as the zoom
report, but records no document-open, zoom or export before the failure.

- Preview prewarm finishes at +6.847 s and export prewarm at +12.525 s.
- egui-wgpu 0.35.0 `Renderer::update_buffers` panics at line 1026 because
  `Queue::write_buffer_with` returned `None` for 1656 UI vertices: **33120
  bytes**, with a destination size/capacity of **61600 bytes**. The required
  copy fits the existing buffer. The panic text does not prove a capacity
  error or OOM; its staging upload is only about 32.3 KiB.
- The panic is journalled before the device-loss callback's message. At
  +21.463 s the device is reported lost with `DeviceError::Unexpected`, the
  common-program loop returns after 8.938 s, and cache persistence yields no
  data. Callback/journal order does not establish which native failure came
  first; the GPU could already be invalid when the upload is attempted.
- wgpu's write API returns `None` if destination validation fails **or**
  staging creation fails. Both core paths first check device validity; the
  allocation path can also fail allocating/mapping staging memory. This
  report does not contain the underlying write error needed to distinguish them.

A staging allocation/mapping error can itself **initiate** logical device loss:
`wgpu-core::StagingBuffer::new` passes both native failures to
`Device::handle_hal_error`, which calls `lose` for OutOfMemory, Lost and
Unexpected. Thus both causal directions are possible: another operation loses
the device before upload validation, or staging creation triggers its loss.
The popup does not independently establish an OS-level GPU reset. Record the
failed branch, underlying error and thread before attributing the first cause;
eliminating egui's panic alone does not repair the native allocation/driver error.

This strengthens the optional-warmup suspicion without identifying its exact
program. It also establishes a recovery hole: CalibRaw checks device loss at
the beginning of its UI callback, whereas egui uploads and paints afterwards.
A loss during that interval can reach this dependency panic before CalibRaw
gets another frame. The recovery handoff alone does not cover this path.

## 2. Confirmed code gaps

| Gap | Evidence in current code | Consequence / proposed repair |
| --- | --- | --- |
| egui converts failed uploads into a panic | `egui-wgpu` 0.35.0 `Renderer::update_buffers` panics on a missing staging view; CalibRaw's recovery check is earlier in `App::ui` | Make frame preparation fallible in a narrow dependency integration/backport, abort the entire failed frame and reach terminal recovery. Include index/vertex/texture uploads and surface operations; a pre-frame check alone races loss |
| Loss flags are published after disk flushing | `gpu_errors.rs` logs and calls `diagnostics::flush` before setting `DEVICE_WAS_LOST` and the UI's pending-loss message | Publish terminal state before blocking I/O so workers stop immediately. Preserve the original loss record and safe persistence ordering |
| Band origin table is sized to the source image, not every dispatched grid | `gpu/construction.rs` calls `DispatchBands::new(device, raw.height)`; `gpu/work.rs::dispatch` replaces a band with every remaining row if its next offset is missing | A larger effect map can escape the 100 ms target. Size/retain origins for the actual dispatch grid; missing capacity must never enlarge a band |
| Timings do not distinguish important workload changes | `PassWork::for_entry` hashes the entry name; the variant covers selected demosaic/effect programs. Glow/defocus dispatches use variant 0; their formats, tiers, slot counts and sampling work vary | A cheap render can size a much more expensive dispatch. Add conservative workload distinctions and a maximum growth rule, then measure on the failing driver |
| Some heavy work is outside adaptive banding | Scene bake/light helpers call `dispatch_compute` and shadow cubes call `draw`; `WorkEncoder::untracked` cuts only after the whole encoded block | The existing target cannot bound a single heavy pass or a block of shadow work. Give those producers explicit bounded submissions and breadcrumbs |
| Export tile planning cannot reduce its default core | `export/worker.rs::device_export_core_edge` returns the default when the fitted core is smaller or no padded tile fits | A reduced computed budget need not reduce the actual tile. Allow smaller valid cores, preserving halo and tone-grid alignment; reject cleanly if even the minimum cannot fit |
| A hard floor overrides current free-memory sizing | `gpu/preview_budget.rs::mobile_export_tile_budget` floors the result at 168 MiB | Free memory can fall without shrinking this allowance. Treat inability to fit as a real outcome; first stage host intermediates and retire optional graphs |
| Separate allowances are not a complete peak budget | `IntermediateMemory::for_this_device` uses installed RAM/8, Android GPU admission uses installed RAM/4, and row bands/previews/model sessions have other accounting | Coordinate live system headroom, GPU reservations and host peaks on unified-memory devices; keep host RAM and discrete VRAM distinct on desktop |
| Reservation accounting omits shared transient peaks | `GpuBudgetReservation::acquire` validates one plan's margin/peak but reserves `persistent_gpu_bytes`; the process counter does not reserve all overlapping transient/host peaks | Establish ownership and lifetime of transient reservations before allowing overlapping jobs; avoid double-counting memory already included in a driver's live usage |
| Failed pacing waits are ignored | `WorkEncoder::submit_chunk` discards a failed wait, clears outstanding state and permits its caller to keep encoding | Stop the producer after loss/cancellation and propagate a typed failure before another submission |
| Windows budget-policy draft is not wired | `memory/policy.rs` exists, but `memory/mod.rs` does not declare it; desktop GPU resource admission still returns `u64::MAX` | Do not count that file as a live protection. Integrate reviewed Windows DXGI policy separately, including shared-memory adapters |

The band-table defect has a concrete arithmetic example: a 171 px-high graph
allocates 22 origins. An effect grid 512 px high has 64 workgroup rows. With a
two-row estimate, the current fallback changes the last band at row 20 into
**44 rows**. At 50 ms per row this is 2.2 seconds instead of the 100 ms target.
This is a code-derived scenario, not an observed timing on either reporter's
device. Glow's full-image grid is 768 cells on its longer edge, so grids larger
than a small preview are a real supported shape.

## 3. Audit of other heavy functions

| Priority | Workload | Existing protection | Remaining work to investigate / bound |
| --- | --- | --- | --- |
| P0 | X-Trans Markesteijn recalculation, complete and homogeneity | 8×8 row bands through `WorkEncoder`; explicit state textures and pass ordering | Reproduce the captured failure with fixed small bands; inspect register/private-memory pressure and dynamic indexing; validate generated SPIR-V. Preserve the algorithm and its per-pixel arithmetic |
| P0 | PowerVR library and Bayer zoom, sensor NR, colour NR and highlight reconstruction | Proxy/crop sizing, selected demosaic variants, row bands; mobile detail replacement already frees an incompatible old graph | Compare both idle library and pan/zoom with optional warmup removed. Capture the underlying queue upload error and measure graph overlap. Test NR on/off as an isolation experiment |
| P1 | Styled Glow pyramids, streaks, star/split and full-image maps | All nine entries already read dispatch bands; near/map allocations have GPU reservations | Fix band-grid capacity; distinguish costly styles/slot counts; bound accumulation loops over large source regions and include every owned/shared map in peak accounting |
| P1 | Lens Blur/defocus gather, fill and maps | All eight entries already read dispatch bands; array layers enter dispatch work; near/map allocations have reservations | Fix capacity for non-photo-size grids, measure Final/Draft and radius/bin costs, and retain Final sampling/output |
| P1 | Scene depth/coverage bake, Relight shadows, visibility and volumetric air light | Bake working edge capped at 1540/1152; cube faces 512/256; tiered taps; reservations | These compute/render producers are not adaptively banded. Air light's Final march is 48 steps with four cube taps per step. Split bounded work and preserve required barriers/reductions |
| P1 | Scene shadow raster | One render pass per cube face; at most eight shadow lights | `encode_cube` draws `12 × (width−1) × (height−1)` vertices per face. A 1152×768 mesh is about 10.6 million vertices per face. Measure mobile intermediate geometry memory and batch contiguous triangle ranges with equivalent depth/load behavior |
| P1 | Global image-light, haze, contour/emission maps and detail pyramids | Existing row-band metadata; on-demand resource reservations | Fixed map dimensions and active effect complexity can differ from the photo grid and timing history. Audit dispatch extents, reductions and cost keys |
| P2 | Geometry resampling, rotate/perspective/lens export | 64 px cells, 32 MiB source chunks, bounded output bands, explicit waits and reservation | Footprint pass still covers the whole output; all direct submissions need loss breadcrumbs. Extreme footprints and wide images require validation; do not describe this already-streamed path as an unbounded full-image allocation |
| P2 | Remove composite, histogram and clipping | Usually bounded region/preview work | Direct dispatches are absent from the main in-flight summary. Record their ownership, extents and completion; prioritize a split only if the measurements require it |
| P2 | HDR/focus-stack/merge, edited thumbnails and AI/model overlap | Reused templates, worker limits, cancellation, AI placement estimates | Account for simultaneous decoded images, kept model sessions and separate-device GPU usage. They are overlap risks, not established causes of these two reports |

Arm documents a mobile intermediate-geometry limit that can produce device
loss. That is relevant to the **shadow raster audit**, not proof that the
X-Trans **compute** pass hit that limit. Do not copy a historical numeric limit
to every current Mali driver or infer it from system RAM alone.

## 4. Implementation order and acceptance gates

### A. Establish repeatable cases and decisive diagnostics

1. Reconnect the Samsung; preserve original RAW, sidecar/edit state, image
   orientation/geometry, installed APK hash, Build ID, driver and free-memory
   baseline. Keep the current failing candidate as the reference.
2. Record planned/reserved/actually allocated resource classes, process
   resident memory and available system memory at pipeline build, before RAW
   dispatch, and at failure. Trace CPU intermediate *length and capacity*
   separately. Obtain driver budget/fault information only where supported.
3. Give each submission a device, job, tile/crop, stage/pass, band origin,
   extent and variant identity. Include direct compute and shadow rendering.
   Preserve the earliest failure; a caught poll panic must not replace it.
   For the library panic, preserve `Queue::write_buffer_with`'s underlying
   validation/allocation error, callback timing and original native result;
   the renderer's later generic panic must not become the only cause shown.
4. Enable the existing GPU stage timestamps in a diagnostic run where the
   adapter supports them. Completion-callback latency is an upper bound.
5. Collect full ADB/wgpu/Vulkan errors and the raw driver result where
   available. Query `VK_EXT_device_fault` only after checking/enabling support
   through the GPU-owned backend; it may not exist on the reporting drivers.
   For Windows DX12 diagnostic builds, use DRED; it does not diagnose Vulkan.

**Gate:** identify the failing stage and distinguish execution fault, timeout,
allocation failure, compiler failure and an OS process kill as far as the
driver evidence allows. A generic device-lost callback is not an OOM finding.

### B. Run experiments that separate competing explanations

| Controlled change | Hold fixed | What a changed result would support |
| --- | --- | --- |
| Much smaller fixed dispatch bands / conservative target | Tile dimensions, textures and host-intermediate policy | Execution granularity, scheduler estimation or per-dispatch driver resource pressure |
| Stage CPU intermediates and retire optional preview graphs | Same 1056×1056 tile and band policy | Aggregate memory/lifetime pressure; collect actual resident usage to interpret it |
| Smaller tile with the same halo | Image/edit and Final algorithms | A useful containment candidate, but it changes both memory and dispatch shape and cannot identify which was causal by itself |
| Disable the optional startup sweep | Same PowerVR photo, NR settings and repeated zoom/pan | Whether startup compilation overlap or a swept program is required for that failure |
| Serialize heavyweight producers while allowing UI frames | Graphs/edits and per-pass shapes | Overlap/order/driver-concurrency involvement, not automatically a shader arithmetic bug |
| Equivalent reduced X-Trans shader/constant-index form | Resources, tile and bands | Shader code-generation/execution involvement, requiring parity and affected-driver confirmation |

A desktop environment variable is not automatically an Android app
environment variable. Apply diagnostic chunk settings before the process's
first scheduler lookup through a deliberate diagnostic build/wrapper; do not
claim an ADB property changed them without an observed startup record.

### C. Repair the proven scheduler and failure-propagation gaps

- Make UI frame preparation/upload failure propagate to controlled terminal
  recovery. Treat a pre-frame loss check as an optimization, not the correctness
  boundary. Do not simply return empty buffers and continue drawing a partial
  frame, or blanket-catch unrelated renderer/programming panics. Evaluate the
  egui/eframe integration's necessary narrow API/backport before changing it.
- Publish loss state before diagnostic disk waits; stop workers immediately
  and preserve failure records before the recovery process ends the renderer.
- Size band origins for actual dispatch grids; remove the fallback that
  silently turns capacity exhaustion into a large dispatch.
- Keep conservative estimates across changes in device, format/tier and
  effect complexity. Bound band-size growth; measure a new workload before
  trusting timings from its cheaper predecessor.
- Route heavy scene compute through bounded scheduling. Batch shadow work
  by measured vertex/driver limits with correct depth and resource lifetime.
- Stop encoding/submission after a failed pacing wait or terminal device loss;
  propagate cancellation separately from GPU loss and programming errors.

**Gate:** every requested band fits its origin table and workload bound;
untracked producers have explicit bounds. Final arithmetic, pass ordering,
tile halos, shared maps and output invalidation remain correct.

### D. Make admission reflect the complete live workload

- Coordinate retained previews, export tile, effect maps, compiler/driver
  headroom and CPU intermediates on integrated/unified-memory devices.
- Stage large intermediates before growing a tile or evicting useful work;
  spilling to a mapped file reduces private heap pressure but is not zero RAM.
- Recheck headroom at allocation boundaries. Admit transient peaks for their
  actual lifetime, and avoid counting an existing allocation twice.
- Permit a smaller valid tile core. Preserve the halo; never silently lower
  export resolution, precision or the Final effect. If no valid tile fits,
  cancel cleanly before submission and remove pending/partial output.
- Integrate the dormant Windows admission policy as a separate reviewed
  change; use actual DXGI process budgets and system headroom for iGPUs.

**Gate:** low-headroom operation stages/trims/reduces a tile or returns an
error before GPU submission, with no reset used as the admission mechanism.

### E. Repair a shader only once the remaining fault identifies it

If the Samsung still loses the device with conservative bands and measured
memory headroom, retain its generated WGSL/SPIR-V and reduce the recalculation
path. `spirv-val` is installed locally; Arm's compiler/profiling tools may help
quantify register/spill pressure, but offline success is not an affected-driver
test. Do not replace Markesteijn, disable precision or blame a GPU family from
the in-flight label alone. Verify the smallest repair on the reporting driver.

## 5. Validation and release conditions

- Samsung: repeated full X100V JPEG exports of the exact failing edit, then
  PNG/TIFF/JXL, reduced and full output sizes, binned/fitted preview and native
  zoom. Include repeated export after relaunch and constrained headroom.
- PowerVR: idle library through startup, then exact DNG and NR settings, open,
  zoom 3.24× and repeated pan/crop replacement, both empty and existing app
  pipeline cache. Record background compiles, UI uploads and graph allocations
  independently. Exercise loss between the app UI callback and egui painting.
- Effects: existing glow styles, defocus, depth/Relight and light-shaft parity
  cases; tiny previews with larger map grids, many slots, Draft→Final and full
  export. Draft may coarsen detail; exports remain Final.
- Windows: packaged DX12/DXC and Vulkan on NVIDIA/Intel/AMD, especially RX 480;
  empty/existing caches, startup, zoom, export and realistic GPU headroom.
- Exercise cancellation and loss at allocation, encoding, submit, wait and
  close. Add only meaningful concurrency/cancellation/cache regression coverage
  through public APIs as allowed by AGENTS.md; no test-only APIs or new harness.
- Run affected-crate checks while iterating, then required workspace checks,
  Android target/bridge checks as applicable, and real-app/device scenarios.
  Render parity and format fixtures must pass without silently blessing output.

Do not publish a claim that the Samsung or PowerVR root cause is fixed until
its own reproduction passes. The already implemented startup containment can
be evaluated independently on PowerVR and the RX 480 startup report.

## Sources and current limits

- Local source: `gpu/work.rs`, `gpu/resources.rs`, `gpu/preview_budget.rs`,
  `gpu/{defocus,styled_glow}.rs`, `gpu/scene/{bake,lights}.rs`, export worker,
  intermediates and geometry paths; preview detail replacement and AI placement.
- [Arm: diagnosing device loss](https://developer.arm.com/community/arm-community-blogs/b/mobile-graphics-and-gaming-blog/posts/diagnosing-vk-error-device-lost-on-mali-gpus-with-vk-ext-device-fault)
  explains supported fault/address-binding diagnostics and their limits.
- [Arm: Vulkan memory limits](https://developer.arm.com/community/arm-community-blogs/b/mobile-graphics-and-gaming-blog/posts/memory-limits-with-vulkan-on-mali-gpus)
  concerns intermediate **geometry** output, not a universal compute/RAM limit.
- [Vulkan: devices and queues](https://docs.vulkan.org/spec/latest/chapters/devsandqueues.html)
  lists multiple implementation-specific causes of device loss.

The Samsung disconnected during follow-up; no new on-device experiment or
fault-extension query was possible. PowerVR supplied a diagnostic journal,
not a full native fault dump. This plan establishes specific code gaps and
controlled next experiments; it does not convert those gaps into a proven
cause for either report.
