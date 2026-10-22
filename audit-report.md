# CalibRaw code quality audit report

Working version: v1.3.1 (root `Cargo.toml` `[workspace.package] version`).
Findings are ordered by discovery. Test code (`tests.rs`, `*_tests.rs`, inline `mod tests`) is out of scope.
### [UI-001] Identical Discord "unavailable in this build" notice duplicated in two views
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/settings.rs:403 ; crates/calibraw-ui/src/ui/onboarding.rs:296
- evidence:
  settings.rs:403: `                    "Unavailable in this build: CALIBRAW_DISCORD_APPLICATION_ID is not configured.",`
  onboarding.rs:296: `            "Unavailable in this build: CALIBRAW_DISCORD_APPLICATION_ID is not configured.",`
  Both sit inside `if !app.discord_rich_presence_configured() { ui.colored_label(ui.visuals().warn_fg_color, ...) }`, which is byte-identical in both files; nothing differs between the two sites.
- why it is a problem: the same warning text, including a hardcoded build-input variable name that is already named in `app/discord_presence.rs` (`const APPLICATION_ID_ENV`), is written out twice in two views, so one can drift out of sync with the other and with the constant.
- fix: return the notice string from one place (e.g. `DiscordPresence::unavailable_notice()` in `app/discord_presence.rs`, built from `APPLICATION_ID_ENV`) and have both views label it; serialized names unchanged.
- effort: S
### [UI-002] Wildcard imports in production modules (AGENTS.md requires explicit imports)
- category: rule-violation
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/library/mod.rs:46-58 ; 116 more `use super::*;` at module scope in crates/calibraw-ui/src ; 42 in crates/calibraw-core/src ; 32 in crates/calibraw-gpu/src ; 9 in crates/calibraw-ai/src ; 7 in crates/calibraw-ffi/src (206 `use super::*;` and 67 sibling-module wildcards such as `use actions::*;` in total, `grep -rn '^use [A-Za-z_.:]*::\*;'` excluding test modules)
- evidence:
  `crates/calibraw-ui/src/ui/library/mod.rs:46:use actions::*;`
  `crates/calibraw-ui/src/ui/library/mod.rs:47:use adjustments::*;`
  `crates/calibraw-ui/src/ui/library/mod.rs:54:use folders::catalog::*;`
  (the same file also lists explicit imports at lines 1-21, so the wildcard block is a choice, not a necessity)
- why it is a problem: AGENTS.md ("Rust") requires explicit imports in production modules, and `use super::*` / `use sibling::*` pulls every public name of the parent or sibling module into scope, so a renamed or newly added item silently changes which symbol a call resolves to.
- fix: replace each wildcard with the list of names the module actually uses (the explicit imports already used elsewhere in the same file show the expected style); do it per file as a hygiene-only change.
- effort: L
### [UI-003] Lens-correction notice passed as an out-parameter that one call site discards with `&mut None`
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/lifecycle/document_load.rs:378 (call site), crates/calibraw-ui/src/app/lifecycle/document_load.rs:664 (definition)
- evidence:
  `crates/calibraw-ui/src/app/lifecycle/document_load.rs:378:        apply_saved_lens_correction(&decoded, saved_lens, context.automatic_lens, &mut None);`
  `crates/calibraw-ui/src/app/lifecycle/document_load.rs:664:fn apply_saved_lens_correction(` ... `crates/calibraw-ui/src/app/lifecycle/document_load.rs:668:    sidecar_warning: &mut Option<String>,`
  Inside the body: `if saved.enabled && !lens_correction.catalog.available { append_notice(sidecar_warning, "The saved lens correction is unavailable in this build.") }`
- why it is a problem: the function returns a 2-tuple plus a third result smuggled through a borrowed out-parameter, and the prefetch caller takes a mutable reference to the literal `None` (`&mut None`), so the "saved lens correction unavailable" notice is silently dropped there with nothing in the code saying that discarding it is intended.
- fix: return the notice as part of the result (a small result struct with named fields, replacing the tuple), and have the prefetch caller bind it to `_sidecar_notice`; behaviour unchanged.
- effort: S
### [UI-004] Nine near-identical destructure-and-copy arms reset mask geometry field by field
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/masks/properties.rs:26-128 (inside `apply_mask_properties_action`, file lines 26-128 = 103 lines)
- evidence:
  `crates/calibraw-ui/src/ui/sidebar/masks/properties.rs:26:                    match (&mut component.geometry, previous) {`
  `...                            MaskGeometry::Radial { center, radius, rotation, initialized, .. },`
  `...                            MaskGeometry::Radial { center: c, radius: r, rotation: angle, initialized: ready, .. },`
  `...                            ) => { *center = c; *radius = r; *rotation = angle; *initialized = ready; }`
  `...                            MaskGeometry::Linear { start, end, initialized, .. },`
  `...                            MaskGeometry::Linear { start: a, end: b, initialized: ready, .. },) => { *start = a; *end = b; *initialized = ready; }`
  Nine arms (Brush, Radial, Linear, Path, Ai, DepthRange, Object, LuminanceRange, ColorRange) all do only `*new_field = old_field` for identically named fields; nothing else differs.
- why it is a problem: the same operation (keep the interaction/state payload, drop the default geometry) is written out once per `MaskGeometry` variant, so adding a variant or a state field means hand-writing another arm, and the trailing `_ => {}` silently skips any variant not enumerated.
- fix: give `MaskGeometry` one method (e.g. `reset_preserving_state(from: MaskGeometry)`) implemented in `calibraw-core` next to the enum, and call it here; the per-variant field lists then exist once instead of in a UI match.
- effort: M
### [UI-005] `MaskGeometry::Placeholder` is never produced; its UI label is a placeholder for nothing
- category: temp-or-hardcoded
- severity: low
- confidence: medium
- where: crates/calibraw-ui/src/ui/sidebar/masks/properties.rs:493 ; crates/calibraw-core/src/pipeline/masks/mask_geometry.rs:91 (variant declaration)
- evidence:
  `crates/calibraw-ui/src/ui/sidebar/masks/properties.rs:493:                MaskGeometry::Placeholder => {`
  `crates/calibraw-ui/src/ui/sidebar/masks/properties.rs:494:                    ui.label("This mask type is not implemented yet.");`
  `crates/calibraw-core/src/pipeline/masks/mask_geometry.rs:91:    Placeholder,`
  `grep -rn 'Placeholder' crates` (excluding the unrelated `PreviewPlaceholder`) finds only these six sites, all patterns: `layers.rs:108: MaskGeometry::Placeholder => {}`, `mask_geometry.rs:249: Self::Placeholder => false`, `mask_geometry.rs:274: | Self::Placeholder => return false`, `validation.rs:428: | (MaskKind::DepthRange, MaskGeometry::Placeholder)`. No expression constructs the variant.
- why it is a problem: the "not implemented yet" branch can only be reached from a hand-edited sidecar that the validator happens to accept for DepthRange, so the app shows an unimplemented-feature message for a state no CalibRaw code can create, and nothing records what feature (if any) `Placeholder` stands for or when the branch may be dropped.
- fix: if `Placeholder` is a reserved sidecar value, keep the variant but give the label a specific reason ("this mask file has no shape data") and note in `mask_geometry.rs` which sidecar versions may contain it; if it is not reserved, delete the variant, its three pattern arms and its test cases.
- effort: S
### [UI-006] Same five-line "selected component is not initialized" guard written four times in one hint match
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/preview/tools/masks/overlay.rs:478-482 (Radial), 487-491 (Linear), 499-503 (ColorRange), 508-512 (DepthRange); function `paint_tool_hint`, starts at crates/calibraw-ui/src/ui/preview/tools/masks/overlay.rs:454
- evidence:
  `crates/calibraw-ui/src/ui/preview/tools/masks/overlay.rs:478:                if !app` then lines 479-482 `.masks / .stack / .selected_component() / .is_some_and(|component| component.geometry.is_initialized()) =>`
  repeated verbatim at line 487-491 for `MaskKind::Linear` (only the label string differs: "Drag across the image to create a linear gradient").
- why it is a problem: the same navigation path and predicate is re-typed inside every guarded arm, so a change to how the selected component is reached has to be repeated four times and one arm can silently disagree with the others.
- fix: compute the predicate once above the match (`let uninitialized_selected = app.masks.stack.selected_component().is_some_and(|c| !c.geometry.is_initialized());`) or add a small named helper on `MaskState`, and match on it.
- effort: S
### [CORE-001] The 32 768-pixel edge limit is written as a bare literal in the sidecar validators
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/sidecar/validation.rs:88-89 ; crates/calibraw-core/src/sidecar/remove_assets.rs:296
- evidence:
  `crates/calibraw-core/src/sidecar/validation.rs:88:                || patch.bounds.width > 32_768`
  `crates/calibraw-core/src/sidecar/validation.rs:89:                || patch.bounds.height > 32_768`
  `crates/calibraw-core/src/sidecar/remove_assets.rs:296:        if asset.width == 0 || asset.height == 0 || asset.width > 32_768 || asset.height > 32_768 {`
  The same limit already exists twice as a named constant: `crates/calibraw-core/src/pipeline/raw_loader.rs:123: pub const MAX_RAW_EDGE: u32 = 32_768;` and `crates/calibraw-gpu/src/pipeline/export/settings.rs:120: pub const MAX_EXPORT_EDGE: u32 = 32_768;`. `grep -rn '32_768' crates --include='*.rs'` returns these four production sites plus the two declarations (the rest are test files).
- why it is a problem: the reject-limit for stored patch and remove-image dimensions is invisible to a reader of the validator (what makes 32 768 special?), and changing the edge limit means finding four unconnected literals instead of one constant.
- fix: name the limit once in `calibraw-core` (e.g. `MAX_SIDEcar_IMAGE_EDGE`) and use it in both validators; keep `MAX_RAW_EDGE`/`MAX_EXPORT_EDGE` as the separately justified loader and export limits.
- effort: S
### [UI-007] The same "install a loading thumbnail" block written four times in cache.rs
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/lifecycle/cache.rs:34-40 (prepare_develop_loading_thumbnail), :78-84 (refresh_develop_loading_thumbnail), :139-145 (prepare_android_develop_loading_thumbnail), :153-159 (refresh_media_library_loading_thumbnail)
- evidence:
  `crates/calibraw-ui/src/app/lifecycle/cache.rs:34:            self.develop_ui.loading_thumbnail.texture = Some(texture);` followed by `texture_size = Some(size); if let Some(color) = adaptive_backdrop { self.ui.adaptive_preview_backdrop = color; }`
  `crates/calibraw-ui/src/app/lifecycle/cache.rs:78:                self.develop_ui.loading_thumbnail.texture = Some(texture);` - the same seven lines, only the indentation and the `desktop_`/`android_` fetch differ. `grep -n 'loading_thumbnail.texture = Some(texture);' crates/calibraw-ui/src/app/lifecycle/cache.rs` returns 34, 78, 139, 153 (only these four sites in the workspace).
- why it is a problem: the three fields that must be written together (`texture`, `texture_size`, and the derived `adaptive_preview_backdrop`) are re-stated four times, so a fifth call site, or a change to how the backdrop is derived, has to be repeated and can leave `texture` and `texture_size` out of step.
- fix: one private helper on `CalibRawApp` taking the `(texture, size, adaptive_backdrop)` triple and writing the three fields; the four call sites become one call.
- effort: S
### [UI-008] Four copies of the same "batch error or clear task, then report" sequence inside one export worker
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/export.rs:602-610, :639-647, :655-663, :671-686 (all inside `poll_export_worker`, declared at crates/calibraw-ui/src/app/processing_export/export.rs:472)
- evidence:
  `crates/calibraw-ui/src/app/processing_export/export.rs:602:                                                if is_batch {` then `android_batch_result = Some(Err(error.clone()));` `} else { self.export.task = None; }` `self.report_error(ErrorKind::Export, format!("Export failed: {error}"));`
  Repeated verbatim (indentation and 254/324-level nesting differ) at :655 - `crates/calibraw-ui/src/app/processing_export/export.rs:655:                                        if is_batch {` with the same four following statements. `grep -n 'android_batch_result = Some(Err(error.clone()));' export.rs` returns 603, 640, 656, 672; `grep -c 'Export failed: {error}'` returns 5 occurrences in the file.
- why it is a problem: an export failure path is maintained in four places, six indentation levels deep, so a change to how a failed export is reported to a batch versus a single export (or to the message) can miss one copy, and the copies cannot be reviewed against each other.
- fix: one local `record_export_failure(error)` closure (or private method on `CalibRawApp`) that performs the `is_batch` choice and the `report_error` call, used by all four failure arms.
- effort: S
### [CORE-002] `weight_for_srgb` is a one-line wrapper that only test code calls
- category: unnecessary-code
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/point_color.rs:141
- evidence:
  `crates/calibraw-core/src/pipeline/point_color.rs:141:    pub fn weight_for_srgb(&self, rgb: [f32; 3]) -> f32 {`
  `crates/calibraw-core/src/pipeline/point_color.rs:142:        self.weight_for_hsl(srgb_to_hsl(rgb))`
  `grep -rn 'weight_for_srgb' crates xtask --include='*.rs'` returns only this definition plus `crates/calibraw-gpu/src/pipeline/gpu/point_color_tests.rs:273:    assert!(point.weight_for_srgb(noisy) < 0.05);` - no production caller, and `grep -rn 'weight_for_hsl'` shows the callers that matter use the HSL form directly.
- why it is a problem: the public API of `PointColor` carries a conversion shortcut that exists to feed assertions in GPU test code, so the crate publishes a colour-space entry point nothing in the app uses.
- fix: call `weight_for_hsl(srgb_to_hsl(..))` in the tests and drop the wrapper, or keep it `#[cfg/test]`-adjacent to the tests that need it.
- effort: S
### [CORE-003] The same bilinear sampling block re-implemented in three crates
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/geometry/straighten/luminance.rs:203-216 (`sample_bilinear`), crates/calibraw-core/src/pipeline/lensfun/backend/correction.rs:351-365, crates/calibraw-gpu/src/pipeline/gpu/remove_scene.rs:63-79, crates/calibraw-ai/src/models/remove/retouch.rs:521-537
- evidence:
  identical lines in all four (only the surrounding finite/zero-size guard and the pixel decode differ):
  `let x = x.clamp(0.0, (width - 1) as f32);` … `let x0 = x.floor() as usize; let y0 = y.floor() as usize; let x1 = (x0 + 1).min(width - 1); let y1 = (y0 + 1).min(height - 1); let tx = x - x0 as f32; let ty = y - y0 as f32;`
  then the same `top`/`bottom` blend: `values[y0 * width + x0] * (1.0 - tx) + …` / `top * (1.0 - ty) + bottom * ty` in `luminance.rs:213-216`, `correction.rs:361-365`, `remove_scene.rs`, `retouch.rs:532-537`.
- why it is a problem: one interpolation rule (edge clamp, corner indices, fractional weights) is written in four places across three crates, so a boundary change - e.g. treating a one-pixel-wide patch as clamped rather than indexing `width - 1` - has to be found and changed four times, and today the four already disagree on their finite and zero-size guards.
- fix: one shared bilinear sampler in `calibraw-core` (`pipeline/geometry`) taking the corner positions and a `sample(x, y)` closure, used by all four call sites; `calibraw-gpu` and `calibraw-ai` already depend on `calibraw-core`.
- effort: M
### [CORE-004] Loader fallback pair and four forwarding closures that only re-order arguments
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader.rs:765-771 and :777-783 (wrapper definitions); call sites :813-819, :821-827, :831-837, :839-845 inside `load_raw_file_with_profile_selection` (declared :802)
- evidence:
  `raw_loader.rs:765: fn try_rawler_then_libraw<T>(` ... `:770:     try_with_fallback(path, operation, ("Rawler", rawler), ("LibRaw", libraw))`
  `raw_loader.rs:777: fn try_libraw_then_rawler<T>(` ... `:782:     try_with_fallback(path, operation, ("LibRaw", libraw), ("Rawler", rawler))`
  and at the call sites the two branches differ only by which loader comes first: `rawler_loader::load_raw_file_with_profile_selection(path, mode, profile_folder, selected_profile,)` wrapped in `|| { ... }` four times.
- why it is a problem: adding a loader or a parameter to `load_raw_file_with_profile_selection` means editing six forwarding sites, and the two wrappers make it unclear that they are the same call with swapped order.
- fix: call `try_with_fallback` directly from `load_raw_file_with_profile_selection` with a `(name, decode-order)` input, keeping the loader names in one place; the four `|| { ... }` blocks become one helper that takes the loader function.
- effort: M
### [CORE-005] `raw_controls_changed` compares fourteen `ExposureParams` fields one line at a time
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/processing.rs:39-54 (`raw_controls_changed`, called from `affected_stage` at :30)
- evidence:
  `crates/calibraw-core/src/pipeline/processing.rs:40:    before.black_point != after.black_point`
  `crates/calibraw-core/src/pipeline/processing.rs:41:        || before.chroma_denoise != after.chroma_denoise`
  `crates/calibraw-core/src/pipeline/processing.rs:42:        || before.luminance_denoise != after.luminance_denoise`
  `crates/calibraw-core/src/pipeline/processing.rs:53:        || before.chromatic_aberration != after.chromatic_aberration`
  `timeout 120 grep -rn 'before\.[a-z_]* != after\.' crates --include='*.rs'` returns 15 hits, 14 of them in these lines - the only such chain in the workspace, and it is the whole body of the function.
- why it is a problem: `ExposureParams` carries about 30 fields, so a new RAW control is silently not compared here, which makes `affected_stage` return `None` and leave an already-rendered stage stale; the compiler cannot see the omission.
- fix: group the RAW controls into a named sub-struct with derived `PartialEq` and compare it once (keep sidecar keys stable with `#[serde(flatten)]`), so the field list exists in one place.
- effort: M
### [CORE-006] A failing DCP profile-folder search is logged and then dropped as "no profiles"
- category: temp-or-hardcoded
- severity: high
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:97-107 (inside `resolve_camera_profiles`, declared :80)
- evidence:
  `crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:97:        .unwrap_or_else(|error| {`
  `crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:99:                log::warn!(`
  `crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:101:                    "could not search DCP profile folder {}: {error:#}",`
  `crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:105:            None`
  `crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:107:        .unwrap_or_default();`
  `grep -n 'log::warn\|log-only' finding.md docs/rework/AUDIT.md` returns nothing; docs/DEVELOPMENT.md states "no failure visible only in the logs".
- why it is a problem: an unreadable or renamed DCP folder becomes the empty profile list, so the user sees "no available camera profiles" and a default look with no error dialog, and the `if let Some(folder)` guard above it means the log line is skipped entirely when `profile_folder` is `None`, hiding the failure even in the logs.
- fix: propagate the search error as part of the result (e.g. return the folder-search failure alongside the profiles, as the existing `LoadedSidecar::migrated`-style notice does) and let the UI report it through `CalibRawApp::report_error`.
- effort: M
### [CORE-007] 59 production sites hand-roll the same `Instant` + "finished in {…}s" timing record
- category: duplicate
- severity: medium
- confidence: high
- where: representative sites crates/calibraw-core/src/pipeline/raw_loader/libraw_loader.rs:136-140, :162-166, :180-184, :198-202; crates/calibraw-ui/src/app/lifecycle/document_load.rs:135-137, :268-270; crates/calibraw-gpu/src/pipeline/export/streaming.rs:164, :225 (37 production files in total)
- evidence:
  `let profile_started = Instant::now();` followed later by `calibraw_core::diagnostics::record(format!("Camera-profile resolution finished in {:.3}s", profile_started.elapsed().as_secs_f64()));`
  `timeout 120 grep -rn 'elapsed().as_secs_f64()' crates --include='*.rs'` (excluding tests) returns 59 sites; `grep -rn 'diagnostics::record' crates` returns 126 sites in 37 files.
  `crates/calibraw-core/src/diagnostics.rs:26:    fn push(&mut self, message: &str) {` already prefixes every event with `"[+{:>5}.{:03}s] "` from the session start.
- why it is a problem: the elapsed-since-session-start stamp is printed twice for the same event, and the two-statement timing pattern (start marker plus a formatted elapsed message) has to be maintained at 59 sites, where the module already owns the clock.
- fix: add `diagnostics::elapsed(label: str, started: Instant)` (or `time(label, work)`) that formats the duration, and replace the `format!("… in {:.3}s", …)` spellings with it.
- effort: M
### [UI-009] Sidecar save drain blocks the caller with a sleep loop behind an unnamed 2 s deadline
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/sidecar_persistence.rs:449-468 (drain loop in the save pump)
- evidence:
  `crates/calibraw-ui/src/app/sidecar_persistence.rs:449:        let deadline = Instant::now() + Duration::from_secs(2);`
  `crates/calibraw-ui/src/app/sidecar_persistence.rs:458:                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {`
  `crates/calibraw-ui/src/app/sidecar_persistence.rs:461:                std::thread::sleep(remaining.min(Duration::from_millis(10)));`
  `crates/calibraw-ui/src/app/sidecar_persistence.rs:464:            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {`
  `grep -rn 'from_millis(10)\|from_secs(2)' crates` shows these are the only two such durations in the workspace, so nothing names them.
- why it is a problem: the caller stops for up to two seconds in 10 ms steps with no explanation of the limit or the step size, and the same three-line deadline guard is written twice, once per branch, so one branch can start counting a different deadline than the other.
- fix: hoist `remaining` (and its `else break`) to the top of the loop body, and name both durations (`SIDECAR_DRAIN_DEADLINE`, `SIDECAR_POLL_STEP`) with a comment stating that this is the shutdown drain window.
- effort: S
### [AI-001] Download retry backoff is an unnamed sleep ladder
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-ai/src/registry/artifact.rs:391-401 (`retry_backoff`)
- evidence:
  `crates/calibraw-ai/src/registry/artifact.rs:393:    let mut remaining = Duration::from_secs(1u64 << attempt.min(3));`
  `crates/calibraw-ai/src/registry/artifact.rs:396:        let sleep_for = remaining.min(Duration::from_millis(100));`
  `crates/calibraw-ai/src/registry/artifact.rs:397:        std::thread::sleep(sleep_for);`
  `grep -rn 'retry_backoff' crates --include='*.rs'` shows one definition and its call sites in this file; no constant in the crate names the base delay, the cap or the step.
- why it is a problem: the base delay (1 s), the exponential cap (`attempt.min(3)`, i.e. 8 s) and the 100 ms cancellation step are three unexplained literals, so nobody can tell how long a cancelled model download may keep the thread waiting or whether the cap was chosen deliberately.
- fix: name the three durations as module constants with a one-line rationale for the cap; keep the slice loop (it exists so `cancellation()` runs during the wait) and comment that once.
- effort: S
### [GPU-001] Seven readback entry points take `x, y, width, height` as loose `u32`s while the module owns a region type
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/readback.rs:529, :553, :565, :576, :617, :644 and :311 (six `pub fn` entry points plus an internal helper)
- evidence:
  `crates/calibraw-gpu/src/pipeline/gpu/readback.rs:526:    pub fn read_output_region_blocking(` then `device: &wgpu::Device, queue: &wgpu::Queue, x: u32, y: u32, width: u32, height: u32,`
  and the very next statements re-pack those four scalars into the type the module already defines:
  `crates/calibraw-gpu/src/pipeline/gpu/readback.rs:4: pub(super) struct TextureReadbackRegion {` with `origin: [u32; 2], extent: [u32; 2], texture_extent: [u32; 2]`
  `grep -rn '^\s*x: u32,$' crates --include='*.rs'` (tests excluded) returns 13 sites, six of them in this file's public API.
- why it is a problem: at six call boundaries the caller can pass `(width, height, x, y)` or an extent where an origin belongs and still compile; the readback then returns the wrong pixels of an export tile with no type error.
- fix: take `TextureReadbackRegion` (or an origin/extent pair type) as one parameter in these entry points instead of four scalars; the `device`/`queue` pair can stay.
- effort: M
### [UI-010] Three startup paths turn a resolution failure into `None` behind `map_err(log).ok()`
- category: temp-or-hardcoded
- severity: high
- confidence: high
- where: crates/calibraw-ui/src/app/lifecycle/startup.rs:419-421 and :424-426; crates/calibraw-ui/src/app/ai_denoise.rs:38-40
- evidence:
  `crates/calibraw-ui/src/app/lifecycle/startup.rs:419:            .map_err(|error| log::warn!("{error}"))`
  `crates/calibraw-ui/src/app/lifecycle/startup.rs:420:            .ok()`
  `crates/calibraw-ui/src/app/lifecycle/startup.rs:421:            .flatten();`
  same three-call shape at `:424` for `gpu_pipeline_cache_dir()`; at `crates/calibraw-ui/src/app/ai_denoise.rs:38:                .map_err(|error| log::warn!("{error}"))` the value is dropped with `.ok(),` and the result becomes `None`.
  `timeout 120 grep -rn 'map_err(|error| log::warn' crates --include='*.rs'` returns these three plus one site that does label the failure (`crates/calibraw-core/src/system_memory.rs:88`).
- why it is a problem: when `performance_settings_path()` fails, the app continues with `None`, which makes the AI/ONNX migration steps at `startup.rs:441` run without a settings folder and the GPU cache fall back to a default location - a failure the user only ever sees as a bare log line, against the project rule that failures are reported through `CalibRawApp::report_error`.
- fix: return the failure (e.g. keep startup's `Vec<String>` failure channel that already exists for `moved_ai_file_failures`) and show it through `report_error`; the `map_err(...).ok().flatten()` chain becomes a plain `match`/`?` that keeps the message.
- effort: M
### [UI-011] 20 production failure logs carry only the message, never the operation that failed
- category: unnecessary-code
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/app/lifecycle/settings.rs:11, :367; crates/calibraw-ui/src/app/lifecycle/profiles.rs:73, :80; crates/calibraw-ui/src/app/lifecycle/pickers.rs:157; crates/calibraw-ui/src/app/lifecycle/startup.rs:437, :462, :486; crates/calibraw-ui/src/app/processing_export/export/task_view.rs:25, :53 (and 10 further sites)
- evidence:
  `crates/calibraw-ui/src/app/lifecycle/settings.rs:11:            log::warn!("{error}");`
  `crates/calibraw-ui/src/app/lifecycle/profiles.rs:73:                    log::warn!("{error}");`
  `timeout 120 grep -rn 'log::warn!("{error}")' crates --include='*.rs'` (tests excluded) returns 20 sites; every labelled variant spells the operation, e.g. `crates/calibraw-ui/src/app/lifecycle/prefetch.rs:86: Err(error) => log::warn!("could not start the neighbour prefetch: {error}"),`.
- why it is a problem: the log line says only what the error text was, so the file that failed, the operation and the app state are unrecoverable from the log, which is the one place these failures are visible at all.
- fix: prefix each with the operation and the affected value, following the `prefetch.rs:86` wording, e.g. `"could not set the light system bars: {error}"`.
- effort: S
### [FFI-001] Three `with_*` JNI accessors differ only in the Java field they fetch
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ffi/src/android.rs:435-449 (`with_storage_manager`), :451-465 (`with_profile_importer`), :467-… (`with_export_publisher`)
- evidence:
  `crates/calibraw-ffi/src/android.rs:435:fn with_storage_manager<T>(` … body is
  `            .get_field(` / `jni::jni_str!("storageManager"),` / `jni::jni_sig!(de.duecki.calibraw.StorageManager),` / `)?` / `.l()?;` / `operation(env, &storage_manager)`
  and the two other functions are byte-identical except for `profileImporter`/`ProfileImporter` and `exportPublisher`/`ExportPublisher`; `timeout 120 grep -rn 'fn with_' crates/calibraw-ffi/src --include='*.rs'` lists these three plus the `with_activity` base at `:423`.
- why it is a problem: 45 export sites depend on these accessors, and adding another Java object means copying another 15-line function, so the class names and the field names that must agree with the Kotlin side are spread over three copies.
- fix: one `with_named_object(app, field: &'static str, signature: jni::jni_sig!(...), operation)` helper (the signature is a macro literal, so the table can hold `(field_name, sig)` pairs per call site) and have the three named wrappers call it.
- effort: S
### [XTASK-001] Named xtask constants exist but the same values are re-spelled five times
- category: duplicate
- severity: low
- confidence: high
- where: xtask/src/android.rs:94, :130, :257, :264 (ABI) and :487 (cargo-ndk version)
- evidence:
  `xtask/src/android.rs:21:const ANDROID_64_BIT_ABIS: [&str; 1] = ["arm64-v8a"];` is used only at `xtask/src/android.rs:669: if !ANDROID_64_BIT_ABIS.contains(&abi) {`, while `xtask/src/android.rs:94:            .unwrap_or_else(|| "arm64-v8a".to_owned()),` and `xtask/src/android.rs:130:        abi: abi.unwrap_or_else(|| "arm64-v8a".to_owned()),` and `xtask/src/android.rs:257:        "arm64-v8a" => Ok((` re-spell the literal.
  `xtask/src/android.rs:22:const CARGO_NDK_VERSION: &str = "4.1.2";` is used at `:498` and `:500`, but `xtask/src/android.rs:487:        "cargo-ndk 4.1.2 is required. Install it with: cargo install cargo-ndk --version 4.1.2 --locked",` spells the version twice.
- why it is a problem: bumping the supported ABI or the cargo-ndk version means finding five literals, and the `--help`/usage text then disagrees with the check that actually runs.
- fix: use `ANDROID_64_BIT_ABIS[0]` (or one `const DEFAULT_ABI: &str = "arm64-v8a"` that the array contains) in the four ABI spots and format `CARGO_NDK_VERSION` into the install hint.
- effort: S
### [CORE-008] CPU `MaskStack` layer rasterization API has no production caller
- category: unnecessary-code
- severity: low
- confidence: medium
- where: crates/calibraw-core/src/pipeline/masks/layers.rs:262 `rasterize_layer`, :276 `rasterize_layer_f16`, :294 `rasterize_layer_region`, :309 `rasterize_layer_region_f16`
- evidence:
  `grep -rn 'rasterize_layer' . --include='*.rs' --exclude-dir=target` (whole repo, `crates/calibraw-core/src/pipeline/masks/layers.rs` excluded) returns only `crates/calibraw-core/benches/mask_rasterization.rs:173: black_box(stack.rasterize_layer_f16(layer, edge, edge, image_width, image_height));` plus `pipeline/masks/effects/tests.rs:162`, `pipeline/masks/lens_tests.rs:209` and `pipeline/masks/zoom_tests.rs` - i.e. benches and `*_tests.rs` only.
  `grep -rn 'MaskStack' crates/calibraw-gpu/src` shows the GPU consumers (`pipeline/gpu/mask_layers.rs:86`, :106 and `pipeline/export/streaming.rs:456`) never call these four entry points.
- why it is a problem: the library keeps four public CPU rasterization entry points that only the test and bench harnesses exercise, so their cost and API surface look like part of the app pipeline while nothing in the app calls them.
- fix: keep the `pub(super)` coverage helpers the tests need and move the four `pub` entry points into the bench/test support that calls them, or document the one intended production caller.
- effort: S
### [CORE-009] `raw_controls_changed` lists 14 `ExposureParams` fields by hand next to a derived `==`
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/processing.rs:40-56 (`raw_controls_changed`), called at :33
- evidence:
  `crates/calibraw-core/src/pipeline/processing.rs:24:    if before == after {` (the derived `PartialEq` of `ExposureParams`, `crates/calibraw-core/src/pipeline/basicadj.rs:256: pub struct ExposureParams {` - about 40 fields)
  and directly below, the second, hand-maintained comparison of a subset:
  `crates/calibraw-core/src/pipeline/processing.rs:40:fn raw_controls_changed(before: &ExposureParams, after: &ExposureParams) -> bool {`
  `before.black_point != after.black_point || ... || before.chromatic_aberration != after.chromatic_aberration` (14 comparisons).
  `grep -rn 'raw_controls_changed' crates` returns only this definition and the one call at :33, so no test pins the pair.
- why it is a problem: adding a RAW-stage control to `ExposureParams` updates `before == after` automatically but leaves the 14-line list untouched, so the change is reported as `ProcessingStage::Output` and the RAW reconstruction stage keeps the old values - a stale image with no error.
- fix: group the RAW-stage controls into a named sub-struct with `#[serde(flatten)]` (serialized keys stay the same) and derive `PartialEq` on it, then compare that sub-struct here; the derived `ExposureParams::PartialEq` keeps covering the rest.
- effort: M
### [XTASK-002] Every xtask argument parser repeats the same `--help` block
- category: duplicate
- severity: low
- confidence: high
- where: xtask/src/android.rs:65, :112, :155; xtask/src/loc.rs:44; xtask/src/colorchecker.rs:30; xtask/src/main.rs:79
- evidence:
  `xtask/src/android.rs:65:            "--help" | "-h" => {` followed by `print_help();` and `std::process::exit(0);` (identical at :112 and :155); `xtask/src/loc.rs:44:            "--help" | "-h" => {` repeats the same two calls; `xtask/src/colorchecker.rs:30:            "--help" | "-h" if !positional_only => {` repeats them with an extra guard; `xtask/src/main.rs:79:        "help" | "--help" | "-h" => {` is the dispatcher's own copy.
  `timeout 60 grep -rn 'print_help' xtask/src --include='*.rs'` returns 6 such sites besides the definition.
- why it is a problem: the exit path for the help text exists in five argument parsers, so one subcommand can keep printing help and exiting 0 while another one forgets to, and adding a subcommand means copying the block again.
- fix: give the parsers one shared `if argument == "--help" | ... -> help` pre-pass in `xtask/src/main.rs` (or a small `help_for(&args)` helper) that every parser calls instead of inlining `print_help(); exit(0)`.
- effort: S
### [CORE-010] Sidecar value ranges are 27 unnamed literals inside `validate_edit_state`
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/sidecar/validation.rs:81-83, :115-117, :141-145, :188-190, :214-218, :235 and 20 more `bounded(...)` calls
- evidence:
  `crates/calibraw-core/src/sidecar/validation.rs:141:            bounded("subject refinement dab x", dab.center[0], -16.0, 16.0)?;`
  `crates/calibraw-core/src/sidecar/validation.rs:214:                        bounded("brush dab x", dab.center[0], -16.0, 16.0)?;`
  `crates/calibraw-core/src/sidecar/validation.rs:83:                bounded("Remove brush radius", point.radius, 0.0, 100_000.0)?;`
  `grep -oE '\-?[0-9_.]+, \-?[0-9_.]+\)\?' ` over the production part of the file returns `0.0, 1.0)?` 19 times, `-16.0, 16.0)?` 12 times, `0.0, 16.0)?` 10 times, `-1.0, 1.0)?` 7 times and `1_000_000.0` 4 times.
  The same file names the other limits: `crates/calibraw-core/src/sidecar/validation.rs:681: if width == 0 || height == 0 || width > MAX_MASK_IMAGE_EDGE || height > MAX_MASK_IMAGE_EDGE {` with `crates/calibraw-core/src/sidecar.rs:50: const MAX_MASK_IMAGE_EDGE: u32 = 8192;`.
- why it is a problem: nothing says what `16.0` means (mask UV space), what `100_000.0` guards, or why a brush radius and a Remove patch coordinate use different sentinel limits, so the guardrails that protect against a hand-edited sidecar cannot be reviewed or changed as a pair with the UI's own ranges.
- fix: name the domains once (`NORMALIZED_UNIT: f32 = 1.0`, `MASK_UV_LIMIT: f32 = 16.0`, `MASK_EXTENT_LIMIT: f32 = 16.0`, `PIXEL_COORDINATE_LIMIT: f32 = 100_000.0`) with a unit on each, and use them in `bounded(...)`; `MAX_MASK_IMAGE_EDGE` already shows this style.
- effort: M
### [CORE-011] Four mask-raster entry points repeat the same loose u32 dimension pack
- category: bad-code
- severity: medium
- confidence: medium
- where: crates/calibraw-core/src/pipeline/masks/layers.rs:161-164, :179-182, :265-268, :279-282 (plus crates/calibraw-core/src/pipeline/masks/brush.rs:43-46 and crates/calibraw-ai/src/models/remove/retouch.rs:130, :153, :189)
- evidence:
  `crates/calibraw-core/src/pipeline/masks/layers.rs:161:        atlas_width: u32,` (followed by `atlas_height: u32, image_width: u32, image_height: u32`) - the same four-parameter tail appears at :179, :265 and :279, and `layers.rs:147: pub fn raster_margin_pixels(&self, image_width: u32, image_height: u32) -> u32 {` takes two of them again.
  `timeout 120 grep -rn 'image_height: u32' crates --include='*.rs'` (tests and benches excluded) returns 26 such parameters across the workspace, so no type models this (atlas extent, image extent) pair.
- why it is a problem: a caller that swaps `atlas_height` for `image_width` still compiles, and the values then index a coverage buffer of a different length; nothing in the signatures says which of these are atlas pixels and which are full-image pixels.
- fix: introduce one `MaskRasterSpace`-style struct (it already exists for the sampler: `crates/calibraw-core/src/pipeline/masks/brush.rs:56: pub(super) fn rasterize_brush(space: MaskRasterSpace, dabs: &[BrushDab]) -> Vec<f32>`) holding the atlas and image extents with named fields and pass that instead of four scalars.
- effort: M

## Top 15

Ranked by severity x confidence, then by smaller effort. All entries below are recorded in this report.

| # | ID | finding |
|---|----|---------|
| 1 | UI-010 | high/high - startup turns a settings-path failure into `None` behind `map_err(log).ok()`, so a failure the user can act on is visible only in a log |
| 2 | CORE-006 | high/high - a failed DCP folder search becomes "no available camera profiles" with a log line only |
| 3 | FFI-001 | medium/high, S - three `with_*` JNI accessors differ only in the Java field name |
| 4 | UI-001 | medium/high, S - one user-visible notice written twice in two views |
| 5 | UI-003 | medium/high, S - a notice out-parameter is passed as `&mut None` by one caller, so it is dropped silently |
| 6 | UI-006 | medium/high, S - one five-line guard written four times in the same match |
| 7 | UI-007 | medium/high, S - the same seven-line thumbnail install block written four times |
| 8 | UI-008 | medium/high, S - the same five-line export failure handling written four times, five to six levels deep |
| 9 | CORE-001 | medium/high, S - `32_768` re-spelled where named limits already exist |
| 10 | GPU-001 | medium/high, M - readback entry points take `x, y, width, height` while the module owns `TextureReadbackRegion` |
| 11 | CORE-003 | medium/high, M - the same bilinear sampling block in three crates with disagreeing guards |
| 12 | CORE-004 | medium/high, M - loader fallback pair plus four closures that only re-order arguments |
| 13 | CORE-005 | medium/high, M - 14 hand-written field comparisons beside the derived `==` (see CORE-009) |
| 14 | CORE-007 | medium/high, M - 59 production sites re-implement the same timing record the diagnostics module already stamps |
| 15 | UI-009 | medium/high, M - a sleep loop with an unnamed 2 s deadline blocks the caller during sidecar drain |

CORE-009 restates CORE-005 (same function, `crates/calibraw-core/src/pipeline/processing.rs:40`), so it is excluded from the ranking; it only adds the derived-`PartialEq` context that CORE-005 already quotes.

## Systemic patterns

1. **Duplicated guard and handling blocks inside one long function**: UI-004 (nine arms), UI-006 (four), UI-007 (four), UI-008 (four), XTASK-002 (six parsers), CORE-004 (four closures), FFI-001 (three accessors). Each copy is where a new variant or subcommand has to be edited, so the copies are where they silently diverge.
2. **Geometry carried as loose scalars even where a region type exists**: GPU-001 (`TextureReadbackRegion` vs six `u32` parameters), CORE-011 (`MaskRasterSpace` already takes exactly the four extents), CORE-003 (the same bilinear block copied instead of sampled once). A transposed pair compiles and returns wrong pixels.
3. **Hand-maintained lists that must mirror a type-level fact**: CORE-005/CORE-009 (14 compared fields vs derived `PartialEq` over ~40), CORE-010 (27 range literals that must match the UI's sliders), XTASK-001 (values whose named constant exists but is not used). The compiler cannot see an omission in any of them.
4. **Failures that reach only the log**: CORE-006, UI-010, UI-011 (20 bare `log::warn!("{error}")`), UI-003 (an out-parameter notice discarded). Four of the six places the app can lose data or a setting show nothing in the dialog channel.
5. **Time kept in two places**: CORE-007 (59 hand-rolled `Instant` pairs against `diagnostics.rs:26` elapsed prefix), AI-001 and UI-009 (unnamed backoff, deadline and poll step as literals).

## Not checked

- `calibraw-ui` files beyond the two chunks of `ui/sidebar/masks/properties.rs` that were read in full: `ui/presets.rs`, `app/library_adjustments.rs`, `ui/sidebar/adjustment_cards.rs`, `ui/library/actions.rs`, `ui/preview/viewport.rs`, `app/processing_export/batch.rs`, `ui/sidebar/develop.rs`, `ui/library/review.rs`, `ui/library/mod.rs`, `app/version_update.rs`, `app/ai/state.rs` (their repeated blocks were checked only with grep).
- `calibraw-gpu` production bodies: `pipeline/gpu/resources.rs`, `gpu.rs`, `params.rs`, `builder/passes.rs`, `pipeline/export/streaming.rs` were scanned for repeated lines and parameter packs, not read line by line; `pipeline/gpu/builder/layouts.rs` and `bind_groups.rs` were not scanned at all (they are F-24/F-34 territory).
- `calibraw-ai` beyond `registry/artifact.rs:391` and `models/remove/retouch.rs:521`: `models/denoise.rs`, `models/object.rs`, `runtime/package.rs`, `registry/*`, `config.rs`.
- WGSL shaders: only the function-name uniqueness check across `crates/calibraw-gpu/src/shaders` was run; no shader body was read.
- `calibraw-cli`, `scripts/`, root files, Gradle and Android Kotlin: outside the six-crate order requested, or already covered by finding.md.
- Test code, generated `bindings.rs`, `target/` (429 GB), `docs/`, `Cargo.lock`.
- Nothing was compiled, so "no production caller" claims (CORE-002, CORE-008) rest on the grep results quoted in those entries.

## Corrections to quoted evidence

Recorded entries are not rewritten, so these three slips are noted here instead:

- CORE-006: the claim that the `if let Some(folder)` guard at `libraw_loader.rs:100` skips the `log::warn!` when `profile_folder` is `None` is wrong - `.map()` on `None` never reaches the error path, so the guard is only defensive. The finding itself still holds: an unreadable or renamed DCP folder yields an empty profile list and a log line only.
- UI-010: `gpu_pipeline_cache_root` is only passed on at `startup.rs:469`; the sentence that the GPU cache "falls back to a default location" was not verified downstream. The verified part is that the failure becomes `None` and is reported only through `log::warn!`.
- CORE-009: `if before == after {` is at `crates/calibraw-core/src/pipeline/processing.rs:29`, not `:24` as quoted; `affected_stage` starts at :28.
### [UI-012] Card help is looked up by comparing the display label back to `MaskEffect::ALL`
- category: bad-code
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:115-123 (`adjustment_card_title`), called from crates/calibraw-ui/src/ui/sidebar/mask_effects/mod.rs:78
- evidence:
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:115:    pub(super) fn adjustment_card_title(ui: &mut Ui, title: &str) {`
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:118:            .find(|effect| effect.label() == title)`
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:119:            .and_then(|effect| mask_effects::effect_description(*effect))`
  `crates/calibraw-ui/src/ui/sidebar/mask_effects/mod.rs:78:                    super::Sidebar::adjustment_card_title(ui, effect.label());`
  The other two callers, `adjustment_cards.rs:597` and :617, pass a card `title` that is never a `MaskEffect` label, so they take the `if let Some(help)` miss path.
- why it is a problem: the caller already holds the `MaskEffect`, so the value is turned into a label, compared against all twelve labels and mapped back; if a label and a title ever drift apart, the help text disappears with no compile error and no test that pins the pair.
- fix: take the effect as a parameter (`fn mask_effect_card_title(ui: &mut Ui, effect: MaskEffect)`) or pass the already-resolved `mask_effects::effect_description(effect)` into the generic title helper, so no string comparison is needed.
- effort: S
- UI-012: `MaskEffect::ALL` has 18 entries (`crates/calibraw-core/src/pipeline/masks/effects/mod.rs:69: pub const ALL: [Self; 18] = [`), not twelve; the entry's "all twelve labels" should read "all 18 labels".
### [AI-002] The same `DevelopedCropJob` is rebuilt field by field three times
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ai/src/models/remove.rs:169-177 and :184-192; crates/calibraw-ai/src/models/remove/retouch.rs:21-29
- evidence:
  `crates/calibraw-ai/src/models/remove.rs:169:                device: request.device.clone(),`
  `crates/calibraw-ai/src/models/remove.rs:170:                queue: request.queue.clone(),`
  `crates/calibraw-ai/src/models/remove.rs:171:                raw: Arc::clone(&request.raw),`
  `crates/calibraw-ai/src/models/remove.rs:173:                exposure: request.exposure,`
  `crates/calibraw-ai/src/models/remove.rs:174:                masks: request.masks.clone(),`
  `crates/calibraw-ai/src/models/remove/retouch.rs:21:                device: request.device.clone(),`
  The three blocks are identical except for the `remove:` and `crop:` lines.
- why it is a problem: nine fields have to be listed at every render call, so a new field on `DevelopedCropJob` (there is room for one between `program_prewarm` and the closing brace) can be left out of one of the three crops and that pass then renders with a default while the other two do not; the repeated `request.masks.clone()` also copies the mask stack per call instead of borrowing it.
- fix: one `DevelopedCropJob::from_request(&request)` (or `request.with(crop, remove)`) that takes the shared parts by reference where possible, and let the call sites set only `crop` and `remove`.
- effort: M
- AI-002: the sentence about a new field being "left out of one of the three crops" is wrong - Rust requires every field at each construction, so the compiler catches that. What remains is the three-fold repetition of nine field assignments and the `request.masks.clone()` copy made at every render call instead of borrowing.
### [AI-003] Three mask-refine filters copy the same dimension guard, and two of the copies name the wrong function
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ai/src/models/mask_refine.rs:147-148 (`guided_filter`), :179-182 (its `guided_filter_by_luminance` callee), :239-242 (`self_guided_filter`)
- evidence:
  `crates/calibraw-ai/src/models/mask_refine.rs:148:        "guided-filter image dimensions mismatch"` (inside `guided_filter`)
  `crates/calibraw-ai/src/models/mask_refine.rs:182:        "guided-filter image dimensions mismatch"` (inside the `pub(super) fn guided_filter_by_luminance` that `:150` calls)
  `crates/calibraw-ai/src/models/mask_refine.rs:242:        "guided-filter image dimensions mismatch"` (inside `pub(super) fn self_guided_filter`, `crates/calibraw-ai/src/models/mask_refine.rs:232`)
  Each copy is preceded by the same two statements: `let pixels = width as usize * height as usize;` and `if pixels == 0 { return Ok(()); }`.
  `grep -n 'dimensions mismatch' crates/calibraw-ai/src/models/mask_refine.rs` returns only these three sites.
- why it is a problem: the error a user sees when a self-guided mask filter gets a mismatched buffer says "guided-filter", which points at the wrong pass in the log and in the dialog detail, and the three guards must be edited together if any filter starts accepting a different extent.
- fix: one `ensure_pixels(label, buffers, pixels)` helper in this module that formats the failing buffer lengths, called with each function's own name, so the message cannot drift from the caller.
- effort: S

## Addenda after the closing summary

Three more entries were recorded after the summary blocks above, so they are not reflected in it:

- UI-012 (`crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:115`) - card help resolved by comparing a display label against `MaskEffect::ALL`.
- AI-002 (`crates/calibraw-ai/src/models/remove.rs:169`) - `DevelopedCropJob` rebuilt field by field at three render calls.
- AI-003 (`crates/calibraw-ai/src/models/mask_refine.rs:148`) - the same dimension guard copied into three filter functions, two of them naming the wrong function in the message.

Delta to the Top 15 if re-ranked: AI-003 (medium/high, S) and AI-002 (medium/high, M) would take slots 10 and 15, moving GPU-001 and UI-009 to the runners-up list.

Delta to the Systemic patterns:
1. Loose scalar geometry: GPU-001, CORE-011, AI-002 and AI-003 all thread `(width, height, ...)` quads where a region or extent type exists or would be one line to add.
2. Copy-pasted error context: AI-003 (a message naming the wrong function copied three times), UI-011 (message names no function at all), CORE-006 and UI-010 (failure reaches only the log).
3. Field-by-field rebuild of a shared struct: AI-002, CORE-004, CORE-009 and CORE-005.

The Corrections block above lists the four evidence slips (CORE-006 guard, UI-010 GPU-cache clause, CORE-009 line number, UI-012 label count, AI-002 field omission).

Checked and found clean, so not reported: banner/separator comments (0 sites), doc comments that restate the function name (0 sites), production `unwrap()`/`expect()` without a stated invariant (the 6 in `ai/models/denoise/result_cache.rs` and 4 in `core/pipeline/processing/proxy.rs` all carry invariant strings), non-clippy `allow` attributes in production (3 sites, all documented, and `platform/mod.rs:16` is F-07), WGSL helper duplication across `crates/calibraw-gpu/src/shaders` (only `haze_depth_base` and `adjustment_base_at` occur twice, and AUDIT row 12 covers the duplicated-helper class), `TODO`/`FIXME`/`HACK`/`XXX` in production (0 sites), `target_os` inside `calibraw-ui` outside `platform/` (0 sites), and named-limit re-spelling other than CORE-001 and XTASK-001.
### [UI-013] The four `PreviewState` pending-stage fields are cleared by hand at 42 places
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/lifecycle/documents.rs:308-311 and :422-424; crates/calibraw-ui/src/app/processing_export/preview/rebuild.rs:399-402; crates/calibraw-ui/src/app/ai_denoise.rs:198-201 and :315-318; plus crates/calibraw-ui/src/app/processing_export/preview/detail.rs:11, :165, crates/calibraw-ui/src/app/processing_export/preview/processing.rs:89, :96, :167, :225, crates/calibraw-ui/src/app/processing_export/lens.rs:415 and crates/calibraw-ui/src/app/eframe_impl.rs:40
- evidence:
  `crates/calibraw-ui/src/app/lifecycle/documents.rs:308:        self.preview.pending_stage = None;`
  `crates/calibraw-ui/src/app/lifecycle/documents.rs:309:        self.preview.detail_pending_stage = None;`
  `crates/calibraw-ui/src/app/lifecycle/documents.rs:310:        self.preview.navigation_pending_stage = None;`
  `crates/calibraw-ui/src/app/lifecycle/documents.rs:311:        self.preview.detail_urgent = false;`
  The same four lines repeat verbatim at rebuild.rs:399, ai_denoise.rs:198 and ai_denoise.rs:315, and three of them repeat at documents.rs:422, detail.rs:11/165, processing.rs:89/96/167/225, lens.rs:415 and eframe_impl.rs:40.
  `grep -rn 'pending_stage = None' crates/calibraw-ui/src --include='*.rs'` (tests excluded) returns 42 assignments; `grep -rn 'detail_urgent = false' ...` returns 17.
  The fields live in one type: `crates/calibraw-ui/src/app/preview_state.rs:275:    pub(crate) detail_pending_stage: Option<ProcessingStage>,` through :285, and the owner already has reset helpers next to them: `crates/calibraw-ui/src/app/preview_state.rs:320:    pub(crate) fn reset_view(&mut self) {` (which clears `pending_view` but not the stages).
- why it is a problem: the four fields describe one state ("a stage is queued for the detail or navigation pass") but are kept consistent by hand across eight files, so a document switch or export that clears three of them leaves the fourth queued for the previous photo and the next layout spends a render pass on it.
- fix: add `PreviewState::reset_pending_stages()` (and a variant that also keeps the urgent flag) beside `reset_view`, call it from every reset site, and drop the `pub(crate)` on the fields so the state cannot be poked outside the owner.
- effort: M
### [UI-014] Transform helpers take the fields of `SourceProjection` apart again
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/preview/transform/crop_workspace.rs (13 functions) and crates/calibraw-ui/src/ui/preview/transform/affine.rs (3 functions)
- evidence:
  `crates/calibraw-ui/src/ui/preview/transform/source_projection.rs:31:pub(crate) struct SourceProjection<'a> {` with members `image_rect: Rect,` (:32) `geometry: GeometryTransform,` (:33) `lens: Option<&'a LensGeometryMap>,` (:36) `source_width: u32,` (:37) `source_height: u32,` (:38)
  `crates/calibraw-ui/src/ui/preview/transform/crop_workspace.rs:74:pub(in crate::ui::preview) fn crop_workspace_source_to_screen(` takes `image_rect: Rect,` :75, `geometry: GeometryTransform,` :76, `source_width: u32,` :77, `source_height: u32,` :78
  `grep -rn 'source_width: u32,' crates/calibraw-ui/src/ui/preview/transform --include='*.rs' | grep -v source_projection.rs` gives 16 signatures, all with the same order; `image_rect: Rect,` gives the same 16.
  Neighbouring modules already pass the struct: `crates/calibraw-ui/src/ui/preview/tools/brush.rs:11:    projection: SourceProjection<'_>,`, and the same in masks.rs:177, inpaint.rs:323, effect_handles/mod.rs:57.
- why it is a problem: the projection data has a type that 16 functions in the module that owns it do not use, so the lens map is not carried at all in those helpers and a call can swap width/height or pair an `image_rect` with another photo's geometry; adding the lens step later means editing 16 signatures and their call sites instead of one type.
- fix: change those signatures to take `projection: SourceProjection<'_>` (they already recompute `geometry.sanitized()` and `source_width.max(1)` internally), keeping the current parameter names as local destructurings inside if that reads better.
- effort: M
- UI-014: the exact counts are 17 signatures outside `source_projection.rs` that take `source_width: u32,` (14 in crop_workspace.rs, 3 in affine.rs), and 10 of those also take `image_rect: Rect,` separately - the entry's "16 ... the same 16" should read that way.
### [SHADER-001] The Gaussian disk sampler is copied between two mask-effect modules
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/shaders/mask_effects/film_finish.wgsl:25-34 and crates/calibraw-gpu/src/shaders/mask_effects/glow.wgsl:91-100
- evidence:
  `crates/calibraw-gpu/src/shaders/mask_effects/film_finish.wgsl:25:    const PAIRS: u32 = 80u;`
  `crates/calibraw-gpu/src/shaders/mask_effects/film_finish.wgsl:28:        let r = sqrt(-2.0 * log(1.0 - unit * 0.988891)) / 3.0;`
  `crates/calibraw-gpu/src/shaders/mask_effects/film_finish.wgsl:29:        let angle = f32(i) * 2.39996323;`
  `crates/calibraw-gpu/src/shaders/mask_effects/glow.wgsl:91:            const PAIRS: u32 = 80u;`
  `crates/calibraw-gpu/src/shaders/mask_effects/glow.wgsl:94:            let r = sqrt(-2.0 * log(1.0 - unit * 0.988891)) / 3.0;`
  `crates/calibraw-gpu/src/shaders/mask_effects/glow.wgsl:95:            let angle = f32(i) * 2.39996323;`
  `grep -rn '0.988891\|2.39996323\|PAIRS' crates/calibraw-gpu/src/shaders` returns only these two blocks; nothing in `common.wgsl` or the Rust side names these values, so no layout or contract test covers them.
- why it is a problem: the sample pattern's shape (80 antipodal pairs, the truncated-Gaussian radius mapping and the golden angle) is spelled twice, so changing the halation or glow sampling means editing two shaders and keeping them visually consistent, and neither copy documents what 0.988891 truncates at. docs/rework/AUDIT.md row 59 claims only `log_luminance` was duplicated in WGSL; that claim no longer holds.
- fix: add one helper to `crates/calibraw-gpu/src/shaders/common.wgsl` - `fn gaussian_disk_offset(index: u32, pairs: u32, radius: f32) -> vec2<f32>` - that both loops call, keeping each module's own emitter call inside the loop; the pair count and the two constants then exist once, and the truncation gets a comment.
- effort: S
### [SHADER-002] The tile-core clipping block is copied between the two analysis shaders
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:266-273 and crates/calibraw-gpu/src/shaders/haze_analysis.wgsl:184-191
- evidence:
  `crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:266:    let bounds = vec4<i32>(Common::camera_uniforms.tone_histogram_bounds);`
  `crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:268:    let tile_end = origin + vec2<i32>(`
  `crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:271:    let lo = max(max(vec2<i32>(span_x.x, span_y.x), bounds.xy), origin);`
  `crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:272:    let hi = min(min(vec2<i32>(span_x.y, span_y.y), bounds.zw), tile_end);`
  `crates/calibraw-gpu/src/shaders/haze_analysis.wgsl:184:    let bounds = vec4<i32>(Common::camera_uniforms.tone_histogram_bounds);`
  `crates/calibraw-gpu/src/shaders/haze_analysis.wgsl:189:    let lo = max(max(vec2<i32>(span_x.x, span_y.x), bounds.xy), origin);`
  The two sites also repeat the same comment with different wording: "Only this tile's core counts, so overlapping tile halos add nothing twice." (tone_analysis.wgsl:265) and "Only boxes starting in this tile's core count, so overlapping tile halos add nothing twice." (haze_analysis.wgsl:182).
  Both modules already import shared helpers (`Common::tile_origin()`), so a shared helper fits the existing pattern.
- why it is a problem: the tile-halo contract - which pixels a tiled pass may count - is spelled twice, so a change to how cores are clipped for one pass desynchronises the other and the histogram and haze guide silently double count or drop edge boxes; no test covers the pair because both sides read the same uniforms.
- fix: one `Common::` helper that turns the spans into the clipped `(lo, hi)` core rect (returning a `vec4<i32>` plus the `any(hi <= lo)` early-out at each call site), called by both analysis passes.
- effort: S
### [SHADER-003] Rec.2020 luma weights are bare literals in 13 shader sites while `Common::LUMA` exists
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/shaders/pass4.wgsl:182, :219, :255, :257; crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:19, :90, :92, :124 and crates/calibraw-gpu/src/shaders/xtrans/markesteijn.wgsl:398; crates/calibraw-gpu/src/shaders/mask_effects/film_finish.wgsl:52, :80; crates/calibraw-gpu/src/shaders/noise_ca_finish.wgsl:16
- evidence:
  `crates/calibraw-gpu/src/shaders/pass4.wgsl:182:    let y = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));`
  `crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:19:    let y = 0.2627 * rgb.r + 0.6780 * rgb.g + 0.0593 * rgb.b;`
  `crates/calibraw-gpu/src/shaders/common.wgsl:187:const LUMA: vec3<f32> = vec3<f32>(0.2627002, 0.6779981, 0.0593017);`
  The named constant is used elsewhere: `crates/calibraw-gpu/src/shaders/haze.wgsl:192:    return clamp(dot(rgb, Common::LUMA) / airlight.w, 0.0, HAZE_GUIDE_MAX);` and `common.wgsl:288:    return max(dot(rgb, LUMA), 1e-6);`
  `grep -rn '0.2627' crates/calibraw-gpu/src/shaders` returns 14 lines, one of them the definition, so 13 sites re-spell the weights; `grep -rn '0\.2627' crates --include='*.rs'` outside tests returns 5 more (crates/calibraw-core/src/pipeline/remove/scene_color.rs:98, :111, :122, crates/calibraw-ai/src/models/remove/lama/grain.rs:266, crates/calibraw-gpu/src/pipeline/export/resize.rs:149).
  The two spellings also differ in precision: `crates/calibraw-gpu/src/pipeline/gpu/dehaze_tests.rs:63:const LUMA: [f32; 3] = [0.262_700_2, 0.677_998_1, 0.059_301_7];` versus `crates/calibraw-gpu/src/pipeline/gpu/scene_gamut_tests.rs:16:const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];`.
- why it is a problem: the same colour constant has two spellings with no comment saying which passes use which, so a pass can be switched to the other one by typing either literal and the difference is invisible in review; the 5 Rust sites have no named constant at all.
- fix: name each spelling once - keep `Common::LUMA` for the passes that already use it and add a second WGSL constant (and a Rust constant in the colour module) for the four-decimal triplet - then replace the 18 bare spellings with those names. The two triplets are not bit-identical, so they must stay two constants; a shader-text test should pin each pass to the constant it uses.
- effort: M
### [SHADER-004] Two X-Trans finish passes copy the same four-neighbour median block
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:118-124 (in `xt_reference_false_color_guard`) and :135-141 (in `xt_frequency_chroma`)
- evidence:
  `crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:118:    let uvn = xt_uv(xt_high(pos + vec2<i32>(0, -1)));`
  `crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:123:        xt_median5(uv0.x, uvn.x, uvs.x, uvw.x, uve.x),`
  `crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:135:    let uvn = xt_uv(xt_high(pos + vec2<i32>(0, -1)));`
  `crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl:140:        xt_median5(uv0.x, uvn.x, uvs.x, uvw.x, uve.x),`
  The nine lines between them are identical; the functions differ only in how they scale the result (`NoiseCaFinish::finish_false_color_strength(rgb, disagreement, 0.50)` at :126 versus `clamp(Common::camera_uniforms.frequency_chroma, 0.0, 1.0)` at :143).
  `grep -n 'xt_uv(xt_high(pos + vec2' crates/calibraw-gpu/src/shaders/xtrans_finish.wgsl` returns these two blocks plus the one loop at :69 that already does the same sampling by iteration.
- why it is a problem: the neighbourhood used as the false-colour reference is written twice, so one pass can be switched to a different support (radius, diagonals) without the other and the two demosaic references then disagree on the same pixel with no test comparing them.
- fix: a local helper `fn xt_neighbourhood_median(pos: vec2<i32>) -> vec4<f32>` returning `uv0` and the median, called by both passes; WGSL already uses local helpers for the same task (`xt_median5`, `xt_uv`).
- effort: S
### [SHADER-005] The neighbourhood in-bounds test is spelled seven times in WGSL
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-gpu/src/shaders/haze_analysis.wgsl:247, :292, :347, :376, :502, :542 and crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:300
- evidence:
  `crates/calibraw-gpu/src/shaders/haze_analysis.wgsl:502:            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }`
  `crates/calibraw-gpu/src/shaders/tone_analysis.wgsl:300:            if any(neighbour < vec2<i32>(0)) || any(neighbour >= grid) { continue; }`
  `grep -rn 'any(neighbour < vec2<i32>(0))' crates/calibraw-gpu/src/shaders` returns these 7 lines and no shared helper; the two modules already share helpers through `Common::` (e.g. `Common::tile_origin()`, `Common::LUMA`).
- why it is a problem: a window filter that needs a different edge rule (clamp-to-edge, halo padding) has to have its bounds test edited at seven places, and one missed edit makes that pass average a different neighbourhood than the pass that feeds it.
- fix: one `fn Common::inside_grid(neighbour: vec2<i32>, grid: vec2<i32>) -> Bool` in common.wgsl and call it at all seven sites.
- effort: S
### [SCRIPT-001] Flatpak SDK versions and a pinned upstream commit are re-spelled per script
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: scripts/flatpak.sh:10, :36, :37
- evidence:
  `scripts/flatpak.sh:10:builder_tools_commit=74697c75b630d7330e77250fc13cb5ea688d9479`
  `scripts/flatpak.sh:36:      org.flatpak.Builder org.freedesktop.Platform//26.08 org.freedesktop.Sdk//26.08 \`
  `scripts/flatpak.sh:37:      org.freedesktop.Sdk.Extension.rust-stable//26.08 org.freedesktop.Sdk.Extension.llvm22//26.08`
  The same release is declared a second time in the manifest the script builds: `packaging/flatpak/io.github.Duecki1.CalibRaw.json:4:    "runtime-version": "26.08",`
  `grep -rn '26\.08\|74697c75b630' scripts packaging` returns only these sites, so nothing names them once.
- why it is a problem: the script installs one SDK release while the manifest asks for another if either edit is forgotten, and the build then succeeds locally and fails on Flathub's lint; the bare commit hash has no comment saying which upstream tool it pins or when it may move.
- fix: read the runtime version from the manifest (`packaging/flatpak/prepare.py` already parses these files) into one `sdk_version` variable used by the install line, and give `builder_tools_commit` a comment stating which tool it pins and that a bump is expected when the generator is re-fetched.
- effort: S
### [SCRIPT-002] Bootstrap download retries and timeouts are bare numbers, and the urllib-then-curl switch has no comment
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: scripts/bootstrap_download.py:59-60, :64, :94, :114, :167
- evidence:
  `scripts/bootstrap_download.py:59:    urllib_attempts = 1 if curl is not None else attempts`
  `scripts/bootstrap_download.py:60:    urllib_timeout = min(timeout, 5.0) if curl is not None else timeout`
  `scripts/bootstrap_download.py:64:                opener.open(request, timeout=min(urllib_timeout, 30.0)) as response,`
  `scripts/bootstrap_download.py:94:                "30",`
  `scripts/bootstrap_download.py:114:            download_https(source, checksum_file, attempts=9, timeout=300)`
  `scripts/bootstrap_download.py:167:        download_https(url, temporary, attempts=9, timeout=900)`
  No comment anywhere in the file explains why a present `curl` reduces urllib to one short attempt (`grep -n 'curl' scripts/bootstrap_download.py` shows only the code).
- why it is a problem: the download policy (9 attempts, 5 s first attempt, 30 s connect timeout spelled twice as 30.0 and "30") is invisible to anyone changing the script, and the two callers re-spell the attempt count, so a CI timeout change has to be found in two places while the effective per-attempt limit is the smaller of two hidden numbers.
- fix: module constants `ATTEMPTS`, `FIRST_ATTEMPT_TIMEOUT`, `CONNECT_TIMEOUT` used by both call sites and both timeout arguments, plus one comment stating that urllib is tried briefly first and curl's retry/backoff is the fallback when curl exists.
- effort: S
### [SCRIPT-003] Release scripts download tooling into fixed /tmp paths and assert with bare `test`
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: scripts/build_linux_release.sh:11-12, :45-46 and scripts/package_linux_appimage.sh:45, :48, :14-18
- evidence:
  `scripts/build_linux_release.sh:11:/tmp/rustup-init \`
  `scripts/build_linux_release.sh:45:ldd target/release/calibraw | tee /tmp/calibraw-ldd.txt`
  `scripts/package_linux_appimage.sh:45:LINUXDEPLOY=/tmp/linuxdeploy-${LINUXDEPLOY_ARCH}.AppImage`
  `scripts/package_linux_appimage.sh:48:  "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-${LINUXDEPLOY_ARCH}.AppImage" \`
  `scripts/package_linux_appimage.sh:14:test -f "$LIBRAW_SO"` and `:16 test -n "$LENSFUN_DB"`, `:36 test -s "$APPIMAGE_ICON"`, `:37 test -L AppDir/.DirIcon`
  `grep -rn '/tmp/' scripts/build_linux_release.sh scripts/package_linux_appimage.sh` returns only these sites, and `scripts/bootstrap_download.py:172` prints "verified cached download" when the destination already matches the digest.
- why it is a problem: a leftover `/tmp/rustup-init` or `/tmp/linuxdeploy-*.AppImage` from an earlier run is accepted by the digest check instead of being downloaded, and two builds sharing /tmp overwrite each other; the bare `test` assertions exit with no message, so a packaging failure looks like a crash with no clue about which file was missing; the linuxdeploy tag carries a date and nothing states that it has to be re-pinned.
- fix: download tooling into a `mktemp -d` prefix (the scripts already use one in scripts/build_libraw.sh:32) and echo the reason before each `test` (`{ test -f "$LIBRAW_SO"; } || { echo "no bundled libraw in target/release"; exit 1; }` style), and comment the linuxdeploy tag as pinned-to-digest with its bump policy.
- effort: S
### [SCRIPT-004] The staged Flathub manifest is patched by module index
- category: bad-code
- severity: medium
- confidence: high
- where: packaging/flatpak/prepare.py:124 and :126-127 (in `snapshot`), :164-165 (in `stage_release`)
- evidence:
  `packaging/flatpak/prepare.py:124:    manifest["modules"][2] = "../packaging/flatpak/onnxruntime-module.json"`
  `packaging/flatpak/prepare.py:126:    manifest["modules"][-1]["sources"] = [`
  `packaging/flatpak/prepare.py:164:    manifest["modules"][-1]["sources"] = [`
  In the checked-in manifest these are `packaging/flatpak/io.github.Duecki1.CalibRaw.json`: module 0 = libraw, 1 = lensfun, 2 = the `onnxruntime-module.json` reference, 3 (last) = the `calibraw` module named at `:3` by `'build-commands': ['python3 packaging/flatpak/generate-licenses.py', ...]`.
  `grep -n 'modules\[' packaging/flatpak/prepare.py` shows the three positional writes and no check that an index holds the expected module.
- why it is a problem: adding a module to the checked-in manifest (a new native dependency module is the common case) moves both targets, so `snapshot` and `stage-release` rewrite an unrelated module and the release manifest that Flathub builds from carries the wrong sources with no error.
- fix: select the module by identity - `onnx = [m for m in manifest["modules"] if m == "onnxruntime-module.json" or (isinstance(m, dict) and m.get("name") == "onnxruntime")]` and the app module by `m.get("name") == "calibraw"` - and raise a `SystemExit` with a clear message if either lookup is missing.
- effort: S
### [SCRIPT-005] A pinned license digest is inline in a branch and the default cache lives in /tmp
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: packaging/flatpak/generate-onnx-sources.py:180, :172 and :211
- evidence:
  `packaging/flatpak/generate-onnx-sources.py:180:            if digest(license_path, "sha256") != "c9bff75738922193e67fa726fa225535870d2aa1059f91452c411736284ad566":`
  The other pins in this generator are named at the top: `packaging/flatpak/generate-onnx-sources.py:19:VERSION = "1.30.0"`, `:20:COMMIT = "f2c39fe2..."`, `:21:DEPS_SHA256 = "880edacbd7..."`.
  `packaging/flatpak/generate-onnx-sources.py:211:    parser.add_argument("--cache-dir", type=Path, default=Path("/tmp/calibraw-onnx-sources"))`
  `packaging/flatpak/generate-onnx-sources.py:172:        url = paths.get(name, (root / "flatpak-disabled-deps" / archive_name(name)).as_posix())` depends on `def archive_name(name):` at :56 `return name + (".tar.gz" if name == "protobuf" else ".zip")`, which carries no comment.
- why it is a problem: the license pin is the only digest in this file that a reader cannot find near the other pins, so bumping the ORT revision leaves it invisible; the default /tmp cache reuses whatever a previous user left there before the script's own SHA-1 checks run.
- fix: name the digest `MP11_LICENSE_SHA256` beside `DEPS_SHA256`, comment the protobuf extension difference in `archive_name`, and default `--cache-dir` to a directory under the repository (the script is run from `packaging/flatpak/`).
- effort: S
### [SCRIPT-006] The synthetic DNG writer spells TIFF tags and its own dimensions as bare numbers
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: packaging/flatpak/smoke-test.py:38-56 (the tag table), :88 (`pixels`) and :112 (the PNG check)
- evidence:
  `packaging/flatpak/smoke-test.py:45:        (262, 3, [32803]), (271, 2, "CalibRaw"),`
  `packaging/flatpak/smoke-test.py:53:        (50710, 1, [0, 1, 2]), (50711, 3, [1]), (50714, 3, [64)],`
  `packaging/flatpak/smoke-test.py:20:    width = height = 64`
  `packaging/flatpak/smoke-test.py:112:    if png[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack(">II", png[16:24]) != (64, 64):`
  No comment explains any of the 27 tag numbers, and `grep -n '64' packaging/flatpak/smoke-test.py` shows the dimension re-spelled in the assertion that is supposed to match `width`/`height`.
- why it is a problem: nobody can tell which tag the writer is meant to emit, so dropping or renaming one TIFF entry (for example the sensor scale at 50717 or the CFA pattern at 50721) makes the file invalid in a way only LibRaw notices at runtime; and the exported size is checked against a literal 64, so changing the synthetic image size silently stops checking anything.
- fix: name the tags the writer depends on (`TAG_WIDTH = 256`, `TAG_CFA_PATTERN = 50721`, ...) or put a short comment per line of the table, and compare the PNG size against `width, height` instead of `(64, 64)`.
- effort: M
### [CLI-001] The reference suite lists 22 of its 25 cases by hand as plus/minus pairs
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-cli/src/bin/calibraw-develop-export.rs:223-249 (`REFERENCE_CONTRAST_SUITE`)
- evidence:
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:227:    ("exposure_plus1_25", Some(("exposure", 1.25))),`
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:228:    ("exposure_minus1_25", Some(("exposure", -1.25))),`
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:233:    ("contrast_plus100", Some(("contrast", 100.0))),`
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:234:    ("contrast_minus100", Some(("contrast", -100.0))),`
  Eleven parameters appear as `+100.0`/`-100.0` twin lines (`grep -c 'plus100\|minus100' crates/calibraw-cli/src/bin/calibraw-develop-export.rs` = 22); only `baseline` and the two exposure pairs are not of that shape.
  Nothing in finding.md or docs/rework/AUDIT.md names this table (`grep -n REFERENCE_CONTRAST_SUITE finding.md docs/rework/AUDIT.md` is empty).
- why it is a problem: adding or renaming a card means writing two lines and keeping the label and the parameter name in step by hand, so a missed minus case drops a PNG from the suite that the CI diff compares silently; the label text and the parameter key are duplicated on one line.
- fix: keep a `&[&str]` of parameter names plus the two special exposure cases, and generate the entries as `(f"{name}_plus100", (name, 100.0))` and its negation, which produces exactly the current file names in `directory.join(format!("{label}.png"))` at :135.
- effort: S
### [CLI-002] The adjustment names are spelled three times in the develop-export CLI
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-cli/src/bin/calibraw-develop-export.rs:251-301 (`set_adjustment`), :223-249 (`REFERENCE_CONTRAST_SUITE`) and :437-455 (`print_help`)
- evidence:
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:256:        "contrast" => exposure.contrast = value.clamp(-100.0, 100.0),`
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:233:    ("contrast_plus100", Some(("contrast", 100.0))),`
  `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:439:        "Supported adjustment names: exposure, contrast, highlights, shadows, whites,\n",`
  The name `contrast` (and the other ten suite parameters) appears as a bare string in all three places; `grep -c '"contrast"' crates/calibraw-cli/src/bin/calibraw-develop-export.rs` = 2 plus the help prose occurrence.
  finding.md F-36 covers only the re-clamped *ranges* in `set_adjustment`, not these name lists.
- why it is a problem: the help the user reads is a hand-copied list, so a renamed or added adjustment in `set_adjustment` prints a name that does not work, and the suite's `--only ENDPOINT` validation at :399-406 checks only the table, so a name missing from the help is invisible while a name missing from the table fails with "unknown suite endpoint".
- fix: one table of `(name, kind)` shared by the dispatch and the suite (CLI-001's generated table), and build the help's name line from that table, so the three lists cannot drift; a `#[test]` that renders the help and asserts every dispatch name appears keeps it honest.
- effort: M
### [CLI-003] The CLI shim re-exports the whole GPU pipeline module with a wildcard
- category: rule-violation
- severity: low
- confidence: high
- where: crates/calibraw-cli/src/lib.rs:1-3 (the whole file)
- evidence:
  `crates/calibraw-cli/src/lib.rs:1:pub mod pipeline {`
  `crates/calibraw-cli/src/lib.rs:2:    pub use calibraw_gpu::pipeline::*;`
  The binaries then import through this alias, e.g. `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:5:use calibraw_cli::pipeline::{` and `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:200:        let source_dimensions = (self.metadata.source_width, self.metadata.source_height);` uses `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:185:                remove: calibraw_cli::pipeline::RemoveEditState::default(),`
  `timeout 60 grep -rn 'calibraw_cli::' crates/calibraw-cli/src/bin` shows the pipeline alias is the only thing lib.rs provides, and no name is picked from it by hand.
  finding.md covers the same shape in the FFI crate only (`finding.md:497:17: pub use android::*;`), so this instance is not tracked.
- why it is a problem: the CLI crate's public API is defined by whatever calibraw-gpu happens to export, so a reader cannot tell which pipeline types the tool depends on and a gpu-side rename or new public type silently changes the shim's API; AGENTS.md asks for explicit imports in production modules.
- fix: since the alias exists only to shorten paths inside one binary, list the names both binaries actually use (`pub mod pipeline { pub use calibraw_gpu::pipeline::{crop_raw, effect_params, export_mask_atlas_edge, load_raw_file, ...}; }`), or drop lib.rs and import from `calibraw_gpu::pipeline` directly in the two binaries.
- effort: S
### [XTASK-003] The default Android ABI is spelled four times while a named list sits unused for it
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: xtask/src/android.rs:21, :94, :130, :257 and :262
- evidence:
  `xtask/src/android.rs:21:const ANDROID_64_BIT_ABIS: [&str; 1] = ["arm64-v8a"];`
  `xtask/src/android.rs:94:            .unwrap_or_else(|| "arm64-v8a".to_owned()),`
  `xtask/src/android.rs:130:        abi: abi.unwrap_or_else(|| "arm64-v8a".to_owned()),`
  `xtask/src/android.rs:257:        "arm64-v8a" => Ok((`
  `xtask/src/android.rs:262:            "Unsupported ABI '{abi}' (use arm64-v8a)"`
  `timeout 60 grep -n 'arm64-v8a\|ANDROID_64_BIT_ABIS' xtask/src/android.rs` returns only these six lines, and the const itself is read once, at `xtask/src/android.rs:669:        if !ANDROID_64_BIT_ABIS.contains(&abi) {`; nothing in finding.md or docs/rework/AUDIT.md names it.
- why it is a problem: the default and the accepted set are five spellings of one decision, so adding an ABI (arm34/x86_64-android is the usual next step) changes the const but leaves the defaults and the match arm behind, and `android_abi_config` then rejects the ABI the default path just picked.
- fix: name the default `const ANDROID_DEFAULT_ABI = ANDROID_64_BIT_ABIS[0]` (or a `const` of its own next to the list) and use that constant in both `unwrap_or_else` defaults and in the usage message, so only the match arm needs the literal.
- effort: S
### [XTASK-004] The 16 KB page policy is a bare `14` and a bare `16` in the APK verifier
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: xtask/src/android.rs:717 (`*alignment < 14`) and :634 (`["-c", "-P", "16", "-v", "4"]`)
- evidence:
  `xtask/src/android.rs:717:    if alignments.iter().any(|alignment| *alignment < 14) {`
  `xtask/src/android.rs:720:        eprintln!("16 KB ELF alignment check failed: {}", library.display());`
  `xtask/src/android.rs:634:    run_checked(` ... `Command::new(&zipalign).args(["-c", "-P", "16", "-v", "4"])`
  `xtask/src/android.rs:743:fn parse_alignment_power(line: &str) -> Option<u32> {` returns the `2**N` exponent read out of `xtask/src/android.rs:745:    let marker = "align 2**";`
  `timeout 60 grep -n '16 KB' xtask/src/android.rs` shows only prose in messages, and nothing in finding.md or docs/rework/AUDIT.md names these numbers.
- why it is a problem: `14` is the log of the page size while `-P 16` is the page size itself, so the two spellings of one requirement can disagree silently (an 8 KB policy would need 13 and `-P 8`); the check is what gates publishing, and its threshold has no name to reason about.
- fix: `const PAGE_ALIGN_POWER = 14; // 2**14 = 16 KiB pages` beside the existing `ANDROID_64_BIT_ABIS` at :21 and `const PAGE_ALIGN_KIB = PAGE_ALIGN_POWER - 4` interpolated into the zipalign arguments, keeping the message text generated from the same constant.
- effort: S
### [XTASK-005] The loc report prints its columns with the same format string three times
- category: duplicate
- severity: low
- confidence: high
- where: xtask/src/loc.rs:566, :577 and :597 (in `print_report`)
- evidence:
  `xtask/src/loc.rs:566:        "{:<34} {:>10} {:>10} {:>8} {:>8} {:>9} {:>6}",`
  `xtask/src/loc.rs:577:            "{:<34} {:>10} {:>10} {:>8} {:>8} {:>9} {:>6}",`
  `xtask/src/loc.rs:597:        "{:<34} {:>10} {:>10} {:>8} {:>8} {:>9} {:>6}",`
  `timeout 60 grep -n '{:<34}' xtask/src/loc.rs` returns only these three lines; the header at :567-574 supplies the labels and the totals block at :597-603 repeats the same seven accessors (`area.code(Category::Production)` ... `area.files`).
  Nothing in finding.md or docs/rework/AUDIT.md names this format.
- why it is a problem: the report's alignment is defined in three places, so a width change applied to the area rows and not to the total row (or to the header labels) puts the table out of step, and the `bench/ex` column merges two categories by hand in each copy.
- fix: one `print_row(name: String, area: Area)` (the totals already build an `Area::default()`) called for the header, each area and the totals, so the seven-column layout and the Bench+Example merge exist once.
- effort: S
### [XTASK-006] A hand-rolled Gregorian calendar converts a day count for one folder name
- category: unnecessary-code
- severity: low
- confidence: high
- where: xtask/src/baseline.rs:390-421 (`civil_from_days`), :384 (`utc_date`) and :378 (`unix_seconds`)
- evidence:
  `xtask/src/baseline.rs:386:    let (year, month, day) = civil_from_days((unix_seconds() / 86_400) as i64);`
  `xtask/src/baseline.rs:396:    let shifted = days + 719_468;`
  `xtask/src/baseline.rs:397:    let era = shifted.div_euclid(146_097);`
  `xtask/src/baseline.rs:401:        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;`
  `xtask/src/baseline.rs:405:    let month_index = (5 * day_of_year + 2) / 153;`
  `grep -n 'civil_from_days\|utc_date\|unix_seconds' xtask/src/baseline.rs` shows the three functions are used only by `create_run_folder` and the run manifest; none of the seven numeric constants carries a comment (the only doc comment names the algorithm's author at :382).
  `xtask/Cargo.toml` has no date library, and this finding appears in neither finding.md nor docs/rework/AUDIT.md.
- why it is a problem: ~40 lines of unexplained calendar arithmetic in the release tooling are only reachable through the run-folder name, so a leap-year or century boundary is invisible to review and a wrong value silently files a capture under the wrong date.
- fix: ask git for the stamp it already has - `run_command_output(Command::new("git").args(["log", "-1", "--date=format:%Y-%m-%d", "--format=%cd"]))` next to the existing `git show -s --format=%ct` in scripts/build_linux_release.sh:12 - and delete `civil_from_days`, `utc_date` and `unix_seconds`; if the arithmetic stays, comment each constant (days per 400-year era, the March-based month shift) and keep the existing round-trip test.
- effort: S
### [XTASK-007] The ICO writer and the Android adaptive icon spell their format numbers bare
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: xtask/src/icons.rs:60-84 (`write_ico`) and :120 (`foreground_edge`) plus :112-118 (`obsolete`)
- evidence:
  `xtask/src/icons.rs:68:    file.write_all(&0_u16.to_le_bytes())?;`
  `xtask/src/icons.rs:72:    let mut offset = 6_u32 + 16_u32 * sizes.len() as u32;`
  `xtask/src/icons.rs:74:        file.write_all(&[if *edge == 256 { 0 } else { *edge as u8 }])?;` (repeated identically at :75 for height)
  `xtask/src/icons.rs:79:        file.write_all(&32_u16.to_le_bytes())?;`
  `xtask/src/icons.rs:120:    let foreground_edge = (source.width() * 66 / 100).max(1);`
  `xtask/src/icons.rs:112:    for obsolete in [` followed by two `android/app/src/main/res/mipmap-anydpi/ic_launcher*.xml` paths and `if obsolete.exists() { fs::remove_file(...) }`
  Only one comment in the file (`/// Application icon generation ...` at :1) and none of `0`, `1`, `6`, `16`, `32`, `256 -> 0` or `66 %` is explained; nothing in finding.md, docs/rework/AUDIT.md or audit-report.md covers this file.
- why it is a problem: the file writes a binary container whose field order and widths the desktop shells depend on, and every number is a format constant (reserved count, image-plane type, directory entry size, the "256 encoded as zero" rule) that a reader can only guess; the 66 % adaptive-icon ratio is the Material safe-area rule and is the only place where it is written, so a packaging change cannot tell whether 66 or the whole `obsolete` deletion is still wanted.
- fix: name the constants (`const ICO_RESERVED = 0`, `ICO_PLANES = 32`, `const ICO_ENTRY_BYTES = 16`, `const ICO_ADAPTIVE_FOREGROUND_PERCENT = 66`) and factor the doubled `[if *edge == 256 { 0 } else { *edge as u8 }]` into one `icon_size_byte(edge)`; comment the `obsolete` list as "Android now ships adaptive icons, the pre-2024 XML launchers must not stay in the tree".
- effort: M
### [XTASK-008] Every xtask synopsis is written twice, in prose and in `print_help`
- category: duplicate
- severity: low
- confidence: high
- where: xtask/src/main.rs:96-124 (`print_help`) against the doc comments xtask/src/android.rs:24, :29, :37, :45, xtask/src/baseline.rs:22, :117 and the module headers xtask/src/ui_lint.rs:1, xtask/src/loc.rs:1, xtask/src/cfg_ratchet.rs:1, xtask/src/migrations.rs:1, xtask/src/arch.rs:1
- evidence:
  `xtask/src/android.rs:24:/// \`cargo xtask build-android [ABI] [PROFILE] [--features LIST]\``
  `xtask/src/main.rs:119:           build-android [ABI] [PROFILE] [--features LIST]\n\`
  `xtask/src/android.rs:45:/// \`cargo xtask verify-android-16kb [APK] [--objdump PATH] [--zipalign PATH]\``
  `xtask/src/main.rs:123:           verify-android-16kb [APK] [--objdump PATH] [--zipalign PATH]"`
  `xtask/src/baseline.rs:22:/// \`cargo xtask baseline-run LABEL [--filter FILTER]\`` against `xtask/src/main.rs:110:           baseline-run LABEL [--filter F]  capture UI review screenshots into\n\`
  XTASK-002 covers the duplicated `--help` *handlers*, not these synopsis strings.
- why it is a problem: the two copies already disagree (`[--filter FILTER]` versus `[--filter F]`), so the dispatcher's help can advertise an option no parser accepts, and adding an option to a parser means hunting the help table in a different file.
- fix: keep the synopsis in the entry point's doc comment (or one `const COMMANDS: &[(&str, &str, &str)]` table of command, synopsis, description next to the dispatch in main.rs) and render the help from that table, so the match arms and the help text come from the same source.
- effort: S
### [XTASK-009] The JNI checker keeps three hand-written primitive-to-descriptor tables
- category: duplicate
- severity: medium
- confidence: high
- where: xtask/src/jni_contract/rust_side.rs:106-124 (`rust_value_descriptor`), xtask/src/jni_contract/rust_side.rs:452-470 (`jni_sig_type_descriptor`) and xtask/src/jni_contract/java_side.rs:141-151 (`TypeResolver::descriptor`)
- evidence:
  `xtask/src/jni_contract/rust_side.rs:109:        "jbyte" => "B",`
  `xtask/src/jni_contract/rust_side.rs:455:        "jbyte" | "byte" | "i8" => Some("B"),`
  `xtask/src/jni_contract/java_side.rs:146:            "byte" => Some("B"),`
  `xtask/src/jni_contract/rust_side.rs:116:        "JString" | "jstring" => "Ljava/lang/String;",` against `xtask/src/jni_contract/rust_side.rs:473:        "JString" => "java/lang/String",`
  Each table lists the same nine primitives (`void`, `boolean`, `byte`, `char`, `short`, `int`, `long`, `float`, `double`) plus the object classes, in different spellings, and `xtask/src/jni_contract.rs:104` compares the two sides with `method.descriptor.as_ref() != Ok(descriptor)`, i.e. table against table.
  Nothing in finding.md, docs/rework/AUDIT.md or audit-report.md covers these functions.
- why it is a problem: the check never sees the jni crate's own mapping, so it only proves the three copies agree with each other; changing one row (for instance making `long` map to `J` on one side only, which is how the two tables already differ for `JString`'s leading slash) keeps the check green while the export and the Java declaration disagree at runtime.
- fix: one `const JNI_PRIMITIVES: &[(&str, &str)] = &[("void", "V"), ("boolean", "Z"), ...]` and one alias table per side (`("jlong" | "long" | "i64")` groups) that all three call sites resolve through, so a new primitive is added once and the class-name formatting stays in `binary_name`.
- effort: M
### [XTASK-010] The Java type resolver accepts an unnamed hard-coded list of 14 `java.lang` names
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: xtask/src/jni_contract/java_side.rs:185-199 (`const JAVA_LANG`) and :201 in `TypeResolver::binary_name`
- evidence:
  `xtask/src/jni_contract/java_side.rs:185:        const JAVA_LANG: [&str; 14] = [`
  `xtask/src/jni_contract/java_side.rs:186:            "Object",` ... `:199:            "Void",`
  `xtask/src/jni_contract/java_side.rs:201:        if rest.is_none() && JAVA_LANG.contains(&head) {`
  `xtask/src/jni_contract/java_side.rs:202:            return Ok(format!("java/lang/{head}"));`
  `xtask/src/jni_contract/java_side.rs:211:        Err(format!("cannot resolve Java type {ty}"))`
  The only comment in that function is `/// \`java/lang/String\`, \`de/duecki/calibraw/Outer\$Inner\`, ...` at :177 and `// Nested types visible from the enclosing class scope, innermost first.` at :180; nothing explains where the 14 names came from, and `timeout 60 grep -rn 'java/lang' xtask/src/jni_contract/java_side.rs` shows no other route to a `java/lang/...` name.
- why it is a problem: a Java declaration that uses any other `java.lang` type (a `java.lang.BigInteger` parameter, or the `JniHelper`/`JavaEnv` types the jni crate itself hands around) resolves to `cannot resolve Java type`, which the check reports as an app-level failure the developer cannot tell apart from a real signature mismatch.
- fix: resolve by rule instead of by list - a name imported from `java.lang.` (or reachable as `java.lang.X`) takes the `java/lang/{head}` form - and keep the constant list only for the documented purpose of accepting unqualified primitive-ish names, with a comment saying so.
- effort: S
### [XTASK-011] `xyz_to_lab` guesses the XYZ scale from the values it is handed
- category: temp-or-hardcoded
- severity: low
- confidence: medium
- where: xtask/src/colorchecker.rs:246-252 (`xyz_to_lab`) called once per side per row at :158 and :163
- evidence:
  `xtask/src/colorchecker.rs:247:    let scale = if xyz.iter().any(|value| value.abs() > 2.0) {`
  `xtask/src/colorchecker.rs:248:        100.0`
  `xtask/src/colorchecker.rs:249:    } else {`
  `xtask/src/colorchecker.rs:250:        1.0`
  `xtask/src/colorchecker.rs:251:    };`
  `xtask/src/colorchecker.rs:256:    let [fx, fy, fz] = std::array::from_fn(|index| transform(xyz[index] / scale / D50[index]));`
  The only guidance is in the tool's own help text at :36 (`Both sides must use the same XYZ scale (Y=1 or Y=100).`), and nothing in the file enforces it: `timeout 60 grep -rn 'Y=1\|scale' xtask/src/colorchecker.rs` returns only these two places. The neighbouring constants (`const D50`, `REQUIRED_COLUMNS`) are named and commented, so this inference is the odd one out.
- why it is a problem: the scale is chosen per call, so reference and rendered rows that use different scales are silently both normalised and compared as if they matched, and one CSV that mixes Y=1 rows with Y=100 rows is scored with a different white point per row. A wrong guess changes every dE00 number without any message pointing at the scale, which makes the check's output depend on data ranges rather than on the documented contract.
- fix: take the scale once for the whole file - from a `--scale` option or a CSV column - and validate each row's Y against it, reporting `CSV line N: Y={value} does not match the {scale} scale` instead of inferring it; keep the existing named `D50` constant.
- effort: S
### [XTASK-012] The log-polling loop buries its two intervals as bare durations
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: xtask/src/android_bench/mod.rs:498 and :500 in `Bench::wait_for_log`
- evidence:
  `xtask/src/android_bench/mod.rs:498:                next_alive_check = now + Duration::from_secs(1);`
  `xtask/src/android_bench/mod.rs:500:            thread::sleep(Duration::from_millis(250));`
  Same file, :47: `/// How often memory is sampled during a step.` then `const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);` — the sampler's interval is named and documented, the wait loop's are not. `timeout 60 grep -rnE 'from_millis\(|from_secs\(' xtask/src/android_bench` returns only these five sites, and `timeout 60 grep -nE 'android.bench' finding.md docs/rework/AUDIT.md` returns nothing.
- why it is a problem: 250 ms decides how quickly a `wait_log` step notices its line and 1 s decides how often a killed app is noticed, and both also set how many `adb` round-trips a failing step makes; nothing in the code says which is the poll and which is the liveness re-check, so changing one (or reusing the named `SAMPLE_INTERVAL`) changes an Android-device timing behaviour that CI and a low-end phone comparison both depend on.
- fix: two named constants next to `SAMPLE_INTERVAL` (`WAIT_LOG_POLL_INTERVAL`, `WAIT_LOG_LIVENESS_INTERVAL`), each with the same one-line comment style, used by the loop.
- effort: S
### [XTASK-013] `android-bench-compare` re-lists the report's figures by hand and misses several
- category: duplicate
- severity: low
- confidence: medium
- where: xtask/src/android_bench/report.rs:43-113 (`StepReport::figures`) against `StepReport` at :24-41
- evidence:
  `xtask/src/android_bench/report.rs:75:        push("post2acquire p50 ms",` — it is the only `post2acquire` figure pushed, although `Percentiles` carries `p90` and `max` as well (`xtask/src/android_bench/metrics.rs:20-24`).
  `xtask/src/android_bench/report.rs:84:            push(&format!("{stage} per s"), stats.per_second);` — `StageStats.max_ms` is never pushed, nor is `memory_after.total_pss_mib`, nor `heap_purges`.
  `xtask/src/android_bench/mod.rs:382:        let duration_ms = started.elapsed().as_millis() as u64;` is stored into the report (`:395`) and written to `android-bench.json`, but `timeout 60 grep -rn 'duration_ms' xtask/src/android_bench` shows no other reader: `print_summary` (:396-414) prints fps, stage medians, RSS and kill counts instead, so the step's own measured duration reaches neither the summary nor the comparison table.
  Nothing in finding.md or docs/rework/AUDIT.md covers `figures`, and each of the 15 figure names is typed out twice (`push("RSS after MiB", ...)` etc.).
- why it is a problem: the two views of one `StepReport` are kept in sync by hand, so a regression in a figure that only exists in the JSON (`p90` frame time, worst GPU stage, `MemAvailable` PSS, the step duration itself) is invisible in the tool whose whole purpose is to compare two runs; adding a metric means editing the struct and then hoping to remember the `push` line.
- fix: describe the figures once as data - e.g. a `const COMPARE_FIGURES: &[(&str, FigureKind)]` or a list of `(name, extractor)` pairs over `StepReport`/`MemorySnapshot`/`StageStats` that both `print_summary` and `comparison` consume - so an added report field either appears in both views or is explicitly marked as JSON-only.
- effort: M
### [XTASK-014] The scenario validator writes its own "plain name" rule for the photo
- category: duplicate
- severity: low
- confidence: high
- where: xtask/src/android_bench/scenario.rs:95-100 in `Scenario::validate` against `is_plain_name` at :164-169
- evidence:
  `xtask/src/android_bench/scenario.rs:98:                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))`
  `xtask/src/android_bench/scenario.rs:164:fn is_plain_name(name: &str) -> bool {`
  `xtask/src/android_bench/scenario.rs:168:            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))`
  The same file uses the helper twice more, for step names (:107) and screenshot names (:137), and its doc comment reads `/// Letters, digits, \`-\` and \`_\`: safe in a file name on every platform.` `timeout 60 grep -nE 'is_plain_name|"\._-"|"-_"' xtask/src/android_bench/scenario.rs` returns only these five sites, so the photo is the only rule that bypasses the helper.
- why it is a problem: two near-identical character-class rules that already disagree (`"."` is accepted for a photo name but rejected for a step and screenshot name, so `screens/step.a.b.png` becomes impossible while a photo may contain dots); a future rule change - adding a space, or rejecting a leading `-` that `adb shell` would read as a flag - has to be typed in both places and the photo path keeps its own copy.
- fix: one `fn plain_name(name: &str, extra: string) -> bool` helper, or two named helpers (`is_step_name`, `is_photo_file_name`) that state why the photo rule also admits `.`, and use them at every call site; while there, validate that the pinned `photo_sha256` is 64 hex digits at load time rather than only when a run compares it.
- effort: S
### [UI-015] The preset "what is included" list is enumerated three times and one copy has no control
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/presets.rs:122-129 (`selection_summary`), :712-720 (`selected_count` inside `show_selection_controls`) and :812-861 (`show_additional_selection`)
- evidence:
  `crates/calibraw-ui/src/ui/presets.rs:123:        (selection.raw_processing, "RAW processing"),` ... `:128:        (selection.lens_correction, "Lens correction"),` — six `(flag, label)` pairs.
  `crates/calibraw-ui/src/ui/presets.rs:714:            selection.raw_processing,` ... `:719:            selection.lens_correction,` — the same six flags as a bare array, counted but unlabelled.
  `crates/calibraw-ui/src/ui/presets.rs:823:                    &mut selection.camera_profile,` then `"Camera profile"`, `"Masks"`, `"AI masks"`, `"Crop & geometry"`, `"Lens correction"` — five `(&mut flag, label, help)` triples, and the labels repeat the first list's strings.
  `crates/calibraw-ui/src/ui/presets.rs:768-805: fn show_adjustment_selection` is the only other control panel and it only offers `AdjustmentGroupSet` chips, so `raw_processing` has no chip: `timeout 60 grep -rnE '\.raw_processing\b' crates/calibraw-ui/src --include='*.rs'` returns just :123 and :714 in this file, and `timeout 60 grep -rn 'raw_processing' crates/calibraw-core/src/presets.rs` shows `pub fn suggested_selection(edits...)` (:445) builds its selection with `..EditSelection::default()`, i.e. `raw_processing: false`.
- why it is a problem: the same six-item category list is kept in three places, so a new category (or a renamed label such as `Crop & geometry`) has to be typed into the summary, the counter and the chip row, and the counter silently keeps counting a category the panel cannot set: `raw_processing` is in both the summary and the "N settings selected" count but no chip can turn it on or off, which makes the count and the displayed chips disagree for any preset whose saved selection carries it.
- fix: one table over the selection's optional categories - `(field, label, help, toggleable)` - that `show_additional_selection` renders as chips, `selection_summary` joins for the row tooltip and `selected_count` counts, so each category appears once and a category without a control is stated as such (or dropped from both views).
- effort: M
### [UI-016] The preset panel's two-column breakpoint is a bare `440.0`
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/presets.rs:701 in `show_selection_controls`
- evidence:
  `crates/calibraw-ui/src/ui/presets.rs:701:    if ui.available_width() >= 440.0 {`
  `crates/calibraw-ui/src/ui/presets.rs:702:        ui.columns(2, |columns| {` — the comment above it explains only the column behaviour (`// Each column retains a readable checkbox width; small viewports stack the same cards inside the dialog's bounded scroll area.`), not the number.
  The crate names such widths elsewhere: `crates/calibraw-ui/src/ui/library/actions.rs:3:const SELECTION_BAR_COUNT_LABEL_BREAKPOINT: f32 = 360.0;`, `crates/calibraw-ui/src/app/eframe_impl.rs:381:                const LIBRARY_MIN_SIDEBAR_WIDTH: f32 = 300.0;`. `timeout 60 grep -rnE 'available_width\(\) [<>]=? ?[0-9]' crates/calibraw-ui/src --include='*.rs'` returns this one site plus `ui/develop_viewport.rs:18`, which compares width to height and needs no constant.
- why it is a problem: 440 is the sum of two readable checkbox rows, but the code says only `columns(2)`; nothing keeps the number in step with the card padding and chip widths it was chosen against, and a dialog that grows or shrinks its cards will quietly start stacking the two lists at the wrong width on the phone builds this path exists for.
- fix: name it (`const PRESET_SELECTION_TWO_COLUMN_WIDTH: f32 = 440.0;` next to the function, with the one line of reasoning) and use the existing `moduwu_design` spacing constants where they make up that width.
- effort: S
### [UI-017] One branch chain sets the AI-refresh flag four times, three of them identically
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/app/library_adjustments.rs:166, :171, :173 and :185 inside `CalibRawApp::apply_edit_transfer_to_current` (declared :100)
- evidence:
  `crates/calibraw-ui/src/app/library_adjustments.rs:166:            self.ai.update_needed |= merged.ai_masks_need_update;` (inside `if pipeline_adjustments_changed`)
  `crates/calibraw-ui/src/app/library_adjustments.rs:171:            self.ai.update_needed = merged.ai_masks_need_update;` (inside `if masks_changed`)
  `crates/calibraw-ui/src/app/library_adjustments.rs:173:            self.ai.update_needed |= merged.ai_masks_need_update;` (same branch)
  `crates/calibraw-ui/src/app/library_adjustments.rs:185:                self.ai.update_needed |= merged.ai_masks_need_update;` (inside `if lens_changed`)
  `timeout 60 grep -rn 'ai_masks_need_update' crates/calibraw-ui/src/app/library_adjustments.rs` returns only these four plus `:196:        let needs_ai_refresh = self.ai_update_needed();` and `:383` in the Library path; nothing in finding.md or docs/rework/AUDIT.md mentions it.
- why it is a problem: the same bookkeeping is written into four unrelated category branches, and one of them uses `=` where the others use `|=`, so the branch order silently decides whether the masks branch can undo an earlier `|=`; adding a category to the chain (or reordering it) means hunting for another copy, and the flag has nothing to do with geometry, lens or masks.
- fix: delete the four writes and set the flag once after the branch chain, next to the existing `let needs_ai_refresh = self.ai_update_needed();` at :196, so the "what became dirty" decision and the "what needs AI work" decision are separate statements.
- effort: S
### [UI-018] The AI-mask refresh state is re-fetched 13 times and its "record failure, move on" step is written 6 times
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/library_adjustments.rs inside `start_next_library_ai_mask_refresh` (:571-652), `on_library_ai_mask_refresh_load_finished` (:646-712) and the phase step (:770-813)
- evidence:
  `crates/calibraw-ui/src/app/library_adjustments.rs:781:                    if let Some(state) = self.ai.library_mask_refresh.as_mut() {`
  `crates/calibraw-ui/src/app/library_adjustments.rs:782:                        state.failures.push(format!("{label}: {reason}"));`
  `crates/calibraw-ui/src/app/library_adjustments.rs:783:                        complete_library_ai_mask_refresh_item(state);`
  `crates/calibraw-ui/src/app/library_adjustments.rs:784:                    }`
  `crates/calibraw-ui/src/app/library_adjustments.rs:785:                    self.start_next_library_ai_mask_refresh(frame);`
  `crates/calibraw-ui/src/app/library_adjustments.rs:807:                    if let Some(state) = self.ai.library_mask_refresh.as_mut() {`
  `crates/calibraw-ui/src/app/library_adjustments.rs:808:                        state.failures.push(format!("{label}: {reason}"));`
  `crates/calibraw-ui/src/app/library_adjustments.rs:812:                if let Some(state) = self.ai.library_mask_refresh.as_mut() {`
  `crates/calibraw-ui/src/app/library_adjustments.rs:813:                    complete_library_ai_mask_refresh_item(state);`
  The same `if let Some(state) = self.ai.library_mask_refresh.as_mut() {` guard appears at :572, :605, :628, :659, :668, :677, :704, :716, :753, :781, :807, :812 (and one more), and the `push` plus `complete_library_ai_mask_refresh_item(state)` pair at :605-609, :628-632, :668-670, :704-709, :781-783 and :807-813. `timeout 60 grep -niE 'library_mask_refresh|as_mut\(\)' finding.md docs/rework/AUDIT.md` returns nothing, so this is not one of the known items.
- why it is a problem: the refresh's state is an `Option`, so every step of the job has to re-find it and hope; when the `push` and the `complete_...` calls are split into two guards (:807 and :812 in the Saving phase) forgetting either leaves the queue stuck or the progress counter wrong, and the failure text is built five different ways with no way to see that one path no longer counts the mask as done.
- fix: give the refresh its own method pair on the state type - `record_failure(&mut self, label, reason)` that pushes and calls `complete_library_ai_mask_refresh_item`, and `advance(...)` that clears `current` and starts the next photo - and match on `self.ai.library_mask_refresh` once per function so the borrow checker forces a single `state` binding; that also makes the Saving phase one call instead of two guards.
- effort: M
### [UI-019] Card header buttons are sized with a bare `26.0` in three files
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:167, crates/calibraw-ui/src/ui/sidebar/mask_effects/mod.rs:81, crates/calibraw-ui/src/ui/develop.rs:525
- evidence:
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:167:            let size = egui::vec2(26.0, 26.0);` then `:168:            let reset = moduwu_design::ghost_icon_button(` for the card's reset, mask-overlay and eye buttons.
  `crates/calibraw-ui/src/ui/sidebar/mask_effects/mod.rs:81:                            let size = egui::vec2(26.0, 26.0);` then `:82: let reset_button = moduwu_design::ghost_icon_button(` - the same control on the effect card.
  The same file already draws that control from the design system: `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:341:        let size = moduwu_design::toolbar_icon_size();`, used for the vertical card's icon buttons.
  `timeout 60 grep -rn --include='*.rs' 'egui::vec2(26.0, 26.0)' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests\|/tests/'` returns exactly these three sites, and nothing in finding.md, docs/rework/AUDIT.md or audit-report.md mentions them.
- why it is a problem: the card's reset button and the same reset button on an effect card are 26 pt because three copies say so, while the vertical card's buttons follow a theme-supplied size; `toolbar_icon_size` follows the installed scale, so on a phone build with a different scale the header buttons stay 26 pt and look smaller than the rest of the card, and a scale change edits two of the three copies.
- fix: take the size from `moduwu_design::toolbar_icon_size()` (or add a named `card_action_icon_size()` to the design layer if the header buttons really are a different step) and delete the three literals; `ui/develop.rs:525` is a close-button box and should get the same treatment with the `rect.right() - 34.0` offset named alongside it.
- effort: S
### [UI-020] Card and effect side-channel actions have three separate copies of the same helpers
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:233, :237, :284-292, :306-314, :294-304 and :319-330
- evidence:
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:233:    fn pending_vertical_card_action_id(scope: Option<usize>, title: &'static str) -> egui::Id {`
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:233:        egui::Id::new(("develop-vertical-card-action", scope, title))`
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:237:    fn pending_vertical_effect_action_id(scope: Option<usize>, title: &'static str) -> egui::Id {`
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:237:        egui::Id::new(("develop-vertical-effect-action", scope, title))`
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:284:    fn take_pending_vertical_card_action(ctx: &egui::Context, title: &'static str) -> CardAction {` — its body reads `current_scope(ctx)`, builds the id, then `let action = data.get_temp::<CardAction>(id).unwrap_or_default(); data.remove::<CardAction>(id); action`.
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:306:    pub(super) fn take_pending_vertical_effect_action(` — same four steps with `EffectCardAction`.
  `crates/calibraw-ui/src/ui/sidebar/adjustment_cards.rs:294:    fn queue_vertical_card_action(` and `:319:    fn queue_vertical_effect_action(` are the same `insert_temp` plus `request_repaint`.
  `timeout 60 grep -niE 'vertical|card action' docs/rework/AUDIT.md finding.md` returns nothing, so this repetition is not among the known items (finding.md's `unreachable!` line at :825 covers only the panic site inside one of these functions).
- why it is a problem: the side channel that carries a clicked card action to the pass that applies it exists twice, so the pair can drift - a queued card action that stops matching the id string used by `take_pending_vertical_card_action` is silently dropped by the `unwrap_or_default()`, and the two drift points are eight lines apart in the same file; the card/effect split adds no behaviour, both sides store one copy of one enum under a tag.
- fix: one `vertical_action_id(channel: &'static str, scope, title)` and one generic pair (`queue_vertical_action(ctx, id, action)`, `take_pending_vertical_action<T>(ctx, id)`) with the caller passing `CardAction`/`EffectCardAction` and the channel tag; keep the two public entry points (`register_vertical_effect_actions`, `register_vertical_card_actions`) that do differ.
- effort: S
### [UI-022] "Is the preview at fit zoom" is spelled three ways, one with a different tolerance
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/preview/mod.rs:21-23, crates/calibraw-ui/src/ui/preview/viewport.rs:614, crates/calibraw-ui/src/ui/top_bar.rs:420
- evidence:
  The named predicate is `crates/calibraw-ui/src/ui/preview/mod.rs:21:pub(crate) fn zoom_is_fit(zoom: f32) -> bool {` with `crates/calibraw-ui/src/ui/preview/mod.rs:22:    (zoom - 1.0).abs() <= 0.0005`, used at `crates/calibraw-ui/src/app/preview_state.rs:344` and `crates/calibraw-ui/src/ui/top_bar.rs:43`.
  Two call sites re-derive it inline: `crates/calibraw-ui/src/ui/preview/viewport.rs:614:        if (viewport.zoom - 1.0).abs() > 0.0005 {` (the inverse test, tolerance matching) and `crates/calibraw-ui/src/ui/top_bar.rs:420:            if (app.preview.zoom - 1.0).abs() > f32::EPSILON || app.preview.center != [0.5, 0.5] {`, which is a *different* tolerance by seven orders of magnitude.
  `timeout 90 grep -rn --include='*.rs' -- '- 1.0).abs()' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` shows exactly these three production spellings; `timeout 60 grep -c '0.0018' finding.md docs/rework/AUDIT.md audit-report.md` returns 0.
- why it is a problem: the top bar decides whether a reset request actually moved the view with an exact comparison, so a zoom of 1.0000001 from the slider readout reports motion and re-renders the preview even though `zoom_is_fit` already calls that view fitted; the two answers have to be kept in step by hand across three sites, and a tolerance change edits two of them.
- fix: use `zoom_is_fit` at both inline sites (and negate it where needed), and give the 0.0005 a name (`FIT_ZOOM_EPSILON`) next to `MIN_PREVIEW_ZOOM`/`MAX_PREVIEW_ZOOM` in `ui/preview/mod.rs`.
- effort: S
### [UI-023] The preview paint path is branched three times inside `paint_preview_image`
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/preview/viewport.rs:674-700 and :709-735 (and the same decision at :630-647)
- evidence:
  `crates/calibraw-ui/src/ui/preview/viewport.rs:674:        if viewport.crop_preview {` calls `paint_crop_workspace_texture(ui, texture_id, projection, full_texture, [0.0, 0.0, 1.0, 1.0]);`, `crates/calibraw-ui/src/ui/preview/viewport.rs:682:        } else if viewport.final_geometry_preview {` calls `paint_final_geometry_texture(...)` and the `else` paints `painter.image(texture_id, viewport.image_rect, full_texture, Color32::WHITE);`.
  `crates/calibraw-ui/src/ui/preview/viewport.rs:709:        if viewport.crop_preview {` and `:717:        } else if viewport.final_geometry_preview {` repeat the same two calls for the detail texture with `detail.texture_uv` and `detail_source_uv` instead of the full-UV literal.
  `timeout 90 grep -rn --include='*.rs' 'if viewport.crop_preview' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` gives these three sites (plus the unrelated `visible_source_uv` at :630/:639), and `paint_crop_workspace_texture`/`paint_final_geometry_texture` have no other production callers.
- why it is a problem: the rule "crop previews are painted through the crop workspace, geometry previews through the final geometry, everything else directly" exists three times, and the detail copy has to re-derive the source-UV literal from `source_uv.min/max`; when a mode is added or the `else` arm changes (it already differs: full image rect vs a computed `detail_rect`) only part of the branches follow, which is exactly how a preview that shows the wrong texture for one mode happens.
- fix: one helper - `paint_preview_texture_for_mode(ui, viewport, projection, texture_id, texture_uv, source_uv: Option<[f32;4]>)` that picks the arm and takes the target rect as an argument - called twice in `paint_preview_image`; keep `visible_source_uv`'s branch separate since it computes geometry, not paint.
- effort: M
### [UI-024] The preview placeholder and the format pill position their text with ten bare offsets
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/preview/mod.rs `show_centered_preview_message` (:78-113) and `paint_rendered_source_pill` (:158-171)
- evidence:
  `let title_offset = if detail.is_some() { -11.0 } else { 8.0 };` and `let spinner_offset = if detail.is_some() { -40.0 } else { -20.0 };`
  `egui::Vec2::splat(20.0)`, then `egui::Spinner::new().size(18.0)`, `egui::FontId::proportional(20.0)` for the title, `rect.center() + egui::vec2(0.0, 15.0)` plus `egui::FontId::proportional(14.0)` and `text_color.gamma_multiply(0.74)` for the detail line.
  In the pill: `canvas.left_top() + egui::vec2(12.0, 12.0)`, `egui::FontId::proportional(11.5)`, `text.size() + egui::vec2(16.0, 8.0)`, `painter.rect_filled(pill, 6.0, Color32::from_black_alpha(150))`.
  `timeout 60 grep -niE 'show_centered_preview_message|paint_rendered_source_pill' finding.md docs/rework/AUDIT.md` returns nothing.
- why it is a problem: the spinner sits 40 pt above a 20 pt title only because both numbers were tuned against each other, so making the title text follow the theme's text size moves the title and leaves the spinner and the detail line where they were, which is a visible defect on phone builds with a larger text scale; nothing here says the offsets are relative to the text height.
- fix: derive the offsets from the text sizes (a `PREVIEW_TITLE_FONT_SIZE`/`PREVIEW_DETAIL_FONT_SIZE` pair, spinner size from the title size, offsets as multiples of them), and keep the `if detail.is_some()` stacking as an explicit vertical stack helper for the three elements.
- effort: S
### [UI-025] Batch export hardcodes a 2048 px range-mask proxy, ignoring the platform proxy size
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/batch.rs:226 and :229
- evidence:
  `let source_raw = if raw.width.max(raw.height) <= 2048 {` then `Arc::clone(&raw)` `} else { Arc::new(build_proxy(&raw, ProxySpec { max_edge: 2048 })) };`
  The proxy spec already carries a platform-appropriate default: `crates/calibraw-core/src/pipeline/processing/proxy.rs:16:                1280` under `if cfg!(target_os = "android") { 1280 } else { 2048 }` inside `impl Default for ProxySpec`.
  `timeout 90 grep -rn --include='*.rs' '2048' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` also shows `crates/calibraw-ui/src/ui/preview/overlays.rs:35:        2048` and `crates/calibraw-ui/src/app/processing_export/preview/mod.rs:193:        2048` as separate limit literals (those sit under `PlatformCaps::CURRENT.constrained_memory()`), and nothing in finding.md, docs/rework/AUDIT.md or audit-report.md mentions this site.
- why it is a problem: the batch worker asks for the desktop proxy size on every platform, so a phone export of a 8064-tall RAW allocates the full-size neutral render this path then reads back (`read_output_region_blocking(..., width, height)`) at 2048² instead of the 1280 the platform default chose, which is the memory-constrained case this code exists for; the threshold and the size are the same number written twice, so changing one without the other proxies images that were already small enough.
- fix: use `ProxySpec::default()` (or a named `BATCH_RANGE_MASK_PROXY_EDGE`) and drop the hand-written threshold by comparing against that constant; the sibling `constrained_memory()` limits in `ui/preview/overlays.rs` and `preview/mod.rs` should get named constants in the same pass.
- effort: S
### [UI-026] Two sidecar branches of batch export rebuild the same default edit state
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/batch.rs `prepare_desktop_library_export_item` :190-249
- evidence:
  `crates/calibraw-ui/src/app/processing_export/batch.rs:202:            Ok(_) => {` then `let mut edits = crate::sidecar::default_edit_state(); edits.exposure = default_exposure; let requested = last_camera_profile.and_then(|relative| camera_profile_folder.map(|root| root.join(relative))); (edits, requested, true)`.
  `crates/calibraw-ui/src/app/processing_export/batch.rs:211:            Err(error) => {` is the same six lines after the `log::warn!("ignoring invalid sidecar during batch export for {}: {error}", job.source.display());`.
  The identical expression `last_camera_profile\n .and_then(|relative| camera_profile_folder.map(|root| root.join(relative)))` appears exactly twice in this file (`timeout 60 grep -n 'and_then(|relative' crates/calibraw-ui/src/app/processing_export/batch.rs`).
- why it is a problem: "no sidecar, or an unreadable one, exports like a first import" is written twice, so a change to what an unreadable sidecar should inherit (for instance honouring the sidecar's camera profile while resetting the rest) has to be made in the `Err` arm only and silently diverges from the `Ok(_)` arm; the two arms also differ in whether the user is told at all, since only the `Err` arm logs - the batch then saves these defaults back over the file the user edited.
- fix: `Ok(None_or_without_adjustments)` and `Err` should both call one `default_export_edit_state(context) -> (ExposureParams, Option<PathBuf>, bool)` and only the `Err` arm keeps the log/notice; the `requested` join is computed once above the `match`.
- effort: S
### [UI-027] Batch export prefixes the photo name to errors at the call sites and again in the helper
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/batch.rs :483, :506, :532, :727 and `complete_android_library_batch_export_item` :543-559
- evidence:
  Callers build the label themselves: `crates/calibraw-ui/src/app/processing_export/batch.rs:506:                    self.complete_android_library_batch_export_item(Err(format!(` with `:507: "{display_name}: {error}"` (same at :532/:533 and :483), while `crates/calibraw-ui/src/app/processing_export/batch.rs:727:                        self.complete_android_library_batch_export_item(Err(error));` passes it through unprefixed.
  The helper then prefixes a second time behind a string test: `:551:                    batch.failures.push(if error.starts_with(&name) {` then `error } else { format!("{name}: {error}") });`.
  `timeout 60 grep -n 'display_name}: {error}\|starts_with(&name)\|complete_android_library_batch_export_item(Err' crates/calibraw-ui/src/app/processing_export/batch.rs` shows these five call sites and the one guard; nothing in finding.md or docs/rework/AUDIT.md covers it.
- why it is a problem: which call site labels the failure is invisible to the reader, and the guard that keeps the label from doubling is a prefix comparison on the message itself, so a photo named `IMG_1` reporting an error that begins `IMG_10 …` (the sibling photo's name, or the message starting with its own file name) is accepted as already-labelled and the failure line loses its attribution in the batch summary the user reads to find the bad file.
- fix: have the helper own the label once - callers pass the reason only (`Err("photo load failed".to_owned())`, `Err(error)`) and `complete_android_library_batch_export_item` always writes `format!("{name}: {reason}")`; delete the `starts_with` test. Where a caller already carries a name in the error, pass the name-less reason instead.
- effort: M
### [UI-028] The two batch-export starts repeat the same guard, state and task construction
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/batch.rs `start_android_library_exports` (:325-383) and `start_library_exports` (:647-712)
- evidence:
  `crates/calibraw-ui/src/app/processing_export/batch.rs:340:                    "An export is already running.".to_owned(),` and `crates/calibraw-ui/src/app/processing_export/batch.rs:662:                    "An export is already running.".to_owned(),` are the same `if <list>.is_empty() || self.export.task.is_some() { if self.export.task.is_some() { self.report_error(ErrorKind::ExportUnavailable, ...) } return; }` preamble in both functions.
  Both then build the same two values: `let cancellation = Arc::new(std::sync::atomic::AtomicBool::new(false));`, a `LibraryBatchExportState { pending, current: None, total, completed: 0, failures: Vec::new(), cancel_requested: false, format, settings }` and `ExportTask::new(ExportTaskKind::LibraryBatch, cancellation, <receiver>, None, total)`.
  The same condition is reported to the user with different words elsewhere: `crates/calibraw-ui/src/ui/sidebar/export.rs:425: "An export is already running. Minimize its progress window to keep editing."`
  `timeout 60 grep -rn 'An export is already running' crates --include='*.rs' | grep -v 'tests.rs\|_tests'` gives exactly these three sites; neither this file nor the pair is in finding.md or docs/rework/AUDIT.md.
- why it is a problem: the batch state's zeroing contract (which fields are reset when a batch starts) is written twice, so `completed`/`failures`/`cancel_requested` can drift between the folder and media-library batch and produce a progress bar that starts at the previous run's counts; and the same blocked condition tells the user two different things depending on whether they came from the library bar or the export panel.
- fix: one `start_library_batch_export(&mut self, pending, format, settings, receiver, frame)` that owns the guard, the `LibraryBatchExportState`, the `ExportTask` and the cancellation flag, with the two entry points reduced to mapping their job type into it; hoist the "already running" text to one shared message (or reuse `ui/sidebar/export.rs`'s longer wording) so both entry points say the same thing.
- effort: M
### [UI-029] Every batch export event re-writes the tile counters, the phase text and the progress
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/batch.rs `poll_library_batch_export_worker` (:769-775, :782-788, :795-801) and `on_library_batch_load_finished`'s sibling reset (:561-569)
- evidence:
  `:769:                    if let Some(task) = self.export.task.as_mut() { :770: task.completed_tiles = 0; :771: task.total_tiles = 0; :772: task.phase = "Rendering image…".to_owned(); :773: }` followed by `:774: self.update_library_batch_export_progress();`
  `:782: ... task.completed_tiles = completed_tiles; task.total_tiles = total_tiles; task.phase = "Rendering image…".to_owned();` then the same progress call, and `:795: task.completed_tiles = 0; :796: task.total_tiles = 0; :797: task.phase = "Preparing next image…".to_owned();` then the same progress call; the same three assignments also appear at `:566` and `:412`.
  `timeout 60 grep -n 'completed_tiles = 0;' crates/calibraw-ui/src/app/processing_export/batch.rs` returns :412, :566, :768, :794 - four sites in this file; none are in finding.md or docs/rework/AUDIT.md.
- why it is a problem: the tile counters and the progress fraction are supposed to move together, and the pairing has to be kept by hand at five sites, so a new event arm that updates `task.phase` without the progress call (or resets `total_tiles` without resetting `completed_tiles`) leaves a progress bar that reads 7/12 tiles of a photo that already finished; the phase strings are also retyped per arm, so "Rendering image…" exists in two arms that must stay in step.
- fix: give `ExportTask` a `set_tile_progress(self, completed: u32?, total: u32?, phase: String)` method that assigns the counters and refreshes the phase, and keep the one place that recomputes `progress` (`update_library_batch_export_progress`) called from inside it; the phase literals can stay as arguments.
- effort: S
### [UI-030] The preset's six opt-in setting categories are listed three times
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/presets.rs `selection_summary` (:118-140), `show_additional_selection` (:827-869) and the `selected_count` block of `show_selection_controls` (:750-765)
- evidence:
  `selection_summary` walks `[(selection.raw_processing, "RAW processing"), (selection.camera_profile, "Camera profile"), (selection.masks, "Masks"), (selection.ai_masks, "AI masks"), (selection.geometry, "Crop & geometry"), (selection.lens_correction, "Lens correction")]`.
  `show_additional_selection` walks `(&mut selection.camera_profile, "Camera profile", "The DCP profile chosen for this photo. …"), (&mut selection.masks, "Masks", "Fullscreen, linear and radial masks …"), (&mut selection.ai_masks, "AI masks", …), (&mut selection.geometry, "Crop & geometry", …), (&mut selection.lens_correction, "Lens correction", …)` - five of the same six fields with the same labels.
  `show_selection_controls` counts `selection.adjustment_groups.iter().count() + [selection.raw_processing, selection.camera_profile, selection.masks, selection.ai_masks, selection.geometry, selection.lens_correction].into_iter().filter(|included| *included).count()`.
  `timeout 60 grep -niE 'selection_summary|EditSelection|show_additional_selection|raw_processing' finding.md docs/rework/AUDIT.md` returns nothing, so this is not one of the known items.
- why it is a problem: the set of preset-selectable settings that are not adjustment groups is stated in three places with no shared source, and they already disagree: `raw_processing` gets a chip and a count but no chip in `show_additional_selection`, so the "N settings selected" number can be one higher than the number of chips the user can see, and the summary line in the preset list names a category that cannot be selected here.
- fix: describe the categories once - a small list of `(field: EditSelectionField, label, summary_label, help)` with a getter/setter on `EditSelection` - and drive the summary, the chips and the count from that list; then `raw_processing` gets a chip (or is deliberately excluded once, with the reason written down).
- effort: M
### [UI-031] The white-balance temperature presets are spelled twice in `show_color`
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/develop.rs `show_color` :359 and :435
- evidence:
  The label the combo box shows for the current setting is derived from a list: `crates/calibraw-ui/src/ui/sidebar/develop.rs:359:                } else if let Some(temperature) = [2500.0, 3200.0, 4500.0, 6000.0, 8500.0]` `.into_iter().find(|temperature| raw.white_balance_offsets_from_temperature_tint(*temperature, 0.0).is_some_and(&matches_current))` then `format!("{temperature:.0}K")`.
  The choices the user can pick are a second copy of the same list: `crates/calibraw-ui/src/ui/sidebar/develop.rs:435:                            for temperature in [2500.0, 3200.0, 4500.0, 6000.0, 8500.0] {` with `ui.selectable_label(false, format!("{temperature:.0}K"))`.
  `timeout 90 grep -rn --include='*.rs' '2500.0, 3200.0' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns exactly these two lines, so the pair is the whole duplication; no entry in finding.md or docs/rework/AUDIT.md mentions it (`timeout 60 grep -rn '2500' finding.md docs/rework/AUDIT.md` is empty).
- why it is a problem: the current-selection name and the selectable list are two spellings of one table, and the two also use different builders - the label path asks `white_balance_offsets_from_temperature_tint` while the click path asks `custom_white_balance` - so a preset that one of the two cannot express is drawn as a choice that reports itself as "user modified" once selected, and adding or retuning a temperature means editing both literals in the same 120-line function.
- fix: one `const CAMERA_TEMPERATURE_PRESETS: [(f32, &'static str)]` (or a small `TemperaturePreset` table) iterated by both the label derivation and the selectable rows, keeping one builder for the white balance of a preset.
- effort: S
### [UI-032] Five sliders hand-write the same 0..1 field to 0..100 percent conversion
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/develop.rs `show_detail` (:599-606 Strength, :626-633 Color) and crates/calibraw-ui/src/ui/sidebar/inpainting.rs (:121 feather, :131 opacity, :246 opacity)
- evidence:
  `crates/calibraw-ui/src/ui/sidebar/develop.rs:599:                let mut strength_percent = exposure.ai_denoise_amount.clamp(0.0, 1.0) * 100.0;` then `:600: if AdjustmentSlider::new("Strength", &mut strength_percent, 0.0..=100.0)` then `:606: exposure.ai_denoise_amount = strength_percent / 100.0;`
  `crates/calibraw-ui/src/ui/sidebar/develop.rs:626:                let mut color_percent = exposure.chroma_denoise.clamp(0.0, 1.0) * 100.0;` with the same slider and `:633: exposure.chroma_denoise = color_percent / 100.0;`
  `crates/calibraw-ui/src/ui/sidebar/inpainting.rs:121:                    let mut feather_percent = (1.0 - app.inpaint.brush_hardness) * 100.0;`, `:131: let mut opacity_percent = app.inpaint.brush_opacity * 100.0;` and `:246: let mut opacity_percent = opacity * 100.0;` are the same read/slider/write-back with no clamp.
  `timeout 90 grep -rn --include='*.rs' '\* 100.0;' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` lists these five sites (the other hits convert a computed value once, not a slider pair); nothing in finding.md or docs/rework/AUDIT.md covers them.
- why it is a problem: the round trip (scale up, let the slider drag in percent, scale back) is written per control, so the pair can drift on one side only - two of the five clamp the source value and three do not, so an out-of-range stored value reaches the slider and paints past 100, and the `Strength` reset at `:604` (`defaults.ai_denoise_amount * 100.0`) is not clamped while the Color reset at `:630` is; a control whose write-back was missed leaves the slider moving a copy that nothing reads.
- fix: add one helper next to `AdjustmentSlider` in the ui crate - `percent_slider(label, &mut fraction, range_max_percent, default_fraction, decimals, step)` that does the `*100.0` read, `reset_to(default * 100.0)` and the `/100.0` write-back once - and call it from all five controls; the inversion for brush hardness (`1.0 - hardness`) stays at the inpainting call site as an argument.
- effort: S
### [UI-033] The review badge measures its width twice, and retypes its margin as a literal
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/library/review.rs `paint_review_badge` (:158-215)
- evidence:
  The full-width expression: `let full_required_width = if has_rating { full_rating_width } else { 0.0 } + if has_flag { flag_width + if has_rating { BADGE_GAP } else { 0.0 } } else { 0.0 } + BADGE_MARGIN * 2.0;`
  Then, seven lines later, the same expression with one term swapped: `let required_width = if has_rating { rating_width } else { 0.0 } + if has_flag { flag_width + if has_rating { BADGE_GAP } else { 0.0 } } else { 0.0 } + BADGE_MARGIN * 2.0;`
  The function declares `const BADGE_MARGIN: f32 = 8.0;` but then writes `let margin = 8.0 * scale;`, and the corner radius is the literal `6.0` in both badge backgrounds (`painter.rect_filled(rating_badge, 6.0 * scale, ...)` and `painter.rect_filled(flag_badge, 6.0 * scale, ...)`) while star/flag sizes are the literals `11.0 * scale` and `12.0 * scale`.
  `timeout 60 grep -niE 'paint_star|paint_flag|BADGE|review.rs|badge' finding.md docs/rework/AUDIT.md` returns nothing, so this file has no known items.
- why it is a problem: the two width formulas decide whether the badge compacts (`let compact_rating = review.rating >= 3 && full_required_width > rect.width();`) and whether it fits at all, so a change to spacing (`BADGE_GAP`, the flag's padding) that reaches one formula and not the other produces a badge that measures one width and draws another - stars clipped at the thumbnail edge on narrow phone grids; the `8.0` literal means `BADGE_MARGIN` no longer describes the layout, so a rename of the constant changes one of the two margins.
- fix: one local `badge_width(rating_width)` closure used by both measurements, plus named `BADGE_RADIUS`, `STAR_EDGE` and `FLAG_EDGE` consts next to the existing ones, with `margin = BADGE_MARGIN * scale`.
- effort: S
### [UI-034] The star-and-flag review row is laid out with bare 5 and 7 in eight places
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/library/review.rs `thumbnail_hover_overlay` (:72), `review_editor` (:266-342) and `review_button_rect` (:356-368)
- evidence:
  `:266:    let hovered_rating = (0..5)` and `:272:    for index in 0..7 {` in the same function, with `:288: || (!overlay && index >= 5 && active)`, `:300:        let value = if index < 5 {`, `:342:        let x = bounds.left() + 6.0 + edge * 5.0 + 6.0;` (the divider between stars and flags) and in `review_button_rect`: `:357: egui::vec2((index as f32 - 2.5) * edge, 0.0)`, `:359: egui::vec2((index as f32 - 6.0) * edge, edge)`, `:368: bounds.left() + 6.0 + edge * index as f32 + if index >= 5 { 12.0 } else { 0.0 }`.
  The same limit is also written as a comparison in core: `crates/calibraw-core/src/sidecar.rs:389:    if review.rating > 5 {` and `crates/calibraw-core/src/sidecar/files.rs:257:    if review.rating > 5 {`, with `crates/calibraw-ui/src/ui/library/actions.rs:53: for rating in 0..=5 {` and `crates/calibraw-ui/src/ui/library/filter.rs:31: let ratings = (0..=5)` offering the rating menu and filter - `timeout 90 grep -rn --include='*.rs' 'MAX_RATING\|rating > 5\|0\.\.=5' crates | grep -v 'tests.rs\|_tests\|/tests/'` finds no named constant anywhere in the workspace.
- why it is a problem: `7` is really "stars + 2 flags" and `6.0`/`2.5` are derived from the star count, so nothing states the relationship: the row breaks when the count in any one place changes (a rating of 6 becomes selectable through `actions.rs` while `review_editor` has no sixth button, and core's `> 5` then rejects the value the UI offered), and the divider at :342 can silently sit inside the star group.
- fix: `const RATING_STARS = 5` (or, better, name it in core once as `PhotoReview::MAX_RATING` and derive the UI const from it) plus `const REVIEW_BUTTONS = RATING_STARS + 2`; express the divider and the group-centring offsets from those, and have `actions.rs`, `filter.rs` and the two core validators use the same constant.
- effort: M
### [UI-035] Both `LibraryAsset` constructors write the same metadata literal with four placeholder zeros
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/library/mod.rs `LibraryAsset::from_desktop_path` (:212-245) and `LibraryAsset::from_android_document` (:266-290)
- evidence:
  `from_desktop_path` ends with `metadata: LibraryAssetMetadata { review, bytes, dimensions_hint,` then `iso_speed: 0.0,` `shutter_seconds: 0.0,` `focal_length: 0.0,` `aperture: 0.0,` `modified_seconds, },`.
  `from_android_document` repeats the same literal: `metadata: LibraryAssetMetadata { review: crate::sidecar::PhotoReview::default(), bytes: document.bytes, dimensions_hint: None, iso_speed: 0.0, shutter_seconds: 0.0, focal_length: 0.0, aperture: 0.0, modified_seconds: document.modified_seconds, },`.
  `LibraryAssetMetadata` already derives `Default` (`#[derive(Clone, Debug, Default)]` at `:305`), so the zero block exists in the type and neither constructor uses it; `timeout 90 grep -rn --include='*.rs' 'iso_speed: 0.0' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` returns only these two blocks.
- why it is a problem: the "capture settings unknown" state is spelled per constructor, so the two platform paths can state different unknowns (a folder photo whose EXIF has no aperture is indistinguishable from one whose field a future constructor forgot to fill), and a new capture field must be zeroed in every constructor.
- fix: one `LibraryAssetMetadata::unknown_capture()` helper (or build the literal from `LibraryAssetMetadata()` defaults and set the three real fields), used by both constructors.
- effort: S
### [UI-036] The same clipboard mode and the same rename-dialog state are declared per kind
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/library/mod.rs (:473-476 and :548-551 clipboard modes; :655-683 the three name dialogs)
- evidence:
  Two structurally identical enums in one file: `enum ImageClipboardMode { Copy, Cut, }` and `enum LibraryFolderClipboardMode { Copy, Cut, }`, neither derived from serde; `timeout 60 grep -rnE 'ImageClipboardMode|LibraryFolderClipboardMode' crates/calibraw-ui/src --include='*.rs'` shows 6 uses of the image kind in ui/library/clipboard.rs (`let verb = if mode == ImageClipboardMode::Copy {`) and 6 of the folder kind in ui/library/folders/mod.rs (`LibraryFolderClipboardMode::Copy => LibraryFolderOperation::Copy {`) - the same "keep the source" / "move the source" pair of meanings.
  The three rename dialogs carry the same three fields: `LibraryFolderNameDialog { kind: LibraryFolderNameDialogKind, name: String, error: Option<String>, focus_requested: bool }`, `LibraryRawNameDialog { asset: LibraryAsset, name: String, error: Option<String>, focus_requested: bool }` and `AndroidLibraryFolderNameDialog { parent: String, name: String, error: Option<String>, focus_requested: bool }`.
- why it is a problem: the editable-text dialog is the same interaction (text, last rejection, keyboard focus) restated per owner, so accessibility state and the error echo can drift between folder, file and Android renames (a focus request handled in one dialog and not another); and because the clipboard modes are separate types, a helper written for one cannot be reused by the other, and code comparing the wrong kind still compiles inside each module.
- fix: one `ClipboardMode` enum in ui/library/mod.rs used by both clipboards, and one `NameDialogState { name, error, focus_requested }` composed with the per-kind owner (`kind`, `asset` or `parent`), keeping the serialized behaviour unchanged (neither enum is persisted).
- effort: M
### [UI-037] Recording a GitHub permission denial clears the check state in two hand-copied blocks
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/version_update.rs `revoke_version_check_permission` (:277-281) and the consent dialog's `Some(ConsentAction::Deny)` arm (:460-464)
- evidence:
  `revoke_version_check_permission` starts `self.preferences.github_update_check_allowed = Some(false); self.preferences.auto_check_updates = false;` then `self.ui.version_check.receiver = None;` `self.ui.version_check.consent_dialog = None;` `self.ui.version_check.dialog = None;` `self.ui.version_check.status = VersionCheckStatus::NotChecked;` `self.persist_version_check_preferences();`.
  The `Deny` arm of `show_version_check_consent_dialog` repeats those same six statements, four of them on the state struct.
  `timeout 60 grep -rn --include='*.rs' 'version_check.status = VersionCheckStatus::NotChecked' crates | grep -v tests` returns exactly these two sites, so the pair is the whole duplication.
- why it is a problem: a denial is one decision expressed in two code paths, and the state has five fields, so clearing a check in one path and not the other leaves a live `receiver` (the app keeps polling a worker whose result nobody will show) or a stale `dialog` (an "update available" popup redraws after the user revoked permission), and adding a sixth field to `VersionCheckState` means finding both copies.
- fix: a `VersionCheckState::clear_check()` method that resets the four transient fields, called by both paths; the two preference writes and the persist call stay with the callers, which differ.
- effort: S
### [UI-038] Subject-refinement interaction repeats the mask-interaction bookkeeping
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/ai/state.rs `note_mask_geometry_interaction` (:197-210) and `note_subject_refinement_interaction` (:213-226)
- evidence:
  `note_mask_geometry_interaction` opens `if self.masks.interaction_dirty_layer != Some(layer) { self.finish_mask_geometry_interaction(); self.masks.interaction_dirty_layer = Some(layer); self.masks.interaction_last_upload = None; }` then `self.masks.overlay_revision = self.masks.overlay_revision.wrapping_add(1); self.masks.interaction_has_uncommitted_change = true; self.flush_mask_geometry_interaction();`
  `note_subject_refinement_interaction` repeats it with one substitution: `if self.masks.interaction_dirty_layer != Some(SHARED_REFINEMENT_LAYER) { self.finish_mask_geometry_interaction(); self.masks.interaction_dirty_layer = Some(SHARED_REFINEMENT_LAYER); self.masks.interaction_last_upload = None; }` then the same three statements.
  `timeout 90 grep -rn --include='*.rs' 'interaction_dirty_layer = Some' crates | grep -v tests` returns exactly these two sites, so both are the whole pattern; each comment inside ("Live coverage follows every edit, independently of throttled uploads." / "Shared refinement coverage must also update between upload commits.") restates the same rule.
- why it is a problem: the interaction slot is one piece of state (which layer is dirty, when it last uploaded, whether coverage is uncommitted) owned by the app, but it is written from two entry points, so the throttle contract - `flush_mask_geometry_interaction` reads `interaction_last_upload` and `INTERACTIVE_MASK_INTERVAL` - can be entered with different bookkeeping by the shared-refinement path; a later rule (say a longer refinement interval) has to be added to both copies, and the second copy's `SHARED_REFINEMENT_LAYER` literal hides that the refinement slot is the pseudo-layer `MAX_LOCAL_MASKS`.
- fix: keep one `note_interaction_layer(&mut self, layer: usize)` body and make `note_subject_refinement_interaction` a call with `SHARED_REFINEMENT_LAYER`, keeping the two comments as a single doc comment on the shared function.
- effort: S
### [UI-039] The interaction commit step is written twice, and the pseudo-layer is tested two ways
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/ai/state.rs `flush_mask_geometry_interaction` (:239-246) and `finish_mask_geometry_interaction` (:252-275)
- evidence:
  The throttled flush commits through the named alias: `if layer == SHARED_REFINEMENT_LAYER { self.mark_all_mask_layers_dirty(); } else { self.mark_mask_geometry_dirty(layer); }` followed by `self.masks.interaction_last_upload = Some(now); self.masks.interaction_has_uncommitted_change = false;`.
  `finish_mask_geometry_interaction` repeats the dispatch through the raw bound: `if let Some(layer) = layer { if layer == MAX_LOCAL_MASKS { self.mark_all_mask_layers_dirty(); } else { self.mark_mask_geometry_dirty(layer); } }`, after already clearing the same pair above (`self.masks.interaction_last_upload = None; self.masks.interaction_has_uncommitted_change = false;`).
  The alias is declared at the top of the file: `crates/calibraw-ui/src/app/ai/state.rs:8: const SHARED_REFINEMENT_LAYER: usize = MAX_LOCAL_MASKS;`, and `timeout 90 grep -rn --include='*.rs' 'layer == MAX_LOCAL_MASKS\|layer == SHARED_REFINEMENT_LAYER' crates | grep -v tests` returns only these two sites.
- why it is a problem: the same "which layers does this commit dirty" rule lives in two functions, one of which re-derives the special slot from `MAX_LOCAL_MASKS` instead of the alias - if the state ever grows a second pseudo-slot (or `MAX_LOCAL_MASKS` changes) the two paths disagree, so a mask edit uploads the wrong atlas set, showing stale coverage in the preview while the GPU state is the other set; `interaction_dirty_layer` is also an out-of-range index (`usize`) standing in for "shared refinement", which the type does not express.
- fix: one `commit_interaction_layer(&mut self, layer: usize)` called by both paths, comparing against `SHARED_REFINEMENT_LAYER`, and type the interaction slot as `Option<MaskInteractionSlot>` (`Layer(index)` / `SharedRefinement`) so the sentinel number disappears from both call sites.
- effort: M
### [UI-040] Idle repaint retries choose their delay with bare millisecond literals at 20 sites
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/preview/detail.rs:84, rebuild.rs:525, processing_export/lens.rs:196, app/foreground.rs:133, app/inpainting.rs:169/227/282, app/preview_histogram.rs:97/130, app/sidecar_persistence.rs:410, app/eframe_impl.rs:237/240, ui/preview/tools/masks/overlay.rs:60, ui/sidebar/masks/strip.rs:495/508, ui/sidebar/info.rs:36, ui/settings.rs:288, app/lifecycle/pickers.rs:211
- evidence:
  `timeout 120 grep -rn --include='*.rs' 'request_repaint_after' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests\|/tests/' | grep 'from_millis\|from_secs'` returns 20 sites whose delay is a literal, e.g. `crates/calibraw-ui/src/app/processing_export/preview/detail.rs:84: .request_repaint_after(Duration::from_millis(50));`, `crates/calibraw-ui/src/app/inpainting.rs:227: .request_repaint_after(Duration::from_millis(16));`, `crates/calibraw-ui/src/app/sidecar_persistence.rs:410: .request_repaint_after(Duration::from_millis(1_200));`, `crates/calibraw-ui/src/app/eframe_impl.rs:237: ctx.request_repaint_after(Duration::from_millis(80));` and `:240: ... from_millis(120)`.
  The two values repeat across owners: `from_millis(50)` at 6 sites, `from_millis(16)` at 4 sites. The repo already names such delays elsewhere - `crates/calibraw-ui/src/ui/library/mod.rs:79-80 const THUMBNAIL_PAUSE_POLL_INTERVAL / THUMBNAIL_QUEUE_POLL_INTERVAL`, `crates/calibraw-ui/src/app/ai/state.rs:7 INTERACTIVE_MASK_INTERVAL` - so the bare ones are the outliers. `timeout 60 grep -niE 'repaint_after|from_millis' finding.md docs/rework/AUDIT.md` finds no known item.
- why it is a problem: a retry delay is a contract with the polling loop it belongs to (16 ms means "ask again next frame", 50 ms means "still working, check again soon", 1200 ms means "offer undo soon after"), and the number carries no owner, so the loops that mean the same thing can drift apart (four "next frame" retries at 16 ms and one at 25 ms behave differently), and tuning responsiveness for phones means hunting literals through nine files.
- fix: name each delay next to the loop that schedules it (`const DETAIL_IDLE_RETRY = 50 ms`, `const FOREGROUND_POLL = 50 ms`, `const UNDO_OFFER_AFTER_SAVE = 1200 ms`, `const NEXT_FRAME_RETRY = 16 ms`) and reuse the existing consts where the interval already has a name; the literals at :237 and :240 in eframe_impl.rs become two named idle levels.
- effort: M
### [UI-041] The undo/redo pair travels as a bool and is re-decided into an AppAction at every use
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/top_bar.rs `show_history_controls` (:200-231) with `history_icon_button` (:181-198)
- evidence:
  The loop carries a flag: `for (redo, tooltip) in [(false, undo_tip), (true, redo_tip)] {` then re-derives the action twice: `app.action_enabled(if redo { AppAction::RedoEdit } else { AppAction::UndoEdit }),` and `app.dispatch_action(if redo { AppAction::RedoEdit } else { AppAction::UndoEdit });` while `history_icon_button` re-derives it a third time for the glyph (`let icon = if redo { egui_phosphor::regular::ARROW_U_UP_RIGHT } else { egui_phosphor::regular::ARROW_U_UP_LEFT };`).
  `timeout 60 grep -rn --include='*.rs' 'if redo {' crates | grep -v tests` returns exactly these three sites in this one function pair; `timeout 60 grep -niE 'redo|UndoEdit' finding.md docs/rework/AUDIT.md` finds no known item.
- why it is a problem: `redo: bool` is an encoded enum, so the two decisions are independent and only one is inverted the other way round (`false` first here), meaning the enabled-check and the dispatch can name different actions - a toolbar button that reports Undo enabled but dispatches Redo - and the shortcut hint is spelled in four literals instead of once per action.
- fix: iterate over the actions themselves, `for (action, label) in [(AppAction::UndoEdit, "Undo the last edit"), (AppAction::RedoEdit, "Redo the last edit")]`, pass `action` to `action_enabled` and `dispatch_action`, and take the `redo` glyph argument from `action == AppAction::RedoEdit` inside `history_icon_button` (or change that parameter to the `AppAction`).
- effort: S
### [UI-042] The tablet and desktop toolbars restate the same control spine
- category: duplicate
- severity: low
- confidence: medium
- where: crates/calibraw-ui/src/ui/top_bar.rs `show_tablet` (:505-525) and `show_desktop` (:528-560 and the inner block at :640-680)
- evidence:
  `show_tablet` opens `moduwu_design::prepare_toolbar(ui); let compact_review = ui.available_width() < TOOLBAR_COMPACT_REVIEW_WIDTH; ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { Self::show_develop_dock_controls(ui, app, compact_review); app.show_background_tasks(ui); ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| { if Self::back_icon_button(...).clicked() { app.activate_tab(AppTab::Library); } ui.separator(); Self::show_history_controls(ui, app, false); Self::show_save_control(ui, app, false); }); });`
  `show_desktop` performs the same calls in the same nesting - `Self::show_develop_dock_controls(ui, app, compact_review);` then `app.show_background_tasks(ui);` then the left-to-right group with `Self::show_history_controls(ui, app, true); Self::show_save_control(ui, app, true);` - with the desktop adding the tab switcher, search field, brand and original-toggle.
  `timeout 60 grep -niE 'top_bar|toolbar|show_tablet|show_desktop' finding.md docs/rework/AUDIT.md` mentions only `top_bar.rs:127 .expect("embedded toolbar brand must be a valid PNG")`, so the layout duplication itself is untracked.
- why it is a problem: the ordering of the toolbar's shared controls (dock controls, background tasks, then the history/save group) is stated per platform, so the two can drift - an added control lands in one toolbar only, and the `show_history_controls(..., <shortcut hint flag>)` argument is hand-set to `false`/`true` at four call sites, which is where a phone build starts advertising Ctrl/Cmd shortcuts it cannot press.
- fix: one `show_toolbar_group(ui, app, hints)` that draws the shared spine in order and takes a small `ToolbarHints` value (tab switcher on/off, search on/off, keyboard hints on/off), called from both entry points, keeping the platform choice at the `show` dispatcher that already exists at `:404`.
- effort: M
### [UI-043] `Visualize range` toggle is wrapped in an empty `if` block
- category: unnecessary-code
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/components/point_color.rs `point_color` (:204-207)
- evidence:
  `204:        if moduwu_design::toggle_with_help(ui, &mut state.visualize_range, "Visualize range", "Show the selected range with the same blue translucent overlay used for masks. Feathered pixels use a softer overlay. This preview is never exported.")` `205:            .changed()` `206:        {` `207:        }`
  The block is empty: nothing is assigned, marked dirty or returned, and the function's value is `before != *colors` at `:209` (`PointColors` is unaffected by the toggle, so the result is never observed here). `timeout 60 grep -rn --include='*.rs' 'toggle_with_help' -A6 crates/calibraw-ui/src | grep -n 'changed()' -A2` shows the sibling usages do use the result (`crates/calibraw-ui/src/ui/sidebar/mask_effects/fog.rs:178-180: if ...toggle_with_help(...).changed() { changed = true; }`), so this site is the outlier; finding.md and docs/rework/AUDIT.md contain no entry for point_color.rs.
- why it is a problem: an empty branch reads as an unfinished change - a reader cannot tell whether the caller is meant to react to the toggle or whether the branch was deleted, and the discarded `Bool` says nothing about which. It is also a trap for the next edit: filling the branch in later silently changes what `point_color` reports as changed.
- fix: call the toggle as a statement - `moduwu_design::toggle_with_help(ui, &mut state.visualize_range, ...);` - dropping the `.changed()` result entirely, or return the toggle's `Bool` if the caller is meant to see range-visualization changes.
- effort: S
### [UI-044] Point Color ranges carry the channel twice: a field plus a bare axis index
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/components/point_color.rs `range_editor` (:408-448) and its callers (:200-202); same pattern in `set_point_color_feather` (:313-318)
- evidence:
  The three call sites pass both the range and its index: `range_editor(ui, "Hue range", &mut point.hue_range, sample, 0);`, `range_editor(ui, "Saturation range", &mut point.saturation_range, sample, 1);`, `range_editor(ui, "Luminance range", &mut point.luminance_range, sample, 2);`
  Inside, the index re-picks the range's own domain and default: `let limit = if axis == 0 { 0.5 } else { 1.0 };` then `let default = [ defaults.hue_range, defaults.saturation_range, defaults.luminance_range ][axis];` and the track gradient uses `hsl[axis] = if axis == 0 { (sample[axis] + offset).rem_euclid(1.0) } else { (sample[axis] + offset).clamp(0.0, 1.0) };`, ending with `*range = track.range;` (`:446`).
  The same per-channel limit is spelled again by field: `set_range_feather(&mut point.hue_range, feather, 0.5); set_range_feather(&mut point.saturation_range, feather, 1.0); set_range_feather(&mut point.luminance_range, feather, 1.0);` at :315-317.
- why it is a problem: the range argument and the index are two names for one channel and nothing relates them, so `range_editor(ui, "Hue range", &mut point.saturation_range, sample, 0)` compiles and writes a saturation range built on the hue domain (`limit = 0.5`, hue default, hue-rem_euclid shading), producing a range outside its own domain; `valid_range` then clamps against that wrong limit, so the photo's range is silently altered and sidecar validation's bounds (`-limit..=limit`) disagree with what the UI stored; and the hue-vs-other distinction (0.5 vs 1.0, rem_euclid vs clamp) is restated in two shapes, so a new channel must be added to both.
- fix: introduce one `PointColorChannel { Hue, Saturation, Luminance }` in this module holding `label()`, `limit()` and accessors for the range, default range and HSL index; take that instead of `(label, range, axis)` and use it in `set_point_color_feather`, so the channel is named once and cannot disagree with itself.
- effort: M
### [UI-045] `MaskStripOrientation` means "card flow" in one file and "panel" in the id salts, while the strip wrappers invert it
- category: rule-violation
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar.rs:33 (enum), crates/calibraw-ui/src/ui/sidebar/masks/strip.rs:28-45 (wrappers) and :88-100 (salts), crates/calibraw-ui/src/ui/sidebar/masks/details.rs:180-194
- evidence:
  The enum has no doc comment: `crates/calibraw-ui/src/ui/sidebar.rs:33: enum MaskStripOrientation {` `Horizontal,` `Vertical,`
  The strip wrappers pass the opposite variant to their own name: `strip.rs:36: Self::show_vertical_mask_strip(...) -> Self::show_mask_strip(ui, app, frame, MaskStripOrientation::Horizontal);` and `strip.rs:44: show_horizontal_mask_strip -> ... MaskStripOrientation::Vertical`, and inside `show_mask_strip_contents` the `Horizontal` arm lays the cards out horizontally (`:88: MaskStripOrientation::Horizontal => { egui::ScrollArea::horizontal()`) but salts it `:90: .id_salt("vertical-mask-card-strip")`, while the `Vertical` arm salts `:100: .id_salt("horizontal-mask-card-strip")`.
  The sibling details wrappers use the other convention: `details.rs:185: show_masks_horizontal_details -> Self::show_mask_details(ui, app, frame, MaskStripOrientation::Horizontal)` and `details.rs:193: show_masks_vertical_details -> ... MaskStripOrientation::Vertical`.
  `timeout 90 grep -rn --include='*.rs' 'MaskStripOrientation' crates | grep -v 'tests.rs\|_tests\|/tests/'` shows 30 sites over sidebar.rs, strip.rs, details.rs; `timeout 60 grep -niE 'MaskStripOrientation|orientation' finding.md docs/rework/AUDIT.md` finds no known item.
- why it is a problem: the same words describe different geometry in the same module - `show_vertical_mask_strip` and `show_masks_vertical_details` both say "vertical" but hand the enum opposite variants - so any new caller has a 50/50 chance of picking the wrong layout, and the wrong choice compiles and silently re-flows the whole strip (cards swap axes, `in_leading_half` at :612 picks the wrong axis, `scroll_strip_near_edge` scrolls the wrong one). The egui id salts also carry the other convention, so persistence identity for a strip is named by panel while the layout code reads it as flow.
- fix: name the variants by what they control (`MaskStripLayout::CardsFlowHorizontal` / `CardsFlowVertical`, or `LeftRailStrip` / `BottomStrip` chosen by panel), document the enum once at `sidebar.rs:33`, and rename the two strip wrappers' arguments so their names match the variant; the id salts move to constants derived from the variant so the persistence key follows the layout it names.
- effort: M
### [UI-046] The mask-strip drag state and its Android overflow menu are built twice, for groups and for sub-masks
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/masks/strip.rs `show_mask_group_card` (:223-231, :246-252) and `show_submask_card` (:379-393, :398-404)
- evidence:
  The same 9-field state literal appears twice: `drag.state = Some(StripDragState { source: StripDragSource::Group(index), source_texture: input.group_textures.get(index).cloned(), source_name: mask.name.clone(), source_badge: badge, source_enabled: mask.enabled, hover_group: None, drop_target: Some(StripDropTarget::Group(index)), target_loss_started: None, });` at :223 and the same fields with a `Component { mask_index, component_index }` source at :379 - including the two fields whose values are fixed there (`hover_group: None` at :229/:388 and `target_loss_started: None` at :231/:393).
  The Android overflow-menu block is the same 6 lines twice: `let overflow_clicked = crate::platform::PlatformCaps::CURRENT.is_mobile() && { let menu_id = ui.make_persistent_id(("android-mask-group-overflow", index)); crate::ui::android_overflow_menu(ui, response.rect, menu_id, 22.0, |ui| { Self::mask_group_context_menu(ui, mask, can_add_group, index, requests); }).clicked() };` versus `("android-submask-overflow", mask_index, component_index)` with `20.0` and `Self::submask_context_menu(...)`.
  `timeout 90 grep -rn --include='*.rs' 'StripDragState {' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns only these two construction sites (plus the declaration in mod.rs); nothing in finding.md or docs/rework/AUDIT.md names them.
- why it is a problem: the drag is one state machine owned by `StripDrag` (mod.rs:101) but each of its two entry points restates the whole record, so a field added for drag - e.g. a `target_loss_started` timer or a `hover_group` default - must be set in both copies, and one-sided edit makes a group drag and a sub-mask drag behave differently (one re-hovers a group, the other resets); the two overflow menus also carry their own hit radius (`22.0` vs `20.0`) as literals, which is where the touch target for one kind shrinks below the other unnoticed.
- fix: a `StripDragState::starting(source, texture, name, badge, enabled, drop_target)` constructor that owns `hover_group: None` and `target_loss_started: None`, plus one `android_overflow_menu_for(ui, response, id_parts, size, menu)` helper called with the per-kind payload.
- effort: M
### [UI-047] The repaint quantum and idle keep-alive are bare 16 ms / 50 ms literals at 11 sites
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: calibraw-ui - app/processing_export/preview/detail.rs:84, app/processing_export/preview/rebuild.rs:525, app/processing_export/lens.rs:196, app/foreground.rs:133, app/inpainting.rs:282, ui/sidebar/masks/strip.rs:508 (all `Duration::from_millis(50)`); app/ai/state.rs:236, app/inpainting.rs:227, app/preview_histogram.rs:97, app/preview_histogram.rs:130, ui/sidebar/masks/strip.rs:495 (all `Duration::from_millis(16)`)
- evidence:
  `timeout 90 grep -rn --include='*.rs' 'from_millis(16)\|from_millis(50)' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests\|/tests/'` returns those 11 production sites (plus app/decoded_raw_cache.rs:342, which is a 50 ms sleep inside that file's test module and is excluded). Every one reads `.request_repaint_after(Duration::from_millis(50));` or `.request_repaint_after(Duration::from_millis(16));` with no constant and no comment on what the number is for.
  The same crate already names its timing contracts: `crates/calibraw-ui/src/app/ai/state.rs:3: const INTERACTIVE_MASK_INTERVAL: Duration = Duration::from_millis(45);`, `app/preview_histogram.rs:4: const UPDATE_INTERVAL: Duration = Duration::from_millis(100);`, `app/sidecar_persistence.rs:7: const SIDECAR_AUTOSAVE_INTERVAL: Duration = Duration::from_millis(900);`, `ui/library/mod.rs:79-80: const THUMBNAIL_PAUSE_POLL_INTERVAL / THUMBNAIL_QUEUE_POLL_INTERVAL ...`, `ui/preview/interaction.rs:16: const HOLD_TIME: std::time::Duration = std::time::Duration::from_millis(350);` - and even strip.rs names the neighbouring values in the same file (`strip.rs:159 const EDGE: f32 = 40.0;`, `:160 const MAX_SPEED: f32 = 600.0;`, `:621 const HOLD_SECONDS: f64 = 0.35;`).
  `timeout 60 grep -niE 'from_millis|repaint' finding.md docs/rework/AUDIT.md` has no matching entry.
- why it is a problem: 16 ms is "one frame at 60 Hz" and 50 ms is the keep-alive step for an in-progress job, and neither is stated - so the frame-rate assumption is restated in five files, and a display refresh change (120 Hz, or a throttled remote session) means hunting eleven literals across six files, with no way to tell whether a 50 in a new place means the same contract or a new one.
- fix: one module-level pair in calibraw-ui - `const REPAINT_ONE_FRAME: Duration = Duration::from_millis(16);` (comment the assumed refresh rate) and `const ACTIVE_JOB_REPAINT: Duration = Duration::from_millis(50);` - used at all eleven sites, following the pattern the crate already uses for its other intervals.
- effort: S
### [UI-048] Mobile tab layout is passed around as positional 4- and 2-float tuples
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/navigation.rs:10 `mobile_tab_text_geometry`, :21 `mobile_tab_icon_geometry`, used at navigation.rs:23 and ui/sidebar/navigation/mobile.rs:349, :358
- evidence:
  `navigation.rs:10: pub(super) fn mobile_tab_text_geometry(height: f32) -> (f32, f32, f32, f32) {` returning `let (icon_size, label_size, icon_center, label_center) = ...; (icon_size, label_size, icon_center, label_center)` (`:19`), and `navigation.rs:21: pub(super) fn mobile_tab_icon_geometry(height: f32, show_label: bool) -> (f32, f32)` returning `(icon_size, icon_center)` in one branch and `((height * 0.5).clamp(21.0, 25.0), height * 0.5)` in the other.
  Every caller re-derives the order by hand: `navigation.rs:23: let (icon_size, _, icon_center, _) = mobile_tab_text_geometry(height);` and `mobile.rs:358: let (_, label_size, _, label_center) = mobile_tab_text_geometry(size.y);` - two of the four call sites throw half the tuple away with `_` placeholders (`timeout 90 grep -rn --include='*.rs' 'mobile_tab_text_geometry\|mobile_tab_icon_geometry' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests\|/tests/'` lists exactly these three plus the sidebar.rs:112 import; the remaining hits are in the file's test module).
- why it is a problem: all four values are pixels of different kinds (two font sizes, two offsets from the tab top), so nothing distinguishes them: swapping `icon_center` and `label_center` - or a font size for an offset - compiles and puts the label above the icon, and the two callers that read positions 1 and 3 with `_` in the other slots make the mistake invisible, since the discarded values are exactly the ones that would look wrong.
- fix: return named values once - `MobileTabText { icon_size, label_size, icon_center, label_center }` and `MobileTabIcon { icon_size, icon_center }` in this module - so callers use fields; the magic `54.0` breakpoint that selects the two label sizes and gaps (`:14-15`) becomes one named constant both branches share.
- effort: M
### [UI-049] `show_adjustments` restates the same nine adjustment cards for the two layouts
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/ui/sidebar/navigation.rs `show_adjustments` (:369-459)
- evidence:
  The vertical branch draws one card per section: `match app.develop_ui.adjustment_section { AdjustmentSection::Light => { changed |= Self::show_basic(ui, &mut app.develop.exposure, false); } AdjustmentSection::ToneCurve => { changed |= Self::show_tone_curve(ui, &mut app.develop.exposure, &mut app.develop_ui.tone_curve_tab, false); } ... }` (:369-444, nine arms, each card call ending in `false`).
  The `else` branch at :445-594 calls the same nine cards in order, each call ending in `true`: `changed |= Self::show_basic(ui, &mut app.develop.exposure, true);`, `Self::show_tone_curve(ui, &mut app.develop.exposure, &mut app.develop_ui.tone_curve_tab, true)`, `Self::show_color(ui, &mut app.develop.exposure, white_balance_raw.as_deref(), &mut app.develop_ui.white_balance_picker_active, true)`, the `show_detail` pair with `changed |= detail_changed; ai_denoise_request = request;`, the `show_presence` pair, `Self::show_hsl(...)`, `let optics = Self::show_optics(ui, app, true); lens_changed |= optics.lens; changed |= optics.exposure;` and `Self::show_global_effects(ui, app, frame, layout, added);`.
  The flag is the same argument everywhere: `timeout 90 grep -rn --include='*.rs' 'fn show_basic(\|fn show_detail(\|fn show_hsl(\|fn show_optics(\|fn show_presence(' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` shows each card's signature ends with `foldable: bool` (`ui/sidebar/develop.rs:264 pub(super) fn show_basic(ui: &mut Ui, exposure: &mut ExposureParams, foldable: bool) -> bool;`, `:27 ... fn show_optics(ui: &mut Ui, app: &mut CalibRawApp, foldable: bool) -> OpticsChanges`).
- why it is a problem: the card list is stated twice, so the two layouts are only kept in sync by hand - a new card added to one branch appears in the docked Develop panel or in the mobile section list but not both, and the same for a card that changes order; and the `foldable` flag is passed as a bare `false`/`true` at eighteen call sites, so flipping one silently makes a mobile card collapsible while the section list still draws it every frame.
- fix: one ordered list of the cards (label, section, draw closure) iterated with a single `foldable` derived from `layout`, keeping the vertical path's existing `adjustment_section` choice as the selector into that list, so both layouts walk the same order and pass the flag once.
- effort: L
### [UI-050] "Is the preview zoomed in?" is restated 17 times, in both directions, against a 1.0005 threshold
- category: unnecessary-code
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/preview/{detail.rs, rebuild.rs, state.rs, processing.rs, navigation.rs} and app/processing_export/export.rs:726
- evidence:
  `timeout 90 grep -rn --include='*.rs' 'DETAIL_ZOOM_START' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests\|/tests/'` returns 17 comparisons of the same predicate: `navigation.rs:13: let zoomed = self.preview.zoom > DETAIL_ZOOM_START;`, `state.rs:131` and `:139`, `rebuild.rs:29`, `:71`, `:91`, `processing.rs:5`, `:18`, `:269`, `export.rs:726`, `detail.rs:142`, `:178`, plus the inverted form `detail.rs:8: if self.preview.zoom <= DETAIL_ZOOM_START || self.ai_job_may_use_gpu() {`, `detail.rs:134: if self.preview.zoom <= DETAIL_ZOOM_START` and `retry_preview_detail_later` at `detail.rs:178: self.preview.motion_at = (self.preview.zoom > DETAIL_ZOOM_START).then(...)`.
  The constant itself is declared once but undocumented: `preview/mod.rs:283: pub(super) const DETAIL_ZOOM_START: f32 = 1.0005;` - the neighbouring `mod.rs:285 pub(in crate::app) fn zoom_detail_idle_delay() -> Duration { Duration::from_millis(if PlatformCaps::CURRENT.constrained_memory() { 220 } else { 140 }) }` shows the same file keeping tuned values inline too.
- why it is a problem: the same question is spelled as `zoom > X` at fourteen sites and as `zoom <= X` at three, and the constant is 1.0 plus a 0.0005 epsilon that is not stated anywhere, so a new site must decide both the comparison direction and whether it belongs on the epsilon side; a `>=` or a plain `1.0` written at one site makes the fit-to-screen view start requesting detail tiles every frame (the `<=` guards at detail.rs:8 and :134 are what keep the idle path off), and nothing in the code says why the extra 0.0005 is there.
- fix: one `Preview::zoomed_for_detail() -> bool` (or a `bool` kept in `PreviewState` alongside `touch_navigation_active`, which already caches such flags) used at all seventeen sites, and a comment on `DETAIL_ZOOM_START` stating that it is "fit-to-screen is exactly 1.0, and this is the first zoom step above it", with `220`/`140` ms named as the constrained/unconstrained idle delays.
- effort: M
### [UI-051] `PreviewDetail` is filled twice: 12 assignments next to a 13-field literal
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/processing_export/preview/detail.rs `install_prepared_preview_detail` - the reuse branch (:331-344) and the fresh-pipeline branch (:433-446)
- evidence:
  Reuse branch writes the fields one by one: `detail.uv_rect = visible;` `detail.texture_uv_rect = texture_uv_rect;` `detail.revision = revision;` `detail.raw = detail_raw;` `detail.source_origin = source_origin;` `detail.source_size = source_size;` `detail.processing_halo = processing_halo;` `detail.full_source_size = [full_raw.width, full_raw.height];` `detail.mask_source_region = mask_region;` `detail.mask_texture_extent = mask_extent;` `detail.virtual_origin = [virtual_origin_x, virtual_origin_y];` `detail.virtual_full_size = [virtual_full_width, virtual_full_height];`
  The other branch builds the same record as a literal: `self.preview.detail = Some(PreviewDetail { pipeline, uv_rect: visible, texture_uv_rect, revision, raw: detail_raw, processing_halo, source_origin, source_size, full_source_size: [full_raw.width, full_raw.height], mask_source_region: mask_region, mask_texture_extent: mask_extent, virtual_origin: [virtual_origin_x, virtual_origin_y], virtual_full_size: [virtual_full_width, virtual_full_height], });`
  The struct is declared at `crates/calibraw-ui/src/app/preview_state.rs:178 pub(crate) struct PreviewDetail {` and `timeout 90 grep -rn --include='*.rs' 'PreviewDetail {' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests\|/tests/'` shows this is the only construction site, so the two lists above are its entire population; `timeout 60 grep -niE 'PreviewDetail|install_prepared_preview_detail' finding.md docs/rework/AUDIT.md` finds nothing.
- why it is a problem: the fields that describe where the tile came from (origin, size, halo, mask region/extent, virtual size) are the bookkeeping that `preview_detail_is_current` and the staleness checks at `detail.rs:145-151` read back, and they are listed twice in two different shapes; a field added to `PreviewDetail` is set in one branch and left at its previous value in the other (the reuse branch mutates in place, so it silently keeps the old tile's `mask_texture_extent`), which makes the cached detail report geometry that does not match the pixels it holds.
- fix: one `PreviewDetail::retime_from(prepared, visible, ...)`-style updater (or destructure once and assign through a local `PreviewDetail` variable in both branches) so both paths set the same fields from the same values, with the fresh branch then `PreviewDetail { pipeline, ..from(prepared) }`.
- effort: M
### [UI-052] The "give up after N name variants" limit is a bare 10_000 at five search loops, in three forms
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/ui/library/folders/files.rs:68 (`copy_raw_bundle_to_folder`), :247 (RAW import copy), :351 (`unique_folder_destination`), crates/calibraw-ui/src/ui/library/merge.rs:199, crates/calibraw-core/src/pipeline/masks.rs:436 (`copied_name`)
- evidence:
  `timeout 90 grep -rn --include='*.rs' '10_000' crates/calibraw-ui/src crates/calibraw-core/src crates/calibraw-cli 2>/dev/null | grep -v 'tests.rs\|_tests\|/tests/'` returns five attempt-limit loops - `files.rs:68: for number in 0..=10_000usize {`, `files.rs:247: for number in 0..=10_000usize {`, `files.rs:351: for number in 0..=10_000usize {`, `merge.rs:199: for number in 0..10_000 {` and `masks.rs:436: for number in 1..=10_000usize {` - and the rest of the hits are unrelated numeric constants (`dcp_index.rs:5: const MAX_DCP_SCAN_FILES: usize = 10_000;` shows the same kind of scan limit *is* named in core).
  The loop bodies use different naming schemes (`numbered_file_name`'s `" ({number})"`, `merge.rs`'s `format!("-{suffix}-{number}.tif")`, `masks.rs`'s `format!("{base} Copy {number}")` and `files.rs:357: copy_name.push(format!(" copy {number}"))`), so only the bound is shared - and it is shared inconsistently: `merge.rs:199` is the only exclusive range, so it tries one variant fewer than the file-copy loops that share the same bound.
- why it is a problem: 10_000 is the point past which the app reports "Could not find an unused name" / falls back to a duplicate (`masks.rs:444: format!("{base} Copy")`), and nothing states that: a reader cannot tell whether a new loop's bound is this policy or a new one, and the off-by-one between inclusive and exclusive ranges is invisible at the call site, so a merge into a folder that already holds 9,999 `-suffix-<n>` variants silently picks a different fallback than a RAW import into the same folder.
- fix: one named limit per crate's file-naming module - `const MAX_NAME_VARIANTS: usize = 10_000;` (mirroring core's existing `MAX_DCP_SCAN_FILES`) - used by all five loops with the same inclusive range.
- effort: S
### [UI-053] The replay's "progress across N frames" ramp is written out five times
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/services/replay/mod.rs - `replay_stage_plan` render loop (:409, :428, :454, :495, :504)
- evidence:
  The same 4-line expression appears at five sites in the same function: `for frame in 0..SPLIT_SWEEP_FRAMES { let t = if SPLIT_SWEEP_FRAMES <= 1 { 1.0 } else { frame as f32 / (SPLIT_SWEEP_FRAMES - 1) as f32 };` at :409 (the reveal) and again at :428 (the return sweep), then `let t = if STAGE_TRANSITION_FRAMES <= 1 { 1.0 } else { frame as f32 / (STAGE_TRANSITION_FRAMES - 1) as f32 };` at :454, `if OUTRO_FADE_TO_BLACK_FRAMES <= 1 {` at :495 and `if OUTRO_BRAND_FADE_FRAMES <= 1 {` at :504. `timeout 90 grep -rn --include='*.rs' 'if [A-Z_]* <= 1 {' crates/calibraw-ui/src/services` matches only these five (frames.rs, plan.rs, android.rs, desktop.rs have none); finding.md and docs/rework/AUDIT.md name no replay ramp.
- why it is a problem: the `<= 1` guard is what keeps `(N - 1)` from underflowing when a hold or fade constant is retuned to 0 or 1 - and it is the only place that ties the frame constants (`SPLIT_SWEEP_FRAMES: u32 = 27`, `STAGE_TRANSITION_FRAMES: u32 = 24`, ...) to a position on the fade - so a fade constant lowered to 1 makes the five ramps disagree if any one copy is edited, and the `0..N` loop bound and the `N - 1` divisor are two numbers that must agree per site with nothing enforcing it.
- fix: one `fn frame_ramp(frame: u32, frames: u32) -> f32` (or a `ramp(frames) -> impl Iterator<Item = f32>` used as `for t in ramp(SPLIT_SWEEP_FRAMES)`) so the guard, the divisor and the loop bound come from one definition.
- effort: S
### [UI-054] "Parent folder, or the current one if the path has no parent" is spelled 12 times in calibraw-ui
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src - ui/library/export.rs:52, ui/sidebar/export.rs:351, app/lifecycle/mod.rs:76, app/ai/models.rs:409, app/lifecycle/profiles.rs:182, app/sidecar_persistence.rs:48, ui/library/actions.rs:801, ui/mod.rs:31, services/replay/mod.rs:273/:362/:536, export_naming.rs:140
- evidence:
  `timeout 90 grep -rn --include='*.rs' 'as_os_str().is_empty()' crates/calibraw-ui/src crates/calibraw-core/src | grep -v 'tests.rs\|_tests\|/tests/'` returns 12 ui hits (plus core/file_ops.rs:52 and one migrations hit). Two shapes of the same expression appear: `path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."))` (replay/mod.rs:271-275, :360-363, app/lifecycle/mod.rs:75-78) and the option-only form `destination.parent().filter(|path| !path.as_os_str().is_empty())` (replay/mod.rs:534-537, ui/library/export.rs:50-52, ui/sidebar/export.rs:349-352).
  core already owns this rule once, privately: `crates/calibraw-core/src/file_ops.rs:52: fn parent_directory(path: &Path) -> &Path { path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or_else(|| Path::new(".")) }`.
- why it is a problem: the empty-parent test is what stops a top-level path (e.g. `photo.CR`, whose `parent()` is the empty string) from being used as a directory - `sync_parent_directory` then fsyncs "." and the export dialog or the atomic write targets the process's working directory instead of the photo's folder - and the rule lives in twelve places, five of which choose the fallback (`"."`) and seven of which keep it an `Option`, so the two groups can disagree and a new caller must decide which group it is in.
- fix: one small helper per crate boundary - expose `parent_directory` from `calibraw_core::file_ops` (it is already the owner of the atomic-write rule that needs it) and add a calibraw-ui `parent_or_current(&Path) -> Path` used at all twelve sites, so the empty-parent and `.` decisions exist once per crate.
- effort: M
### [UI-055] `ExportFormat`'s settings names are spelled three times in a hand-written serde pair
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/performance_settings.rs `mod export_format_serde` (:97-127) and the field `:75 #[serde(with = "export_format_serde")] pub export_format: ExportFormat,`
- evidence:
  The same four wire names are written as three parallel tables: `serializer.serialize_str(match format { ExportFormat::Jpeg => "jpeg", ExportFormat::Png => "png", ExportFormat::Tiff => "tiff", ExportFormat::JpegXl => "jxl", })` (:105-110), `match String::deserialize(deserializer)?.as_str() { "jpeg" => Ok(ExportFormat::Jpeg), "png" => ..., "tiff" => ..., "jxl" => ...` (:116-121) and the error list `value => Err(serde::de::Error::unknown_variant(value, &["jpeg", "png", "tiff", "jxl"]))` (:124).
  `timeout 90 grep -rn --include='*.rs' 'jxl' crates | grep -v 'tests.rs\|_tests\|/tests/'` shows the other places that name the same enum keep their own spellings (`ui/sidebar/export.rs:71: (ExportFormat::JpegXl, "JXL")` for the UI label, the CLI's `--output must use the .png or .jxl extension` at `crates/calibraw-cli/src/bin/calibraw-develop-export.rs:424` and `matches_extension`), so no shared name table exists anywhere; `performance_settings.rs` is the only hand-written serde pair for this enum, and neither finding.md nor docs/rework/AUDIT.md mentions it.
- why it is a problem: the three lists are only in sync by hand, so adding a format to the enum means editing three entries in this module plus the CLI and UI spellings elsewhere; and because `load` treats any deserialization error as "start over with defaults" (`performance_settings.rs:305 Err(error) => { log::warn!(...); PerformanceSettings::default() }`), a format name that disagrees between the write and read halves silently discards the user's whole settings file, which is exactly what a half-updated pair produces.
- fix: give the enum one canonical short name in `calibraw-core` (`ExportFormat::short_name()` plus a lookup, or derive serde with stable names) and have `performance_settings.rs` use that; the `unknown_variant` list becomes the same table's keys, so the error text cannot lag the mapping.
- effort: M
### [UI-056] A settings file the app cannot read silently resets every preference
- category: rule-violation
- severity: high
- confidence: high
- where: crates/calibraw-ui/src/performance_settings.rs `load` (:252-286) and its only startup caller app/lifecycle/startup.rs:455
- evidence:
  `load` has no failure channel - `pub(crate) fn load(path: Option<&Path>) -> PerformanceSettings {` - and every problem path returns defaults: `Ok(_) => return PerformanceSettings::default(),` at :258 for a file that exists but is not a regular file or is larger than `MAX_SETTINGS_BYTES` (this branch logs nothing at all), `Err(error) if error.kind() == std::io::ErrorKind::NotFound => return PerformanceSettings::default()` (:259-261, correct for a first run), `Err(error) => { log::warn!("could not inspect performance settings {}: {error}", path.display()); return PerformanceSettings::default(); }` (:262-267) and `Err(error) => { log::warn!("could not load performance settings {}: {error}", path.display()); PerformanceSettings::default() }` (:276-281) for unreadable or malformed JSON.
  The startup caller takes the value as-is: `crates/calibraw-ui/src/app/lifecycle/startup.rs:455: let performance = crate::performance_settings::load(performance_settings_path.as_deref());` (the second caller, `ui/library/thumbnails/developed.rs:92`, likewise cannot report). `timeout 60 grep -rn --include='*.rs' 'performance_settings::load' crates/calibraw-ui/src | grep -v 'tests.rs\|_tests'` lists only these two.
  `timeout 60 grep -niE 'performance_settings|could not load performance' finding.md docs/rework/AUDIT.md` returns nothing about this path (the one finding.md hit is the unrelated `startup.rs:24 fn from_performance_settings` row).
- why it is a problem: a truncated, permission-denied or hand-edited settings file leaves the user with a fresh-install state - theme, export template, thumbnail workers, AI consent and library folder all reset - while the app looks healthy; the only clue is a `log::warn` line, which the app hides in normal runs, and the >64 KiB branch at :258 produces no output at all, so the most likely corruption (a partially written file after a crash, which `write_bytes_atomically` should prevent but does not if the rename fails across filesystems) is undiagnosable. AGENTS.md requires a typed error where callers distinguish failures and context at boundaries, and forbids a failure being visible only in the logs.
- fix: return a small result that the caller can act on - `Result<PerformanceSettings, String>` or a `LoadedSettings { settings, problem: Option<SettingsProblem> }` shape like the sidecar reporting AGENTS.md prescribes - so startup can `report_error` ("CalibRaw could not read your settings; the defaults are in use, and your saved preferences were left in <path>") instead of swallowing it, keeping the "no file yet" case as the non-error path it already separates at :259.
- effort: M
### [UI-057] Brush dab painting is written twice: `paint_subject_refinement` copies `paint_brush_dabs`
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/mask_tool.rs - `paint_subject_refinement` (:101-125, a method on CalibRawApp) and `paint_brush_dabs` (:222-256, a free function)
- evidence:
  The two bodies are the same loop with different caps: `if stroke.first && !stroke.samples.is_empty() && refinement.dabs.len() < MAX_REFINEMENT_DABS { refinement.stroke_starts.push(refinement.dabs.len()); }` (:105-107) against `if stroke.first && !stroke.samples.is_empty() && dabs.len() < MAX_STROKE_POINTS { stroke_starts.push(dabs.len()); }` (:239-241); and `for &center in &stroke.samples { if refinement.dabs.len() >= MAX_REFINEMENT_DABS { break; } refinement.dabs.push(BrushDab { center, opacity, size: stroke.dab_size, feather: refinement.feather, }); changed = true; }` (:109-120) against `for &center in &stroke.samples { if dabs.len() >= MAX_STROKE_POINTS { break; } dabs.push(BrushDab { center, opacity, size: stroke.dab_size, feather: *feather, }); changed = true; }` (:243-254). Only the cap constant, the feather source and the `last_brush_point`/`request_repaint` tail differ.
  `timeout 60 grep -rn --include='*.rs' 'BrushDab {' crates --include='*.rs' | grep -v 'tests.rs\|_tests\|/tests/'` hits exactly these two constructions (plus the struct definition in `crates/calibraw-core/src/pipeline/masks/raster.rs`), so there is no other caller to justify the pair; `timeout 60 grep -niE 'paint_brush_dabs|subject_refinement' finding.md docs/rework/AUDIT.md` is empty.
- why it is a problem: a brush dab has one shape (centre, opacity, size, feather) and one stroke-start rule (record the index where a new stroke begins, clamp to a cap), written twice, so subject/background refinement can drift from the brush in opacity handling (`refinement.flow` vs `*opacity_enabled`/`*brush_opacity`) or in the cap check, and the two paths already disagree on which feather applies, so a change to dab geometry is one edit in one path; and the copy is also the only place where the "new stroke starts" bookkeeping can be dropped.
- fix: one helper `append_stroke(dabs: &mut Vec<BrushDab>, starts: &mut Vec<usize>, cap: usize, feather: f32, opacity: f32, stroke: &BrushStrokeSamples) -> bool` used by both, keeping the differing cap, feather and opacity as arguments; the subject path keeps the `last_brush_point`/`note_subject_refinement_interaction`/`request_repaint` tail.
- effort: M
### [UI-058] Mask gesture geometry uses bare pixel/unit thresholds with no names or units
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-ui/src/app/mask_tool.rs `drag_radial` (:285, :287, :305, :308), `drag_linear` (:345), `drag_path` (:416) and crates/calibraw-ui/src/ui/preview/interaction.rs:149/:151
- evidence:
  `*radius = [rx.max(0.005), ry.max(0.005)]; *rotation = 0.0; *initialized = rx > 0.008 || ry > 0.008;` with `if rx < 0.01 && ry >= 0.01 { rx = ry * 0.66; } if ry < 0.01 && rx >= 0.01 { ry = rx * 0.66; }` above it (:275-287), and the same 0.005 clamp again in the resize arms `radius[0] = ((cos_r * dx + sin_r * dy).abs() / source_width.max(1) as f32).max(0.005);` / `radius[1] = ((-sin_r * dx + cos_r * dy).abs() / source_height.max(1) as f32).max(0.005);` (:305, :308).
  `*initialized = dx * dx + dy * dy > 0.000_025;` (:345) is the square of the same 0.005, and `if px * px + py * py >= 9.0 {` (:416, where `px = dx * source_width.max(1) as f32`) is 3 source pixels squared, stated nowhere.
  The minimum is re-implemented on the hit-test side: `ui/preview/interaction.rs:149: / (radius[0].abs().max(0.005) * source_width.max(1) as f32)` and `:151: / (radius[1].abs().max(0.005) * source_height.max(1) as f32)`. `timeout 90 grep -rn --include='*.rs' '0\.005' crates | grep -v 'tests.rs\|_tests\|/tests/'` shows these five mask sites, unrelated 0.005 constants elsewhere (`range_filters.rs:27 const GUIDED_EPSILON`, `basicadj.rs:79 const MIN_X_GAP`, `tone_curve_editor.rs:10 const MIN_POINT_X_GAP`) - which is the pattern the project already uses for named thresholds - and nothing about these in finding.md or docs/rework/AUDIT.md (`timeout 60 grep -niE 'mask_tool|0\.66|0\.000_025' finding.md docs/rework/AUDIT.md` is empty).
- why it is a problem: the same rule ("a radial mask is never smaller than 0.005 of the source, and is not initialized until it is bigger than 0.008") lives in five expressions across two files with no unit comment, so hit-testing, the create gesture and the resize gesture can each be retuned independently and the create/initialize pair (0.005 vs 0.008) already reads as an unexplained near-equal pair, where a mistyped value silently lets an invisible mask be created or lets a visible one be treated as uninitialized (and the 0.66 factor - a portrait/landscape aspect nudge - is guessable only from the test file).
- fix: `const MIN_MASK_RADIUS: f32 = 0.005;` (normalized fraction of the source edge, used by the create clamp, both resize clamps and `ui/preview/interaction.rs` hit-testing from one shared constant in the masks module), `const MIN_INITIALIZED_RADIUS: f32 = 0.008;`, `*initialized = dx * dx + dy * dy > MIN_MASK_LINEAR_LENGTH * MIN_MASK_LINEAR_LENGTH` and `const MIN_PATH_HANDLE_PX: f32 = 3.0; if px * px + py * py >= MIN_PATH_HANDLE_PX * MIN_PATH_HANDLE_PX`, with the 0.01/0.66 pair named as the aspect nudge for a near-zero drag axis.
- effort: M
### [UI-059] The library folder sidebar re-implements the Develop sidebar's maximum-width formula
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/app/eframe_impl.rs:385-387 and crates/calibraw-ui/src/ui/layout.rs:59-63
- evidence:
  `layout.rs:59 pub(crate) fn develop_sidebar_max_width(viewport: Vec2) -> f32 { (viewport.x * 0.48).clamp(ScreenLayout::MIN_HORIZONTAL_SIDEBAR_WIDTH, ScreenLayout::MAX_HORIZONTAL_SIDEBAR_WIDTH) }`, used at `eframe_impl.rs:328` and `top_bar.rs:99`.
  The library sidebar writes the same formula inline instead: `let panel_max = (viewport_size.x * 0.48).max(aligned_width).clamp(ScreenLayout::MIN_HORIZONTAL_SIDEBAR_WIDTH, ScreenLayout::MAX_HORIZONTAL_SIDEBAR_WIDTH);` (:385-387, with `const LIBRARY_MIN_SIDEBAR_WIDTH: f32 = 300.0;` above it at :381). `timeout 60 grep -rn 'viewport_size.x \* 0\.\|viewport.x \* 0\.' crates/calibraw-ui/src --include='*.rs' | grep -v 'tests.rs\|_tests'` lists only these two sites.
- why it is a problem: the same 48 % of viewport width and the same MIN/MAX clamp are maintained in two places for two sidebars that are designed to line up with each other (the code aligns the library panel's left edge to the toolbar separator via `TopBar::library_sidebar_default_width`), so retuning one leaves the other at the old fraction and the panels' maximums stop matching at runtime.
- fix: name the rule once - keep `sidebar_max_width(viewport)` in `ui/layout.rs` (dropping the `develop_` prefix, which already leaks: the function is the general horizontal-sidebar limit) and have the library branch call it, keeping its own `.max(aligned_width)` and `LIBRARY_MIN_SIDEBAR_WIDTH` as the library-specific part.
- effort: S
### [UI-060] The four Android system-bar safe areas are one block copied per screen edge
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ui/src/app/eframe_impl.rs `handle_android_platform_events` (:149-171)
- evidence:
  Four near-identical blocks follow `let [left, top, right, bottom] = self.platform.system_bar_insets_points(ui.ctx().pixels_per_point());` (:147): `if top > 0.0 { egui::Panel::top("android_status_bar_safe_area").resizable(false).exact_size(top).show(ui, |_| {}); }` (:149-153), then the same shape for `egui::Panel::bottom("android_navigation_bar_safe_area")` with `bottom` (:155-159), `egui::Panel::left("android_left_system_safe_area")` with `left` (:161-165) and `egui::Panel::right("android_right_system_safe_area")` with `right` (:167-171). `timeout 60 grep -rn 'safe_area' crates/calibraw-ui/src --include='*.rs' | grep -v 'tests.rs\|_tests'` returns only these four ids.
- why it is a problem: the rule ("reserve the system-bar inset as an empty, non-interactive panel when the inset is positive") is written four times, and the panel id strings (`android_status_bar_safe_area`, `android_navigation_bar_safe_area`, `android_left_system_safe_area`, `android_right_system_safe_area`) already break one naming convention (two name the bar, two name the side), so a change to how insets are reserved - e.g. adding the `interactable` handling egui needs for a tap-blocking strip - is four edits, one of which can be missed.
- fix: one local helper `fn reserve_safe_area(ui: &mut egui::Ui, side: egui::Side, inset: f32)` that maps the side to `egui::Panel::left/right/top/bottom` and to one id derived from the side, called with the four `[left, top, right, bottom]` values; the positive-inset test moves inside the helper.
- effort: S
NOTE: calibraw-ui has reached its 60-finding cap (UI-001…UI-060). Further calibraw-ui candidates found after this point are listed by file:line in the closing "Systemic patterns"/"Not checked" sections as still-open, not as new entries.
### [CORE-012] The TIFF sniffing pass throws away every parse error, so a broken file is reported as "not a RAW photo"
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/tiff_loader.rs `is_raster_tiff` (:119-129), reached from crates/calibraw-core/src/pipeline/raw_loader.rs:595
- evidence:
  `pub(super) fn is_raster_tiff(path: &Path) -> Result<bool) { if is_tiff_path(path) { return Ok(inspect_tiff_container(path)? == TiffContainerKind::Raster); } if path.extension().is_some() { return Ok(false); } // Anything that does not read as a raster TIFF stays with the sensor decoders.` then `Ok(!has_camera_raw_marks(path).unwrap_or(true) && matches!(inspect_tiff_container(path), Ok(TiffContainerKind::Raster)))` (:127-128). The caller is the format-sniffing chain: `crates/calibraw-core/src/pipeline/raw_loader.rs:595: Ok(super::tiff_loader::is_raster_tiff(path)?.then_some(Self::Tiff))`.
  The named-extension branch propagates (`inspect_tiff_container(path)?`), the extensionless branch - the one Android uses, per the comment "Android opens documents as `/proc/self/fd/<n>`, which loses the name, so an extensionless path is judged by its content" - does not: `unwrap_or(true)` turns a failed `has_camera_raw_marks` into "has camera raw marks", and `matches!(inspect_tiff_container(path), Ok(...))` turns a failed container inspection into "not raster", so both failures take the `Ok(false)` path.
- why it is a problem: a truncated or unreadable Android document - a common case, since the picker hands over a descriptor path that can be closed or short - is then routed to the sensor loaders and reported as "no camera RAW loader could open this file" (or silently dropped from the choice), which tells the user the wrong thing and hides the actual `open TIFF … / read TIFF header …` context that `open_tiff` already builds; the same file with a `.tif` extension gets the real message, so behaviour differs by name only. AGENTS.md requires a typed error where callers distinguish failures and context at boundaries.
- fix: let the sniffing branch propagate: evaluate `has_camera_raw_marks(path)` and `inspect_tiff_container(path)` with `match`/`?` so an `Err` returns from `is_raster_tiff` (the signature already returns `Result<bool>`), keeping the "not a TIFF container" answer only for the `Ok` cases; then the comment's rule reads as "no camera-RAW marks and it inspects as a raster container" without collapsing errors into `false`.
- effort: S
### [CORE-013] Brush-dab validation is pasted twice: subject refinement and brush mask components
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/sidecar/validation.rs `validate_edit_state` - subject refinement at :120-145, brush geometry at :193-218
- evidence:
  The stroke-boundary walk and per-dab range checks are the same code twice, only the label differs: `let mut previous_start = None; for &start in &refinement.stroke_starts { if start >= refinement.dabs.len() || previous_start.is_some_and(|previous| start <= previous) { return invalid("subject refinement contains invalid stroke boundaries"); } previous_start = Some(start); }` (:121-128) and `let mut previous_start = None; for &start in stroke_starts { if start >= dabs.len() || previous_start.is_some_and(|previous| start <= previous) { return invalid("brush mask contains invalid stroke boundaries"); } previous_start = Some(start); }` (:194-201); the five range lines are identical except the label - `bounded("subject refinement dab x", dab.center[0], -16.0, 16.0)?; ... bounded("subject refinement dab feather", dab.feather, 0.0, 1.0)?;` (:141-145) against `bounded("brush dab x", dab.center[0], -16.0, 16.0)?; ... bounded("brush dab feather", dab.feather, 0.0, 1.0)?;` (:214-218), with the same `finite(..., &[dab.center[0], dab.center[1], dab.opacity, dab.size, dab.feather])` above each. `timeout 60 grep -n 'subject refinement dab\|brush dab\|previous_start' crates/calibraw-core/src/sidecar/validation.rs` returns only these two groups.
  The caps differ but are the same named constant: `if refinement.dabs.len() > MAX_BRUSH_DABS` (:117) and `if dabs.len() > MAX_BRUSH_DABS` (:206).
- why it is a problem: a dab has one contract (centre within -16..16, opacity -1..1, size 0..16, feather 0..1, stroke starts strictly increasing and inside the dab list) and it is written twice, so tightening the size limit or the stroke-boundary rule for one path leaves the other accepting the old values - which for a sidecar means the file loads and later raster work reads an out-of-range feather or a start index past the array; the two copies already disagree on one point, since the subject path's cap line sits outside the loop structure of the brush path.
- fix: one helper `fn validate_dabs(label: &str, dabs: &[BrushDab], starts: &[usize]) -> Result<(), SidecarError>` holding the stroke-boundary walk and the five range checks (the message becomes `"{label} contains invalid stroke boundaries"`), called from both sites; the ranges become named constants next to `finite`/`bounded` (:659/:667) so the same dab contract is stated once.
- effort: M
### [CORE-014] `validate_exposure` and `validate_local_adjustments` restate the same tail of adjustment validation
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/sidecar/validation.rs `validate_exposure` (:508-...: tail) and `validate_local_adjustments` (:580-616)
- evidence:
  Both end with the same four calls, differing only in the label prefix: `finite("global HSL hue", &exposure.hsl_hue)?; finite("global HSL saturation", &exposure.hsl_luminance... ` — concretely `finite("global HSL hue", &exposure.hsl_hue)?;` / `finite("global HSL saturation", &exposure.hsl_saturation)?;` / `finite("global HSL luminance", &exposure.hsl_luminance)?;` / `validate_point_colors(&exposure.point_colors)?;` / `validate_curves(&[&exposure.tone_curve, &exposure.tone_curve_red, &exposure.tone_curve_green, &exposure.tone_curve_blue], "global tone curve")?;` / `validate_grading(&exposure.color_grading, "global color grading")` against the same six lines with `adjustments.*` and `"local ..."` labels at :595-615. `timeout 60 grep -n 'validate_point_colors\|validate_curves\|validate_grading' crates/calibraw-core/src/sidecar/validation.rs` shows these as the only two callers.
- why it is a problem: the shared adjustment contract - HSL triplets finite, point colours validated, the four tone curves validated, colour grading validated - is written per struct, so a new adjustment-bearing struct (or a new shared field such as a curve set) must have the tail copied, and a copy that forgets `validate_grading` or `validate_curves` accepts a sidecar whose grading curves were never range-checked, which then reaches the shader stage unvalidated.
- fix: give the shared part one owner: a `ValidatableAdjustments` trait/method `validate_adjustment_tail(label: &str) -> Result<(), SidecarError>` implemented on `ExposureParams` and `LocalAdjustments` (the curve quadruple, HSL triplets, point colours and grading are fields of both), called once from each site with its label; the per-struct finite field lists stay separate because their scalar fields differ.
- effort: M
### [CORE-015] The mask geometry validation arm ends in `_ => {}`
- category: rule-violation
- severity: low
- confidence: high
- where: crates/calibraw-core/src/sidecar/validation.rs:399, inside `validate_edit_state`'s `match &component.geometry` (:184-399)
- evidence:
  The match arms are `MaskGeometry::Brush { .. }`, `Radial`, `Linear`, `Path`, `Ai`, `Object`, `LuminanceRange`, `ColorRange`, `DepthRange` and then `_ => {}` (:399). `crates/calibraw-core/src/pipeline/masks/mask_geometry.rs:6 pub enum MaskGeometry {` declares eleven variants - `Fullscreen, Brush, Radial, Linear, Path, Ai, Object, LuminanceRange, ColorRange, DepthRange, Placeholder` - so the wildcard silently covers `Fullscreen` and `Placeholder` (the latter is accepted for `DepthRange` by `geometry_matches_kind` at :415: `(MaskKind::DepthRange, MaskGeometry::Placeholder)`).
- why it is a problem: the match is presented as "validate every geometry", but the catch-all means a geometry variant added to `MaskGeometry` - the enum already grows with new mask types, and `Placeholder` shows the shape exists for not-yet-implemented masks - compiles while its numeric fields stay unvalidated, so a hand-edited sidecar can carry out-of-range values that reach the shader stage; today the two variants it hides genuinely have no fields, which is exactly the fact the arm should state instead of asserting through a wildcard.
- fix: replace `_ => {}` with `MaskGeometry::Fullscreen, MaskGeometry::Placeholder => {}` and a comment saying those variants carry no fields to range-check, so the compiler reports a new geometry as unhandled instead of accepting it unvalidated.
- effort: S
### [CORE-016] The mask distance transform is two hand-mirrored passes
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/remove.rs `RemoveMask::max_half_width` (:368-414), passes at :388-396 and :398-407
- evidence:
  `let neighbors = [(-1isize, 0isize, ORTHOGONAL), (0, -1, ORTHOGONAL), (-1, -1, DIAGONAL), (1, -1, DIAGONAL)];` (:382-386) feeds two loops that differ only in the sign of the neighbour offset and in tracking `maximum`: `for y in 1..height - 1 { for x in 1..width - 1 { let index = y * width + x; for (dx, dy, step) in neighbors { let neighbor = (index as isize + dy * width as isize + dx) as usize; distance[index] = distance[index].min(distance[neighbor] + step); } } }` (:388-396) and `for y in (1..height - 1).rev() { for x in (1..width - 1).rev() { let index = y * width + x; for (dx, dy, step) in neighbors { let neighbor = (index as isize - dy * width as isize - dx) as usize; distance[index] = distance[index].min(distance[neighbor] + step); } maximum = maximum.max(distance[index]); } }` (:398-407). The function's constants are `const ORTHOGONAL: u32 = 3; const DIAGONAL: u32 = 4;` (:369-370) and the result is `maximum as f32 / ORTHOGONAL as f32` (:413), with no statement of the "distance is kept in thirds of a pixel" contract. `timeout 60 grep -rn 'max_half_width' crates --include='*.rs' | grep -v 'tests.rs\|_tests\|/tests/'` finds only this definition and its call sites; finding.md and docs/rework/AUDIT.md contain nothing about it.
- why it is a problem: the forward and reverse sweeps of a two-pass distance transform are one rule written twice, and the only difference between them is a hand-negated offset expression (`+ dy * width + dx` versus `- dy * width - dx`) buried inside the loop; flipping one sign by hand is how a stroke's reported half thickness stops matching the mask, and the duplicated `min` line means a change to the metric (a different diagonal weight, a 4-connected variant) is two coordinated edits; the 3/4 weights and the `/ ORTHOGONAL` divisor are also stated only as numbers, so the "distance in third-pixels" unit is invisible where the value is finally converted back to pixels.
- fix: one local pass helper `fn relax(path: &[usize], neighbors: ..., distance: &[u32]) -> void` (or a closure taking the visit order and a sign) called twice with the forward and reversed orders, with `const PIXEL_UNIT = 3` / `DIAGONAL_STEP = 4` documented as "octile distance scaled by 3, √2 ≈ 4/3" and the final conversion line referencing the same names.
- effort: M
### [CORE-017] Case-insensitive group equality is expressed by searching a one-element slice
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/presets.rs `rename_group` (:656 and :660) and `delete_group` (:689 and :692), against `find_group` (:561-568)
- evidence:
  `if find_group(std::slice::from_ref(&old), stored.preset.group()).is_some() {` (:656), `if find_group(std::slice::from_ref(&group), stored.preset.group()).is_none() { continue; }` (:690) and `let moves_to_itself = find_group(std::slice::from_ref(&group), DEFAULT_PRESET_GROUP).is_some();` (:692). The helper being called is `pub fn find_group<'g>(groups: &'g [String], name: &str) -> Option<&'g str) { let name = name.trim().to_lowercase(); groups.iter().find(|group| group.to_lowercase() == name).map(String::as_str) }` (:561), i.e. building a temporary single-element slice just to ask "do these two names mean the same group".
  `timeout 60 grep -rn 'from_ref' crates/calibraw-core/src/presets.rs` returns only these three sites (all of the workspace's `std::slice::from_ref` uses: `timeout 90 grep -rn --include='*.rs' 'slice::from_ref' crates xtask | grep -v 'tests.rs\|_tests\|/tests/'` shows them plus unrelated ones).
- why it is a problem: the equality rule for group names ("trim, lowercase, compare") now lives inside a list-searching function, so the intent at the call site reads as "search the folder's groups" when it is "compare two names" - and because `find_group` returns the *stored* spelling, a reader must work out that discarding that value is harmless here; the three copies also differ (two test membership of the loop's group, one compares against `DEFAULT_PRESET_GROUP`), so the rule can drift between the comparison used when moving presets and the one used when rewriting the group list at :696-700 (`filter(|existing| *existing != group)`), which already compares exactly rather than case-insensitively - an inconsistency that leaves a group whose spelling differs only by case in the list after its presets were moved.
- fix: extract the rule as `pub(super) fn same_group_name(left: &str, right: &str) -> bool { left.trim().to_lowercase() == right.trim().to_lowercase() }`, use it inside `find_group` and at the three sites, and make the `filter`/`map` group-list rewrites (:669-672 and :696-700) use it too so all four places compare names identically.
- effort: S
### [CORE-018] Group create/rename/delete reload every preset and throw away the files that failed
- category: unnecessary-code
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/presets.rs `create_group` (:615-624), `rename_group` (:631-668) and `delete_group` (:673-707), all calling `load_preset_folder` (:546-573); `read_group_list` (:577-592) already exists
- evidence:
  Each mutating group operation starts with `let contents = load_preset_folder(folder)?;` (:617, :634, :676). `load_preset_folder` opens the directory, then for each `*.calibraw-preset` file calls `read_preset_file` -> `Preset::decode`, which JSON-decodes a full `EditState`, runs `validate_edit_state` and rebuilds through `Preset::new` (:344-362 `Self::new(...)` path), and ends with `contents.presets.sort_by_cached_key(...)` plus `merge_groups(...)` (:566-570). `create_group` uses only `contents.groups` (:618-623); `rename_group` and `delete_group` use `contents.presets[].path/group` (:647-655, :681-691) but never the decoded settings.
  The same call collects `pub failures: Vec<String>` - `Err(error) => contents.failures.push(format!("{}: {error}", path.file_name().unwrap_or_default().to_string_lossy()))` (:556-560) - and none of the three operations reads `contents.failures`; `timeout 60 grep -rn 'contents.failures\|\.failures' crates/calibraw-ui/src crates/calibraw-core/src --include='*.rs' | grep -v 'tests.rs\|_tests\|/tests/'` shows it being surfaced only from the browse/list path, not from these three.
- why it is a problem: renaming or deleting a group re-decodes every preset in the folder (up to `MAX_PRESET_FILES` = 4096) even though the operation needs a name list and file paths, which makes a group edit cost a full preset load - the same work the UI does when listing presets - on every click; and a preset file that failed to decode is dropped from `contents.presets` silently, so `rename_group` leaves that file's group field pointing at the deleted group and `delete_group` leaves its preset (and the file) untouched while reporting success, which is exactly the half-renamed state the user then has to discover.
- fix: split the read: `read_preset_folder_contents` already separates groups from presets, so let `create_group` use `read_group_list` (:577) alone, and give `rename_group`/`delete_group` a lighter `StoredPreset` scan (path + group + name is all they use) that returns the failures, which the three operations must propagate (`Err` or a returned message) instead of dropping, so a preset that cannot be read blocks the rename rather than being skipped.
- effort: M
### [CORE-019] `normalize_group` copies `validate_group_name`
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/presets.rs `normalize_group` (:380-387) and `validate_group_name` (:390-397)
- evidence:
  `fn normalize_group(group: &str) -> Result<String, PresetError> { let group = group.trim(); if group.is_empty() { return Ok(DEFAULT_PRESET_GROUP.to_owned()); } validate_label(group, "group name")?; Ok(group.to_owned()) }` and `pub fn validate_group_name(name: &str) -> Result<String, PresetError> { let name = name.trim(); if name.is_empty() { return invalid("enter a group name"); } validate_label(name, "group name")?; Ok(name.to_owned()) }` - identical except the empty-group answer. `timeout 60 grep -rn --include='*.rs' 'validate_group_name\|normalize_group' crates | grep -v 'tests.rs\|_tests\|/tests/'` shows `validate_group_name` called from `read_group_list`, `create_group` and `rename_group`, and `normalize_group` from `Preset::new` and `Preset::is_named`, so both are live and neither is a wrapper for the other.
- why it is a problem: the group-name rule (trim, reject control characters, cap at `MAX_PRESET_NAME_CHARS`, keep the trimmed spelling) is stated twice, so a change to how groups are accepted - e.g. allowing spaces where they are currently stripped only at the edges, or the limit changing - has to be made in both, and the pair already answers "empty" differently on purpose (default group versus an error) which is the one fact the second copy should be reading, not restating.
- fix: make `normalize_group` delegate: after the `is_empty` default branch, `validate_group_name(group)`, so `validate_label`, the trim and the `to_owned` live once.
- effort: S
### [CORE-020] `save_jpeg` re-validates the thumbnail with its own copy of the cache limits
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/thumbnail_cache.rs `save_jpeg` (:303-322) against `accepted_thumbnail_layout` (:176-207)
- evidence:
  `save_jpeg` opens with `if thumbnail.width == 0 || thumbnail.height == 0 || thumbnail.width > MAX_CACHED_THUMBNAIL_EDGE || thumbnail.height > MAX_CACHED_THUMBNAIL_EDGE { return Err("thumbnail dimensions are outside the cache safety limit".to_owned()); }` (:304-310) and then computes its own `let expected = usize::try_from(thumbnail.width).ok().and_then(|width| usize::try_from(thumbnail.height).ok().and_then(|height| width.checked_mul(height))).and_then(|pixels| pixels.checked_mul(4)).ok_or_else(|| "thumbnail byte count overflow".to_owned())?;` (:311-319).
  `accepted_thumbnail_layout(width, height) -> Result<(u64, u64, u64), String>` already performs exactly these checks - `if width == 0 || height == 0`, `if width > MAX_CACHED_THUMBNAIL_EDGE || height > MAX_CACHED_THUMBNAIL_EDGE`, the `checked_mul` pixel count against `MAX_CACHED_THUMBNAIL_PIXELS`, and `decoded_bytes` against `MAX_CACHED_THUMBNAIL_DECODE_BYTES` (:176-207) - and is already used on the read side twice: `if accepted_thumbnail_layout(source_width, source_height).is_err() {` (:247) and `let (_, _, decoded_bytes) = match accepted_thumbnail_layout(width, height) {` (:277).
- why it is a problem: the write path accepts a thumbnail the read path would reject - `save_jpeg` never compares the pixel count with `MAX_CACHED_THUMBNAIL_PIXELS` or the decoded bytes with `MAX_CACHED_THUMBNAIL_DECODE_BYTES`, so writing a cache entry near the limits succeeds and `load_jpeg` then reports it invalid and deletes it, which turns a large cached thumbnail into a permanent regenerate/write/delete cycle; and the two error strings ("thumbnail dimensions are outside the cache safety limit", "thumbnail byte count overflow") carry none of the detail the shared validator already builds (`thumbnail {width}x{height} contains {pixels} pixels, above the ... limit`), so a user-facing message from `save_jpeg` names neither the size nor the limit.
- fix: call the shared validator once in `save_jpeg`: `let (_, _, expected) = accepted_thumbnail_layout(thumbnail.width, thumbnail.height)?;` then keep only `if thumbnail.rgba.len() != expected { return Err("thumbnail has an invalid RGBA byte count".to_owned()); }`, deleting the duplicated dimension and overflow block so the write path and read path state one limit set with one message.
- effort: M
### [CORE-021] FNV-1a is re-implemented three times with magic constants
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/thumbnail_cache.rs `fnv1a64` (:341-349) and the `#[cfg(windows)] desktop_path_fingerprint` (:532-542); crates/calibraw-core/src/diagnostics.rs `fnv1a_step` + `sampled_fingerprint` (:159-176)
- evidence:
  `pub fn fnv1a64(bytes: &[u8]) -> u64 { let mut hash = 0xcbf29ce484222325u64; for byte in bytes { hash ^= u64::from(*byte); hash = hash.wrapping_mul(0x100000001b3); } hash }` (:341-349). In the same file, the Windows branch re-states the same loop: `let mut hash = 0xcbf29ce484222325u64; for word in path.as_os_str().encode_wide() { for byte in word.to_le_bytes() { hash ^= u64::from(byte); hash = hash.wrapping_mul(0x100000001b3); } } hash` (:532-542). diagnostics.rs keeps a third copy: `fn fnv1a_step(hash: u64, byte: u8) -> u64 { (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3) }` (:159-161) fed by `let mut hash = 0xcbf29ce484222325u64;` (:170). `timeout 60 grep -rn --include='*.rs' '0xcbf29ce484222325\|0x100000001b3' crates xtask | grep -v 'tests.rs\|_tests\|/tests/'` returns exactly those three sites plus crates/calibraw-core/examples/raw_open_bench.rs:46-47, so there is no shared definition anywhere in the workspace; finding.md and docs/rework/AUDIT.md contain no FNV entry.
- why it is a problem: the hash is the identity of a cache entry - `desktop_cache_path_for_raw` (:519-523) turns it into a filename, so the three copies must agree bit for bit or one platform's cache files stop being found by the others (and `load_desktop_raw_thumbnail`'s fingerprint file inherits the same coupling); as written they agree only by inspection, the two bare constants carry no name or comment saying this is FNV-1a and which variant, and a reader comparing the Windows branch with the unix one (`desktop_path_fingerprint` (:526-530) hashes `as_os_str().as_bytes()` while the Windows branch hashes UTF-16 code units) has to re-derive that the two encodings are deliberate rather than one of them being a re-implementation slip.
- fix: keep one definition in a leaf module (thumbnail_cache's `fnv1a64`, made to accept a byte iterator) and name the constants once with a comment stating "FNV-1a 64-bit, offset basis 14695981039346656037, prime 1099511627775"; the Windows branch then calls it over `path.as_os_str().encode_wide().flat_map(|word| word.to_le_bytes())`, which produces the identical byte stream, and diagnostics.rs calls the same shared step function.
- effort: M
### [CORE-022] The DCP index cache is checked twice by copy-paste
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/dcp_index.rs `indexed_dcp_profiles` (:63-69 and :75-81)
- evidence:
  `if let Ok(cache) = dcp_profile_index_cache().lock() { if let Some(index) = cache.get(&cache_key) { if index.root_modified == root_modified { return Ok(Arc::clone(&index.profiles)); } } }` appears twice in a row, at :63-69 and again at :75-81 after `let _scan_guard = DCP_PROFILE_SCAN_LOCK.get_or_init(|| Mutex::new(())).lock().ok();` (:71-74) - nine lines, character-for-character identical, and the file's only two cache reads: `timeout 60 grep -n 'cache.get' crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/dcp_index.rs` returns :64 and :76 only (the write is `cache.insert(cache_key, CachedDcpIndex {...})` at :158-165).
- why it is a problem: the copy is the point of the second check (re-test after paying for the scan-lock wait so a folder already indexed by another thread is not re-scanned), but nothing says so - the reader sees two identical `if let Ok` chains nine lines apart and has to work out which is the cheap pre-check and which is the post-lock re-check, and the pair defines the cache's staleness rule (`index.root_modified == root_modified`, with `root_modified` itself `metadata.modified().ok()`, i.e. `None == None` compares equal) in two places, so a change to that rule (adding file count or size, which a same-second edit of the folder would otherwise miss) must be made in both, and if the second copy is dropped the folder is rescanned on every open.
- fix: extract `fn cached_profiles(folder_key: PathBuf, root_modified: Option<SystemTime>) -> Option<Arc<Vec<IndexedDcpProfile>>>` used by both call sites, and comment the second call as the post-scan-lock re-check.
- effort: S
### [CORE-023] DCP index locks answer "poisoned" by skipping the operation
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/dcp_index.rs `invalidate_dcp_profile_index` (:33-39), `indexed_dcp_profiles` (:63, :75, :158) and `prewarm_dcp_profile_index` (:46)
- evidence:
  Every lock site in this file treats a poisoned mutex as "do nothing": `if let Ok(mut cache) = cache.lock() { cache.clear(); }` (:36-38), `if let Ok(cache) = dcp_profile_index_cache().lock() {` (:63, :75), `if let Ok(mut cache) = dcp_profile_index_cache().lock() { cache.insert(...) }` (:158-165) and `let _scan_guard = DCP_PROFILE_SCAN_LOCK.get_or_init(|| Mutex::new(())).lock().ok();` (:71-74). `timeout 120 grep -rn --include='*.rs' 'PoisonError' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns 37 sites that resolve a poisoned lock instead - e.g. crates/calibraw-core/src/thumbnail_cache.rs:45-47, :57-59 and :95-97 use `.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` - and `timeout 120 grep -rn --include='*.rs' 'lock().ok()' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns this file plus crates/calibraw-ffi/src/android.rs:149 and :222, so this file is the only core cache that abandons the operation on poison.
- why it is a problem: `invalidate_dcp_profile_index` is how a changed DCP folder or a settings change drops the stale index, and when the mutex is poisoned that call does nothing while still returning void, so the app keeps serving profiles from a folder the user just changed (matching `cache_key` and the unchanged `root_modified` would not save it: the folder path is the key and its mtime may be the same second); symmetrically, skipping `cache.insert` makes every later open rescan up to `MAX_DCP_SCAN_FILES` = 10_000 entries silently, and skipping the scan lock lets two threads scan that same folder at once, which is the only reason the scan lock exists - the degradation is invisible in all three cases because the failures are folded into `if let Ok` rather than reported, against the file's own handling of IO failures, which logs each one.
- fix: follow the workspace's existing convention: `let Ok(mut cache) = ...` becomes `cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` at :36, :63, :75 and :158, and the scan guard binds the `Lock` directly (`let scan_guard = DCP_PROFILE_SCAN_LOCK.get_or_init(...).lock().unwrap_or_else(std::sync::PoisonError::into_inner);` named without the leading `_` so the RAII hold is visible), so a poisoned mutex keeps the cache correct instead of silently disabling it.
- effort: S
### [CORE-024] The DCP profile match ranking is a ladder of unnamed weights
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/dcp_index.rs `dcp_match_score` (:271-330)
- evidence:
  The whole ranking is bare integers: `let mut score = if !declared.is_empty() { if declared == model_key || declared == combined_key { 1000 } else if model_key.len() >= 4 && declared.contains(model_key) { 900 } else { return 0; } } else if filename == model_key || filename == combined_key { 700 } else if model_key.len() >= 4 && filename.contains(model_key) { 600 } else if model_key.len() >= 4 && path_key.contains(model_key) { 550 } else { return 0; };` (:285-302), then the bonuses `score += 25;` (:311), `score += 20;` (:318), `score += 15;` (:320), `score -= 30;` (:327), each keyed on vendor substrings (`profile_name.contains("camerastandard") || filename.contains("camerast") ... || profile_name.ends_with("camerabw") || filename.ends_with("camerabw")` :313-326). No constant, comment or table in the file states what the weights mean or that their ordering is what picks the profile: `timeout 60 grep -in 'dcp' finding.md docs/rework/AUDIT.md` shows finding.md's only DCP entries are `color_profile/dcp.rs:503,561` (unreachable!) in the unwrap table at :826 and docs/rework/AUDIT.md:46's saved-document row, so nothing here is already reported.
- why it is a problem: the numbers are the user-visible decision - `find_matching_dcp_profiles` sorts by this score (:196-214 `matches.sort_by(...right.score.cmp(&left.score)...)`) and the UI lists the result as the available camera profiles, so a profile that should win by a hair silently loses with no way to tell why; and the weights are entangled in one long expression, where `1000/900` (declared model matches) versus `700/600/550` (filename/path matches) versus the ±25/±20/±15/±30 adjustments is only reconstructible by arithmetic: `score` starts at 550 and the adjustments can total +60, i.e. an "unknown" vendor-tagged profile matched only by path can outscore a filename match only if the reader happens to add the ranges up, and `model_key.len() >= 4` is repeated at :291, :298 and :300 so a change to that minimum-name length is three coordinated edits - exactly the kind of numerical contract AGENTS.md asks to document.
- fix: name the weights as documented constants in one place (`PROFILE_MODEL_DECLARED_EXACT = 1000`, `PROFILE_MODEL_DECLARED_CONTAINS = 900`, `..._FILENAME_EXACT = 700`, `..._FILENAME_CONTAINS = 600`, `..._PATH_ONLY = 550`, `MAKE_MATCH_BONUS = 25`, `CAMERA_STANDARD_BONUS = 20`, `..._ABBREVIATED = 15`, `MONOCHROME_PENALTY = -30`, `MIN_MODEL_KEY_LENGTH = 4`) with a comment stating that a declared-model match must outrank any filename match and that the bonuses cannot cross the 100-point gap between tiers (they currently cannot: 25+20+15 = 60), and collect the abbreviation strings once (`"camerastandard"`, `"camerast"`, `"camerabw"`, `"camerast"`... as a documented list of Adobe's DCP name spellings) instead of inlining them in two conditions.
- effort: M
### [CORE-025] The DCP reader re-implements the TIFF scalar reader
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/color_profile/dcp.rs `enum Endian` + readers (:307-335), field-size match (:572-579) against crates/calibraw-core/src/pipeline/tiff_loader.rs `enum ByteOrder` + readers (:34-58), `field_width` (:298-305)
- evidence:
  dcp.rs: `enum Endian { Little, Big }` (:307-310) with `fn u16(self, bytes: [u8; 2]) -> u16 { match self { Self::Little => u16::from_le_bytes(bytes), Self::Big => u16::from_be_bytes(bytes) } }` (:313-317), the same shape for `u32` (:319-323), `i32` (:325-329) and `u64` (:331-335), and the header sniff `b"II" => Endian::Little, b"MM" => Endian::Big,` (:368-369). tiff_loader.rs has `enum ByteOrder { Little, Big }` (:34-37) with `fn u16` (:40-44), `fn u32` (:47-51) and `fn u64` (:54-58) - character-identical bodies - and the same sniff `b"II" => ByteOrder::Little, b"MM" => ByteOrder::Big,` (:79-80). The field-size table is copied too: dcp.rs `match field_type { 1 | 2 | 6 | 7 => Some(1), 3 | 8 => Some(2), 4 | 9 | 11 | 13 => Some(4), 5 | 10 | 12 | 16 | 17 | 18 => Some(8), _ => None, }` (:573-579) versus `field_width(field_type: u16)` in tiff_loader.rs:298 with the identical arms (:299-305). `timeout 60 grep -rn --include='*.rs' 'from_le_bytes' crates/calibraw-core/src | grep -v 'tests.rs\|_tests\|/tests/' | grep -c 'Little\|Big\|from_be'` shows these two files are the only byte-order switch blocks in core; finding.md's DCP rows (:826 `color_profile/dcp.rs:503,561`) are about `unreachable!`, not this.
- why it is a problem: a DCP is a TIFF (dcp.rs parses the same `II`/`MM` magic, the same `unsupported BigTIFF` case and the same field types), so the format's scalar rules now live in two places that must agree: the copy is missing `i32` on the TIFF side and the TIFF side is missing the `1 | 2 | 6 | 7` table in dcp.rs's favour, and any change to the TIFF field-size table - adding the signed variants 15 (IFD) or the 19/20 64-bit types, which a future sidecar or DCP file may use - has to be edited in both tables or the DCP reader starts rejecting (or misreading offsets in) files the TIFF reader accepts, which is a silently wrong profile, not a reported failure; the two names (`Endian`/`ByteOrder`) also make the pair invisible to a reader looking for "the TIFF reader".
- fix: one shared leaf module for TIFF scalar reading (e.g. `crates/calibraw-core/src/pipeline/tiff/scalars.rs`: the endianness enum, `u16`/`u32`/`i32`/`u64`, the `II`/`MM` sniff and the field-type -> width table), used by `tiff_loader.rs` and by `color_profile/dcp.rs`; keep each file's own struct/parse code on top of it.
- effort: M
### [CORE-026] DCP calibration sets 1 and 2 are written out twice
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/color_profile/dcp.rs (:127-139 inside `let matrices = [...]`)
- evidence:
  `let matrices = [ DcpMatrixSet { illuminant: read_u32_tag(reader, tags, CALIBRATION_ILLUMINANT_1)?.first().copied().and_then(|v| u16::try_from(v).ok()), color_matrix: read_matrix_4x3(reader, tags, COLOR_MATRIX_1)?, camera_calibration: read_matrix_4x4(reader, tags, CAMERA_CALIBRATION_1)?, forward_matrix: read_matrix_3x4(reader, tags, FORWARD_MATRIX_1)?, }, DcpMatrixSet { ...same four reads with the _2 tags... }, ];` (:127-151) - thirteen lines, differing only in the `_1`/`_2` tag constant. The same pattern repeats one level down: `read_matrix_4x3` (:205-224) and `read_matrix_3x4` (:227-247) open with the identical six-line prologue `let values = read_f32_tag(reader, tags, tag)?; if values.is_empty() { return Ok(None); } if values.len() != 9 && values.len() != 12 { bail!("DCP matrix tag {tag} has {} values, expected 9 or 12", values.len()); }` (:210-216 and :232-238). `timeout 60 grep -rn 'read_matrix_4x3\|read_matrix_3x4\|read_matrix_4x4' crates/calibraw-core/src | grep -v 'tests.rs\|_tests\|/tests/'` shows the functions are each called exactly twice, from :133-135 and :142-144, so nothing else consumes them.
- why it is a problem: DCP carries two calibration sets and the code spells both out, so the tag set is only correct if the two blocks stay aligned: swapping one `COLOR_MATRIX_2` for `COLOR_MATRIX_1` yields a profile whose illuminant and matrices come from different sets, which is a plausible-looking but wrong colour translation (the two sets are what distinguish a DCP's "v2" entries, and `camera_calibration_signature`/`calibration_signature` at :118-125 are computed from them, so the wrong pairing also mis-signatures the profile) and no test or type prevents it; the same six-line acceptance rule ("9 or 12 values") is duplicated across the two matrix readers plus a third variant in `read_matrix_4x4`, so a tightening of the rule - e.g. rejecting the 9-value form for `forward_matrix`, whose fourth row/column is currently left as zeros by the `out[row][..planes].copy_from_slice(...)` fill at :239-243 - is two coordinated edits inside one file.
- fix: one local helper `fn read_matrix_set(reader, tags, illuminant: u16, color_matrix: u16, camera_calibration: u16, forward: u16) -> DcpMatrixSet`, called as `read_matrix_set(reader, tags, CALIBRATION_ILLUMINANT_1, COLOR_MATRIX_1, CAMERA_CALIBRATION_1, FORWARD_MATRIX_1)` and the `_2` equivalent; and hoist the shared "read tag, empty means absent, length must be 9 or 12" prologue into `read_f32_matrix_tag(reader, tags, tag, shape) -> Vec<f32>` used by all three matrix readers.
- effort: M
### [CORE-027] ICC sampled-curve interpolation is pasted into both curve evaluators
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/color_profile/icc.rs `TransferCurve::forward` (:20-26) and `TransferCurve::forward_extended` (:48-53)
- evidence:
  `if values.is_empty() { return x; } let location = x * (values.len() - 1) as f32; let i = location.floor() as usize; let j = (i + 1).min(values.len() - 1); values[i] + (values[j] - values[i]) * (location - i as f32)` appears in both `Self::Sampled(values) =>` arms (:20-26 in `forward`, :48-53 in `forward_extended`), and the two functions otherwise differ only in the clamping (`forward` starts `let x = x.clamp(0.0, 1.0);` :16) and the two extrapolation branches of `forward_extended` (:33-47). `timeout 60 grep -rn 'location - i as f32' crates/calibraw-core/src --include='*.rs'` returns only these two sites, so there is no shared evaluator; finding.md and docs/rework/AUDIT.md have no ICC entry.
- why it is a problem: the linear interpolation of a TRC sample table is one rule with two copies, and the copies sit either side of 25 lines of extrapolation code, so the pair - not the clamping - is what a change to sampled-curve handling must edit twice; the two arms already answer `values.is_empty()` the same way (return `x`) and `forward_extended` then adds a separate `values.len() == 1` guard (:35-37) that `forward` does not need, i.e. the empty/one-sample degenerate cases are stated twice in an unbalanced way, which is where such a copy usually breaks.
- fix: extract `fn interpolate_sampled(values: &[f32], x: f32) -> f32` (the five-line body above, empty-table degenerate case included) and call it from both arms, keeping `forward_extended`'s extrapolation branches around it.
- effort: S
### [CORE-028] Layer rasterisation has u8/u16 twins that repeat the geometry argument pack
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks/layers.rs `rasterize_layer` (:262-275) vs `rasterize_layer_f16` (:276-292), and `rasterize_layer_region` (:294-308) vs `rasterize_layer_region_f16` (:309-322)
- evidence:
  `pub fn rasterize_layer(&self, layer: usize, atlas_width: u32, atlas_height: u32, image_width: u32, image_height: u32) -> Vec<u8> { self.rasterize_layer_coverage(layer, atlas_width, atlas_height, image_width, image_height).into_par_iter().map(|value| (value * 255.0 + 0.5) as u8).collect() }` (:262-275) and `pub fn rasterize_layer_f16(&self, layer: usize, atlas_width: u32, atlas_height: u32, image_width: u32, image_height: u32) -> Vec<u16> { self.rasterize_layer_coverage(layer, atlas_width, atlas_height, image_width, image_height).into_par_iter().map(|value| f16::from_f32(value).to_bits()).collect() }` (:276-292) - identical except the final `map`. `rasterize_layer_region` and `rasterize_layer_region_f16` repeat the same relationship for the `(layer, extent, region, full_size, lens)` pack (:294-322), and `rasterize_layer_coverage` (:158-175) is itself a forwarding wrapper over `rasterize_layer_coverage_with_sampling` (:176-...) that re-lists the same five arguments before adding `None`. `timeout 120 grep -rn --include='*.rs' 'atlas_width' crates/calibraw-core/src | grep -v 'tests.rs\|_tests\|/tests/'` shows the pack appears in exactly these four signatures (:161, :179, :265, :279) and is re-passed at :168, :185, :206, :284, :289 - there is no geometry struct anywhere; finding.md's `too_many_arguments` row (F-07, 12 sites) lists `noise.rs:63`, `retouch.rs:219`, `gpu/builder/passes.rs:280`, `gpu/readback.rs:424` and UI sites - none in layers.rs, which carries no such attribute.
- why it is a problem: the two element order `(atlas_width, atlas_height, image_width, image_height)` is stated nine times in this file, so a call that swaps the image pair - which compiles and type-checks because all four are `u32` - rasterises a layer at the wrong scale/offset and only shows up as a wrong mask edge, and the same duplication rule applies to the coverage pack (`extent`, `region`, `full_size`) where `region` is a 4-tuple `[x, y, w, h]` next to a 2-tuple `extent`; and each new storage format (an f32 readback path, a bit-packed coverage path) would today add a third copy of the same eleven lines, with the risk that one twin forgets the `+ 0.5` rounding or the `f16::from_f32` conversion that the other has
- fix: one quantising helper per output format taking the shared coverage (`fn encode_u8(coverage: Vec<f32>) -> Vec<u8>` / `fn encode_f16(...) -> Vec<u16>`, or one generic over the conversion) so the twins are one call each, and bundle the geometry as one argument (`RasterGeometry { atlas: [u32; 2], image: [u32; 2] }` and `RegionGeometry { extent, region, full_size }`) so the order cannot be transposed and each new function takes one value instead of four; `rasterize_layer_coverage` then becomes `coverage(layer, geometry, None)`.
- effort: M
### [CORE-029] The sigmoid validator calls the curve three times with the same six arguments
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/sigmoid.rs `coefficients_are_valid` (:121-166)
- evidence:
  `let black = generalized_loglogistic_sigmoid(0.0, coefficients.white_target, coefficients.paper_exposure, coefficients.film_fog, coefficients.film_power, coefficients.paper_power); let grey = generalized_loglogistic_sigmoid(MIDDLE_GREY, coefficients.white_target, coefficients.paper_exposure, coefficients.film_fog, coefficients.film_power, coefficients.paper_power); let bright = generalized_loglogistic_sigmoid(1.0e6, coefficients.white_target, coefficients.paper_exposure, coefficients.film_fog, coefficients.film_power, coefficients.paper_power);` (:129-166) - three calls whose six arguments are identical, preceded by `let values = [coefficients.white_target, coefficients.black_target, coefficients.paper_exposure, coefficients.film_fog, coefficients.film_power, coefficients.paper_power, coefficients.hue_preservation, coefficients.color_processing];` (:121-130), so the same field list is spelled four times in one function. `timeout 60 grep -rn --include='*.rs' 'generalized_loglogistic_sigmoid' crates | grep -v 'tests.rs\|_tests\|/tests/'` shows these three plus the production tone-curve evaluation site, i.e. there is no partial-application helper.
- why it is a problem: this function decides whether a saved tone curve is accepted, and it evaluates the curve at three probe points; because each call re-picks the six coefficients by hand, dropping one argument - e.g. leaving out `coefficients.paper_power` in the `grey` call while the sanity rule `grey <= bright` (:162-164) still holds for most coefficient sets - silently validates a different curve than the one the renderer evaluates, and `SigmoidCoefficients` grows with every added term, so keeping the four copies in step is the only thing holding the validator honest; the probe points `0.0`, `MIDDLE_GREY`, `1.0e6` and the tolerance `5e-3` (:165) also carry no names, so nothing states that `1.0e6` is the "HDR headroom" probe or that `5e-3` is the grey-point tolerance.
- fix: bind the coefficients once inside the function - `let curve = |x| generalized_loglogistic_sigmoid(x, coefficients.white_target, coefficients.paper_exposure, coefficients.film_fog, coefficients.film_power, coefficients.paper_power);` then `let black = curve(0.0); let grey = curve(MIDDLE_GREY); let bright = curve(1.0e6);` - and name the probe points (`PROBE_SHADOW = 0.0`, `PROBE_HDR = 1.0e6`, `GREY_TOLERANCE = 5e-3`) with the `values` finiteness check driven off the struct's fields once rather than a re-listed array.
- effort: S
### [CORE-030] Sidecar size estimate and measurement walk the mask stack twice
- category: duplicate
- severity: high
- confidence: high
- where: crates/calibraw-core/src/sidecar/size_limits.rs `estimate_sidecar_bytes` (:62-139) and `measure_sidecar_dynamic_bytes` (:141-214)
- evidence:
  The two functions have the same shape: both open by declaring the same eight constants (`const DOCUMENT_HEADROOM: u64 = 1024 * 1024; const MASK_HEADROOM: u64 = 16 * 1024; const COMPONENT_HEADROOM: u64 = 2 * 1024; const BRUSH_DAB_HEADROOM: u64 = 256; const PATH_POINT_HEADROOM: u64 = 192; const OBJECT_STROKE_HEADROOM: u64 = 128; const OBJECT_POINT_HEADROOM: u64 = 96;` at :63-69 and again at :142-148, with `MASK_PNG_FIXED_HEADROOM: u64 = 64 * 1024` at :70 only in the estimating copy), then run the same loop nest `for mask in &masks.masks { checked_add(..., MASK_HEADROOM)?; checked_add(..., escaped_json_string_bound(&mask.name)?)?; for component in &mask.components { ... match &component.geometry { MaskGeometry::Brush { dabs, .. } => checked_add_scaled(&mut x, dabs.len(), BRUSH_DAB_HEADROOM)? ... } } }` with the identical `MaskGeometry::Ai { mask: Some(image), .. } | MaskGeometry::DepthRange { depth: Some(image), .. }` and `MaskGeometry::Object { mask, strokes, .. }` arms and the same trailing `_ => {}` (:133 in the first, :208 in the second); the only difference is the asset helper called three times in each - `add_unique_mask_asset_bound(&mut estimated, image, &mut unique_images, &mut image_buckets, MASK_PNG_FIXED_HEADROOM)` (:81, :107, :116) versus `add_unique_mask_asset_measured(&mut measured, image, &mut unique_images, &mut image_buckets)` (:159, :184, :192). Both are driven from one place: `preflight_sidecar_dynamic_data_with_limit` (:48-56) runs `estimate_sidecar_bytes(masks)?` then, only if the estimate exceeds the limit, `measure_sidecar_dynamic_bytes(masks)?`.
- why it is a problem: the pair implements one rule - "what a mask stack costs in bytes" - twice, and the rule's constants are two independent copies, so the preflight already disagrees with itself wherever the copies drift: the measured path has no `MASK_PNG_FIXED_HEADROOM` at all (it counts asset bytes exactly), which is deliberate but unstated, while every other headroom is duplicated and must be kept equal by hand, and a new geometry kind or per-item cost has to be added in both traversals and in the right three places in each; when they disagree in the other direction (the estimate too low, e.g. a component kind that one traversal charges and the other does not) a sidecar the estimate accepted is rejected by the measurement, so saving fails with `SidecarError::TooLarge` reporting a number the user cannot relate to the mask they just drew, and both walks allocate the same `unique_images`/`image_buckets` deduplication bookkeeping twice per save.
- fix: one traversal (`fn sidecar_byte_cost(masks, asset_cost: FnOnce<&MaskImage> -> u64, ...)` or a visitor returning the per-component cost) that takes the asset-costing function, with the eight headroom constants hoisted to module level so both callers read the same numbers; `estimate_sidecar_bytes` and `measure_sidecar_dynamic_bytes` then become calls with `|image| png_bound_bytes(image)` and `|image| measured_png_bytes(image)`, and document that the estimating path adds `MASK_PNG_FIXED_HEADROOM` per asset while the measured path does not.
- effort: M
### [CORE-031] Subject-refinement dabs and brush-mask dabs are validated by the same rules written twice
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/sidecar/validation.rs `validate_edit_state` — subject-refinement loop (:130-145) and the `MaskGeometry::Brush` arm's dab loop (:203-218)
- evidence:
  `for dab in &refinement.dabs { finite("subject refinement dab", &[dab.center[0], dab.center[1], dab.opacity, dab.size, dab.feather])?; bounded("subject refinement dab x", dab.center[0], -16.0, 16.0)?; bounded("subject refinement dab y", dab.center[1], -16.0, 16.0)?; bounded("subject refinement dab opacity", dab.opacity, -1.0, 1.0)?; bounded("subject refinement dab size", dab.size, 0.0, 16.0)?; bounded("subject refinement dab feather", dab.feather, 0.0, 1.0)?; }` (:130-145) and `for dab in dabs { finite("brush dab", &[dab.center[0], dab.center[1], dab.opacity, dab.size, dab.feather])?; bounded("brush dab x", dab.center[0], -16.0, 16.0)?; ... }` (:203-218) - the ten rules and all five bound pairs are byte-identical, only the label string differs. The same file re-states the same coordinate window elsewhere: `bounded` appears 61 times (`timeout 30 grep -c 'bounded(' crates/calibraw-core/src/sidecar/validation.rs`), including `-16.0, 16.0` for radial centers, linear points, color samples and luminance low/high, and `bounded("radial rotation", *rotation, -1_000_000.0, 1_000_000.0)` (:246) as an effective no-limit. finding.md's validation.rs rows (:277, :334, :899) are about `validate_edit_state`'s size and a missing interaction, not this pair; docs/rework/AUDIT.md has no entry.
- why it is a problem: one rule set - "what a saved dab may contain" - has two copies, so tightening or relaxing either one (a longer brush reach, a different opacity range) must be edited in both places and a missed half lets a sidecar carry a subject-refinement dab that a brush mask would reject, or vice versa, and the renderer then rasterises values only one of the two paths bounded; and the coordinate window `-16.0..16.0` is a bare literal at ~20 of the 61 call sites, so there is no place that states "mask coordinates are normalized to at most 16 image widths" and no way to change it coherently, while `-1_000_000.0` encodes "unbounded" as a magic value that a finite check already covers, implying the two limits mean different things in the same shape
- fix: extract `fn validate_dab(label: &str, dab: MaskDab) -> Result<(), SidecarError>` (or `validate_dabs(dabs, label)`) holding the finite check and the five bounds, called from both sites, and hoist the coordinate window to named module constants (`NATIVE_COORD_LIMIT = 16.0`, `HANDLE_COORD_LIMIT = 32.0`) used at the call sites; for rotation, validate finiteness only (drop the ±1e6 pair) or state the real limit.
- effort: M
### [CORE-032] TIFF container detection re-opens and re-walks the file and hides its errors
- category: unnecessary-code
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/tiff_loader.rs `is_raster_tiff` (:114-121), `has_camera_raw_marks` (:123-147), `inspect_tiff_container` (:167-181), `has_dng_version` (:157-165); callers crates/calibraw-core/src/pipeline/raw_loader.rs:595 and :731
- evidence:
  `Ok(!has_camera_raw_marks(path).unwrap_or(true) && matches!(inspect_tiff_container(path), Ok(TiffContainerKind::Raster)))` (:117) - an unreadable or malformed file becomes "not marked" and "not a raster TIFF" rather than an error, and each predicate re-parses the container: `has_camera_raw_marks`, `inspect_tiff_container` and `has_dng_version` each call `walk_tiff_fields(path, ...)` (:150, :171, :159), whose first statement is `let (mut file, header) = open_tiff(path)?;` (:189-...), i.e. one `File::open` + metadata + full IFD-chain walk per question. The two are queried separately from the loader: `Ok(super::tiff_loader::is_raster_tiff(path)?.then_some(Self::Tiff))` (raw_loader.rs:595) and, for the same document later, `None => super::tiff_loader::has_dng_version(path).unwrap_or(false),` (raw_loader.rs:731). For an extensionless Android `/proc/self/fd/<n>` document, `is_tiff_path` returns false, so `is_raster_tiff` runs `has_camera_raw_marks` and `inspect_tiff_container`, then `has_dng_version` walks a third time.
- why it is a problem: opening a TIFF or DNG on Android parses the same IFD chain three times - each walk re-opens the descriptor (`/proc/self/fd/N` re-reads the file), re-reads the header and re-seeks every IFD entry - so detection cost scales with entry count three times over on the load path, and the two `unwrap_or`/`matches!(..., Ok(...))` collapses (:117 and raw_loader.rs:731) turn a real failure (permission denied, truncated header, an IFD offset past EOF) into the same answer as "this is not a DNG", so a file the loader could have reported as damaged is instead silently handed to the sensor decoders and usually ends in a generic decode error that names the wrong decoder; the `&& pending.len() < MAX_TIFF_SUBIFDS` guard in the same walker (:296) additionally stops walking silently once 64 sub-IFDs are queued, so an over-limit container is answered as "no such tag" rather than "too many sub-IFDs", which is the opposite of the `anyhow::ensure!(inspected < MAX_TIFF_IFDS, "TIFF contains too many IFDs")` (:211) that does report.
- fix: parse the container once per open - a `ContainerFacts` value (kind, sensor marks, DNG version) built by a single walk - and pass that value to the format probe and to `has_dng_version`, with the facts threaded to the loader instead of re-querying by path; propagate the walk's `Result` through `is_raster_tiff` (keep the existing default only where a file is genuinely not a TIFF container) and make the sub-IFD cap report an error like the IFD cap does.
- effort: M
### [CORE-034] The decoded-asset byte budget is written five times and its two limits are two copies of one value
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/sidecar.rs :54-64 (constants) and crates/calibraw-core/src/sidecar/mask_assets.rs :54-71, :184-193, :200-211, crates/calibraw-core/src/sidecar/remove_assets.rs :144-157, :295-312
- evidence:
  `const MAX_DECODED_MASK_ASSET_BYTES: u64 = if cfg!(target_os = "android") { 256 * 1024 * 1024 } else { 512 * 1024 * 1024 };` (:54) and `const MAX_DECODED_REMOVE_ASSET_BYTES: u64 = if cfg!(target_os = "android") { 256 * 1024 * 1024 } else { 512 * 1024 * 1024 };` (:59) - the same platform expression and the same two values under two names. The accumulation rule around them is copy-pasted five times: `let pixels = u64::from(asset.width).checked_mul(u64::from(asset.height)).ok_or(SidecarError::TooLarge(u64::MAX))?; decoded_bytes = decoded_bytes.checked_add(pixels).ok_or(SidecarError::TooLarge(u64::MAX))?; if decoded_bytes > MAX_DECODED_MASK_ASSET_BYTES { ... }` at mask_assets.rs:200-211, the same multiply/add/compare at :188-193 for inline depth pixels (`decoded_bytes.checked_add(image.pixels.len() as u64)`), the same again in the `add_image` closure at :64-71 (`checked_add(&mut decoded_asset_bytes, pixels)?`), and in remove_assets.rs twice with a per-pixel factor: `let decoded_bytes = pixels.checked_mul(7).ok_or(SidecarError::TooLarge(u64::MAX))?;` (:147-152 and :302-307). `timeout 90 grep -rn --include='*.rs' 'MAX_DECODED_MASK_ASSET_BYTES\|MAX_DECODED_REMOVE_ASSET_BYTES' crates` returns only these five use sites plus the two definitions; finding.md's size-limit rows cover `MAX_SIDECAR_BYTES`/`unwrap_or_default` counts, not this budget, and docs/rework/AUDIT.md has no entry.
- why it is a problem: one rule - "the decoded assets a sidecar may carry" - has five copies whose arithmetic differs (the mask paths add pixel counts, the retouch paths multiply by a bare `7` for RGB+alpha PNG planes), so the seven-channel assumption is stated nowhere and changing either the channel count or the limit means finding all five, while a copy that forgets `checked_mul` or the `checked_add` turns a large but valid sidecar into the same `SidecarError::TooLarge(u64::MAX)` that the UI reports without a byte count; the two identical constants also imply two independent limits that are in fact one number, so raising the mask budget and forgetting the retouch budget (or vice versa) is silent, and nothing states that they must move together
- fix: one `const MAX_DECODED_ASSET_BYTES` (keeping the platform `cfg!` once) used by both paths, and one helper - `fn add_decoded_pixels(&budget, &mut over_limit, pixels: u64, per_pixel: u32) -> Result<(), SidecarError>` - performing the `checked_mul(per_pixel)`/`checked_add`/limit compare, called from all five sites, with the retouch path passing its plane count as a named value (`const REMOVE_PATCH_PLANES_PER_PIXEL = 7`).
- effort: M
### [CORE-035] Card ownership of settings is mapped twice, once for global and once for mask-local edits
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/adjustment_groups.rs `ExposureParams::copy_section_from` (:142, match :158-253) and `LocalAdjustments::reset_group` (:264-303)
- evidence:
  `ExposureSection::Group(AdjustmentGroup::Light) => { self.exposure = exposure; self.contrast = contrast; self.highlights = highlights; self.shadows = shadows; self.whites = whites; self.blacks = blacks; }` (:172-179) and, in the other function, `AdjustmentGroup::Light => { self.exposure = defaults.exposure; self.contrast = defaults.contrast; self.highlights = defaults.highlights; self.shadows = defaults.shadows; self.whites = defaults.whites; self.blacks = defaults.blacks; }` (:272-279) - the same six fields under the same card; `ToneCurve` (four curves), `ColorGrading` (`color_grading`), `Effects` (`texture`, `clarity`, `dehaze`) and `ColorMixer` (`hsl_hue`, `hsl_saturation`, `hsl_luminance`, `point_colors`) are likewise assigned in both switches. The pair already disagrees where it must: `AdjustmentGroup::Detail => {}` (:290) silently resets nothing for mask-local edits, with no comment, while `Color` copies `white_balance/hue/saturation/vibrance` globally (:213-218) but `temperature/tint/hue/saturation` locally (:281-286). The file's own test `every_card_reset_preserves_every_unrelated_field` (tests, out of scope for the audit but confirming intent) hard-codes the same field lists a third time as strings, e.g. `(AdjustmentGroup::Detail, "luminance_denoise chroma_denoise denoise_detail ...")`.
- why it is a problem: "which sidebar card owns which setting" is one contract with two implementations, so moving a slider between cards (a frequent UI change) means editing both switches and the test table, and a missed half makes a mask-local reset or card reset leave the setting untouched - which is exactly the silent no-op `Detail => {}` already reads like, with nothing saying whether it is deliberate or a forgotten arm; a field added to `LocalAdjustments` also compiles into neither switch (unlike the global one, whose destructuring comment at :143 states that a new field must be assigned to exactly one section), so the invariant the file claims to protect exists for only one of the two structs
- fix: drive both resets from one section mapping - e.g. an `ExposureSection`-style accessor list shared by the two structs, or implement `LocalAdjustments::reset_group` as `copy_section_from` over a `LocalAdjustments`-shaped section enum that states its own subset - and make the local Detail case explicit (`AdjustmentGroup::Detail => {}` with a comment saying mask-local edits have no Detail settings, or an arm that resets the local Detail fields that do exist).
- effort: M
### [CORE-036] Mask-kind availability is a stub that lists every kind as available
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks.rs `MaskKind::is_available` (:148-165); consumers crates/calibraw-core/src/pipeline/masks/editing.rs:7, :21 and crates/calibraw-ui/src/ui/sidebar/masks/menus.rs:20-30, crates/calibraw-ui/src/app/ai/state.rs:379, crates/calibraw-ui/src/app/edit_history.rs:592
- evidence:
  `pub const fn is_available(self) -> bool { matches!(self, Self::Brush | Self::Fullscreen | Self::Radial | Self::Linear | Self::Path | Self::Subject | Self::Background | Self::Sky | Self::Object | Self::LuminanceRange | Self::ColorRange | Self::DepthRange) }` (:148-165) - the twelve alternatives are exactly the twelve `MaskKind` variants declared at :74-87, so the predicate is `true` for every kind today, while its neighbour `pub const fn is_experimental(self) -> bool { matches!(self, Self::DepthRange) }` (:141-143) marks `DepthRange` as both available and experimental. The consumers nonetheless implement the unavailable path: `let label = if !kind.is_available() { format!("{} · soon", kind.label()) } else if kind.is_experimental() { ... }` (:20-24) plus `.on_disabled_hover_text(unavailable_message)` (:30), whose only argument in the app is the literal `"This mask type is planned but not implemented yet."` (crates/calibraw-ui/src/ui/sidebar/masks/details.rs:52), and `if self.masks.len() >= MAX_LOCAL_MASKS || !kind.is_available() { ... }` (editing.rs:7). `timeout 90 grep -rn --include='*.rs' 'is_available' crates | grep -v masks.rs` shows no other definition of mask-kind availability; finding.md and docs/rework/AUDIT.md have no entry for `is_available`, `· soon` or that message.
- why it is a problem: the availability rule is stated as data (a hand-maintained variant list) that currently names no unavailable kind, so the "· soon" label, the disabled selectable label and its hover text are unreachable in every build, and the message they would show is a string the caller passes down two menu functions (:41 parameter) that no code path can trigger; the same list is now written four times (the enum, `is_available`, `MaskKind::label` and the menu's own `for kind in [MaskKind::Brush, ...]` at menus.rs:14-24), so adding a kind means editing all four, and forgetting `is_available` silently makes an unfinished kind selectable in the sidebar and creatable in `editing.rs` - the failure the stub exists to prevent
- fix: make availability data-driven and honest: keep the kinds the app can actually run in one named place (e.g. `UNFINISHED_KINDS: []` or a `const fn available() -> bool` per kind, defaulting false for a new variant) and derive `is_available` from it, so a new variant cannot default to selectable; and either drop the "· soon" branch plus its `unavailable_message` parameter while no kind is unavailable, or keep it with the message in one constant next to the availability data instead of a literal at details.rs:52.
- effort: S
### [CORE-037] The opposed-chroma cache key re-normalises its inputs with unnamed constants copied from the maths
- category: unnecessary-comment
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/opposed_chroma.rs `LoadedRaw::inpaint_opposed_chroma` (:378-386) and the estimators it keys
- evidence:
  `let key = OpposedChromaCacheKey { source_identity: Arc::as_ptr(&self.opposed_chroma_source_identity) as usize, wb_bits: wb_coeffs.map(f32::to_bits), black_point_bits: black_point.clamp(-0.25, 0.25).to_bits(), clip_threshold_bits: clip_threshold.max(0.01).to_bits(), use_ai_cfa };` (:378-386) reproduces two numeric rules that also live inside the code whose result the key names: `let black_offset = black_point.clamp(-0.25, 0.25) * sensor_range;` (:57, `opposed_sensor_value`) and `let clip = 0.987 * clip_threshold.max(0.01);` (:138, `prepare_opposed_chroma_candidates`), with the same clamp repeated at crates/calibraw-core/src/pipeline/raw_loader/white_balance.rs:242 (`+ black_point.clamp(-0.25, 0.25) * sensor_range)`). `timeout 90 grep -rn --include='*.rs' 'clamp(-0.25, 0.25)\|max(0.01)' crates/calibraw-core/src crates/calibraw-gpu/src | grep -v 'tests.rs\|_tests\|/tests/'` finds exactly these five (one is the `#[cfg(test)]` serial twin at :248), i.e. no named constant for either; finding.md and docs/rework/AUDIT.md have no opposed-chroma entry.
- why it is a problem: the key is what makes a cache hit stand for "the chroma of these exact inputs", so the key's clamps must equal the maths' clamps, and today they only do by coincidence: three bare `0.25` literals in two files and two bare `0.01` literals encode two contracts ("a sidecar's black point moves at most a quarter of the sensor range", "a clip threshold below 1 % counts as 1 %") with no name and no statement that the pairs must move together, so widening the accepted black-point range in `opposed_sensor_value` while leaving the key clamp in place - or the reverse, tightening the key so two different inputs collapse to one entry - yields a cached `[f32; 3]` chroma computed for parameters the caller never passed, which surfaces as wrongly coloured highlight reconstruction rather than any error
- fix: name the rules once - `const MAX_BLACK_POINT_SENSOR_FRACTION = 0.25` and `const MIN_CLIP_THRESHOLD = 0.01` - and normalise the inputs at one place (a `fn chroma_input(black_point, clip_threshold) -> (f32, f32)` or a small `ChromaInputs` struct) that the estimator and the cache key both call, so the key is built from the values the maths actually used instead of a third copy of the clamping.
- effort: S
### [CORE-038] Both crop fitters run their own unnamed binary search with different iteration counts and the same shrink-by-a-hair factor
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/geometry.rs `GeometryTransform::fit_crop_inside_transformed_source` (:248, :265) and `constrain_crop_drag_to_transformed_source` (:318, :327)
- evidence:
  `let mut low = 0.0_f32; let mut high = 1.0_f32; for _ in 0..32 { let mid = (low + high) * 0.5; if feasible_crop_center_bounds(geometry, width * mid, height * mid, source_width_f, source_height_f).is_some() { low = mid } else { high = mid } }` then `let scale = (low * 0.999_999).clamp(0.0, 1.0);` (:248-265), and in the sibling solver `for _ in 0..28 { let mid = (low + high) * 0.5; let candidate = lerp_crop(start, proposed, mid); if crop_fits_transformed_source(geometry, candidate, source_width, source_height) { low = mid } else { high = mid } }` then `sanitized_crop(lerp_crop(start, proposed, low * 0.999_999))` (:318-327). `timeout 90 grep -rn --include='*.rs' '0.999_999\|for _ in 0..32\|for _ in 0..28' crates/calibraw-core/src crates/calibraw-ui/src crates/calibraw-gpu/src | grep -v 'tests.rs\|_tests\|/tests/'` finds `0.999_999` only at geometry.rs:265 and :327 (plus an unrelated bin index at noise.rs:129) and the two loop bounds only there; finding.md and docs/rework/AUDIT.md do not cover these fitters (F-23 is affine geometry math, F-25 clamp sampling).
- why it is a problem: one rule is written twice with different numbers and no name: "search until the answer stops changing" is spelled as a bare 32 in one solver and a bare 28 in the other, so the two crops that should agree about how precisely a crop must sit inside the transformed source now disagree - the drag path accepts a crop up to 2^-28 of the source away from its limit while the fit path searches to the f32 limit at 2^-32 - and "stay strictly inside, do not touch the failing size" is the same unexplained `0.999_999` at both sites, so tightening either search or removing one copy of the factor (which is what makes the result re-pass the predicate instead of landing exactly on it) changes whether a crop drag ends 0.001 % outside the rotated source and gets silently re-sanitized by `sanitized_crop` on the next read, appearing as a crop that drifts a hair each time the image is opened
- fix: extract one search helper (e.g. `fn search_fit(lower_ok, upper_bad, predicate) -> f32` named `largest_inside`, or `geometry.rs: largest_scale_inside` / `largest_blend_inside` sharing it) that takes the predicate as a closure, and express the safety factor once as a named `const CROP_FIT_SAFETY: f32 = 0.999_999` with a comment saying it keeps the fitted crop strictly inside the predicate that rejected the requested size; the two callers then differ only in their predicate.
- effort: S
### [CORE-039] Per-stage timing instrumentation is hand-copied hundreds of times instead of living with the diagnostics API that records it
- category: duplicate
- severity: medium
- confidence: high
- where: ~54 production sites across 37 files in four crates; e.g. crates/calibraw-core/src/pipeline/chromatic_aberration.rs `LateralChromaticAberration::estimate`, crates/calibraw-ui/src/app/lifecycle/document_load.rs:134-137 and :162-171, crates/calibraw-ai/src/placement/session.rs, crates/calibraw-gpu/src/pipeline/export/worker.rs
- evidence:
  in chromatic_aberration.rs the measurement is written around one statement: `let started = std::time::Instant::now(); let model = match raw.scene_linear_raster() { ... }; let message = format!("Lateral chromatic aberration measured in {:.3}s: red {:.2} px, blue {:.2} px at the corners", started.elapsed().as_secs_f64(), model.largest_corner_shift(LateralChannel::Red, extent), model.largest_corner_shift(LateralChannel::Blue, extent)); log::debug!("{message}"); crate::diagnostics::record(message);` - the same start/elapsed/`{:.3}s`/record idiom again at document_load.rs:135-138 (`calibraw_core::diagnostics::record(format!("RAW sidecar lookup finished in {:.3}s", sidecar_started.elapsed().as_secs_f64()));`), :163-166 (`"RAW decode ready in {:.3}s"`) and inside an error arm at :169-171 (`"RAW decode failed after {:.3}s: {error:#}"`). `timeout 60 grep -rc --include='*.rs' 'diagnostics::record(' crates/calibraw-core/src crates/calibraw-ui/src crates/calibraw-gpu/src crates/calibraw-ai/src` reports 127 production calls (heaviest document_load.rs 26, lifecycle/mod.rs 16, export/worker.rs and export/streaming.rs 6 each, placement/session.rs 6), and `grep -rn 'elapsed().as_secs_f64' crates xtask` outside tests, benches and examples finds 54 hand-written elapsed expressions; `timeout 30 grep -rn 'pub ' crates/calibraw-core/src/diagnostics.rs` shows the API is only `record`, `set_device_info`, `set_gpu_info`, `clear`, `snapshot`, `record_raw` - there is no start/stop or scoped timer to hold the clock; finding.md and docs/rework/AUDIT.md have no diagnostics-instrumentation entry.
- why it is a problem: the same three-step ritual (start a clock, format seconds with three decimals, record into the diagnostics buffer, sometimes also `log::debug!` the same string) is spelled per stage by hand, so the app carries ~54 measurements of unrelated stages - sidecar lookup, RAW decode, demosaic, CA estimate, export passes, ONNX placement - that no owner can turn off, rename or drop as a release change: the message text is what the hidden diagnostics view shows, so it is duplicated product surface with no contract, and the copies drift apart in wording ("finished in", "ready in", "after") and precision already; because the ritual is interleaved with the stage it measures, several copies sit inside control-flow arms (`Err(error) => calibraw_core::diagnostics::record(format!(...)`) where the call is the arm's whole body, so removing one measurement means editing that arm's shape instead of deleting a helper, and an arm whose only purpose was measuring stays as a no-op branch
- fix: give diagnostics.rs the owner of the ritual - a scoped `StageRecord` RAII type (constructed with the stage label, writing `label finished in {:.3}s` when the scope ends, with an optional error-context variant so the `Err(...)` arm keeps its real body) plus one `record_stage(label, started)` helper for the spots that need a hand-placed clock; then each of the ~54 sites becomes one line of instrumentation, the `{:.3}s` wording and precision exist once, and the duplicated `log::debug!` copy disappears behind the helper.
- effort: M
### [CORE-040] The Big-LaMa tone shoulder and its luma cap are written twice with four bare, mutually dependent constants
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/remove/scene_color.rs `remove_scene_to_model_srgb` (:96-99), `remove_model_srgb_to_canonical_scene` (:109-113) and `remove_model_view_gain` (:118-137)
- evidence:
  forward: `let luma = (scaled[0] * 0.2627 + scaled[1] * 0.6780 + scaled[2] * 0.0593).max(0.0); let shoulder = 1.0 / (1.0 + luma); display_linear_rec2020_to_model_srgb(scaled.map(|value| value * shoulder));` (:96-99); inverse: `let mapped_luma = (...).clamp(0.0, 0.985); let undo_shoulder = 1.0 / (1.0 - mapped_luma).max(0.015);` (:111-112); and the same cap again in the exposure estimate `luminance.push(value.min(64.0));` (:124) plus `(target_linear / p75).clamp(0.25, 64.0)` (:137). No name carries the rule: `timeout 60 grep -n '0.985\|0.015\|64.0' crates/calibraw-core/src/pipeline/remove/scene_color.rs` shows only those four literals (:111, :112, :124, :137) and the file's doc-comment says only "Place the upper quartile like a normally exposed photo (about 0.63 in sRGB)". The values encode one cap, scene luma 64, in three different forms - 0.985 is 64/65, 0.015 is 1/66.7 - and finding.md and docs/rework/AUDIT.md have no entry for this file's tone mapping (grep 'shoulder' finds neither).
- why it is a problem: the forward map is `display = luma/(1+luma)`, so the inverse's `0.985` and `0.015` must be exactly what that map yields at the scene-luma cap, otherwise `remove_model_srgb_to_canonical_scene` no longer undoes `remove_scene_to_model_srgb`: at the cap the forward side reaches 0.9846, the inverse clamps at 0.985 and floors the divisor at 0.015, which re-introduces a luma of 65.7 instead of 64. Nothing states those are the same limit, and the limit itself is written a third and fourth time as `64.0` in the view-gain estimator, so raising the highlight ceiling that the estimator clamps to (or lowering the `0.25` floor that feeds it) leaves the shoulder clamps at the old cap and every retouch (remove) composite tone-shifts by that much, with no error and no log - the failure mode is the pixels Big-LaMa puts back into the photo.
- fix: state the limit once (e.g. `const MAX_REMOVE_SCENE_LUMA: f32 = 64.0`) and derive the two clamps from it (`MAX_REMOVE_SCENE_LUMA / (1.0 + MAX_REMOVE_SCENE_LUMA)` for the display cap, `1.0 / (1.0 + MAX_REMOVE_SCENE_LUMA)` for the divisor floor), then share one forward/inverse shoulder helper pair between the three functions so the curve exists once and the estimator's `min(64.0)` and `clamp(0.25, 64.0)` name the same constant.
- effort: S
### [CORE-041] scene_color.rs hand-inlines the 3×3 matrix-vector product twice instead of using matrix::transform
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/remove/scene_color.rs `pipeline_scene_to_working_rec2020` (:43-48) and `working_rec2020_to_canonical_remove_scene` (:82-87)
- evidence:
  both functions end with the same three lines: `let m = remove_camera_transform(raw);` then `[m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2], m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2], m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2]]` (:43-48) and, from the inverted matrix, `let Some(m) = invert_remove_camera_matrix(raw) else { return rgb; }; let camera_wb = [m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2], ...];` (:82-87) - `timeout 30 grep -rn 'm\[0\]\[0\]' crates/calibraw-core/src/pipeline/remove/scene_color.rs` returns only :45 and :84, i.e. exactly two copies in one 138-line file. The same file already uses the shared helper's module: `matrix::IDENTITY3` and `matrix::invert(rgb_columns)` (:83 and :75), and `matrix.rs:23 pub fn transform<const ROWS: usize, const COLUMNS: usize>(...)` is the intended call shape - `timeout 60 grep -rn --include='*.rs' 'matrix::transform' crates | grep -v 'tests.rs\|_tests\|/tests/'` shows 9 production callers elsewhere (ai/models/denoise/inference.rs:338 and :423, libraw_loader/camera_matrix.rs:192 and :214, temperature_tint.rs ×4) all spelling the same product as one call. finding.md and docs/rework/AUDIT.md have no scene_color entry.
- why it is a problem: the hand-written index form is the only place in this pair that decides which columns of `remove_camera_transform`'s `[[f32; 4]; 3]` reach the pixel: the 4th column (the white-balance coefficient the row also carries) is dropped by omitting `m[row][3]`, and that choice is visible only as an omitted term, so the two functions agree today by coincidence of copy-paste. In the second function `m` is already a `[[f32; 3]; 3]` from `matrix::invert`, where the same spelling repeats the dropping rule for a matrix that has no 4th column to drop - a future edit that widens either matrix, or that fixes one copy to the 4-column form, changes whether the remove composite's white balance includes the coefficient, which moves the colour of every retouch patch rather than failing.
- fix: give `remove_camera_transform` a `rgb_columns` view (`[[f32; 3]; 3]`) as its own accessor - `invert_remove_camera_matrix` already builds one by hand at :76 - and call `matrix::transform(rgb_columns, rgb)` in both places, so the column choice lives in one named place and both conversions read as the helper every other colour site already uses.
- effort: S
### [CORE-042] TIFF and DNG tag codes are bare numbers throughout the hand-written container reader and writer
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/tiff_loader.rs (reader predicates `has_camera_raw_marks` :137-146, `has_dng_version` :163, `inspect_tiff_container` :170-178, `walk_tiff_fields` :262, `read embedded ICC` :441) and its writer (:555, :591, :649); plus crates/calibraw-core/src/pipeline/raw_loader/rawler_loader.rs:291
- evidence:
  the reader decides "this is a camera RAW TIFF" from unexplained integers: `let sensor = match tag { 258 => integer_values(file, field, 4)?.into_iter().any(|bits| !matches!(bits, 1 | 8 | 16 | 32 | 64)), 259 => scalar_u64(file, field)?.is_some_and(|compression| SENSOR_COMPRESSIONS.contains(&compression)), _ => false };` (:138-140), `Ok((tag == 50706 && field.field_type == 1 && field.count == 4).then_some(true))` (:163), `if matches!(tag, 33421 | 33422) { return Ok(Some(TiffContainerKind::Sensor)) }` (:170-172), `if tag == 262 && field.count == 1 { ... if matches!(value, 32803 | 34892) ... }` (:173-177), `if tag == 330 && pending.len() < MAX_TIFF_SUBIFDS {` (:262) and `if tag != 34675 {` (:441); the writer emits three of the same codes `put_u16(&mut bytes, 58, 33421);` (:555), `bytes.extend_from_slice(&32803u16.to_le_bytes());` (:591), `bytes.extend_from_slice(&50706u16.to_le_bytes());` (:649), and rawler_loader.rs:291 compares `photometric == 32803 ... photometric == 34892`. The repo already has a naming convention for exactly these codes: `rawler_loader.rs:23 use rawler::tags::{DngTag, TiffCommonTag};` used as `TiffCommonTag::ImageWidth`, `DngTag::ActiveArea` (:129, :140) and, for the tag the enum lacks, `rawler_loader.rs:32 const TIFF_TAG_PLANAR_CONFIGURATION: u16 = 284;`; tiff_loader.rs names only its size limits (`MAX_TIFF_IFDS`, `MAX_TIFF_SUBIFDS`, `MIN_TIFF_ICC_BYTES`) and one compression list. finding.md and docs/rework/AUDIT.md have no tiff_loader tag entry (grep '50706\|33421\|32803' in both returns nothing).
- why it is a problem: eight tag codes and two photometric values are the format's identity, and they are only written as digits in one module that must keep them consistent in two directions: 33421 is read at :170 to call a file sensor data and written at :555 to make a DNG that the reader will recognise, 50706 likewise (:163 / :649), 32803 (:175 / :591) - so a transposed digit in one of the two copies does not fail to compile, it makes the app's own output file take the other branch of `is_raster_tiff` (:198 `is_tiff_path` + content sniff) when it is reopened, i.e. a DNG the app wrote is later decoded as a plain raster TIFF (wrong colours, no demosaic, no camera profiles) or a real camera file is opened through the raster path, with nothing in either case pointing at the tag that disagrees. The same digits also drive `has_camera_raw_marks`, whose only comment describes compressions, not what 258 means ("BitsPerSample: only 1/8/16/32/64-bit is a raster, 12-bit and 14-bit are sensors") or what 262/32803 mean.
- fix: introduce one tag vocabulary for this module - either reuse `rawler::tags` (`TiffCommonTag::BitsPerSample`, `TiffCommonTag::Compression`, `TiffCommonTag::SubIFDs`, `DngTag::...`) as the sibling loader already does, or declare `TIFF_TAG_BITS_PER_SAMPLE: u16 = 258` etc. following the `TIFF_TAG_PLANAR_CONFIGURATION` precedent, with the DNG photometric values named (`TIFF_PHOTOMETRIC_BAYER_CFA`, `TIFF_PHOTOMETRIC_LINEAR_PIXELS`) - then the reader predicates and the writer reference the same names, and a short comment per name states which format field it is.
- effort: M
### [CORE-043] Public wildcard module re-exports and use super::* in every core sub-module
- category: rule-violation
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks.rs (:32, :40, :45-:47), geometry.rs (:11, :12, :14), processing.rs (:8, :9), raw_loader.rs (:33, :34), remove.rs (:11-:14), sidecar.rs (:23, :26), raw_loader/libraw_loader.rs (:46, :48-:51) and 42 further `use super::*;` first lines in core child modules
- evidence:
  23 public wildcard re-exports sit next to explicit imports in the same blocks: `pub use brush::*;` (:32), `pub use shapes::*;` (:40), `pub use adjustments::*;` (:45), `pub use mask_geometry::*;` (:46), `pub use primitives::*;` (:47) in masks.rs; `pub use inverse_map::*;` / `pub use lens_map::*;` / `pub use thumbnail::*;` in geometry.rs; `pub use proxy::*; pub use tiles::*;` in processing.rs; `pub use compact_pixel_map::*; pub use opposed_chroma::*;` in raw_loader.rs; `pub use brush::*; pub use composite::*; pub use passes::*; pub use scene_color::*;` in remove.rs; `pub use files::*; pub use size_limits::*;` in sidecar.rs; `pub(super) use camera_matrix::*; ... thumbnail::*;` in libraw_loader.rs. Child modules then start by pulling everything back in: `use super::*;` is the first line of 42 production core files (e.g. color_profile/dcp.rs:1, icc.rs:1, masks/brush.rs:3, layers.rs:3, primitives.rs:3, probability.rs:3, shapes.rs:3, editing.rs:3) - `timeout 60 grep -rn --include='*.rs' -E '^use super::\*;' crates/calibraw-core/src | grep -v 'tests.rs\|/tests/' | wc -l` = 42 (53 including test modules). AGENTS.md's Rust rules require "Keep imports explicit in production modules"; finding.md has no core import entry (only row 15 in docs/rework/AUDIT.md, which is about Moduwu re-exports in calibraw-ui and is already implemented).
- why it is a problem: the public surface of eight core modules is not in source order, so the crate boundary that `cargo xtask arch-check` enforces cannot be read or reviewed: adding a helper to masks/raster.rs becomes a public `pipeline::*` name whether or not it was meant to be public, two child modules that both export `load` collide with a confusing error rather than the intended one, and every `use super::*;` child resolves names like `MAX_RAW_EDGE` or `ensure` through a search path that is not visible at the call site, which is what makes a rename in one sibling module (or a new sibling with the same name) break an unrelated file. The `#[serde(default)]` field names on the same structs keep serialized keys, but a wildcard re-export is exactly the mechanism that lets a type be constructed from outside its module without its `::new` validation path.
- fix: replace each `pub use <module>::*;` with the explicit `pub use <module>::{names...}` list (or re-export the module itself, `pub mod brush;`), and replace `use super::*;` in child modules with the named items each file actually uses; keep the child-module splitting, just name the imports. This is hygiene only and changes no behaviour.
- effort: M
### [CORE-044] Preset group names are validated by two functions that differ only in the empty-group case
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/presets.rs `normalize_group` (:380-387) and `pub fn validate_group_name` (:389-397)
- evidence:
  `fn normalize_group(group: &str) -> Result<String, PresetError> { let group = group.trim(); if group.is_empty() { return Ok(DEFAULT_PRESET_GROUP.to_owned()); } validate_label(group, "group name")?; Ok(group.to_owned()) }` and `/// Checks a group name typed by the user and returns it trimmed. pub fn validate_group_name(name: &str) -> Result<String, PresetError> { let name = name.trim(); if name.is_empty() { return invalid("enter a group name"); } validate_label(name, "group name")?; Ok(name.to_owned()) }` - the trim, the empty branch, the same `validate_label(..., "group name")` call and the `to_owned()` are identical. Both are live: `normalize_group` from `Preset::new` (:101), `Preset::renamed` (:139), `is_named` (:148); `validate_group_name` from `read_group_list` (:600), `create_group` (:619), `rename_group` (:632) - and both are used only inside this file (`timeout 60 grep -rn --include='*.rs' 'validate_group_name' crates` returns 4 lines, all presets.rs). The limit itself lives in a third place, `validate_label` (:368, checking `MAX_PRESET_NAME_CHARS` and control characters), while `is_named` (:148) compares `self.name.to_lowercase() == name.trim().to_lowercase()` without either helper. finding.md and docs/rework/AUDIT.md have no presets.rs entry.
- why it is a problem: "what a group name may be" is decided in two places that must stay in step: today the two differ only in how an empty group is treated (file it under `User Presets` vs reject with "enter a group name"), which is a real but undocumented difference - `create_group("")` fails while `Preset::new(name, "")` succeeds. A future relaxation (allowing spaces, raising `MAX_PRESET_NAME_CHARS`, or rejecting a character) has to be made in both, and if it is made in one the app will accept a group in the preset dialog that the group-list reader then refuses, so a preset written in that session fails to load on the next start with "invalid group list" - a persisted-format failure with no path to fix it. `is_named`'s inline trim+lowercase comparison is a third normalization rule, so renaming a preset whose name differs only by surrounding whitespace can be reported as a new preset next to the old one instead of a rename.
- fix: keep one validating function (`validate_group_name`) and express the empty-group default at the call site that needs it (`Preset::new`/`renamed`/`is_named` pass `DEFAULT_PRESET_GROUP` when given `""`), and make `is_named` compare the two names through that same normalization so all three share one rule; state the "empty group means the default group" contract in a doc comment on `Preset::new`.
- effort: S
### [CORE-045] The desktop thumbnail stamp re-invents the .tif/.tiff test and picks the pipeline generation from two bare numbers
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/thumbnail_cache.rs `desktop_raw_stamp` (:552-573)
- evidence:
  `let modified = metadata.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map(|duration| duration.as_nanos()).unwrap_or_default(); let generation = if raw_path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| { extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff") }) { // Generation 3 drops thumbnails whose float downscale lifted black. 3 } else { 1 }; Ok(format!("v{generation}:{}:{modified}", metadata.len()))` - `timeout 60 grep -rn --include='*.rs' 'eq_ignore_ascii_case("tif' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns exactly two spellings of the same test, the other being `crates/calibraw-core/src/pipeline/tiff_loader.rs:155` inside `pub(super) fn is_tiff_path` (:148), which thumbnail_cache.rs cannot call from `crate::thumbnail_cache` (pub(super) stops at `crate::pipeline`); so "is this a TIFF" now exists twice in the app. The same grep pair shows nothing about this in finding.md or docs/rework/AUDIT.md (`grep -rn 'desktop_raw_stamp\|generation.*thumbnail' finding.md docs/rework/AUDIT.md` = no output). The two generation numbers have no names, no list of what generation 2 was, and the `modified` value is a cache key that silently becomes 0 when a filesystem reports no modification time.
- why it is a problem: the generation exists to invalidate cached thumbnails when the code that makes them changes, but it is chosen from the file's *extension*, i.e. from what the file is called, not from what was rendered - a float/hdr RAW or an exported `.tif` that is not a raster TIFF (`tiff_loader.rs:198` `is_raster_tiff` decides by content, CFA/compression/bit depth, and the extension test is only the first branch) gets the v3 key while the pipeline that rendered it is the one `is_raster_tiff` chose, so the two tests disagree exactly for the extensionless Android descriptor paths and for `.tif` files that carry sensor marks, and a stale v1 thumbnail is then accepted for content that renders with the float path (or vice versa) - the app shows a thumbnail from the other colour pipeline, with nothing left to invalidate it. `unwrap_or_default()` makes "no modification time" equal to mtime 0, which is a valid timestamp, so a photo whose mtime cannot be read is permanently stamped as `v1:<size>:0` and any later edit to it keeps the same key: the cached thumbnail is never invalidated for the file that actually changed.
- fix: make the stamp state what it is keyed on - name the generations (an enum or `const THUMBNAIL_STAMP_GENERATION_FLOAT_RASTER / DEFAULT` with the change list in a doc comment) and derive the one used from the loader's own decision (`is_raster_tiff`, widened to `pub` or moved to a shared `pipeline` helper together with `is_tiff_path` so one extension test serves both) rather than a second copy of the extension test; and make a missing mtime a distinct stamp component (e.g. an explicit `mtime-unavailable` token plus the file size) so such a file fails to match after it changes instead of matching the 0 sentinel forever.
- effort: M
### [CORE-046] crop_mask_image and crop_rgb_image are one function copied for two image types
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks/raster.rs `crop_mask_image` (:67-70) and `crop_rgb_image` (:73-82)
- evidence:
  `pub(super) fn crop_mask_image(source: &MaskImage, u0: f32, v0: f32, du: f32, dv: f32) -> MaskImage { let mut cropped = source.clone(); cropped.sampling_rect = crop_sampling_rect(source.sampling_rect, u0, v0, du, dv); cropped }` and `pub(super) fn crop_rgb_image(source: &MaskRgbImage, u0: f32, v0: f32, du: f32, dv: f32) -> MaskRgbImage { ... }` - the bodies are byte-identical apart from the type, `timeout 60 grep -rn --include='*.rs' 'crop_mask_image\|crop_rgb_image' crates | grep -v 'tests.rs\|/tests/'` shows 4 call sites, all in masks/layers.rs (:74, :88, :105, :112), and `crop_sampling_rect` (:85) is shared already. finding.md and docs/rework/AUDIT.md do not list this pair.
- why it is a problem: the crop rule (which corner `u0/v0` is in, that `du/dv` are fractions of the *source* rect, that the rect is widened by `source[2]-source[0]` rather than by an edge length) lives only inside the shared helper, which is fine, but the pair itself means the masks module now has two names for the same operation and each caller must know which image kind it holds: a luminance layer cropped with `crop_rgb_image`, or an rgb layer with `crop_mask_image`, compiles silently because both types expose `sampling_rect: [f32; 4]`, and the two then disagree about which buffer the rect indexes - the layer renders with the other image's sampling window, i.e. a mask or colour layer sampled at the wrong place in the photo, with no compile error and no runtime check.
- fix: express it once over the shared shape - both types already agree on `sampling_rect: [f32; 4]`, so one `pub(super) fn crop_sampling_rect_into(...)` returning the new rect plus `.with_sampling_rect(...)` at the 4 call sites, or make the helper generic over the image type with a `with_rect` method so the type only appears at the call site that needs the concrete name.
- effort: S
### [CORE-047] The D65/Rec.2020 luma weights are spelled as four-decimal literals at every Rust call site
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks/raster.rs:380 (luminance-range mask), crates/calibraw-ai/src/models/mask_refine.rs:165, crates/calibraw-ui/src/ui/theme.rs:79 and :95-97 (UI contrast/appearance)
- evidence:
  `luminance_to_lightness(0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2])` (raster.rs:380), `(0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32) / 255.0` (mask_refine.rs:165), `let luma = average[0] * 0.2126 + average[1] * 0.7152 + average[2] * 0.0722;` (theme.rs:79) and the three-line `+` form at :95-97 - `timeout 60 grep -rn --include='*.rs' '0.2126\|0.7152\|0.0722' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns these 5 Rust lines and no Rust constant; the GPU side already names the same weights, `crates/calibraw-gpu/src/shaders/common.wgsl:188 const SRGB_LUMA: vec3<f32> = vec3<f32>(0.2126729, 0.7151522, 0.0721750);`, while `preview_histogram.wgsl:44` re-spells them bare. SHADER-003 covers only the other triplet (the sRGB-space 0.2627/0.6780/0.0593 weights) and this triplet is not in finding.md or docs/rework/AUDIT.md (`grep -rn '0.2126' finding.md docs/rework/AUDIT.md` = no output).
- why it is a problem: four call sites in three crates re-type the weights, and they are not the shader's values: the shader's `SRGB_LUMA` is 0.2126729/0.7151522/0.0721750 while the Rust sites use 0.2126/0.7152/0.0722, so a mask matte, an AI mask refinement and the histogram that feeds the same tone curve are built from three different luma definitions that all claim to be "Rec.2020 luma". Today the difference is ~7e-5, below the visible threshold, but nothing states which definition is contractual: the histogram bin, the luminance-range mask matte and the L* conversion in `luminance_range_weight` sit on either side of one another in the same pipeline, and a range mask whose boundary the UI drew from a curve can rasterize on the other side of a pixel that the preview said was inside it once the weights are nudged - the failure appears as a hairline of wrong coverage in the edit, not as an error.
- fix: add one `pub const` per spelling in the colour module (e.g. `LINEAR_LUMA_D65` for the 7-digit pair matching the shader and, if the four-decimal form is intentional, a second constant with a comment stating why the UI and masks round), and replace the 5 Rust spellings and `preview_histogram.wgsl:44` with those names so the Rust and shader definitions are comparable by name; a layout/contract test should pin each shader to the constant it uses, as SHADER-003's fix proposes for the other triplet.
- effort: S
### [CORE-048] luminance_to_lightness and its inverse re-declare the CIE constants and place the split point in different units
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks/raster.rs `luminance_to_lightness` (:408-417) and `lightness_to_luminance` (:420-429)
- evidence:
  the forward function starts `const EPSILON: f32 = 216.0 / 24_389.0; const KAPPA: f32 = 24_389.0 / 27.0; let luminance = luminance.clamp(0.0, 1.0); if luminance > EPSILON { 1.16 * luminance.cbrt() - 0.16 } else { KAPPA * luminance / 100.0 }` and the inverse starts `const KAPPA: f32 = 24_389.0 / 27.0; let lightness = lightness.clamp(0.0, 1.0); if lightness > 0.08 { ((lightness + 0.16) / 1.16).powi(3) } else { lightness * 100.0 / KAPPA }` - `timeout 60 grep -rn --include='*.rs' '24_389' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns only raster.rs:410 (`EPSILON: f32 = 216.0 / 24_389.0`), :411 and :422, i.e. `KAPPA` is written twice, once per direction, and nothing else in the workspace uses these constants. The doc comments say only `/// Inverse of [luminance_to_lightness].` - neither states that `0.08` is the lightness form of `216.0 / 24_389.0`.
- why it is a problem: the pair is presented as exact inverses, and they are only inverses while the two guards agree: the forward branches at luminance > 216/24389 (= 0.0088528...) and the inverse branches at lightness > 0.08, which is the same CIE split point only by coincidence of the standard's rounding, stated here as an unexplained rounded literal. Because `EPSILON` and `KAPPA` are function-local, the shared meaning is invisible: editing one direction's threshold (or reusing these helpers from another crate, which cannot see them) leaves a wedge where `lightness_to_luminance(luminance_to_lightness(x)) != x` at the branch boundary, and that wedge sits inside the luminance-range mask and histogram path that both call these functions - coverage at the mask's low boundary flips between the analytic and linear half of the ramp, i.e. the mask edge and its preview disagree by a pixel band, with no error anywhere.
- fix: lift the CIE constants to module-level `const CIE_LIGHTNESS_EPSILON`/`CIE_LIGHTNESS_KAPPA`/`CIE_LIGHTNESS_SPLIT` with the invariant stated once (`0.08` derived as `1.16 * EPSILON.cbrt() - 0.16`, or branch both directions on the same value), and have the inverse's guard use that derived constant so the pair is inverse by construction rather than by agreement of two literals.
- effort: S
### [CORE-049] rasterize_mask_image and sample_rgb copy the whole sampling_rect bilinear resampler in one file
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/masks/raster.rs `rasterize_mask_image` (:317-352) and `sample_rgb` (:565-604)
- evidence:
  both functions re-derive the same coordinate mapping and the same blend inside a parallel row loop. rasterize_mask_image: `let sample_v = mask.sampling_rect[1] + (y as f32 + 0.5) / height as f32 * (mask.sampling_rect[3] - mask.sampling_rect[1]); let source_y = (sample_v * mask.height as f32 - 0.5).clamp(0.0, mask.height.saturating_sub(1) as f32); let y0 = source_y.floor() as usize; let y1 = (y0 + 1).min(mask.height as usize - 1); let fy = source_y - y0 as f32;` then per x the same five lines for `sample_u`/`source_x`/`x0`/`x1`/`fx`, then `let top_left = mask.pixels[y0 * mask.width as usize + x0] as f32 / 255.0;` ×4 and `let top = top_left + (top_right - top_left) * fx; let bottom = ...; *value = top + (bottom - top) * fy;` (:330-352). sample_rgb: `let sample_v = source.sampling_rect[1] + (y as f32 + 0.5) / height.max(1) as f32 * (source.sampling_rect[3] - source.sampling_rect[1]); let source_y = (sample_v * source.height as f32 - 0.5).clamp(0.0, source.height.saturating_sub(1) as f32); let y0 = ...; let y1 = (y0 + 1).min(source.height as usize - 1); let fy = source_y - y0 as f32;` and per x the same, with the blend moved inside `std::array::from_fn` over channels and indexing `source.rgba[(y * source.width + x) * 4 + channel]` (:578-604). `timeout 60 grep -rn --include='*.rs' 'sampling_rect\[0\]' crates | grep -v 'tests.rs\|_tests\|/tests/'` returns only these two bilinear sites (plus `probability.rs:88`, a nearest-neighbour variant, and `raster_edge` :487) - `grep -n 'fn sample_rgb\|fn rasterize_mask_image' crates/calibraw-core/src/pipeline/masks/raster.rs` confirms there is no shared sampler, and unlike CORE-003's four plain bilinear sites these two also carry the sampling-rectangle mapping. Both are live: `rasterize_mask_image` from the Ai/Object/DepthRange arms of `rasterize_component_internal`, `sample_rgb` from `rasterize_luminance_range` and `rasterize_color_range`.
- why it is a problem: the mask raster's coordinate contract lives in these two copies: that the raster covers the source's `sampling_rect` as a fraction of the *mask* image, that pixel centres are `(i + 0.5)/extent`, that the source sample is offset by `-0.5` before clamping to `dim-1`, and that the edge clamp is `.min(dim-1)` rather than a wrap. Two places hold it, one of them 60 lines from the other and with a different zero-guard (`height.max(1)` in `sample_rgb` vs a bare `height` in `rasterize_mask_image`, which survives only because each function has its own `width == 0` guard above it). A colour-range mask and an AI mask are sampled by different copies of this rule, so a change to one - e.g. dropping the `0.5` when a future caller passes an exact-size raster, or clamping to `dim` instead of `dim-1` - makes the colour matte and the AI matte sample half a pixel apart in the same edit: the feathered boundary of a colour-range mask lands offset from the AI mask it was drawn next to, in the rendered photo and in the exported file, with no error and no test comparing the two.
- fix: extract one sampler over the shared rectangle - a `sample_at(rect, extent, source_extent) -> (lower, upper, weight)` pair (the shape already exists as a lambda inside `resample_coverage`, :275) plus a `blend` helper, or a single `sample_at_rect(source, rect, width, height, channel_at: impl)` used by both, keeping each function's own per-channel conversion (`/255.0` plane vs RGBA) behind the callback; document the `-0.5`/clamp/`+0.5` convention once on the shared helper.
- effort: M
### [CORE-050] The sensor noise estimator hard-codes the raster path's sample count and highlight cut
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/noise.rs `NoiseProfile::estimate` (:113, :181)
- evidence:
  the RAW-path estimator writes `Some(v) if v < 0.985 => v,` (:113) and `let confidence = (points.iter().map(|point| point.2).sum::<f32>() / 2_000.0).clamp(0.0, 1.0);` (:181), while the raster estimator of the same struct names both values: `const RASTER_FULL_CONFIDENCE_SAMPLES: f32 = 2_000.0;` (:32, used as `(samples.len() as f32 / RASTER_FULL_CONFIDENCE_SAMPLES).min(1.0)` at :258) and `const RASTER_MEASURED_LEVELS: Range<f32> = 0.02..0.985;` (:23, used as `if !RASTER_MEASURED_LEVELS.contains(&center) { continue }`) - `timeout 30 grep -n '2_000\|0.985' crates/calibraw-core/src/pipeline/noise.rs` returns :23, :32 (the definitions) and :113, :181 (the bare copies) and nothing else, so each value exists in exactly one named and one unnamed spelling in the same 240-line production region. The doc comments describe the raster constants ("Samples a channel needs before its measurement replaces the nominal one"; "Encoded levels whose noise a raster estimate measures. Crushed blacks and clipped highlights are flat and would read as clean") but say nothing about the RAW copies. finding.md and docs/rework/AUDIT.md have no noise.rs entry (`grep -rn '2_000\|noise.rs' finding.md docs/rework/AUDIT.md` = no output).
- why it is a problem: `confidence` is the blend between the ISO-based fallback and the measured fit (`shot[channel] = fallback.shot * (1 - confidence) + fitted_a * confidence`), so the number of samples that fully trusts a measurement is a tuning constant that the two paths now hold separately: retuning the raster path's `RASTER_FULL_CONFIDENCE_SAMPLES` (which is what a developer would edit, since it is the named one) leaves the RAW estimator at 2000, and RAW and raster then reach full trust at different sample counts - the same photo's denoise strength differs between the RAW edit and the exported/rendered raster, i.e. visible denoise disagreement rather than an error. Likewise `v < 0.985` skips clipped highlights in the RAW fit; the raster path skips the same levels with a documented range, so raising the raster range's upper bound (to measure near-white noise) leaves the RAW estimator ignoring exactly the levels it was written to ignore, silently making the two profiles disagree at the highlight end where denoise matters most.
- fix: name the RAW path's own thresholds (`RAW_MEASURED_LEVEL_MAX`, `NOISE_FULL_CONFIDENCE_SAMPLES`) next to the raster ones with the derivation stated, use the same confidence expression in both paths (one helper returning the 0-1 trust from a sample count), and if the two paths genuinely share a level cut, use the single named range for both.
- effort: S
### [CORE-051] adaptive_detail_defaults maps noise to the denoise sliders with a line of unlabelled tuning constants
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/noise.rs `NoiseProfile::adaptive_detail_defaults` (:288-330); applied by `RawImage::apply_adaptive_detail_defaults` (raw_loader.rs:563-576) at 7 production call sites (raw_loader.rs:561, rendered_loader.rs:302, cli calibraw-develop-export.rs:214, ui document_load.rs:208/:375, batch.rs:200, export.rs:748, ui/sidebar/navigation.rs:372)
- evidence:
  the mapping is `let luminance_need = smoothstep(0.018, 0.080, relative_signal_sigma); let chroma_need = smoothstep(0.035, 0.160, relative_opponent_sigma); let luminance_denoise = 34.0 * luminance_need.powf(1.15); let chroma_denoise = 0.38 * chroma_need.powf(0.8); ... denoise_detail: 50.0 + 8.0 * luminance_need, denoise_quality: if luminance_need > 0.72 || chroma_need > 0.40 { DenoiseQuality::High } else { DenoiseQuality::Balanced }` - the doc comment above the function explains the variance combination ("The profile with each CFA channel's variance multiplied by its factor in factors...") and the body names two constants (`REFERENCE_SIGNAL: f32 = 0.18`, `METADATA_VARIANCE_FLOOR: f32 = 1.0`) but the ten numbers that set the slider values - 0.018/0.080, 0.035/0.160, 34.0, 1.15, 0.38, 0.8, 50.0, 8.0, 0.72, 0.40 - are unlabelled, as is the luma weighting above them (`0.25f32.powi(2) * variance[0] + 0.50f32.powi(2) * variance[1] + 0.25f32.powi(2) * variance[2]`), which is a third set of luma weights beside the D65 weights in CORE-047 and the sRGB 0.2627 weights in SHADER-003; `timeout 30 grep -n 'default_denoise_detail' crates/calibraw-core/src/pipeline/basicadj.rs` shows `const fn default_denoise_detail() -> f32 { 50.0 }` (:405) used as `denoise_detail: default_denoise_detail()` (:373) and `#[serde(default = "default_denoise_detail")]` (:275), so the same "untouched" detail value exists twice, once named and once bare inside `50.0 + 8.0 * luminance_need`; `finding.md`/`docs/rework/AUDIT.md` contain no noise.rs entry.
- why it is a problem: these are the values the app shows as "auto denoise" for every photo, so they are the one place where noise measurement becomes a slider position, and nothing states what `34.0` means (an implicit ceiling: with `luminance_need` in [0,1], the auto value can never exceed 34.0 on a 0-100 slider - `grep -rn '"luminance_denoise"' crates/calibraw-cli/src/bin/calibraw-develop-export.rs` shows the slider is clamped to 0..100 elsewhere) or which sigma the 0.018/0.080 window is: the mapping is a tuned recipe, and re-tuning it for a noisier sensor generation means editing four numbers that a reviewer cannot tell apart from units, scales, or an accident. Two of the interactions are silently load-bearing: `denoise_detail: 50.0 + 8.0 * luminance_need` re-derives the untouched-detail baseline from the literal 50.0 instead of the named `default_denoise_detail()`, so a change to that baseline (which is also the sidecar default, so every old file relies on it) makes adaptive detail disagree with the file default it was written against; and `0.38 * chroma_need.powf(0.8)` produces a chroma value on the 0-1 scale while the UI/CLI scale chroma denoise by 100 (cli:286 `value.clamp(0.0, 100.0) / 100.0`), so one of these three spellings of the chroma unit is wrong for the others and a future edit to any one silently rescales the auto chroma denoise of every photo.
- fix: give the tuning block named constants with units and intent (`AUTO_LUMINANCE_DENOISE_MAX`, `AUTO_LUMINANCE_SIGMA_FADE_LOW/HIGH`, `AUTO_CHROMA_DENOISE_MAX` with the 0-1 scale stated, `AUTO_DENOISE_DETAIL_BASELINE = default_denoise_detail()`, `AUTO_DENOISE_DETAIL_SLOPE`, `HIGH_QUALITY_NEED_LUMINANCE/CHROMA`), keep the `powf` exponents named as curve shapes, and name the luma weighting as one constant referenced by every weighting in the codebase so the CORE-047/SHADER-003 spellings can be compared by name.
- effort: S
### [CORE-052] The as-shot white balance falls back to a bare D65 and clamps to a range no other reader uses
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/camera_matrix.rs:72 and :129-131; crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/temperature_tint.rs:65
- evidence:
  `camera_matrix.rs:129-131` computes the camera's as-shot correlated temperature as `let base_cct = cct_from_profile_weight(&color_model, weight).or_else(|| estimate_scene_cct(color, wb_coeffs, cdesc)).unwrap_or(6504.0).clamp(1500.0, 50_000.0);`, and the simpler single-matrix path at :72 uses `base_cct: cct_from_profile_weight(&color, weight).unwrap_or(6504.0)` with no clamp at all, while `temperature_tint.rs:65` starts the daylight search at `let xyz_to_camera = xyz_to_camera_at(&model.color, 6504.0);` - `timeout 60 grep -rn --include='*.rs' '1500.0, 50_000.0|50_000.0|MIN_TEMPERATURE_KELVIN' crates | grep -v 'tests.rs|_tests|/tests/|benches'` returns `clamp(1500.0, 50_000.0)` once (camera_matrix.rs:131) and every other temperature clamp as the named pair `MIN_TEMPERATURE_KELVIN = 1_901.0` / `MAX_TEMPERATURE_KELVIN = 25_000.0` (basicadj.rs:171-172, used at basicadj.rs:194/:196/:200/:201 and temperature_tint.rs:310/:461/:530/:565/:638), and `grep -rn 6504 crates` shows the same bare `6504.0` at 3 production sites plus test literals, while the D65 *tristimulus* value next to it is named (`D65_XYZ` at temperature_tint.rs:72).
- why it is a problem: the temperature of the as-shot white is the base that every temperature offset and every `older_reader_offsets` value is measured against, and here the "usable range" of a camera's own white is invented as 1500-50000 K with two unlabelled literals while the controls that consume it clamp to 1901-25000: an inferred as-shot white of, say, 40000 K is stored, then `temperature_offset_from_kelvin` clamps the *target* to 25000 (`(1_000_000.0/base - 1_000_000.0/target).clamp(-GLOBAL_TEMPERATURE_LIMIT, GLOBAL_TEMPERATURE_LIMIT)`, basicadj.rs:200-203) so the slider's full range no longer spans the photo's own white - "as-shot" and "+0" then render different whites, and pasting that edit onto a photo with a normal as-shot white shifts it by the difference, in the edit and in the exported file, with nothing reported. The single-matrix path skips the clamp entirely, so the two paths disagree about whether an as-shot temperature is bounded at all, which is the thing that decides whether a preset's absolute kelvin survives onto a camera whose profile has no illuminant metadata. The bare `6504.0` carries the same problem in a smaller way: when no profile illuminant and no scene estimate is available, the app silently claims D65 daylight, and a photo shot under tungsten then gets as-shot temperature = D65, so `Custom { kelvin }` edits and `older_reader_offsets` written for v1.2-1.3 are computed from a white the camera never had; the fallback is not stated at either site and is indistinguishable from a real measurement in the saved file.
- fix: name both (`AS_SHOT_CCT_FALLBACK` = D65 correlated temperature, next to the existing `D65_XYZ`, and the as-shot bounds as one named `AS_SHOT_TEMPERATURE_RANGE` defined in basicadj.rs beside `MIN/MAX_TEMPERATURE_KELVIN` with the relation between the two ranges stated, or reuse the slider pair if the wider range is not intentional), use them at all three sites, and clamp the single-matrix path with the same range so both paths bound the as-shot white identically.
- effort: S
### [CORE-053] kelvin↔mired conversion repeats 1_000_000.0 ten times in three files
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/cct.rs:66-68, crates/calibraw-core/src/pipeline/raw_loader/libraw_loader/temperature_tint.rs:160-162, crates/calibraw-core/src/pipeline/basicadj.rs:194-201
- evidence:
  `let first = 1_000_000.0 / first_cct.max(1.0); let second = 1_000_000.0 / second_cct.max(1.0); let scene = 1_000_000.0 / cct.max(1.0);` (cct.rs:66-68), `let first = 1_000_000.0 / endpoints[0].cct?.max(1.0); let second = 1_000_000.0 / endpoints[1].cct?.max(1.0); Some(1_000_000.0 / (first + (second - first) * weight.clamp(0.0, 1.0)))` (temperature_tint.rs:160-162) and `(1_000_000.0 / base - 1_000_000.0 / target)` plus the two clamped forms (basicadj.rs:194-201) - `timeout 60 grep -rn --include='*.rs' '1_000_000.0' crates | grep -v 'tests.rs|_tests|/tests/|benches'` lists these 10 lines and otherwise only byte-size formatting (`settings.rs:183`, `consent.rs:29`, `worker.rs:103-104`) and unrelated bound checks in sidecar/validation.rs, i.e. there is no `MIREDS_PER_KELVIN` constant anywhere; finding.md and docs/rework/AUDIT.md have no entry for it.
- why it is a problem: the mired unit is the one place the white-balance models disagree about scale: DNG interpolates in mired, the sliders store mired offsets (basicadj.rs), and the colour model converts per endpoint (cct.rs, temperature_tint.rs). With the conversion factor written as a bare literal, `1e6` and `1e-6` are visually indistinguishable in these expressions and a future change to a stored unit - e.g. storing offsets in kelvin for v2.0.0, which `migrations::white_balance_offsets` is scheduled to allow - must find and change all 10 sites in 3 files consistently; miss one and the as-shot white and every temperature offset keep different units, so colour temperature drifts by a factor of 1e6/1e6-clamped (the `.max(1.0)` and the ±500 mired clamp make it an absurd-looking white rather than a crash). The mired weight interpolation also appears twice with identical structure (`first + (second - first) * weight` at temperature_tint.rs:162 versus `mired_interpolation_weight(cct, first, second)` used at camera_matrix.rs:141), i.e. the same helper exists in one form and one call site re-implements it.
- fix: add `pub const MIREDS_PER_KELVIN: f32 = 1_000_000.0` (units in the doc comment) in the colour/white-balance module, use it at all 10 sites, and reuse the existing `mired_interpolation_weight` helper at temperature_tint.rs:162 instead of re-deriving the blend.
- effort: S
### [GPU-002] The mask-atlas lower edge and the GPU alignment are magic numbers, and the atlas clamp is applied twice
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/builder/surfaces.rs:38-40 and crates/calibraw-gpu/src/pipeline/gpu/resources.rs:73 and :239
- evidence:
  `surfaces.rs:38-40` computes `let mask_atlas_edge = config.mask_atlas_edge_override.unwrap_or_else(|| interactive_mask_atlas_edge(raw.width, raw.height)).clamp(64, export_mask_atlas_edge_limit());`, and `resources.rs:73` defines that very helper as `pub(super) fn interactive_mask_atlas_edge(width: u32, height: u32) -> u32 { mask_atlas_edge().min(width.max(height)).clamp(64, export_mask_atlas_edge_limit()) }`, so the non-override branch is clamped twice and the override branch relies on a second copy of the same unlabelled bounds - `timeout 60 grep -rn --include='*.rs' 'clamp(64,' crates | grep -v 'tests.rs|_tests|/tests/'` returns exactly these two lines, i.e. the minimum atlas edge has no name and lives only inside a clamp chain. Beside it, `resources.rs:239` writes `let padded_row = checked_align_up(row_bytes, 256, "GPU copy row")?;` while the same file defines `const GPU_ALLOCATION_ALIGNMENT_BYTES: u64 = 256;` (:76) and uses it at :221 and in `aligned_buffer_bytes` (:232); `grep -n '256' resources.rs` returns only these three, so the copy-row alignment is the lone un-named copy of a constant that already exists named in the same file.
- why it is a problem: the atlas edge decides the mask raster resolution (mask coverage and feathering resolution) and both places that bound it disagree about where the bound belongs: `interactive_mask_atlas_edge` already guarantees 64..limit, so surfaces.rs' second clamp exists only for the `mask_atlas_edge_override` path - and `mask_atlas_edge_override` is a user/test setting that today is silently clamped, so a caller asking for 8 gets 64 with no report. Raising the minimum (e.g. to 128 to stop upsampled mask mattes) requires finding both spellings; change one and the export path accepts a 64-texel atlas while the interactive path refuses it, which changes mask edge sharpness between the preview and the exported file with nothing logged. For the alignment: `aligned_copy_buffer_bytes` sizes the staging copy of a render result; it uses 256 while `texture_allocation_bytes` (:221) and `aligned_buffer_bytes` use the named constant, so the reservation math and the actual copy are aligned by two definitions that live 160 lines apart. If the allocation alignment is raised (a Vulkan/dXTC or tiled export path requiring 512-byte rows), the reservation stops matching the copy: the copy writes rows padded to 256 against a buffer reserved for 512-byte rows - the render result comes back with a half-row offset in every second line of the exported image, i.e. corrupt export rather than a failure, since both sizes still fit the buffer.
- fix: name the atlas bounds once (`MIN_MASK_ATLAS_EDGE`, and keep `export_mask_atlas_edge_limit()` as the upper) and apply them in exactly one place - a `clamp_mask_atlas_edge(u32) -> u32` used by both the override branch and `interactive_mask_atlas_edge`; and replace `resources.rs:239`'s literal with `GPU_ALLOCATION_ALIGNMENT_BYTES` (or a named `GPU_ROW_ALIGNMENT_BYTES` if the copy row's alignment is meant to be a distinct contract, with the relation to the allocation alignment stated).
- effort: S
### [GPU-003] The 256-bin histogram is re-spelled in three files while a named bin count already exists
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/histogram.rs:5, crates/calibraw-gpu/src/pipeline/gpu/resources.rs:403-408
- evidence:
  `histogram.rs:5-6` starts `const HISTOGRAM_BYTES: u64 = 4 * 256 * std::mem::size_of::<u32>() as u64; const SAMPLE_EDGE: u32 = 256;` and `resources.rs:403-408` re-derives the same buffer size as `let histogram_bytes = u64::try_from(std::mem::size_of::<u32>()).ok().and_then(|word_bytes| word_bytes.checked_mul(256)).ok_or_else(...)` for the `tone histogram buffer` entry - meanwhile `gpu.rs:121` defines `const TONE_HISTOGRAM_BIN_COUNT: u32 = 256;`, used only at `surfaces.rs:371` (`u64::from(TONE_HISTOGRAM_BIN_COUNT) * std::mem::size_of::<u32>() as u64`), which is the allocation the plan entry reserves; `timeout 60 grep -rn --include='*.rs' 'HISTOGRAM' crates/calibraw-gpu/src | grep -v 'tests.rs|_tests'` returns these plus `resources.rs` (nothing uses `TONE_HISTOGRAM_BIN_COUNT` in histogram.rs), and `PreviewHistogramGpu` allocates `HISTOGRAM_BYTES` twice (bins + readback, histogram.rs:102/:110) for the four-channel preview histogram.
- why it is a problem: the 256 here means three different things that must stay equal: bins per channel (`TONE_HISTOGRAM_BIN_COUNT`, matching `[[u32; 256]; 4]` in `PreviewHistogram` and the WGSL bin arrays in preview_histogram.wgsl), and the sampling edge (`SAMPLE_EDGE = 256`, a 256×256 sampling grid, unrelated to bins). Written as identical bare literals in two files, they are indistinguishable: changing the histogram resolution (bins) to 512 must also change `PreviewHistogram.channels`, the WGSL bin arrays and the readback math, and the two bare spellings are what the plan reservation and the preview buffer follow - if `TONE_HISTOGRAM_BIN_COUNT` is raised but `resources.rs:408` is not, the plan reserves space for the old bin count while `surfaces.rs:371` allocates the new one, so the budget check understates GPU memory and, on a tiled export that is already near the process limit, the pipeline is refused by the accounting ("GPU pipelines already reserve ... exceeding the ... MiB process budget") for a reason the message blames on mask capacity and proxy size rather than on the changed histogram; conversely leaving `SAMPLE_EDGE` at 256 while bins grow keeps the preview histogram reading a 256×256 sampling grid against a wider bin table, so the displayed histogram silently aggregates fewer pixels per bin.
- fix: have both sites use `TONE_HISTOGRAM_BIN_COUNT` (and a named `PREVIEW_HISTOGRAM_CHANNELS` for the `4 *` factor, with `HISTOGRAM_BYTES` defined from both) and rename `SAMPLE_EDGE` to something that states it is the sampling grid edge, so no 256 in these files is a bare literal.
- effort: S
### [GPU-004] The stage-uniform allocation is hand-padded with 512 and 256 and its total is asserted as a magic number
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu.rs:112 and crates/calibraw-gpu/src/pipeline/gpu/params.rs:217
- evidence:
  `gpu.rs:110-112` defines `const GPU_STAGE_UNIFORM_SIZE_BYTES: u32 = CAMERA_UNIFORMS_SIZE_BYTES + SCENE_TONE_UNIFORMS_SIZE_BYTES + EFFECTS_UNIFORMS_SIZE_BYTES; const GPU_STAGE_UNIFORM_ALLOCATION_BYTES: u64 = 512 + SCENE_TONE_UNIFORMS_SIZE_BYTES as u64 + 256;` and `params.rs:217` asserts `const _: () = assert!(GPU_STAGE_UNIFORM_SIZE_BYTES == 2_288);` - the three struct sizes are themselves compile-time checked against `std::mem::size_of` (`params.rs:76/:192/:216 assert!(std::mem::size_of::<CameraUniforms>() == CAMERA_UNIFORMS_SIZE_BYTES as usize)` etc.), so 384/1712/192 are enforced, but the `512` and `256` in the allocation expression are not tied to any of them, and `timeout 60 grep -rn --include='*.rs' 'GPU_STAGE_UNIFORM_ALLOCATION_BYTES' crates/calibraw-gpu/src` shows the value is consumed only as `stage_uniform_buffer_bytes:` in the resource plan (`construction.rs:228`, `resources.rs:647`, `resources.rs:1655`), while the real buffers are sized from the structs themselves (`builder/surfaces.rs:343-346 create_initialized_buffer(device, "calibraw camera uniforms", bytemuck::bytes_of(camera), ...)`) - no assert anywhere relates `GPU_STAGE_UNIFORM_ALLOCATION_BYTES` to `GPU_STAGE_UNIFORM_SIZE_BYTES`.
- why it is a problem: the plan's `stage uniform buffers` entry is the figure the memory budget compares against the process limit, and it is currently 2480 because someone wrote 512 (= align_up(384,256)) and 256 (= align_up(192,256)) as paddings that are never derived; the intent is invisible and unenforced. Grow `CameraUniforms` past 512 bytes (adding an effect's parameters to the camera block is a normal edit, and `params.rs:76` forces the author to update `CAMERA_UNIFORMS_SIZE_BYTES`, so the assert passes) and the plan keeps reserving 2480 while the pipeline allocates more: the budget then admits one pipeline too many on a memory-tight device, so the OOM surfaces as a wgpu allocation failure inside an edit - "GPU pipelines already reserve ... exceeding the ... MiB process budget" is not raised and the user sees a hard GPU error mid-edit instead of the graceful "reduce proxy size or mask capacity" message. The `== 2_288` assert has the opposite problem: it passes only while the three hand-maintained sizes happen to sum to that literal, so it reports nothing about the relation it is meant to guard and fails as a confusing compile error ("GPU_STAGE_UNIFORM_SIZE_BYTES == 2_288") on every legitimate uniform addition.
- fix: derive the allocation (`aligned_buffer_bytes(CAMERA_UNIFORMS_SIZE_BYTES) + aligned_buffer_bytes(SCENE_TONE_UNIFORMS_SIZE_BYTES) + aligned_buffer_bytes(EFFECTS_UNIFORMS_SIZE_BYTES)`, or one named `UNIFORM_SLOT_ALIGNMENT_BYTES`) and replace `== 2_288` with `>= GPU_STAGE_UNIFORM_SIZE_BYTES` plus the existing per-struct asserts, so the relation that matters is enforced and neither number is a literal.
- effort: S
### [GPU-005] The two readback paths re-spelled texel sizes and hand-copy padded rows twice
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/readback.rs `read_rgba8_texture_region_blocking` (:41-111), `PendingRgba32Readback::finish_with_context` (:122-200) and `begin_rgba32_texture_region_rgb_readback` (:204)
- evidence:
  the RGBA8 path starts `let (readback, unpadded_bytes_per_row, padded_bytes_per_row) = create_readback_buffer(device, width, height, 4, "RGBA8", label)?;` and then copies rows with `let source = row.checked_mul(padded_row).ok_or_else(...)?; let source_end = source.checked_add(unpadded_row).ok_or_else(...)?; let destination = row.checked_mul(unpadded_row).ok_or_else(...)?; ... destination_slice.copy_from_slice(source_slice);` (:88-105), with the output length computed as `u64::from(width).checked_mul(u64::from(height)).and_then(|value| value.checked_mul(4))` (:70-74); the RGBA32F path does the same thing with its own numbers, `create_readback_buffer(device, width, height, 16, "RGBA32F", label)` (:204), `u64::from(self.width).checked_mul(u64::from(self.height)).and_then(|value| value.checked_mul(3))` (:148-152), `width.checked_mul(16)` for the row, then `for pixel in row_slice.chunks_exact(16) { for channel in 0..3 { let offset = channel * 4; ... } }` (:175-185) - `timeout 30 grep -n 'create_readback_buffer|checked_mul(3)|checked_mul(4)|chunks_exact(16)|, 16,|, 4,' readback.rs` returns these 5 sites (:49, :74, :152, :175/:177, :204, plus :232's `padded_bytes_per_row(width, 16, "RGBA32F")`) with no named texel size anywhere in the file. The crate already owns the mapping: `resources.rs:182 fn texture_format_bytes_per_texel(format: wgpu::TextureFormat) -> Result<u64>` with `Rgba16Float => Ok(8)`, `Rgba32Float => Ok(16)` and a catch-all `Err(anyhow!("unaccounted GPU texture format {format:?}"))`, but it is declared without `pub(super)` (unlike the `pub(super) const` formats around it), so readback.rs cannot reach it and passes bare `4`/`16` instead.
- why it is a problem: the same buffer is read back by two functions that each assert their own texel size and channel count: the plan reserves bytes via `texture_format_bytes_per_texel` (:208), the copy uses the literals, and the "display-linear export readback" also converts RGBA texels to RGB output (`checked_mul(3)` against `chunks_exact(16)`) so the two numbers 16 and 3 encode the working format and the output channel count in one expression each. Change the working format of the display-linear texture (e.g. Rgba32Float → Rgba16Float to halve export memory, which the budget code and SHADER-003-style format changes encourage) and only the plan, the shader and the surface creation follow: the readback keeps copying 16-byte texels from an 8-byte texel buffer, so every second pixel of the exported file is read from the next row's bytes - a colour- and geometry-shifted export with no error, since `get_mapped_range(..)` still yields a legal slice and the values are all finite, so the `{output_label} contains NaN or infinity` guard passes. Conversely, the "unaccounted GPU texture format" error - the project's way of noticing an unmapped format - never fires for the readback path because that path never consults the mapping.
- fix: make `texture_format_bytes_per_texel` `pub(super)` and have both readback entry points take the `wgpu::TextureFormat` (or a named `READBACK_TEXEL_BYTES`/`OUTPUT_CHANNELS` pair derived from one format argument) instead of a bare `u32`, so the size, the row padding and the channel unpacking all come from the same value the reservation uses; and extract one `copy_unpadded_rows(mapped, padded_row, unpadded_row, rows, out)` (or a `decode_rgb_from_rgba_texels`) helper so the two paths differ only by their per-texel decode.
- effort: M
### [GPU-006] The X-Trans Markesteijn passes are selected by hand-written positions 6, 7 and 8
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/builder/passes.rs:25-27 and crates/calibraw-gpu/src/pipeline/gpu/builder/layouts.rs:252-269 and builder/passes.rs:345-356
- evidence:
  `passes.rs:18-27` lists the passes by name, `const XTRANS_MARKESTEIJN_ENTRIES: [&str; 9] = [ "xtrans_markesteijn_green", "xtrans_markesteijn_other", "xtrans_markesteijn_recalculate", "xtrans_markesteijn_other", "xtrans_markesteijn_recalculate", "xtrans_markesteijn_other", "xtrans_markesteijn_homogeneity", "xtrans_markesteijn_accumulate", "xtrans_markesteijn_smooth" ];`, and three lines below it names their positions as bare integers, `pub(in crate::pipeline::gpu) const XTRANS_HOMOGENEITY_PASS: usize = 6; pub(in crate::pipeline::gpu) const XTRANS_ACCUMULATE_PASS: usize = 7; pub(in crate::pipeline::gpu) const XTRANS_SMOOTHING_PASS: usize = 8;` - `builder/passes.rs:345-356` then selects each pass's bind group layout with `let layout = match pass { XTRANS_HOMOGENEITY_PASS => &layouts.bgl_xtrans_homogeneity, XTRANS_ACCUMULATE_PASS => &layouts.bgl_xtrans_accumulate, XTRANS_SMOOTHING_PASS => &layouts.bgl_xtrans_smoothing, _ => &layouts.bgl_xtrans_state };` inside `for (pass, (entry, bind_group)) in XTRANS_MARKESTEIJN_ENTRIES.iter().zip(bind_groups).enumerate()` (i.e. the entry's own name is in scope and is not used), and `layouts.rs:252/:261/:269` reach the same three programs as `plan.demosaic_start_index + XTRANS_HOMOGENEITY_PASS` etc. - `timeout 60 grep -rn --include='*.rs' 'XTRANS_HOMOGENEITY_PASS|XTRANS_ACCUMULATE_PASS|XTRANS_SMOOTHING_PASS' crates/calibraw-gpu/src` returns only these two files, so the position of one pass is stated in two places 400 lines apart, and no test asserts that the constants and the name list agree.
- why it is a problem: the demosaic state machine's last three passes need different bind groups (homogeneity writes the map, accumulate reads it, smoothing uses the plain state), while the first six share `bgl_xtrans_state`, and the distinction is carried by three integers counted by hand over a nine-element list whose entries are already named `"xtrans_markesteijn_homogeneity"`, `"xtrans_markesteijn_accumulate"`, `"xtrans_markesteijn_smooth"`. Markesteijn implementations change by adding passes - an extra `recalculate` iteration, which the list already repeats three times - so inserting one entry in front of the block is a normal edit, and then the constants must be renumbered in two files: forget one, and `xtrans_markesteijn_smooth` runs with the accumulate layout, i.e. the smoothing pass reads the homogeneity map it was never bound to, and the accumulated result is written into whatever the wrong layout points at. On a sampler-bound mismatch wgpu rejects the draw, but the layouts have the same arity here, so the failure is a wrong-but-valid demosaic: X-Trans sensors (Fujifilm bodies) render with the smoothing/accumulation stages swapped, i.e. severe demosaicing artefacts in the photo and in the exported file, with no error and no failing test.
- fix: select by name instead of position, matching on the `entry` string already in scope (or the WGSL entry names as constants in one module shared by both files), and if a position is genuinely needed, derive it with a `const fn` over `XTRANS_MARKESTEIJN_ENTRIES` (e.g. `index_of("xtrans_markesteijn_accumulate")`) so the integers cannot drift from the list, or add a compile-time assert that each constant's entry has the expected name.
- effort: S
### [AI-004] The Remove pass re-implements core's distance transform with a different diagonal weight
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ai/src/models/remove/lama.rs:283-341 `chamfer_distance` (used by `FeatherAlpha::shrunk`'s caller `feather_alpha`, lama.rs:215-236) and crates/calibraw-core/src/pipeline/masks/probability.rs:5-44 `chamfer_distance`
- evidence:
  both do the same two-pass 5-neighbour relaxation over the full rectangle. `lama.rs:283 fn chamfer_distance(region: NativeRect, target: &RemoveMask) -> Vec<f32>` starts `const ORTHOGONAL: u32 = 3; const DIAGONAL: u32 = 4; let mut distance = vec![u32::MAX / 2; width * height];` then relaxes with `let relax = |distance: &mut [u32], index: usize, neighbor: usize, step: u32| { let candidate = distance[neighbor] + step; if candidate < distance[index] { distance[index] = candidate; } };` over forward (`index - 1`, `index - width`, `index - width - 1`, `index - width + 1`) and reverse (`index + 1`, `index + width`, `index + width + 1`, `index + width - 1`) passes and ends `distance.into_iter().map(|value| value as f32 / ORTHOGONAL as f32).collect()`. `probability.rs:5 fn chamfer_distance(binary: &[u8], width: usize, height: usize, target: u8) -> Vec<f32>` does the identical relaxation in float with `const DIAGONAL: f32 = std::f32::consts::SQRT_2;` and the same four neighbour offsets. `timeout 60 grep -rn --include='*.rs' 'fn chamfer_distance' crates | grep -v 'tests.rs|_tests'` returns exactly these two definitions, and `crates/calibraw-ai/Cargo.toml:14 calibraw-core.workspace = true` shows the AI crate already links the crate that owns the transform (the core copy is `pub(super)` inside `pipeline/masks`, so the AI crate cannot currently call it).
- why it is a problem: feather widths are the contract that keeps a mask edge soft in both places: core grows a mask's feather by sampling this distance (probability.rs `shape_probability_mask` `:46-...`), and the Remove patch fades its generated fill over the same distance (lama.rs `feather_alpha` `(1.0 - distance / (feather + 1.0)).clamp(0.0, 1.0)` `:236`), so the alpha that is stored on the patch and the alpha the preview/mask pipeline computes for the same region come from two implementations that call the same step different amounts: 4/3 ≈ 1.333 for a diagonal step versus √2 ≈ 1.414, which is already a 6 % error per diagonal step and grows along the diagonal to a full-pixel offset by about sixteen steps, so on a diagonal Remove stroke the patch's feather no longer matches the mask's feather - the fill shows a harder edge than the edited photo's mask promises, in the exported file. Keeping the approximation only inside one copy also means the two files must be changed together whenever a feather limit is retuned; and the copies differ further in the input (a global `RemoveMask` predicate over a region versus a local `u8` buffer with a target value), so the duplication is not forced by the types - building the region's buffer once and reusing the float version would give one definition and the exact diagonal.
- fix: promote one distance transform in calibraw-core (e.g. `pub` `chamfer_distance` in `pipeline/masks/probability.rs`, documented with the √2 diagonal) and have lama.rs build the region's binary mask then call it, deleting the 3-4 variant; if the integer version was chosen for speed on large crops, keep it as an internal helper of that one function and add a comment stating the accepted error, plus a test comparing both against a brute-force Euclidean distance.
- effort: M
### [AI-005] The automatic runtime download swallows progress and cancellation with do-nothing closures
- category: unnecessary-code
- severity: medium
- confidence: high
- where: crates/calibraw-ai/src/runtime/package.rs:372-378 `ensure_automatic_runtime` (reached from `pub(crate) fn ensure_automatic_onnx_runtime()` at package.rs:332, called by crates/calibraw-ai/src/runtime/load.rs:36 inside `initialize_runtime`), against crates/calibraw-ai/src/registry/artifact.rs:34-44 `ensure_artifact`
- evidence:
  the call is `ensure_artifact(&archive_path, artifact, runtime_download_options(), |_, _| {}, || Ok(()))` with the surrounding context `ensure_artifact(&archive_path, artifact, runtime_download_options(), |_, _| {}, || Ok((),)).with_context(|| format!("download automatic ONNX Runtime for {}", package.platform))?;`, i.e. the last two arguments of `pub(crate) fn ensure_artifact<F, C>(path: &Path, artifact: ModelArtifact, options: DownloadOptions, progress: F, cancellation: C)` (`artifact.rs:34-44`, `F: FnMut(u64, u64)`, `C: FnMut() -> Result<()>`) are literal do-nothing closures. The crate's other caller does the opposite: `registry/install.rs:70 ensure_artifact(path, self.artifact, self.download, |downloaded, total| { progress(ModelDownloadProgress { label: self.progress_label, downloaded, total }); }, cancellation, ...)` - `timeout 60 grep -rn --include='*.rs' 'ensure_artifact(' crates/calibraw-ai/src | grep -v 'tests.rs|_tests'` returns only these two sites, so the no-op pair is the odd one out rather than the convention. The options passed here are `runtime_download_options()` (`package.rs:400-408`): `connect_timeout: Duration::from_secs(45)`, `response_timeout: Duration::from_secs(60)`, `body_timeout: Duration::from_secs(30 * 60)`, `attempts: 5`, `resume: true`, and the artifact is a 11-21 MB package (`bytes: 17_876_357` etc.), and `package.rs:332 pub(crate) fn ensure_automatic_onnx_runtime()` is called from the load path, which the app runs when an AI job starts.
- why it is a problem: the same download the model installer reports to the UI is reported to nobody here: while the runtime archive downloads, the AI job shows no progress (the UI's consent screen already announced the byte count from `AutomaticOnnxRuntimeInfo.download_bytes`, so the user sees a promise that then goes silent), and the returned error's `context("download automatic ONNX Runtime for {}")` is the only signal that anything happened. Worse, `cancellation` being `|| Ok(())` means the 5 attempts × 30-minute body timeout the caller configures cannot be interrupted by the user: quitting, closing the document or cancelling the AI job leaves the thread inside `ensure_artifact` until the network layer gives up, which is up to ~2.5 h of an un-cancelable, un-reported stall before `load.rs:37 .context("prepare the automatic ONNX Runtime")` turns it into an error dialog. The two no-ops also hide that decision from the reader: nothing in this file says the runtime download deliberately reports nothing, so it reads as an unfinished call.
- fix: pass the same kind of progress closure the model installer passes (a label such as the package's `platform` name into the existing `ModelDownloadProgress` reporting path) and the same cancellation check the AI job already has; if a call site genuinely must not report or check, express that with named no-op helpers (e.g. `NoProgress` / `NoCancellation` in `artifact.rs`, documented as "for downloads outside a job") instead of in-place empty closures.
- effort: S
### [AI-006] The selected-runtime size guard is a bare one-MB-to-one-GB range
- category: temp-or-hardcoded
- severity: low
- confidence: high
- where: crates/calibraw-ai/src/runtime/load.rs:48-51 `load`
- evidence:
  `anyhow::ensure!((1_000_000..=1_000_000_000).contains(&metadata.len()), "selected ONNX Runtime has an implausible size of {} bytes", metadata.len());` - `timeout 60 grep -rn --include='*.rs' '1_000_000|1_000_000_000' crates/calibraw-ai/src | grep -v 'tests.rs|_tests'` returns this one site, so neither bound is named or reused, and no comment states where either number comes from. The neighbouring lines in the same function are the guard's companions (`metadata.is_file()` `:42`, the SHA-256 equality check `:56-59` against the hash the user approved), so the range is the weakest of the three checks the app performs before handing the path to `ort::load_*`.
- why it is a problem: the guard exists to reject a file the user pointed at that is not the runtime library, and the two bare numbers say nothing about which library size is expected: the automatic packages are 11-21 MB (`RuntimePackage.bytes` in package.rs), so a one gigabyte upper bound admits a file ~50x bigger than anything CalibRaw installs, and a one megabyte lower bound admits an empty-ish file; the message "implausible size" then tells the user nothing actionable. Because AGENTS.md requires units and contracts to be documented, an un-named range in a check whose purpose is to stop a wrong file from being loaded into the process is exactly the kind of shortcut that hides the decision - a maintainer who has to raise or lower the bound (e.g. when a bundled runtime is a fat static build, or a plugin package grows) cannot tell whether the bound is a measurement, a loose safety margin or a placeholder, and a silently too-generous upper bound is the one bound that lets a hostile file through the other two checks if the hash is somehow bypassed.
- fix: name both bounds next to the guard as `MIN_RUNTIME_BYTES`/`MAX_RUNTIME_BYTES` (or a single `MAX_SELECTED_RUNTIME_BYTES` if only the upper bound is a real limit) with a comment stating the derivation (e.g. "largest automatic package plus margin"), and put the limit in the same module as `RuntimeSource::Manual` so the manual path's contract states it; keep the comparison and message as they are, this is a naming/documentation change.
- effort: S
### [AI-007] Two copies of the same cancellation check each hard-code its own cancel message
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-ai/src/models/denoise.rs:235-240 and crates/calibraw-ai/src/models/remove.rs:114-119, against crates/calibraw-ai/src/jobs.rs:47-53 `JobContext::ensure_not_cancelled`
- evidence:
  the two free helpers are the same four lines with a different literal: `fn ensure_not_cancelled(cancellation: &AtomicBool) -> Result<()> { anyhow::ensure!( !cancellation.load(Ordering::Acquire), "AI denoise cancelled" ); Ok(()) }` and `fn ensure_not_cancelled(cancellation: &AtomicBool) -> Result<()> { anyhow::ensure!( !cancellation.load(Ordering::Acquire), "Remove operation cancelled" ); Ok(()) }`; the crate already owns the parameterised version, `jobs.rs:47 pub(crate) fn ensure_not_cancelled(&self) -> Result<()> { anyhow::ensure!( !self.cancellation.load(Ordering::Acquire), "{}", self.kind.cancelled ); Ok(()) }`, and the messages are JobKind fields, `denoise.rs:85 cancelled: "AI denoise cancelled",` and `remove.rs:46 cancelled: "Remove operation cancelled",` plus `remove.rs:52` repeating the same literal for the second Remove `JobKind`. `timeout 60 grep -rn --include='*.rs' 'fn ensure_not_cancelled' crates | grep -v 'tests.rs|_tests'` returns 4 definitions (the two above, jobs.rs and crates/calibraw-ui/src/services/replay/mod.rs:148, which returns `ReplayError::Cancelled` instead and is a different error type), and the free helpers are reached from `denoise.rs:74/307`-style call sites in models/denoise/inference.rs and models/denoise/result_cache.rs plus `remove.rs:6/43/55/58/251/303`, which pass a bare `&AtomicBool` (`request.cancellation`) rather than a `JobContext`.
- why it is a problem: the user-visible text of a cancelled job is stated twice per feature: the job's `JobKind.cancelled` literal (which `spawn_job` and `JobContext::ensure_not_cancelled` report) and the helper's own literal (which the callbacks that only have the flag report). Renaming a feature in one place - "AI denoise cancelled" becomes "AI denoise cancelled by request", or the Remove job's two JobKinds get the wording one of them lacks - leaves the two paths disagreeing inside one error string the UI shows in the AI dialog, which reads as a wrong job name ("Remove operation cancelled" for a retouch pass) and makes the cancel state harder to describe for the actual feature. The copies also make the contract unclear: nothing states why a call site may not take a `JobContext`, and a third free copy in another crate would compile silently.
- fix: give the free helper the message it needs - `fn ensure_not_cancelled(cancellation: &AtomicBool, kind: &'static JobKind) -> Result<()>` formatting `"{}", kind.cancelled` like `jobs.rs:47` - and delete the literals so each feature states its cancel text once (this also merges the two helpers into one shared in `jobs.rs` since both crates' Remove and denoise call sites already hold the `JobKind` in scope or can take it as an argument); keep `replay/mod.rs:148` separate since it returns `ReplayError::Cancelled`.
- effort: S
### [AI-008] The session-status table swallows a poisoned lock at all three sites, silently
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ai/src/placement/status.rs:53-64 `publish_status`, :67-71 `clear` and :73-77 `ai_session_statuses`
- evidence:
  all three touch the same `static STATUSES: OnceLock<Mutex<BTreeMap<&'static str, AiSessionStatus>>>` (`status.rs:42-45 fn statuses()`), each discarding the `LockFailure`: `if let Ok(mut statuses) = statuses().lock() { statuses.insert(model_name, AiSessionStatus { ... }); }` (no `else`, so the write is dropped), `if let Ok(mut statuses) = statuses().lock() { statuses.clear(); }`, and `pub fn ai_session_statuses() -> Vec<AiSessionStatus> { statuses().lock().map(|statuses| statuses.values().cloned().collect()).unwrap_or_default() }` - the query returns an empty list on poisoning, which is the same value as "no AI session has run yet". The same crate's other global mutex handles the identical failure the opposite way, `crates/calibraw-ai/src/runtime/package.rs:414-418 fn install_lock() -> MutexGuard<'static, ()> { ... LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }` (`timeout 60 grep -rn --include='*.rs' 'lock().unwrap_or_else|if let Ok(mut|\.lock() *$' crates/calibraw-ai/src | grep -v 'tests.rs|_tests'` shows these are the only two styles, and `grep -n 'unwrap_or_default' audit-report.md finding.md` shows this call site is not among the 79 `unwrap_or_default` sites already listed in finding.md).
- why it is a problem: `ai_session_statuses()` feeds the Settings/diagnostics view that tells the user where each model ran - the whole point of the page is the "GPU vs CPU · reason" text `AiSessionStatus::label` builds - and after a poisoned lock the page shows no sessions at all with nothing logged: no `log::warn!` and no `calibraw_core::diagnostics::record` at any of the three sites, so a user whose model placement degraded (e.g. a GPU provider failed and the CPU fallback status never got recorded) gets the wrong answer in the UI, cannot tell it is a fallback rather than "AI never ran", and has no log line to reason from. The dropped write matters more than the dropped read: `publish_status` is the only place the placement is recorded, and the state then stays stale for the rest of the session while every later session reports the stale value. Finally the crate contradicts itself about poisoning - `install_lock()` treats it as recoverable by taking the inner value, this file treats it as "do nothing" - so a reader cannot tell which is intended, and the recovery style is the one that keeps the table correct (the map is never mutated by a panicking thread here: `insert`/`clear` hold no invariants a partial write could break, unlike the runtime install, where a partial download would matter).
- fix: use the crate's existing recovery style at the three sites - a small `fn statuses_guard(&self) -> impl MutexGuard` helper doing `.lock().unwrap_or_else(|poisoned| poisoned.into_inner())` like `install_lock()` - so one poisoning cannot silently drop a status; if any site must stay non-failing, log the discarded failure (`log::warn!`) and record it through `calibraw_core::diagnostics::record` as the other log-only paths in this crate do.
- effort: S
### [AI-009] Every model repeats the same "take the first/one output tensor" ceremony
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ai/src/models/denoise/inference.rs:23-26, crates/calibraw-ai/src/models/subject.rs:196-199, crates/calibraw-ai/src/models/remove/lama.rs:560-563, crates/calibraw-ai/src/models/object/sam.rs:80-83 and sam.rs:127-129
- evidence:
  five sites spell the same extraction. Three take the first output: `let output = outputs .values() .next() .context("RawNIND returned no output tensor")?;`, `let output = outputs .values() .next() .context("BiRefNet returned no output tensors")?;` and `let output = outputs .values() .next() .context("Big-LaMa returned no output tensor")?;`; two take one by position with a formatted message: `let value = outputs .values() .nth(index) .with_context(|| format!("SAM 2.1 returned no {label}"))?;` written twice in sam.rs (:80 and :127, in the same file, with the same variable name `value`). `timeout 60 grep -rn --include='*.rs' -B1 -A2 '\.values()' crates/calibraw-ai/src | grep -v 'tests.rs|_tests'` returns exactly these five plus one unrelated `statuses.values().cloned()` in placement/status.rs:74, and no helper exists: `timeout 60 grep -rn --include='*.rs' 'fn .*output.*tensor|first_output' crates/calibraw-ai/src` finds nothing. The three message literals differ inconsistently - "returned no output tensor" (singular) in two places, "returned no output tensors" (plural) in subject.rs, and the SAM sites name the requested output via `{label}` - and `grep -n 'output tensor' finding.md docs/rework/AUDIT.md` is empty, so this is untracked.
- why it is a problem: this is the ort/TensorRT idiom, not project logic, repeated once per model, and it is the part a maintainer edits most when models change (a new model gets a second output; an existing one is re-exported with a renamed output). The wording drift is already visible in-tree: one model says "output tensor", another "output tensors", so the error dialog a user sees after a failed ONNX run differs by number without meaning, and none of the messages says which output was expected (index or name), which is exactly what a user needs to report when a downloaded model file is wrong. Two of the five sites (`sam.rs:80`, `sam.rs:127`) are in the same file and must stay in sync if SAM 2.1's output order changes.
- fix: add one helper next to the crate's other runtime plumbing (e.g. `fn require_output_tensor(outputs, index: usize, model: &'static str, label: &'static str) -> ort::value::Tensor` or a small `OrtOutputs` wrapper) that formats a single message template - "the {model} model returned no output {label} at position {index}" - and have the five sites call it, keeping each model's expected output list and index visible once.
- effort: S
### [FFI-002] The safe-area insets are four atomics written one at a time and read as a group
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ffi/src/android.rs:123-126 (declarations), crates/calibraw-ffi/src/android.rs:195-202 `system_bar_insets_points` and crates/calibraw-ffi/src/android/callbacks.rs:22-36 `Java_de_duecki_calibraw_CalibRawActivity_nativeOnSystemInsetsChanged`
- evidence:
  the state is four independent cells, `static SYSTEM_INSET_LEFT_PX: AtomicI32 = AtomicI32::new(0); static SYSTEM_INSET_TOP_PX: AtomicI32 = AtomicI32::new(0); static SYSTEM_INSET_RIGHT_PX: AtomicI32 = AtomicI32::new(0); static SYSTEM_INSET_BOTTOM_PX: AtomicI32 = AtomicI32::new(0);`, the JNI callback that Java invokes writes them in four separate stores (`SYSTEM_INSET_LEFT_PX.store(left.max(0), Ordering::Release); SYSTEM_INSET_TOP_PX.store(top.max(0), Ordering::Release); SYSTEM_INSET_RIGHT_PX.store(right.max(0), Ordering::Release); SYSTEM_INSET_BOTTOM_PX.store(bottom.max(0), Ordering::Release); request_repaint();`) and the reader assembles them as one value, `pub fn system_bar_insets_points(pixels_per_point: f32) -> [f32; 4] { ... [ to_points(SYSTEM_INSET_LEFT_PX.load(Ordering::Acquire)), to_points(SYSTEM_INSET_TOP_PX.load(Ordering::Acquire)), to_points(SYSTEM_INSET_RIGHT_PX.load(Ordering::Acquire)), to_points(SYSTEM_INSET_BOTTOM_PX.load(Ordering::Acquire)), ] }`. `timeout 60 grep -rn --include='*.rs' 'SYSTEM_INSET' crates xtask android` shows these are the only sites - no other reader, no setter - and the Java side supplies all four at once, `android/app/src/main/java/de/duecki/calibraw/CalibRawActivity.java:201 nativeOnSystemInsetsChanged(` (inside the insets-changed handler) and `:207 nativeOnSystemInsetsChanged(0, 0, 0, 0);` with the declaration `:253 private static native void nativeOnSystemInsetsChanged(int left, int top, int right, int bottom);`.
- why it is a problem: a single inset is not a coherent value: the four edges are meaningful only together, and the only writer that ever changes them produces a torn read - the UI thread polls `system_bar_insets_points` on its next frame after `request_repaint()` (per the file's own comment, "the UI thread polls them on its next frame"), and a rotation or a status-bar change fires the JNI callback with all four new edges while the reader may sample between any two stores, e.g. the new left and top with the old right and bottom. On rotating portrait→landscape that is exactly the mixed case: the previous landscape's right inset plus the new portrait's left inset, so the toolbar and content region are computed from edges that never existed together - the safe area comes out narrower or off-centre (on a wide notch, an image canvas drawn into a strip), for one frame or, since nothing re-reads until the next Java event, until the next insets change. Nothing detects this: the values are individually legal (each is `max(0)`), so there is no error, no log and no test comparing the four as a set, and the symptom is a one-frame wrong layout on Android only, which is precisely the kind of bug the app's Android checks exist to catch.
- fix: keep one cell for the whole group, e.g. `static SYSTEM_INSETS: Mutex<[i32; 4]>` written once per callback (matching the existing `OnceLock<Mutex<...>>` style used by the other statics in this file), or an index-packed pair of `AtomicI64`s / a single `AtomicPackedInsets` updated with one `store` so the reader's `[f32; 4]` comes from one load; either way state in the accessor that the four edges are read as one snapshot.
- effort: S
### [FFI-003] Java-callback results and progress are queued under `if let Ok` with nothing logged
- category: bad-code
- severity: medium
- confidence: high
- where: crates/calibraw-ffi/src/android.rs:149 `take_queued`, :152-160 `request_repaint`, :165-172 `attach_ui`, :176-180 `detach_ui`, :222 `take_import_progress`, and crates/calibraw-ffi/src/android/callbacks.rs:68, :109, :178, :216, :238, :316
- evidence:
  ten sites in the crate discard the `LockFailure` and no site reports it: the single reader is `fn take_queued<T>(queue: &'static Mutex<VecDeque<T>>) -> Option<T> { queue.lock().ok()?.pop_front() }` (also `IMPORT_PROGRESS.lock().ok()?.take()` at android.rs:222), and the writers inside JNI callbacks are all `if let Ok(mut queue) = results().lock() { queue.push_back(...) }` forms (callbacks.rs:68 `results()`, :109 `queue` (the `EXPORT_RESULTS`/`EXTERNAL_OPEN_RESULTS` local), :178/:216 `camera_profile_folder_results()`, :238 `IMPORT_PROGRESS`, :316 `export_results()`), with `request_repaint` taking the notifier through `.lock().ok().and_then(|installed| installed.clone())`. `timeout 60 grep -rn --include='*.rs' 'lock()\.ok()\|if let Ok' crates/calibraw-ffi/src` lists exactly these sites and `timeout 60 grep -rn --include='*.rs' 'log::warn' crates/calibraw-ffi/src` shows the crate does log elsewhere (android/replay.rs:73, android/sidecars.rs:34/41/172/193/250, android/thumbnail_cache.rs:17/24/31/41) - none of them is one of these ten, so the queue paths are the only failure-prone code in the crate that reports nothing.
- why it is a problem: these ten sites are the whole channel between Java and the UI: a Java callback (file picked, share/open received, export published, camera-profile folder chosen, import progress) stores its answer in the queue and then calls `request_repaint`, and the UI thread's `take_picker_result`/`take_export_publish_result`/`take_import_progress` pick it up on the next frame. If one of those locks poisons, the result is gone: the photo the user just picked is never opened, an export that MediaStore already published is not reported (so the "your export is in this folder" notice never appears and the user cannot find the file), and `request_repaint`'s notifier being `None` means the UI is not woken even once - the app sits waiting for a repaint that nothing will trigger, with no `log::warn!` line and no diagnostics record, i.e. an Android-only hang with an empty log. `attach_ui`/`detach_ui` have the same shape, so a poisoned notifier cell also silently leaves the UI with no notifier, which is indistinguishable from `detach_ui` having run. AGENTS.md requires typed errors where callers distinguish failures and context at boundaries; here the caller cannot distinguish "empty queue" from "queue lost", and the two states look identical to the UI loop.
- fix: recover the guard like the crate's sibling static does (`lock().unwrap_or_else(|poisoned| poisoned.into_inner())` - the VecDeque/HashMap inner values are never left half-updated by a panicking thread, unlike a partial download), which makes the queue paths infallible and removes the silent drop; for `request_repaint`, keep the non-blocking `try_lock` but `log::warn!` the failure (the repaint is a hint, the queue is not), and make `take_queued`'s contract explicit in a doc comment about what poisoning would mean once the guard is recovered.
- effort: S
### [FFI-004] The same five-line Java count clamp is written five times in the callbacks
- category: duplicate
- severity: low
- confidence: high
- where: crates/calibraw-ffi/src/android/callbacks.rs:213, :235, :236, :250, :251
- evidence:
  the identical expression appears at all five sites: `profiles: usize::try_from(profile_count.max(0)).unwrap_or(0),` in `nativeOnCameraProfileFolderPicked`, `completed: usize::try_from(completed_count.max(0)).unwrap_or(0), total: usize::try_from(total_count.max(0)).unwrap_or(0),` in `nativeOnImportProgress` and `imported: usize::try_from(imported_count.max(0)).unwrap_or(0), failed: usize::try_from(failed_count.max(0)).unwrap_or(0),` in `batch_imported_result` - `timeout 60 grep -rn --include='*.rs' 'usize::try_from' crates/calibraw-ffi/src` returns exactly these five, so there is no named conversion in the crate and the neighbouring JNI callbacks each re-spell it (the crate's other int conversions are separate: `android.rs` uses `AtomicI32` loads directly). The file's own helper style exists right beside them (`picked_fd_result`, `batch_imported_result`, `queue_result`), so a conversion helper would match the file.
- why it is a problem: these are the app's Java→Rust count conversions, and the clamp chain carries three decisions (negative → 0, out-of-range → 0, and the `max(0)` that Java's signed `jint` needs) that are invisible in the struct fields they fill: `PickerResult::BatchImported { imported, failed, errors }` is what the UI turns into "N photos imported", so an out-of-range (or poisoned) count reads back as "0 imported" - a success message that reports no photos, in the multi-photo import flow where the photos really were copied into the library, i.e. the user cannot tell that the count is wrong, and `imported: 0` also drives `delete_after_decode`/cleanup decisions in the UI's batch handling. Because the expression is spelled five times, each of the four flows (profile folder, progress, batch, external batch) can be changed alone: raising the clamp style in one place (e.g. switching to `.max(0)` clamping of `usize::MAX`) leaves the other three disagreeing about what a bad count means, and nothing in the file states which meaning that is.
- fix: add one documented helper in this file - `fn java_count_to_size(value: jni::sys::jint) -> usize` with the comment stating the contract (negative means unset, and what an unrepresentable count means, e.g. "report `usize::MAX` so the UI shows a count it cannot have") - and use it at all five sites, so the decision is stated once and each flow's fields visibly come from the same conversion.
- effort: S
### [GPU-007] Seven pipeline textures are kept in `RawGpuPipeline` as underscore-named fields, two of them read
- category: unnecessary-code
- severity: low
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu.rs:457-477 (fields), crates/calibraw-gpu/src/pipeline/gpu/construction.rs:333-348 (initialisers), crates/calibraw-gpu/src/pipeline/gpu/remove_scene.rs:176, :243 (the only Rust reads)
- evidence:
  the struct declares `_reconstructed_raw_texture: wgpu::Texture,` (gpu.rs:457), `_highlight_work_b: wgpu::Texture,` (:465), `_tex1: wgpu::Texture,` (:466), `_tex2: wgpu::Texture,` (:467), `_xtrans_homogeneity: Option<wgpu::Texture>,` (:468), `_tone_guide_a: wgpu::Texture,` (:476), `_tone_guide_b: wgpu::Texture,` (:477) - each with no doc comment, unlike every neighbour (`/// Whether `highlight_work_a` still holds the AI-denoised scene: a RAW` sits above `ai_scene_intact: AtomicBool,` at :464). Grepping each name across `crates/calibraw-gpu/src` (excluding `*tests.rs`) shows `_tex1` and `_tex2` read in production code - `crates/calibraw-gpu/src/pipeline/gpu/remove_scene.rs:176: texture: &self._tex1,` and `:243: texture: &self._tex2,` - while the other five appear only once more, as the initialisers in `construction.rs` (`_highlight_work_b: surfaces.highlight_work_b,`, `_tone_guide_a: surfaces.tone_guide_a,` …), and their real users are the builder's bind groups (`bind_groups.rs:269,271,284,290 texture_binding(24, highlight_work_b_view)`, `:365,414 texture_binding(17, tone_guide_a_view)`), which already keep the resources alive.
- why it is a problem: a leading underscore is Rust's "deliberately unused" spelling, so the compiler will never tell a maintainer if one of these textures stops being bound in a pass - a renamed or dropped binding silently turns into a missing GPU resource and the pass reads whatever the layout slot now holds, which is a wrong-pixel edit rather than a failure. The convention is already contradicted in the same list: `_tex1` and `_tex2` are read in `remove_scene.rs`, so the marker says "unused" for two fields that are not, and `_tex1`/`_tex2` name nothing about which pass scratch they hold, so the reader cannot tell whether a new pass may reuse slot 24/25 or must allocate its own.
- fix: drop the underscore on `_tex1`/`_tex2` (they are read) and give all seven fields the one-line doc comment the neighbouring fields already carry, naming the pass(es) that bind them (`bind_groups` slot numbers included); if a field is only a keep-alive handle, keep it but say so ("bound only through `Pass::bindings`, held here for `readback`"), which is what turns the underscore convention into an actual contract.
- effort: S
### [GPU-008] Every export tone tile uploads the same GPU parameters twice, and the mask buffer is rewritten unconditionally
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu.rs:548-589 (`upload_params`), :839-853 (`dispatch_stage_with_remove`), :920-947 (`accumulate_export_tone_tile_with_remove`), crates/calibraw-gpu/src/pipeline/export/streaming.rs:210 (the per-tile call)
- evidence:
  `gpu.rs:548:    fn upload_params(&self, queue: &wgpu::Queue, params: &GpuParams) {` writes four buffers; the three uniform buffers are guarded by a byte comparison (`if bytemuck::bytes_of(&uploaded.camera) != bytemuck::bytes_of(&camera) { queue.write_buffer(...)` etc.) but the last one is not: `gpu.rs:589:        queue.write_buffer(&self.mask_data_buffer, 0, params.mask_data_bytes());` runs on every call, and the buffer is the wide one - `gpu.rs:114:const MASK_DATA_SIZE_BYTES: u64 = (std::mem::size_of::<MaskData>() * MAX_RENDER_MASK_SLOTS) as u64;` with `MAX_RENDER_MASK_SLOTS = MAX_LOCAL_MASKS * (MAX_EFFECT_COMPONENTS + 1) + MAX_EFFECT_COMPONENTS` (gpu.rs:119, and `MAX_LOCAL_MASKS = 32`, `MAX_EFFECT_COMPONENTS = 12` in `crates/calibraw-core/src/pipeline/masks.rs:49-50`), i.e. one entry per mask layer for every supported mask. The second call is inside the same operation: `gpu.rs:920:    pub fn accumulate_export_tone_tile_with_remove(` starts `self.dispatch_stage_with_remove(queue, device, params, ProcessingStage::Raw, remove)?;` and the next statement is `gpu.rs:928:        self.upload_params(queue, params);`, while `dispatch_stage_with_remove` (:839) already calls `self.dispatch_stage(queue, device, params, stage)`, whose first statement (:650) is `self.upload_params(queue, params);` - the same `params` reference, unchanged between the two calls. This runs once per tile: `streaming.rs:210:            .accumulate_export_tone_tile_with_remove(` sits inside the export tile loop (`tile.global_origin_y` at :198, `tile.core_x/core_y/core_width/core_height` at :204-207).
- why it is a problem: the full export re-uploads the whole mask-slot table once per tile on top of the upload the stage dispatch already did, so a 200-tile export pushes the mask table twice as often as it needs to - bytes that are already in the buffer and already bound - on the same queue that the tile's own passes are submitted to, which lengthens every export's GPU traffic without any state having changed in between. The unconditional write also means the guard style that the file uses for the uniform buffers does not apply to the largest buffer, so an export whose params never change still pays a full mask-table upload per call, and nothing states whether that is required (the shader may read mask data written by an earlier pass) or just defensive.
- fix: have `accumulate_export_tone_tile_with_remove` reuse the upload that `dispatch_stage_with_remove` performed (e.g. split `dispatch_stage` into `upload_params` + `encode_stage`, and let the tile helper call `upload_params` once then `encode_stage(Raw)`), and extend the existing compare-before-write style to the mask buffer (`if bytemuck::bytes_of(&uploaded.mask_data) != params.mask_data_bytes()`, or keep the unconditional write with a comment saying the mask table must always be re-pushed because mask effects write into it mid-stage).
- effort: M
### [GPU-009] The passes that run after Blur live outside the pass list and are selected by four hand-written `if blur_active` branches
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu.rs:447-451 (fields), :1079-1153 (branches in `encode_output_stage`), crates/calibraw-gpu/src/pipeline/gpu/builder/passes.rs:226-230, :705-764
- evidence:
  `gpu.rs:1079:        let blur_active = params.needs_blur_passes();` is followed by four copies of the same shape: `if blur_active {` at :1100 (Glow), :1118 (Pixelate), :1129 (scene-depth guide + creative) and :1149 (render), each taking a dedicated struct field in the blur branch and an index into `self.passes` in the other branch (`&self.post_blur_pixelate_blocks_pass,` with the label `"post-Blur Pixelate block pass"` against `self.indices.pixelate_blocks_pass_index`). Those five passes are fields, not list entries - `gpu.rs:447-451: post_blur_glow_passes: Vec<Pass>, post_blur_pixelate_blocks_pass: Pass, post_blur_scene_depth_guide_pass: Pass, post_blur_creative_pass: Pass, post_blur_render_pass: Pass` - and the same five names are spelled again in `builder/passes.rs:226-230` (struct fields), `:705-740` (construction, one `let post_blur_* = Pass {` per pass) and `:760-764` (return tuple), plus `construction.rs:278-282` (struct literal), so every one of these passes is named in four files. Meanwhile `encode_pass`/`encode_pass_range` address the other passes only through `self.indices` (`self.indices.glow_prepare_pass_index`, `self.indices.pixelate_blocks_pass_index`), which is the mechanism every other stage in the file uses. `timeout 30 grep -n 'blur_active' crates/calibraw-gpu/src/pipeline/gpu.rs` returns exactly these six sites, i.e. the decision is not encoded anywhere else, and nothing in `layout_contract_tests` ties the four branches to the pass list.
- why it is a problem: the post-blur variants are the only GPU passes whose order is re-decided in Rust at dispatch time, so the one contract the rest of the pipeline states declaratively (`StageIndices`: where a stage's passes start and end) has to be worked around for these five, and the "which variant runs when Blur is on" rule exists only as four hand-copied branches plus five field names. Adding a blur-sensitive effect (or reordering one, e.g. moving Glow behind Pixelate for the blur path) means editing four branches in a 75-line ladder and re-naming a field in four files, and a branch that is forgotten makes that effect run its pre-blur variant after the blur passes - a subtly wrong edit result that still passes every layout and bind-group check, because only the branch order in this function changed.
- fix: add the post-blur passes to the same pass plan with a marker for "runs after the blur passes" (the plan already assigns indices) and have `encode_output_stage` iterate the plan, deriving the label from the pass entry as `encode_pass` already does (`self.encode_bound_pass(encoder, pass, &pass.pipeline.entry)`); that removes the five fields, their labels in `passes.rs`, and the four duplicated branches at once, leaving one place that owns post-blur order.
- effort: M
### [SHADER-006] Pi is spelled five times inline and three times as consts, at three different precisions
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/shaders/basic_adjustments.wgsl:70, crates/calibraw-gpu/src/shaders/view_transform.wgsl:179, :217, :334, :350, and the consts crates/calibraw-gpu/src/shaders/mask_effects/tilt_shift.wgsl:2, mask_effects/lens_blur.wgsl:2, mask_effects/light_rays.wgsl:1
- evidence:
  the hue math repeats a truncated literal inline: `basic_adjustments.wgsl:70:    let hue = fract(atan2(lab.z, lab.y) / (2.0 * 3.14159265359) + 1.0);`, `view_transform.wgsl:179:    let angle = atan2(lab.z, lab.y) + rotation * 3.14159265359 / 180.0;`, `:217:    let angle = wheel.x * 2.0 * 3.14159265359;`, `:334:    let selector_hue = fract(atan2(sample.hue_vector.y, sample.hue_vector.x) / (2.0 * 3.14159265359) + 1.0);`, `:350:        let target_angle = center_angle + hue_shift * 2.0 * 3.14159265359;`. The same value is then named three different ways at three precisions: `const MASK_TILT_SHIFT_PI: f32 = 3.14159265;`, `const MASK_LENS_BLUR_PI: f32 = 3.14159265;` and `const LIGHT_RAY_PI: f32 = 3.141592653589793;` - `timeout 30 grep -rno '3\.14159265359\|3\.14159265\b\|3\.141592653589793' crates/calibraw-gpu/src/shaders | sort | uniq -c` returns exactly these eight lines, i.e. eight sites and no single definition, and nothing in `common.wgsl` holds it (the file already exports `Common::LUMA`, `Common::clamp_pos`, `Common::MAX_RENDER_MASK_SLOTS`, which these same modules consume as `Common::LUMA`).
- why it is a problem: the truncated copy is not the full-precision copy - `3.14159265359` differs from `3.141592653589793` by ~1e-12, and these are used as full turns in `fract(atan2(...) / (2.0 * pi) + 1.0)` hue wraps, so the hue circle closes at a slightly different angle than the one Light Rays rotates by. Today that difference is invisible in f32, so the discrepancy is a latent one: it only shows once a shader composes two of these expressions (a wrap-then-rotate chain), where the mismatch becomes an accumulated angular offset instead of a rounding artefact, and a reviewer comparing `MASK_LENS_BLUR_PI` (8 digits, itself below f32 precision) with `LIGHT_RAY_PI` cannot tell which is authoritative or which site was edited. With three consts already declared, a fourth spelling in a new shader keeps the count growing while the "one true value" for hue turns is never written down.
- fix: declare the constant once in the module every shader already imports (`common.wgsl`, e.g. `const Common::HUE_FULL_TURNS: f32 = ...` next to `LUMA`) with a comment stating that hue angles in this crate are turns, and use it in all five inline sites and the three per-shader consts, so the two hue-wrap expressions and the Light Rays rotation read from the same declaration.
- effort: S
### [GPU-010] A field named `mask_counts` carries four different packed values, spelled again on the WGSL side
- category: temp-or-hardcoded
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/params.rs:177 (declaration), :569-600 (`with_mask_uv_rect`, `with_mask_uv_rect_and_extent`, `set_mask_uv_rect`), crates/calibraw-gpu/src/shaders/scene_adjustments.wgsl:36-61
- evidence:
  the uniform field is `pub(super) mask_counts: [u32; 4],` (params.rs:177) with no comment, and the four slots are written by index with three different meanings: `params.rs:571:        self.scene_tone.mask_counts[3] = u32::MAX;` (sentinel), `params.rs:583:        self.scene_tone.mask_counts[3] = width | (height << 16);` (two u16s), and `params.rs:597:        self.scene_tone.mask_counts[1] = pack(min_u, min_v);` / `:598: self.scene_tone.mask_counts[2] = pack(max_u, max_v);` where `set_mask_uv_rect`'s local `pack` is `let u = (u.clamp(0.0, 1.0) * 65_535.0).round() as u32;` - while slot 0 means something else entirely (`params.rs:311:        let count = (self.scene_tone.mask_counts[0] as usize).min(MAX_RENDER_MASK_SLOTS);`). The WGSL side re-spells the same packing: `scene_adjustments.wgsl:39:    let packed_min = Common::scene_tone_uniforms.mask_counts.y;`, `:58:    if Common::scene_tone_uniforms.mask_counts.w != 0xffffffffu {`, `:60: Common::scene_tone_uniforms.mask_counts.w & 65535u,`, `:61: Common::scene_tone_uniforms.mask_counts.w >> 16u,` and `:44:    ) / 65535.0;`. `timeout 60 grep -rn --include='*.rs' 'mask_counts' crates/calibraw-gpu/src | grep -v 'tests.rs|_tests'` returns only these Rust sites, and `grep -rn '65_535\|65535' crates` shows the u16 scale repeated at `export/rows.rs:106`, `export/color.rs:88`, `gpu/construction.rs:119` and in the shader, i.e. no name exists for it; `layout_contract_tests.rs:251` only lists `mask_counts,` as a field name, so nothing checks that the two packings agree. Four call sites feed this through `with_mask_uv_rect_and_extent` (`export/crop.rs:230`, `export/streaming.rs:302`, `calibraw-ui/src/app/processing_export/preview/state.rs:250`).
- why it is a problem: the field's name advertises "counts", so the reader has to know - only from reading both languages - that slots 1/2 are UV rectangles and slot 3 is an atlas extent or a magic `u32::MAX`/"not supplied" marker; the marker is `u32::MAX` in Rust and `0xffffffffu` in WGSL, so the two spellings are the thing that has to stay in step, and an extent whose packed width or height reaches 65 535 is indistinguishable from the sentinel (the Rust side clamps to `u16::MAX`, which allows exactly that value). The u16 scale `65_535.0` is written in five Rust sites and in the shader, so a change to the sub-image encoding (or a future switch to a 10-bit packing) has to be repeated in two languages with no compile-time check in between: mismatching just the sentinel or the shift leaves the shader treating a real extent as "no mask crop" and the whole mask atlas samples the wrong region, which is a wrong-pixel export rather than a failure.
- fix: name the packing once on each side and make them share it: a `mask_uv_rect` / `mask_atlas_extent` accessor pair in `params.rs` (or a `[u32; 4]` writer that takes a `MaskAtlasInfo` struct with `min_uv`, `max_uv`, `extent`, `absent` fields and does the shifts in one documented place), a named `const MASK_UV_SCALE = 65_535` and `const MASK_EXTENT_ABSENT` in `common.wgsl` used by `scene_adjustments.wgsl`, and a `layout_contract_tests` case asserting `decode(encode(rect)) == rect` plus `extent != absent` for the maximum extent, which is where the `u16::MAX` collision becomes a compile-time failure instead of a silent one.
- effort: M
### [GPU-011] The predicate that decides whether the intermediate passes run hand-compares fourteen effect ids, re-implementing the crate's own `applies` helper
- category: duplicate
- severity: medium
- confidence: high
- where: crates/calibraw-gpu/src/pipeline/gpu/params.rs:656-669 (inside `needs_intermediate_adjustment_passes`), same file :739-744, :798-806, :265 (`applies`)
- evidence:
  the run decision spells every effect name twice - once as a type, once as an id comparison: `if effect_id == MaskEffect::Neon.shader_id()` (`:656`) chained with `|| effect_id == MaskEffect::Glow.shader_id()` (:657), `LightRays` (:658), `Relight` (:659), `Blur` (:660), `LensBlur` (:661), `MotionBlur` (:662), `RadialBlur` (:663), `TiltShift` (:664), `EdgeGlow` (:665), `Pixelate` (:666), `Fog` (:667), `Smoke` (:668), `Halation` (:669) - inside `self.mask_slots().iter().any(|local| ... )`. The same file already owns the helper that does exactly this: `params.rs:265` (`pub(super) fn applies(&self, effect: MaskEffect) -> bool { self.is_active() && self.effect_id() == effect.shader_id() }`) and the neighbouring predicates use it with arrays - `params.rs:744:            [MaskEffect::Fog, MaskEffect::Smoke, MaskEffect::Relight]` with `.into_iter().any(|effect| mask.applies(effect))`, and `params.rs:798:    pub(super) fn needs_blur_passes(&self) -> bool {` taking a five-entry array in the same style - so two conventions for the same question live 100 lines apart. `timeout 30 grep -n 'effect_id == MaskEffect::' crates/calibraw-gpu/src/pipeline/gpu/params.rs` returns only these fourteen lines, i.e. this is the only place the id comparison is expanded by hand; core's completeness test (`crates/calibraw-core/src/pipeline/masks/effects/tests.rs:29: assert_eq!(MaskEffect::ALL.len(), effects.len());`) keeps `MaskEffect::ALL` in sync with the enum but nothing ties this chain to it.
- why it is a problem: the chain is the app's "does this edit need the scene-neighbourhood passes" gate, and it is the only pass-run predicate in this crate that names effects one by one, so a `MaskEffect` added to `MaskEffect::ALL` (which core's own test enforces) is invisible to this gate until someone remembers to add a fifteenth line: the edit then reports "nothing after the tone pass is needed", the intermediate passes are skipped and the new effect is dropped from the rendered photo - an edit that silently does nothing, with no compile error, no layout test failure and no log entry. The hand-expansion also loses the `is_active()` half of `applies` (see `params.rs:265`), so an inactive slot's id still counts here, which the file's other predicates do not do; the two halves of the same helper disagree on the same condition, and `is_active()` also matters for the gate's cost (the local-value scan at :674 already re-derives slot state by hand: `local.adjust_0[1..].iter().any(|value| value.abs() > 1e-6)` etc.).
- fix: replace the chain with the crate's own array idiom - `const SCENE_NEIGHBOURHOOD_EFFECTS: [&MaskEffect; 14] = [MaskEffect::Neon, ..., MaskEffect::Halation];` next to `MaskEffect::ALL` in core, and here `SCENE_NEIGHBOURHOOD_EFFECTS.into_iter().any(|effect| mask.applies(effect))` - then assert in `layout_contract_tests` that the list equals `MaskEffect::ALL` minus the documented post-display effects (`Grain`, `Vignette`, `Adjustment`), so the set that needs the passes is stated once and a new effect either appears in it or fails that assertion at compile time.
- effort: M

---

## Closing summary (Working version v1.3.1, appended after the second sweep)

### Top 15
Ranked by `severity x confidence / effort` (high=3, medium=2, low=1; effort S=1, M=2, L=3). Forty-two entries share the top score 6.00, so ties are broken by the number of affected sites named in the entry, then by ID order.

| # | ID | sev/conf/eff | finding |
|---|----|--------------|---------|
| 1 | UI-047 | medium/high/S | repaint quantum and idle keep-alive are bare 16 ms / 50 ms literals at 11 sites |
| 2 | CORE-051 | medium/high/S | `adaptive_detail_defaults` maps noise to the denoise sliders with a line of unlabelled tuning constants (8 sites) |
| 3 | SHADER-006 | medium/high/S | Pi spelled five times inline and three times as consts, at three precisions |
| 4 | AI-005 | medium/high/S | the automatic runtime download swallows progress and cancellation with do-nothing closures (4 sites) |
| 5 | UI-022 | medium/high/S | "is the preview at fit zoom" spelled three ways, one with a different tolerance |
| 6 | GPU-006 | medium/high/S | the X-Trans Markesteijn passes are selected by hand-written positions 6, 7 and 8 |
| 7 | AI-007 | medium/high/S | two copies of the same cancellation check each hard-code their own cancel message |
| 8 | FFI-002 | medium/high/S | the safe-area insets are four atomics written one at a time and read as a group |
| 9 | UI-006 | medium/high/S | the same five-line "selected component is not initialized" guard written four times |
| 10 | UI-008 | medium/high/S | four copies of the same "batch error or clear task, then report" sequence in one export worker |
| 11 | UI-007 | medium/high/S | the same "install a loading thumbnail" block written four times in cache.rs |
| 12 | CORE-052 | medium/high/S | as-shot white balance falls back to a bare D65 and clamps to a range no other reader uses |
| 13 | CORE-001 | medium/high/S | the 32 768-pixel edge limit is a bare literal in the sidecar validators |
| 14 | GPU-002 | medium/high/S | the mask-atlas lower edge and the GPU alignment are magic numbers, and the atlas clamp is applied twice |
| 15 | UI-003 | medium/high/S | lens-correction notice passed as an out-parameter that one call site discards with `&mut None` |

The four highest-severity entries score 4.50 (high/high/M) and therefore fall below the 6.00 tier in this formula; by consequence they are the most important findings in the report: CORE-006 (a failed DCP profile-folder search is logged and dropped as "no profiles"), UI-010 (three startup paths turn a resolution failure into `None` behind `map_err(log).ok()`), UI-056 (a settings file the app cannot read silently resets every preference), CORE-030 (the sidecar size estimate and the measurement walk the mask stack twice).

### Systemic patterns (5)
1. **Per-stage timing instrumentation hand-copied everywhere.** CORE-007, CORE-039 (these two entries describe the same issue; see the note below) and the sites quoted in CORE-007's `where` (`libraw_loader.rs`, `document_load.rs`, `export/streaming.rs`, `placement/session.rs`). ~126 `diagnostics::record` calls in 37 production files, ~57 hand-written `Instant::now()` clocks, no scoped timer in `core/src/diagnostics.rs`.
2. **Loose positional argument packs and hand-written field comparisons.** CORE-005, CORE-009, CORE-011, CORE-028, CORE-029, CORE-041, CORE-046, CORE-049, GPU-001, GPU-010, AI-002, UI-014, UI-044, UI-048 - `x, y, width, height` as four `u32`s, six-argument curve calls repeated three times, layer/atlases packs, and 14-field `!=` chains where a derived `PartialEq` exists.
3. **Numeric limits, weights and budgets duplicated across Rust, WGSL, xtask and scripts.** CORE-001, CORE-010, CORE-034, CORE-042, CORE-047, CORE-048, CORE-051, CORE-052, CORE-053, GPU-002, GPU-003, GPU-004, GPU-010, SHADER-003, SHADER-006, UI-016, UI-019, UI-024, UI-033, UI-034, UI-040, UI-047, UI-052, UI-058, CLI-002, XTASK-001, XTASK-014, SCRIPT-005.
4. **Failures swallowed, poisoned, or visible only in the logs.** CORE-006, CORE-012, CORE-023, CORE-032, AI-005, AI-007, AI-008, UI-010, UI-011, UI-056, FFI-003, UI-009 (unnamed 2 s deadline in a sleep loop).
5. **The same algorithm re-implemented per crate or per module.** CORE-003 (bilinear block in three crates), CORE-016 / AI-004 (distance transform, different diagonal weight), CORE-049, GPU-005, GPU-009, SHADER-001, SHADER-002, SHADER-004, UI-057, UI-059.

### Not checked
The run did not maintain the per-file chunk log the protocol asked for, so coverage below is reconstructed from the report: 164 entries were recorded across 522 production files (all `.rs` under `crates/` and `xtask/` minus tests, benches, generated and migration files; 52 `.wgsl`; 12 root build/tooling files and `scripts/`). 273 files appear in at least one entry; 249 do not, and for those the only evidence of inspection is the systemic greps, not a read.

- `crates/calibraw-ui`: 94 of 210 production files appear in no finding. Largest untouched: `ui/components/depth_range_slider.rs` (723), `ui/components/feathered_range.rs` (657), `ui/library/view.rs` (624), `app/develop_state.rs` (570), `ui/library/dialogs.rs` (557), `app/ai/generated.rs` (543), `ui/preview/tools/effect_handles/band.rs` (521), `app/preview_visibility.rs` (494), `app.rs` (474), `ui/library/state/scan.rs` (471), `ui/library/catalog.rs` (464), `app/mask_strip.rs` (456) plus 82 smaller ones.
- `crates/calibraw-core`: 64 of 125 appear in no finding. Largest untouched: `pipeline/alignment.rs` (991), `pipeline/color_profile.rs` (662), `pipeline/raw_loader/libraw_loader/sensor.rs` (606), `pipeline/raw_loader/libraw_loader/thumbnail.rs` (518), `pipeline/lensfun/backend/matching.rs` (485), `pipeline/geometry/straighten/segments.rs` (449), `pipeline/focus_stack.rs` (433), `pipeline/hdr.rs` (431), `pipeline/processing/tiles.rs` (418), `pipeline/masks/raster_cache.rs` (411), `build.rs` (423) plus 53 smaller ones.
- `crates/calibraw-gpu`: opened in this run only `gpu.rs`, `params.rs`, `mask_params.rs`, `mask_layers.rs`, `gpu/readback.rs`, `gpu.rs` (partly), `export/resize.rs`, `export/streaming.rs`, `gpu/scene_surface.rs`, `gpu/effect_inputs.rs`, `effect_lanes.rs`, `resources.rs` (lines 1-170). Not opened: `device.rs`, `gpu_errors.rs`, `memory/{dxgi,metal,vulkan}.rs`, `export/{destination,encoders,metadata,tiff}.rs`, `focus_stack.rs`, `hdr.rs`, `raw_merge.rs`, `stage_timing.rs`, `gpu_cache.rs`, `shader_manager.rs`, `shaders.rs`, `builder.rs`, `builder/bind_groups.rs`, `clipping.rs`, `direct_shader.rs`, `effect_lanes.rs` (only as cited), `scene_depth.rs`, `scene_surface.rs` (171-681), `effect_inputs.rs` (171-675), `stage_params.rs`, `wait.rs`, `surfaces.rs`, `construction.rs`, `histogram.rs` and `resources.rs` (171-1875) - 28 files, plus 35 of 52 `.wgsl` shaders never opened: `cfa_linear, chromatic_aberration, color, color_denoise, creative_effects, detail_capture, detail_scale_space, detail_utils, dual_demosaic, dual_demosaic_detail, highlights, atmosphere, blur, edge_glow, motion_blur, neon, pixelate, radial_blur, relight, scene_depth, scene_lights, shared, noise, pass1, pass2, pass3, preview_clipping, preview_histogram, profile, raster_finish, raw_sampling, remove_composite, scene_source, tone_common, tonemap, xtrans_demosaic` (module names).
- `scripts/` and root build files: 12 total; `about.toml`, `deny.toml`, `rust-toolchain.toml`, `scripts/flathub-sync.sh`, `scripts/flatpak-run.sh`, `scripts/generate_licenses.sh` appear in no finding and were grep-scanned only.
- `xtask`: all production files were opened file-by-file (`android_bench/{mod,metrics,report,scenario,adb}.rs`, `jni_contract/{rust_side,java_side}.rs`, `colorchecker.rs`, `error.rs`, `baseline.rs`, `build.rs`, `arch.rs`, `cfg_ratchet.rs`, `migrations.rs`, `loc.rs`, `ui_lint.rs`, `main.rs`, `android.rs`, `compare.rs` and the remaining side files); 14 findings recorded.
- `calibraw-ai` (42 files), `calibraw-ffi` (9), `calibraw-cli` (3): every production file appears in at least one entry, but coverage came from targeted greps and the files behind each finding, not a full file-by-file sweep; AI-004, AI-005, AI-006, AI-008 and FFI-002..FFI-004 name sites read directly, the rest were located by grep.

### Report defects and internal overlaps (disclosed, entries above were left unchanged)
- **Numbering gaps:** `UI-021` and `CORE-033` were never appended, so the report holds 59 UI entries numbered to UI-060 and 52 CORE entries numbered to CORE-053. No crate hit its 60-finding cap (UI is at 59, CORE at 52).
- **Six entries duplicate or heavily overlap an earlier one**, so the 164 entries are 158 distinct issues: CORE-009 repeats CORE-005 (same `raw_controls_changed`), CORE-039 repeats CORE-007 (same timing ritual), CORE-031 repeats CORE-013 (same `validate_edit_state` dab pair), CORE-044 repeats CORE-019 (same `normalize_group`/`validate_group_name` pair), CORE-032 repeats CORE-012 (same `is_raster_tiff`), and UI-047 repeats UI-040 (six of the same files). Related-but-distinct pairs kept separate: CORE-047 vs SHADER-003 (same weights, Rust vs WGSL), UI-050 vs UI-022 (fit-zoom vs zoomed-in predicates), CORE-043 vs UI-002 (same import rule, different crates), CORE-016 vs AI-004, CORE-028 vs CORE-046, UI-013 vs UI-051, CORE-040 vs AI-005.
- **CORE-006 contains one wrong sub-claim.** Its `why it is a problem` says "the `if let Some(folder)` guard above it means the log line is skipped entirely when `profile_folder` is `None`". That is not what the code does: the chain is `profile_folder.map(...).transpose().unwrap_or_else(...)`, so when `profile_folder` is `None` the closure never runs and there is no error to log; the `if let Some(folder)` guard is defensive, not load-bearing. The main claim still holds: an unreadable or renamed DCP folder yields the empty profile list with only a `log::warn!` and no reported error.
