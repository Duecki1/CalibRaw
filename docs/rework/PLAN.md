# CalibRaw repository rework plan

Status: in progress. Baselines are recorded in [BASELINE.md](BASELINE.md); progress in [COVERAGE.md](COVERAGE.md).

## 1. Objective

Rework CalibRaw and its shared Moduwu design library into readable, reusable, maintainable Rust, WGSL, Android Java and build tooling while preserving every supported feature, persisted format, interaction, platform behavior and processing capability.

Reduce code by consolidating duplicate implementations and repeated configuration. Moving code between files or repositories improves ownership but does not count as a reduction in the combined codebase.

The UI migration is one workstream within this repository-wide rework.

### Success criteria

- Existing features remain available on their supported platforms.
- Existing settings, presets, sidecars and edit history retain their compatibility contracts.
- Numerical changes stay within declared tolerances; intentional image changes are separate work.
- Performance, memory use and responsiveness remain within recorded budgets.
- The CLI and UI reuse processing services wherever their behavior is equivalent.
- A standalone headless CLI build does not require egui or egui-wgpu.
- Shared widgets and tokens have clear ownership in Moduwu; specialized photo controls remain in CalibRaw.
- Rust/WGSL buffer layouts, bindings and shared IDs are verified automatically.
- Rust/Java JNI exports and Java `native` declarations are verified automatically.
- Shared controls keep or improve their accessibility information; extraction never loses it.
- Production duplication decreases across CalibRaw and Moduwu together.
- Tests remain useful and readable; they are not removed to meet a line-count target.

Do not set an arbitrary percentage reduction before auditing duplication. Estimate savings per candidate, then measure the actual result.

## 2. Surveyed starting point

The surveyed source inventory contains 130,883 lines of Rust and 7,643 lines of WGSL in CalibRaw. These totals include tests and build scripts and exclude the sibling Moduwu repository. Recount from the selected baseline commit rather than treating these figures as permanent targets.

Established foundations to extend:

- Separate core, GPU, AI, UI, CLI and Android integration crates.
- Moduwu themes, metrics, buttons, forms, frames, menus and responsive helpers.
- Shared effect parameter specifications, including `FloatParamSpec`.
- Naga-based shader composition and validation.
- Rust GPU size assertions and packing regression tests.
- Existing worker helpers and AI model-runtime management.
- CPU, GPU, serialization and UI interaction regression tests.
- A visual-review harness with four themes, three viewport sizes and eleven states: 132 captures.
- RAW-open and mask-rasterization performance tools.

Concrete coupling and maintenance candidates:

- `calibraw-gpu` depends on and re-exports egui and egui-wgpu. GPU pipelines own egui texture IDs and participate in renderer registration and cleanup.
- The CLI consequently pulls UI dependencies through the GPU crate.
- A production remove-composite shader is embedded in `gpu.rs` as a Rust string.
- Replay planning, rendering and video writing live under the UI crate. The worker uses egui to request repaints, while application startup and polling depend on `CalibRawApp`.
- Numeric fields and presentation patterns are repeated across UI areas.
- Large modules exist in RAW loading, GPU construction, export, masks, replay, AI and Android integration. Size alone does not establish duplication.

Other contracts in scope:

- The Android app has about 3,600 lines of Java and 8 `Java_de_duecki_calibraw_*` JNI exports. Signature mismatches can escape Rust compilation and surface at runtime.
- Desktop eframe is built with `accesskit`, and `calibraw-ui` sets `widget_info` explicitly in only one place. Built-in egui controls also supply accessibility information and actions; the explicit call count does not measure accessibility coverage.
- The Android eframe dependency does not enable `accesskit`, so TalkBack receives no information from the app today. Accessibility preservation applies to desktop; Android accessibility would be a new feature and is out of scope for this rework.
- 104 Rust files contain inline `#[cfg(test)]` modules, so ordinary line counters mix tests into production counts.
- No public API currently has doc examples, and `cargo test --all-targets` does not run doc tests.
- Java unit tests exist (`AndroidStorageContractTest`, `ReplayVideoEncoderTest`), but CI only assembles the APK and does not run them.
- Build tooling is maintained project logic: `build.rs` in core, GPU and UI; `xtask`; Gradle; CMake; `scripts/*.sh` and `scripts/bootstrap_download.py`; and the CI/release workflows. Example duplication: the root `settings.gradle` and `android/settings.gradle` are identical except for `rootProject.projectDir`.

Record the branch and working-tree state at Step 1 rather than relying on a snapshot in this plan.

## 3. Working principles

1. Preserve observable behavior during structural extraction. Redesigns, algorithm changes and new features use separate changes.
2. Each extraction must remove duplication, enable a real additional caller, or establish useful ownership. Record which benefit it provides.
3. Prefer small functions and concrete types. Introduce traits only for actual alternative implementations or meaningful test boundaries.
4. Reuse existing helpers before introducing another abstraction.
5. Share equivalent behavior, not merely similar-looking syntax. Document intentional differences.
6. Keep public APIs small. Prefer private fields when construction or mutation must preserve invariants.
7. Split files by responsibility. Approximately 600 lines is a review signal, not a hard limit or acceptance metric.
8. Keep preparation proportionate. Use short working tables and existing fixtures; add coverage only for material gaps.
9. Keep migrations focused and reviewable. Avoid combining broad formatting, file movement, API changes and numerical changes in one PR.
10. Measure combined production code separately from tests and generated code. Inline `#[cfg(test)]` modules count as tests.

## 4. Target ownership and dependencies

Document the actual dependency graph and the intended boundaries in `docs/ARCHITECTURE.md`.

| Area | Owns | Boundary |
|---|---|---|
| `calibraw-core` | Image/edit models, colour mathematics, geometry, parameter validation, loading and persistence primitives | No UI dependency; no dependency on app coordination |
| `calibraw-gpu` | GPU resource planning, shader composition, execution, processing output and readback | Depends on core; headless processing does not require egui |
| `calibraw-ai` | Model artifacts, inference, model-specific preprocessing/postprocessing and execution-provider fallback | Uses core and GPU only where needed; no UI dependency |
| Application services | Document operations, export/replay orchestration, history, jobs and cancellation | Reusable by actual callers; presentation-independent APIs |
| `calibraw-ui` | Views, interaction state, preview presentation, commands and application coordination | Calls services and owns presentation integration |
| Platform adapters | Android content URIs/JNI, desktop filesystem and codec integrations | Expose explicit platform behavior behind narrow interfaces |
| `calibraw-cli` | Arguments, diagnostics and headless invocation | Calls shared services instead of reproducing UI workflows |
| `moduwu-design` | Reusable tokens, controls and presentation helpers | No CalibRaw business logic or photo-processing dependency |

Application services may initially be modules. Add a crate when callers and dependency ownership justify it. Do not create a general-purpose framework or a new crate for every responsibility.

Platform bridges may depend on domain types where necessary; they must not depend on the concrete application or create dependency cycles. Record existing exceptions and migrate them explicitly. In particular, inspect Android FFI's egui dependency when assessing platform-wide decoupling.

Separate the headless processing engine from egui texture registration. GPU image processing and a visual preview are different responsibilities even when both use the same output texture.

### Enforced boundaries

`docs/ARCHITECTURE.md` lists a few automated dependency checks so later changes cannot quietly reverse the decoupling. Start with:

- `calibraw-cli`, `calibraw-core`, `calibraw-gpu` (headless configuration) and `calibraw-ai` do not depend on egui, egui-wgpu or eframe.
- `calibraw-core` does not depend on `calibraw-gpu`, `calibraw-ai` or `calibraw-ui`.
- `moduwu-design` depends on no CalibRaw crate.

Implement each check against a successfully produced dependency graph covering normal and build dependencies (see §14), and run them in ordinary CI. Add a check when a boundary is established, not in advance.

## 5. Documentation and agent instructions

### `AGENTS.md`

Keep it around 40 lines of actionable rules. Hold each item below to one or two lines and link to `docs/ARCHITECTURE.md` or `docs/DEVELOPMENT.md` for detail.

Use the repository `AGENTS.md` as the default home for this project's delegation strategy, with `CLAUDE.md` importing it. Reconcile any duplicate project policy in user-global instructions when implementing the documentation step. The policy itself: focused implementation, exploration and test tasks can be delegated; architecture, ambiguous requirements, integration and final verification stay with the coordinator.

Include:

- Rust ownership, narrow APIs, meaningful types, explicit imports and documented unsafe boundaries.
- WGSL naming, numerical assumptions, coordinate/colour spaces and verified Rust/WGSL contracts.
- Moduwu ownership of reusable presentation; CalibRaw ownership of domain rendering and photo data.
- The concrete-benefit requirement for extractions.
- Compatibility and behavior preservation.
- Focused changes and the required verification commands.
- A link to `docs/ARCHITECTURE.md`.

Do not put audit results, benchmark output, the feature matrix or the full migration checklist into agent instructions.

Give Moduwu a similarly short `AGENTS.md` covering presentation-only ownership, complete presets, meaningful widget coverage, accessibility information for custom widgets and API compatibility. Moduwu's `rust-version` must stay at or below CalibRaw's (both 1.92 at survey time). Do not require a builder for every trivial helper.

Claude Code reads `CLAUDE.md`, not `AGENTS.md`. Add a `CLAUDE.md` containing only `@AGENTS.md` in both repositories instead of duplicating policy. Preserve operational guidance in `docs/DEVELOPMENT.md`; replace duplicated conventions with links without losing instructions on dismissal, focus, navigation or serialization.

### Other documents

- `docs/ARCHITECTURE.md`: responsibilities, allowed dependencies and their automated checks, state ownership, thread affinity and platform boundaries.
- Working review coverage table (§15): one row per crate and supporting area.
- Working feature matrix: feature, platforms, existing test/fixture, coverage gap and acceptance evidence.
- Working audit: candidate, callers, semantic differences, estimated savings, cost, risk and next PR.
- Working baselines: commit IDs, commands, environment, screenshots, numerical fixtures and performance budgets.

Keep the working documents compact and update them as changes land.

Keep the plan, feature matrix, audit and baseline manifests in version control under `docs/` as the default. Store large screenshots, fixtures and measurement artifacts in the fixed external `CALIBRAW_BASELINE_DIR` described in Step 3. Link artifacts from the manifests and record revisions, fixture hashes, environment, commands and comparison results. These documentation defaults do not block initial work; moving this plan under `docs/` is a future documentation step.

## 6. Rust implementation guidelines

Use the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) as the reference for reusable public APIs, applying judgment rather than enforcing every recommendation mechanically.

- Use enums to model mutually exclusive states instead of combinations of loosely related flags.
- Use units or coordinate-space types where confusing values can cause bugs: image versus viewport coordinates, encoded versus linear colour, or pixels versus fractions.
- Borrow by default. Justify clones of image buffers, edit snapshots and other expensive data.
- Use `Arc` when data is genuinely shared across jobs; do not add shared mutability merely to avoid designing ownership.
- Use typed errors when callers distinguish failure classes. Add context at application boundaries and preserve actionable diagnostics.
- Use `Option` for absence, not as a substitute for recording failure.
- Restrict `unwrap`/`expect` in production to justified invariants with useful messages. Tests can use them for setup and assertions.
- Use RAII for native handles, temporary resources, GPU reservations and cleanup. Document safety assumptions at unsafe operations and expose safe wrappers where possible.
- Preserve existing checked arithmetic, input validation and failure recovery during consolidation.
- Document public behavior, ownership, units and numerical contracts. Explain why an unusual choice exists rather than narrating obvious code.
- Keep imports explicit in production modules. Test-module conventions can remain concise where they improve readability.
- Avoid unnecessary dependencies, excessive generic parameters and generated boilerplate that obscures behavior.

Review large modules by responsibility, especially RAW decoding/profile selection, GPU construction/resources, tiled export, mask operations, replay, AI preparation and Android integration. Retain deliberate low-level specialization and native compatibility handling.

## 7. UI and Moduwu workstream

### Ownership rule

Reusable presentation goes in Moduwu. Photo data, specialized image controls and application composition stay in CalibRaw and consume shared presentation tokens.

Custom painting is allowed for tone curves, histograms, masks, colour controls, geometry and image canvases. Their coordinates and dimensions are not automatically spacing tokens.

Keep HSL/RGB channel colours, temperature/tint gradients, mask identifiers and data-driven colour conversion in domain modules. Review `rating`, backdrop choices and similar roles individually before adding them to a generic palette. A saved preview backdrop must retain its existing meaning across theme changes.

### Tokens

Add missing semantic text/status roles, overlay background/foreground roles, typography, radii, strokes and platform control metrics where real callers need them. Every preset must define its applicable roles.

First preserve existing appearance and geometry. Do not round existing font sizes, radii or strokes to a new scale during extraction. Standardize deliberately in later visual changes.

Contrast checks must account for alpha compositing where applicable. Check overlays over representative photo backgrounds, not only against an opaque nominal token.

### Widgets

Prioritize:

1. `NumberField`: focus-aware arrow stepping/repeat, integral and fractional ranges, display precision, suffixes, enabled state, explicit IDs, and live versus commit-after-edit updates.
2. Shared slider: track/value layout, keyboard and pointer behavior, touch targets, reset semantics, gradients and scroll ownership.
3. Text fields/areas, badges and overlay surfaces.
4. Icon sizing, checkbox, spinner/progress and frame helpers where existing Moduwu APIs cannot already serve the callers.

Start with export and settings as the first complete `NumberField` migration. Move the slider's existing interaction regression tests with its reusable behavior. Keep `FloatParamSpec` adaptation and photo-specific gradient definitions in CalibRaw.

The slider API must retain meaningful interaction information and stable widget identity.

Shared controls must preserve accessibility behavior, including role, label, value where applicable, keyboard focus, disabled state and supported actions such as activation, setting a value or increment/decrement. Built-in egui controls already supply information and actions; wrappers must preserve them rather than replace them with incomplete metadata. Custom-painted controls must supply equivalent information through the appropriate egui/AccessKit APIs and respond to their advertised actions.

Record what the current controls expose before extraction. Add focused regression checks for accessibility information and actions, and a screen-reader review of the shared controls on a desktop integration, Linux (Orca/AT-SPI) first. Android has no AccessKit integration today; do not treat TalkBack behavior as a preservation contract. Accessibility acceptance is based on behavior, not explicit `widget_info` call counts. Keep depth-range, point-colour, tone-curve, HSL and wheel behavior local where it is domain-specific.

Generic gradients must preserve existing hue interpolation, wrapping, colour-space assumptions and variable neutral points. Colour stops alone are sufficient only when they reproduce the required behavior.

Build a gallery for reusable controls in all four themes with desktop and Android metrics. Allow runtime metric selection so the desktop gallery can actually exercise both layouts. Include disabled, focused and relevant interaction states.

### Imports and theme integration

Import shared primitives directly from Moduwu. An optional prelude is a convenience; explicit imports remain valid.

Remove shared-primitive re-exports from CalibRaw's `theme.rs` after migration. Retain or relocate application theme installation, font setup, selection and photo helpers according to ownership. Move `UiDesign` and `PreviewBackdrop` to an appropriate settings/model module while preserving serde names and behavior.

### Views and actions

Give views narrow inputs. Permit mutable view-local drafts, selection, focus and drag state. Return domain actions or an ordered collection of actions as needed; do not mandate `Option<Action>` for every view.

Application handlers own domain mutation, persistence and background work. Define edit transaction boundaries so slider drags retain undo grouping and preview/save behavior. Move presentation drawing out of application services separately from widget extraction.

### UI lint

Introduce `cargo xtask ui-lint` after the first extraction establishes practical rules.

- Start with checks for actual duplicated chrome and bypasses of established shared controls.
- Exempt domain rendering explicitly with a reason and a narrow scope.
- Avoid blanket bans on `vec2`, colour construction, strokes or custom painting.
- Track reviewable occurrences, not only counts per file. Replacing one violation with another must not silently pass; file movement must not reset allowances.
- Require explanations for new exceptions and review baseline changes in each migration PR.
- Add the check to CI once it is reliable.

The final target is zero unapproved violations. Documented domain-rendering exceptions remain valid.

## 8. Shared processing services

Audit complete preview, export, batch, replay, thumbnail and AI preparation workflows. Identify repeated loading, validation, colour conversion, geometry, processing configuration, rendering, progress and output preparation.

Consolidate shared steps into small services with explicit inputs and results. Preserve caller-specific quality, resolution, tiling, halo, memory, cancellation and output policies.

Useful boundaries may include immutable document/edit snapshots, validated render requests, region/tile planning, render execution and output writing. Introduce them when the audit establishes actual common behavior.

Move replay stage planning, rendering and video writing into presentation-independent modules. Leave application command handling, worker startup/polling and displayed progress in the app. Use a narrow notification callback or existing event mechanism for repaint requests.

Prove reuse with real callers. Use the same processing path from the CLI and UI where their contracts match. Do not duplicate application business logic inside the CLI or add a new CLI feature solely to justify an abstraction.

## 9. Jobs, state and platform consolidation

Build on existing worker, event-draining and AI-runtime helpers. Audit common lifecycle handling before designing additional infrastructure.

Standardize where behavior is shared:

- Progress, success, failure, cancellation and disconnection handling.
- Document/job generations so stale results cannot overwrite a newly selected image or later edit.
- Ownership of immutable inputs, cache entries and model/GPU resources.
- Repaint notification at the presentation boundary.
- Cleanup and diagnostics after cancellation, provider failure, allocation failure or document closure.

Distinguish document state, temporary interaction state, active work and caches. Do not force unrelated jobs into one universal scheduler.

### Concurrency contracts

Document these alongside the ownership design in `docs/ARCHITECTURE.md` for each affected component. No new scheduler is required.

- **Thread affinity:** which threads may access egui/UI state, JNI environments and Java objects, ONNX model sessions, and the wgpu device, queue and egui renderer.
- **Queue growth:** whether queues are bounded, coalesced (latest request wins) or unbounded, and why.
- **Document closure:** what happens to queued and running work when a document closes or another one is selected.
- **Cancellation semantics:** whether cancellation stops execution, or only discards the result while the work runs to completion. Record who owns the cancellation token.
- **Shutdown and failure:** how workers stop, how they release GPU/model/native resources, and what happens when a worker panics or its channel disconnects.
- **Synchronization:** which component owns each lock or channel, and lock ordering where more than one lock is held.

Verify changed contracts with focused tests where deterministic, for example closing a document with queued work or a worker channel disconnecting.

Keep AI model-specific normalization, tensor shapes and postprocessing explicit. Consolidate only equivalent preparation and runtime behavior; preserve fallback, consent and model-retention policy.

Localize platform differences through narrow storage, picker and codec adapters. Preserve Android content-URI behavior, JNI lifetimes, desktop path handling, permission failures and codec-specific requirements. Test platform adapters independently where possible.

### Rust/Java JNI contract

Treat the JNI boundary like the Rust/WGSL boundary: two languages, one contract, with mismatches that can escape compilation.

- Add a check that compares every `Java_de_duecki_calibraw_*` export in `calibraw-ffi` with Java `native` declarations: class, method name, parameter and return types, and static-versus-instance status. Account for the implicit environment and class/object arguments; matching symbol names alone is insufficient.
- Cover Rust-to-Java calls through `env.call_method`, static calls and explicit method lookups where practical. Verify receiver/class, name and descriptor, including instance calls used by the current code. Use compiled Java descriptors where practical to avoid ambiguous source-level type matching.
- Run the static contract check in ordinary CI; it needs no device. Record any Java-toolchain requirement.
- Keep JNI reference lifetimes, exception checks and thread attachment explicit in Rust wrappers.
- Retain an Android runtime smoke test for the affected calls. Static signature checks do not establish correct lifetimes, exception handling or activity lifecycle behavior.

### Android Java

Include the Java sources in the duplication audit and the review coverage table, not only their JNI surface:

- Responsibility boundaries between `CalibRawActivity`, storage (`StorageManager`, `AndroidStorageContract`, `PickerLocationStore`), export/publishing, replay encoding, thumbnails and notifications.
- Lifecycle ownership: what each class holds across pause/resume and activity recreation.
- Resource cleanup: streams, file descriptors, `MediaCodec`/muxer instances and bitmaps.
- Storage and export logic duplicated between Java and Rust, or across Java classes.

Keep the existing Java unit tests passing and extend them where Java changes. Add Android lint checks only when Java changes justify them, and record existing lint findings before making any check mandatory.

### Build tooling

Audit `build.rs` scripts, `xtask`, Gradle, CMake, shell/Python scripts and CI/release workflows as maintained code.

- Consolidate repeated build metadata and setup where practical, for example the duplicated `settings.gradle` files and version, SDK and dependency revisions repeated across Cargo metadata, Gradle, CMake and workflows.
- Preserve source pins, checksums, license generation, signing, packaging (AppImage, Flatpak, Windows) and F-Droid buildability.
- Validate workflow changes with a real run on a branch (or `workflow_dispatch`) before relying on them. Release workflow changes need a dry run that stops before publishing.

## 10. WGSL and Rust/WGSL contracts

### Style and modules

- Keep production shader source in `.wgsl` files, including the current remove-composite shader.
- Use consistent names and small functions; reserve entrypoints for resource access and algorithm coordination where practical.
- Document input/output colour spaces, coordinate systems, boundary handling, precision and mathematical assumptions.
- Consolidate equivalent transfer functions, sampling and interpolation helpers within existing composable modules.
- Preserve operation order, clamps, negative/HDR handling and deliberate specialization during structural cleanup.
- Keep resource bindings and algorithm responsibilities clear. Preserve pass order, barriers, tile halos, global anchoring and cache behavior.
- Consolidate repeated shader lists/metadata when it reduces maintenance without hiding entrypoints or variants.

Use the [WGSL specification](https://www.w3.org/TR/WGSL/) as the language/layout reference, while supporting the capabilities of the project's pinned Naga/wgpu versions and actual target devices.

### Naga layout verification

Use tests rather than a code-generation build step for the initial implementation. Naga is already available to GPU tests.

Inspect the final modules produced through the same `ShaderManager` composition and source-specialization paths used in production. Raw parsing of individual imported files is insufficient.

Compare applicable shader declarations with Rust upload/readback types:

- Struct spans and member offsets against Rust `size_of` and `offset_of!`.
- Array strides, nested structures and element representations.
- Required buffer layout/alignment and binding-size contracts.
- Binding groups, numbers, resource kinds, access and texture formats.
- Effect IDs, limits, metadata packing and shared constants.

Do not assume `#[repr(C)]` or `bytemuck::Pod` proves WGSL compatibility. Rust's native `align_of` need not equal a shader vector's alignment; verify the actual byte layout and GPU binding requirements.

Extend existing size and packing assertions. Test relevant Preview/High formats, Bayer/X-Trans paths and specialized modes. Keep validator capabilities consistent with supported profiles; successful validation with every capability enabled does not establish device support.

Where CPU and GPU implement equivalent mathematics, use shared fixtures and numerical comparisons. Keep understandable implementations in both languages rather than introducing a cross-language generator by default.

## 11. Feature preservation and validation

### Compact feature matrix

Link each row to existing tests or fixtures; add coverage only for material gaps.

| Feature group | Contracts to preserve |
|---|---|
| Image loading | Supported RAW/rendered formats, orientation, metadata, embedded thumbnails and fallback behavior |
| Colour processing | Profiles, white balance, transfer functions, working spaces, HDR/negative-value behavior |
| Processing | Demosaicing modes, tone/detail/noise controls, stage order, region and tiled/full-frame equivalence |
| Geometry | Crop, rotation, flips, transforms, lens corrections and coordinate mapping |
| Masks/effects | Composition, brush/shape/depth/subject/object masks, all effect parameters and packing |
| AI | Installation, consent, providers/fallback, denoise/removal/masks and memory retention |
| Editing/persistence | Undo/redo, grouped edits, save/load, sidecar transfer, existing serde names/defaults and safe handling of unsupported newer data |
| Presets/library | Groups, preview/apply, dialogs, selection/filtering, thumbnails, catalog/storage and batch actions |
| Export/replay | Formats, bit depths, resizing, profiles/metadata, replay ordering and platform codecs |
| UI/platforms | Four themes, responsive layouts, keyboard/pointer/touch behavior, desktop accessibility behavior, Android storage/lifecycle and desktop integrations |
| Failure/recovery | Interrupted saves, cancelled output, malformed data, stale results, allocation failure and resource cleanup |

Existing entrypoints include core sidecar/preset tests, GPU shader/packing and image regressions, mask and preview interaction tests, `portrait_gpu_layout_and_input`, and `gpu_ui_review`. Record exact tests in the matrix at baseline time.

### Persistence compatibility

Use representative historical settings, presets and sidecars in addition to round-trips of current types. Decide the required forward-compatibility policy before changing persistence.

Unsupported newer versions or fields must be handled explicitly: preserve them according to a documented round-trip policy, or reject the operation safely with an actionable error. Successfully opening a file does not prove compatibility if saving it silently drops edits. Tests must establish the selected policy and that rejected or failed operations do not overwrite the original data.

### Failure-path acceptance

Link existing tests or add focused coverage for material gaps. Record the expected state and output after each affected failure, not merely that an error is returned.

| Scenario | Required acceptance evidence |
|---|---|
| Failed/interrupted save | Existing data remains intact under the storage backend's documented guarantees; temporary resources are cleaned up and failure is reported |
| Cancelled/failed export or replay | Partial-output handling is explicit and consistent; final output is not reported as successful; resources are released |
| Malformed or unsupported persisted data | No silent data loss or overwrite; validation and compatibility policy determine the outcome |
| Document/edit changes during active work | Stale results cannot mutate the current document or replace its preview; affected resources are retired |
| Host/GPU allocation failure | Recovery, fallback or termination follows existing policy with useful diagnostics and resource cleanup |
| Android pause/resume or activity recreation | Affected jobs and callbacks follow documented lifecycle policy without applying obsolete results or using invalid activity references |

Put deterministic failure checks in ordinary CI. Use explicit GPU or Android checks for failures requiring those environments; record platform guarantees rather than assuming desktop atomic-replacement behavior applies to content providers.

### Test tiers

| Tier | Checks | Execution |
|---|---|---|
| Ordinary CI | Formatting, Clippy, licenses, CPU/unit tests, doc tests, deterministic failure/concurrency/accessibility checks, composed Naga validation/layout tests, JNI contract check, serde compatibility and architecture dependency checks | Every applicable PR; deterministic and no physical GPU required |
| Android JVM | Java unit tests (`./gradlew :app:testDebugUnitTest`) | Android workflow, which already has a JDK and Android SDK; locally when Java changes |
| GPU validation | Numerical fixtures, image comparisons, tile/region equivalence, pointer/touch preview tests and screenshot review | Explicit local commands or a configured GPU runner |
| Performance | Loading, pipeline/shader preparation, processing/export throughput, memory and responsiveness | Controlled environment with recorded hardware/software |
| Platform acceptance | Android build, APK assembly, JNI smoke tests and affected lifecycle behavior; Android lint where Java changes justify it; desktop screen-reader review (Linux first); Linux, Windows and macOS checks appropriate to affected code | Existing platform workflows plus targeted runtime review |

Ignored tests require explicit invocation. Ordinary `cargo test` does not establish their result. Retain existing GPU checks and document which need an adapter, optional assets or other environment setup.

### Numerical acceptance

Choose a small representative golden/fixture set before changing numerical code. Include applicable Bayer/X-Trans, dark/highlight/HDR, mask/edge and tiled processing cases, using reproducible synthetic or appropriately licensed fixtures.

Record adapter, backend, driver, processing quality and fixture versions. Specify tolerances per operation, with absolute/relative error where appropriate, maximum error, localized checks and non-finite-value handling. An average image metric alone can hide seams and small broken regions.

Dependency updates may invalidate affected comparisons. Dependabot proposes weekly cargo updates, and egui, wgpu, naga or naga_oil changes can alter rendering, shader compilation and timing. Land relevant updates separately from structural refactors and review their effects independently. Preserve the original baseline; record new reference measurements with the updated dependency/lockfile revisions rather than overwriting history or attributing dependency changes to the refactor.

Do not loosen tolerances simply to accept a regression. Document and review intentional differences separately. Structural cleanup should initially preserve output on the same environment; cross-adapter acceptance needs its own established tolerances.

### Visual acceptance

Capture before/after images for affected states through the existing 132-case review harness. Use filters for focused PRs and a full review at milestones. A gallery validates shared controls; app captures validate their composition.

Portrait desktop captures are useful but do not substitute for Android runtime validation. Review Linux and Android in all four themes for appearance changes; validate Windows/macOS where platform integration changes.

### Performance acceptance

Record existing benchmarks before implementation. Add a benchmark only when a changed hot path lacks meaningful evidence.

Define practical budgets for initial loading, first preview, shader preparation, repeated editing, export/replay, resident/peak host and GPU memory, and interaction latency. Run repeated measurements under comparable conditions and record normal variance. Preserve caches and avoid extra allocation/copies during abstraction changes.

### Cache validity and benchmark state

Record the keys and versioning rules for relevant thumbnails, processing results, model/preparation results and GPU pipeline caches. Preserve valid reuse and explicitly invalidate incompatible entries. Check source/edit changes, corrupt entries and relevant shader, dependency or schema changes against the applicable cache contract; do not assume every cache has identical invalidation requirements.

Measure cold and warm loading/processing separately. Define what is cleared or prewarmed, isolate benchmark cache state from ordinary use, and record that state in the results. Cached work must not conceal first-use or cache-miss regressions.

## 12. Implementation sequence

### Step 1: finish and merge `presets`

Verify branch status, review the diff, run relevant checks and finish the branch through the repository's normal review/merge process. Record the stable main commit used for baselines. Do not mix the rework into unfinished preset changes.

### Step 2: write concise rules and architecture

Add the short CalibRaw/Moduwu agent instructions and `docs/ARCHITECTURE.md`. Preserve current operational conventions. Create compact placeholders for the feature matrix, baseline record and audit list.

### Step 3: establish protection and baselines

Record commit IDs, inventory counts, feature/test links, relevant screenshots, a small numerical fixture set and existing benchmark results. Add initial Naga layout checks against representative shared buffers before moving them.

Choose a consistent, documented production/test counting method that accounts for inline `#[cfg(test)]` modules, test files, benches, examples and generated code. Reuse an existing suitable tool or implement a bounded helper such as `cargo xtask loc` if needed. Record the method and exclusions with the baseline and use them consistently across both repositories. Do not move test modules solely to simplify counting or turn measurement tooling into another major workstream.

Store baseline screenshots and measurements in one fixed, persistent location outside the repository, referenced through `CALIBRAW_BASELINE_DIR` (for example `~/calibraw-baselines/`). Inside it, use one folder per capture run, named from the date, CalibRaw revision and label, for example `2026-10-05-ca545c3-ui-before/`. Never overwrite an existing run folder: two worktrees must not overwrite each other's captures, and the original reference must survive dependency updates. Each run folder contains a manifest recording the CalibRaw and Moduwu revisions, `Cargo.lock` hash, command, environment and adapter. Never use `/tmp`, `target/` or a path derived from the current checkout: §13 uses separate worktrees, and a per-checkout directory would scatter baselines and compare "after" captures against an empty or wrong "before". A directory inside the repository is also removed by `git clean -fdx`. Use an in-repository git-ignored directory only for a single-checkout workflow. Keep the small baseline manifests in version control and link to the artifacts.

Keep this phase bounded. Prioritize the first migrations and track remaining coverage gaps instead of waiting for exhaustive documentation.

### Step 4: land early decoupling wins

Separate headless GPU processing from egui integration. Prefer UI ownership of texture registration/retirement; optional presentation dependencies can be an intermediate migration if smaller.

Replace replay worker repaint coupling with a callback/event boundary and extract its presentation-independent work where straightforward.

Verify standalone CLI dependency isolation, feature combinations, preview registration/retirement, error cleanup and replay output. Compare these changes against the initial baselines.

### Step 5: perform a time-boxed duplication audit

Use one focused initial pass, approximately one working day, followed by targeted follow-ups for candidates selected for implementation. Produce a ranked table rather than a full report.

Cover the Rust crates, WGSL, Android Java (§9) and build tooling (§9). For each candidate record the duplicate behavior, real callers, intentional differences, estimated production savings, maintenance benefit, cost, risk and required checks. Give each selected PR an explicit acceptance contract.

Start the review coverage table (§15) here, so that every area gets at least an initial review row even when it yields no candidates.

### Step 6: migrate shared UI in parallel

The UI workstream can proceed alongside the audit after token/API ownership is agreed. Start with `NumberField` in export/settings, then extract reusable slider behavior with its tests.

Continue by area: presets, library, preview overlays, develop controls and presentation drawing currently in application modules. Adjust order for dependencies and audit findings.

Use isolated PRs, gallery entries, relevant app captures and interaction checks. Introduce the practical UI lint after the first migration, then reduce its approved baseline as areas migrate.

### Step 7: consolidate shared processing services

Implement the highest-value audit candidates across preview, export, batch, replay, thumbnails and AI preparation. Prove reuse through existing UI and CLI callers. Remove replaced implementations in the same migration once callers and checks pass.

### Step 8: consolidate jobs and platform adapters

Unify equivalent lifecycle handling, narrow state ownership and isolate storage/codec integrations. Verify stale-result rejection, cancellation, history transactions, recovery and platform behavior.

### Step 9: clean up WGSL and complete interface checks

Move embedded source, consolidate equivalent helpers and repeated metadata, and extend Naga contract checks to the relevant variants. Keep shader structural changes separate from algorithm/performance redesign.

### Step 10: finish remaining Rust cleanup and acceptance

Split responsibility-heavy modules, narrow APIs/imports, remove dead paths proven unused, simplify dependencies and finish unresolved audit items. Run milestone regression, visual, performance and platform checks.

Record achieved savings and remaining deliberate duplication. Do not declare completion while required feature preservation or compatibility checks remain unresolved.

## 13. PR and delegation workflow

- Delegate focused exploration, implementation and test tasks where subagent tools are available. Define ownership, allowed files, expected result and verification for each task.
- Keep architecture decisions, ambiguous behavior, integration review and final verification with the coordinator.
- Agree shared APIs before parallel implementation. Avoid concurrent edits to central modules and shared metadata.
- Use isolated branches/worktrees for independent work. Keep baseline capture and benchmark work isolated from configuration or source mutations.
- Each PR states the problem, resulting behavior, reuse/reduction benefit, compatibility implications and validation evidence.
- Remove temporary wrappers and replaced code when migration completes. If an adapter remains intentionally, document its owner and purpose.
- Pin a real Moduwu commit and update the lockfile before merging CalibRaw changes. Local path overrides are development-only and must not become a required sibling checkout.
- Track API compatibility in Moduwu; coordinate versions/releases for breaking public changes.
- Push every pinned Moduwu commit to the public repository before the CalibRaw change merges. F-Droid builds from source and must be able to fetch it.
- When dependencies change (for example egui leaving the CLI graph, or a Moduwu bump), run `bash scripts/generate_licenses.sh`, update `THIRD_PARTY_NOTICES.md` where affected, and pass `cargo deny check`.

### Releases during the rework

- `main` stays releasable after every merged PR; releases continue on the normal schedule.
- Release tags are validated against the Cargo version in `release.yml`. A release made mid-rework must pass the same checks as any other release, including Android.
- Update `fastlane/metadata/android/en-US/changelogs/` only for user-visible changes. Pure refactors need no changelog entry.
- If a workstream cannot land in releasable steps, finish it on a branch and merge it as one reviewed change, rather than shipping a half-migrated state.

No commit, push, merge or publishing action is performed merely by writing this plan. Execute those actions under the user's authorization and normal repository workflow.

## 14. Verification commands

Current standard workspace checks, with the documented native dependencies installed:

```sh
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --all-features -- \
  -D warnings -W clippy::perf -W clippy::large_stack_arrays \
  -W clippy::redundant_clone -W unreachable-pub
cargo test --locked --workspace --all-targets
cargo deny check
```

Planned addition once public APIs gain doc examples:

```sh
cargo test --locked --workspace --doc
```

`--all-targets` does not run doc tests, and CI does not run this command yet. Before adding it to CI, run it once to confirm it succeeds with the bin-only `xtask` package and the `cdylib`+`rlib` UI crate.

After dependency changes:

```sh
bash scripts/generate_licenses.sh
```

Use focused crate/test runs while implementing. Complete the required checks before finishing; repeat broad checks only after changes or unresolved failures justify them. An all-features check does not replace testing the headless/default-disabled configuration after adding optional presentation features.

Headless dependency check, run for the standalone CLI rather than the entire workspace. Include build dependencies as well as normal ones, which is Cargo's closer approximation of what is actually built:

```sh
cargo check --locked -p calibraw-cli
cargo tree --locked -p calibraw-cli -e normal,build --target all \
  --prefix none --format '{p}' > cli-deps.txt
```

The check passes only if the `cargo tree` command itself succeeds and its output contains no `egui`, `egui-wgpu` or `eframe` package. Do not use `cargo tree -i <package>` failing as evidence: Cargo exits with the same error code (101) for "package not found" as for any other failure, such as a lockfile or network problem. Apply the same pattern to the other boundaries in §4.

Today the graph contains egui and egui-wgpu through `calibraw-gpu`; Step 4 removes that path. Check relevant feature configurations; account for Cargo feature unification when interpreting workspace builds.

UI and GPU checks:

```sh
cargo test --locked -p calibraw-ui --lib
cargo test --locked -p calibraw-gpu --lib
cargo test --locked -p calibraw-ui --lib portrait_gpu_layout_and_input -- \
  --ignored --nocapture --test-threads=1
```

Screenshot capture into a new run folder (POSIX shell; from fish, run it with `bash`):

```sh
: "${CALIBRAW_BASELINE_DIR:?set CALIBRAW_BASELINE_DIR to the fixed baseline location}"
test -d "$CALIBRAW_BASELINE_DIR" || { echo "baseline dir missing" >&2; exit 1; }
run_dir="$CALIBRAW_BASELINE_DIR/$(date +%F)-$(git rev-parse --short HEAD)-ui-before"
mkdir "$run_dir"   # fails instead of overwriting an existing run
CALIBRAW_UI_REVIEW_DIR="$run_dir" \
  cargo test --locked -p calibraw-ui --lib \
  app::ui_review_tests::gpu_ui_review -- \
  --ignored --exact --nocapture --test-threads=1
```

The guard prevents an unset variable from writing to `/ui-review/...`, and `mkdir` without `-p` refuses to reuse an existing folder. Write the run manifest (Step 3) next to the captures. If these steps are repeated often, replace them with a small helper such as `cargo xtask baseline-run <label>` that validates the location, creates the folder, writes the manifest and works from any shell.

Run the portrait test with an isolated `XDG_CONFIG_HOME`, as documented in `docs/DEVELOPMENT.md`. The review harness isolates its child-process configuration itself. `CALIBRAW_BASELINE_DIR` is the fixed persistent location from Step 3, shared by all worktrees. Use separate before/after run folders and `CALIBRAW_UI_REVIEW_FILTER` for targeted comparisons. Explicitly run other applicable ignored GPU tests identified in the feature matrix.

Existing benchmark:

```sh
cargo bench --locked -p calibraw-core --bench mask_rasterization
```

Record the invocation and fixture used for `crates/calibraw-core/examples/raw_open_bench.rs` when establishing loading baselines. Use existing Android/platform build and verification commands from `docs/DEVELOPMENT.md` and the workflows.

Android Java unit tests (from the repository root, which the root `settings.gradle` supports):

```sh
./gradlew :app:testDebugUnitTest
```

Record whether this task also triggers the native build and therefore needs the NDK and Rust Android toolchain. Unit tests, APK assembly and the JNI contract check provide different evidence; one does not replace another. When Java changes justify it, also run `./gradlew :app:lintDebug`, after recording its existing findings.

After implementing the UI lint and JNI check, plus the line counter if needed:

```sh
cargo xtask ui-lint
cargo xtask loc
cargo xtask jni-contract
```

The command names are proposals; record the final names in `docs/DEVELOPMENT.md`. If an existing counting tool is selected, document its command in place of `cargo xtask loc`.

Run independent Moduwu formatting, tests and Clippy in its own checkout. Do not present planned commands or unavailable GPU/platform checks as already completed.

## 15. Completion record

For each workstream record:

| Field | Evidence |
|---|---|
| Baseline/final revision | CalibRaw and Moduwu commit IDs |
| Behavior preserved | Feature-matrix rows and regression checks |
| Reuse achieved | Real callers of the consolidated API |
| Duplication removed | Replaced implementations and actual production-code delta |
| Compatibility | Historical persisted fixtures, unsupported-data policy and relevant platform contracts |
| Failure/recovery | Expected state/output after affected failures, cancellation and lifecycle changes |
| Accessibility | Preserved information/actions, focused regression checks and screen-reader review |
| Numerical/visual result | Tolerances, fixture results and before/after captures |
| Performance | Comparable cold/warm timing and memory measurements within agreed budgets; verified cache validity |
| Dependency improvement | Headless graph, automated boundary checks and documented ownership |
| Concurrency | Documented thread affinity, queue, cancellation and shutdown contracts for affected components |
| Remaining exceptions | Deliberate specialization, domain rendering and deferred items |

### Review coverage

Keep one compact row per area. Every area gets a review; changes follow demonstrated problems.

| Area | Reviewed responsibilities | Refactor candidates | Accepted exceptions | Verification |
|---|---|---|---|---|
| `calibraw-core` | | | | |
| `calibraw-gpu` (Rust) | | | | |
| WGSL shaders | | | | |
| `calibraw-ai` | | | | |
| `calibraw-ui` | | | | |
| `calibraw-ffi` | | | | |
| `calibraw-cli` | | | | |
| Android Java | | | | |
| `moduwu-design` | | | | |
| `build.rs` scripts and `xtask` | | | | |
| Gradle and CMake | | | | |
| Scripts and packaging | | | | |
| CI and release workflows | | | | |

"Reviewed" means the area's responsibilities, public surface, coupling and duplication were examined and the outcome recorded, including "no change needed". It does not require rewriting every file or splitting every module below 600 lines.

### Definition of done

The whole-project review is complete when:

- every row of the coverage table is filled in;
- each area has clear ownership, appropriately narrow APIs and coupling that matches `docs/ARCHITECTURE.md`, enforced by the automated boundary checks;
- the selected duplication has been removed, and remaining duplication is recorded as deliberate;
- the agreed feature contracts pass, the standalone CLI remains headless, Rust/WGSL and Rust/Java JNI interfaces are verified, shared controls expose accessibility information, and migrated code uses the shared implementations;
- combined production duplication has decreased.

Preserve necessary platform differences, numerical specializations, tests and explanatory documentation.

## 16. Documentation defaults and remaining compatibility decision

Use the following documentation defaults for initial work. Resolve forward compatibility before changing persistence; documentation placement does not need to delay Step 2.

1. **Compatibility with older app versions: remaining decision.** New versions must read supported old settings, presets and sidecars. Decide whether older app versions must also read files written by newer ones, particularly when Linux and Android share sidecars. If required, add tests using the relevant older reader and fixtures written by the new version. In either case, apply the explicit unsupported-data policy in §11 so opening and saving cannot silently discard edits.
2. **Home of the delegation strategy: repository default.** Keep project-specific policy in the repository `AGENTS.md`, imported by `CLAUDE.md`; reconcile duplicate project instructions when implementing §5.
3. **Documentation and artifacts: versioned manifests, persistent large artifacts.** Keep this plan and the working feature matrix, audit and baseline manifests under `docs/` in version control. Store large artifacts in the fixed external `CALIBRAW_BASELINE_DIR` (Step 3) and link them from the manifests. Moving the current root-level plan is a future documentation step.
