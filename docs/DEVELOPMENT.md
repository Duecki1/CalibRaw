# Development

Use Rust 1.92 with LibRaw, Lensfun, libclang, and the platform graphics
dependencies. Crate ownership, dependency rules and threading contracts are in
[ARCHITECTURE.md](ARCHITECTURE.md).

```sh
cargo run -p calibraw-ui --bin calibraw --release
```

Release builds on every platform use the LibRaw master commit pinned as
`libraw_revision` in `Cargo.toml`, because LibRaw releases up to 0.22.2 lack
cameras such as the Sony A7 V. A distribution LibRaw still builds and runs,
but cannot open those cameras. To develop against the pinned commit on Linux
(macOS needs the extra flags in `build-macos.yml`):

```sh
scripts/build_libraw.sh "$HOME/.local/calibraw-libraw"
export PKG_CONFIG_PATH="$HOME/.local/calibraw-libraw/lib/pkgconfig"
export LD_LIBRARY_PATH="$HOME/.local/calibraw-libraw/lib"
```

To move the pin, update `Cargo.toml`, `scripts/build_libraw.sh`,
`android/app/src/main/cpp/CMakeLists.txt`, the Flatpak manifest and the
`libraw-<commit>` cache key in `build-linux-appimage.yml` together.

## Checks

Ordinary CI runs these on every push and pull request; run them before
finishing a change:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- \
  -D warnings -W clippy::perf -W clippy::large_stack_arrays \
  -W clippy::redundant_clone -W unreachable-pub
cargo test --locked --workspace --all-targets
cargo xtask arch-check
cargo xtask cfg-ratchet
cargo xtask jni-contract
cargo xtask ui-lint
cargo xtask migrations
cargo deny check
```

- `cargo xtask arch-check` enforces the crate dependency rules listed in
  ARCHITECTURE.md against `cargo tree -e normal,build --target all`.
- `cargo xtask cfg-ratchet` counts the `cfg`, `cfg!` and `cfg_attr`
  conditions that mention `target_os` in every Rust file under `crates/` and
  `xtask/`, tests included, and compares them with `xtask/cfg-baseline.txt`.
  It fails when a file's count rises or a file gains its first one; a drop
  passes, and `--update` lowers the baseline (it refuses while any count
  rose). Directories that own platform differences are exempt and listed in
  `ALLOWED_DIRECTORIES` in `xtask/src/cfg_ratchet.rs`. Raising an entry is a
  reviewed edit of the baseline file.
- `cargo xtask jni-contract` checks every Rust JNI export and Java `native`
  declaration, and every Rust-to-Java call, against each other.
- `cargo xtask ui-lint` reports UI code that bypasses an established shared
  control (`DragValue` instead of `NumberField`, raw `ComboBox`, `egui::Slider`,
  singleline `TextEdit` or `Window`) or re-exports Moduwu items. Findings are
  keyed by rule, enclosing function and source line, so moving code keeps its
  approval but replacing one bypass with another does not. Approved findings
  live in `xtask/ui-lint-baseline.json`, each with a reason; stale approvals
  fail too. `--suggest` prints entries for new findings, whose reasons must be
  written before they pass. Review baseline changes like code.
- `cargo xtask migrations` lists every `migration: remove in vX.Y.Z` marker
  under `crates/` and fails once the workspace version reaches one, or when a
  file in `crates/calibraw-core/src/migrations/` has no marker. Bumping the
  version for a release therefore requires deleting the migrations it retires.
- `layout_contract_tests` in `calibraw-gpu` compare Rust uniform/storage
  structs, buffer bindings and shared constants with the WGSL modules the
  production `ShaderManager` composes. They need no GPU.
- `cargo xtask loc [--json PATH] [REPO...]` reports production, test and build
  line counts. It walks each crate's module tree from `cargo metadata` targets;
  `#[test]`, `#[cfg(test)]` items and test modules count as tests, and blank or
  comment-only lines are excluded from code totals. Pass `../moduwu-design` to
  count the design library with the same method.

Code compiled only for Android is not checked by host builds. After changing
`cfg(target_os = "android")` code, check the Android library with the
environment `cargo xtask build-android` uses:

```sh
export ANDROID_NDK_HOME="$ANDROID_SDK_ROOT/ndk/28.2.13676358"
host="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64"
CALIBRAW_LIBRAW_ROOT="$PWD/android/native/libraw/arm64-v8a" \
CALIBRAW_LENSFUN_ROOT="$PWD/android/native/lensfun/arm64-v8a" \
BINDGEN_EXTRA_CLANG_ARGS="--target=aarch64-linux-android26 --sysroot=$host/sysroot" \
LIBCLANG_PATH="$host/lib" CARGO_TARGET_DIR=target/android-check \
  cargo ndk -t arm64-v8a check --locked -p calibraw-ui --lib
```

The staged LibRaw and Lensfun come from `cargo xtask build-android-libraw` and
`build-android-lensfun`. Test targets are host-only and do not build for Android.

### Screenshot and numerical baselines

`CALIBRAW_BASELINE_DIR` names one persistent folder outside every checkout
(for example `~/calibraw-baselines`); never `/tmp`, `target/` or a path inside
a worktree. `cargo xtask baseline-run LABEL [--filter FILTER]` creates a new
`<UTC date>-<revision>-LABEL` folder (it refuses to reuse one), records the
CalibRaw and Moduwu revisions, the `Cargo.lock` hash, command and environment in
`run-manifest.json`, and captures the GPU UI review (4 themes × 3 viewports ×
11 states) into it. `--filter` takes comma-separated substrings of
`theme/size/state.png`.

`cargo xtask baseline-compare BEFORE AFTER [--tolerance N] [--diff-dir DIR]`
compares two runs pixel by pixel, reports missing, resized and changed
captures, and optionally writes diff images (changed pixels in red). The
review harness is deterministic, so structural changes are accepted at
tolerance 0. The recorded baselines are listed in
[rework/BASELINE.md](rework/BASELINE.md).

GPU-backed tests that need extra tools are ignored by default and run
explicitly, for example the end-to-end edit replay (needs a wgpu adapter and
FFmpeg with libx264):

```sh
cargo test --locked -p calibraw-ui --lib services::replay -- --ignored --test-threads=1
```

### AI parity

`crates/calibraw-ai/tests/parity.rs` runs every published AI model through
its public job API on one fixed input from the Demo photos and compares the
result with a golden in `data/ai-parity/`: IoU for masks, mean and maximum
difference for depth and denoise, PSNR for Remove. Run it for every change
that touches `calibraw-ai`; a CPU run of unchanged code reports
`bit-identical`.

```sh
CALIBRAW_PARITY_PHOTOS=/path/to/Demo \
  cargo test --locked -p calibraw-ai --test parity -- --ignored --nocapture
```

The inputs are pinned by SHA-256. Models come from the desktop model folder,
or `CALIBRAW_PARITY_MODELS`; `CALIBRAW_PARITY_DOWNLOAD=1` fetches missing
ones. `CALIBRAW_PARITY_RUNTIME` loads a specific ONNX Runtime library, and
`CALIBRAW_PARITY_GPU=1` keeps GPU acceleration on to compare GPU sessions
with the CPU goldens. `CALIBRAW_PARITY_BLESS=1` rewrites the goldens and
their pins from CPU sessions; do it only for an intended output change, and
review the new goldens like code. The Remove and X-Trans denoise cases
render or demosaic on the GPU first, so their goldens hold only on the
adapter that blessed them. The file header documents the formats.

For Linux sandbox development and packaging, see the [Flatpak workflow](FLATPAK.md),
including local snapshots, debug builds, validation, and unpublished release staging.

The CPU brush-raster baseline is a harness-free benchmark (kept independent
of the test suite): `cargo bench -p calibraw-core --bench mask_rasterization`.
It reports throughput for positive and erase dabs on a fixed 512x512 raster;
the benchmark does not alter production rasterization or numerical behavior.

## Design library setup

[Moduwu Design](https://github.com/Duecki1/Moduwu) is an independent Rust
library. CalibRaw imports it from GitHub through the root `Cargo.toml`, pinned
to a full commit hash. Cargo fetches it automatically for local builds and CI;
no sibling folder or separate clone is required. `Cargo.lock` records the same
Git source. `deny.toml` permits this repository while rejecting unknown Git
sources.

To update the library, set `rev` to a published commit in `Cargo.toml`, run
`cargo check -p calibraw-ui`, and commit both `Cargo.toml` and `Cargo.lock`.

For library development and its independent checks, optionally clone it:

```sh
git clone https://github.com/Duecki1/Moduwu.git ../moduwu-design
cd ../moduwu-design
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

This optional checkout is not used by CalibRaw's normal builds. Test changes
in the library, publish the commit, and update CalibRaw's pinned dependency.

To develop both together, add a temporary override at the end of the root
`Cargo.toml`:

```toml
[patch."https://github.com/Duecki1/Moduwu"]
moduwu-design = { path = "../moduwu-design" }
```

and run `cargo update -p moduwu-design`. CI has no sibling checkout, so before
merging publish the Moduwu commit, set `rev` (and `version` if it changed),
remove the override, and run `cargo update -p moduwu-design` again so
`Cargo.lock` records the Git source.

`cargo run --example gallery` in the Moduwu checkout shows every shared control
in each theme, with desktop or Android metrics selectable at runtime, plus
disabled and keyboard-focus states; its test renders every combination.

## UI conventions

`moduwu-design` owns reusable UI styling: the six built-in presets,
palette-driven egui themes, control/layout metrics, cards, toolbar rows, form
controls, `NumberField`, `Slider`, buttons, menus, dialogs and responsive
helpers. Import them from `moduwu_design` directly; CalibRaw does not re-export
them (`ui-lint` enforces this). `crates/calibraw-ui/src/ui/theme.rs` holds theme
installation and photo/editor-specific colours, and `appearance.rs` the
persisted `UiDesign` and `PreviewBackdrop` settings. Keep specialized image
canvases, mask cards, and colour controls in their existing components.

- Use secondary/primary action buttons for forms, settings and dialog footers,
  `menu_item` for regular menu actions, `menu_checkbox_item` for on/off options
  in menus, and `context_menu_item` for selectable navigation menus. Preserve
  each menu's explicit `ui.close()` behavior.
- Icon actions in toolbars, panel and card headers and tool rows use
  `ghost_icon_button` and `ghost_icon_toggle_button`, which show a frame only on
  hover, focus or selection; wrap them in `add_enabled_ui` to disable them.
  `icons` holds the folder disclosure control and the desktop tool rail glyphs.
- Use the shared form rows, text edits, `NumberField` and combo builders.
  `moduwu_design::Slider` owns value editing, reset, focus, keyboard and
  accessibility actions, and pointer/scroll handling on desktop and touch
  layouts; `AdjustmentSlider` adapts it to `FloatParamSpec` and adds the
  photographic track gradients. Scroll areas that contain sliders consult
  `moduwu_design::slider_scroll_locked`.
- Pick the control from what the value means: `moduwu_design::toggle` or
  `toggle_with_help` (a switch) for on/off options, `segmented_control` for one
  choice among a few, `chip` rows for filters and quick choices (one or
  several), `toggle_button` only for tool modes that stay active while used,
  `AngleDial` (via `float_param_angle`) for directions, the shared
  feathered range (`components::feathered_range`) for ranges with soft edges
  such as depth, luminance and Point Color, and `pattern_seed` (Shuffle) for
  random pattern seeds.
- Every dialog is a modal `moduwu_design::Dialog`: actions go in the footer of
  `show_with_footer` (primary action first), and it becomes a bottom sheet on
  narrow and touch portrait screens. Use `dialog_confirmation_actions` for
  confirm/cancel footers and `dialog_keyboard_action` for Escape and Enter.
  Keep initial focus requests one-time, and run keyboard fallback after
  controls process input. Backdrop clicks dismiss nothing unless a dialog opts
  in.
- Work that may run while the app stays usable (exports, Edit Replays, HDR
  merges, focus stacks) reports through the toolbar task indicator
  (`ui/background_tasks.rs`), not a dialog. Work that edits the open document,
  such as AI masks and denoise, keeps its blocking progress dialog.
- Report every failure the user should know about with
  `CalibRawApp::report_error(ErrorKind, message)` (`app/error_dialogs.rs`). It
  queues the shared error dialog, keeps the message in the status line and logs
  it, so no failure is visible only in the logs. Add an `ErrorKind` when no
  existing title fits. `ui.notice` alone is for successes, cancellations and
  precondition hints such as "Open a photo first". Only errors that offer
  recovery actions, such as the sidecar save failure, keep their own dialog.
- Route user tab navigation through `activate_tab`, and sidebar/tool changes
  through `AppAction`. Background document loading and batch operations contain
  documented exceptions: interactive tab activation can cancel their AI work.
- Keep serialized preference names stable when changing UI labels or helpers.
- Do not add descriptive text to editing panels, cards, tool rows or the
  toolbar: no explanatory paragraphs under headings and no hint lines below
  controls. When a control needs explaining, use a tooltip
  (`on_hover_text`, `*_with_help`), and only if its label, icon and behavior do
  not already make it clear. Controls that are dragged (sliders, range and
  depth controls, dials, colour fields, curve editors, on-image handles) never
  get a tooltip, because it would cover them while they are adjusted; put that
  help on a static label or card heading instead. Settings and the
  first-run setup may keep descriptions. Status lines (progress, empty states,
  "source set") and dialog text that explains a choice are not descriptions.

Run the independent library checks above for reusable design-system regressions
and the screenshot baselines (`cargo xtask baseline-run` and `baseline-compare`)
for rendered UI regressions. Review desktop and Android layouts in all six
themes when a change affects appearance.

### Feature tips

A tip is a dialog with a short looping clip and one or two sentences, shown the
first time a feature is used (`ui/tips.rs` holds the tips and the dialog,
`app/tips.rs` decides when one is offered). Every tip ships with a clip of the
real app, recorded with `demorec`, the demo recorder (a separate repository; the
maintainer's checkout is `/data/Documents/GitProjects/demo-recorder`). Asking for
a tip is asking for its recording too, so record the clip as part of the change.
A recording takes over the mouse and screen for about a minute: say so before
each launch.

1. Add a `Tip` variant and fill in the `match` arms the compiler points to. The
   key is saved in settings and never changes. The text says how to use the
   feature, not what it is.
2. Offer it from the handler of the feature, not from a view:
   `self.offer_tip(Tip::…)`, or a line in `mask_tip` for a mask kind.
3. Record the clip by following the "CalibRaw tip clips" section of the
   recorder's `AGENTS.md`: a scenario `examples/calibraw/tip-<name>.toml` started
   from `seed-tips`, on a staged copy of an approved Demo photo. A tip about a
   whole tab or panel keeps the full window; a tip about one tool is cropped to
   where it is used and trimmed to that use, with waits cut out. Rebuild the
   release binary first if the UI changed.
4. Encode it to `crates/calibraw-ui/assets/tips/<name>.webp` (animated WebP, at
   most 640 px wide, 1 MB and 12 s; the tests in `ui/tips.rs` enforce this), and
   check the encoded clip frame by frame.
5. Add the variant to `Tip::ALL` and its key to `keys_are_unique_and_stable`.

## Diagnostics and release helpers

`scripts/generate_licenses.sh` is the canonical reproducible wrapper around
cargo-about. It normalizes generated line endings and writes the ignored
`THIRD_PARTY_LICENSES.md` bundle used by release packaging. The script pins
cargo-about to 0.9.2; run `bash scripts/generate_licenses.sh` to generate the
bundle locally.

`cargo xtask colorchecker-wb-validate patches.csv` compares rendered and reference
D50 XYZ ColorChecker patches using CIEDE2000. The CSV requires `patch`,
`reference_x`, `reference_y`, `reference_z`, `rendered_x`, `rendered_y`, and
`rendered_z` columns; `illuminant`, `stage`, and `neutral` are optional.
Both sides must use the same D50 reference white and XYZ scale (Y=1 or Y=100).
Adapt values from other white points to D50 before comparing them.
`--json PATH` additionally writes machine-readable results, including per-patch
errors and summaries grouped by illuminant and stage.

Run `cargo test --locked -p xtask colorchecker` for the color-difference reference
check and CSV/report regression tests. These also run with the workspace test
suite in CI; Python is not required for this diagnostic.

`calibraw-wb-diagnostics` is an intentionally separate CLI binary for inspecting
camera white-balance coefficients and the camera-to-working matrix without
starting the UI. Build it with `cargo build -p calibraw-cli --bin calibraw-wb-diagnostics`,
then run `target/debug/calibraw-wb-diagnostics RAW [--dcp PROFILE]
[--temperature K] [--tint T]`.

The workspace crates are application-internal and explicitly set
`publish = false`; their package manifests intentionally retain repository
assets and build-time native contracts rather than pretending to be isolated
crates.

## RAW open and export performance

Use release builds for timings. The headless exporter now accepts `RUST_LOG`,
including per-program GPU compilation timings at debug level:

```sh
RUST_LOG=calibraw_core=info,calibraw_gpu=debug cargo run --release \
  -p calibraw-cli --bin calibraw-develop-export -- \
  --input photo.ARW --output photo.png
```

The UI assigns tile rendering 90% of export progress. With lens correction,
rotation, or cropping, the remaining phase also includes resampling the whole
image, final sharpening, color encoding, and file compression. Its duration
must be measured separately from tile rendering. Diagnostics report geometry
finalization and JPEG compression separately.

Final resampling, sharpening, and color conversion use ordered batches of 32
rows across CPU cores. Memory stays bounded and each pixel retains the serial
calculation order. PNG uses fast lossless compression; file size can increase.
GPU templates share deferred programs and compile the selected Bayer mode and
active denoise/CA paths. A newly enabled effect or mode may compile on first use.

Local measurements on a 7028x4688 Sony ARW, Ryzen 7 5700X3D and RTX 4070 Ti
(Linux/Vulkan, release build, separate processes without an application program
template; driver caches were not cleared):

| Operation | Before | After |
| --- | ---: | ---: |
| First 1920-pixel preview, including decode and proxy | 26.4 s | 5.2 s |
| Full-size 16-bit PNG, including GPU setup | 48.3 s | 7.1 s |
| Full-size JPEG with 2-degree rotation, after the last tile | 12.1 s | 2.3 s |

The rotated JPEG files were byte-identical. Decoded RGBA16 PNG samples were
identical; the fast-compressed PNG was about 5.4% larger. These are single-image
local measurements, not a cross-platform comparison.

Regression coverage is in `calibraw-gpu`: serial versus parallel export rows,
batch boundaries, lens/rotation resampling, cancellation and writer errors,
deferred program reuse, and specialized versus dynamic Bayer shader output
for every mode with denoise and CA enabled and disabled.

## Dependency duplicates

`cargo deny check bans` is reviewed with all supported target triples. Some
duplicate versions are unavoidable: desktop and Android graphics stacks,
Wayland/winit platform adapters, bindgen's parser toolchain, and framework
transitive dependencies have incompatible version requirements. Workspace
direct dependencies are kept on the versions used by the application, and no
dependency is upgraded solely to silence a duplicate-version warning.

## Experimental iOS

An unsupported prototype: the Android mobile UI on Metal, with a sandboxed
platform layer in `crates/calibraw-ui/src/platform/ios*.{rs,m}`.

- **Library.** Files and Photos pickers copy photos into `Documents/CalibRaw`
  (exposed in the Files app and USB file sharing). Library IDs are paths
  relative to that folder, so they survive container moves. Imports never
  overwrite. Photos uses the out-of-process picker without read permission and
  prefers RAW representations where Photos offers them.
- **Exports** are saved to Photos with Add Photos Only permission
  (`NSPhotoLibraryAddUsageDescription`); a denied or failed save reports an
  error rather than falling back to Files.
- **AI** links the pinned ONNX Runtime XCFramework and runs on the constrained
  CPU configuration; CoreML is not enabled. Device builds request the Increased
  Memory Limit entitlement, and BiRefNet is refused before loading when its CPU
  peak does not fit `os_proc_available_memory()`.
- **Not implemented:** Lensfun, camera-profile folders, CoreML, replay
  encoding and background-task integration.

Requirements: macOS, full Xcode with an iOS simulator runtime, Python 3,
autoconf, automake, GNU libtool and pkg-config, and rustup's cargo/rustc on
PATH (Homebrew's keg-only rustup: prepend `$(brew --prefix rustup)/bin`).
`scripts/build_ios.py` builds the pinned LibRaw for the target, downloads and
verifies the pinned ONNX Runtime, then bundles and signs the app:

```sh
# Simulator (Intel Macs: x86_64-apple-ios). --device <UDID> picks a simulator,
# --no-run only builds, --release optimizes.
rustup target add aarch64-apple-ios-sim
python3 scripts/build_ios.py

# Physical device with Developer Mode, paired in Xcode (xcrun devicectl list devices).
rustup target add aarch64-apple-ios
python3 scripts/build_ios.py --physical-device <UDID> --team <TEAM_ID>

# Unsigned ARM64 IPA for re-signing (e.g. AltStore); no device or team needed.
python3 scripts/build_ios.py --release --ipa
```

Device builds use `packaging/ios/CalibRaw.xcodeproj` for automatic development
signing and install `de.duecki.calibraw.ios-dev`; free accounts allow three
development apps with short-lived profiles. The IPA is written to
`target/ios-ipa/release/CalibRaw-ios-arm64.ipa`; the Build iOS workflow uploads
it and tagged releases attach it.

Tests: `python3 -m unittest discover -s scripts/tests -p 'test_build_ios.py'`
checks target selection without Xcode. `packaging/ios/tests/picker_lifecycle.m`
checks that the native pickers complete exactly once; run it on a booted Apple
Silicon simulator:

```sh
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
xcrun --sdk iphonesimulator clang -target arm64-apple-ios17.0-simulator \
  -isysroot "$sdk" -fobjc-arc -Werror packaging/ios/tests/picker_lifecycle.m \
  -framework UIKit -framework Foundation -framework CoreGraphics \
  -framework UniformTypeIdentifiers -framework Photos -framework PhotosUI \
  -o target/picker-lifecycle-test
xcrun simctl spawn booted "$PWD/target/picker-lifecycle-test"
```

## Android Build & Signing

### Requirements

- Rust target `aarch64-linux-android`
- JDK 17, Python 3, `libclang`, `make`, and `pkg-config`
- Android SDK, Build Tools, and NDK versions from `Cargo.toml`
- CMake 3.22.1 with Ninja and `cargo-ndk` 4.1.2

```sh
export ANDROID_SDK_ROOT="$HOME/Android/Sdk"
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
```

Set `LIBCLANG_PATH` when `libclang.so` is outside the system search path.

### Build

```sh
./gradlew assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

Gradle builds pinned LibRaw and Lensfun dependencies and packages the APK at
`android/app/build/outputs/apk/debug/app-debug.apk`. To build one ABI through
the compatibility helper, use `cargo xtask build-android arm64-v8a release`.

Depth masks, fog and Relight on every platform share a pinned Depth Anything
V2 Small FP32 model (~99 MB), run at 518 × 518. Android runs it on CPU with two
inference threads and weight prepacking disabled. Its inverse-depth
predictions are converted to the shared near=0, far=255 convention.

The app supports 16 KB pages. Verify a built APK with:

```sh
cargo xtask verify-android-16kb android/app/build/outputs/apk/debug/app-debug.apk
```

### Profiling on a device

`./gradlew assembleProfiling` builds
`android/app/build/outputs/apk/profiling/app-profiling.apk`: release Rust and
native libraries from the working tree (`cargo xtask build-android ABI
profiling`), signed with the debug key and `profileable` by the shell, so
simpleperf, heapprofd and `dumpsys` can attach. It installs as
`de.duecki.calibraw.profiling` ("CalibRaw Profiling") beside the release app
and keeps its own data.

GPU stage timing writes a timestamp around every compute pass of the editor's
preview stages and logs one diagnostics line per stage once the GPU ran it:
`GPU stage Output 652x978: 54.21 ms in 3 passes (...)`. Stage totals are
reliable; the split between passes is not, because GPUs overlap dispatches.
It needs the adapter's `TIMESTAMP_QUERY` and costs one atomic load per pass
when off. Turn it on with `CALIBRAW_GPU_STAGE_TIMING=1` on the desktop, or by
launching Android with the extra
`--ez de.duecki.calibraw.GPU_STAGE_TIMING true`.

While a control is dragged, a slow GPU shows a half-size preview that the full one replaces on release, fitted or zoomed in (`interactive_preview.rs`). The editor chooses by timing the batches of the preview that is shown; on the desktop `CALIBRAW_INTERACTIVE_PREVIEW=always` or `off` forces either path to compare them. After every control drag the diagnostics line `Control drag: …` (`Control drag (zoomed in): …` when zoomed) logs how many GPU batches each preview ran and their median times, which is how to compare the two paths on a device.

`cargo xtask android-bench LABEL` measures the connected device. It installs
the profiling APK (`--apk PATH`, or `--no-install`), copies the scenario's
photo from `CALIBRAW_BENCH_PHOTOS` (checked against its SHA-256) and its
sidecar into the app's library, launches it with stage timing on and plays
the scenario's steps through `adb shell input`. For each step it records
SurfaceFlinger timestats (frames, present-to-present and post-to-acquire
percentiles), the GPU stage times and how often each stage ran, RSS and
`MemAvailable` sampled every second, memory after the step (`/proc`,
`dumpsys meminfo`, `dumpsys gpu --gpumem`), the app's heap purges and every
low-memory-killer kill. The report, `android-bench.json`, and the device log
of the run go to a new `$CALIBRAW_BASELINE_DIR/<date>-<revision>-LABEL`
folder; the app is stopped afterwards.

```sh
./gradlew assembleProfiling
CALIBRAW_BENCH_PHOTOS=/path/to/Demo cargo xtask android-bench before
cargo xtask android-bench-compare "$CALIBRAW_BASELINE_DIR"/<before> "$CALIBRAW_BASELINE_DIR"/<after>
```

The default scenario, `xtask/android-bench/tab-m11.json`, holds the Lenovo Tab
M11's coordinates in landscape (`--scenario` picks another; `--package`
another installed build). Its taps open the first library tile, so the
profiling app's library must hold only the scenario's photo, and the app
must have been opened once, with onboarding finished and update checks
declined. Its last steps run every AI job (Sky, Depth, Object, Remove,
Subject; each waits for the `AI job … finished` diagnostics line), so the
profiling app must also have downloaded every model, which it keeps in its
private storage, and dismissed the Mask tip; run each job by hand once. The
Subject job runs last because the system may kill the app during it, which
ends the run. The bench refuses a display whose current size differs from
the scenario's.

Scenario steps list measured `actions` and unmeasured `then` actions (undo,
opening a panel section): `launch`, `tap`, `double_tap`, `swipe`, `sleep_ms`,
`wait_log` (a line the app logs) and `screenshot`, which saves
`screens/<step>-<name>.png` in the run folder for checking coordinates.
Coordinates must not depend on a scroll position: `adb shell input` swipes
scroll a different distance from run to run, so the scenario reaches lower
controls by collapsing the sections above them, from the top of the panel.
Each launch starts with only Light expanded.
`android-bench-compare BEFORE AFTER` prints every figure of both runs side by
side, matched by step and figure name.

### GitHub release signing

Direct pushes to `main` and `v*` tags published through the **Release** workflow
build the same signed release APK. Pull requests, pushes to other branches, and
manual runs build a debug APK and do not access the release-signing secrets.
Signed APKs use `1000000 + git rev-list --count HEAD` as their Android version
code, so a release tag on a main-branch commit has the same code as that commit's
main build. This also keeps new signed APKs above the older run-number-based
builds, allowing Android to install them as updates when the signing key matches.

Create the upload keystore once and keep it backed up securely. Losing it means
future APK updates cannot be signed with the same identity.

```sh
keytool -genkeypair \
  -keystore calibraw-release.keystore \
  -storetype PKCS12 \
  -alias calibraw \
  -keyalg RSA \
  -keysize 4096 \
  -validity 10000
openssl base64 -A -in calibraw-release.keystore > calibraw-release.keystore.base64
```

In the GitHub repository, open **Settings > Secrets and variables > Actions**,
choose **New repository secret**, and add:

- `CALIBRAW_ANDROID_KEYSTORE_BASE64`: contents of
  `calibraw-release.keystore.base64`
- `CALIBRAW_ANDROID_STORE_PASSWORD`: the keystore password
- `CALIBRAW_ANDROID_KEY_ALIAS`: the alias passed to `keytool` (`calibraw` above)
- `CALIBRAW_ANDROID_KEY_PASSWORD`: the key password (use the keystore password
  for the PKCS12 keystore generated above)

The release job fails instead of creating an unsigned release when any secret
is absent or invalid. Its artifact is named
`calibraw-android-release-arm64-v8a` and contains the APK plus its SHA-256 file.

### Storage

Imports use Android's document picker and are copied to
`Android/media/de.duecki.calibraw/.library`; matching `.calibraw` sidecars remain next
to each RAW. Legacy layouts are migrated only after a successful copy. Exports
are published to `Pictures/CalibRaw` through MediaStore where available;
MediaStore numbers a name that is already taken, and the reported location is
read back from it.


Long-running operations require the app to remain open. Android 13 and newer
may request notification permission for progress updates.
